begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table(
  'platform_store',
  'activity_run_terminal_evidence',
  'terminal evidence is retained separately from Room truth'
);
select has_table(
  'platform_store',
  'indexed_activity_results',
  'one immutable result index exists'
);
select has_table(
  'platform_store',
  'indexed_activity_result_payloads',
  'bounded result payloads are retained separately'
);
select has_table(
  'platform_store',
  'activity_run_index_events',
  'result and integrity reconciliation is append-only'
);
select ok(
  (select bool_and(classes.relrowsecurity)
   from pg_class classes
   join pg_namespace namespaces on namespaces.oid = classes.relnamespace
   where namespaces.nspname = 'platform_store'
     and classes.relname in (
       'activity_run_terminal_evidence',
       'indexed_activity_results',
       'indexed_activity_result_payloads',
       'activity_run_index_events'
     )),
  'every result reconciliation table has RLS enabled'
);
select ok(
  not has_table_privilege('anon', 'platform_store.indexed_activity_results', 'select')
  and not has_table_privilege(
    'authenticated', 'platform_store.indexed_activity_results', 'select'
  )
  and not has_table_privilege(
    'anon', 'platform_store.indexed_activity_result_payloads', 'select'
  )
  and not has_table_privilege(
    'authenticated', 'platform_store.activity_run_index_events', 'select'
  ),
  'browser roles cannot read private result reconciliation rows'
);
select has_function('platform_api', 'record_run_terminal_v1', array['uuid', 'bytea', 'bytea']);
select has_function(
  'platform_api',
  'record_result_v1',
  array['uuid', 'bytea', 'bytea', 'bytea', 'bytea']
);
select has_function(
  'platform_api',
  'record_integrity_observation_v1',
  array['uuid', 'bytea', 'bytea']
);
select has_function(
  'platform_api',
  'record_terminal_conflict_v1',
  array['uuid', 'bytea', 'bytea']
);
select has_function('platform_api', 'list_reconciliation_candidates_v1', array['integer']);
select ok(
  has_function_privilege(
    'service_role', 'platform_api.record_result_v1(uuid,bytea,bytea,bytea,bytea)',
    'execute'
  )
  and not has_function_privilege(
    'anon', 'platform_api.record_result_v1(uuid,bytea,bytea,bytea,bytea)',
    'execute'
  )
  and not has_function_privilege(
    'authenticated', 'platform_api.record_result_v1(uuid,bytea,bytea,bytea,bytea)',
    'execute'
  ),
  'only the service role can record a result'
);

insert into platform_store.platform_accounts(account_id)
values ('70000000-0000-4000-8000-000000000001');

insert into platform_store.activity_listing_revisions (
  listing_revision_digest,
  listing_key,
  canonical_document,
  pack_revision_digest,
  client_release_digest,
  client_surface_id,
  catalog_visibility,
  creator_access,
  public_viewing_policy,
  result_publication_policy,
  result_projector_revision_digest,
  public_projection_schema,
  public_projection_schema_digest,
  result_output_schema,
  result_output_schema_digest,
  result_canonicalizer_version,
  result_output_max_bytes,
  room_setup_configuration,
  seat_templates,
  allow_multiple_seats_per_account,
  pre_start_deadline_seconds
) values (
  'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'worldstream.test.results',
  convert_to('{}', 'utf8'),
  'blake3:8888888888888888888888888888888888888888888888888888888888888888',
  'sha256:7777777777777777777777777777777777777777777777777777777777777777',
  'test-results-web',
  'public',
  'must_claim_seat',
  'anonymous_by_link',
  'public_recent_results',
  'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'test/projection/v1',
  'blake3:5555555555555555555555555555555555555555555555555555555555555555',
  'test/result/v1',
  'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'worldstream/canonical-json/v1',
  1024,
  '{}'::jsonb,
  '[{"seat_id":"host","role":"host","display_name":"Host","required":true,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
  false,
  1800
);

