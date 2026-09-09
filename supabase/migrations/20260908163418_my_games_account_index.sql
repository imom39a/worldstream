-- A private, bounded discovery index. This derives only platform coordination
-- and immutable Run correspondence; it does not mirror Room state or expose a
-- Membership credential. Entry still goes through the existing account-bound
-- Launch/Run re-entry path.
create function platform_api.list_my_games_v1(
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
  then
    return null;
  end if;

  with visible as (
    select
      launches.launch_request_id,
      launches.state as launch_state,
      launches.creator_account_id = p_requesting_account_id as can_manage,
      launches.last_transition_at,
      listings.canonical_document,
      runs.activity_run_id,
      runs.public_id,
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
      case when runs.activity_run_id is null then null
        else platform_store.public_run_state_v1(runs.activity_run_id)
      end as public_run_state,
      exists (
        select 1 from platform_store.activity_run_terminal_evidence terminal
        where terminal.activity_run_id = runs.activity_run_id
      ) as terminal_recorded
    from platform_store.launch_requests launches
    join platform_store.activity_listing_revisions listings
      on listings.listing_revision_digest = launches.listing_revision_digest
    left join platform_store.activity_runs runs
      on runs.launch_request_id = launches.launch_request_id
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
  ), page as (
    select * from visible
    order by last_transition_at desc, launch_request_id desc
    limit p_limit
  ), next_page as (
    select last_transition_at, launch_request_id
    from page
    where exists (select 1 from visible offset p_limit)
    -- The next request uses a strict "before" comparison. Carry the last
    -- included item, not the first excluded item, so it is neither skipped
    -- nor repeated.
    order by last_transition_at asc, launch_request_id asc
    limit 1
  )
  select coalesce(jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
    'launch_id', page.launch_request_id,
    'title', convert_from(page.canonical_document, 'utf8')::jsonb ->> 'title',
    'state', case
      when page.terminal_recorded then 'completed'
      else page.launch_state
    end,
    'updated_at', page.last_transition_at,
    'participation', case when page.controls_external_agent
      then 'external_agent' else 'human' end,
    'action', case
      when page.can_manage
        and page.activity_run_id is null
        and page.launch_state in ('collecting_roster', 'provisioning', 'reconciling')
      then 'continue_setup'
      when page.public_id is not null and page.public_run_state = 'result'
      then 'view_result'
      when page.has_run_correspondence then 'return_to_game'
      else 'none'
    end,
    'result_public_id', case
      when page.public_id is not null and page.public_run_state = 'result'
      then page.public_id
    end
  )) order by page.last_transition_at desc, page.launch_request_id desc), '[]'::jsonb),
  (select jsonb_build_object(
    'before_at', last_transition_at,
    'before_launch_id', launch_request_id
  ) from next_page)
  into v_items, v_next
  from page;

  return jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_my_games.v1',
    'items', v_items,
    'next', v_next
  ));
end;
$$;

revoke execute on function platform_api.list_my_games_v1(uuid, timestamptz, uuid, integer)
  from public, anon, authenticated, service_role;
grant execute on function platform_api.list_my_games_v1(uuid, timestamptz, uuid, integer)
  to service_role;
