-- A post-Genesis pre-start abandonment deliberately retains its Activity Run
-- history without fabricating terminal evidence or a public result. It is no
-- longer a live result source and must not keep public read-repair coupled to
-- a Host operation that has been durably fenced.
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
    join platform_store.launch_requests launches
      on launches.launch_request_id = runs.launch_request_id
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
    where launches.state = 'run_created'
      and runs.initial_reconciliation_state = 'ready'
      and not exists (
        select 1 from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id
          and receipts.disposition = 'conflict'
      )
      -- A summary plus an indexed result is complete and immutable. It is
      -- intentionally absent from this queue even when its last event is
      -- older than the five-minute live-result recheck interval.
      and not (
        terminals.projector_status = 'summary'
        and results.activity_result_id is not null
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