insert into platform_store.launch_requests (
  launch_request_id,
  creator_account_id,
  listing_revision_digest,
  idempotency_namespace,
  idempotency_key_digest,
  canonical_launch_input,
  launch_input_digest,
  canonicalizer_version,
  house_fill_choice,
  creator_access_choice,
  creator_seat_id,
  creator_participation_kind,
  state,
  expires_at,
  created_at,
  last_transition_at,
  roster_frozen_at,
  frozen_roster,
  frozen_roster_digest,
  frozen_room_setup_specification,
  frozen_room_setup_specification_digest,
  host_installation_id,
  room_setup_operation_id,
  host_mutation_started_at
) values (
  '71000000-0000-4000-8000-000000000001',
  '70000000-0000-4000-8000-000000000001',
  'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'results-test',
  decode(repeat('11', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'host',
  'account_human',
  'run_created',
  '2026-09-06 00:00:00+00'::timestamptz,
  '2026-09-05 00:00:00+00'::timestamptz,
  '2026-09-05 00:00:01+00'::timestamptz,
  '2026-09-05 00:00:01+00'::timestamptz,
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  convert_to('{}', 'utf8'),
  'blake3:3333333333333333333333333333333333333333333333333333333333333333',
  'result-test-host',
  'result-test-operation',
  '2026-09-05 00:00:02+00'::timestamptz
);

insert into platform_store.activity_runs (
  activity_run_id,
  launch_request_id,
  listing_revision_digest,
  creator_account_id,
  host_installation_id,
  room_setup_operation_id,
  room_id,
  launch_request_digest,
  pack_id,
  pack_version,
  pack_revision_digest,
  genesis_room_seq,
  genesis_or_transition_hash,
  canonical_genesis_evidence,
  genesis_evidence_digest,
  public_id,
  evidence_class,
  initial_reconciliation_state
) values (
  '72000000-0000-4000-8000-000000000001',
  '71000000-0000-4000-8000-000000000001',
  'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  '70000000-0000-4000-8000-000000000001',
  'result-test-host',
  'result-test-operation',
  '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'worldstream.test',
  '1.0.0',
  'blake3:8888888888888888888888888888888888888888888888888888888888888888',
  0,
  'blake3:1111111111111111111111111111111111111111111111111111111111111111',
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  repeat('a', 32),
  'unranked',
  'ready'
);

insert into platform_store.activity_run_memberships (
  activity_run_id,
  membership_id,
  access_mode,
  purpose,
  principal_kind,
  principal_id,
  service_scope_digest
) values (
  '72000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'spectator',
  'result_indexer',
  'agent',
  '01ARZ3NDEKTSV4RRFFQ69G5FAY',
  extensions.digest(convert_to('result-indexer-scopes', 'utf8'), 'sha256')
);

insert into platform_store.capacity_reservations (
  launch_request_id,
  kind,
  reservation_class,
  controlling_account_id,
  activity_run_id
) values (
  '71000000-0000-4000-8000-000000000001',
  'active_run',
  'authorized',
  '70000000-0000-4000-8000-000000000001',
  '72000000-0000-4000-8000-000000000001'
);

create temporary table result_heads (
  room_seq bigint primary key,
  head jsonb not null
);
insert into result_heads(room_seq, head)
select room_seq, jsonb_build_object(
  'room_id', '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'room_seq', room_seq,
  'genesis_or_transition_hash', 'blake3:' || repeat(substr(room_seq::text, 1, 1), 64),
  'core_schema_version', 'worldstream.core-room-state.v1',
  'pack_digest', 'blake3:8888888888888888888888888888888888888888888888888888888888888888',
  'core_state_hash', 'blake3:' || repeat('a', 64),
  'activity_state_hash', 'blake3:' || repeat('b', 64),
  'authoritative_state_hash', 'blake3:' || repeat('c', 64)
)
from generate_series(14, 18) room_seq;

create temporary table terminal_documents (
  name text primary key,
  document bytea not null
);
insert into terminal_documents(name, document)
select 'initial', convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-terminal-observation/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_projector_revision_digest', 'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'projector_status', 'summary',
  'host_installation_id', 'result-test-host',
  'room_id', '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 14),
  'integrity_status', 'healthy',
  'integrity_generation', 1,
  'projection_schema', 'test/projection/v1',
  'projection_hash', 'blake3:' || repeat('d', 64),
  'host_evidence_digest', 'sha256:' || repeat('e', 64)
)::text, 'utf8')
union all
select 'later', convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-terminal-observation/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_projector_revision_digest', 'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'projector_status', 'summary',
  'host_installation_id', 'result-test-host',
  'room_id', '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 15),
  'integrity_status', 'healthy',
  'integrity_generation', 2,
  'projection_schema', 'test/projection/v1',
  'projection_hash', 'blake3:' || repeat('f', 64),
  'host_evidence_digest', 'sha256:' || repeat('1', 64)
)::text, 'utf8');

