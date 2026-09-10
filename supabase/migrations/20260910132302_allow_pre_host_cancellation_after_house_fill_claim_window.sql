-- A House-fill claim window prevents late claims.  It must not strand a
-- collecting, pre-Host Launch Request when its creator uses the existing
-- cancellation RPC: that RPC only releases an already-live claim and then
-- releases the Launch Request's capacity.
--
-- Keep this exception deliberately narrower than the cancellation RPC.  Once
-- the window is closed, the only allowed seat-claim update is that exact
-- cancellation release.  The ordinary immutable-claim trigger remains in
-- force, and this function independently keeps frozen and Host-mutated
-- Launch Requests outside the exception.
create or replace function platform_store.reject_closed_house_fill_claim_v1()
returns trigger
language plpgsql
security invoker
set search_path = ''
as $$
begin
  if not platform_store.house_fill_claims_closed_v1(new.launch_request_id) then
    return new;
  end if;

  if tg_op = 'UPDATE'
    and old.released_at is null
    and new.released_at is not null
    and new.release_reason = 'launch_cancelled'
    and new.claim_id = old.claim_id
    and new.launch_request_id = old.launch_request_id
    and new.seat_id = old.seat_id
    and new.role = old.role
    and new.controlling_account_id = old.controlling_account_id
    and new.participation_kind = old.participation_kind
    and new.principal_reference = old.principal_reference
    and new.claimed_at = old.claimed_at
    and exists (
      select 1
      from platform_store.launch_requests launches
      where launches.launch_request_id = new.launch_request_id
        and launches.state in ('collecting_roster', 'provisioning')
        and launches.roster_frozen_at is null
        and launches.host_mutation_started_at is null
    )
  then
    return new;
  end if;

  raise exception using errcode = '55000', message = 'house_fill_claim_window_closed';
end;
$$;
