-- A creator closes one hosted launch lineage, not an arbitrary Room. The
-- platform first freezes that intent, the Host then prevents future setup and
-- archives any canonical Room, and capacity is released only after the exact
-- Host evidence is retained here.

alter table platform_store.launch_requests
  drop constraint launch_request_state;

alter table platform_store.launch_requests
  add constraint launch_request_state check (state in (
    'collecting_roster',
    'provisioning',
    'reconciling',
    'run_created',
    'closing',
    'closed_by_creator',
    'cancelled',
    'expired',
    'failed_pre_genesis',
    'abandoned_prestart'
  ));

alter table platform_store.capacity_reservations
  drop constraint capacity_reservation_release_shape;

alter table platform_store.capacity_reservations
  add constraint capacity_reservation_release_shape check (
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
        'creator_closed',
        'projector_terminal'
      ))
  );

create or replace function platform_store.release_launch_capacity_v1(
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
  if p_reason not in (
    'cancelled', 'expired', 'failed_pre_genesis', 'abandoned_prestart',
    'creator_closed'
  ) then
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

create or replace function platform_store.protect_launch_request_v1()
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
      'provisioning', 'cancelled', 'expired', 'failed_pre_genesis', 'closing'
    ))
    or
    (old.state = 'provisioning' and new.state in (
      'reconciling', 'run_created', 'cancelled', 'expired',
      'failed_pre_genesis', 'closing'
    ))
    or
    (old.state = 'reconciling' and new.state in (
      'provisioning', 'run_created', 'failed_pre_genesis', 'closing'
    ))
    or
    (old.state = 'run_created' and new.state in ('abandoned_prestart', 'closing'))
    or
    (old.state = 'closing' and new.state = 'closed_by_creator')
  ) then
    raise exception using errcode = '23000', message = 'invalid_launch_transition';
  end if;

  return new;
end;
$$;