grant select on result_heads, terminal_documents to service_role;

set local role service_role;

select is(
  platform_api.read_terminal_reconciliation_v1(
    '72000000-0000-4000-8000-000000000001'
  ) ->> 'reconciliation_state',
  'pending',
  'a ready Run begins without duplicated terminal truth'
);

create temporary table first_terminal as
select platform_api.record_run_terminal_v1(
  '72000000-0000-4000-8000-000000000001',
  document,
  extensions.digest(document, 'sha256')
) as receipt
from terminal_documents where name = 'initial';
select is(
  (select receipt ->> 'safe_code' from first_terminal),
  'terminal_recorded',
  'the first projector terminal observation is recorded'
);
select is(
  (select count(*)::integer
   from platform_store.capacity_reservations
   where activity_run_id = '72000000-0000-4000-8000-000000000001'
     and released_at is null),
  0,
  'the first terminal observation releases active capacity atomically'
);
select is(
  (select release_reason
   from platform_store.capacity_reservations
   where activity_run_id = '72000000-0000-4000-8000-000000000001'),
  'projector_terminal',
  'terminal capacity release has the closed projector reason'
);
select is(
  (platform_api.record_run_terminal_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from terminal_documents where name = 'initial'),
    extensions.digest(
      (select document from terminal_documents where name = 'initial'), 'sha256'
    )
  ) ->> 'disposition'),
  'duplicate',
  'an exact terminal retry is idempotent'
);
select is(
  (platform_api.record_run_terminal_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from terminal_documents where name = 'later'),
    extensions.digest(
      (select document from terminal_documents where name = 'later'), 'sha256'
    )
  ) ->> 'safe_code'),
  'terminal_already_recorded',
  'a later Head can confirm the same terminal classification'
);
select is(
  (select count(*)::integer from platform_store.activity_run_terminal_evidence
   where activity_run_id = '72000000-0000-4000-8000-000000000001'),
  1,
  'terminal truth remains one immutable row'
);

create temporary table result_payloads(name text primary key, payload bytea not null);
insert into result_payloads(name, payload) values
  ('first', convert_to('{"schema":"test/result/v1","result":"alpha"}', 'utf8')),
  ('divergent', convert_to('{"schema":"test/result/v1","result":"beta"}', 'utf8'));

create temporary table result_documents(name text primary key, document bytea not null);
insert into result_documents(name, document)
select 'initial', convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-indexed-result-evidence/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_projector_revision_digest', 'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'result_output_schema', 'test/result/v1',
  'result_output_schema_digest', 'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'result_canonicalizer_version', 'worldstream/canonical-json/v1',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 15),
  'integrity_status', 'healthy',
  'integrity_generation', 2,
  'projection_hash', 'blake3:' || repeat('f', 64),
  'replay_verifier_revision', 'worldstream.authorized-replay/v1',
  'replay_verified_head', (select head from result_heads where room_seq = 15),
  'replay_projection_hash', 'blake3:' || repeat('f', 64),
  'replay_receipt_digest', 'sha256:' || repeat('2', 64),
  'host_evidence_digest', 'sha256:' || repeat('3', 64),
  'summary_digest', 'sha256:' || encode(
    extensions.digest((select payload from result_payloads where name = 'first'), 'sha256'),
    'hex'
  )
)::text, 'utf8')
union all
select 'later', convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-indexed-result-evidence/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_projector_revision_digest', 'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'result_output_schema', 'test/result/v1',
  'result_output_schema_digest', 'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'result_canonicalizer_version', 'worldstream/canonical-json/v1',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 16),
  'integrity_status', 'healthy',
  'integrity_generation', 3,
  'projection_hash', 'blake3:' || repeat('0', 64),
  'replay_verifier_revision', 'worldstream.authorized-replay/v1',
  'replay_verified_head', (select head from result_heads where room_seq = 16),
  'replay_projection_hash', 'blake3:' || repeat('0', 64),
  'replay_receipt_digest', 'sha256:' || repeat('4', 64),
  'host_evidence_digest', 'sha256:' || repeat('5', 64),
  'summary_digest', 'sha256:' || encode(
    extensions.digest((select payload from result_payloads where name = 'first'), 'sha256'),
    'hex'
  )
)::text, 'utf8');

