begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_function('platform_api', 'read_public_run_v1', array['text']);
select has_function('platform_api', 'list_recent_results_v1', array['integer']);
select ok(
  has_function_privilege(
    'service_role', 'platform_api.read_public_run_v1(text)', 'execute'
  )
  and not has_function_privilege(
    'anon', 'platform_api.read_public_run_v1(text)', 'execute'
  )
  and not has_function_privilege(
    'authenticated', 'platform_api.read_public_run_v1(text)', 'execute'
  ),
  'only the server-secret role can build a public Run DTO'
);

insert into auth.users(id) values
  ('81000000-0000-4000-8000-000000000001'),
  ('81000000-0000-4000-8000-000000000002');

insert into platform_store.platform_accounts(account_id) values
  ('82000000-0000-4000-8000-000000000001'),
  ('82000000-0000-4000-8000-000000000002');

insert into platform_store.github_identities (
  auth_user_id,
  account_id,
  provider_subject,
  github_login,
  avatar_url,
  public_profile_enabled
) values
(
  '81000000-0000-4000-8000-000000000001',
  '82000000-0000-4000-8000-000000000001',
  'public-run-human',
  'visible-human',
  'https://avatars.githubusercontent.com/u/8101',
  true
),
(
  '81000000-0000-4000-8000-000000000002',
  '82000000-0000-4000-8000-000000000002',
  'public-run-agent-owner',
  'hidden-controller',
  null,
  false
);

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
  'blake3:8181818181818181818181818181818181818181818181818181818181818181',
  'worldstream.agent-heist.result-only-test',
  convert_to('{"title":"Agent Heist","description":"Result-only public test"}', 'utf8'),
  'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820',
  'sha256:88f76571d3d5736f0ca52740d619761f9ad6dcb9112792e806d8072c2a983148',
  'heist-web',
  'unlisted',
  'must_claim_seat',
  'disabled',
  'public_recent_results',
  'blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf',
  'agent-heist/projection/v1',
  'blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f',
  'worldstream/result-summary/v1',
  'blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6',
  'worldstream/canonical-json/v1',
  4096,
  '{"roles":["navigator"]}'::jsonb,
  '[{"seat_id":"navigator","role":"navigator","display_name":"Navigator","required":true,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
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
) values
(
  '83000000-0000-4000-8000-000000000001',
  '82000000-0000-4000-8000-000000000001',
  'blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1',
  'public-run-test',
  decode(repeat('11', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'navigator',
  'account_human',
  'run_created',
  '2026-09-06 10:00:00+00'::timestamptz,
  '2026-09-05 10:00:00+00'::timestamptz,
  '2026-09-05 10:00:02+00'::timestamptz,
  '2026-09-05 10:00:01+00'::timestamptz,
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  convert_to('{}', 'utf8'),
  'blake3:' || repeat('1', 64),
  'public-run-host',
  'public-run-operation',
  '2026-09-05 10:00:02+00'::timestamptz
),
(
  '83000000-0000-4000-8000-000000000002',
  '82000000-0000-4000-8000-000000000001',
  'blake3:8181818181818181818181818181818181818181818181818181818181818181',
  'public-run-test',
  decode(repeat('22', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'navigator',
  'account_human',
  'run_created',
  '2026-09-06 11:00:00+00'::timestamptz,
  '2026-09-05 11:00:00+00'::timestamptz,
  '2026-09-05 11:00:02+00'::timestamptz,
  '2026-09-05 11:00:01+00'::timestamptz,
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  convert_to('{}', 'utf8'),
  'blake3:' || repeat('2', 64),
  'public-run-host',
  'result-only-operation',
  '2026-09-05 11:00:02+00'::timestamptz
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
  initial_reconciliation_state,
  genesis_observed_at
) values
(
  '84000000-0000-4000-8000-000000000001',
  '83000000-0000-4000-8000-000000000001',
  'blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1',
  '82000000-0000-4000-8000-000000000001',
  'public-run-host',
  'public-run-operation',
  '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'blake3:' || repeat('3', 64),
  'worldstream.agent-heist',
  '0.2.0',
  'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820',
  0,
  'blake3:' || repeat('4', 64),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  repeat('a', 32),
  'unranked',
  'ready',
  '2026-09-05 10:00:03+00'::timestamptz
),
(
  '84000000-0000-4000-8000-000000000002',
  '83000000-0000-4000-8000-000000000002',
  'blake3:8181818181818181818181818181818181818181818181818181818181818181',
  '82000000-0000-4000-8000-000000000001',
  'public-run-host',
  'result-only-operation',
  '01ARZ3NDEKTSV4RRFFQ69G5FAW',
  'blake3:' || repeat('5', 64),
  'worldstream.agent-heist',
  '0.2.0',
  'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820',
  0,
  'blake3:' || repeat('6', 64),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  repeat('b', 32),
  'unranked',
  'ready',
  '2026-09-05 11:00:03+00'::timestamptz
);

insert into platform_store.activity_run_memberships (
  activity_run_id,
  membership_id,
  access_mode,
  purpose,
  seat_id,
  role,
  public_seat_label,
  principal_kind,
  participation_source,
  controlling_account_id,
  principal_id,
  entry_selector,
  service_scope_digest
) values
(
  '84000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FAX',
  'participant',
  'participant',
  'navigator',
  'navigator',
  'Navigator',
  'human',
  'account_human',
  '82000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FAY',
  repeat('1', 32),
  null
),
(
  '84000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FAZ',
  'participant',
  'participant',
  'insider',
  'insider',
  'Insider',
  'agent',
  'account_external_agent',
  '82000000-0000-4000-8000-000000000002',
  '01ARZ3NDEKTSV4RRFFQ69G5FB0',
  repeat('2', 32),
  null
),
(
  '84000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FB1',
  'spectator',
  'result_indexer',
  null,
  null,
  null,
  'agent',
  null,
  null,
  '01ARZ3NDEKTSV4RRFFQ69G5FB2',
  null,
  extensions.digest(convert_to('result-indexer', 'utf8'), 'sha256')
),
(
  '84000000-0000-4000-8000-000000000002',
  '01ARZ3NDEKTSV4RRFFQ69G5FB3',
  'participant',
  'participant',
  'navigator',
  'navigator',
  'Navigator',
  'human',
  'account_human',
  '82000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FB4',
  repeat('3', 32),
  null
);

insert into platform_store.indexed_activity_results (
  activity_result_id,
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
  '85000000-0000-4000-8000-000000000001',
  '84000000-0000-4000-8000-000000000001',
  'blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1',
  'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820',
  'public_recent_results',
  'blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf',
  'worldstream/result-summary/v1',
  'blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6',
  'worldstream/canonical-json/v1',
  '01ARZ3NDEKTSV4RRFFQ69G5FB1',
  '{}'::jsonb,
  8,
  1,
  'blake3:' || repeat('7', 64),
  'worldstream.authorized-replay/v1',
  '{}'::jsonb,
  'blake3:' || repeat('7', 64),
  decode(repeat('8', 64), 'hex'),
  decode(repeat('9', 64), 'hex'),
  decode(repeat('a', 64), 'hex'),
  '2026-09-05 10:05:00+00'::timestamptz
);

insert into platform_store.indexed_activity_result_payloads (
  activity_result_id,
  result_output_schema,
  canonical_payload,
  payload_digest
) values (
  '85000000-0000-4000-8000-000000000001',
  'worldstream/result-summary/v1',
  convert_to('{"status":"summary","summary":{"schema":"worldstream/result-summary/v1","outcome":"success","selected_plan_id":"plan-alpha","score":5,"reason":"scored_selected_plan"}}', 'utf8'),
  decode(repeat('a', 64), 'hex')
);

insert into platform_store.activity_run_index_events (
  activity_run_id,
  activity_result_id,
  event_kind,
  source_head,
  source_room_seq,
  projection_hash,
  integrity_status,
  integrity_generation,
  replay_receipt_digest,
  evidence_digest,
  disposition,
  safe_code,
  observed_at
) values (
  '84000000-0000-4000-8000-000000000001',
  '85000000-0000-4000-8000-000000000001',
  'result_published',
  '{}'::jsonb,
  8,
  'blake3:' || repeat('7', 64),
  'healthy',
  1,
  decode(repeat('8', 64), 'hex'),
  decode(repeat('b', 64), 'hex'),
  'applied',
  'result_published',
  '2026-09-05 10:05:00+00'::timestamptz
);

set local role service_role;

select is(
  platform_api.read_public_run_v1(repeat('a', 32)) ->> 'state',
  'result',
  'a Replay-verified healthy result is public'
);
select is(
  platform_api.read_public_run_v1(repeat('a', 32))
    #>> '{participants,1,identity,login}',
  'visible-human',
  'an opted-in current GitHub profile is joined at read time'
);
select is(
  platform_api.read_public_run_v1(repeat('a', 32))
    #>> '{participants,0,notice}',
  'External agent — unverified',
  'an external agent publishes no claimed model identity'
);
select ok(
  platform_api.read_public_run_v1(repeat('a', 32))::text
    !~ '(room_id|membership_id|principal_id|account_id|entry_selector|replay|prompt|provider_response)',
  'the public DTO contains no private correspondence or execution material'
);
select is(
  platform_api.read_public_run_v1(repeat('b', 32)) #>> '{live,available}',
  'false',
  'a result-only public ID cannot resolve to live Fly viewing'
);
select is(
  jsonb_array_length(platform_api.list_recent_results_v1(100) -> 'results'),
  1,
  'Recent Results is bounded and includes the public Agent Heist result'
);
select is(
  platform_api.list_recent_results_v1(100) ->> 'maximum',
  '20',
  'the public feed advertises its hard maximum of twenty'
);

reset role;
update platform_store.github_identities
set public_profile_enabled = false
where auth_user_id = '81000000-0000-4000-8000-000000000001';
set local role service_role;
select is(
  platform_api.read_public_run_v1(repeat('a', 32))
    #>> '{participants,1,identity,kind}',
  'pseudonym',
  'profile disable falls back to the reviewed seat pseudonym'
);

reset role;
update platform_store.platform_accounts
set erased_at = clock_timestamp()
where account_id = '82000000-0000-4000-8000-000000000001';
update platform_store.github_identities
set erasure_requested_at = clock_timestamp(), public_profile_enabled = false
where auth_user_id = '81000000-0000-4000-8000-000000000001';
set local role service_role;
select is(
  platform_api.read_public_run_v1(repeat('a', 32))
    #>> '{participants,1,identity,kind}',
  'pseudonym',
  'account erasure retains shared history with pseudonymous attribution'
);

reset role;
insert into platform_store.activity_run_index_events (
  activity_run_id,
  activity_result_id,
  event_kind,
  source_head,
  source_room_seq,
  projection_hash,
  integrity_status,
  integrity_generation,
  evidence_digest,
  disposition,
  safe_code
) values (
  '84000000-0000-4000-8000-000000000001',
  '85000000-0000-4000-8000-000000000001',
  'result_suppressed_privacy',
  '{}'::jsonb,
  8,
  'blake3:' || repeat('7', 64),
  'healthy',
  1,
  decode(repeat('c', 64), 'hex'),
  'applied',
  'privacy_suppressed'
),
(
  '84000000-0000-4000-8000-000000000001',
  '85000000-0000-4000-8000-000000000001',
  'result_reverified',
  '{}'::jsonb,
  9,
  'blake3:' || repeat('7', 64),
  'healthy',
  2,
  decode(repeat('d', 64), 'hex'),
  'applied',
  'result_reverified'
);
set local role service_role;
select is(
  platform_api.read_public_run_v1(repeat('a', 32)),
  '{"version":"public_run.v1","state":"unavailable"}'::jsonb,
  'privacy suppression returns only the generic unavailable state'
);
select is(
  jsonb_array_length(platform_api.list_recent_results_v1(20) -> 'results'),
  0,
  'a permanently suppressed result leaves Recent Results'
);

reset role;
select throws_ok(
  $$insert into platform_store.activity_run_index_events (
      activity_run_id, activity_result_id, event_kind, source_head,
      source_room_seq, projection_hash, integrity_status,
      integrity_generation, evidence_digest, disposition, safe_code
    ) values (
      '84000000-0000-4000-8000-000000000001',
      '85000000-0000-4000-8000-000000000001',
      'unreviewed_public_event', '{}'::jsonb, 9,
      'blake3:7777777777777777777777777777777777777777777777777777777777777777',
      'healthy', 3, decode(repeat('e', 64), 'hex'), 'applied', 'invalid'
    )$$,
  '23514',
  null,
  'only reviewed public visibility event kinds are accepted'
);

select * from finish();
rollback;
