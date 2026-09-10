-- Resolve immutable selection inside the existing row-locked formation operations.
-- These are operational seat constraints, never edits to the retained Listing.
create or replace function platform_store.selected_listing_seats_v2(
  p_listing platform_store.activity_listing_revisions,
  p_input bytea
) returns jsonb
language plpgsql immutable security invoker set search_path = ''
as $$
declare
  document jsonb := convert_from(p_listing.canonical_document, 'utf8')::jsonb;
  input jsonb := convert_from(p_input, 'utf8')::jsonb;
  option jsonb;
  seats jsonb;
begin
  if document #>> '{launch_input_schema,schema}' is distinct from 'worldstream/launch-input-schema/v2' then
    if input <> '{}'::jsonb then
      raise exception using errcode = '22023', message = 'invalid_launch_input';
    end if;
    return p_listing.seat_templates;
  end if;
  if document #>> '{launch_input_schema,accepts}' <> 'roster_option'
    or jsonb_typeof(input) <> 'object' or input - 'roster_option' <> '{}'::jsonb
    or jsonb_typeof(input -> 'roster_option') is distinct from 'string' then
    raise exception using errcode = '22023', message = 'invalid_launch_input';
  end if;
  select value into option
  from jsonb_array_elements(document #> '{launch_input_schema,roster_options}')
  where value ->> 'option_id' = input ->> 'roster_option';
  if option is null then
    raise exception using errcode = '22023', message = 'unsupported_roster_option';
  end if;
  if jsonb_array_length(option -> 'house_agent_assignments') > 2 or exists (
    select 1 from jsonb_array_elements(option -> 'house_agent_assignments') assignment
    where not ((option -> 'seat_ids') ? (assignment ->> 'seat_id')) or not exists (
      select 1 from jsonb_array_elements(p_listing.seat_templates) seat
      where seat ->> 'seat_id' = assignment ->> 'seat_id'
        and (seat -> 'allowed_participation') ? 'house_agent_fill'
        and (seat -> 'allowed_house_agent_revisions') ? (assignment ->> 'house_agent_revision_digest'))
  ) then
    raise exception using errcode = '22023', message = 'invalid_roster_option';
  end if;
  -- Selected seats must all be resolved before freezing. A pinned supplied seat
  -- cannot be claimed by an account; a non-supplied seat cannot be randomly filled.
  select jsonb_agg(seat.value || jsonb_build_object(
    'required', true,
    'allowed_participation', case when assignment.value is not null then '["house_agent_fill"]'::jsonb
      else (seat.value -> 'allowed_participation') - 'house_agent_fill' end,
    'allowed_house_agent_revisions', case when assignment.value is not null
      then jsonb_build_array(assignment.value ->> 'house_agent_revision_digest') else '[]'::jsonb end
  ) order by seat.ordinality) into seats
  from jsonb_array_elements(p_listing.seat_templates) with ordinality seat(value, ordinality)
  left join lateral (
    select value from jsonb_array_elements(option -> 'house_agent_assignments')
    where value ->> 'seat_id' = seat.value ->> 'seat_id'
  ) assignment on true
  where (option -> 'seat_ids') ? (seat.value ->> 'seat_id');
  if seats is null or jsonb_array_length(seats) <> jsonb_array_length(option -> 'seat_ids')
    or not platform_store.valid_listing_seats_v1(seats)
    or exists (select 1 from jsonb_array_elements(p_listing.seat_templates) seat
      where (seat ->> 'required')::boolean and not ((option -> 'seat_ids') ? (seat ->> 'seat_id'))) then
    raise exception using errcode = '22023', message = 'invalid_roster_option';
  end if;
  return seats;
end;
$$;
revoke execute on function platform_store.selected_listing_seats_v2(platform_store.activity_listing_revisions, bytea)
  from public, anon, authenticated;
grant execute on function platform_store.selected_listing_seats_v2(platform_store.activity_listing_revisions, bytea)
  to service_role;

create or replace function platform_api.create_launch_request_v1(
  p_creator_account_id uuid,
  p_listing_revision_digest text,
  p_idempotency_namespace text,
  p_idempotency_key_digest bytea,
  p_canonical_launch_input bytea,
  p_launch_input_digest bytea,
  p_canonicalizer_version text,
  p_house_fill_choice text,
  p_creator_access_choice text,
  p_creator_seat_id text,
  p_creator_participation_kind text
)
returns table(
  launch_request_id uuid,
  launch_state text,
  expires_at timestamptz,
  was_created boolean
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  account_row platform_store.platform_accounts%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  existing platform_store.launch_requests%rowtype;
  creator_seat jsonb;
  sampled_at timestamptz := clock_timestamp();
  new_launch_id uuid := gen_random_uuid();
  parsed_input jsonb;
begin
  if p_creator_account_id is null
    or p_listing_revision_digest !~ '^blake3:[0-9a-f]{64}$'
    or length(p_idempotency_namespace) not between 1 and 64
    or p_idempotency_namespace !~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
    or octet_length(p_idempotency_key_digest) <> 32
    or octet_length(p_canonical_launch_input) not between 2 and 16384
    or octet_length(p_launch_input_digest) <> 32
    or p_launch_input_digest <> extensions.digest(p_canonical_launch_input, 'sha256')
    or p_canonicalizer_version <> 'worldstream/canonical-json/v1'
    or p_house_fill_choice not in ('disabled', 'fill_unclaimed')
    or p_creator_access_choice not in ('seat', 'spectator')
    or p_creator_participation_kind not in ('account_human', 'account_external_agent')
  then
    raise exception using errcode = '22023', message = 'invalid_launch_request';
  end if;

  begin
    parsed_input := convert_from(p_canonical_launch_input, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_launch_input';
  end;
  if jsonb_typeof(parsed_input) <> 'object' then
    raise exception using errcode = '22023', message = 'invalid_launch_input';
  end if;

  select accounts.*
  into account_row
  from platform_store.platform_accounts accounts
  where accounts.account_id = p_creator_account_id
  for update;
  if not found or account_row.erased_at is not null then
    raise exception using errcode = '55000', message = 'account_unavailable';
  end if;

  select listings.*
  into listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = p_listing_revision_digest;
  if not found then
    raise exception using errcode = '55000', message = 'listing_unavailable';
  end if;

  select launches.*
  into existing
  from platform_store.launch_requests launches
  where launches.creator_account_id = p_creator_account_id
    and launches.idempotency_namespace = p_idempotency_namespace
    and launches.idempotency_key_digest = p_idempotency_key_digest;
  if found then
    if existing.listing_revision_digest <> p_listing_revision_digest
      or existing.canonical_launch_input <> p_canonical_launch_input
      or existing.launch_input_digest <> p_launch_input_digest
      or existing.canonicalizer_version <> p_canonicalizer_version
      or existing.house_fill_choice <> p_house_fill_choice
      or existing.creator_access_choice <> p_creator_access_choice
      or existing.creator_seat_id is distinct from p_creator_seat_id
      or existing.creator_participation_kind is distinct from p_creator_participation_kind
    then
      raise exception using errcode = '23505', message = 'launch_idempotency_conflict';
    end if;
    return query select existing.launch_request_id, existing.state, existing.expires_at, false;
    return;
  end if;

  perform platform_store.selected_listing_seats_v2(listing_row, p_canonical_launch_input);
  if convert_from(listing_row.canonical_document, 'utf8')::jsonb #>> '{launch_input_schema,schema}' = 'worldstream/launch-input-schema/v2'
    and (exists (select 1 from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, p_canonical_launch_input)) seat
      where (seat -> 'allowed_participation') ? 'house_agent_fill')) <> (p_house_fill_choice = 'fill_unclaimed') then
    raise exception using errcode = '22023', message = 'roster_fill_choice_mismatch';
  end if;

  if exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.controlling_account_id = p_creator_account_id
      and reservations.kind = 'pre_genesis'
      and reservations.reservation_class = 'authorized'
      and reservations.released_at is null
  ) then
    raise exception using errcode = '55000', message = 'pre_genesis_capacity_unavailable';
  end if;

  if p_house_fill_choice = 'fill_unclaimed'
    and not exists (
      select 1
      from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, p_canonical_launch_input)) seats(value)
      where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
    )
  then
    raise exception using errcode = '22023', message = 'house_fill_not_allowed';
  end if;

  if listing_row.creator_access = 'must_claim_seat'
    and p_creator_access_choice <> 'seat'
  then
    raise exception using errcode = '22023', message = 'creator_must_claim_seat';
  end if;
  if p_creator_access_choice = 'spectator' then
    if listing_row.creator_access <> 'may_spectate'
      or p_creator_seat_id is not null
      or p_creator_participation_kind <> 'account_human'
    then
      raise exception using errcode = '22023', message = 'creator_spectator_not_allowed';
    end if;
  else
    if p_creator_seat_id is null then
      raise exception using errcode = '22023', message = 'creator_seat_required';
    end if;
    select seats.value
    into creator_seat
    from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, p_canonical_launch_input)) seats(value)
    where seats.value ->> 'seat_id' = p_creator_seat_id;
    if not found
      or not ((creator_seat -> 'allowed_participation') ? p_creator_participation_kind)
    then
      raise exception using errcode = '22023', message = 'creator_seat_not_allowed';
    end if;
  end if;

  insert into platform_store.launch_requests (
    launch_request_id,
    creator_account_id,
    listing_revision_digest,
    idempotency_namespace,
    idempotency_key_digest,
    canonical_launch_input,
    launch_input_digest,
    canonicalizer_version,
    house_fill_choice,
    creator_access_choice,
    creator_seat_id,
    creator_participation_kind,
    state,
    expires_at,
    created_at,
    last_transition_at
  ) values (
    new_launch_id,
    p_creator_account_id,
    p_listing_revision_digest,
    p_idempotency_namespace,
    p_idempotency_key_digest,
    p_canonical_launch_input,
    p_launch_input_digest,
    p_canonicalizer_version,
    p_house_fill_choice,
    p_creator_access_choice,
    p_creator_seat_id,
    p_creator_participation_kind,
    'collecting_roster',
    sampled_at + interval '24 hours',
    sampled_at,
    sampled_at
  );

  insert into platform_store.capacity_reservations (
    launch_request_id,
    kind,
    reservation_class,
    controlling_account_id,
    reserved_at
  ) values (
    new_launch_id,
    'pre_genesis',
    'authorized',
    p_creator_account_id,
    sampled_at
  );

  if p_creator_access_choice = 'seat' then
    insert into platform_store.seat_claims (
      launch_request_id,
      seat_id,
      role,
      controlling_account_id,
      participation_kind,
      principal_reference,
      claimed_at
    ) values (
      new_launch_id,
      p_creator_seat_id,
      creator_seat ->> 'role',
      p_creator_account_id,
      p_creator_participation_kind,
      'seat:' || p_creator_seat_id,
      sampled_at
    );
  end if;

  return query
  select new_launch_id, 'collecting_roster'::text, sampled_at + interval '24 hours', true;
