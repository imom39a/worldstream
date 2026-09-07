-- Scheduling metadata is not Room truth or result evidence. Keep failed and
-- unchanged old rechecks from permanently occupying the first page.
create table platform_store.reconciliation_attempts (
  launch_request_id uuid primary key references platform_store.launch_requests(launch_request_id) on delete restrict,
  last_attempt_at timestamptz not null default clock_timestamp()
);
alter table platform_store.reconciliation_attempts enable row level security;
revoke all on platform_store.reconciliation_attempts from public, anon, authenticated, service_role;
grant select, insert, update on platform_store.reconciliation_attempts to service_role;

create function platform_api.mark_reconciliation_attempt_v1(p_launch_request_id uuid)
returns void
language sql
volatile
security invoker
set search_path = ''
as $$
  insert into platform_store.reconciliation_attempts (launch_request_id)
  select launch_request_id from platform_store.launch_requests
  where launch_request_id = p_launch_request_id and host_mutation_started_at is not null
  on conflict (launch_request_id) do update set last_attempt_at = clock_timestamp();
$$;
revoke execute on function platform_api.mark_reconciliation_attempt_v1(uuid)
  from public, anon, authenticated, service_role;
grant execute on function platform_api.mark_reconciliation_attempt_v1(uuid) to service_role;

create or replace function platform_api.list_reconciliation_candidates_v1(
  p_limit integer
)
returns table(
  candidate_kind text,
  launch_request_id uuid,
  activity_run_id uuid,
  listing_revision_digest text,
  launch_request_digest text,
  host_installation_id text,
  room_setup_operation_id text
)
language sql
stable
security invoker
set search_path = ''
as $$
  with candidates as (
    select
      0 as priority,
      'genesis'::text as candidate_kind,
      launches.launch_request_id,
      null::uuid as activity_run_id,
      launches.listing_revision_digest,
      null::text as launch_request_digest,
      launches.host_installation_id,
      launches.room_setup_operation_id,
      launches.last_transition_at as due_at
    from platform_store.launch_requests launches
    where launches.host_mutation_started_at is not null
      and launches.state in ('provisioning', 'reconciling')
      and not exists (
        select 1 from platform_store.activity_runs runs
        where runs.launch_request_id = launches.launch_request_id
      )
      and not exists (
        select 1 from platform_store.reconciliation_receipts receipts
        where receipts.launch_request_id = launches.launch_request_id
          and receipts.disposition = 'conflict'
      )
    union all
    select
      case when terminals.activity_run_id is null or results.activity_result_id is null then 1 else 2 end as priority,
      'result_source'::text,
      runs.launch_request_id,
      runs.activity_run_id,
      runs.listing_revision_digest,
      runs.launch_request_digest,
      runs.host_installation_id,
      runs.room_setup_operation_id,
      coalesce(latest.observed_at, runs.genesis_observed_at) as due_at
    from platform_store.activity_runs runs
    left join platform_store.activity_run_terminal_evidence terminals
      on terminals.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_results results
      on results.activity_run_id = runs.activity_run_id
    left join lateral (
      select events.observed_at
      from platform_store.activity_run_index_events events
      where events.activity_run_id = runs.activity_run_id
      order by events.integrity_generation desc,
        events.observed_at desc,
        events.activity_run_index_event_id desc
      limit 1
    ) latest on true
    where runs.initial_reconciliation_state = 'ready'
      and not exists (
        select 1 from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id
          and receipts.disposition = 'conflict'
      )
      and (
        terminals.activity_run_id is null
        or (terminals.projector_status = 'summary' and results.activity_result_id is null)
        or latest.observed_at is null
        or latest.observed_at < statement_timestamp() - interval '5 minutes'
      )
  )
  select candidate_kind, launch_request_id, activity_run_id,
    listing_revision_digest, launch_request_digest, host_installation_id,
    room_setup_operation_id
  from candidates
  left join platform_store.reconciliation_attempts attempts using (launch_request_id)
  order by
    case when exists (
      select 1 from platform_store.capacity_reservations capacity
      where capacity.launch_request_id = candidates.launch_request_id
        and capacity.kind = 'active_run' and capacity.released_at is null
    ) then 0 else 1 end,
    attempts.last_attempt_at nulls first, priority, due_at, launch_request_id
  limit least(greatest(coalesce(p_limit, 0), 0), 100);
$$;
