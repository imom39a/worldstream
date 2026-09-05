create extension if not exists pgcrypto with schema extensions;

create function platform_store.valid_listing_seats_v1(p_seats jsonb)
returns boolean
language plpgsql
immutable
security invoker
set search_path = ''
as $$
declare
  seat jsonb;
  participation jsonb;
  house_revision jsonb;
begin
  if jsonb_typeof(p_seats) <> 'array'
    or jsonb_array_length(p_seats) not between 1 and 32
    or octet_length(p_seats::text) > 65536
  then
    return false;
  end if;

  if (
    select count(*) <> count(distinct value ->> 'seat_id')
    from jsonb_array_elements(p_seats)
  ) then
    return false;
  end if;

  for seat in select value from jsonb_array_elements(p_seats)
  loop
    if jsonb_typeof(seat) <> 'object'
      or exists (
        select 1
        from jsonb_object_keys(seat) as key
        where key not in (
          'seat_id',
          'role',
          'display_name',
          'required',
          'allowed_participation',
          'allowed_house_agent_revisions'
        )
      )
      or not (seat ?& array[
        'seat_id',
        'role',
        'display_name',
        'required',
        'allowed_participation',
        'allowed_house_agent_revisions'
      ])
      or (seat ->> 'seat_id') !~ '^[a-z][a-z0-9_-]{0,63}$'
      or (seat ->> 'role') !~ '^[a-z][a-z0-9_-]{0,63}$'
      or length(seat ->> 'display_name') not between 1 and 128
      or (seat ->> 'display_name') ~ '[[:cntrl:]]'
      or jsonb_typeof(seat -> 'required') <> 'boolean'
      or jsonb_typeof(seat -> 'allowed_participation') <> 'array'
      or jsonb_array_length(seat -> 'allowed_participation') not between 1 and 3
      or jsonb_typeof(seat -> 'allowed_house_agent_revisions') <> 'array'
      or jsonb_array_length(seat -> 'allowed_house_agent_revisions') > 16
    then
      return false;
    end if;

    if (
      select count(*) <> count(distinct value)
      from jsonb_array_elements(seat -> 'allowed_participation')
    ) then
      return false;
    end if;

    for participation in
      select value from jsonb_array_elements(seat -> 'allowed_participation')
    loop
      if jsonb_typeof(participation) <> 'string'
        or participation #>> '{}' not in (
          'account_human',
          'account_external_agent',
          'house_agent_fill'
        )
      then
        return false;
      end if;
    end loop;

    if not ((seat -> 'allowed_participation') ? 'house_agent_fill')
      and jsonb_array_length(seat -> 'allowed_house_agent_revisions') <> 0
    then
      return false;
    end if;

    for house_revision in
      select value from jsonb_array_elements(seat -> 'allowed_house_agent_revisions')
    loop
      if jsonb_typeof(house_revision) <> 'string'
        or house_revision #>> '{}' !~ '^blake3:[0-9a-f]{64}$'
      then
        return false;
      end if;
    end loop;
  end loop;

  return true;
end;
$$;