end;
$$;
create or replace function platform_api.read_launch_request_v1(
  p_requesting_account_id uuid,
  p_launch_request_id uuid
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  seats jsonb;
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id;
  if not found or not (
    launch_row.creator_account_id = p_requesting_account_id
    or exists (
      select 1
      from platform_store.seat_claims claims
      where claims.launch_request_id = p_launch_request_id
        and claims.controlling_account_id = p_requesting_account_id
        and claims.released_at is null
    )
  ) then
    return null;
  end if;

  select listings.*
  into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  select jsonb_agg(
    jsonb_build_object(
      'seat_id', template.value ->> 'seat_id',
      'role', template.value ->> 'role',
      'display_name', template.value ->> 'display_name',
      'required', (template.value ->> 'required')::boolean,
      'claimed', claims.claim_id is not null,
      'claimed_by_requester', coalesce(
        claims.controlling_account_id = p_requesting_account_id,
        false
      ),
      'participation_kind', claims.participation_kind
    ) order by template.ordinality
  )
  into seats
  from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input))
       with ordinality as template(value, ordinality)
  left join platform_store.seat_claims claims
    on claims.launch_request_id = p_launch_request_id
   and claims.seat_id = template.value ->> 'seat_id'
   and claims.released_at is null;

  return jsonb_build_object(
    'version', 'platform_launch_request.v1',
    'launch_request_id', launch_row.launch_request_id,
    'listing_revision_digest', launch_row.listing_revision_digest,
    'state', launch_row.state,
    'expires_at', launch_row.expires_at,
    'creator_access_choice', launch_row.creator_access_choice,
    'creator_seat_id', launch_row.creator_seat_id,
    'house_fill_choice', launch_row.house_fill_choice,
    'roster_frozen', launch_row.roster_frozen_at is not null,
    'can_manage', launch_row.creator_account_id = p_requesting_account_id,
    'seats', coalesce(seats, '[]'::jsonb)
  );
end;
$$;

