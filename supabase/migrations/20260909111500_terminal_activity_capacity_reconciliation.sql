-- Immutable terminal evidence is the only authority that can release an
-- active-Run reservation after Genesis. This bounded repair closes the
-- historical gap where a later Genesis reconciliation could insert a
-- quarantined-recovery reservation after terminal evidence already existed.

create function platform_store.prevent_post_terminal_activity_capacity_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if new.kind = 'active_run'
    and new.activity_run_id is not null
    and exists (
      select 1
      from platform_store.activity_run_terminal_evidence terminal
      where terminal.activity_run_id = new.activity_run_id
    )
  then
    -- A post-terminal reconciliation may retain its receipt, but it cannot
    -- reacquire admission capacity for a Run whose terminal evidence is
    -- already immutable.
    return null;
  end if;
  return new;
end;
$$;

create trigger prevent_post_terminal_activity_capacity_v1
before insert on platform_store.capacity_reservations
for each row execute function platform_store.prevent_post_terminal_activity_capacity_v1();

create function platform_api.reconcile_terminal_activity_capacity_v1(
  p_limit integer
)
returns integer
language plpgsql
security invoker
set search_path = ''
as $$
declare
  released integer;
begin
  if p_limit is null or p_limit < 1 or p_limit > 100 then
    raise exception using errcode = '22023', message = 'invalid_capacity_reconciliation_limit';
  end if;

  with candidates as (
    select reservations.reservation_id
    from platform_store.capacity_reservations reservations
    join platform_store.activity_run_terminal_evidence terminal
      on terminal.activity_run_id = reservations.activity_run_id
    where reservations.kind = 'active_run'
      and reservations.released_at is null
    order by terminal.observed_at, reservations.reservation_id
    limit p_limit
    for update of reservations skip locked
  )
  update platform_store.capacity_reservations reservations
  set released_at = clock_timestamp(),
      release_reason = 'projector_terminal'
  from candidates
  where reservations.reservation_id = candidates.reservation_id;
  get diagnostics released = row_count;
  return released;
end;
$$;

revoke all on function platform_store.prevent_post_terminal_activity_capacity_v1()
  from public, anon, authenticated, service_role;
revoke all on function platform_api.reconcile_terminal_activity_capacity_v1(integer)
  from public, anon, authenticated, service_role;
grant execute on function platform_api.reconcile_terminal_activity_capacity_v1(integer)
  to service_role;