create temporary table first_result as
select platform_api.record_result_v1(
  '72000000-0000-4000-8000-000000000001',
  (select document from result_documents where name = 'initial'),
  extensions.digest(
    (select document from result_documents where name = 'initial'), 'sha256'
  ),
  (select payload from result_payloads where name = 'first'),
  extensions.digest(
    (select payload from result_payloads where name = 'first'), 'sha256'
  )
) as receipt;
select is(
  (select receipt ->> 'safe_code' from first_result),
  'result_recorded',
  'healthy equal Projection and Replay hashes publish one result'
);
select ok(
  (platform_api.read_result_reconciliation_v1(
    '72000000-0000-4000-8000-000000000001'
  ) ->> 'publishable')::boolean,
  'the replay-verified result is publishable'
);
select is(
  (platform_api.record_result_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from result_documents where name = 'initial'),
    extensions.digest(
      (select document from result_documents where name = 'initial'), 'sha256'
    ),
    (select payload from result_payloads where name = 'first'),
    extensions.digest(
      (select payload from result_payloads where name = 'first'), 'sha256'
    )
  ) ->> 'disposition'),
  'duplicate',
  'an exact result retry is idempotent'
);
select is(
  (platform_api.record_result_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from result_documents where name = 'later'),
    extensions.digest(
      (select document from result_documents where name = 'later'), 'sha256'
    ),
    (select payload from result_payloads where name = 'first'),
    extensions.digest(
      (select payload from result_payloads where name = 'first'), 'sha256'
    )
  ) ->> 'safe_code'),
  'result_already_recorded',
  'a later replay-verified Head can confirm the same summary'
);
select is(
  (select count(*)::integer from platform_store.indexed_activity_results
   where activity_run_id = '72000000-0000-4000-8000-000000000001'),
  1,
  'a Run has at most one immutable indexed result'
);
reset role;
select throws_ok(
  $$update platform_store.indexed_activity_result_payloads
    set canonical_payload = convert_to('{}', 'utf8')$$,
  '23000',
  'immutable_activity_run_row',
  'a result payload cannot be rewritten'
);
set local role service_role;

create temporary table integrity_documents(name text primary key, document bytea not null);
insert into integrity_documents(name, document)
select 'unhealthy', convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-run-integrity-observation/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 17),
  'integrity_status', 'faulted',
  'integrity_generation', 4,
  'projection_hash', 'blake3:' || repeat('6', 64),
  'host_evidence_digest', 'sha256:' || repeat('7', 64),
  'replay', null,
  'result_payload_digest', null
)::text, 'utf8')
union all
select 'healthy', convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-run-integrity-observation/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 18),
  'integrity_status', 'healthy',
  'integrity_generation', 5,
  'projection_hash', 'blake3:' || repeat('8', 64),
  'host_evidence_digest', 'sha256:' || repeat('9', 64),
  'replay', jsonb_build_object(
    'verifier_revision', 'worldstream.authorized-replay/v1',
    'verified_head', (select head from result_heads where room_seq = 18),
    'projection_hash', 'blake3:' || repeat('8', 64),
    'verification_receipt_digest', 'sha256:' || repeat('a', 64)
  ),
  'result_payload_digest', 'sha256:' || encode(
    extensions.digest((select payload from result_payloads where name = 'first'), 'sha256'),
    'hex'
  )
)::text, 'utf8');

