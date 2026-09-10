begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

insert into platform_store.platform_accounts(account_id)
values
  ('70000000-0000-4000-8000-000000000001'),
  ('70000000-0000-4000-8000-000000000002');

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
  'blake3:' || repeat('7', 64), 'worldstream.test.my-games',
  convert_to('{"title":"My games test"}', 'utf8'),
  'blake3:' || repeat('8', 64), 'sha256:' || repeat('9', 64), 'test-web',
  'private', 'may_spectate', 'disabled', 'disabled', 'blake3:' || repeat('a', 64),
  'test/projection/v1', 'blake3:' || repeat('b', 64),
  'test/result/v1', 'blake3:' || repeat('c', 64), 'worldstream/canonical-json/v1',
  1024, '{}'::jsonb,
  '[{"seat_id":"observer","role":"observer","display_name":"Observer","required":false,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
  false, 1800
);

insert into platform_store.launch_requests (
  launch_request_id, creator_account_id, listing_revision_digest,
  idempotency_namespace, idempotency_key_digest, canonical_launch_input,
  launch_input_digest, canonicalizer_version, house_fill_choice,
  creator_access_choice, creator_seat_id, creator_participation_kind,
  state, created_at, last_transition_at, expires_at
)
select
  format('70000000-0000-4000-8000-%s', lpad(number::text, 12, '0'))::uuid,
  '70000000-0000-4000-8000-000000000001', 'blake3:' || repeat('7', 64),
  'my-games-test-' || number, decode(lpad(number::text, 64, '0'), 'hex'),
  convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1', 'disabled', 'spectator', null, 'account_human',
  case when number in (2, 3, 4) then 'run_created' when number = 20 then 'failed_pre_genesis' when number in (1, 18) then 'abandoned_prestart' else 'cancelled' end,
  sampled_at - (number || ' seconds')::interval,
  sampled_at - (number || ' seconds')::interval,
  sampled_at - (number || ' seconds')::interval + interval '24 hours'
from generate_series(1, 21) number
cross join (select clock_timestamp() as sampled_at) clock;

insert into platform_store.launch_requests (
  launch_request_id, creator_account_id, listing_revision_digest,
  idempotency_namespace, idempotency_key_digest, canonical_launch_input,
  launch_input_digest, canonicalizer_version, house_fill_choice,
  creator_access_choice, creator_seat_id, creator_participation_kind,
  state, created_at, last_transition_at, expires_at
) values (
  '70000000-0000-4000-8000-000000000888',
  '70000000-0000-4000-8000-000000000002', 'blake3:' || repeat('7', 64),
  'my-games-other-account', decode(repeat('f', 64), 'hex'),
  convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1', 'disabled', 'spectator', null, 'account_human',
  'cancelled', current_timestamp, current_timestamp, current_timestamp + interval '24 hours'
);

insert into platform_store.activity_runs (
  activity_run_id, launch_request_id, listing_revision_digest, creator_account_id,
  host_installation_id, room_setup_operation_id, room_id, launch_request_digest,
  pack_id, pack_version, pack_revision_digest, genesis_room_seq,
  genesis_or_transition_hash, canonical_genesis_evidence, genesis_evidence_digest,
  public_id, evidence_class, initial_reconciliation_state
) values (
  '70000000-0000-4000-8000-000000000099',
  '70000000-0000-4000-8000-000000000001',
  'blake3:' || repeat('7', 64), '70000000-0000-4000-8000-000000000001',
  'test-host', 'test-operation', '01ARZ3NDEKTSV4RRFFQ69G5FAV',
  'blake3:' || repeat('d', 64), 'worldstream.test', '1.0.0',
  'blake3:' || repeat('8', 64), 0, 'blake3:' || repeat('e', 64),
  convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  repeat('f', 32), 'unranked', 'ready'
);
insert into platform_store.activity_run_memberships (
  activity_run_id, membership_id, access_mode, purpose, seat_id, role,
  public_seat_label, principal_kind, participation_source, controlling_account_id,
  principal_id, entry_selector
) values (
  '70000000-0000-4000-8000-000000000099', '01ARZ3NDEKTSV4RRFFQ69G5FAY',
  'participant', 'participant', 'observer', 'observer', 'Observer', 'agent',
  'account_external_agent', '70000000-0000-4000-8000-000000000001',
  '01ARZ3NDEKTSV4RRFFQ69G5FAZ', repeat('a', 32)
);
-- Isolate the My-games action policy from public-result projector fixtures.
insert into platform_store.activity_runs (
  activity_run_id, launch_request_id, listing_revision_digest, creator_account_id,
  host_installation_id, room_setup_operation_id, room_id, launch_request_digest,
  pack_id, pack_version, pack_revision_digest, genesis_room_seq,
  genesis_or_transition_hash, canonical_genesis_evidence, genesis_evidence_digest,
  public_id, evidence_class, initial_reconciliation_state
)
select '70000000-0000-4000-8000-000000000098', '70000000-0000-4000-8000-000000000002',
  listing_revision_digest, creator_account_id, host_installation_id, 'private-terminal-operation',
  '01ARZ3NDEKTSV4RRFFQ69G5FB0', launch_request_digest, pack_id, pack_version,
  pack_revision_digest, genesis_room_seq, genesis_or_transition_hash,
  canonical_genesis_evidence, genesis_evidence_digest, repeat('e', 32), evidence_class, initial_reconciliation_state
