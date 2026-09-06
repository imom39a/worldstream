alter table platform_store.activity_run_index_events
  drop constraint run_index_event_kind;

alter table platform_store.activity_run_index_events
  add constraint run_index_event_kind check (event_kind in (
    'terminal_observed',
    'terminal_confirmed',
    'terminal_conflict',
    'result_published',
    'result_confirmed',
    'result_conflict',
    'integrity_observed',
    'integrity_conflict',
    'result_suppressed',
    'result_reverified',
    'result_suppressed_privacy',
    'result_payload_purged'
  ));

create function platform_store.public_run_state_v1(p_activity_run_id uuid)
returns text
language sql
stable
security invoker
set search_path = ''
as $$
  with run_facts as (
    select
      runs.activity_run_id,
      runs.public_id,
      runs.initial_reconciliation_state,
      listings.public_viewing_policy,
      listings.result_publication_policy,
      terminals.activity_run_id is not null as terminal_recorded,
      terminals.projector_status,
      results.activity_result_id,
      results.integrity_generation as result_generation,
      payloads.activity_result_id is not null as payload_retained
    from platform_store.activity_runs runs
    join platform_store.activity_listing_revisions listings
      on listings.listing_revision_digest = runs.listing_revision_digest
    left join platform_store.activity_run_terminal_evidence terminals
      on terminals.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_results results
      on results.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_result_payloads payloads
      on payloads.activity_result_id = results.activity_result_id
    where runs.activity_run_id = p_activity_run_id
  ),
  latest_generation as (
    select max(events.integrity_generation) as value
    from platform_store.activity_run_index_events events
    where events.activity_run_id = p_activity_run_id
  ),
  visibility as (
    select
      facts.*,
      exists (
        select 1
        from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = facts.activity_run_id
          and receipts.disposition = 'conflict'
      ) or exists (
        select 1
        from platform_store.activity_run_index_events events
        where events.activity_run_id = facts.activity_run_id
          and (
            events.disposition = 'conflict'
            or events.event_kind in (
              'terminal_conflict',
              'result_conflict',
              'integrity_conflict',
              'result_suppressed_privacy',
              'result_payload_purged'
            )
          )
      ) as permanently_suppressed,
      exists (
        select 1
        from platform_store.activity_run_index_events events
        cross join latest_generation latest
        where events.activity_run_id = facts.activity_run_id
          and events.integrity_generation = latest.value
          and (
            events.integrity_status in ('faulted', 'quarantined')
            or events.event_kind = 'result_suppressed'
          )
      ) as latest_generation_suppressed,
      exists (
        select 1
        from platform_store.activity_run_index_events events
        cross join latest_generation latest
        where events.activity_run_id = facts.activity_run_id
          and events.integrity_generation = latest.value
          and events.integrity_status = 'healthy'
          and events.disposition in ('applied', 'duplicate')
          and events.event_kind in (
            'result_published',
            'result_confirmed',
            'integrity_observed',
            'result_reverified'
          )
      ) as latest_generation_publishable,
      (select value from latest_generation) as latest_generation
    from run_facts facts
  )
  select case
    when facts.public_id is null
      or facts.initial_reconciliation_state <> 'ready'
      or facts.permanently_suppressed
    then 'unavailable'
    when facts.activity_result_id is not null then case
      when facts.result_publication_policy = 'public_recent_results'
        and facts.payload_retained
        and facts.latest_generation is not null
        and facts.latest_generation >= facts.result_generation
        and not facts.latest_generation_suppressed
        and facts.latest_generation_publishable
      then 'result'
      else 'unavailable'
    end
    when facts.terminal_recorded then 'unavailable'
    when facts.public_viewing_policy = 'anonymous_by_link'
      or facts.result_publication_policy = 'public_recent_results'
    then 'live'
    else 'unavailable'
  end
  from visibility facts;
$$;

create function platform_store.public_run_attributions_v1(p_activity_run_id uuid)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select coalesce(
    jsonb_agg(
      jsonb_strip_nulls(jsonb_build_object(
        'seat_label', memberships.public_seat_label,
        'role', memberships.role,
        'kind', case memberships.participation_source
          when 'account_human' then 'human'
          when 'account_external_agent' then 'external_agent'
          when 'platform_house_agent' then 'house_agent'
        end,
        'identity', case
          when memberships.participation_source in (
            'account_human', 'account_external_agent'
          ) then case
            when accounts.erased_at is null
              and identities.erasure_requested_at is null
              and identities.public_profile_enabled
              and identities.github_login is not null
            then jsonb_strip_nulls(jsonb_build_object(
              'kind', 'github',
              'login', identities.github_login,
              'avatar_url', identities.avatar_url,
              'fallback_label', memberships.public_seat_label
            ))
            else jsonb_build_object(
              'kind', 'pseudonym',
              'label', memberships.public_seat_label
            )
          end
        end,
        'notice', case memberships.participation_source
          when 'account_external_agent' then 'External agent — unverified'
          when 'platform_house_agent' then 'Exhibition — platform-supplied agents'
        end,
        'house_agent', case
          when memberships.participation_source = 'platform_house_agent'
          then jsonb_build_object(
            'display_name', house_revisions.display_name,
            'revision_digest', house_revisions.house_agent_revision_digest,
            'route', jsonb_build_object(
              'gateway', 'openrouter',
              'provider_slug', house_revisions.provider_slug,
              'model_slug', house_revisions.model_slug
            ),
            'allowance', house_revisions.execution_allowance
          )
        end
      ))
      order by memberships.seat_id
    ),
    '[]'::jsonb
  )
  from platform_store.activity_run_memberships memberships
  left join platform_store.platform_accounts accounts
    on accounts.account_id = memberships.controlling_account_id
  left join platform_store.github_identities identities
    on identities.account_id = memberships.controlling_account_id
  left join platform_store.house_agent_assignments house_assignments
    on house_assignments.house_agent_assignment_id = memberships.house_agent_assignment_id
  left join platform_store.house_agent_revisions house_revisions
    on house_revisions.house_agent_revision_digest =
      house_assignments.house_agent_revision_digest
  where memberships.activity_run_id = p_activity_run_id
    and memberships.purpose = 'participant';