create table platform_store.launch_closure_intents (
  launch_request_id uuid primary key
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  host_installation_id text not null,
  launch_request_digest text not null,
  room_setup_operation_id text not null,
  canonical_closure_request bytea not null,
  closure_request_digest bytea not null,
  requested_at timestamptz not null default clock_timestamp(),
  constraint launch_closure_intent_host_shape check (
    host_installation_id ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
  ),
  constraint launch_closure_intent_launch_digest_shape check (
    launch_request_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint launch_closure_intent_operation_shape check (
    room_setup_operation_id ~ '^[a-z][a-z0-9-]{0,63}$'
  ),
  constraint launch_closure_intent_request_shape check (
    octet_length(canonical_closure_request) between 2 and 8192
    and octet_length(closure_request_digest) = 32
  )
);

create table platform_store.launch_closures (
  launch_request_id uuid primary key
    references platform_store.launch_requests(launch_request_id) on delete restrict,
  activity_run_id uuid unique
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  disposition text not null,
  room_id text,
  canonical_closure_evidence bytea not null,
  closure_evidence_digest bytea not null unique,
  recorded_at timestamptz not null default clock_timestamp(),
  constraint launch_closure_disposition check (
    disposition in ('cancelled_before_genesis', 'room_archived')
  ),
  constraint launch_closure_room_shape check (
    (disposition = 'cancelled_before_genesis' and room_id is null and activity_run_id is null)
    or
    (disposition = 'room_archived' and room_id ~ '^[0-7][0-9A-HJKMNP-TV-Z]{25}$')
  ),
  constraint launch_closure_evidence_shape check (
    octet_length(canonical_closure_evidence) between 2 and 16384
    and octet_length(closure_evidence_digest) = 32
  )
);

alter table platform_store.launch_closure_intents enable row level security;
alter table platform_store.launch_closures enable row level security;

create trigger protect_launch_closure_intent_v1
before update or delete on platform_store.launch_closure_intents
for each row execute function platform_store.reject_immutable_formation_row_v1();

create trigger protect_launch_closure_v1
before update or delete on platform_store.launch_closures
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

-- Once a close wins the launch-row lock, a late Genesis recorder may not
-- manufacture a quarantined Run or reacquire capacity behind that fence.
create function platform_store.reject_activity_run_for_closed_launch_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_state text;
begin
  select launches.state into launch_state
  from platform_store.launch_requests launches
  where launches.launch_request_id = new.launch_request_id
  for update;
  if launch_state in ('closing', 'closed_by_creator') then
    raise exception using errcode = '55000', message = 'launch_closure_in_progress';
  end if;
  return new;
end;
$$;

create trigger reject_activity_run_for_closed_launch_v1
before insert on platform_store.activity_runs
for each row execute function platform_store.reject_activity_run_for_closed_launch_v1();

-- Serialize late House reservation results with the same launch row that owns
-- the closure intent. Whichever operation wins is then visible to Host cleanup;
-- a response arriving after the close fence cannot recreate live bookkeeping.
create function platform_store.reject_house_runner_result_for_closed_launch_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_state text;
begin
  if new.state = old.state
    or new.state not in ('succeeded', 'terminal_failed')
  then return new;
  end if;
  select launches.state into launch_state
  from platform_store.launch_requests launches
  where launches.launch_request_id = new.launch_request_id
  for update;
  if launch_state in ('closing', 'closed_by_creator') then
    raise exception using errcode = '55000', message = 'launch_closure_in_progress';
  end if;
  return new;
end;
$$;

create trigger reject_house_runner_result_for_closed_launch_v1
before update on platform_store.house_runner_reservations
for each row execute function
  platform_store.reject_house_runner_result_for_closed_launch_v1();

create function platform_store.reject_house_assignment_for_closed_launch_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_state text;
begin
  select launches.state into launch_state
  from platform_store.launch_requests launches
  where launches.launch_request_id = new.launch_request_id
  for update;
  if launch_state in ('closing', 'closed_by_creator') then
    raise exception using errcode = '55000', message = 'launch_closure_in_progress';
  end if;
  return new;
end;
$$;

create trigger reject_house_assignment_for_closed_launch_v1
before insert on platform_store.house_agent_assignments
for each row execute function
  platform_store.reject_house_assignment_for_closed_launch_v1();

create function platform_api.request_launch_closure_v1(
  p_creator_account_id uuid,
  p_launch_request_id uuid,
  p_host_installation_id text,
  p_canonical_closure_request bytea,
  p_closure_request_digest bytea
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  run_row platform_store.activity_runs%rowtype;
  existing platform_store.launch_closure_intents%rowtype;
  request_document jsonb;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_creator_account_id is null
    or p_launch_request_id is null
    or p_host_installation_id is null
    or p_host_installation_id !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or p_canonical_closure_request is null
    or octet_length(p_canonical_closure_request) not between 2 and 8192
    or p_closure_request_digest is null
    or octet_length(p_closure_request_digest) <> 32
    or p_closure_request_digest <> extensions.digest(p_canonical_closure_request, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_request';
  end if;
  begin
    request_document := convert_from(p_canonical_closure_request, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_request';
  end;
  if jsonb_typeof(request_document) <> 'object'
    or not (request_document ?& array[
      'schema', 'launch_request_id', 'listing_revision_digest',
      'launch_request_digest', 'room_setup_operation_id'
    ])
    or exists (
      select 1 from jsonb_object_keys(request_document) keys(key)
      where keys.key not in (
        'schema', 'launch_request_id', 'listing_revision_digest',
        'launch_request_digest', 'room_setup_operation_id'
      )
    )
    or coalesce(request_document ->> 'schema', '')
      <> 'worldstream/hosted-launch-closure-request/v1'
    or coalesce(request_document ->> 'launch_request_id', '')
      <> p_launch_request_id::text
    or coalesce(request_document ->> 'launch_request_digest', '')
      !~ '^blake3:[0-9a-f]{64}$'
    or coalesce(request_document ->> 'listing_revision_digest', '')
      !~ '^blake3:[0-9a-f]{64}$'
    or coalesce(request_document ->> 'room_setup_operation_id', '')
      !~ '^[a-z][a-z0-9-]{0,63}$'
  then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_request';
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('launch-closure:' || p_launch_request_id::text, 0)
  );
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found or launch_row.creator_account_id <> p_creator_account_id then
    return false;
  end if;

  select intents.* into existing
  from platform_store.launch_closure_intents intents
  where intents.launch_request_id = p_launch_request_id;
  if found then
    return existing.host_installation_id = p_host_installation_id
      and existing.canonical_closure_request = p_canonical_closure_request
      and existing.closure_request_digest = p_closure_request_digest
      and launch_row.state in ('closing', 'closed_by_creator');
  end if;

  if launch_row.state not in (
    'collecting_roster', 'provisioning', 'reconciling', 'run_created'
  )
    or request_document ->> 'listing_revision_digest' <> launch_row.listing_revision_digest
    or (launch_row.host_installation_id is not null
      and launch_row.host_installation_id <> p_host_installation_id)
    or (launch_row.room_setup_operation_id is not null
      and launch_row.room_setup_operation_id <> request_document ->> 'room_setup_operation_id')
    or (launch_row.room_setup_operation_id is null
      and request_document ->> 'room_setup_operation_id'
        <> 'launch-' || replace(p_launch_request_id::text, '-', ''))
  then
    return false;
  end if;

  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.launch_request_id = p_launch_request_id;
  if found and (
    run_row.host_installation_id <> p_host_installation_id
    or run_row.launch_request_digest <> request_document ->> 'launch_request_digest'
    or run_row.room_setup_operation_id <> request_document ->> 'room_setup_operation_id'
  ) then
    return false;
  end if;

  insert into platform_store.launch_closure_intents (
    launch_request_id, host_installation_id, launch_request_digest,
    room_setup_operation_id, canonical_closure_request,
    closure_request_digest, requested_at
  ) values (
    p_launch_request_id, p_host_installation_id,
    request_document ->> 'launch_request_digest',
    request_document ->> 'room_setup_operation_id',
    p_canonical_closure_request, p_closure_request_digest, sampled_at
  );
  update platform_store.launch_requests launches
  set state = 'closing', last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

create function platform_api.record_launch_closure_v1(
  p_launch_request_id uuid,
  p_canonical_closure_evidence bytea,
  p_closure_evidence_digest bytea
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  launch_row platform_store.launch_requests%rowtype;
  intent_row platform_store.launch_closure_intents%rowtype;
  run_row platform_store.activity_runs%rowtype;
  existing platform_store.launch_closures%rowtype;
  evidence jsonb;
  disposition text;
  room_id text;
  sampled_at timestamptz := clock_timestamp();
begin
  if p_launch_request_id is null
    or p_canonical_closure_evidence is null
    or octet_length(p_canonical_closure_evidence) not between 2 and 16384
    or p_closure_evidence_digest is null
    or octet_length(p_closure_evidence_digest) <> 32
    or p_closure_evidence_digest <> extensions.digest(p_canonical_closure_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_evidence';
  end if;
  begin
    evidence := convert_from(p_canonical_closure_evidence, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_evidence';
  end;
  if jsonb_typeof(evidence) <> 'object'
    or not (evidence ?& array[
      'schema', 'host_installation_id', 'launch_request_id',
      'listing_revision_digest', 'launch_request_digest',
      'room_setup_operation_id', 'disposition', 'room_id', 'room_head',
      'closure_fence_digest', 'authentication_tag'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema', 'host_installation_id', 'launch_request_id',
        'listing_revision_digest', 'launch_request_digest',
        'room_setup_operation_id', 'disposition', 'room_id', 'room_head',
        'closure_fence_digest', 'authentication_tag'
      )
    )
    or coalesce(evidence ->> 'schema', '')
      <> 'worldstream/hosted-launch-closure-evidence/v1'
    or coalesce(evidence ->> 'launch_request_id', '') <> p_launch_request_id::text
    or coalesce(evidence ->> 'host_installation_id', '')
      !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or coalesce(evidence ->> 'listing_revision_digest', '')
      !~ '^blake3:[0-9a-f]{64}$'
    or coalesce(evidence ->> 'launch_request_digest', '')
      !~ '^blake3:[0-9a-f]{64}$'
    or coalesce(evidence ->> 'room_setup_operation_id', '')
      !~ '^[a-z][a-z0-9-]{0,63}$'
    or coalesce(evidence ->> 'closure_fence_digest', '')
      !~ '^blake3:[0-9a-f]{64}$'
    or coalesce(evidence ->> 'authentication_tag', '') !~ '^[0-9a-f]{64}$'
  then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_evidence';
  end if;
  disposition := evidence ->> 'disposition';
  room_id := evidence ->> 'room_id';
  if not coalesce((
    (disposition = 'cancelled_before_genesis'
      and evidence -> 'room_id' = 'null'::jsonb
      and evidence -> 'room_head' = 'null'::jsonb)
    or
    (disposition = 'room_archived'
      and room_id ~ '^[0-7][0-9A-HJKMNP-TV-Z]{25}$'
      and jsonb_typeof(evidence -> 'room_head') = 'object'
      and (evidence -> 'room_head') ?& array[
        'room_id', 'room_seq', 'genesis_or_transition_hash',
        'core_schema_version', 'pack_digest', 'core_state_hash',
        'activity_state_hash', 'authoritative_state_hash'
      ]
      and not exists (
        select 1 from jsonb_object_keys(evidence -> 'room_head') keys(key)
        where keys.key not in (
          'room_id', 'room_seq', 'genesis_or_transition_hash',
          'core_schema_version', 'pack_digest', 'core_state_hash',
          'activity_state_hash', 'authoritative_state_hash'
        )
      )
      and evidence #>> '{room_head,room_id}' = room_id
      and jsonb_typeof(evidence #> '{room_head,room_seq}') = 'number'
      and (evidence #>> '{room_head,room_seq}')::numeric between 1 and 9007199254740991
      and evidence #>> '{room_head,core_schema_version}' = 'worldstream.core-room-state.v1'
      and evidence #>> '{room_head,genesis_or_transition_hash}' ~ '^blake3:[0-9a-f]{64}$'
      and evidence #>> '{room_head,pack_digest}' ~ '^blake3:[0-9a-f]{64}$'
      and evidence #>> '{room_head,core_state_hash}' ~ '^blake3:[0-9a-f]{64}$'
      and evidence #>> '{room_head,activity_state_hash}' ~ '^blake3:[0-9a-f]{64}$'
      and evidence #>> '{room_head,authoritative_state_hash}' ~ '^blake3:[0-9a-f]{64}$')
  ), false) then
    raise exception using errcode = '22023', message = 'invalid_launch_closure_evidence';
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('launch-closure:' || p_launch_request_id::text, 0)
  );
  select launches.* into launch_row
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
  for update;
  if not found then return false; end if;
  select intents.* into intent_row
  from platform_store.launch_closure_intents intents
  where intents.launch_request_id = p_launch_request_id;
  if not found then return false; end if;
  select closures.* into existing
  from platform_store.launch_closures closures
  where closures.launch_request_id = p_launch_request_id;
  if found then
    return existing.canonical_closure_evidence = p_canonical_closure_evidence
      and existing.closure_evidence_digest = p_closure_evidence_digest
      and launch_row.state = 'closed_by_creator';
  end if;
  if launch_row.state <> 'closing'
    or evidence ->> 'host_installation_id' <> intent_row.host_installation_id
    or evidence ->> 'launch_request_id' <> intent_row.launch_request_id::text
    or evidence ->> 'listing_revision_digest' <> launch_row.listing_revision_digest
    or evidence ->> 'launch_request_digest' <> intent_row.launch_request_digest
    or evidence ->> 'room_setup_operation_id' <> intent_row.room_setup_operation_id
  then return false;
  end if;

  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.launch_request_id = p_launch_request_id;
  if found and (
    disposition <> 'room_archived'
    or run_row.host_installation_id <> intent_row.host_installation_id
    or run_row.room_id <> room_id
    or run_row.launch_request_digest <> intent_row.launch_request_digest
    or run_row.room_setup_operation_id <> intent_row.room_setup_operation_id
    or evidence #>> '{room_head,pack_digest}' <> run_row.pack_revision_digest
  ) then return false;
  end if;

  insert into platform_store.launch_closures (
    launch_request_id, activity_run_id, disposition, room_id,
    canonical_closure_evidence, closure_evidence_digest, recorded_at
  ) values (
    p_launch_request_id, run_row.activity_run_id, disposition, room_id,
    p_canonical_closure_evidence, p_closure_evidence_digest, sampled_at
  );

  -- The Host emits launch-closure evidence only after every retained House
  -- child for this launch is stopped or proved unable to start.
  update platform_store.house_runner_reservations reservations
  set state = 'released', released_at = sampled_at,
      release_receipt_digest = p_closure_evidence_digest,
      last_transition_at = sampled_at
  where reservations.launch_request_id = p_launch_request_id
    and reservations.state = 'succeeded'
    and reservations.released_at is null;
  perform platform_store.release_launch_capacity_v1(
    p_launch_request_id, 'creator_closed'
  );
  update platform_store.launch_requests launches
  set state = 'closed_by_creator', last_transition_at = sampled_at
  where launches.launch_request_id = p_launch_request_id;
  return true;
end;
$$;

create or replace function platform_api.read_hosted_recovery_material_v1(
  p_launch_request_id uuid
)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select platform_api.read_hosted_launch_material_v1(
    launches.creator_account_id, launches.launch_request_id
  )
  from platform_store.launch_requests launches
  where launches.launch_request_id = p_launch_request_id
    and (
      launches.state = 'closing'
      or (
        launches.host_mutation_started_at is not null
        and launches.roster_frozen_at is not null
        and launches.state in ('provisioning', 'reconciling', 'run_created')
        and not exists (
          select 1 from platform_store.reconciliation_receipts receipts
          where receipts.launch_request_id = launches.launch_request_id
            and receipts.disposition = 'conflict'
        )
      )
    );
$$;

create function platform_api.list_pending_launch_closures_v1(p_limit integer)
returns table(launch_request_id uuid)
language sql
stable
security invoker
set search_path = ''
as $$
  select launches.launch_request_id
  from platform_store.launch_requests launches
  join platform_store.launch_closure_intents intents
    on intents.launch_request_id = launches.launch_request_id
  where launches.state = 'closing'
    and not exists (
      select 1 from platform_store.launch_closures closures
      where closures.launch_request_id = launches.launch_request_id
    )
  order by intents.requested_at, launches.launch_request_id
  limit least(greatest(coalesce(p_limit, 0), 0), 100);
$$;

revoke all on platform_store.launch_closure_intents
  from public, anon, authenticated, service_role;
revoke all on platform_store.launch_closures
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.reject_activity_run_for_closed_launch_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.reject_house_runner_result_for_closed_launch_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.reject_house_assignment_for_closed_launch_v1()
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.request_launch_closure_v1(
  uuid, uuid, text, bytea, bytea
) from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_launch_closure_v1(uuid, bytea, bytea)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.list_pending_launch_closures_v1(integer)
  from public, anon, authenticated, service_role;

grant select, insert on platform_store.launch_closure_intents to service_role;
grant select, insert on platform_store.launch_closures to service_role;
grant execute on function platform_api.request_launch_closure_v1(
  uuid, uuid, text, bytea, bytea
) to service_role;
grant execute on function platform_api.record_launch_closure_v1(uuid, bytea, bytea)
  to service_role;
grant execute on function platform_api.list_pending_launch_closures_v1(integer)
  to service_role;

-- Keep a closing launch actionable in My Games, and classify a completed
-- creator closure as retained history even when a Run had already existed.
create or replace function platform_api.list_my_games_v1(
  p_requesting_account_id uuid,
  p_before_at timestamptz default null,
  p_before_launch_id uuid default null,
  p_limit integer default 20
)
returns jsonb
language plpgsql
stable
security invoker
set search_path = ''
as $$
declare
  v_items jsonb;
  v_next jsonb;
begin
  if p_requesting_account_id is null
    or p_limit not between 1 and 50
    or ((p_before_at is null) <> (p_before_launch_id is null))
    or not exists (
      select 1 from platform_store.platform_accounts accounts
      where accounts.account_id = p_requesting_account_id
        and accounts.erased_at is null
    )
  then return null;
  end if;

  with visible as (
    select
      launches.launch_request_id,
      launches.state as launch_state,
      launches.creator_account_id = p_requesting_account_id as can_manage,
      launches.last_transition_at,
      listings.canonical_document,
      listings.result_publication_policy,
      runs.activity_run_id,
      runs.public_id,
      runs.initial_reconciliation_state,
      exists (
        select 1 from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = runs.activity_run_id
          and memberships.controlling_account_id = p_requesting_account_id
      ) as has_run_correspondence,
      exists (
        select 1 from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = runs.activity_run_id
          and memberships.controlling_account_id = p_requesting_account_id
          and memberships.participation_source = 'account_external_agent'
      ) or exists (
        select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = launches.launch_request_id
          and claims.controlling_account_id = p_requesting_account_id
          and claims.participation_kind = 'account_external_agent'
      ) as controls_external_agent,
      terminals.projector_status,
      results.activity_result_id is not null as result_recorded,
      case when runs.activity_run_id is null then null
        else platform_store.public_run_state_v1(runs.activity_run_id) end as public_run_state,
      latest.event_kind as latest_event_kind,
      latest.integrity_status as latest_integrity_status,
      exists (
        select 1 from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id
          and receipts.disposition = 'conflict'
      ) as has_conflict
    from platform_store.launch_requests launches
    join platform_store.activity_listing_revisions listings
      on listings.listing_revision_digest = launches.listing_revision_digest
    left join platform_store.activity_runs runs
      on runs.launch_request_id = launches.launch_request_id
    left join platform_store.activity_run_terminal_evidence terminals
      on terminals.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_results results
      on results.activity_run_id = runs.activity_run_id
    left join lateral (
      select events.event_kind, events.integrity_status
      from platform_store.activity_run_index_events events
      where events.activity_run_id = runs.activity_run_id
      order by events.integrity_generation desc, events.observed_at desc,
        events.activity_run_index_event_id desc
      limit 1
    ) latest on true
    where (
      launches.creator_account_id = p_requesting_account_id
      or exists (
        select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = launches.launch_request_id
          and claims.controlling_account_id = p_requesting_account_id
      )
      or exists (
        select 1 from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = runs.activity_run_id
          and memberships.controlling_account_id = p_requesting_account_id
      )
    )
      and (
        p_before_at is null
        or (launches.last_transition_at, launches.launch_request_id)
          < (p_before_at, p_before_launch_id)
      )
    order by launches.last_transition_at desc, launches.launch_request_id desc
    limit p_limit + 1
  ), classified as (
    select visible.*, case
      when launch_state = 'closing' then 'activity_closing'
      when launch_state = 'closed_by_creator' then 'activity_closed'
      when launch_state = 'abandoned_prestart' then 'setup_abandoned'
      when activity_run_id is null and launch_state = 'cancelled' then 'setup_cancelled'
      when activity_run_id is null and launch_state = 'expired' then 'setup_abandoned'
      when activity_run_id is null and launch_state = 'failed_pre_genesis' then 'setup_failed'
      when activity_run_id is null then 'setup_pending'
      when initial_reconciliation_state <> 'ready' then 'dependency_failure'
      when projector_status is null then 'live'
      when projector_status = 'terminal_without_outcome' then 'terminal_without_outcome'
      when not result_recorded then 'publication_pending'
      when has_conflict
        or latest_event_kind in (
          'terminal_conflict', 'result_conflict', 'integrity_conflict',
          'result_suppressed_privacy', 'result_payload_purged'
        )
        or latest_integrity_status in ('faulted', 'quarantined')
        or latest_event_kind = 'result_suppressed'
      then 'result_suppressed'
      when result_publication_policy <> 'public_recent_results' then 'result_suppressed'
      when public_run_state = 'result' then 'verified_result'
      else 'publication_pending'
    end as result_status
    from visible
  ), page as (
    select * from classified
    order by last_transition_at desc, launch_request_id desc
    limit p_limit
  ), next_page as (
    select last_transition_at, launch_request_id from page
    where exists (select 1 from visible offset p_limit)
    order by last_transition_at asc, launch_request_id asc
    limit 1
  )
  select coalesce(jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
    'launch_id', page.launch_request_id,
    'title', convert_from(page.canonical_document, 'utf8')::jsonb ->> 'title',
    'state', page.result_status,
    'updated_at', page.last_transition_at,
    'participation', case
      when page.controls_external_agent then 'external_agent' else 'human' end,
    'action', case
      when page.result_status = 'verified_result' then 'view_result'
      when page.can_manage and page.result_status = 'activity_closing'
        then 'finish_closing'
      when page.can_manage and page.result_status = 'setup_pending'
        and page.launch_state in (
          'collecting_roster', 'provisioning', 'reconciling'
        ) then 'continue_setup'
      when page.has_run_correspondence
        and page.result_status in ('live', 'publication_pending') then 'return_to_game'
      else 'none' end,
    'result_public_id', case
      when page.result_status = 'verified_result' then page.public_id end
  )) order by page.last_transition_at desc, page.launch_request_id desc), '[]'::jsonb),
  (
    select jsonb_build_object(
      'before_at', last_transition_at,
      'before_launch_id', launch_request_id
    ) from next_page
  )
  into v_items, v_next from page;
  return jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_my_games.v1',
    'items', v_items,
    'next', v_next
  ));
end;
$$;
