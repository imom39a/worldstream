-- Freeze the complete mixed human/external/House roster that the Host will
-- independently verify before it creates or resumes one Room operation.
alter table platform_store.launch_requests
  add constraint launch_request_host_operation_runtime_shape check (
    room_setup_operation_id is null
    or room_setup_operation_id ~ '^[a-z][a-z0-9-]{0,63}$'
  );

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
          from jsonb_array_elements(listing_row.seat_templates) seats(value)
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
    from jsonb_array_elements(listing_row.seat_templates) seats(value)
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
