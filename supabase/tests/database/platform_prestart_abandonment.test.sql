begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table(
  'platform_store', 'prestart_abandonments',
  'post-Genesis pre-start abandonment evidence is retained privately'
);
select has_function(
  'platform_api', 'record_prestart_abandonment_v1', array['uuid', 'bytea', 'bytea']
);
select has_function(
  'platform_api', 'list_prestart_abandonment_candidates_v1', array['integer']
);
select ok(
  has_function_privilege(
    'service_role',
    'platform_api.record_prestart_abandonment_v1(uuid, bytea, bytea)', 'execute'
  ) and not has_function_privilege(
    'authenticated',
    'platform_api.record_prestart_abandonment_v1(uuid, bytea, bytea)', 'execute'
  ) and not has_table_privilege('authenticated', 'platform_store.prestart_abandonments', 'select'),
  'only the server can record or read retained abandonment evidence'
);

insert into platform_store.platform_accounts(account_id)
values
  ('93000000-0000-4000-8000-000000000001'),
  ('93000000-0000-4000-8000-000000000002'),
  ('93000000-0000-4000-8000-000000000003');

insert into platform_store.activity_listing_revisions (
  listing_revision_digest, listing_key, canonical_document,
  pack_revision_digest, client_release_digest, client_surface_id,
  catalog_visibility, creator_access, public_viewing_policy,
  result_publication_policy, result_projector_revision_digest,
  public_projection_schema, public_projection_schema_digest,
  result_output_schema, result_output_schema_digest, result_canonicalizer_version,
  result_output_max_bytes, room_setup_configuration, seat_templates,
  allow_multiple_seats_per_account, pre_start_deadline_seconds
) values (
  'blake3:' || repeat('3', 64), 'worldstream.test.prestart-abandonment',
  convert_to('{"title":"Pre-start abandonment test"}', 'utf8'),
  'blake3:' || repeat('4', 64), 'sha256:' || repeat('5', 64), 'test-web',
  'private', 'may_spectate', 'disabled', 'disabled', 'blake3:' || repeat('6', 64),
  'test/projection/v1', 'blake3:' || repeat('7', 64),
  'test/result/v1', 'blake3:' || repeat('8', 64), 'worldstream/canonical-json/v1',
  1024, '{}'::jsonb,
  '[{"seat_id":"observer","role":"observer","display_name":"Observer","required":false,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
  false, 60
);

create function pg_temp.insert_prestart_run(
  p_launch_id uuid,
  p_run_id uuid,
  p_creator_id uuid,
  p_observed_at timestamptz default clock_timestamp()
)
returns void language plpgsql as $$
declare
  suffix text := right(replace(p_launch_id::text, '-', ''), 1);
begin
  insert into platform_store.launch_requests (
    launch_request_id, creator_account_id, listing_revision_digest,
    idempotency_namespace, idempotency_key_digest, canonical_launch_input,
    launch_input_digest, canonicalizer_version, house_fill_choice,
    creator_access_choice, creator_seat_id, creator_participation_kind,
    state, expires_at, created_at, last_transition_at,
    roster_frozen_at, frozen_roster, frozen_roster_digest,
    frozen_room_setup_specification, frozen_room_setup_specification_digest,
    host_installation_id, room_setup_operation_id, host_mutation_started_at
  ) values (
    p_launch_id, p_creator_id, 'blake3:' || repeat('3', 64),
    'prestart-' || replace(p_launch_id::text, '-', ''),
    extensions.digest(convert_to(p_launch_id::text, 'utf8'), 'sha256'),
    convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    'worldstream/canonical-json/v1', 'disabled', 'spectator', null, 'account_human',
    'run_created', p_observed_at + interval '24 hours', p_observed_at, p_observed_at,
    p_observed_at, convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    convert_to('{}', 'utf8'), 'blake3:' || repeat('9', 64),
    'test-host-' || suffix, 'test-operation-' || suffix, p_observed_at
  );
  insert into platform_store.activity_runs (
    activity_run_id, launch_request_id, listing_revision_digest, creator_account_id,
    host_installation_id, room_setup_operation_id, room_id, launch_request_digest,
    pack_id, pack_version, pack_revision_digest, genesis_room_seq,
    genesis_or_transition_hash, canonical_genesis_evidence, genesis_evidence_digest,
    evidence_class, initial_reconciliation_state, genesis_observed_at, inserted_at
  ) values (
    p_run_id, p_launch_id, 'blake3:' || repeat('3', 64), p_creator_id,
    'test-host-' || suffix, 'test-operation-' || suffix,
    substr('01ARZ3NDEKTSV4RRFFQ69G5FAV', 1, 25) || upper(suffix),
    'blake3:' || repeat('a', 64), 'worldstream.test', '1.0.0',
    'blake3:' || repeat('4', 64), 0, 'blake3:' || repeat('b', 64),
    convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    'unranked', 'ready', p_observed_at, p_observed_at
  );
  insert into platform_store.capacity_reservations (
    launch_request_id, kind, reservation_class, controlling_account_id, activity_run_id
  ) values (p_launch_id, 'active_run', 'authorized', p_creator_id, p_run_id);