create or replace function platform_api.rotate_seat_invitation_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid,
  p_seat_id text
)
returns table(
  seat_id text,
  generation integer,
  invitation_token text,
  expires_at timestamptz
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_seats jsonb;
  next_generation integer;
  raw_token text;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found or launch_row.creator_account_id <> p_creator_account_id then
    raise exception using errcode = '55000', message = 'launch_unavailable';
  end if;
  if launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.expires_at <= sampled_at
  then
    raise exception using errcode = '55000', message = 'launch_not_collecting';
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  where accounts.account_id = p_creator_account_id
    and accounts.erased_at is null
  for update;
  if not found then
    raise exception using errcode = '55000', message = 'account_unavailable';
  end if;

  select platform_store.selected_listing_seats_v2(listings, launch_row.canonical_launch_input)
  into listing_seats
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;
  if not exists (
    select 1
    from jsonb_array_elements(listing_seats) seats(value)
    where seats.value ->> 'seat_id' = p_seat_id
      and ((seats.value -> 'allowed_participation') ? 'account_human'
        or (seats.value -> 'allowed_participation') ? 'account_external_agent')
  ) then
    raise exception using errcode = '22023', message = 'seat_unavailable';
  end if;
  if exists (
    select 1
    from platform_store.seat_claims claims
    where claims.launch_request_id = p_launch_request_id
      and claims.seat_id = p_seat_id
      and claims.released_at is null
  ) then
    raise exception using errcode = '55000', message = 'seat_already_claimed';
  end if;

  perform invitations.invitation_id
  from platform_store.seat_invitations invitations
  where invitations.launch_request_id = p_launch_request_id
    and invitations.seat_id = p_seat_id
  order by invitations.generation
  for update;

  select coalesce(max(invitations.generation), 0) + 1
  into next_generation
  from platform_store.seat_invitations invitations
  where invitations.launch_request_id = p_launch_request_id
    and invitations.seat_id = p_seat_id;
  if next_generation > 2147483647 then
    raise exception using errcode = '54000', message = 'invitation_generation_exhausted';
  end if;

  update platform_store.seat_invitations invitations
  set revoked_at = sampled_at
  where invitations.launch_request_id = p_launch_request_id
    and invitations.seat_id = p_seat_id
    and invitations.consumed_at is null
    and invitations.revoked_at is null;

  raw_token := encode(extensions.gen_random_bytes(32), 'hex');
  insert into platform_store.seat_invitations (
    launch_request_id,
    seat_id,
    generation,
    token_digest,
    expires_at,
    created_at
  ) values (
    p_launch_request_id,
    p_seat_id,
    next_generation,
    extensions.digest(convert_to(raw_token, 'utf8'), 'sha256'),
    launch_row.expires_at,
    sampled_at
  );

  return query select p_seat_id, next_generation, raw_token, launch_row.expires_at;
end;
$$;

create or replace function platform_api.claim_invited_seat_v1(
  p_claiming_account_id uuid,
  p_invitation_token_digest bytea,
  p_participation_kind text
)
returns table(
  launch_request_id uuid,
  seat_id text,
  role text,
  participation_kind text,
  claimed_at timestamptz
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  selected_launch_id uuid;
  launch_row platform_store.launch_requests%rowtype;
  invitation_row platform_store.seat_invitations%rowtype;
  account_row platform_store.platform_accounts%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  selected_seat jsonb;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_claiming_account_id is null
    or octet_length(p_invitation_token_digest) <> 32
    or p_participation_kind not in ('account_human', 'account_external_agent')
  then
    raise exception using errcode = '22023', message = 'invalid_seat_claim';
  end if;

  select invitations.launch_request_id
  into selected_launch_id
  from platform_store.seat_invitations invitations
  where invitations.token_digest = p_invitation_token_digest;
  if not found then
    raise exception using errcode = '55000', message = 'invitation_unavailable';
  end if;

  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = selected_launch_id
  for update;
  if not found
    or launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.expires_at <= sampled_at
  then
    raise exception using errcode = '55000', message = 'invitation_unavailable';
  end if;

  select accounts.*
  into account_row
  from platform_store.platform_accounts accounts
  where accounts.account_id = p_claiming_account_id
  for update;
  if not found or account_row.erased_at is not null then
    raise exception using errcode = '55000', message = 'account_unavailable';
  end if;

  select invitations.*
  into invitation_row
  from platform_store.seat_invitations invitations
  where invitations.token_digest = p_invitation_token_digest
    and invitations.launch_request_id = selected_launch_id
  for update;
  if not found
    or invitation_row.consumed_at is not null
    or invitation_row.revoked_at is not null
    or invitation_row.expires_at <= sampled_at
    or invitation_row.expires_at <> launch_row.expires_at
  then
    raise exception using errcode = '55000', message = 'invitation_unavailable';
  end if;

  select listings.*
  into listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;
  select seats.value
  into selected_seat
  from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
  where seats.value ->> 'seat_id' = invitation_row.seat_id;
  if not found or not ((selected_seat -> 'allowed_participation') ? p_participation_kind) then
    raise exception using errcode = '55000', message = 'invitation_unavailable';
  end if;

  if p_claiming_account_id = launch_row.creator_account_id and (
    launch_row.creator_access_choice = 'spectator'
    or launch_row.creator_seat_id <> invitation_row.seat_id
  ) then
    raise exception using errcode = '55000', message = 'creator_choice_conflict';
  end if;

  if not listing_row.allow_multiple_seats_per_account and exists (
    select 1
    from platform_store.seat_claims claims
    where claims.launch_request_id = selected_launch_id
      and claims.controlling_account_id = p_claiming_account_id
      and claims.released_at is null
  ) then
    raise exception using errcode = '55000', message = 'account_seat_limit';
  end if;

  insert into platform_store.seat_claims (
    launch_request_id,
    seat_id,
    role,
    controlling_account_id,
    participation_kind,
    principal_reference,
    claimed_at
  ) values (
    selected_launch_id,
    invitation_row.seat_id,
    selected_seat ->> 'role',
    p_claiming_account_id,
    p_participation_kind,
    'seat:' || invitation_row.seat_id,
    sampled_at
  );

  update platform_store.seat_invitations invitations
  set consumed_at = sampled_at
  where invitations.invitation_id = invitation_row.invitation_id;

  return query select
    selected_launch_id,
    invitation_row.seat_id,
    selected_seat ->> 'role',
    p_participation_kind,
    sampled_at;
end;
$$;

create or replace function platform_api.freeze_launch_request_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid,
  p_frozen_roster bytea,
  p_frozen_roster_digest bytea,
  p_frozen_room_setup_specification bytea,
  p_frozen_room_setup_specification_digest text,
  p_host_installation_id text,
  p_room_setup_operation_id text
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  parsed_roster jsonb;
  parsed_setup jsonb;
  frozen_member_count integer;
  sampled_at timestamptz := clock_timestamp();
begin
  if octet_length(p_frozen_roster) not between 2 and 65536
    or octet_length(p_frozen_roster_digest) <> 32
    or p_frozen_roster_digest <> extensions.digest(p_frozen_roster, 'sha256')
    or octet_length(p_frozen_room_setup_specification) not between 2 and 65536
    or p_frozen_room_setup_specification_digest !~ '^blake3:[0-9a-f]{64}$'
    or p_host_installation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or p_room_setup_operation_id !~ '^[a-z][a-z0-9-]{0,63}$'
  then
    raise exception using errcode = '22023', message = 'invalid_frozen_launch';
  end if;

  begin
    parsed_roster := convert_from(p_frozen_roster, 'utf8')::jsonb;
    parsed_setup := convert_from(p_frozen_room_setup_specification, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_frozen_launch';
  end;

  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found or launch_row.creator_account_id <> p_creator_account_id then
    return false;
  end if;

  if launch_row.roster_frozen_at is not null then
    if launch_row.frozen_roster = p_frozen_roster
      and launch_row.frozen_roster_digest = p_frozen_roster_digest
      and launch_row.frozen_room_setup_specification = p_frozen_room_setup_specification
      and launch_row.frozen_room_setup_specification_digest = p_frozen_room_setup_specification_digest
      and launch_row.host_installation_id = p_host_installation_id
      and launch_row.room_setup_operation_id = p_room_setup_operation_id
    then
      return true;
    end if;
    raise exception using errcode = '23505', message = 'launch_freeze_conflict';
  end if;

  if launch_row.state <> 'collecting_roster' or launch_row.expires_at <= sampled_at then
    raise exception using errcode = '55000', message = 'launch_not_collecting';
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  join (
    select launch_row.creator_account_id as account_id
    union
    select claims.controlling_account_id
    from platform_store.seat_claims claims
    where claims.launch_request_id = p_launch_request_id
      and claims.released_at is null
  ) controlling on controlling.account_id = accounts.account_id
  order by accounts.account_id
  for update of accounts;
  if exists (
    select 1
    from platform_store.platform_accounts accounts
    join (
      select launch_row.creator_account_id as account_id
      union
      select claims.controlling_account_id
      from platform_store.seat_claims claims
      where claims.launch_request_id = p_launch_request_id
        and claims.released_at is null
    ) controlling on controlling.account_id = accounts.account_id
    where accounts.erased_at is not null
  ) then
    raise exception using errcode = '55000', message = 'account_unavailable';
  end if;

  select listings.*
  into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  if launch_row.house_fill_choice = 'fill_unclaimed' and not exists (
    select 1
    from platform_store.house_fill_operations operations
    where operations.launch_request_id = p_launch_request_id
      and operations.state = 'assignments_complete'
  ) then
    raise exception using errcode = '55000', message = 'house_fill_incomplete';
  end if;
  if launch_row.house_fill_choice = 'disabled' and exists (
    select 1
    from platform_store.house_agent_assignments assignments
    where assignments.launch_request_id = p_launch_request_id
  ) then
    raise exception using errcode = '55000', message = 'unexpected_house_assignment';
  end if;
  if exists (
    select 1
    from platform_store.house_agent_assignments assignments
    where assignments.launch_request_id = p_launch_request_id
      and (
        assignments.listing_revision_digest <> launch_row.listing_revision_digest
        or not exists (
          select 1
          from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
          where seats.value ->> 'seat_id' = assignments.seat_id
            and (seats.value -> 'allowed_participation') ? 'house_agent_fill'
            and (seats.value -> 'allowed_house_agent_revisions')
              ? assignments.house_agent_revision_digest
        )
      )
  ) then
    raise exception using errcode = '55000', message = 'house_assignment_not_allowed';
  end if;

  if exists (
    select 1
    from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
    where (seats.value ->> 'required')::boolean
      and not exists (
        select 1
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = seats.value ->> 'seat_id'
          and claims.released_at is null
      )
      and not exists (
        select 1
        from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.seat_id = seats.value ->> 'seat_id'
      )
  ) then
    raise exception using errcode = '55000', message = 'required_seat_unfilled';
  end if;

  if launch_row.creator_access_choice = 'seat' and not exists (
    select 1
    from platform_store.seat_claims claims
    where claims.launch_request_id = p_launch_request_id
      and claims.seat_id = launch_row.creator_seat_id
      and claims.controlling_account_id = launch_row.creator_account_id
      and claims.released_at is null
  ) then
    raise exception using errcode = '55000', message = 'creator_seat_unfilled';
  end if;

  select
    (select count(*)
     from platform_store.seat_claims claims
     where claims.launch_request_id = p_launch_request_id
       and claims.released_at is null)
    +
    (select count(*)
     from platform_store.house_agent_assignments assignments
     where assignments.launch_request_id = p_launch_request_id)
  into frozen_member_count;

  if jsonb_typeof(parsed_roster) <> 'object'
    or parsed_roster ->> 'schema' <> 'worldstream/frozen-roster/v1'
    or parsed_roster ->> 'listing_revision_digest' <> launch_row.listing_revision_digest
    or jsonb_typeof(parsed_roster -> 'members') <> 'array'
    or jsonb_array_length(parsed_roster -> 'members') <> frozen_member_count
    or (
      select count(*) <> count(distinct member ->> 'seat_id')
      from jsonb_array_elements(parsed_roster -> 'members') members(member)
    )
    or (
      select count(*) <> count(distinct member ->> 'principal_reference')
      from jsonb_array_elements(parsed_roster -> 'members') members(member)
    )
    or exists (
      select 1
      from platform_store.seat_claims claims
      where claims.launch_request_id = p_launch_request_id
        and claims.released_at is null
        and not exists (
          select 1
          from jsonb_array_elements(parsed_roster -> 'members') members(member)
          where member ->> 'seat_id' = claims.seat_id
            and member ->> 'participation' = claims.participation_kind
            and member ->> 'principal_reference' = claims.principal_reference
        )
    )
    or exists (
      select 1
      from platform_store.house_agent_assignments assignments
      join platform_store.house_agent_revisions revisions
        on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
      where assignments.launch_request_id = p_launch_request_id
        and not exists (
          select 1
          from jsonb_array_elements(parsed_roster -> 'members') members(member)
          where member ->> 'seat_id' = assignments.seat_id
            and member ->> 'participation' = 'house_agent_fill'
            and member ->> 'principal_reference' = assignments.setup_principal_reference
            and member ->> 'display_name' = revisions.display_name
            and member ->> 'house_agent_revision_digest'
              = assignments.house_agent_revision_digest
            and member #>> '{agent_profile,profile_id}' = assignments.agent_profile_id
            and member #>> '{agent_profile,revision}' = assignments.agent_profile_revision
            and member #>> '{runner_template,template_id}' = assignments.runner_template_id
            and member #>> '{runner_template,revision}' = assignments.runner_template_revision
        )
    )
    or exists (
      select 1
      from jsonb_array_elements(parsed_roster -> 'members') members(member)
      where not exists (
        select 1
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.released_at is null
          and claims.seat_id = member ->> 'seat_id'
          and claims.participation_kind = member ->> 'participation'
          and claims.principal_reference = member ->> 'principal_reference'
      )
      and not exists (
        select 1
        from platform_store.house_agent_assignments assignments
        join platform_store.house_agent_revisions revisions
          on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
        where assignments.launch_request_id = p_launch_request_id
          and assignments.seat_id = member ->> 'seat_id'
          and member ->> 'participation' = 'house_agent_fill'
          and assignments.setup_principal_reference = member ->> 'principal_reference'
          and revisions.display_name = member ->> 'display_name'
          and assignments.house_agent_revision_digest
            = member ->> 'house_agent_revision_digest'
          and assignments.agent_profile_id = member #>> '{agent_profile,profile_id}'
          and assignments.agent_profile_revision = member #>> '{agent_profile,revision}'
          and assignments.runner_template_id = member #>> '{runner_template,template_id}'
          and assignments.runner_template_revision = member #>> '{runner_template,revision}'
      )
    )
  then
    raise exception using errcode = '22023', message = 'frozen_roster_mismatch';
  end if;

  if convert_from(listing_row.canonical_document, 'utf8')::jsonb #>> '{launch_input_schema,schema}' = 'worldstream/launch-input-schema/v2' and not exists (
    select 1
    from jsonb_array_elements(convert_from(listing_row.canonical_document, 'utf8')::jsonb #> '{launch_input_schema,roster_options}') option
    where option ->> 'option_id' = convert_from(launch_row.canonical_launch_input, 'utf8')::jsonb ->> 'roster_option'
      and parsed_setup -> 'configuration' = option -> 'configuration'
      and (select jsonb_agg(seat ->> 'label' order by position)
        from jsonb_array_elements(parsed_setup -> 'seats') with ordinality seats(seat, position))
        = (select jsonb_agg(seat ->> 'seat_id' order by position)
          from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) with ordinality seats(seat, position))
  ) then
    raise exception using errcode = '22023', message = 'room_setup_roster_option_mismatch';
  end if;

  if jsonb_typeof(parsed_setup) <> 'object'
    or parsed_setup ->> 'schema' <> 'worldstream/room-setup/v2'
    or parsed_setup #>> '{pack,digest}' <> listing_row.pack_revision_digest
    or coalesce((parsed_setup ->> 'operator_view')::boolean, true)
    or jsonb_typeof(parsed_setup -> 'seats') <> 'array'
    or jsonb_typeof(parsed_setup -> 'spectators') <> 'array'
    or not exists (
      select 1
      from jsonb_array_elements(parsed_setup -> 'spectators') spectators(value)
      where spectators.value ->> 'purpose' = 'result_indexer'
    )
    or (
      listing_row.public_viewing_policy = 'anonymous_by_link'
      and not exists (
        select 1
        from jsonb_array_elements(parsed_setup -> 'spectators') spectators(value)
        where spectators.value ->> 'purpose' = 'public_relay'
      )
    )
    or (
      launch_row.creator_access_choice = 'spectator'
      and not exists (
        select 1
        from jsonb_array_elements(parsed_setup -> 'spectators') spectators(value)
        where spectators.value ->> 'purpose' = 'creator'
      )
    )
  then
    raise exception using errcode = '22023', message = 'room_setup_mismatch';
  end if;

  update platform_store.seat_invitations invitations
  set revoked_at = sampled_at
  where invitations.launch_request_id = p_launch_request_id
    and invitations.consumed_at is null
    and invitations.revoked_at is null;

  update platform_store.launch_requests launches
  set roster_frozen_at = sampled_at,
      frozen_roster = p_frozen_roster,
      frozen_roster_digest = p_frozen_roster_digest,
      frozen_room_setup_specification = p_frozen_room_setup_specification,
      frozen_room_setup_specification_digest = p_frozen_room_setup_specification_digest,
      host_installation_id = p_host_installation_id,
      room_setup_operation_id = p_room_setup_operation_id,
      state = 'provisioning',
      last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

create or replace function platform_api.retain_house_fill_selection_v1(
  p_launch_request_id uuid,
  p_host_installation_id text
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  operation_row platform_store.house_fill_operations%rowtype;
  seat_record record;
  selected_revision text;
  selected_rank integer := 0;
  selected_count integer;
  candidate_count integer;
  active_runner_count integer;
  runner_limit integer;
  sampled_at timestamptz := clock_timestamp();
  candidate_digest bytea;
  exclusion_digest bytea;
begin
  if p_host_installation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$' then
    raise exception using errcode = '22023', message = 'invalid_host_installation';
  end if;
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then
    return null;
  end if;
  select operations.* into operation_row
  from platform_store.house_fill_operations operations
  where operations.launch_request_id = p_launch_request_id
  for update;
  if not found then
    raise exception using errcode = '55000', message = 'house_fill_not_started';
  end if;
  if operation_row.state <> 'claim_window_open' then
    if operation_row.host_installation_id is not null
      and operation_row.host_installation_id <> p_host_installation_id
    then
      raise exception using errcode = '23505', message = 'house_fill_host_conflict';
    end if;
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if operation_row.claim_window_closes_at > sampled_at then
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.host_mutation_started_at is not null
    or launch_row.expires_at <= sampled_at
  then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'launch_unavailable'
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  select listings.* into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  if exists (
    select 1
    from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
    where (seats.value ->> 'required')::boolean
      and not exists (
        select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = seats.value ->> 'seat_id'
          and claims.released_at is null
      )
      and not ((seats.value -> 'allowed_participation') ? 'house_agent_fill')
  ) then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'required_seat_not_fillable'
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  select count(*)::integer into selected_count
  from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
  where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
    and not exists (
      select 1 from platform_store.seat_claims claims
      where claims.launch_request_id = p_launch_request_id
        and claims.seat_id = seats.value ->> 'seat_id'
        and claims.released_at is null
    );
  if selected_count > 2 then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'assignment_limit_exceeded'
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;
  if selected_count = 0 then
    candidate_digest := extensions.digest(convert_to('[]', 'utf8'), 'sha256');
    exclusion_digest := extensions.digest(convert_to('[]', 'utf8'), 'sha256');
    update platform_store.house_fill_operations operations
    set host_installation_id = p_host_installation_id,
        selection_retained_at = sampled_at,
        candidate_set_digest = candidate_digest,
        exclusion_set_digest = exclusion_digest,
        selected_assignment_count = 0,
        state = 'assignments_complete',
        completed_at = sampled_at
    where operations.house_fill_operation_id = operation_row.house_fill_operation_id;
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  insert into platform_store.house_fill_candidate_evidence (
    house_fill_operation_id,
    house_agent_revision_digest,
    disposition,
    exclusion_reason,
    random_draw,
    retained_at
  )
  select
    operation_row.house_fill_operation_id,
    revisions.house_agent_revision_digest,
    case
      when exists (
        select 1
        from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
        where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
          and (seats.value -> 'allowed_house_agent_revisions')
            ? revisions.house_agent_revision_digest
          and not exists (
            select 1 from platform_store.seat_claims claims
            where claims.launch_request_id = p_launch_request_id
              and claims.seat_id = seats.value ->> 'seat_id'
              and claims.released_at is null
          )
      )
      and approvals.house_agent_revision_digest is not null
      and approvals.revoked_at is null
      and approvals.available_for_new_assignments
      and not exists (
        select 1 from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.house_agent_revision_digest = revisions.house_agent_revision_digest
      ) then 'candidate'
      else 'excluded'
    end,
    case
      when not exists (
        select 1
        from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
        where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
          and (seats.value -> 'allowed_house_agent_revisions')
            ? revisions.house_agent_revision_digest
          and not exists (
            select 1 from platform_store.seat_claims claims
            where claims.launch_request_id = p_launch_request_id
              and claims.seat_id = seats.value ->> 'seat_id'
              and claims.released_at is null
          )
      ) then 'not_listing_allowlisted'
      when approvals.house_agent_revision_digest is null or approvals.revoked_at is not null
        then 'host_not_approved'
      when not approvals.available_for_new_assignments then 'host_unavailable'
      when exists (
        select 1 from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.house_agent_revision_digest = revisions.house_agent_revision_digest
      ) then 'already_selected_in_lineage'
      else null
    end,
    case
      when approvals.house_agent_revision_digest is not null
        and approvals.revoked_at is null
        and approvals.available_for_new_assignments
        and exists (
          select 1
          from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
          where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
            and (seats.value -> 'allowed_house_agent_revisions')
              ? revisions.house_agent_revision_digest
            and not exists (
              select 1 from platform_store.seat_claims claims
              where claims.launch_request_id = p_launch_request_id
                and claims.seat_id = seats.value ->> 'seat_id'
                and claims.released_at is null
            )
        )
        and not exists (
          select 1 from platform_store.house_agent_assignments assignments
          where assignments.launch_request_id = p_launch_request_id
            and assignments.house_agent_revision_digest = revisions.house_agent_revision_digest
        )
      then extensions.gen_random_bytes(32)
      else null
    end,
    sampled_at
  from platform_store.house_agent_revisions revisions
  left join platform_store.house_agent_host_approvals approvals
    on approvals.house_agent_revision_digest = revisions.house_agent_revision_digest
   and approvals.host_installation_id = p_host_installation_id;

  select count(*)::integer into candidate_count
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.disposition = 'candidate';
  if candidate_count < selected_count then
    select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
      'revision', candidates.house_agent_revision_digest,
      'draw', encode(candidates.random_draw, 'hex')
    ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
    into candidate_digest
    from platform_store.house_fill_candidate_evidence candidates
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.disposition = 'candidate';
    select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
      'revision', candidates.house_agent_revision_digest,
      'reason', candidates.exclusion_reason
    ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
    into exclusion_digest
    from platform_store.house_fill_candidate_evidence candidates
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.disposition = 'excluded';
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'insufficient_candidates',
      p_host_installation_id,
      candidate_digest,
      exclusion_digest,
      selected_count
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  for seat_record in
    select seats.value, seats.ordinality,
      (
        select count(*)
        from platform_store.house_fill_candidate_evidence candidates
        where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
          and candidates.disposition = 'candidate'
          and (seats.value -> 'allowed_house_agent_revisions')
            ? candidates.house_agent_revision_digest
      ) as compatible_count
    from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input))
         with ordinality seats(value, ordinality)
    where (seats.value -> 'allowed_participation') ? 'house_agent_fill'
      and not exists (
        select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = seats.value ->> 'seat_id'
          and claims.released_at is null
      )
    order by compatible_count, seats.ordinality
  loop
    select candidates.house_agent_revision_digest into selected_revision
    from platform_store.house_fill_candidate_evidence candidates
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.disposition = 'candidate'
      and candidates.selected_seat_id is null
      and (seat_record.value -> 'allowed_house_agent_revisions')
        ? candidates.house_agent_revision_digest
    order by candidates.random_draw, candidates.house_agent_revision_digest
    limit 1;
    if selected_revision is null then
      select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
        'revision', candidates.house_agent_revision_digest,
        'draw', encode(candidates.random_draw, 'hex')
      ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
      into candidate_digest
      from platform_store.house_fill_candidate_evidence candidates
      where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
        and candidates.disposition = 'candidate';
      select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
        'revision', candidates.house_agent_revision_digest,
        'reason', candidates.exclusion_reason
      ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
      into exclusion_digest
      from platform_store.house_fill_candidate_evidence candidates
      where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
        and candidates.disposition = 'excluded';
      perform platform_store.fail_house_fill_v1(
        operation_row.house_fill_operation_id,
        'incompatible_candidate_set',
        p_host_installation_id,
        candidate_digest,
        exclusion_digest,
        selected_count
      );
      return platform_store.house_fill_operation_json_v1(p_launch_request_id);
    end if;
    selected_rank := selected_rank + 1;
    update platform_store.house_fill_candidate_evidence candidates
    set selected_seat_id = seat_record.value ->> 'seat_id', selection_rank = selected_rank
    where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
      and candidates.house_agent_revision_digest = selected_revision;
    selected_revision := null;
  end loop;

  select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
    'revision', candidates.house_agent_revision_digest,
    'draw', encode(candidates.random_draw, 'hex'),
    'seat', candidates.selected_seat_id
  ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
  into candidate_digest
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.disposition = 'candidate';
  select extensions.digest(convert_to(coalesce(jsonb_agg(jsonb_build_object(
    'revision', candidates.house_agent_revision_digest,
    'reason', candidates.exclusion_reason
  ) order by candidates.house_agent_revision_digest)::text, '[]'), 'utf8'), 'sha256')
  into exclusion_digest
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.disposition = 'excluded';

  select gates.hard_limit into strict runner_limit
  from platform_store.house_runner_capacity_gates gates
  where gates.gate_kind = 'runner_unit'
  for update;
  perform reservations.reservation_operation_id
  from platform_store.house_runner_reservations reservations
  where reservations.state in ('pending', 'ambiguous', 'succeeded')
    and reservations.released_at is null
  order by reservations.reservation_operation_id
  for update;
  select count(*)::integer into active_runner_count
  from platform_store.house_runner_reservations reservations
  where reservations.state in ('pending', 'ambiguous', 'succeeded')
    and reservations.released_at is null;
  if active_runner_count + selected_count > runner_limit then
    perform platform_store.fail_house_fill_v1(
      operation_row.house_fill_operation_id,
      'runner_capacity_unavailable',
      p_host_installation_id,
      candidate_digest,
      exclusion_digest,
      selected_count
    );
    return platform_store.house_fill_operation_json_v1(p_launch_request_id);
  end if;

  insert into platform_store.house_runner_reservations (
    house_fill_operation_id,
    launch_request_id,
    seat_id,
    house_agent_revision_digest,
    reserved_at,
    last_transition_at
  )
  select
    operation_row.house_fill_operation_id,
    p_launch_request_id,
    candidates.selected_seat_id,
    candidates.house_agent_revision_digest,
    sampled_at,
    sampled_at
  from platform_store.house_fill_candidate_evidence candidates
  where candidates.house_fill_operation_id = operation_row.house_fill_operation_id
    and candidates.selected_seat_id is not null
  order by candidates.selection_rank;

  update platform_store.house_fill_operations operations
  set host_installation_id = p_host_installation_id,
      selection_retained_at = sampled_at,
      candidate_set_digest = candidate_digest,
      exclusion_set_digest = exclusion_digest,
      selected_assignment_count = selected_count,
      state = 'reserving'
  where operations.house_fill_operation_id = operation_row.house_fill_operation_id;
  return platform_store.house_fill_operation_json_v1(p_launch_request_id);
end;
$$;

create or replace function platform_api.record_genesis_v1(
  p_launch_request_id uuid,
  p_canonical_genesis_evidence bytea,
  p_genesis_evidence_digest bytea
)
returns table(
  activity_run_id uuid,
  public_id text,
  reconciliation_state text,
  disposition text,
  safe_code text,
  evidence_class text
)
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  existing_run platform_store.activity_runs%rowtype;
  existing_membership platform_store.activity_run_memberships%rowtype;
  evidence jsonb;
  frozen_roster jsonb;
  frozen_setup jsonb;
  listing_document jsonb;
  member jsonb;
  derived_member jsonb;
  roster_member jsonb;
  listing_seat jsonb;
  derived_members jsonb := '[]'::jsonb;
  v_run_id uuid;
  conflicting_run_id uuid;
  v_public_id text;
  v_initial_state text;
  v_evidence_class text;
  v_reconciliation_state text;
  v_disposition text;
  v_safe_code text;
  v_source_key text;
  v_purpose text;
  v_setup_purpose text;
  v_source text;
  v_expected_kind text;
  v_controlling_account_id uuid;
  v_house_agent_assignment_id uuid;
  v_scope_digest bytea;
  expected_members integer;
  current_members integer;
  repaired_members integer := 0;
  identity_valid boolean := true;
  membership_valid boolean := true;
  authorization_valid boolean := false;
  correspondence_conflict boolean := false;
  authorized_capacity_present boolean := false;
  has_prior_conflict boolean := false;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_launch_request_id is null
    or octet_length(p_canonical_genesis_evidence) not between 2 and 262144
    or octet_length(p_genesis_evidence_digest) <> 32
    or p_genesis_evidence_digest
      <> extensions.digest(p_canonical_genesis_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_genesis_evidence';
  end if;

  begin
    evidence := convert_from(p_canonical_genesis_evidence, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_genesis_evidence';
  end;

  if jsonb_typeof(evidence) <> 'object'
    or evidence ->> 'schema' <> 'worldstream/hosted-genesis-evidence/v1'
    or not (evidence ?& array[
      'schema',
      'host_installation_id',
      'launch_request_id',
      'listing_revision_digest',
      'launch_request_digest',
      'frozen_roster_digest',
      'room_setup_specification_digest',
      'room_setup_operation_id',
      'room_id',
      'pack',
      'genesis_head',
      'memberships'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema',
        'host_installation_id',
        'launch_request_id',
        'listing_revision_digest',
        'launch_request_digest',
        'frozen_roster_digest',
        'room_setup_specification_digest',
        'room_setup_operation_id',
        'room_id',
        'pack',
        'genesis_head',
        'memberships'
      )
    )
    or (evidence ->> 'launch_request_id')
      !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
    or (evidence ->> 'launch_request_id')::uuid <> p_launch_request_id
    or (evidence ->> 'host_installation_id')
      !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or (evidence ->> 'listing_revision_digest') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence ->> 'launch_request_digest') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence ->> 'frozen_roster_digest') !~ '^sha256:[0-9a-f]{64}$'
    or (evidence ->> 'room_setup_specification_digest')
      !~ '^blake3:[0-9a-f]{64}$'
    or (evidence ->> 'room_setup_operation_id') !~ '^[a-z][a-z0-9-]{0,63}$'
    or (evidence ->> 'room_id') !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or jsonb_typeof(evidence -> 'pack') <> 'object'
    or not ((evidence -> 'pack') ?& array['id', 'version', 'digest'])
    or exists (
      select 1 from jsonb_object_keys(evidence -> 'pack') keys(key)
      where keys.key not in ('id', 'version', 'digest')
    )
    or length(evidence #>> '{pack,id}') not between 1 and 128
    or (evidence #>> '{pack,id}') !~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    or length(evidence #>> '{pack,version}') not between 1 and 64
    or (evidence #>> '{pack,version}') ~ '[[:cntrl:]]'
    or (evidence #>> '{pack,digest}') !~ '^blake3:[0-9a-f]{64}$'
    or jsonb_typeof(evidence -> 'genesis_head') <> 'object'
    or not ((evidence -> 'genesis_head') ?& array[
      'room_id',
      'room_seq',
      'genesis_or_transition_hash',
      'core_schema_version',
      'pack_digest',
      'core_state_hash',
      'activity_state_hash',
      'authoritative_state_hash'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence -> 'genesis_head') keys(key)
      where keys.key not in (
        'room_id',
        'room_seq',
        'genesis_or_transition_hash',
        'core_schema_version',
        'pack_digest',
        'core_state_hash',
        'activity_state_hash',
        'authoritative_state_hash'
      )
    )
    or evidence #>> '{genesis_head,room_id}' <> evidence ->> 'room_id'
    or jsonb_typeof(evidence #> '{genesis_head,room_seq}') <> 'number'
    or evidence #>> '{genesis_head,room_seq}' <> '0'
    or length(evidence #>> '{genesis_head,core_schema_version}') not between 1 and 128
    or (evidence #>> '{genesis_head,core_schema_version}')
      !~ '^[A-Za-z0-9][A-Za-z0-9._:/-]*$'
    or (evidence #>> '{genesis_head,genesis_or_transition_hash}')
      !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,pack_digest}') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,core_state_hash}') !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,activity_state_hash}')
      !~ '^blake3:[0-9a-f]{64}$'
    or (evidence #>> '{genesis_head,authoritative_state_hash}')
      !~ '^blake3:[0-9a-f]{64}$'
    or evidence #>> '{genesis_head,pack_digest}' <> evidence #>> '{pack,digest}'
    or jsonb_typeof(evidence -> 'memberships') <> 'array'
    or jsonb_array_length(evidence -> 'memberships') not between 1 and 35
    or exists (
      select 1
      from jsonb_array_elements(evidence -> 'memberships') members(value)
      where jsonb_typeof(members.value) <> 'object'
        or not (members.value ?& array[
          'access_mode',
          'purpose',
          'seat_id',
          'role',
          'principal_kind',
          'principal_id',
          'membership_id'
        ])
        or exists (
          select 1 from jsonb_object_keys(members.value) member_keys(key)
          where member_keys.key not in (
            'access_mode',
            'purpose',
            'seat_id',
            'role',
            'principal_kind',
            'principal_id',
            'membership_id',
            'scopes'
          )
        )
        or members.value ->> 'access_mode' not in ('participant', 'spectator')
        or members.value ->> 'purpose' not in (
          'participant',
          'creator_spectator',
          'result_indexer',
          'public_projection_relay'
        )
        or members.value ->> 'principal_kind' not in ('human', 'agent')
        or members.value ->> 'principal_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
        or members.value ->> 'membership_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    )
    or (
      select count(*) <> count(distinct value ->> 'principal_id')
      from jsonb_array_elements(evidence -> 'memberships')
    )
    or (
      select count(*) <> count(distinct value ->> 'membership_id')
      from jsonb_array_elements(evidence -> 'memberships')
    )
    or (
      select count(*) <> count(distinct value ->> 'seat_id')
      from jsonb_array_elements(evidence -> 'memberships')
      where value ->> 'purpose' = 'participant'
    )
    or (
      select count(*) <> count(distinct value ->> 'purpose')
      from jsonb_array_elements(evidence -> 'memberships')
      where value ->> 'purpose' <> 'participant'
    )
    or (
      select count(*) <> 1
      from jsonb_array_elements(evidence -> 'memberships')
      where value ->> 'purpose' = 'result_indexer'
    )
  then
    raise exception using errcode = '22023', message = 'invalid_genesis_evidence';
  end if;

  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then
    raise exception using errcode = '55000', message = 'launch_not_found';
  end if;

  select listings.*
  into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  begin
    frozen_roster := convert_from(launch_row.frozen_roster, 'utf8')::jsonb;
    frozen_setup := convert_from(launch_row.frozen_room_setup_specification, 'utf8')::jsonb;
    listing_document := convert_from(listing_row.canonical_document, 'utf8')::jsonb;
  exception when others then
    identity_valid := false;
    frozen_roster := jsonb_build_object('members', '[]'::jsonb);
    frozen_setup := jsonb_build_object('spectators', '[]'::jsonb);
    listing_document := '{}'::jsonb;
  end;

  v_source_key := 'host:' || (evidence ->> 'host_installation_id')
    || '/room:' || (evidence ->> 'room_id') || '/genesis:0';
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended(
      'host:' || (evidence ->> 'host_installation_id')
        || '/operation:' || (evidence ->> 'room_setup_operation_id'),
      0
    )
  );
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended(v_source_key, 0)
  );

  select runs.*
  into existing_run
  from platform_store.activity_runs runs
  where runs.launch_request_id = p_launch_request_id;
  if found then
    v_run_id := existing_run.activity_run_id;
    if existing_run.genesis_evidence_digest <> p_genesis_evidence_digest
      or existing_run.host_installation_id <> evidence ->> 'host_installation_id'
      or existing_run.room_setup_operation_id <> evidence ->> 'room_setup_operation_id'
      or existing_run.room_id <> evidence ->> 'room_id'
      or existing_run.listing_revision_digest <> launch_row.listing_revision_digest
      or existing_run.launch_request_digest <> evidence ->> 'launch_request_digest'
      or existing_run.pack_revision_digest <> evidence #>> '{pack,digest}'
      or existing_run.genesis_or_transition_hash
        <> evidence #>> '{genesis_head,genesis_or_transition_hash}'
    then
      insert into platform_store.reconciliation_receipts (
        receipt_kind,
        source_key,
        launch_request_id,
        activity_run_id,
        evidence_digest,
        disposition,
        safe_code,
        observed_at
      ) values (
        'genesis',
        v_source_key,
        p_launch_request_id,
        v_run_id,
        p_genesis_evidence_digest,
        'conflict',
        'genesis_identity_conflict',
        sampled_at
      ) on conflict do nothing;
      return query select
        v_run_id,
        existing_run.public_id,
        'quarantined'::text,
        'conflict'::text,
        'genesis_identity_conflict'::text,
        existing_run.evidence_class;
      return;
    end if;
    select exists (
      select 1
      from platform_store.reconciliation_receipts receipts
      where receipts.activity_run_id = v_run_id
        and receipts.disposition = 'conflict'
    ) into has_prior_conflict;
  else
    select runs.activity_run_id
    into conflicting_run_id
    from platform_store.activity_runs runs
    where (
        runs.host_installation_id = evidence ->> 'host_installation_id'
        and runs.room_id = evidence ->> 'room_id'
      )
      or (
        runs.host_installation_id = evidence ->> 'host_installation_id'
        and runs.room_setup_operation_id = evidence ->> 'room_setup_operation_id'
    )
    order by runs.activity_run_id
    limit 1;
    if found then
      insert into platform_store.reconciliation_receipts (
        receipt_kind,
        source_key,
        launch_request_id,
        activity_run_id,
        evidence_digest,
        disposition,
        safe_code,
        observed_at
      ) values (
        'genesis',
        v_source_key,
        p_launch_request_id,
        conflicting_run_id,
        p_genesis_evidence_digest,
        'conflict',
        'room_identity_conflict',
        sampled_at
      ) on conflict do nothing;
      return query select
        conflicting_run_id,
        null::text,
        'quarantined'::text,
        'conflict'::text,
        'room_identity_conflict'::text,
        null::text;
      return;
    end if;
  end if;

  identity_valid := coalesce(identity_valid
    and launch_row.roster_frozen_at is not null
    and launch_row.host_installation_id = evidence ->> 'host_installation_id'
    and launch_row.room_setup_operation_id = evidence ->> 'room_setup_operation_id'
    and launch_row.listing_revision_digest = evidence ->> 'listing_revision_digest'
    and ('sha256:' || encode(launch_row.frozen_roster_digest, 'hex'))
      = evidence ->> 'frozen_roster_digest'
    and launch_row.frozen_room_setup_specification_digest
      = evidence ->> 'room_setup_specification_digest'
    and listing_row.pack_revision_digest = evidence #>> '{pack,digest}'
    and listing_document -> 'pack' = evidence -> 'pack'
    and evidence #>> '{genesis_head,pack_digest}' = listing_row.pack_revision_digest,
    false
  );

  if frozen_roster is null
    or frozen_setup is null
    or jsonb_typeof(frozen_roster -> 'members') is distinct from 'array'
    or jsonb_typeof(frozen_setup -> 'spectators') is distinct from 'array'
  then
    membership_valid := false;
    expected_members := -1;
  else
    expected_members := jsonb_array_length(frozen_roster -> 'members')
      + jsonb_array_length(frozen_setup -> 'spectators');
    membership_valid := coalesce(
      expected_members = jsonb_array_length(evidence -> 'memberships'),
      false
    );
  end if;

  for member in select value from jsonb_array_elements(evidence -> 'memberships')
  loop
    v_purpose := member ->> 'purpose';
    v_source := null;
    v_expected_kind := null;
    v_controlling_account_id := null;
    v_house_agent_assignment_id := null;
    v_scope_digest := null;
    roster_member := null;
    listing_seat := null;

    if v_purpose = 'participant' then
      if member ->> 'access_mode' <> 'participant'
        or jsonb_typeof(member -> 'seat_id') <> 'string'
        or (member ->> 'seat_id') !~ '^[a-z][a-z0-9_-]{0,63}$'
        or jsonb_typeof(member -> 'role') <> 'string'
        or (member ->> 'role') !~ '^[a-z][a-z0-9_-]{0,63}$'
        or (member ? 'scopes' and member -> 'scopes' <> '[]'::jsonb)
      then
        membership_valid := false;
        continue;
      end if;

      select roster.value
      into roster_member
      from jsonb_array_elements(frozen_roster -> 'members') roster(value)
      where roster.value ->> 'seat_id' = member ->> 'seat_id';
      select seats.value
      into listing_seat
      from jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input)) seats(value)
      where seats.value ->> 'seat_id' = member ->> 'seat_id';
      if roster_member is null
        or listing_seat is null
        or listing_seat ->> 'role' <> member ->> 'role'
      then
        membership_valid := false;
        continue;
      end if;

      v_source := case roster_member ->> 'participation'
        when 'account_human' then 'account_human'
        when 'account_external_agent' then 'account_external_agent'
        when 'house_agent_fill' then 'platform_house_agent'
        else null
      end;
      v_expected_kind := case v_source
        when 'account_human' then 'human'
        when 'account_external_agent' then 'agent'
        when 'platform_house_agent' then 'agent'
        else null
      end;
      if v_source in ('account_human', 'account_external_agent') then
        select claims.controlling_account_id
        into v_controlling_account_id
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = member ->> 'seat_id'
          and claims.participation_kind = roster_member ->> 'participation'
          and claims.principal_reference = roster_member ->> 'principal_reference'
          and claims.released_at is null;
        if v_controlling_account_id is null then
          membership_valid := false;
        end if;
      elsif v_source = 'platform_house_agent' then
        select assignments.house_agent_assignment_id
        into v_house_agent_assignment_id
        from platform_store.house_agent_assignments assignments
        where assignments.launch_request_id = p_launch_request_id
          and assignments.seat_id = member ->> 'seat_id'
          and assignments.setup_principal_reference = roster_member ->> 'principal_reference'
          and assignments.house_agent_revision_digest
            = roster_member ->> 'house_agent_revision_digest';
        if v_house_agent_assignment_id is null then
          membership_valid := false;
        end if;
      else
        membership_valid := false;
      end if;
      if member ->> 'principal_kind' is distinct from v_expected_kind then
        membership_valid := false;
      end if;
    else
      if member ->> 'access_mode' <> 'spectator'
        or jsonb_typeof(member -> 'seat_id') <> 'null'
        or jsonb_typeof(member -> 'role') <> 'null'
        or jsonb_typeof(member -> 'scopes') <> 'array'
      then
        membership_valid := false;
        continue;
      end if;
      v_setup_purpose := case v_purpose
        when 'creator_spectator' then 'creator'
        when 'result_indexer' then 'result_indexer'
        when 'public_projection_relay' then 'public_relay'
        else null
      end;
      if not exists (
        select 1
        from jsonb_array_elements(frozen_setup -> 'spectators') spectators(value)
        where spectators.value ->> 'purpose' = v_setup_purpose
      ) then
        membership_valid := false;
      end if;
      if v_purpose = 'creator_spectator' then
        v_source := 'account_human';
        v_expected_kind := 'human';
        v_controlling_account_id := launch_row.creator_account_id;
        if member -> 'scopes'
          <> '["room:attach","room:observe_public"]'::jsonb
        then
          membership_valid := false;
        end if;
      elsif v_purpose = 'result_indexer' then
        v_expected_kind := 'agent';
        if member -> 'scopes'
          <> '["room:attach","room:observe_public","room:replay"]'::jsonb
        then
          membership_valid := false;
        end if;
      elsif v_purpose = 'public_projection_relay' then
        v_expected_kind := 'agent';
        if member -> 'scopes'
          <> '["room:attach","room:observe_public"]'::jsonb
        then
          membership_valid := false;
        end if;
      end if;
      if member ->> 'principal_kind' is distinct from v_expected_kind then
        membership_valid := false;
      end if;
      v_scope_digest := extensions.digest(
        convert_to((member -> 'scopes')::text, 'utf8'),
        'sha256'
      );
    end if;

    if exists (
      select 1
      from platform_store.activity_run_memberships memberships
      where (
          memberships.membership_id = member ->> 'membership_id'
          or memberships.principal_id = member ->> 'principal_id'
        )
        and (v_run_id is null or memberships.activity_run_id <> v_run_id)
    ) then
      membership_valid := false;
    end if;

    derived_member := jsonb_build_object(
      'membership_id', member ->> 'membership_id',
      'access_mode', member ->> 'access_mode',
      'purpose', v_purpose,
      'seat_id', member ->> 'seat_id',
      'role', member ->> 'role',
      'public_seat_label', case
        when v_purpose = 'participant' then listing_seat ->> 'display_name'
        else null
      end,
      'principal_kind', member ->> 'principal_kind',
      'participation_source', v_source,
      'controlling_account_id', v_controlling_account_id,
      'principal_id', member ->> 'principal_id',
      'service_scope_digest', case
        when v_scope_digest is null then null
        else encode(v_scope_digest, 'hex')
      end,
      'house_agent_assignment_id', v_house_agent_assignment_id
    );
    derived_members := derived_members || jsonb_build_array(derived_member);
  end loop;

  if jsonb_array_length(derived_members) <> jsonb_array_length(evidence -> 'memberships') then
    membership_valid := false;
  end if;

  if v_run_id is not null and membership_valid then
    for derived_member in select value from jsonb_array_elements(derived_members)
    loop
      select memberships.*
      into existing_membership
      from platform_store.activity_run_memberships memberships
      where memberships.activity_run_id = v_run_id
        and (
          memberships.membership_id = derived_member ->> 'membership_id'
          or memberships.principal_id = derived_member ->> 'principal_id'
          or (
            derived_member ->> 'purpose' = 'participant'
            and memberships.seat_id = derived_member ->> 'seat_id'
          )
          or (
            derived_member ->> 'purpose' <> 'participant'
            and memberships.purpose = derived_member ->> 'purpose'
          )
        )
      limit 1;
      if found and (
        existing_membership.membership_id <> derived_member ->> 'membership_id'
        or existing_membership.access_mode <> derived_member ->> 'access_mode'
        or existing_membership.purpose <> derived_member ->> 'purpose'
        or existing_membership.seat_id is distinct from derived_member ->> 'seat_id'
        or existing_membership.role is distinct from derived_member ->> 'role'
        or existing_membership.public_seat_label
          is distinct from derived_member ->> 'public_seat_label'
        or existing_membership.principal_kind <> derived_member ->> 'principal_kind'
        or existing_membership.participation_source
          is distinct from derived_member ->> 'participation_source'
        or existing_membership.controlling_account_id
          is distinct from (derived_member ->> 'controlling_account_id')::uuid
        or existing_membership.principal_id <> derived_member ->> 'principal_id'
        or existing_membership.service_scope_digest is distinct from case
          when jsonb_typeof(derived_member -> 'service_scope_digest') = 'null' then null
          else decode(derived_member ->> 'service_scope_digest', 'hex')
        end
        or existing_membership.house_agent_assignment_id
          is distinct from (derived_member ->> 'house_agent_assignment_id')::uuid
        or (
          derived_member ->> 'controlling_account_id' is not null
          and existing_membership.entry_selector is null
        )
        or (
          derived_member ->> 'controlling_account_id' is null
          and existing_membership.entry_selector is not null
        )
      ) then
        correspondence_conflict := true;
      end if;
    end loop;
  end if;

  if correspondence_conflict then
    insert into platform_store.reconciliation_receipts (
      receipt_kind,
      source_key,
      launch_request_id,
      activity_run_id,
      evidence_digest,
      disposition,
      safe_code,
      observed_at
    ) values (
      'genesis',
      v_source_key,
      p_launch_request_id,
      v_run_id,
      p_genesis_evidence_digest,
      'conflict',
      'membership_correspondence_conflict',
      sampled_at
    ) on conflict do nothing;
    return query select
      v_run_id,
      existing_run.public_id,
      'quarantined'::text,
      'conflict'::text,
      'membership_correspondence_conflict'::text,
      existing_run.evidence_class;
    return;
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  join (
    select launch_row.creator_account_id as account_id
    union
    select claims.controlling_account_id
    from platform_store.seat_claims claims
    where claims.launch_request_id = p_launch_request_id
      and claims.released_at is null
  ) controlling on controlling.account_id = accounts.account_id
  order by accounts.account_id
  for update of accounts;

  select exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.launch_request_id = p_launch_request_id
      and reservations.kind = 'active_run'
      and reservations.reservation_class = 'authorized'
      and reservations.released_at is null
      and (reservations.activity_run_id is null or reservations.activity_run_id = v_run_id)
  ) into authorized_capacity_present;

  authorization_valid := coalesce(identity_valid
    and membership_valid
    and launch_row.host_mutation_started_at is not null
    and launch_row.state in ('provisioning', 'reconciling', 'run_created')
    and authorized_capacity_present
    and not exists (
      select 1
      from platform_store.platform_accounts accounts
      join (
        select launch_row.creator_account_id as account_id
        union
        select claims.controlling_account_id
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.released_at is null
      ) controlling on controlling.account_id = accounts.account_id
      where accounts.erased_at is not null
    ), false);

  v_evidence_class := case
    when exists (
      select 1
      from platform_store.house_agent_assignments assignments
      where assignments.launch_request_id = p_launch_request_id
    ) then 'exhibition_platform_house_agents'
    else 'unranked'
  end;

  if v_run_id is null then
    v_run_id := gen_random_uuid();
    v_initial_state := case
      when identity_valid and membership_valid and authorization_valid then 'ready'
      else 'quarantined'
    end;
    if listing_row.public_viewing_policy = 'anonymous_by_link'
      or listing_row.result_publication_policy = 'public_recent_results'
    then
      v_public_id := encode(extensions.gen_random_bytes(16), 'hex');
    end if;
    insert into platform_store.activity_runs (
      activity_run_id,
      launch_request_id,
      listing_revision_digest,
      creator_account_id,
      host_installation_id,
      room_setup_operation_id,
      room_id,
      launch_request_digest,
      pack_id,
      pack_version,
      pack_revision_digest,
      genesis_room_seq,
      genesis_or_transition_hash,
      canonical_genesis_evidence,
      genesis_evidence_digest,
      public_id,
      evidence_class,
      initial_reconciliation_state,
      genesis_observed_at,
      inserted_at
    ) values (
      v_run_id,
      p_launch_request_id,
      launch_row.listing_revision_digest,
      launch_row.creator_account_id,
      evidence ->> 'host_installation_id',
      evidence ->> 'room_setup_operation_id',
      evidence ->> 'room_id',
      evidence ->> 'launch_request_digest',
      evidence #>> '{pack,id}',
      evidence #>> '{pack,version}',
      evidence #>> '{pack,digest}',
      0,
      evidence #>> '{genesis_head,genesis_or_transition_hash}',
      p_canonical_genesis_evidence,
      p_genesis_evidence_digest,
      v_public_id,
      v_evidence_class,
      v_initial_state,
      sampled_at,
      sampled_at
    );
  else
    v_public_id := existing_run.public_id;
    v_initial_state := existing_run.initial_reconciliation_state;
    v_evidence_class := existing_run.evidence_class;
  end if;

  if membership_valid and v_initial_state in ('ready', 'quarantined') then
    for derived_member in select value from jsonb_array_elements(derived_members)
    loop
      if not exists (
        select 1
        from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = v_run_id
          and memberships.membership_id = derived_member ->> 'membership_id'
      ) then
        insert into platform_store.activity_run_memberships (
          activity_run_id,
          membership_id,
          access_mode,
          purpose,
          seat_id,
          role,
          public_seat_label,
          principal_kind,
          participation_source,
          controlling_account_id,
          principal_id,
          entry_selector,
          service_scope_digest,
          house_agent_assignment_id,
          promoted_at
        ) values (
          v_run_id,
          derived_member ->> 'membership_id',
          derived_member ->> 'access_mode',
          derived_member ->> 'purpose',
          derived_member ->> 'seat_id',
          derived_member ->> 'role',
          derived_member ->> 'public_seat_label',
          derived_member ->> 'principal_kind',
          derived_member ->> 'participation_source',
          (derived_member ->> 'controlling_account_id')::uuid,
          derived_member ->> 'principal_id',
          case
            when derived_member ->> 'controlling_account_id' is not null
              then encode(extensions.gen_random_bytes(16), 'hex')
            else null
          end,
          case
            when jsonb_typeof(derived_member -> 'service_scope_digest') = 'null' then null
            else decode(derived_member ->> 'service_scope_digest', 'hex')
          end,
          (derived_member ->> 'house_agent_assignment_id')::uuid,
          sampled_at
        );
        repaired_members := repaired_members + 1;
      end if;
    end loop;
  end if;

  select count(*)::integer
  into current_members
  from platform_store.activity_run_memberships memberships
  where memberships.activity_run_id = v_run_id;
  if membership_valid and current_members <> expected_members then
    insert into platform_store.reconciliation_receipts (
      receipt_kind,
      source_key,
      launch_request_id,
      activity_run_id,
      evidence_digest,
      disposition,
      safe_code,
      observed_at
    ) values (
      'genesis',
      v_source_key,
      p_launch_request_id,
      v_run_id,
      p_genesis_evidence_digest,
      'conflict',
      'membership_count_conflict',
      sampled_at
    ) on conflict do nothing;
    return query select
      v_run_id,
      v_public_id,
      'quarantined'::text,
      'conflict'::text,
      'membership_count_conflict'::text,
      v_evidence_class;
    return;
  end if;

  update platform_store.capacity_reservations reservations
  set activity_run_id = v_run_id
  where reservations.launch_request_id = p_launch_request_id
    and reservations.kind = 'active_run'
    and reservations.reservation_class = 'authorized'
    and reservations.released_at is null
    and reservations.activity_run_id is null;

  if not exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.launch_request_id = p_launch_request_id
      and reservations.kind = 'active_run'
      and reservations.released_at is null
      and reservations.activity_run_id = v_run_id
  ) then
    insert into platform_store.capacity_reservations (
      launch_request_id,
      kind,
      reservation_class,
      controlling_account_id,
      activity_run_id,
      reserved_at
    ) values (
      p_launch_request_id,
      'active_run',
      'quarantined_recovery',
      launch_row.creator_account_id,
      v_run_id,
      sampled_at
    );
  end if;

  update platform_store.capacity_reservations reservations
  set released_at = sampled_at,
      release_reason = 'genesis_observed'
  where reservations.launch_request_id = p_launch_request_id
    and reservations.kind = 'pre_genesis'
    and reservations.released_at is null;

  if launch_row.state in ('provisioning', 'reconciling') then
    update platform_store.launch_requests launches
    set state = 'run_created',
        last_transition_at = sampled_at
    where launches.launch_request_id = p_launch_request_id;
  end if;

  if v_initial_state = 'quarantined' or has_prior_conflict then
    v_disposition := case
      when existing_run.activity_run_id is null then 'blocked'
      else 'duplicate'
    end;
    v_safe_code := case
      when not identity_valid then 'genesis_identity_mismatch'
      when not membership_valid then 'membership_evidence_mismatch'
      when not authorization_valid then 'authorization_recovery_required'
      else 'genesis_quarantined'
    end;
    v_reconciliation_state := 'quarantined';
  elsif existing_run.activity_run_id is not null and repaired_members = 0 then
    v_disposition := 'duplicate';
    v_safe_code := 'genesis_already_recorded';
    v_reconciliation_state := 'ready';
  elsif existing_run.activity_run_id is not null then
    v_disposition := 'applied';
    v_safe_code := 'membership_correspondence_repaired';
    v_reconciliation_state := 'ready';
  else
    v_disposition := 'applied';
    v_safe_code := 'genesis_recorded';
    v_reconciliation_state := 'ready';
  end if;

  insert into platform_store.reconciliation_receipts (
    receipt_kind,
    source_key,
    launch_request_id,
    activity_run_id,
    evidence_digest,
    disposition,
    safe_code,
    observed_at
  ) values (
    'genesis',
    v_source_key,
    p_launch_request_id,
    v_run_id,
    p_genesis_evidence_digest,
    v_disposition,
    v_safe_code,
    sampled_at
  ) on conflict do nothing;

  return query select
    v_run_id,
    v_public_id,
    v_reconciliation_state,
    v_disposition,
    v_safe_code,
    v_evidence_class;
end;
$$;

create or replace function platform_api.read_hosted_launch_material_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  claims jsonb;
  assignments jsonb;
begin
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
    and launches.creator_account_id = p_creator_account_id;
  if not found then return null; end if;

  select listings.* into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  select coalesce(jsonb_agg(jsonb_build_object(
    'seat_id', claims.seat_id,
    'role', claims.role,
    'display_name', seats.value ->> 'display_name',
    'participation_kind', claims.participation_kind,
    'principal_reference', claims.principal_reference
  ) order by seats.ordinality), '[]'::jsonb)
  into claims
  from platform_store.seat_claims claims
  join jsonb_array_elements(platform_store.selected_listing_seats_v2(listing_row, launch_row.canonical_launch_input))
       with ordinality seats(value, ordinality)
    on seats.value ->> 'seat_id' = claims.seat_id
  where claims.launch_request_id = p_launch_request_id
    and claims.released_at is null;

  select coalesce(jsonb_agg(jsonb_build_object(
    'house_agent_assignment_id', assignments.house_agent_assignment_id,
    'seat_id', assignments.seat_id,
    'principal_reference', assignments.setup_principal_reference,
    'display_name', revisions.display_name,
    'house_agent_revision_digest', assignments.house_agent_revision_digest,
    'agent_profile', jsonb_build_object(
      'profile_id', assignments.agent_profile_id,
      'revision', assignments.agent_profile_revision
    ),
    'runner_template', jsonb_build_object(
      'template_id', assignments.runner_template_id,
      'revision', assignments.runner_template_revision
    ),
    'reservation_receipt', convert_from(reservations.reservation_receipt, 'utf8')::jsonb
  ) order by assignments.seat_id), '[]'::jsonb)
  into assignments
  from platform_store.house_agent_assignments assignments
  join platform_store.house_agent_revisions revisions
    on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
  join platform_store.house_runner_reservations reservations
    on reservations.reservation_operation_id = assignments.reservation_operation_id
  where assignments.launch_request_id = p_launch_request_id
    -- A released reservation ends execution capacity, not the immutable
    -- Assignment or its original successful receipt. Recovery and terminal
    -- private re-entry must reconstruct the same frozen setup after retirement.
    and reservations.state in ('succeeded', 'released')
    and reservations.reservation_receipt is not null;

  return jsonb_build_object(
    'version', 'platform_hosted_launch_material.v1',
    'launch_request_id', launch_row.launch_request_id,
    'listing_revision_digest', launch_row.listing_revision_digest,
    'state', launch_row.state,
    'expires_at', launch_row.expires_at,
    'launch_inputs', convert_from(launch_row.canonical_launch_input, 'utf8')::jsonb,
    'house_fill_choice', launch_row.house_fill_choice,
    'creator_access_choice', launch_row.creator_access_choice,
    'creator_seat_id', launch_row.creator_seat_id,
    'host_installation_id', launch_row.host_installation_id,
    'room_setup_operation_id', launch_row.room_setup_operation_id,
    'roster_frozen', launch_row.roster_frozen_at is not null,
    'host_mutation_started', launch_row.host_mutation_started_at is not null,
    'claims', claims,
    'house_assignments', assignments
  );
end;
$$;