$$;

create function platform_store.public_run_dto_v1(p_activity_run_id uuid)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  with selected as (
    select
      runs.*,
      listings.listing_key,
      convert_from(listings.canonical_document, 'utf8')::jsonb as listing_document,
      results.indexed_at,
      case when payloads.canonical_payload is null then null
        else convert_from(payloads.canonical_payload, 'utf8')::jsonb
      end as result_summary,
      platform_store.public_run_state_v1(runs.activity_run_id) as public_state
    from platform_store.activity_runs runs
    join platform_store.activity_listing_revisions listings
      on listings.listing_revision_digest = runs.listing_revision_digest
    left join platform_store.indexed_activity_results results
      on results.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_result_payloads payloads
      on payloads.activity_result_id = results.activity_result_id
    where runs.activity_run_id = p_activity_run_id
  )
  select case
    when selected.public_state is null or selected.public_state = 'unavailable'
    then jsonb_build_object(
      'version', 'public_run.v1',
      'state', 'unavailable'
    )
    else jsonb_strip_nulls(jsonb_build_object(
      'version', 'public_run.v1',
      'state', selected.public_state,
      'public_id', selected.public_id,
      'activity', jsonb_build_object(
        'listing_key', selected.listing_key,
        'title', selected.listing_document ->> 'title',
        'description', selected.listing_document ->> 'description',
        'listing_revision', selected.listing_revision_digest,
        'pack', jsonb_build_object(
          'id', selected.pack_id,
          'version', selected.pack_version,
          'revision', selected.pack_revision_digest
        )
      ),
      'started_at', selected.genesis_observed_at,
      'completed_at', case when selected.public_state = 'result'
        then selected.indexed_at
      end,
      'evidence', jsonb_build_object(
        'class', selected.evidence_class,
        'label', case selected.evidence_class
          when 'exhibition_platform_house_agents'
            then 'Exhibition — platform-supplied agents'
          else 'Unranked activity'
        end
      ),
      'participants', platform_store.public_run_attributions_v1(
        selected.activity_run_id
      ),
      'live', case when selected.public_state = 'live' then jsonb_build_object(
        'available', false
      ) end,
      'result', case when selected.public_state = 'result'
        then selected.result_summary
      end
    ))
  end
  from selected;
$$;

create function platform_api.read_public_run_v1(p_public_id text)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select coalesce(
    (
      select platform_store.public_run_dto_v1(runs.activity_run_id)
      from platform_store.activity_runs runs
      where p_public_id ~ '^[0-9a-f]{32}$'
        and runs.public_id = p_public_id
    ),
    jsonb_build_object(
      'version', 'public_run.v1',
      'state', 'unavailable'
    )
  );
$$;

create function platform_api.list_recent_results_v1(p_limit integer default 20)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  with visible as (
    select
      results.indexed_at,
      platform_store.public_run_dto_v1(runs.activity_run_id) as dto
    from platform_store.indexed_activity_results results
    join platform_store.activity_runs runs
      on runs.activity_run_id = results.activity_run_id
    where runs.pack_id = 'worldstream.agent-heist'
      and runs.public_id is not null
      and platform_store.public_run_state_v1(runs.activity_run_id) = 'result'
    order by results.indexed_at desc, results.activity_result_id
    limit least(greatest(coalesce(p_limit, 20), 1), 20)
  )
  select jsonb_build_object(
    'version', 'recent_results.v1',
    'activity', 'agent-heist',
    'order', 'newest_first',
    'maximum', 20,
    'results', coalesce(
      jsonb_agg(visible.dto order by visible.indexed_at desc),
      '[]'::jsonb
    )
  )
  from visible;
$$;

revoke execute on function platform_store.public_run_state_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.public_run_attributions_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_store.public_run_dto_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_public_run_v1(text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.list_recent_results_v1(integer)
  from public, anon, authenticated, service_role;

grant execute on function platform_store.public_run_state_v1(uuid)
  to service_role;
grant execute on function platform_store.public_run_attributions_v1(uuid)
  to service_role;
grant execute on function platform_store.public_run_dto_v1(uuid)
  to service_role;
grant execute on function platform_api.read_public_run_v1(text)
  to service_role;
grant execute on function platform_api.list_recent_results_v1(integer)
  to service_role;
