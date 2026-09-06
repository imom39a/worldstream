create table platform_store.public_projection_relay_bindings (
  activity_run_id uuid primary key
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  public_id text not null unique,
  canonical_binding_request bytea not null,
  binding_request_digest bytea not null unique,
  canonical_binding_receipt bytea not null,
  binding_receipt_digest bytea not null unique,
  bound_at timestamptz not null default clock_timestamp(),
  constraint public_projection_relay_public_id_shape check (
    public_id ~ '^[0-9a-f]{32}$'
  ),
  constraint public_projection_relay_request_bounds check (
    octet_length(canonical_binding_request) between 2 and 65536
    and octet_length(binding_request_digest) = 32
  ),
  constraint public_projection_relay_receipt_bounds check (
    octet_length(canonical_binding_receipt) between 2 and 65536
    and octet_length(binding_receipt_digest) = 32
  )
);

alter table platform_store.public_projection_relay_bindings enable row level security;

create trigger protect_public_projection_relay_binding_v1
before update or delete on platform_store.public_projection_relay_bindings
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create function platform_api.read_public_relay_binding_candidate_v1(
  p_activity_run_id uuid
)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select jsonb_build_object(
    'schema', 'worldstream/hosted-public-relay-bind-request/v1',
    'public_run_id', runs.public_id,
    'activity_run_id', runs.activity_run_id,
    'host_installation_id', runs.host_installation_id,
    'launch_request_id', runs.launch_request_id,
    'listing_revision_digest', runs.listing_revision_digest,
    'launch_request_digest', runs.launch_request_digest,
    'room_setup_operation_id', runs.room_setup_operation_id,
    'room_id', runs.room_id,
    'pack', jsonb_build_object(
      'id', runs.pack_id,
      'version', runs.pack_version,
      'digest', runs.pack_revision_digest
    ),
    'relay_principal_id', memberships.principal_id,
    'relay_membership_id', memberships.membership_id
  )
  from platform_store.activity_runs runs
  join platform_store.activity_listing_revisions listings
    on listings.listing_revision_digest = runs.listing_revision_digest
  join platform_store.activity_run_memberships memberships
    on memberships.activity_run_id = runs.activity_run_id
   and memberships.purpose = 'public_projection_relay'
  where runs.activity_run_id = p_activity_run_id
    and runs.initial_reconciliation_state = 'ready'
    and runs.public_id is not null
    and listings.public_viewing_policy = 'anonymous_by_link'
    and memberships.access_mode = 'spectator'
    and memberships.principal_kind = 'agent'
    and memberships.role is null
    and memberships.seat_id is null
    and memberships.controlling_account_id is null
    and memberships.entry_selector is null
    and memberships.service_scope_digest = extensions.digest(
      convert_to(
        '["room:attach", "room:observe_public"]'::jsonb::text,
        'utf8'
      ),
      'sha256'
    )
    and not exists (
      select 1
      from platform_store.reconciliation_receipts receipts
      where receipts.activity_run_id = runs.activity_run_id
        and receipts.disposition = 'conflict'
    );
$$;

create function platform_api.record_public_relay_binding_v1(
  p_activity_run_id uuid,
  p_canonical_binding_request bytea,
  p_binding_request_digest bytea,
  p_canonical_binding_receipt bytea,
  p_binding_receipt_digest bytea
)
returns boolean
language plpgsql
security invoker
set search_path = ''
as $$
declare
  candidate jsonb;
  request_value jsonb;
  receipt_value jsonb;
  existing platform_store.public_projection_relay_bindings%rowtype;
  expected_receipt jsonb;