create table platform_store.activity_listing_revisions (
  listing_revision_digest text primary key,
  listing_key text not null,
  canonical_document bytea not null,
  pack_revision_digest text not null,
  client_release_digest text not null,
  client_surface_id text not null,
  catalog_visibility text not null,
  creator_access text not null,
  public_viewing_policy text not null,
  result_publication_policy text not null,
  result_projector_revision_digest text not null,
  public_projection_schema text not null,
  public_projection_schema_digest text not null,
  result_output_schema text not null,
  result_output_schema_digest text not null,
  result_canonicalizer_version text not null,
  result_output_max_bytes integer not null,
  room_setup_configuration jsonb not null,
  seat_templates jsonb not null,
  allow_multiple_seats_per_account boolean not null default false,
  pre_start_deadline_seconds integer not null,
  deployed_at timestamptz not null default clock_timestamp(),
  constraint activity_listing_revision_digest_shape
    check (listing_revision_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint activity_listing_key_bounds check (
    length(listing_key) between 1 and 128
    and listing_key ~ '^[a-z0-9][a-z0-9._-]*$'
  ),
  constraint activity_listing_document_bounds
    check (octet_length(canonical_document) between 2 and 262144),
  constraint activity_listing_pack_digest_shape
    check (pack_revision_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint activity_listing_client_digest_shape
    check (client_release_digest ~ '^(blake3|sha256):[0-9a-f]{64}$'),
  constraint activity_listing_surface_bounds check (
    length(client_surface_id) between 1 and 128
    and client_surface_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
  ),
  constraint activity_listing_visibility
    check (catalog_visibility in ('public', 'unlisted', 'private')),
  constraint activity_listing_creator_access
    check (creator_access in ('must_claim_seat', 'may_spectate')),
  constraint activity_listing_public_viewing
    check (public_viewing_policy in ('disabled', 'anonymous_by_link')),
  constraint activity_listing_result_publication
    check (result_publication_policy in ('disabled', 'public_recent_results')),
  constraint activity_listing_projector_digest_shape
    check (result_projector_revision_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint activity_listing_projection_schema_bounds check (
    length(public_projection_schema) between 1 and 128
    and public_projection_schema !~ '[[:cntrl:]]'
  ),
  constraint activity_listing_projection_schema_digest_shape
    check (public_projection_schema_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint activity_listing_output_schema_bounds check (
    length(result_output_schema) between 1 and 128
    and result_output_schema !~ '[[:cntrl:]]'
  ),
  constraint activity_listing_output_schema_digest_shape
    check (result_output_schema_digest ~ '^blake3:[0-9a-f]{64}$'),
  constraint activity_listing_canonicalizer
    check (result_canonicalizer_version = 'worldstream/canonical-json/v1'),
  constraint activity_listing_output_bound
    check (result_output_max_bytes between 2 and 65536),
  constraint activity_listing_room_setup_bounds check (
    jsonb_typeof(room_setup_configuration) = 'object'
    and octet_length(room_setup_configuration::text) between 2 and 32768
  ),
  constraint activity_listing_seats_valid
    check (platform_store.valid_listing_seats_v1(seat_templates)),
  constraint activity_listing_deadline_bound
    check (pre_start_deadline_seconds between 60 and 86400)
);

create table platform_store.launch_requests (
  launch_request_id uuid primary key default gen_random_uuid(),
  creator_account_id uuid not null
    references platform_store.platform_accounts(account_id) on delete restrict,
  listing_revision_digest text not null
    references platform_store.activity_listing_revisions(listing_revision_digest)
    on delete restrict,
  idempotency_namespace text not null,
  idempotency_key_digest bytea not null,
  canonical_launch_input bytea not null,
  launch_input_digest bytea not null,
  canonicalizer_version text not null,
  house_fill_choice text not null,
  creator_access_choice text not null,
  creator_seat_id text,
  creator_participation_kind text,
  state text not null default 'collecting_roster',
  expires_at timestamptz not null,
  created_at timestamptz not null default clock_timestamp(),
  last_transition_at timestamptz not null default clock_timestamp(),
  roster_frozen_at timestamptz,
  frozen_roster bytea,
  frozen_roster_digest bytea,
  frozen_room_setup_specification bytea,
  frozen_room_setup_specification_digest text,
  host_installation_id text,
  room_setup_operation_id text,
  host_mutation_started_at timestamptz,
  constraint launch_request_idempotency_namespace_bounds check (
    length(idempotency_namespace) between 1 and 64
    and idempotency_namespace ~ '^[A-Za-z0-9][A-Za-z0-9._-]*$'
  ),
  constraint launch_request_idempotency_digest_sha256
    check (octet_length(idempotency_key_digest) = 32),
  constraint launch_request_input_bounds
    check (octet_length(canonical_launch_input) between 2 and 16384),
  constraint launch_request_input_digest_sha256
    check (octet_length(launch_input_digest) = 32),
  constraint launch_request_canonicalizer
    check (canonicalizer_version = 'worldstream/canonical-json/v1'),
  constraint launch_request_house_fill
    check (house_fill_choice in ('disabled', 'fill_unclaimed')),
  constraint launch_request_creator_choice
    check (creator_access_choice in ('seat', 'spectator')),
  constraint launch_request_creator_seat check (
    (creator_access_choice = 'seat'
      and creator_seat_id is not null
      and creator_seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'
      and creator_participation_kind in ('account_human', 'account_external_agent'))
    or
    (creator_access_choice = 'spectator'
      and creator_seat_id is null
      and creator_participation_kind = 'account_human')
  ),
  constraint launch_request_state check (state in (
    'collecting_roster',
    'provisioning',
    'reconciling',
    'run_created',
    'cancelled',
    'expired',
    'failed_pre_genesis'
  )),
  constraint launch_request_exact_lifetime
    check (expires_at = created_at + interval '24 hours'),
  constraint launch_request_transition_order
    check (last_transition_at >= created_at),
  constraint launch_request_freeze_shape check (
    (roster_frozen_at is null
      and frozen_roster is null
      and frozen_roster_digest is null
      and frozen_room_setup_specification is null
      and frozen_room_setup_specification_digest is null
      and host_installation_id is null
      and room_setup_operation_id is null)
    or
    (roster_frozen_at is not null
      and roster_frozen_at >= created_at
      and octet_length(frozen_roster) between 2 and 65536
      and octet_length(frozen_roster_digest) = 32
      and octet_length(frozen_room_setup_specification) between 2 and 65536
      and frozen_room_setup_specification_digest ~ '^blake3:[0-9a-f]{64}$'
      and host_installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
      and room_setup_operation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$')
  ),
  constraint launch_request_host_mutation_order check (
    host_mutation_started_at is null
    or (
      roster_frozen_at is not null
      and host_mutation_started_at >= roster_frozen_at
    )
  ),
  constraint launch_request_account_idempotency unique (
    creator_account_id,
    idempotency_namespace,
    idempotency_key_digest
  ),
  constraint launch_request_host_operation unique (
    host_installation_id,
    room_setup_operation_id
  )
);

create index launch_requests_creator_state_expiry_idx
  on platform_store.launch_requests (creator_account_id, state, expires_at);
create index launch_requests_state_expiry_idx
  on platform_store.launch_requests (state, expires_at, launch_request_id);

create table platform_store.capacity_gates (
  gate_kind text primary key,
  hard_limit integer not null,
  created_at timestamptz not null default clock_timestamp(),
  constraint capacity_gate_single_kind check (gate_kind = 'active_run'),
  constraint capacity_gate_limit check (hard_limit between 1 and 100)
);

insert into platform_store.capacity_gates(gate_kind, hard_limit)
values ('active_run', 10);

create table platform_store.capacity_reservations (
  reservation_id uuid primary key default gen_random_uuid(),
  launch_request_id uuid not null
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  kind text not null,
  reservation_class text not null default 'authorized',
  controlling_account_id uuid not null
    references platform_store.platform_accounts(account_id) on delete restrict,
  activity_run_id uuid,
  reserved_at timestamptz not null default clock_timestamp(),
  released_at timestamptz,
  release_reason text,
  constraint capacity_reservation_kind
    check (kind in ('pre_genesis', 'active_run')),
  constraint capacity_reservation_class
    check (reservation_class in ('authorized', 'quarantined_recovery')),
  constraint capacity_reservation_run_shape check (
    (kind = 'pre_genesis' and activity_run_id is null)
    or kind = 'active_run'
  ),
  constraint capacity_reservation_release_shape check (
    (released_at is null and release_reason is null)
    or
    (released_at is not null
      and released_at >= reserved_at
      and release_reason in (
        'cancelled',
        'expired',
        'failed_pre_genesis',
        'genesis_observed',
        'abandoned_prestart',
        'projector_terminal'
      ))
  )
);

create unique index capacity_reservations_live_launch_kind_uq
  on platform_store.capacity_reservations (launch_request_id, kind)
  where released_at is null;
create unique index capacity_reservations_live_pre_genesis_account_uq
  on platform_store.capacity_reservations (controlling_account_id)
  where kind = 'pre_genesis'
    and reservation_class = 'authorized'
    and released_at is null;
create unique index capacity_reservations_live_active_account_uq
  on platform_store.capacity_reservations (controlling_account_id)
  where kind = 'active_run'
    and reservation_class = 'authorized'
    and released_at is null;
create index capacity_reservations_active_gate_idx
  on platform_store.capacity_reservations (
    kind,
    reservation_class,
    released_at,
    controlling_account_id,
    launch_request_id
  );
create unique index capacity_reservations_recovery_run_uq
  on platform_store.capacity_reservations (activity_run_id)
  where kind = 'active_run'
    and reservation_class = 'quarantined_recovery'
    and activity_run_id is not null;

create table platform_store.seat_invitations (
  invitation_id uuid primary key default gen_random_uuid(),
  launch_request_id uuid not null
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  seat_id text not null,
  generation integer not null,
  token_digest bytea not null unique,
  expires_at timestamptz not null,
  consumed_at timestamptz,
  revoked_at timestamptz,
  created_at timestamptz not null default clock_timestamp(),
  constraint seat_invitation_seat_bounds
    check (seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'),
  constraint seat_invitation_generation check (generation between 1 and 2147483647),
  constraint seat_invitation_digest_sha256 check (octet_length(token_digest) = 32),
  constraint seat_invitation_time_order check (
    expires_at > created_at
    and (consumed_at is null or consumed_at between created_at and expires_at)
    and (revoked_at is null or revoked_at >= created_at)
    and not (consumed_at is not null and revoked_at is not null)
  ),
  constraint seat_invitation_generation_unique
    unique (launch_request_id, seat_id, generation)
);

create unique index seat_invitations_live_seat_uq
  on platform_store.seat_invitations (launch_request_id, seat_id)
  where consumed_at is null and revoked_at is null;
create index seat_invitations_token_lookup_idx
  on platform_store.seat_invitations (token_digest, expires_at)
  where consumed_at is null and revoked_at is null;

create table platform_store.seat_claims (
  claim_id uuid primary key default gen_random_uuid(),
  launch_request_id uuid not null
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  seat_id text not null,
  role text not null,
  controlling_account_id uuid not null
    references platform_store.platform_accounts(account_id) on delete restrict,
  participation_kind text not null,
  principal_reference text not null,
  claimed_at timestamptz not null default clock_timestamp(),
  released_at timestamptz,
  release_reason text,
  constraint seat_claim_seat_bounds
    check (seat_id ~ '^[a-z][a-z0-9_-]{0,63}$'),
  constraint seat_claim_role_bounds
    check (role ~ '^[a-z][a-z0-9_-]{0,63}$'),
  constraint seat_claim_participation
    check (participation_kind in ('account_human', 'account_external_agent')),
  constraint seat_claim_principal_reference
    check (principal_reference = 'seat:' || seat_id),
  constraint seat_claim_release_shape check (
    (released_at is null and release_reason is null)
    or
    (released_at is not null
      and released_at >= claimed_at
      and release_reason in ('self_released', 'creator_reset', 'launch_cancelled', 'launch_expired'))
  )
);

create unique index seat_claims_live_seat_uq
  on platform_store.seat_claims (launch_request_id, seat_id)
  where released_at is null;
create index seat_claims_live_account_idx
  on platform_store.seat_claims (launch_request_id, controlling_account_id, seat_id)
  where released_at is null;

alter table platform_store.activity_listing_revisions enable row level security;
alter table platform_store.launch_requests enable row level security;
alter table platform_store.capacity_gates enable row level security;
alter table platform_store.capacity_reservations enable row level security;
alter table platform_store.seat_invitations enable row level security;
alter table platform_store.seat_claims enable row level security;

create function platform_store.reject_immutable_formation_row_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  raise exception using errcode = '23000', message = 'immutable_formation_row';
end;
$$;

create trigger protect_activity_listing_revision_v1
before update or delete on platform_store.activity_listing_revisions
for each row execute function platform_store.reject_immutable_formation_row_v1();

create trigger protect_capacity_gate_v1
before update or delete on platform_store.capacity_gates
for each row execute function platform_store.reject_immutable_formation_row_v1();

create function platform_store.protect_launch_request_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE'
    or new.launch_request_id <> old.launch_request_id
    or new.creator_account_id <> old.creator_account_id
    or new.listing_revision_digest <> old.listing_revision_digest
    or new.idempotency_namespace <> old.idempotency_namespace
    or new.idempotency_key_digest <> old.idempotency_key_digest
    or new.canonical_launch_input <> old.canonical_launch_input
    or new.launch_input_digest <> old.launch_input_digest
    or new.canonicalizer_version <> old.canonicalizer_version
    or new.house_fill_choice <> old.house_fill_choice
    or new.creator_access_choice <> old.creator_access_choice
    or new.creator_seat_id is distinct from old.creator_seat_id
    or new.creator_participation_kind is distinct from old.creator_participation_kind
    or new.expires_at <> old.expires_at
    or new.created_at <> old.created_at
    or new.last_transition_at < old.last_transition_at
    or old.roster_frozen_at is not null and (
      new.roster_frozen_at is distinct from old.roster_frozen_at
      or new.frozen_roster is distinct from old.frozen_roster
      or new.frozen_roster_digest is distinct from old.frozen_roster_digest
      or new.frozen_room_setup_specification is distinct from old.frozen_room_setup_specification
      or new.frozen_room_setup_specification_digest is distinct from old.frozen_room_setup_specification_digest
      or new.host_installation_id is distinct from old.host_installation_id
      or new.room_setup_operation_id is distinct from old.room_setup_operation_id
    )
    or old.host_mutation_started_at is not null
       and new.host_mutation_started_at is distinct from old.host_mutation_started_at
  then
    raise exception using errcode = '23000', message = 'immutable_launch_request';
  end if;

  if new.state <> old.state and not (
    (old.state = 'collecting_roster' and new.state in (
      'provisioning', 'cancelled', 'expired', 'failed_pre_genesis'
    ))
    or
    (old.state = 'provisioning' and new.state in (
      'reconciling', 'run_created', 'cancelled', 'expired', 'failed_pre_genesis'
    ))
    or
    (old.state = 'reconciling' and new.state in (
      'provisioning', 'run_created', 'failed_pre_genesis'
    ))
  ) then
    raise exception using errcode = '23000', message = 'invalid_launch_transition';
  end if;

  return new;
end;
$$;

create trigger protect_launch_request_v1
before update or delete on platform_store.launch_requests
for each row execute function platform_store.protect_launch_request_v1();

create function platform_store.protect_capacity_reservation_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if tg_op = 'DELETE'
    or new.reservation_id <> old.reservation_id
    or new.launch_request_id <> old.launch_request_id
    or new.kind <> old.kind
    or new.reservation_class <> old.reservation_class
    or new.controlling_account_id <> old.controlling_account_id
    or new.reserved_at <> old.reserved_at
    or old.activity_run_id is not null
       and new.activity_run_id is distinct from old.activity_run_id
    or old.released_at is not null
       and (
         new.released_at is distinct from old.released_at
         or new.release_reason is distinct from old.release_reason
       )
  then
    raise exception using errcode = '23000', message = 'immutable_capacity_reservation';
  end if;
  return new;
end;
$$;

create trigger protect_capacity_reservation_v1
before update or delete on platform_store.capacity_reservations
for each row execute function platform_store.protect_capacity_reservation_v1();

create function platform_store.protect_seat_invitation_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
declare
  frozen_at timestamptz;
begin
  if tg_op = 'DELETE' then
    raise exception using errcode = '23000', message = 'immutable_seat_invitation';
  end if;

  select launches.roster_frozen_at
  into frozen_at
  from platform_store.launch_requests launches
  where launches.launch_request_id = new.launch_request_id;
  if frozen_at is not null then
    raise exception using errcode = '23000', message = 'launch_roster_frozen';
  end if;

  if tg_op = 'UPDATE' and (
    new.invitation_id <> old.invitation_id
    or new.launch_request_id <> old.launch_request_id
    or new.seat_id <> old.seat_id
    or new.generation <> old.generation
    or new.token_digest <> old.token_digest
    or new.expires_at <> old.expires_at
    or new.created_at <> old.created_at
    or old.consumed_at is not null and new.consumed_at is distinct from old.consumed_at
    or old.revoked_at is not null and new.revoked_at is distinct from old.revoked_at
  ) then
    raise exception using errcode = '23000', message = 'immutable_seat_invitation';
  end if;

  return new;
end;
$$;

create trigger protect_seat_invitation_v1
before insert or update or delete on platform_store.seat_invitations
for each row execute function platform_store.protect_seat_invitation_v1();

create function platform_store.protect_seat_claim_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
declare
  frozen_at timestamptz;
begin
  if tg_op = 'DELETE' then
    raise exception using errcode = '23000', message = 'immutable_seat_claim';
  end if;

  select launches.roster_frozen_at
  into frozen_at
  from platform_store.launch_requests launches
  where launches.launch_request_id = new.launch_request_id;
  if frozen_at is not null then
    raise exception using errcode = '23000', message = 'launch_roster_frozen';
  end if;

  if tg_op = 'UPDATE' and (
    new.claim_id <> old.claim_id
    or new.launch_request_id <> old.launch_request_id
    or new.seat_id <> old.seat_id
    or new.role <> old.role
    or new.controlling_account_id <> old.controlling_account_id
    or new.participation_kind <> old.participation_kind
    or new.principal_reference <> old.principal_reference
    or new.claimed_at <> old.claimed_at
    or old.released_at is not null and (
      new.released_at is distinct from old.released_at
      or new.release_reason is distinct from old.release_reason
    )
  ) then
    raise exception using errcode = '23000', message = 'immutable_seat_claim';
  end if;

  return new;
end;
$$;

create trigger protect_seat_claim_v1
before insert or update or delete on platform_store.seat_claims
for each row execute function platform_store.protect_seat_claim_v1();

create function platform_store.release_launch_capacity_v1(
  p_launch_request_id uuid,
  p_reason text
)
returns integer
language plpgsql
security invoker
set search_path = ''
as $$
declare
  released integer;
begin
  if p_reason not in ('cancelled', 'expired', 'failed_pre_genesis', 'abandoned_prestart') then
    raise exception using errcode = '22023', message = 'invalid_capacity_release';
  end if;

  perform reservations.reservation_id
  from platform_store.capacity_reservations reservations
  where reservations.launch_request_id = p_launch_request_id
    and reservations.released_at is null
  order by reservations.reservation_id
  for update;

  update platform_store.capacity_reservations reservations
  set released_at = clock_timestamp(), release_reason = p_reason
  where reservations.launch_request_id = p_launch_request_id
    and reservations.released_at is null;
  get diagnostics released = row_count;
  return released;
end;
$$;

create function platform_api.create_launch_request_v1(
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
      from jsonb_array_elements(listing_row.seat_templates) seats(value)
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
    from jsonb_array_elements(listing_row.seat_templates) seats(value)
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

create function platform_api.read_launch_request_v1(
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
  from jsonb_array_elements(listing_row.seat_templates)
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

create function platform_api.cancel_launch_request_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found or launch_row.creator_account_id <> p_creator_account_id then
    return false;
  end if;
  if launch_row.state = 'cancelled' then
    return true;
  end if;
  if launch_row.host_mutation_started_at is not null
    or launch_row.state not in ('collecting_roster', 'provisioning')
  then
    raise exception using errcode = '55000', message = 'launch_cannot_cancel';
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  where accounts.account_id = launch_row.creator_account_id
  for update;

  if launch_row.roster_frozen_at is null then
    update platform_store.seat_invitations invitations
    set revoked_at = sampled_at
    where invitations.launch_request_id = p_launch_request_id
      and invitations.consumed_at is null
      and invitations.revoked_at is null;
    update platform_store.seat_claims claims
    set released_at = sampled_at, release_reason = 'launch_cancelled'
    where claims.launch_request_id = p_launch_request_id
      and claims.released_at is null;
  end if;

  perform platform_store.release_launch_capacity_v1(p_launch_request_id, 'cancelled');
  update platform_store.launch_requests launches
  set state = 'cancelled', last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

create function platform_api.expire_launch_request_v1(p_launch_request_id uuid)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then
    return false;
  end if;
  if launch_row.state = 'expired' then
    return true;
  end if;
  if launch_row.expires_at > sampled_at
    or launch_row.host_mutation_started_at is not null
    or launch_row.state not in ('collecting_roster', 'provisioning')
  then
    return false;
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  where accounts.account_id = launch_row.creator_account_id
  for update;

  if launch_row.roster_frozen_at is null then
    update platform_store.seat_invitations invitations
    set revoked_at = sampled_at
    where invitations.launch_request_id = p_launch_request_id
      and invitations.consumed_at is null
      and invitations.revoked_at is null;
    update platform_store.seat_claims claims
    set released_at = sampled_at, release_reason = 'launch_expired'
    where claims.launch_request_id = p_launch_request_id
      and claims.released_at is null;
  end if;

  perform platform_store.release_launch_capacity_v1(p_launch_request_id, 'expired');
  update platform_store.launch_requests launches
  set state = 'expired', last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

create function platform_api.rotate_seat_invitation_v1(
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

  select listings.seat_templates
  into listing_seats
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;
  if not exists (
    select 1
    from jsonb_array_elements(listing_seats) seats(value)
    where seats.value ->> 'seat_id' = p_seat_id
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

create function platform_api.claim_invited_seat_v1(
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
  from jsonb_array_elements(listing_row.seat_templates) seats(value)
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

create function platform_api.release_seat_claim_v1(
  p_claiming_account_id uuid,
  p_launch_request_id uuid,
  p_seat_id text
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found
    or launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.expires_at <= sampled_at
  then
    return false;
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  where accounts.account_id = p_claiming_account_id
    and accounts.erased_at is null
  for update;
  if not found then
    return false;
  end if;

  update platform_store.seat_claims claims
  set released_at = sampled_at, release_reason = 'self_released'
  where claims.launch_request_id = p_launch_request_id
    and claims.seat_id = p_seat_id
    and claims.controlling_account_id = p_claiming_account_id
    and claims.released_at is null;
  return found;
end;
$$;

create function platform_api.reset_seat_claim_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid,
  p_seat_id text
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found
    or launch_row.creator_account_id <> p_creator_account_id
    or launch_row.state <> 'collecting_roster'
    or launch_row.roster_frozen_at is not null
    or launch_row.expires_at <= sampled_at
  then
    return false;
  end if;

  perform accounts.account_id
  from platform_store.platform_accounts accounts
  where accounts.account_id = p_creator_account_id
    and accounts.erased_at is null
  for update;
  if not found then
    return false;
  end if;

  update platform_store.seat_claims claims
  set released_at = sampled_at, release_reason = 'creator_reset'
  where claims.launch_request_id = p_launch_request_id
    and claims.seat_id = p_seat_id
    and claims.released_at is null;
  return found;
end;
$$;

create function platform_api.freeze_launch_request_v1(
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
  active_claim_count integer;
  sampled_at timestamptz := clock_timestamp();
begin
  if octet_length(p_frozen_roster) not between 2 and 65536
    or octet_length(p_frozen_roster_digest) <> 32
    or p_frozen_roster_digest <> extensions.digest(p_frozen_roster, 'sha256')
    or octet_length(p_frozen_room_setup_specification) not between 2 and 65536
    or p_frozen_room_setup_specification_digest !~ '^blake3:[0-9a-f]{64}$'
    or p_host_installation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or p_room_setup_operation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
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
  into listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = launch_row.listing_revision_digest;

  if exists (
    select 1
    from jsonb_array_elements(listing_row.seat_templates) seats(value)
    where (seats.value ->> 'required')::boolean
      and not exists (
        select 1
        from platform_store.seat_claims claims
        where claims.launch_request_id = p_launch_request_id
          and claims.seat_id = seats.value ->> 'seat_id'
          and claims.released_at is null
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

  select count(*)::integer
  into active_claim_count
  from platform_store.seat_claims claims
  where claims.launch_request_id = p_launch_request_id
    and claims.released_at is null;

  if jsonb_typeof(parsed_roster) <> 'object'
    or parsed_roster ->> 'schema' <> 'worldstream/frozen-roster/v1'
    or parsed_roster ->> 'listing_revision_digest' <> launch_row.listing_revision_digest
    or jsonb_typeof(parsed_roster -> 'members') <> 'array'
    or jsonb_array_length(parsed_roster -> 'members') <> active_claim_count
    or (
      select count(*) <> count(distinct member ->> 'seat_id')
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
    )
  then
    raise exception using errcode = '22023', message = 'frozen_roster_mismatch';
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
        where spectators.value ->> 'purpose' = 'creator_spectator'
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

create function platform_api.authorize_host_mutation_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid,
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
  gate_limit integer;
  active_count integer;
  sampled_at timestamptz := clock_timestamp();
begin
  select launches.*
  into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found or launch_row.creator_account_id <> p_creator_account_id then
    return false;
  end if;
  if launch_row.roster_frozen_at is null
    or launch_row.state not in ('provisioning', 'reconciling')
    or launch_row.host_installation_id <> p_host_installation_id
    or launch_row.room_setup_operation_id <> p_room_setup_operation_id
  then
    raise exception using errcode = '55000', message = 'frozen_launch_mismatch';
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

  select gates.hard_limit
  into strict gate_limit
  from platform_store.capacity_gates gates
  where gates.gate_kind = 'active_run'
  for update;

  perform reservations.reservation_id
  from platform_store.capacity_reservations reservations
  where reservations.launch_request_id = p_launch_request_id
    or reservations.controlling_account_id = launch_row.creator_account_id
  order by reservations.reservation_id
  for update;

  if launch_row.host_mutation_started_at is not null then
    if exists (
      select 1
      from platform_store.capacity_reservations reservations
      where reservations.launch_request_id = p_launch_request_id
        and reservations.kind = 'active_run'
        and reservations.reservation_class = 'authorized'
        and reservations.released_at is null
    ) then
      return true;
    end if;
    raise exception using errcode = '55000', message = 'host_authorization_incomplete';
  end if;

  if launch_row.expires_at <= sampled_at then
    raise exception using errcode = '55000', message = 'launch_expired';
  end if;
  if not exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.launch_request_id = p_launch_request_id
      and reservations.kind = 'pre_genesis'
      and reservations.reservation_class = 'authorized'
      and reservations.released_at is null
  ) then
    raise exception using errcode = '55000', message = 'pre_genesis_capacity_missing';
  end if;
  if exists (
    select 1
    from platform_store.capacity_reservations reservations
    where reservations.controlling_account_id = launch_row.creator_account_id
      and reservations.kind = 'active_run'
      and reservations.reservation_class = 'authorized'
      and reservations.released_at is null
      and reservations.launch_request_id <> p_launch_request_id
  ) then
    raise exception using errcode = '55000', message = 'account_active_run_capacity_unavailable';
  end if;

  select count(*)::integer
  into active_count
  from platform_store.capacity_reservations reservations
  where reservations.kind = 'active_run'
    and reservations.released_at is null;
  if active_count >= gate_limit then
    raise exception using errcode = '55000', message = 'global_active_run_capacity_unavailable';
  end if;

  insert into platform_store.capacity_reservations (
    launch_request_id,
    kind,
    reservation_class,
    controlling_account_id,
    reserved_at
  ) values (
    p_launch_request_id,
    'active_run',
    'authorized',
    launch_row.creator_account_id,
    sampled_at
  );

  update platform_store.launch_requests launches
  set host_mutation_started_at = sampled_at,
      last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

revoke all on platform_store.activity_listing_revisions
  from public, anon, authenticated, service_role;
revoke all on platform_store.launch_requests
  from public, anon, authenticated, service_role;
revoke all on platform_store.capacity_gates
  from public, anon, authenticated, service_role;
revoke all on platform_store.capacity_reservations
  from public, anon, authenticated, service_role;
revoke all on platform_store.seat_invitations
  from public, anon, authenticated, service_role;
revoke all on platform_store.seat_claims
  from public, anon, authenticated, service_role;

revoke execute on function platform_store.valid_listing_seats_v1(jsonb)
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.reject_immutable_formation_row_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_launch_request_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_capacity_reservation_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_seat_invitation_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.protect_seat_claim_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.release_launch_capacity_v1(uuid, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.create_launch_request_v1(
  uuid, text, text, bytea, bytea, bytea, text, text, text, text, text
) from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_launch_request_v1(uuid, uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.cancel_launch_request_v1(uuid, uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.expire_launch_request_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.rotate_seat_invitation_v1(uuid, uuid, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.claim_invited_seat_v1(uuid, bytea, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.release_seat_claim_v1(uuid, uuid, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.reset_seat_claim_v1(uuid, uuid, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.freeze_launch_request_v1(
  uuid, uuid, bytea, bytea, bytea, text, text, text
) from public, anon, authenticated, service_role;
revoke execute on function platform_api.authorize_host_mutation_v1(uuid, uuid, text, text)
  from public, anon, authenticated, service_role;

grant select, insert on platform_store.activity_listing_revisions to service_role;
grant select, insert on platform_store.launch_requests to service_role;
grant update (
  state,
  last_transition_at,
  roster_frozen_at,
  frozen_roster,
  frozen_roster_digest,
  frozen_room_setup_specification,
  frozen_room_setup_specification_digest,
  host_installation_id,
  room_setup_operation_id,
  host_mutation_started_at
) on platform_store.launch_requests to service_role;
grant select on platform_store.capacity_gates to service_role;
grant update (created_at) on platform_store.capacity_gates to service_role;
grant select, insert on platform_store.capacity_reservations to service_role;
grant update (activity_run_id, released_at, release_reason)
  on platform_store.capacity_reservations to service_role;
grant select, insert on platform_store.seat_invitations to service_role;
grant update (consumed_at, revoked_at) on platform_store.seat_invitations to service_role;
grant select, insert on platform_store.seat_claims to service_role;
grant update (released_at, release_reason) on platform_store.seat_claims to service_role;

grant execute on function platform_store.valid_listing_seats_v1(jsonb) to service_role;
grant execute on function platform_store.release_launch_capacity_v1(uuid, text)
  to service_role;
grant execute on function platform_api.create_launch_request_v1(
  uuid, text, text, bytea, bytea, bytea, text, text, text, text, text
) to service_role;
grant execute on function platform_api.read_launch_request_v1(uuid, uuid)
  to service_role;
grant execute on function platform_api.cancel_launch_request_v1(uuid, uuid)
  to service_role;
grant execute on function platform_api.expire_launch_request_v1(uuid)
  to service_role;
grant execute on function platform_api.rotate_seat_invitation_v1(uuid, uuid, text)
  to service_role;
grant execute on function platform_api.claim_invited_seat_v1(uuid, bytea, text)
  to service_role;
grant execute on function platform_api.release_seat_claim_v1(uuid, uuid, text)
  to service_role;
grant execute on function platform_api.reset_seat_claim_v1(uuid, uuid, text)
  to service_role;
grant execute on function platform_api.freeze_launch_request_v1(
  uuid, uuid, bytea, bytea, bytea, text, text, text
) to service_role;
grant execute on function platform_api.authorize_host_mutation_v1(uuid, uuid, text, text)
  to service_role;