end;
$$;

select pg_temp.insert_prestart_run(
  '93000000-0000-4000-8000-000000000101',
  '93000000-0000-4000-8000-000000000201',
  '93000000-0000-4000-8000-000000000001'
);

create temporary table exact_evidence as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/hosted-prestart-abandonment-evidence/v1',
  'host_installation_id', runs.host_installation_id,
  'launch_request_id', runs.launch_request_id,
  'listing_revision_digest', runs.listing_revision_digest,
  'launch_request_digest', runs.launch_request_digest,
  'room_setup_operation_id', runs.room_setup_operation_id,
  'room_id', runs.room_id,
  'lobby_launch_committed', false,
  'abandonment_fence_digest', 'blake3:' || repeat('c', 64),
  'authentication_tag', repeat('d', 64)
)::text, 'utf8') as value
from platform_store.activity_runs runs
where runs.launch_request_id = '93000000-0000-4000-8000-000000000101';

grant select on exact_evidence to service_role;

set local role service_role;
select ok(
  platform_api.record_prestart_abandonment_v1(
    '93000000-0000-4000-8000-000000000101',
    (select value from exact_evidence),
    extensions.digest((select value from exact_evidence), 'sha256')
  ),
  'exact fresh Host evidence can abandon an unstarted human-only Run'
);
select ok(
  (select launches.state = 'abandoned_prestart'
    and reservations.released_at is not null
    and reservations.release_reason = 'abandoned_prestart'
    and runs.activity_run_id is not null
    and not exists (
      select 1 from platform_store.indexed_activity_results results
      where results.activity_run_id = runs.activity_run_id
    )
   from platform_store.launch_requests launches
   join platform_store.activity_runs runs using (launch_request_id)
   join platform_store.capacity_reservations reservations using (launch_request_id)
   where launches.launch_request_id = '93000000-0000-4000-8000-000000000101'
     and reservations.kind = 'active_run'),
  'abandonment retains Run history without a result and releases only active capacity'
);
select is(
  (select count(*)::integer
   from platform_api.list_reconciliation_candidates_v1(100) candidates
   where candidates.launch_request_id = '93000000-0000-4000-8000-000000000101'),
  0,
  'an abandoned pre-start Run is not polled forever as a result source'
);
select ok(
  platform_api.record_prestart_abandonment_v1(
    '93000000-0000-4000-8000-000000000101',
    (select value from exact_evidence),
    extensions.digest((select value from exact_evidence), 'sha256')
  ),
  'the exact evidence retry is idempotent after an interrupted caller restart'
);
select is(
  (select count(*)::integer
   from platform_store.prestart_abandonments abandonments
   where abandonments.launch_request_id = '93000000-0000-4000-8000-000000000101'),
  1,
  'an idempotent retry does not create another receipt for the exact Launch Request'
);

select pg_temp.insert_prestart_run(
  '93000000-0000-4000-8000-000000000102',
  '93000000-0000-4000-8000-000000000202',
  '93000000-0000-4000-8000-000000000002'
);
create temporary table ambiguous_evidence as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/hosted-prestart-abandonment-evidence/v1',
  'host_installation_id', runs.host_installation_id,
  'launch_request_id', runs.launch_request_id,
  'listing_revision_digest', runs.listing_revision_digest,
  'launch_request_digest', runs.launch_request_digest,
  'room_setup_operation_id', runs.room_setup_operation_id,
  'room_id', '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'lobby_launch_committed', false,
  'abandonment_fence_digest', 'blake3:' || repeat('c', 64),
  'authentication_tag', repeat('d', 64)
)::text, 'utf8') as value
from platform_store.activity_runs runs
where runs.launch_request_id = '93000000-0000-4000-8000-000000000102';
select ok(
  not platform_api.record_prestart_abandonment_v1(
    '93000000-0000-4000-8000-000000000102',
    (select value from ambiguous_evidence),
    extensions.digest((select value from ambiguous_evidence), 'sha256')
  ),
  'ambiguous or wrong-Room evidence cannot release a Run'
);
select ok(
  (select reservations.released_at is null
   from platform_store.capacity_reservations reservations
   where reservations.launch_request_id = '93000000-0000-4000-8000-000000000102'
     and reservations.kind = 'active_run'),
  'rejected evidence leaves capacity unchanged'
);