from platform_store.activity_runs where activity_run_id = '70000000-0000-4000-8000-000000000099';
insert into platform_store.activity_run_memberships (
  activity_run_id, membership_id, access_mode, purpose, seat_id, role,
  public_seat_label, principal_kind, participation_source, controlling_account_id,
  principal_id, entry_selector
)
select '70000000-0000-4000-8000-000000000098', '01ARZ3NDEKTSV4RRFFQ69G5FB1',
  access_mode, purpose, seat_id, role, public_seat_label, principal_kind,
  participation_source, controlling_account_id, '01ARZ3NDEKTSV4RRFFQ69G5FB2', repeat('b', 32)
from platform_store.activity_run_memberships where activity_run_id = '70000000-0000-4000-8000-000000000099';
insert into platform_store.activity_run_terminal_evidence (
  activity_run_id, listing_revision_digest, result_projector_revision_digest,
  projector_status, result_indexer_membership_id, source_head, source_room_seq,
  source_projection_hash, integrity_status, integrity_generation,
  host_evidence_digest, canonical_terminal_evidence, terminal_evidence_digest
) values (
  '70000000-0000-4000-8000-000000000098', 'blake3:' || repeat('7', 64), 'blake3:' || repeat('a', 64),
  'summary', '01ARZ3NDEKTSV4RRFFQ69G5FB3', '{}'::jsonb, 12,
  'blake3:' || repeat('d', 64), 'healthy', 1,
  decode(repeat('d', 64), 'hex'), convert_to('{}', 'utf8'), decode(repeat('e', 64), 'hex')
);

-- A private terminal-without-outcome disposition represents an expired
-- activity. It retains the original participant correspondence for debrief
-- re-entry without inventing an indexed or public result.
insert into platform_store.activity_runs
select (jsonb_populate_record(null::platform_store.activity_runs, to_jsonb(runs) || jsonb_build_object(
  'activity_run_id', '70000000-0000-4000-8000-000000000096',
  'launch_request_id', '70000000-0000-4000-8000-000000000004',
  'room_setup_operation_id', 'private-expired-operation',
  'room_id', '01ARZ3NDEKTSV4RRFFQ69G5FB5', 'public_id', repeat('c', 32)
))).* from platform_store.activity_runs runs
where activity_run_id = '70000000-0000-4000-8000-000000000098';
insert into platform_store.activity_run_memberships
select (jsonb_populate_record(null::platform_store.activity_run_memberships, to_jsonb(memberships) || jsonb_build_object(
  'activity_run_id', '70000000-0000-4000-8000-000000000096',
  'membership_id', '01ARZ3NDEKTSV4RRFFQ69G5FB6',
  'principal_id', '01ARZ3NDEKTSV4RRFFQ69G5FB7', 'entry_selector', repeat('c', 32)
))).* from platform_store.activity_run_memberships memberships
where activity_run_id = '70000000-0000-4000-8000-000000000098';
insert into platform_store.activity_run_terminal_evidence
select (jsonb_populate_record(null::platform_store.activity_run_terminal_evidence, to_jsonb(evidence) || jsonb_build_object(
  'activity_run_id', '70000000-0000-4000-8000-000000000096',
  'projector_status', 'terminal_without_outcome', 'source_room_seq', 13
))).* from platform_store.activity_run_terminal_evidence evidence
where activity_run_id = '70000000-0000-4000-8000-000000000098';

-- The rollback encloses this replacement, so the durable public view policy is
-- never changed by the test.
create or replace function platform_store.public_run_state_v1(p_activity_run_id uuid)
returns text language sql stable security invoker set search_path = ''
as $$ select 'result'::text; $$;

set local role service_role;

create temporary table first_page as
select platform_api.list_my_games_v1(
  '70000000-0000-4000-8000-000000000001', null, null, 20
) as value;

