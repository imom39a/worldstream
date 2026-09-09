-- Private, bounded result status for an account's own retained history. It
-- reads immutable Run/index evidence only; it never polls or copies Room
-- state, Membership authority, or Pack-specific result fields.
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
      launches.launch_request_id, launches.state as launch_state,
      launches.creator_account_id = p_requesting_account_id as can_manage,
      launches.last_transition_at, listings.canonical_document,
      listings.result_publication_policy,
      runs.activity_run_id, runs.public_id, runs.initial_reconciliation_state,
      exists (select 1 from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = runs.activity_run_id
          and memberships.controlling_account_id = p_requesting_account_id) as has_run_correspondence,
      exists (select 1 from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = runs.activity_run_id
          and memberships.controlling_account_id = p_requesting_account_id
          and memberships.participation_source = 'account_external_agent')
        or exists (select 1 from platform_store.seat_claims claims
          where claims.launch_request_id = launches.launch_request_id
            and claims.controlling_account_id = p_requesting_account_id
            and claims.participation_kind = 'account_external_agent') as controls_external_agent,
      terminals.projector_status,
      results.activity_result_id is not null as result_recorded,
      case when runs.activity_run_id is null then null
        else platform_store.public_run_state_v1(runs.activity_run_id) end as public_run_state,
      latest.event_kind as latest_event_kind,
      latest.integrity_status as latest_integrity_status,
      exists (select 1 from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id and receipts.disposition = 'conflict') as has_conflict
    from platform_store.launch_requests launches
    join platform_store.activity_listing_revisions listings
      on listings.listing_revision_digest = launches.listing_revision_digest
    left join platform_store.activity_runs runs on runs.launch_request_id = launches.launch_request_id
    left join platform_store.activity_run_terminal_evidence terminals on terminals.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_results results on results.activity_run_id = runs.activity_run_id
    left join lateral (
      select events.event_kind, events.integrity_status
      from platform_store.activity_run_index_events events
      where events.activity_run_id = runs.activity_run_id
      order by events.integrity_generation desc, events.observed_at desc, events.activity_run_index_event_id desc
      limit 1
    ) latest on true
    where (
      launches.creator_account_id = p_requesting_account_id
      or exists (select 1 from platform_store.seat_claims claims
        where claims.launch_request_id = launches.launch_request_id and claims.controlling_account_id = p_requesting_account_id)
      or exists (select 1 from platform_store.activity_run_memberships memberships
        where memberships.activity_run_id = runs.activity_run_id and memberships.controlling_account_id = p_requesting_account_id)
    ) and (p_before_at is null or (launches.last_transition_at, launches.launch_request_id) < (p_before_at, p_before_launch_id))
    order by launches.last_transition_at desc, launches.launch_request_id desc
    limit p_limit + 1
  ), classified as (
    select visible.*, case
      -- A Run can exist briefly before it becomes usable. Immutable
      -- abandonment evidence marks that post-Genesis setup as abandoned, not
      -- as a live or result-pending activity.
      when launch_state = 'abandoned_prestart' then 'setup_abandoned'
      when activity_run_id is null and launch_state = 'cancelled' then 'setup_cancelled'
      when activity_run_id is null and launch_state = 'expired' then 'setup_abandoned'
      when activity_run_id is null and launch_state = 'failed_pre_genesis' then 'setup_failed'
      when activity_run_id is null then 'setup_pending'
      when initial_reconciliation_state <> 'ready' then 'dependency_failure'
      when projector_status is null then 'live'
      when projector_status = 'terminal_without_outcome' then 'terminal_without_outcome'
      when not result_recorded then 'publication_pending'
      when has_conflict or latest_event_kind in ('terminal_conflict', 'result_conflict', 'integrity_conflict', 'result_suppressed_privacy', 'result_payload_purged')
        or latest_integrity_status in ('faulted', 'quarantined') or latest_event_kind = 'result_suppressed'
      then 'result_suppressed'
      -- An indexed result may be healthy while the reviewed Listing keeps it
      -- private. It is a retained history item, not a publication retry.
      when result_publication_policy <> 'public_recent_results'
      then 'result_suppressed'
      -- public_run_state_v1 already suppresses these conditions, but retain
      -- the explicit private classification above so a future public policy
      -- change cannot accidentally make suppressed history look verified.
      when public_run_state = 'result' then 'verified_result'
      else 'publication_pending'
    end as result_status
    from visible
  ), page as (
    select * from classified order by last_transition_at desc, launch_request_id desc limit p_limit
  ), next_page as (
    select last_transition_at, launch_request_id from page
    where exists (select 1 from visible offset p_limit)
    order by last_transition_at asc, launch_request_id asc limit 1
  )
  select coalesce(jsonb_agg(jsonb_strip_nulls(jsonb_build_object(
    'launch_id', page.launch_request_id,
    'title', convert_from(page.canonical_document, 'utf8')::jsonb ->> 'title',
    'state', page.result_status,
    'updated_at', page.last_transition_at,
    'participation', case when page.controls_external_agent then 'external_agent' else 'human' end,
    'action', case
      when page.result_status = 'verified_result' then 'view_result'
      when page.can_manage and page.activity_run_id is null and page.result_status = 'setup_pending'
        and page.launch_state in ('collecting_roster', 'provisioning', 'reconciling') then 'continue_setup'
      when page.has_run_correspondence and page.result_status in ('live', 'publication_pending') then 'return_to_game'
      else 'none' end,
    'result_public_id', case when page.result_status = 'verified_result' then page.public_id end
  )) order by page.last_transition_at desc, page.launch_request_id desc), '[]'::jsonb),
  (select jsonb_build_object('before_at', last_transition_at, 'before_launch_id', launch_request_id) from next_page)
  into v_items, v_next from page;
  return jsonb_strip_nulls(jsonb_build_object('version', 'platform_my_games.v1', 'items', v_items, 'next', v_next));
end;
$$;