select pg_temp.insert_prestart_run(
  '93000000-0000-4000-8000-000000000103',
  '93000000-0000-4000-8000-000000000203',
  '93000000-0000-4000-8000-000000000003',
  clock_timestamp() - interval '61 seconds'
);
select ok(
  exists (
    select 1 from platform_api.list_prestart_abandonment_candidates_v1(10) candidates
    where candidates.launch_request_id = '93000000-0000-4000-8000-000000000103'
  ),
  'database time selects a reviewed-deadline candidate without a JavaScript elapsed-time guess'
);

-- House cleanup is not coupled to the active-Run reservation. The Host's
-- signed child-stop receipt must release exactly its one retained unit after
-- abandonment evidence has been made immutable.
insert into platform_store.house_agent_revisions (
  house_agent_revision_digest, house_agent_key, display_name, canonical_document,
  model_slug, provider_slug, behavior_policy_id, behavior_policy_revision,
  agent_profile_id, agent_profile_revision, runner_template_id,
  runner_template_revision, accounting_tokenizer_id, accounting_tokenizer_revision,
  execution_allowance, tool_set
) values (
  'blake3:' || repeat('d', 64), 'worldstream.test.prestart-house', 'Pre-start House',
  convert_to('{}', 'utf8'), 'openai/test', 'openrouter', 'worldstream/test', 'v1',
  'worldstream/test-profile', 'v1', 'worldstream/test-runner', 'v1',
  'worldstream/test-tokenizer', 'v1',
  '{"call_timeout_seconds":60,"concurrent_calls":1,"input_tokens_per_call":12000,"model_call_attempts":10,"output_tokens_per_call":1000,"total_input_tokens":120000,"total_output_tokens":10000}'::jsonb,
  '[]'::jsonb
);
select pg_temp.insert_prestart_run(
  '93000000-0000-4000-8000-000000000104',
  '93000000-0000-4000-8000-000000000204',
  '93000000-0000-4000-8000-000000000001'
);
insert into platform_store.house_fill_operations (
  house_fill_operation_id, launch_request_id, listing_revision_digest, state,
  claim_window_opened_at, claim_window_closes_at, host_installation_id,
  selection_retained_at, candidate_set_digest, exclusion_set_digest,
  selected_assignment_count, completed_at
) with sampled as (select clock_timestamp() as value)
select
  '93000000-0000-4000-8000-000000000301',
  '93000000-0000-4000-8000-000000000104',
  'blake3:' || repeat('3', 64), 'assignments_complete',
  sampled.value - interval '31 seconds', sampled.value - interval '1 second',
  'test-host-4', sampled.value, decode(repeat('1', 64), 'hex'),
  decode(repeat('2', 64), 'hex'), 1, sampled.value
