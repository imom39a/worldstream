-- Result payloads belong to Activity Packs. A JSON null can be meaningful to
-- their declared schema, so remove only absent envelope fields and never run a
-- recursive jsonb_strip_nulls over the Pack-owned result.
create or replace function platform_store.public_run_dto_v1(
  p_activity_run_id uuid
)
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
      convert_from(listings.canonical_document, 'utf8')::jsonb
        as listing_document,
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
    then jsonb_build_object('version', 'public_run.v1', 'state', 'unavailable')
    else jsonb_build_object(
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
      'evidence', jsonb_build_object(
        'class', selected.evidence_class,
        'label', case selected.evidence_class
          when 'exhibition_platform_house_agents'
            then 'Exhibition — platform-supplied agents'
          else 'Unranked activity'
        end
      ),
      'participants',
        platform_store.public_run_attributions_v1(selected.activity_run_id)
    ) || case selected.public_state
      when 'live' then jsonb_build_object(
        'live', jsonb_build_object('available', true)
      )
      when 'result' then jsonb_build_object(
        'completed_at', selected.indexed_at,
        'result', selected.result_summary
      )
      else '{}'::jsonb
    end
  end
  from selected;
$$;