begin
  if p_activity_run_id is null
    or octet_length(p_canonical_binding_request) not between 2 and 65536
    or octet_length(p_canonical_binding_receipt) not between 2 and 65536
    or octet_length(p_binding_request_digest) <> 32
    or octet_length(p_binding_receipt_digest) <> 32
    or p_binding_request_digest
      <> extensions.digest(p_canonical_binding_request, 'sha256')
    or p_binding_receipt_digest
      <> extensions.digest(p_canonical_binding_receipt, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_public_relay_binding';
  end if;

  begin
    request_value := convert_from(p_canonical_binding_request, 'utf8')::jsonb;
    receipt_value := convert_from(p_canonical_binding_receipt, 'utf8')::jsonb;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_public_relay_binding';
  end;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended(
      'public-relay:' || p_activity_run_id::text,
      0
    )
  );

  perform runs.activity_run_id
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  if not found then
    return false;
  end if;

  candidate := platform_api.read_public_relay_binding_candidate_v1(p_activity_run_id);
  if candidate is null or request_value <> candidate then
    return false;
  end if;
  expected_receipt := jsonb_build_object(
    'schema', 'worldstream/hosted-public-relay-bind-receipt/v1',
    'public_run_id', candidate ->> 'public_run_id',
    'activity_run_id', candidate ->> 'activity_run_id',
    'binding_request_digest',
      'sha256:' || encode(p_binding_request_digest, 'hex'),
    'bound', true
  );
  if receipt_value <> expected_receipt then
    return false;
  end if;

  select bindings.*
  into existing
  from platform_store.public_projection_relay_bindings bindings
  where bindings.activity_run_id = p_activity_run_id
     or bindings.public_id = candidate ->> 'public_run_id'
  order by bindings.activity_run_id
  limit 1;
  if found then
    return existing.activity_run_id = p_activity_run_id
      and existing.public_id = candidate ->> 'public_run_id'
      and existing.canonical_binding_request = p_canonical_binding_request
      and existing.binding_request_digest = p_binding_request_digest
      and existing.canonical_binding_receipt = p_canonical_binding_receipt
      and existing.binding_receipt_digest = p_binding_receipt_digest;
  end if;

  insert into platform_store.public_projection_relay_bindings (
    activity_run_id,
    public_id,
    canonical_binding_request,
    binding_request_digest,
    canonical_binding_receipt,
    binding_receipt_digest
  ) values (
    p_activity_run_id,
    candidate ->> 'public_run_id',
    p_canonical_binding_request,
    p_binding_request_digest,
    p_canonical_binding_receipt,
    p_binding_receipt_digest
  );
  return true;
end;
$$;

create or replace function platform_store.public_run_state_v1(p_activity_run_id uuid)
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
      payloads.activity_result_id is not null as payload_retained,
      bindings.activity_run_id is not null as public_relay_bound
    from platform_store.activity_runs runs
    join platform_store.activity_listing_revisions listings
      on listings.listing_revision_digest = runs.listing_revision_digest
    left join platform_store.activity_run_terminal_evidence terminals
      on terminals.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_results results
      on results.activity_run_id = runs.activity_run_id
    left join platform_store.indexed_activity_result_payloads payloads
      on payloads.activity_result_id = results.activity_result_id
    left join platform_store.public_projection_relay_bindings bindings
      on bindings.activity_run_id = runs.activity_run_id
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
        select 1 from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = facts.activity_run_id
          and receipts.disposition = 'conflict'
      ) or exists (
        select 1 from platform_store.activity_run_index_events events
        where events.activity_run_id = facts.activity_run_id
          and (
            events.disposition = 'conflict'
            or events.event_kind in (
              'terminal_conflict', 'result_conflict', 'integrity_conflict',
              'result_suppressed_privacy', 'result_payload_purged'
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
            'result_published', 'result_confirmed', 'integrity_observed',
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
      and facts.public_relay_bound
    then 'live'
    else 'unavailable'
  end
  from visibility facts;
$$;

create or replace function platform_store.public_run_dto_v1(p_activity_run_id uuid)
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
    then jsonb_build_object('version', 'public_run.v1', 'state', 'unavailable')
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
      'participants', platform_store.public_run_attributions_v1(selected.activity_run_id),
      'live', case when selected.public_state = 'live'
        then jsonb_build_object('available', true)
      end,
      'result', case when selected.public_state = 'result'
        then selected.result_summary
      end
    ))
  end
  from selected;
$$;

revoke all on platform_store.public_projection_relay_bindings
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_public_relay_binding_candidate_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_public_relay_binding_v1(
  uuid, bytea, bytea, bytea, bytea
) from public, anon, authenticated, service_role;

grant select, insert on platform_store.public_projection_relay_bindings to service_role;
grant execute on function platform_api.read_public_relay_binding_candidate_v1(uuid)
  to service_role;
grant execute on function platform_api.record_public_relay_binding_v1(
  uuid, bytea, bytea, bytea, bytea
) to service_role;
