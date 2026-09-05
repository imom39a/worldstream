create table platform_store.activity_run_terminal_evidence (
  activity_run_id uuid primary key
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  listing_revision_digest text not null
    references platform_store.activity_listing_revisions(listing_revision_digest)
    on delete restrict,
  result_projector_revision_digest text not null,
  projector_status text not null,
  result_indexer_membership_id text not null,
  source_head jsonb not null,
  source_room_seq bigint not null,
  source_projection_hash text not null,
  integrity_status text not null,
  integrity_generation bigint not null,
  host_evidence_digest bytea not null,
  canonical_terminal_evidence bytea not null,
  terminal_evidence_digest bytea not null,
  observed_at timestamptz not null default clock_timestamp(),
  constraint terminal_projector_digest_shape check (
    result_projector_revision_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint terminal_projector_status check (
    projector_status in ('terminal_without_outcome', 'summary')
  ),
  constraint terminal_membership_shape check (
    result_indexer_membership_id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
  ),
  constraint terminal_head_bounds check (
    jsonb_typeof(source_head) = 'object'
    and octet_length(source_head::text) between 2 and 4096
  ),
  constraint terminal_sequence_bound check (
    source_room_seq between 0 and 9007199254740991
  ),
  constraint terminal_projection_hash_shape check (
    source_projection_hash ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint terminal_integrity_status check (
    integrity_status in ('healthy', 'faulted', 'quarantined')
  ),
  constraint terminal_integrity_generation_bound check (
    integrity_generation between 0 and 9007199254740991
  ),
  constraint terminal_host_evidence_sha256 check (
    octet_length(host_evidence_digest) = 32
  ),
  constraint terminal_evidence_bounds check (
    octet_length(canonical_terminal_evidence) between 2 and 262144
  ),
  constraint terminal_evidence_sha256 check (
    octet_length(terminal_evidence_digest) = 32
  )
);

create table platform_store.indexed_activity_results (
  activity_result_id uuid primary key default gen_random_uuid(),
  activity_run_id uuid not null unique
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  listing_revision_digest text not null
    references platform_store.activity_listing_revisions(listing_revision_digest)
    on delete restrict,
  pack_revision_digest text not null,
  result_publication_policy text not null,
  result_projector_revision_digest text not null,
  result_output_schema text not null,
  result_output_schema_digest text not null,
  result_canonicalizer_version text not null,
  result_indexer_membership_id text not null,
  source_head jsonb not null,
  source_room_seq bigint not null,
  integrity_generation bigint not null,
  projection_hash text not null,
  replay_verifier_revision text not null,
  replay_verified_head jsonb not null,
  replay_projection_hash text not null,
  replay_receipt_digest bytea not null,
  host_evidence_digest bytea not null,
  result_payload_digest bytea not null,
  indexed_at timestamptz not null default clock_timestamp(),
  constraint indexed_result_pack_digest_shape check (
    pack_revision_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint indexed_result_publication_policy check (
    result_publication_policy = 'public_recent_results'
  ),
  constraint indexed_result_projector_digest_shape check (
    result_projector_revision_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint indexed_result_output_schema_bounds check (
    length(result_output_schema) between 1 and 128
    and result_output_schema !~ '[[:cntrl:]]'
  ),
  constraint indexed_result_output_digest_shape check (
    result_output_schema_digest ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint indexed_result_canonicalizer check (
    result_canonicalizer_version = 'worldstream/canonical-json/v1'
  ),
  constraint indexed_result_membership_shape check (
    result_indexer_membership_id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
  ),
  constraint indexed_result_head_bounds check (
    jsonb_typeof(source_head) = 'object'
    and jsonb_typeof(replay_verified_head) = 'object'
    and octet_length(source_head::text) between 2 and 4096
    and octet_length(replay_verified_head::text) between 2 and 4096
  ),
  constraint indexed_result_sequence_bound check (
    source_room_seq between 0 and 9007199254740991
    and integrity_generation between 0 and 9007199254740991
  ),
  constraint indexed_result_projection_hashes check (
    projection_hash ~ '^blake3:[0-9a-f]{64}$'
    and replay_projection_hash = projection_hash
  ),
  constraint indexed_result_replay_verifier check (
    replay_verifier_revision = 'worldstream.authorized-replay/v1'
  ),
  constraint indexed_result_digest_lengths check (
    octet_length(replay_receipt_digest) = 32
    and octet_length(host_evidence_digest) = 32
    and octet_length(result_payload_digest) = 32
  )
);

create table platform_store.indexed_activity_result_payloads (
  activity_result_id uuid primary key
    references platform_store.indexed_activity_results(activity_result_id)
    on delete restrict,
  result_output_schema text not null,
  canonical_payload bytea not null,
  payload_digest bytea not null,
  inserted_at timestamptz not null default clock_timestamp(),
  constraint indexed_payload_schema_bounds check (
    length(result_output_schema) between 1 and 128
    and result_output_schema !~ '[[:cntrl:]]'
  ),
  constraint indexed_payload_bounds check (
    octet_length(canonical_payload) between 2 and 65536
  ),
  constraint indexed_payload_sha256 check (octet_length(payload_digest) = 32)
);

create table platform_store.activity_run_index_events (
  activity_run_index_event_id uuid primary key default gen_random_uuid(),
  activity_run_id uuid not null
    references platform_store.activity_runs(activity_run_id) on delete restrict,
  activity_result_id uuid
    references platform_store.indexed_activity_results(activity_result_id)
    on delete restrict,
  event_kind text not null,
  source_head jsonb not null,
  source_room_seq bigint not null,
  projection_hash text not null,
  integrity_status text not null,
  integrity_generation bigint not null,
  replay_receipt_digest bytea,
  evidence_digest bytea not null,
  disposition text not null,
  safe_code text not null,
  observed_at timestamptz not null default clock_timestamp(),
  constraint run_index_event_kind check (event_kind in (
    'terminal_observed',
    'terminal_confirmed',
    'terminal_conflict',
    'result_published',
    'result_confirmed',
    'result_conflict',
    'integrity_observed',
    'integrity_conflict',
    'result_suppressed',
    'result_reverified'
  )),
  constraint run_index_event_head_bounds check (
    jsonb_typeof(source_head) = 'object'
    and octet_length(source_head::text) between 2 and 4096
  ),
  constraint run_index_event_sequence_bound check (
    source_room_seq between 0 and 9007199254740991
    and integrity_generation between 0 and 9007199254740991
  ),
  constraint run_index_event_projection_hash_shape check (
    projection_hash ~ '^blake3:[0-9a-f]{64}$'
  ),
  constraint run_index_event_integrity_status check (
    integrity_status in ('healthy', 'faulted', 'quarantined')
  ),
  constraint run_index_event_digest_lengths check (
    (replay_receipt_digest is null or octet_length(replay_receipt_digest) = 32)
    and octet_length(evidence_digest) = 32
  ),
  constraint run_index_event_disposition check (
    disposition in ('applied', 'duplicate', 'conflict', 'blocked')
  ),
  constraint run_index_event_safe_code_shape check (
    safe_code ~ '^[a-z][a-z0-9_]{0,63}$'
  ),
  unique (activity_run_id, event_kind, evidence_digest)
);

create index indexed_activity_results_recent_listing_idx
  on platform_store.indexed_activity_results (
    listing_revision_digest,
    indexed_at desc,
    activity_result_id
  );
create index activity_run_index_events_reconciliation_idx
  on platform_store.activity_run_index_events (
    activity_run_id,
    integrity_generation desc,
    observed_at desc,
    activity_run_index_event_id
  );

alter table platform_store.activity_run_terminal_evidence enable row level security;
alter table platform_store.indexed_activity_results enable row level security;
alter table platform_store.indexed_activity_result_payloads enable row level security;
alter table platform_store.activity_run_index_events enable row level security;

create function platform_store.valid_complete_result_head_v1(
  p_head jsonb,
  p_room_id text,
  p_pack_digest text
)
returns boolean
language plpgsql
immutable
security invoker
set search_path = ''
as $$
declare
  v_room_seq bigint;
begin
  if p_head is null
    or jsonb_typeof(p_head) <> 'object'
    or not (p_head ?& array[
      'room_id',
      'room_seq',
      'genesis_or_transition_hash',
      'core_schema_version',
      'pack_digest',
      'core_state_hash',
      'activity_state_hash',
      'authoritative_state_hash'
    ])
    or exists (
      select 1 from jsonb_object_keys(p_head) keys(key)
      where keys.key not in (
        'room_id',
        'room_seq',
        'genesis_or_transition_hash',
        'core_schema_version',
        'pack_digest',
        'core_state_hash',
        'activity_state_hash',
        'authoritative_state_hash'
      )
    )
    or p_head ->> 'room_id' <> p_room_id
    or p_head ->> 'pack_digest' <> p_pack_digest
    or p_head ->> 'core_schema_version' <> 'worldstream.core-room-state.v1'
    or p_head ->> 'genesis_or_transition_hash' !~ '^blake3:[0-9a-f]{64}$'
    or p_head ->> 'core_state_hash' !~ '^blake3:[0-9a-f]{64}$'
    or p_head ->> 'activity_state_hash' !~ '^blake3:[0-9a-f]{64}$'
    or p_head ->> 'authoritative_state_hash' !~ '^blake3:[0-9a-f]{64}$'
    or jsonb_typeof(p_head -> 'room_seq') <> 'number'
  then
    return false;
  end if;
  begin
    v_room_seq := (p_head ->> 'room_seq')::bigint;
  exception when others then
    return false;
  end;
  return v_room_seq between 0 and 9007199254740991;
end;
$$;

create trigger protect_activity_run_terminal_evidence_v1
before update or delete on platform_store.activity_run_terminal_evidence
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create trigger protect_indexed_activity_result_v1
before update or delete on platform_store.indexed_activity_results
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create trigger protect_indexed_activity_result_payload_v1
before update or delete on platform_store.indexed_activity_result_payloads
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create trigger protect_activity_run_index_event_v1
before update or delete on platform_store.activity_run_index_events
for each row execute function platform_store.reject_immutable_activity_run_row_v1();

create function platform_api.read_terminal_reconciliation_v1(
  p_activity_run_id uuid
)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_terminal_reconciliation.v1',
    'run_id', runs.activity_run_id,
    'launch_request_id', runs.launch_request_id,
    'listing_revision_digest', runs.listing_revision_digest,
    'terminal_recorded', terminals.activity_run_id is not null,
    'projector_status', terminals.projector_status,
    'source_room_seq', terminals.source_room_seq,
    'source_projection_hash', terminals.source_projection_hash,
    'terminal_observed_at', terminals.observed_at,
    'reconciliation_state', case
      when runs.initial_reconciliation_state = 'quarantined'
        or exists (
          select 1
          from platform_store.reconciliation_receipts receipts
          where receipts.activity_run_id = runs.activity_run_id
            and receipts.disposition = 'conflict'
        )
      then 'quarantined'
      when terminals.activity_run_id is null then 'pending'
      else 'terminal'
    end,
    'needs_result_source_pull',
      runs.initial_reconciliation_state = 'ready'
      and terminals.activity_run_id is null
      and not exists (
        select 1
        from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id
          and receipts.disposition = 'conflict'
      )
  ))
  from platform_store.activity_runs runs
  left join platform_store.activity_run_terminal_evidence terminals
    on terminals.activity_run_id = runs.activity_run_id
  where runs.activity_run_id = p_activity_run_id;
$$;

create function platform_api.read_result_reconciliation_v1(
  p_activity_run_id uuid
)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  with latest_integrity as (
    select events.integrity_status, events.integrity_generation,
      events.event_kind, events.observed_at
    from platform_store.activity_run_index_events events
    where events.activity_run_id = p_activity_run_id
    order by events.integrity_generation desc,
      events.observed_at desc,
      events.activity_run_index_event_id desc
    limit 1
  )
  select jsonb_strip_nulls(jsonb_build_object(
    'version', 'platform_result_reconciliation.v1',
    'run_id', runs.activity_run_id,
    'listing_revision_digest', runs.listing_revision_digest,
    'terminal_recorded', terminals.activity_run_id is not null,
    'projector_status', terminals.projector_status,
    'result_recorded', results.activity_result_id is not null,
    'activity_result_id', results.activity_result_id,
    'result_output_schema', results.result_output_schema,
    'result_payload_digest', case
      when results.result_payload_digest is null then null
      else 'sha256:' || encode(results.result_payload_digest, 'hex')
    end,
    'result_summary', case
      when payloads.canonical_payload is null then null
      else convert_from(payloads.canonical_payload, 'utf8')::jsonb
    end,
    'integrity_status', latest_integrity.integrity_status,
    'integrity_generation', latest_integrity.integrity_generation,
    'publishable',
      results.activity_result_id is not null
      and runs.initial_reconciliation_state = 'ready'
      and latest_integrity.integrity_status = 'healthy'
      and latest_integrity.event_kind in (
        'result_published',
        'result_confirmed',
        'integrity_observed',
        'result_reverified'
      )
      and not exists (
        select 1
        from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id
          and receipts.disposition = 'conflict'
      ),
    'needs_result_source_pull',
      runs.initial_reconciliation_state = 'ready'
      and (
        terminals.activity_run_id is null
        or (terminals.projector_status = 'summary' and results.activity_result_id is null)
        or latest_integrity.observed_at < statement_timestamp() - interval '5 minutes'
      )
      and not exists (
        select 1
        from platform_store.reconciliation_receipts receipts
        where receipts.activity_run_id = runs.activity_run_id
          and receipts.disposition = 'conflict'
      )
  ))
  from platform_store.activity_runs runs
  left join platform_store.activity_run_terminal_evidence terminals
    on terminals.activity_run_id = runs.activity_run_id
  left join platform_store.indexed_activity_results results
    on results.activity_run_id = runs.activity_run_id
  left join platform_store.indexed_activity_result_payloads payloads
    on payloads.activity_result_id = results.activity_result_id
  left join latest_integrity on true
  where runs.activity_run_id = p_activity_run_id;
$$;

create function platform_api.read_integrity_reconciliation_v1(
  p_activity_run_id uuid
)
returns jsonb
language sql
stable
security invoker
set search_path = ''
as $$
  select jsonb_build_object(
    'version', 'platform_integrity_reconciliation.v1',
    'run_id', runs.activity_run_id,
    'integrity_status', events.integrity_status,
    'integrity_generation', events.integrity_generation,
    'source_room_seq', events.source_room_seq,
    'projection_hash', events.projection_hash,
    'last_event_kind', events.event_kind,
    'observed_at', events.observed_at,
    'result_suppressed',
      events.event_kind = 'result_suppressed'
      or events.integrity_status in ('faulted', 'quarantined')
  )
  from platform_store.activity_runs runs
  join lateral (
    select indexed.*
    from platform_store.activity_run_index_events indexed
    where indexed.activity_run_id = runs.activity_run_id
    order by indexed.integrity_generation desc,
      indexed.observed_at desc,
      indexed.activity_run_index_event_id desc
    limit 1
  ) events on true
  where runs.activity_run_id = p_activity_run_id;
$$;

create function platform_api.list_reconciliation_candidates_v1(
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
      1 as priority,
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
  order by priority, due_at, launch_request_id
  limit least(greatest(coalesce(p_limit, 0), 0), 100);
$$;

create function platform_api.record_run_terminal_v1(
  p_activity_run_id uuid,
  p_canonical_terminal_evidence bytea,
  p_terminal_evidence_digest bytea
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  run_row platform_store.activity_runs%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  existing platform_store.activity_run_terminal_evidence%rowtype;
  evidence jsonb;
  v_head jsonb;
  v_sequence bigint;
  v_generation bigint;
  v_host_digest bytea;
  v_event text;
  v_disposition text;
  v_code text;
  v_now timestamptz := clock_timestamp();
begin
  if p_activity_run_id is null
    or octet_length(p_canonical_terminal_evidence) not between 2 and 262144
    or octet_length(p_terminal_evidence_digest) <> 32
    or p_terminal_evidence_digest
      <> extensions.digest(p_canonical_terminal_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_terminal_evidence';
  end if;
  begin
    evidence := convert_from(p_canonical_terminal_evidence, 'utf8')::jsonb;
    v_sequence := (evidence #>> '{source_head,room_seq}')::bigint;
    v_generation := (evidence ->> 'integrity_generation')::bigint;
    v_host_digest := decode(substring(evidence ->> 'host_evidence_digest' from 8), 'hex');
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_terminal_evidence';
  end;
  if jsonb_typeof(evidence) <> 'object'
    or not (evidence ?& array[
      'schema',
      'run_id',
      'listing_revision_digest',
      'result_projector_revision_digest',
      'projector_status',
      'host_installation_id',
      'room_id',
      'result_indexer_membership_id',
      'source_head',
      'integrity_status',
      'integrity_generation',
      'projection_schema',
      'projection_hash',
      'host_evidence_digest'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema',
        'run_id',
        'listing_revision_digest',
        'result_projector_revision_digest',
        'projector_status',
        'host_installation_id',
        'room_id',
        'result_indexer_membership_id',
        'source_head',
        'integrity_status',
        'integrity_generation',
        'projection_schema',
        'projection_hash',
        'host_evidence_digest'
      )
    )
    or evidence ->> 'schema' <> 'worldstream/platform-terminal-observation/v1'
    or evidence ->> 'run_id' <> p_activity_run_id::text
    or evidence ->> 'listing_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'result_projector_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'projector_status' not in ('terminal_without_outcome', 'summary')
    or evidence ->> 'host_installation_id' !~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$'
    or evidence ->> 'room_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or evidence ->> 'result_indexer_membership_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or evidence ->> 'integrity_status' not in ('healthy', 'faulted', 'quarantined')
    or v_generation not between 0 and 9007199254740991
    or length(evidence ->> 'projection_schema') not between 1 and 128
    or evidence ->> 'projection_schema' ~ '[[:cntrl:]]'
    or evidence ->> 'projection_hash' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'host_evidence_digest' !~ '^sha256:[0-9a-f]{64}$'
    or octet_length(v_host_digest) <> 32
  then
    raise exception using errcode = '22023', message = 'invalid_terminal_evidence';
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('run:' || p_activity_run_id::text, 0)
  );
  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  if not found then
    raise exception using errcode = '55000', message = 'run_not_found';
  end if;
  select listings.* into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = run_row.listing_revision_digest;
  v_head := evidence -> 'source_head';
  if run_row.initial_reconciliation_state <> 'ready'
    or evidence ->> 'listing_revision_digest' <> run_row.listing_revision_digest
    or evidence ->> 'result_projector_revision_digest'
      <> listing_row.result_projector_revision_digest
    or evidence ->> 'host_installation_id' <> run_row.host_installation_id
    or evidence ->> 'room_id' <> run_row.room_id
    or evidence ->> 'projection_schema' <> listing_row.public_projection_schema
    or not platform_store.valid_complete_result_head_v1(
      v_head, run_row.room_id, run_row.pack_revision_digest
    )
    or not exists (
      select 1 from platform_store.activity_run_memberships memberships
      where memberships.activity_run_id = p_activity_run_id
        and memberships.membership_id = evidence ->> 'result_indexer_membership_id'
        and memberships.access_mode = 'spectator'
        and memberships.purpose = 'result_indexer'
    )
  then
    raise exception using errcode = '55000', message = 'terminal_identity_mismatch';
  end if;

  select terminals.* into existing
  from platform_store.activity_run_terminal_evidence terminals
  where terminals.activity_run_id = p_activity_run_id;
  if found then
    if existing.result_projector_revision_digest
        = evidence ->> 'result_projector_revision_digest'
      and existing.projector_status = evidence ->> 'projector_status'
      and v_sequence >= existing.source_room_seq
      and (
        v_sequence > existing.source_room_seq
        or (
          v_head = existing.source_head
          and evidence ->> 'projection_hash' = existing.source_projection_hash
        )
      )
    then
      v_event := 'terminal_confirmed';
      v_disposition := 'duplicate';
      v_code := 'terminal_already_recorded';
    else
      v_event := 'terminal_conflict';
      v_disposition := 'conflict';
      v_code := 'terminal_evidence_conflict';
    end if;
  else
    insert into platform_store.activity_run_terminal_evidence (
      activity_run_id,
      listing_revision_digest,
      result_projector_revision_digest,
      projector_status,
      result_indexer_membership_id,
      source_head,
      source_room_seq,
      source_projection_hash,
      integrity_status,
      integrity_generation,
      host_evidence_digest,
      canonical_terminal_evidence,
      terminal_evidence_digest,
      observed_at
    ) values (
      p_activity_run_id,
      run_row.listing_revision_digest,
      evidence ->> 'result_projector_revision_digest',
      evidence ->> 'projector_status',
      evidence ->> 'result_indexer_membership_id',
      v_head,
      v_sequence,
      evidence ->> 'projection_hash',
      evidence ->> 'integrity_status',
      v_generation,
      v_host_digest,
      p_canonical_terminal_evidence,
      p_terminal_evidence_digest,
      v_now
    );
    update platform_store.capacity_reservations reservations
    set released_at = v_now,
        release_reason = 'projector_terminal'
    where reservations.activity_run_id = p_activity_run_id
      and reservations.kind = 'active_run'
      and reservations.released_at is null;
    v_event := 'terminal_observed';
    v_disposition := 'applied';
    v_code := 'terminal_recorded';
  end if;

  insert into platform_store.activity_run_index_events (
    activity_run_id,
    event_kind,
    source_head,
    source_room_seq,
    projection_hash,
    integrity_status,
    integrity_generation,
    evidence_digest,
    disposition,
    safe_code,
    observed_at
  ) values (
    p_activity_run_id,
    v_event,
    v_head,
    v_sequence,
    evidence ->> 'projection_hash',
    evidence ->> 'integrity_status',
    v_generation,
    p_terminal_evidence_digest,
    v_disposition,
    v_code,
    v_now
  ) on conflict do nothing;
  insert into platform_store.reconciliation_receipts (
    receipt_kind,
    source_key,
    launch_request_id,
    activity_run_id,
    evidence_digest,
    disposition,
    safe_code,
    observed_at
  ) values (
    'terminal',
    'run:' || p_activity_run_id::text || '/room-seq:' || v_sequence::text,
    run_row.launch_request_id,
    p_activity_run_id,
    p_terminal_evidence_digest,
    v_disposition,
    v_code,
    v_now
  ) on conflict do nothing;
  return jsonb_build_object(
    'version', 'platform_terminal_record.v1',
    'run_id', p_activity_run_id,
    'terminal_recorded', true,
    'projector_status', coalesce(existing.projector_status, evidence ->> 'projector_status'),
    'disposition', v_disposition,
    'safe_code', v_code
  );
end;
$$;

create function platform_api.record_terminal_conflict_v1(
  p_activity_run_id uuid,
  p_canonical_conflict_evidence bytea,
  p_conflict_evidence_digest bytea
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  run_row platform_store.activity_runs%rowtype;
  terminal_row platform_store.activity_run_terminal_evidence%rowtype;
  evidence jsonb;
  v_head jsonb;
  v_sequence bigint;
  v_generation bigint;
  v_now timestamptz := clock_timestamp();
begin
  if p_activity_run_id is null
    or octet_length(p_canonical_conflict_evidence) not between 2 and 262144
    or octet_length(p_conflict_evidence_digest) <> 32
    or p_conflict_evidence_digest
      <> extensions.digest(p_canonical_conflict_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_terminal_conflict';
  end if;
  begin
    evidence := convert_from(p_canonical_conflict_evidence, 'utf8')::jsonb;
    v_head := evidence -> 'source_head';
    v_sequence := (v_head ->> 'room_seq')::bigint;
    v_generation := (evidence ->> 'integrity_generation')::bigint;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_terminal_conflict';
  end;
  if jsonb_typeof(evidence) <> 'object'
    or not (evidence ?& array[
      'schema', 'run_id', 'listing_revision_digest',
      'result_projector_revision_digest', 'conflict_code', 'source_head',
      'integrity_status', 'integrity_generation', 'projection_hash',
      'host_evidence_digest'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema', 'run_id', 'listing_revision_digest',
        'result_projector_revision_digest', 'conflict_code', 'source_head',
        'integrity_status', 'integrity_generation', 'projection_hash',
        'host_evidence_digest'
      )
    )
    or evidence ->> 'schema' <> 'worldstream/platform-terminal-conflict/v1'
    or evidence ->> 'run_id' <> p_activity_run_id::text
    or evidence ->> 'conflict_code' not in (
      'nonterminal_reversion', 'terminal_disagreement'
    )
    or evidence ->> 'integrity_status' not in ('healthy', 'faulted', 'quarantined')
    or v_generation not between 0 and 9007199254740991
    or evidence ->> 'projection_hash' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'host_evidence_digest' !~ '^sha256:[0-9a-f]{64}$'
  then
    raise exception using errcode = '22023', message = 'invalid_terminal_conflict';
  end if;
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('run:' || p_activity_run_id::text, 0)
  );
  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  select terminals.* into terminal_row
  from platform_store.activity_run_terminal_evidence terminals
  where terminals.activity_run_id = p_activity_run_id;
  if run_row.activity_run_id is null
    or terminal_row.activity_run_id is null
    or evidence ->> 'listing_revision_digest' <> run_row.listing_revision_digest
    or evidence ->> 'result_projector_revision_digest'
      <> terminal_row.result_projector_revision_digest
    or not platform_store.valid_complete_result_head_v1(
      v_head, run_row.room_id, run_row.pack_revision_digest
    )
  then
    raise exception using errcode = '55000', message = 'terminal_conflict_identity_mismatch';
  end if;
  insert into platform_store.activity_run_index_events (
    activity_run_id, event_kind, source_head, source_room_seq, projection_hash,
    integrity_status, integrity_generation, evidence_digest, disposition,
    safe_code, observed_at
  ) values (
    p_activity_run_id, 'terminal_conflict', v_head, v_sequence,
    evidence ->> 'projection_hash', evidence ->> 'integrity_status', v_generation,
    p_conflict_evidence_digest, 'conflict', evidence ->> 'conflict_code', v_now
  ) on conflict do nothing;
  insert into platform_store.reconciliation_receipts (
    receipt_kind, source_key, launch_request_id, activity_run_id,
    evidence_digest, disposition, safe_code, observed_at
  ) values (
    'terminal',
    'run:' || p_activity_run_id::text || '/room-seq:' || v_sequence::text,
    run_row.launch_request_id,
    p_activity_run_id,
    p_conflict_evidence_digest,
    'conflict',
    evidence ->> 'conflict_code',
    v_now
  ) on conflict do nothing;
  return jsonb_build_object(
    'version', 'platform_terminal_record.v1',
    'run_id', p_activity_run_id,
    'terminal_recorded', true,
    'projector_status', terminal_row.projector_status,
    'disposition', 'conflict',
    'safe_code', evidence ->> 'conflict_code'
  );
end;
$$;

create function platform_api.record_result_v1(
  p_activity_run_id uuid,
  p_canonical_result_evidence bytea,
  p_result_evidence_digest bytea,
  p_canonical_result_payload bytea,
  p_result_payload_digest bytea
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  run_row platform_store.activity_runs%rowtype;
  listing_row platform_store.activity_listing_revisions%rowtype;
  terminal_row platform_store.activity_run_terminal_evidence%rowtype;
  existing platform_store.indexed_activity_results%rowtype;
  evidence jsonb;
  payload jsonb;
  v_source_head jsonb;
  v_replay_head jsonb;
  v_sequence bigint;
  v_generation bigint;
  v_replay_receipt bytea;
  v_host_digest bytea;
  v_result_id uuid;
  v_event text;
  v_disposition text;
  v_code text;
  v_now timestamptz := clock_timestamp();
begin
  if p_activity_run_id is null
    or octet_length(p_canonical_result_evidence) not between 2 and 262144
    or octet_length(p_result_evidence_digest) <> 32
    or p_result_evidence_digest <> extensions.digest(p_canonical_result_evidence, 'sha256')
    or octet_length(p_canonical_result_payload) not between 2 and 65536
    or octet_length(p_result_payload_digest) <> 32
    or p_result_payload_digest <> extensions.digest(p_canonical_result_payload, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_result_evidence';
  end if;
  begin
    evidence := convert_from(p_canonical_result_evidence, 'utf8')::jsonb;
    payload := convert_from(p_canonical_result_payload, 'utf8')::jsonb;
    v_source_head := evidence -> 'source_head';
    v_replay_head := evidence -> 'replay_verified_head';
    v_sequence := (v_source_head ->> 'room_seq')::bigint;
    v_generation := (evidence ->> 'integrity_generation')::bigint;
    v_replay_receipt := decode(
      substring(evidence ->> 'replay_receipt_digest' from 8), 'hex'
    );
    v_host_digest := decode(
      substring(evidence ->> 'host_evidence_digest' from 8), 'hex'
    );
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_result_evidence';
  end;
  if jsonb_typeof(evidence) <> 'object'
    or jsonb_typeof(payload) <> 'object'
    or not (evidence ?& array[
      'schema',
      'run_id',
      'listing_revision_digest',
      'result_projector_revision_digest',
      'result_output_schema',
      'result_output_schema_digest',
      'result_canonicalizer_version',
      'result_indexer_membership_id',
      'source_head',
      'integrity_status',
      'integrity_generation',
      'projection_hash',
      'replay_verifier_revision',
      'replay_verified_head',
      'replay_projection_hash',
      'replay_receipt_digest',
      'host_evidence_digest',
      'summary_digest'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema',
        'run_id',
        'listing_revision_digest',
        'result_projector_revision_digest',
        'result_output_schema',
        'result_output_schema_digest',
        'result_canonicalizer_version',
        'result_indexer_membership_id',
        'source_head',
        'integrity_status',
        'integrity_generation',
        'projection_hash',
        'replay_verifier_revision',
        'replay_verified_head',
        'replay_projection_hash',
        'replay_receipt_digest',
        'host_evidence_digest',
        'summary_digest'
      )
    )
    or evidence ->> 'schema' <> 'worldstream/platform-indexed-result-evidence/v1'
    or evidence ->> 'run_id' <> p_activity_run_id::text
    or evidence ->> 'listing_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'result_projector_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or length(evidence ->> 'result_output_schema') not between 1 and 128
    or evidence ->> 'result_output_schema' ~ '[[:cntrl:]]'
    or evidence ->> 'result_output_schema_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'result_canonicalizer_version' <> 'worldstream/canonical-json/v1'
    or evidence ->> 'result_indexer_membership_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or evidence ->> 'integrity_status' <> 'healthy'
    or v_generation not between 0 and 9007199254740991
    or evidence ->> 'projection_hash' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'replay_verifier_revision' <> 'worldstream.authorized-replay/v1'
    or evidence ->> 'replay_projection_hash' <> evidence ->> 'projection_hash'
    or evidence ->> 'replay_receipt_digest' !~ '^sha256:[0-9a-f]{64}$'
    or evidence ->> 'host_evidence_digest' !~ '^sha256:[0-9a-f]{64}$'
    or evidence ->> 'summary_digest' !~ '^sha256:[0-9a-f]{64}$'
    or evidence ->> 'summary_digest'
      <> 'sha256:' || encode(p_result_payload_digest, 'hex')
    or octet_length(v_replay_receipt) <> 32
    or octet_length(v_host_digest) <> 32
    or v_source_head <> v_replay_head
    or evidence ->> 'projection_hash' <> evidence ->> 'replay_projection_hash'
  then
    raise exception using errcode = '22023', message = 'invalid_result_evidence';
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('run:' || p_activity_run_id::text, 0)
  );
  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  if not found then
    raise exception using errcode = '55000', message = 'run_not_found';
  end if;
  select listings.* into strict listing_row
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest = run_row.listing_revision_digest;
  select terminals.* into terminal_row
  from platform_store.activity_run_terminal_evidence terminals
  where terminals.activity_run_id = p_activity_run_id;
  if terminal_row.activity_run_id is null
    or terminal_row.projector_status <> 'summary'
    or run_row.initial_reconciliation_state <> 'ready'
    or listing_row.result_publication_policy <> 'public_recent_results'
    or evidence ->> 'listing_revision_digest' <> run_row.listing_revision_digest
    or evidence ->> 'result_projector_revision_digest'
      <> listing_row.result_projector_revision_digest
    or evidence ->> 'result_projector_revision_digest'
      <> terminal_row.result_projector_revision_digest
    or evidence ->> 'result_output_schema' <> listing_row.result_output_schema
    or evidence ->> 'result_output_schema_digest' <> listing_row.result_output_schema_digest
    or evidence ->> 'result_canonicalizer_version' <> listing_row.result_canonicalizer_version
    or octet_length(p_canonical_result_payload) > listing_row.result_output_max_bytes
    or v_sequence < terminal_row.source_room_seq
    or not platform_store.valid_complete_result_head_v1(
      v_source_head, run_row.room_id, run_row.pack_revision_digest
    )
    or not exists (
      select 1 from platform_store.activity_run_memberships memberships
      where memberships.activity_run_id = p_activity_run_id
        and memberships.membership_id = evidence ->> 'result_indexer_membership_id'
        and memberships.access_mode = 'spectator'
        and memberships.purpose = 'result_indexer'
    )
    or exists (
      select 1 from platform_store.reconciliation_receipts receipts
      where receipts.activity_run_id = p_activity_run_id
        and receipts.disposition = 'conflict'
    )
  then
    raise exception using errcode = '55000', message = 'result_publication_blocked';
  end if;

  select results.* into existing
  from platform_store.indexed_activity_results results
  where results.activity_run_id = p_activity_run_id;
  if found then
    v_result_id := existing.activity_result_id;
    if existing.result_payload_digest = p_result_payload_digest
      and existing.result_projector_revision_digest
        = evidence ->> 'result_projector_revision_digest'
      and v_sequence >= existing.source_room_seq
      and (
        v_sequence > existing.source_room_seq
        or (
          v_source_head = existing.source_head
          and evidence ->> 'projection_hash' = existing.projection_hash
        )
      )
    then
      v_event := 'result_confirmed';
      v_disposition := 'duplicate';
      v_code := 'result_already_recorded';
    else
      v_event := 'result_conflict';
      v_disposition := 'conflict';
      v_code := 'result_summary_conflict';
    end if;
  else
    insert into platform_store.indexed_activity_results (
      activity_run_id,
      listing_revision_digest,
      pack_revision_digest,
      result_publication_policy,
      result_projector_revision_digest,
      result_output_schema,
      result_output_schema_digest,
      result_canonicalizer_version,
      result_indexer_membership_id,
      source_head,
      source_room_seq,
      integrity_generation,
      projection_hash,
      replay_verifier_revision,
      replay_verified_head,
      replay_projection_hash,
      replay_receipt_digest,
      host_evidence_digest,
      result_payload_digest,
      indexed_at
    ) values (
      p_activity_run_id,
      run_row.listing_revision_digest,
      run_row.pack_revision_digest,
      listing_row.result_publication_policy,
      listing_row.result_projector_revision_digest,
      listing_row.result_output_schema,
      listing_row.result_output_schema_digest,
      listing_row.result_canonicalizer_version,
      evidence ->> 'result_indexer_membership_id',
      v_source_head,
      v_sequence,
      v_generation,
      evidence ->> 'projection_hash',
      evidence ->> 'replay_verifier_revision',
      v_replay_head,
      evidence ->> 'replay_projection_hash',
      v_replay_receipt,
      v_host_digest,
      p_result_payload_digest,
      v_now
    ) returning activity_result_id into v_result_id;
    insert into platform_store.indexed_activity_result_payloads (
      activity_result_id,
      result_output_schema,
      canonical_payload,
      payload_digest,
      inserted_at
    ) values (
      v_result_id,
      listing_row.result_output_schema,
      p_canonical_result_payload,
      p_result_payload_digest,
      v_now
    );
    v_event := 'result_published';
    v_disposition := 'applied';
    v_code := 'result_recorded';
  end if;

  insert into platform_store.activity_run_index_events (
    activity_run_id, activity_result_id, event_kind, source_head,
    source_room_seq, projection_hash, integrity_status, integrity_generation,
    replay_receipt_digest, evidence_digest, disposition, safe_code, observed_at
  ) values (
    p_activity_run_id, v_result_id, v_event, v_source_head,
    v_sequence, evidence ->> 'projection_hash', 'healthy', v_generation,
    v_replay_receipt, p_result_evidence_digest, v_disposition, v_code, v_now
  ) on conflict do nothing;
  insert into platform_store.reconciliation_receipts (
    receipt_kind, source_key, launch_request_id, activity_run_id,
    evidence_digest, disposition, safe_code, observed_at
  ) values (
    'result',
    'run:' || p_activity_run_id::text || '/room-seq:' || v_sequence::text,
    run_row.launch_request_id,
    p_activity_run_id,
    p_result_evidence_digest,
    v_disposition,
    v_code,
    v_now
  ) on conflict do nothing;
  return jsonb_build_object(
    'version', 'platform_result_record.v1',
    'run_id', p_activity_run_id,
    'activity_result_id', v_result_id,
    'result_recorded', true,
    'disposition', v_disposition,
    'safe_code', v_code
  );
end;
$$;

create function platform_api.record_integrity_observation_v1(
  p_activity_run_id uuid,
  p_canonical_integrity_evidence bytea,
  p_integrity_evidence_digest bytea
)
returns jsonb
language plpgsql
security invoker
set search_path = ''
as $$
declare
  run_row platform_store.activity_runs%rowtype;
  terminal_row platform_store.activity_run_terminal_evidence%rowtype;
  result_row platform_store.indexed_activity_results%rowtype;
  latest_event platform_store.activity_run_index_events%rowtype;
  evidence jsonb;
  replay jsonb;
  v_head jsonb;
  v_replay_head jsonb;
  v_sequence bigint;
  v_generation bigint;
  v_replay_receipt bytea;
  v_event text;
  v_disposition text;
  v_code text;
  v_now timestamptz := clock_timestamp();
begin
  if p_activity_run_id is null
    or octet_length(p_canonical_integrity_evidence) not between 2 and 262144
    or octet_length(p_integrity_evidence_digest) <> 32
    or p_integrity_evidence_digest
      <> extensions.digest(p_canonical_integrity_evidence, 'sha256')
  then
    raise exception using errcode = '22023', message = 'invalid_integrity_evidence';
  end if;
  begin
    evidence := convert_from(p_canonical_integrity_evidence, 'utf8')::jsonb;
    replay := evidence -> 'replay';
    v_head := evidence -> 'source_head';
    v_sequence := (v_head ->> 'room_seq')::bigint;
    v_generation := (evidence ->> 'integrity_generation')::bigint;
  exception when others then
    raise exception using errcode = '22023', message = 'invalid_integrity_evidence';
  end;
  if jsonb_typeof(evidence) <> 'object'
    or not (evidence ?& array[
      'schema', 'run_id', 'listing_revision_digest',
      'result_indexer_membership_id', 'source_head', 'integrity_status',
      'integrity_generation', 'projection_hash', 'host_evidence_digest',
      'replay', 'result_payload_digest'
    ])
    or exists (
      select 1 from jsonb_object_keys(evidence) keys(key)
      where keys.key not in (
        'schema', 'run_id', 'listing_revision_digest',
        'result_indexer_membership_id', 'source_head', 'integrity_status',
        'integrity_generation', 'projection_hash', 'host_evidence_digest',
        'replay', 'result_payload_digest'
      )
    )
    or evidence ->> 'schema' <> 'worldstream/platform-run-integrity-observation/v1'
    or evidence ->> 'run_id' <> p_activity_run_id::text
    or evidence ->> 'listing_revision_digest' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'result_indexer_membership_id' !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
    or evidence ->> 'integrity_status' not in ('healthy', 'faulted', 'quarantined')
    or v_generation not between 0 and 9007199254740991
    or evidence ->> 'projection_hash' !~ '^blake3:[0-9a-f]{64}$'
    or evidence ->> 'host_evidence_digest' !~ '^sha256:[0-9a-f]{64}$'
    or (
      jsonb_typeof(evidence -> 'result_payload_digest') <> 'null'
      and evidence ->> 'result_payload_digest' !~ '^sha256:[0-9a-f]{64}$'
    )
  then
    raise exception using errcode = '22023', message = 'invalid_integrity_evidence';
  end if;
  if jsonb_typeof(replay) <> 'null' then
    begin
      v_replay_head := replay -> 'verified_head';
      v_replay_receipt := decode(
        substring(replay ->> 'verification_receipt_digest' from 8), 'hex'
      );
    exception when others then
      raise exception using errcode = '22023', message = 'invalid_integrity_evidence';
    end;
    if jsonb_typeof(replay) <> 'object'
      or not (replay ?& array[
        'verifier_revision', 'verified_head', 'projection_hash',
        'verification_receipt_digest'
      ])
      or exists (
        select 1 from jsonb_object_keys(replay) keys(key)
        where keys.key not in (
          'verifier_revision', 'verified_head', 'projection_hash',
          'verification_receipt_digest'
        )
      )
      or replay ->> 'verifier_revision' <> 'worldstream.authorized-replay/v1'
      or replay ->> 'projection_hash' <> evidence ->> 'projection_hash'
      or replay ->> 'verification_receipt_digest' !~ '^sha256:[0-9a-f]{64}$'
      or octet_length(v_replay_receipt) <> 32
      or v_replay_head <> v_head
    then
      raise exception using errcode = '22023', message = 'invalid_integrity_evidence';
    end if;
  end if;

  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('run:' || p_activity_run_id::text, 0)
  );
  select runs.* into run_row
  from platform_store.activity_runs runs
  where runs.activity_run_id = p_activity_run_id;
  select terminals.* into terminal_row
  from platform_store.activity_run_terminal_evidence terminals
  where terminals.activity_run_id = p_activity_run_id;
  select results.* into result_row
  from platform_store.indexed_activity_results results
  where results.activity_run_id = p_activity_run_id;
  if run_row.activity_run_id is null
    or terminal_row.activity_run_id is null
    or evidence ->> 'listing_revision_digest' <> run_row.listing_revision_digest
    or not platform_store.valid_complete_result_head_v1(
      v_head, run_row.room_id, run_row.pack_revision_digest
    )
    or not exists (
      select 1 from platform_store.activity_run_memberships memberships
      where memberships.activity_run_id = p_activity_run_id
        and memberships.membership_id = evidence ->> 'result_indexer_membership_id'
        and memberships.access_mode = 'spectator'
        and memberships.purpose = 'result_indexer'
    )
  then
    raise exception using errcode = '55000', message = 'integrity_identity_mismatch';
  end if;

  select events.* into latest_event
  from platform_store.activity_run_index_events events
  where events.activity_run_id = p_activity_run_id
  order by events.integrity_generation desc,
    events.observed_at desc,
    events.activity_run_index_event_id desc
  limit 1;
  if latest_event.activity_run_index_event_id is not null
    and v_generation < latest_event.integrity_generation
  then
    return jsonb_build_object(
      'version', 'platform_integrity_record.v1',
      'run_id', p_activity_run_id,
      'integrity_status', latest_event.integrity_status,
      'integrity_generation', latest_event.integrity_generation,
      'disposition', 'duplicate',
      'safe_code', 'stale_integrity_observation'
    );
  end if;
  if latest_event.activity_run_index_event_id is not null
    and v_generation = latest_event.integrity_generation
  then
    if latest_event.integrity_status = evidence ->> 'integrity_status'
      and latest_event.source_head = v_head
      and latest_event.projection_hash = evidence ->> 'projection_hash'
    then
      return jsonb_build_object(
        'version', 'platform_integrity_record.v1',
        'run_id', p_activity_run_id,
        'integrity_status', latest_event.integrity_status,
        'integrity_generation', latest_event.integrity_generation,
        'disposition', 'duplicate',
        'safe_code', 'integrity_already_recorded'
      );
    end if;
    v_event := 'integrity_conflict';
    v_disposition := 'conflict';
    v_code := 'integrity_generation_conflict';
  elsif evidence ->> 'integrity_status' in ('faulted', 'quarantined') then
    v_event := case
      when result_row.activity_result_id is null then 'integrity_observed'
      else 'result_suppressed'
    end;
    v_disposition := 'applied';
    v_code := case
      when result_row.activity_result_id is null then 'integrity_recorded'
      else 'result_suppressed'
    end;
  elsif result_row.activity_result_id is not null then
    if jsonb_typeof(replay) <> 'object'
      or evidence ->> 'result_payload_digest'
        <> 'sha256:' || encode(result_row.result_payload_digest, 'hex')
      or v_sequence < result_row.source_room_seq
    then
      return jsonb_build_object(
        'version', 'platform_integrity_record.v1',
        'run_id', p_activity_run_id,
        'integrity_status', latest_event.integrity_status,
        'integrity_generation', latest_event.integrity_generation,
        'disposition', 'blocked',
        'safe_code', 'integrity_reverification_incomplete'
      );
    end if;
    v_event := 'result_reverified';
    v_disposition := 'applied';
    v_code := 'result_reverified';
  else
    v_event := 'integrity_observed';
    v_disposition := 'applied';
    v_code := 'integrity_recorded';
  end if;

  insert into platform_store.activity_run_index_events (
    activity_run_id, activity_result_id, event_kind, source_head,
    source_room_seq, projection_hash, integrity_status, integrity_generation,
    replay_receipt_digest, evidence_digest, disposition, safe_code, observed_at
  ) values (
    p_activity_run_id, result_row.activity_result_id, v_event, v_head,
    v_sequence, evidence ->> 'projection_hash', evidence ->> 'integrity_status',
    v_generation, v_replay_receipt, p_integrity_evidence_digest,
    v_disposition, v_code, v_now
  ) on conflict do nothing;
  insert into platform_store.reconciliation_receipts (
    receipt_kind, source_key, launch_request_id, activity_run_id,
    evidence_digest, disposition, safe_code, observed_at
  ) values (
    'integrity',
    'run:' || p_activity_run_id::text || '/generation:' || v_generation::text,
    run_row.launch_request_id,
    p_activity_run_id,
    p_integrity_evidence_digest,
    v_disposition,
    v_code,
    v_now
  ) on conflict do nothing;
  return jsonb_build_object(
    'version', 'platform_integrity_record.v1',
    'run_id', p_activity_run_id,
    'integrity_status', evidence ->> 'integrity_status',
    'integrity_generation', v_generation,
    'disposition', v_disposition,
    'safe_code', v_code
  );
end;
$$;

revoke all on platform_store.activity_run_terminal_evidence
  from public, anon, authenticated, service_role;
revoke all on platform_store.indexed_activity_results
  from public, anon, authenticated, service_role;
revoke all on platform_store.indexed_activity_result_payloads
  from public, anon, authenticated, service_role;
revoke all on platform_store.activity_run_index_events
  from public, anon, authenticated, service_role;

revoke execute on function platform_store.valid_complete_result_head_v1(jsonb, text, text)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_terminal_reconciliation_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_result_reconciliation_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.read_integrity_reconciliation_v1(uuid)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.list_reconciliation_candidates_v1(integer)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_run_terminal_v1(uuid, bytea, bytea)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_terminal_conflict_v1(uuid, bytea, bytea)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_result_v1(uuid, bytea, bytea, bytea, bytea)
  from public, anon, authenticated, service_role;
revoke execute on function platform_api.record_integrity_observation_v1(uuid, bytea, bytea)
  from public, anon, authenticated, service_role;

grant select, insert on platform_store.activity_run_terminal_evidence to service_role;
grant select, insert on platform_store.indexed_activity_results to service_role;
grant select, insert on platform_store.indexed_activity_result_payloads to service_role;
grant select, insert on platform_store.activity_run_index_events to service_role;

grant execute on function platform_store.valid_complete_result_head_v1(jsonb, text, text)
  to service_role;
grant execute on function platform_api.read_terminal_reconciliation_v1(uuid)
  to service_role;
grant execute on function platform_api.read_result_reconciliation_v1(uuid)
  to service_role;
grant execute on function platform_api.read_integrity_reconciliation_v1(uuid)
  to service_role;
grant execute on function platform_api.list_reconciliation_candidates_v1(integer)
  to service_role;
grant execute on function platform_api.record_run_terminal_v1(uuid, bytea, bytea)
  to service_role;
grant execute on function platform_api.record_terminal_conflict_v1(uuid, bytea, bytea)
  to service_role;
grant execute on function platform_api.record_result_v1(uuid, bytea, bytea, bytea, bytea)
  to service_role;
grant execute on function platform_api.record_integrity_observation_v1(uuid, bytea, bytea)
  to service_role;