select is(
  (platform_api.record_integrity_observation_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from integrity_documents where name = 'unhealthy'),
    extensions.digest(
      (select document from integrity_documents where name = 'unhealthy'), 'sha256'
    )
  ) ->> 'safe_code'),
  'result_suppressed',
  'a later unhealthy integrity generation suppresses the result'
);
select ok(
  not (platform_api.read_result_reconciliation_v1(
    '72000000-0000-4000-8000-000000000001'
  ) ->> 'publishable')::boolean,
  'an unhealthy result is not publishable'
);
select is(
  (platform_api.record_integrity_observation_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from integrity_documents where name = 'healthy'),
    extensions.digest(
      (select document from integrity_documents where name = 'healthy'), 'sha256'
    )
  ) ->> 'safe_code'),
  'result_reverified',
  'strictly later healthy Replay evidence re-verifies the same result'
);
select ok(
  (platform_api.read_result_reconciliation_v1(
    '72000000-0000-4000-8000-000000000001'
  ) ->> 'publishable')::boolean,
  'a replay-reverified healthy result becomes publishable again'
);

select is(
  (select count(*)::integer from platform_api.list_reconciliation_candidates_v1(1000)),
  0,
  'candidate enumeration is bounded and excludes a freshly reconciled Run'
);

create temporary table divergent_result_document as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-indexed-result-evidence/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_projector_revision_digest', 'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'result_output_schema', 'test/result/v1',
  'result_output_schema_digest', 'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'result_canonicalizer_version', 'worldstream/canonical-json/v1',
  'result_indexer_membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'source_head', (select head from result_heads where room_seq = 18),
  'integrity_status', 'healthy',
  'integrity_generation', 5,
  'projection_hash', 'blake3:' || repeat('8', 64),
  'replay_verifier_revision', 'worldstream.authorized-replay/v1',
  'replay_verified_head', (select head from result_heads where room_seq = 18),
  'replay_projection_hash', 'blake3:' || repeat('8', 64),
  'replay_receipt_digest', 'sha256:' || repeat('a', 64),
  'host_evidence_digest', 'sha256:' || repeat('9', 64),
  'summary_digest', 'sha256:' || encode(
    extensions.digest((select payload from result_payloads where name = 'divergent'), 'sha256'),
    'hex'
  )
)::text, 'utf8') as document;
select is(
  (platform_api.record_result_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from divergent_result_document),
    extensions.digest((select document from divergent_result_document), 'sha256'),
    (select payload from result_payloads where name = 'divergent'),
    extensions.digest(
      (select payload from result_payloads where name = 'divergent'), 'sha256'
    )
  ) ->> 'safe_code'),
  'result_summary_conflict',
  'a divergent later summary records conflict without overwrite'
);
select is(
  convert_from(
    (select payloads.canonical_payload
     from platform_store.indexed_activity_result_payloads payloads
     join platform_store.indexed_activity_results results using (activity_result_id)
     where results.activity_run_id = '72000000-0000-4000-8000-000000000001'),
    'utf8'
  ),
  '{"schema":"test/result/v1","result":"alpha"}',
  'a divergent summary cannot replace the first payload'
);
select ok(
  not (platform_api.read_result_reconciliation_v1(
    '72000000-0000-4000-8000-000000000001'
  ) ->> 'publishable')::boolean,
  'a reconciliation conflict fails publication closed'
);

create temporary table nonterminal_conflict_document as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/platform-terminal-conflict/v1',
  'run_id', '72000000-0000-4000-8000-000000000001',
  'listing_revision_digest', 'blake3:9999999999999999999999999999999999999999999999999999999999999999',
  'result_projector_revision_digest', 'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'conflict_code', 'nonterminal_reversion',
  'source_head', (select head from result_heads where room_seq = 18),
  'integrity_status', 'healthy',
  'integrity_generation', 5,
  'projection_hash', 'blake3:' || repeat('8', 64),
  'host_evidence_digest', 'sha256:' || repeat('9', 64)
)::text, 'utf8') as document;
select is(
  (platform_api.record_terminal_conflict_v1(
    '72000000-0000-4000-8000-000000000001',
    (select document from nonterminal_conflict_document),
    extensions.digest((select document from nonterminal_conflict_document), 'sha256')
  ) ->> 'safe_code'),
  'nonterminal_reversion',
  'a later nonterminal projection is recorded as a permanent conflict'
);
select is(
  (select count(*)::integer from platform_store.activity_run_terminal_evidence
   where activity_run_id = '72000000-0000-4000-8000-000000000001'),
  1,
  'a nonterminal reversion does not erase terminal truth'
);