select is(
  (select item ->> 'state' from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000002'),
  'terminal_private', 'private terminal evidence does not wait for result publication'
);
select is(
  (select item ->> 'action' from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000002'),
  'return_to_game', 'the original Participant can return to the private debrief'
);
select ok(
  (select not (item ? 'result_public_id') from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000002'),
  'private terminal history has no public result link'
);
select is(
  (select item ->> 'state' from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000004'),
  'terminal_private', 'private expiry is retained as terminal private history'
);
select is(
  (select item ->> 'action' from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000004'),
  'return_to_game', 'the original Participant can return to a private expiry debrief'
);
select ok(
  (select not (item ? 'result_public_id') from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000004'),
  'private expiry history has no public result link'
);

select is(
  jsonb_array_length((select value -> 'items' from first_page)), 20,
  'the first bounded page contains the requested number of retained activities'
);
select ok(
  (select value -> 'next' is not null from first_page),
  'an extra retained activity supplies a cursor'
);
select is(
  (select item ->> 'action'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000001'),
  'none',
  'a post-Genesis abandoned setup retains no fabricated result or re-entry'
);
select is(
  (select item ->> 'state'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000001'),
  'setup_abandoned',
  'a post-Genesis abandoned setup is not misreported as live or publication pending'
);
select is(
  (select item ->> 'action'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000020'),
  'none',
  'failed pre-Genesis history is retained without a fabricated entry action'
);
select is(
  (select item ->> 'state'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000020'),
  'setup_failed',
  'failed pre-Genesis history is classified without a fabricated Room result'
);
select ok(
  (select item ->> 'launch_id' = '70000000-0000-4000-8000-000000000019'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000019'),
  'cancelled history is retained in the owner index'
);
select is(
  (select item ->> 'state'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000019'),
  'setup_cancelled',
  'cancelled setup history is distinct from a terminal Run'
);
select is(
  (select item ->> 'state'
   from first_page, jsonb_array_elements(value -> 'items') item
   where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000018'),
  'setup_abandoned',
  'abandoned setup history is distinct from a live or pending result'
);
select is(
  platform_api.list_my_games_v1(
    '70000000-0000-4000-8000-000000000002', null, null, 20
  ) -> 'items' -> 0 ->> 'launch_id',
  '70000000-0000-4000-8000-000000000888',
  'the second account receives only its own retained activity'
);
select ok(
  not ((platform_api.list_my_games_v1(
    '70000000-0000-4000-8000-000000000001', null, null, 50
  ) -> 'items') @> jsonb_build_array(jsonb_build_object(
    'launch_id', '70000000-0000-4000-8000-000000000888'
  ))),
  'the first account cannot discover the second account activity'
);

create temporary table second_page as
select platform_api.list_my_games_v1(
  '70000000-0000-4000-8000-000000000001',
  (select value #>> '{next,before_at}' from first_page)::timestamptz,
  (select value #>> '{next,before_launch_id}' from first_page)::uuid,
  20
) as value;

select is(
  jsonb_array_length((select value -> 'items' from second_page)), 1,
  'the strict cursor returns the first excluded item rather than skipping it'
);
create temporary table traversed_pages as
select (value -> 'items' -> index) ->> 'launch_id' as launch_id
from first_page cross join generate_series(0, 19) index
union all
select (value -> 'items' -> 0) ->> 'launch_id' from second_page;

select is(
  (select count(distinct launch_id)::integer from traversed_pages),
  21,
  'two pages contain 21 distinct retained activity IDs'
);
select is(
  (select array_agg(launch_id order by launch_id) from traversed_pages),
  (select array_agg(
    format('70000000-0000-4000-8000-%s', lpad(number::text, 12, '0'))
    order by number
  ) from generate_series(1, 21) number),
  'two pages traverse the exact expected launch-ID range without a gap'
);

reset role;
insert into platform_store.activity_runs
select (jsonb_populate_record(null::platform_store.activity_runs, to_jsonb(runs) || jsonb_build_object(
  'activity_run_id', '70000000-0000-4000-8000-000000000097',
  'launch_request_id', '70000000-0000-4000-8000-000000000003',
  'room_setup_operation_id', 'private-unhealthy-operation',
  'room_id', '01ARZ3NDEKTSV4RRFFQ69G5FB4', 'public_id', repeat('d', 32)
))).* from platform_store.activity_runs runs
where activity_run_id = '70000000-0000-4000-8000-000000000098';
insert into platform_store.activity_run_terminal_evidence
select (jsonb_populate_record(null::platform_store.activity_run_terminal_evidence, to_jsonb(evidence) || jsonb_build_object(
  'activity_run_id', '70000000-0000-4000-8000-000000000097', 'integrity_status', 'faulted'
))).* from platform_store.activity_run_terminal_evidence evidence
where activity_run_id = '70000000-0000-4000-8000-000000000098';
insert into platform_store.reconciliation_receipts (
  receipt_kind, source_key, activity_run_id, evidence_digest, disposition, safe_code
) values ('terminal', 'private-terminal-conflict-test', '70000000-0000-4000-8000-000000000098',
  decode(repeat('c', 64), 'hex'), 'conflict', 'terminal_conflict');

set local role service_role;
create temporary table unavailable_page as
select platform_api.list_my_games_v1('70000000-0000-4000-8000-000000000001', null, null, 20) as value;
select is((select item ->> 'state' from unavailable_page, jsonb_array_elements(value -> 'items') item
  where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000003'),
  'dependency_failure', 'unhealthy private terminal evidence is not publication pending');
select is((select item ->> 'state' from unavailable_page, jsonb_array_elements(value -> 'items') item
  where item ->> 'launch_id' = '70000000-0000-4000-8000-000000000002'),
  'dependency_failure', 'conflicted private terminal evidence is not publication pending');
select ok((select bool_and(item ->> 'action' = 'none' and not (item ? 'result_public_id'))
  from unavailable_page, jsonb_array_elements(value -> 'items') item
  where item ->> 'launch_id' in ('70000000-0000-4000-8000-000000000002', '70000000-0000-4000-8000-000000000003')),
  'unverified private histories expose neither a result link nor an entry action');

select * from finish();
rollback;