from sampled;
insert into platform_store.house_runner_reservations (
  reservation_operation_id, house_fill_operation_id, launch_request_id, seat_id,
  house_agent_revision_digest, state, runner_unit_id, reservation_receipt,
  reservation_receipt_digest
) values (
  '93000000-0000-4000-8000-000000000401',
  '93000000-0000-4000-8000-000000000301',
  '93000000-0000-4000-8000-000000000104', 'house',
  'blake3:' || repeat('d', 64), 'succeeded', 'house-unit-104',
  convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256')
);
insert into platform_store.house_agent_assignments (
  house_agent_assignment_id, launch_request_id, house_fill_operation_id,
  listing_revision_digest, seat_id, setup_principal_reference,
  house_agent_revision_digest, model_slug, provider_slug, behavior_policy_id,
  behavior_policy_revision, agent_profile_id, agent_profile_revision,
  runner_template_id, runner_template_revision, accounting_tokenizer_id,
  accounting_tokenizer_revision, execution_allowance, selection_evidence_digest,
  reservation_operation_id, reservation_receipt_digest
) values (
  '93000000-0000-4000-8000-000000000501',
  '93000000-0000-4000-8000-000000000104',
  '93000000-0000-4000-8000-000000000301', 'blake3:' || repeat('3', 64),
  'house', 'house:104', 'blake3:' || repeat('d', 64), 'openai/test', 'openrouter',
  'worldstream/test', 'v1', 'worldstream/test-profile', 'v1',
  'worldstream/test-runner', 'v1', 'worldstream/test-tokenizer', 'v1',
  '{"call_timeout_seconds":60,"concurrent_calls":1,"input_tokens_per_call":12000,"model_call_attempts":10,"output_tokens_per_call":1000,"total_input_tokens":120000,"total_output_tokens":10000}'::jsonb,
  decode(repeat('3', 64), 'hex'),
  '93000000-0000-4000-8000-000000000401',
  extensions.digest(convert_to('{}', 'utf8'), 'sha256')
);
create temporary table house_abandonment_evidence as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/hosted-prestart-abandonment-evidence/v1',
  'host_installation_id', runs.host_installation_id,
  'launch_request_id', runs.launch_request_id,
  'listing_revision_digest', runs.listing_revision_digest,
  'launch_request_digest', runs.launch_request_digest,
  'room_setup_operation_id', runs.room_setup_operation_id,
  'room_id', runs.room_id,
  'lobby_launch_committed', false,
  'abandonment_fence_digest', 'blake3:' || repeat('c', 64),
  'authentication_tag', repeat('d', 64)
)::text, 'utf8') as value
from platform_store.activity_runs runs
where runs.launch_request_id = '93000000-0000-4000-8000-000000000104';
select ok(
  platform_api.record_prestart_abandonment_v1(
    '93000000-0000-4000-8000-000000000104',
    (select value from house_abandonment_evidence),
    extensions.digest((select value from house_abandonment_evidence), 'sha256')
  ),
  'exact Host evidence can abandon a Run with a retained House assignment'
);
create temporary table house_retirement_receipt as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/house-runner-retirement-receipt/v1',
  'host_installation_id', runs.host_installation_id,
  'reservation_operation_id', reservations.reservation_operation_id,
  'launch_request_id', runs.launch_request_id,
  'house_agent_assignment_id', assignments.house_agent_assignment_id,
  'runner_unit_id', reservations.runner_unit_id,
  'disposition', 'pre_start_abandoned',
  'platform_evidence_digest', 'sha256:' || encode(abandoned.abandonment_evidence_digest, 'hex'),
  'stop_witness', 'blake3:' || repeat('e', 64),
  'authentication_tag', repeat('f', 64)
)::text, 'utf8') as value
from platform_store.activity_runs runs
join platform_store.prestart_abandonments abandoned
  on abandoned.activity_run_id = runs.activity_run_id
join platform_store.house_agent_assignments assignments
  on assignments.launch_request_id = runs.launch_request_id
join platform_store.house_runner_reservations reservations
  on reservations.reservation_operation_id = assignments.reservation_operation_id
where runs.activity_run_id = '93000000-0000-4000-8000-000000000204';
select ok(
  platform_api.record_prestart_house_runner_retirement_v1(
    '93000000-0000-4000-8000-000000000204',
    '93000000-0000-4000-8000-000000000501',
    (select value from house_retirement_receipt),
    extensions.digest((select value from house_retirement_receipt), 'sha256')
  ),
  'the exact pre-start House receipt releases its one stopped unit'
);
select ok(
  (select reservations.state = 'released'
    and assignments.execution_allowance = '{"call_timeout_seconds":60,"concurrent_calls":1,"input_tokens_per_call":12000,"model_call_attempts":10,"output_tokens_per_call":1000,"total_input_tokens":120000,"total_output_tokens":10000}'::jsonb
   from platform_store.house_runner_reservations reservations
   join platform_store.house_agent_assignments assignments
     on assignments.reservation_operation_id = reservations.reservation_operation_id
   where reservations.reservation_operation_id = '93000000-0000-4000-8000-000000000401'),
  'House retirement preserves the retained allowance and releases no unrelated unit'
);

create temporary table started_evidence as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/hosted-prestart-abandonment-evidence/v1',
  'host_installation_id', runs.host_installation_id,
  'launch_request_id', runs.launch_request_id,
  'listing_revision_digest', runs.listing_revision_digest,
  'launch_request_digest', runs.launch_request_digest,
  'room_setup_operation_id', runs.room_setup_operation_id,
  'room_id', runs.room_id,
  'lobby_launch_committed', true,
  'abandonment_fence_digest', 'blake3:' || repeat('c', 64),
  'authentication_tag', repeat('d', 64)
)::text, 'utf8') as value
from platform_store.activity_runs runs
where runs.launch_request_id = '93000000-0000-4000-8000-000000000103';
select throws_ok(
  format(
    $query$select platform_api.record_prestart_abandonment_v1(%L, decode(%L, 'hex'), decode(%L, 'hex'))$query$,
    '93000000-0000-4000-8000-000000000103',
    encode((select value from started_evidence), 'hex'),
    encode(extensions.digest((select value from started_evidence), 'sha256'), 'hex')
  ),
  '22023', 'invalid_prestart_abandonment',
  'evidence that reports an actually-started Lobby is never an abandonment proof'
);

select * from finish();
rollback;