-- Selection-only fixtures, not evidence submitted to the verifier.
reset role;
create temporary table fairness_runs (position integer, run_id uuid, launch_id uuid);
set local session_replication_role = replica;
do $$
declare
  base_run platform_store.activity_runs%rowtype;
  base_launch platform_store.launch_requests%rowtype;
  launch_id uuid;
  run_id uuid;
begin
  select * into strict base_run from platform_store.activity_runs
  where activity_run_id = '72000000-0000-4000-8000-000000000001';
  select * into strict base_launch from platform_store.launch_requests
  where launch_request_id = base_run.launch_request_id;
  for i in 1..13 loop
    launch_id := gen_random_uuid(); run_id := gen_random_uuid();
    insert into fairness_runs values (i, run_id, launch_id);
    insert into platform_store.launch_requests
    select (jsonb_populate_record(null::platform_store.launch_requests,
      to_jsonb(base_launch) || jsonb_build_object(
        'launch_request_id', launch_id, 'room_setup_operation_id', 'fairness-' || i,
        'idempotency_key_digest', extensions.digest('fairness-' || i, 'sha256')
      ))).*;
    insert into platform_store.activity_runs
    select (jsonb_populate_record(null::platform_store.activity_runs,
      to_jsonb(base_run) || jsonb_build_object(
        'activity_run_id', run_id, 'launch_request_id', launch_id,
        'room_id', repeat('0', 24) || lpad(i::text, 2, '0'),
        'room_setup_operation_id', 'fairness-' || i, 'public_id', null,
        'genesis_observed_at', case when i = 13 then clock_timestamp() else clock_timestamp() - interval '1 day' end,
        'inserted_at', clock_timestamp()
      ))).*;
    if i < 13 then
      insert into platform_store.activity_run_terminal_evidence
      select (jsonb_populate_record(null::platform_store.activity_run_terminal_evidence,
        to_jsonb(terminals) || jsonb_build_object('activity_run_id', run_id))).*
      from platform_store.activity_run_terminal_evidence terminals
      where terminals.activity_run_id = base_run.activity_run_id;
      insert into platform_store.indexed_activity_results
      select (jsonb_populate_record(null::platform_store.indexed_activity_results,
        to_jsonb(results) || jsonb_build_object('activity_run_id', run_id, 'activity_result_id', gen_random_uuid()))).*
      from platform_store.indexed_activity_results results
      where results.activity_run_id = base_run.activity_run_id;
    else
      insert into platform_store.capacity_reservations
        (launch_request_id, kind, controlling_account_id, activity_run_id)
      values (launch_id, 'active_run', base_run.creator_account_id, run_id);
    end if;
  end loop;
end;
$$;
set local session_replication_role = origin;
select is(
  (select activity_run_id from platform_api.list_reconciliation_candidates_v1(1)),
  (select run_id from fairness_runs where position = 13),
  'new active capacity is selected before twelve old published rechecks'
);
create temporary table first_fair_batch as
select * from platform_api.list_reconciliation_candidates_v1(10);
select is((select count(*)::integer from first_fair_batch), 10, 'the recovery batch remains bounded');
select platform_api.mark_reconciliation_attempt_v1(launch_request_id) from first_fair_batch;
select ok(
  exists (
    select 1 from platform_api.list_reconciliation_candidates_v1(10) candidates
    where candidates.activity_run_id not in (select activity_run_id from first_fair_batch)
  ),
  'unsuccessful or unchanged historical attempts rotate instead of starving the next page'
);
set local session_replication_role = replica;
delete from platform_store.indexed_activity_results
where activity_run_id in (select run_id from fairness_runs where position <= 10);
update platform_store.activity_run_terminal_evidence set projector_status = 'terminal_without_outcome'
where activity_run_id in (select run_id from fairness_runs where position <= 10);
set local session_replication_role = origin;
delete from platform_store.reconciliation_attempts
where launch_request_id in (select launch_id from fairness_runs);
select platform_api.mark_reconciliation_attempt_v1(launch_request_id)
from platform_api.list_reconciliation_candidates_v1(10);
select ok(
  exists (
    select 1 from platform_api.list_reconciliation_candidates_v1(10) candidates
    join fairness_runs on candidates.activity_run_id = fairness_runs.run_id
    where fairness_runs.position in (11, 12)
  ),
  'ten terminal-without-outcome Runs cannot starve published integrity rechecks'
);
select finish();
rollback;
