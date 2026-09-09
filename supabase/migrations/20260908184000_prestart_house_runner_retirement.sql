-- House Runner cleanup is a separate retained receipt. Abandonment frees the
-- platform active-Run reservation only after exact Host proof; each House
-- unit remains reserved until the same Host retirement primitive proves that
-- exact child stopped.
create table platform_store.prestart_house_runner_retirements (
  activity_run_id uuid not null
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  house_agent_assignment_id uuid not null
    references platform_store.house_agent_assignments(house_agent_assignment_id)
    on delete restrict,
  reservation_operation_id uuid not null
    references platform_store.house_runner_reservations(reservation_operation_id)
    on delete restrict,
  abandonment_evidence_digest bytea not null,
  canonical_retirement_receipt bytea not null,
  retirement_receipt_digest bytea not null,
  recorded_at timestamptz not null default clock_timestamp(),
  primary key (activity_run_id, house_agent_assignment_id),
  unique (retirement_receipt_digest),
  constraint prestart_house_runner_retirement_evidence_digest
    check (octet_length(abandonment_evidence_digest) = 32),
  constraint prestart_house_runner_retirement_receipt_bounds
    check (
      octet_length(canonical_retirement_receipt) between 2 and 8192
      and octet_length(retirement_receipt_digest) = 32
    )
);

alter table platform_store.prestart_house_runner_retirements enable row level security;

create trigger protect_prestart_house_runner_retirement_v1
before update or delete on platform_store.prestart_house_runner_retirements
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create function platform_api.read_prestart_house_runner_retirements_v1(
  p_activity_run_id uuid
)
returns table(
  host_installation_id text,
  reservation_operation_id uuid,
  launch_request_id uuid,
  house_agent_assignment_id uuid,
  abandonment_evidence_digest text
)
language sql
stable
security invoker
set search_path = ''
as $$
  select
    runs.host_installation_id,
    reservations.reservation_operation_id,
    runs.launch_request_id,
    assignments.house_agent_assignment_id,
    'sha256:' || encode(abandoned.abandonment_evidence_digest, 'hex')
  from platform_store.activity_runs runs
  join platform_store.prestart_abandonments abandoned
    on abandoned.activity_run_id = runs.activity_run_id
  join platform_store.house_agent_assignments assignments
    on assignments.launch_request_id = runs.launch_request_id
  join platform_store.house_runner_reservations reservations
    on reservations.reservation_operation_id = assignments.reservation_operation_id
  where runs.activity_run_id = p_activity_run_id
    and runs.initial_reconciliation_state = 'ready'
    and reservations.launch_request_id = runs.launch_request_id
    and reservations.state = 'succeeded'
    and reservations.released_at is null
    and not exists (
      select 1 from platform_store.prestart_house_runner_retirements retired
      where retired.activity_run_id = runs.activity_run_id
        and retired.house_agent_assignment_id = assignments.house_agent_assignment_id
    )
  order by assignments.house_agent_assignment_id;
$$;

create function platform_api.list_prestart_house_runner_retirement_candidates_v1(
  p_limit integer
)
returns table(activity_run_id uuid)
language sql
stable
security invoker
set search_path = ''
as $$
  select runs.activity_run_id
  from platform_store.activity_runs runs
  join platform_store.prestart_abandonments abandoned
    on abandoned.activity_run_id = runs.activity_run_id
  where runs.initial_reconciliation_state = 'ready'
    and exists (
      select 1
      from platform_store.house_agent_assignments assignments
      join platform_store.house_runner_reservations reservations
        on reservations.reservation_operation_id = assignments.reservation_operation_id
      where assignments.launch_request_id = runs.launch_request_id
        and reservations.launch_request_id = runs.launch_request_id
        and reservations.state = 'succeeded'
        and reservations.released_at is null
        and not exists (
          select 1 from platform_store.prestart_house_runner_retirements retired
          where retired.activity_run_id = runs.activity_run_id
            and retired.house_agent_assignment_id = assignments.house_agent_assignment_id
        )
    )
  order by abandoned.recorded_at, runs.activity_run_id
  limit least(greatest(coalesce(p_limit, 0), 0), 100);
$$;

create function platform_api.record_prestart_house_runner_retirement_v1(
  p_activity_run_id uuid,
  p_house_agent_assignment_id uuid,
  p_canonical_retirement_receipt bytea,
  p_retirement_receipt_digest bytea
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  run_row platform_store.activity_runs%rowtype;
  abandonment_row platform_store.prestart_abandonments%rowtype;
  assignment_row platform_store.house_agent_assignments%rowtype;
  reservation_row platform_store.house_runner_reservations%rowtype;
  existing platform_store.prestart_house_runner_retirements%rowtype;
  receipt jsonb;
  expected_abandonment_digest text;
begin
  if p_activity_run_id is null
    or p_house_agent_assignment_id is null
    or octet_length(p_canonical_retirement_receipt) not between 2 and 8192
    or octet_length(p_retirement_receipt_digest) <> 32
    or p_retirement_receipt_digest
      <> extensions.digest(p_canonical_retirement_receipt, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_prestart_house_retirement';
  end if;
  begin
    receipt := convert_from(p_canonical_retirement_receipt, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_prestart_house_retirement';
  end;
  if jsonb_typeof(receipt) <> 'object'
    or not (receipt ?& array[
      'schema', 'host_installation_id', 'reservation_operation_id',
      'launch_request_id', 'house_agent_assignment_id', 'runner_unit_id',
      'disposition', 'platform_evidence_digest', 'stop_witness',
      'authentication_tag'
    ])
    or exists (
      select 1 from jsonb_object_keys(receipt) keys(key)
      where keys.key not in (
        'schema', 'host_installation_id', 'reservation_operation_id',
        'launch_request_id', 'house_agent_assignment_id', 'runner_unit_id',
        'disposition', 'platform_evidence_digest', 'stop_witness',
        'authentication_tag'
      )
    )
    or receipt ->> 'schema' <> 'worldstream/house-runner-retirement-receipt/v1'
    or receipt ->> 'runner_unit_id' !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or receipt ->> 'disposition' <> 'pre_start_abandoned'
    or receipt ->> 'platform_evidence_digest' !~ '^sha256:[0-9a-f]{64}$'
    or receipt ->> 'stop_witness' !~ '^blake3:[0-9a-f]{64}$'
    or receipt ->> 'authentication_tag' !~ '^[0-9a-f]{64}$'
  then
    raise exception using errcode = '22023', message = 'invalid_prestart_house_retirement';
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended(
      'prestart-house-retirement:' || p_activity_run_id::text || ':'
        || p_house_agent_assignment_id::text,
      0
    )
  );
  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  if not found or run_row.initial_reconciliation_state <> 'ready' then
    return false;
  end if;
  select abandoned.* into abandonment_row
  from platform_store.prestart_abandonments abandoned
  where abandoned.activity_run_id = p_activity_run_id;
  if not found then
    return false;
  end if;
  select assignments.* into assignment_row
  from platform_store.house_agent_assignments assignments
  where assignments.house_agent_assignment_id = p_house_agent_assignment_id;
  if not found or assignment_row.launch_request_id <> run_row.launch_request_id then
    return false;
  end if;
  select reservations.* into reservation_row
  from platform_store.house_runner_reservations reservations
  where reservations.reservation_operation_id = assignment_row.reservation_operation_id
  for update;
  if not found
    or reservation_row.launch_request_id <> run_row.launch_request_id
    or reservation_row.state not in ('succeeded', 'released')
  then
    return false;
  end if;
  expected_abandonment_digest := 'sha256:' || encode(
    abandonment_row.abandonment_evidence_digest, 'hex'
  );
  if receipt ->> 'host_installation_id' <> run_row.host_installation_id
    or receipt ->> 'reservation_operation_id' <> reservation_row.reservation_operation_id::text
    or receipt ->> 'launch_request_id' <> run_row.launch_request_id::text
    or receipt ->> 'house_agent_assignment_id' <> assignment_row.house_agent_assignment_id::text
    or receipt ->> 'runner_unit_id' <> reservation_row.runner_unit_id
    or receipt ->> 'platform_evidence_digest' <> expected_abandonment_digest
  then
    return false;
  end if;
  select retired.* into existing
  from platform_store.prestart_house_runner_retirements retired
  where retired.activity_run_id = p_activity_run_id
    and retired.house_agent_assignment_id = p_house_agent_assignment_id;
  if found then
    return existing.canonical_retirement_receipt = p_canonical_retirement_receipt
      and existing.retirement_receipt_digest = p_retirement_receipt_digest
      and existing.abandonment_evidence_digest = abandonment_row.abandonment_evidence_digest;
  end if;
  if reservation_row.state <> 'succeeded' or reservation_row.released_at is not null then
    return false;
  end if;
  insert into platform_store.prestart_house_runner_retirements (
    activity_run_id,
    house_agent_assignment_id,
    reservation_operation_id,
    abandonment_evidence_digest,
    canonical_retirement_receipt,
    retirement_receipt_digest
  ) values (
    p_activity_run_id,
    p_house_agent_assignment_id,
    reservation_row.reservation_operation_id,
    abandonment_row.abandonment_evidence_digest,
    p_canonical_retirement_receipt,
    p_retirement_receipt_digest
  );
  update platform_store.house_runner_reservations reservations
  set state = 'released',
      released_at = clock_timestamp(),
      release_receipt_digest = p_retirement_receipt_digest,
      last_transition_at = clock_timestamp()
  where reservations.reservation_operation_id = reservation_row.reservation_operation_id
    and reservations.state = 'succeeded'
    and reservations.released_at is null;
  return found;
end;
$$;

revoke all on platform_store.prestart_house_runner_retirements
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_prestart_house_runner_retirements_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.list_prestart_house_runner_retirement_candidates_v1(integer)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_prestart_house_runner_retirement_v1(
  uuid, uuid, bytea, bytea
) from public, anon, authenticated, service_role;

grant select, insert on platform_store.prestart_house_runner_retirements to service_role;
grant execute on function platform_api.read_prestart_house_runner_retirements_v1(uuid)
  to service_role;
grant execute on function platform_api.list_prestart_house_runner_retirement_candidates_v1(integer)
  to service_role;
grant execute on function platform_api.record_prestart_house_runner_retirement_v1(
  uuid, uuid, bytea, bytea
) to service_role;
