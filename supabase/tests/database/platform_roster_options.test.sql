begin;
create extension if not exists pgtap with schema extensions;
select no_plan();
create temporary table fixture_source as select $fixture${"schema": "worldstream/activity-listing-revision/v1", "listing_id": "worldstream.roster-contract.fixture", "version": "0.1.0", "title": "Reviewed roster contract fixture", "description": "Identify the authentic ledger, gain access to the vault, and escape through the Atrium. Solo Standard expedition: 16 turns and 3 power charges; reading costs no turns.", "catalog": {"visibility": "private", "review_status": "reviewed"}, "pack": {"id": "worldstream.midnight-archive", "version": "0.1.0", "digest": "blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64"}, "client": {"client_id": "worldstream.midnight-archive.web", "release_digest": "sha256:9be6d6548f2d62baba805ec461b27fbd1021ed0d61fbd0e8ffd1d779616bc534", "client_contract": "worldstream/activity-client-protocol/v1", "surface_id": "midnight-archive-hosted-web"}, "creator_access": "must_claim_seat", "launch_input_schema": {"schema": "worldstream/launch-input-schema/v2", "accepts": "roster_option", "defaults": {"roster_option": "solo"}, "roster_options": [{"option_id": "solo", "label": "Solo", "seat_ids": ["lead"], "configuration": {"scenario_id": "standard-v1"}, "house_agent_assignments": []}, {"option_id": "first", "label": "One supplied agent", "seat_ids": ["lead", "mira"], "configuration": {"scenario_id": "standard-v1"}, "house_agent_assignments": [{"seat_id": "mira", "house_agent_revision_digest": "blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81"}]}, {"option_id": "second", "label": "Other supplied agent", "seat_ids": ["lead", "jonah"], "configuration": {"scenario_id": "standard-v1"}, "house_agent_assignments": [{"seat_id": "jonah", "house_agent_revision_digest": "blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"}]}, {"option_id": "both", "label": "Two supplied agents", "seat_ids": ["lead", "mira", "jonah"], "configuration": {"scenario_id": "standard-v1"}, "house_agent_assignments": [{"seat_id": "mira", "house_agent_revision_digest": "blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81"}, {"seat_id": "jonah", "house_agent_revision_digest": "blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"}]}]}, "room_setup": {"configuration": {"scenario_id": "standard-v1"}}, "seats": [{"seat_id": "lead", "role": "lead", "display_name": "Expedition lead", "required": true, "allowed_participation": ["account_human"], "allowed_house_agent_revisions": []}, {"seat_id": "mira", "role": "mira", "display_name": "Mira", "required": false, "allowed_participation": ["house_agent_fill"], "allowed_house_agent_revisions": ["blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81"]}, {"seat_id": "jonah", "role": "jonah", "display_name": "Jonah", "required": false, "allowed_participation": ["house_agent_fill"], "allowed_house_agent_revisions": ["blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"]}], "pre_start_deadline_seconds": 1800, "public_viewing_policy": "disabled", "result": {"projection": {"schema": "worldstream.midnight-archive/public-projection/v3", "digest": "blake3:d6042c76a9f09bc4bc1ea3fac8c443bc6f7f154f39aed4cd7aad5b60b956949f"}, "projector": {"id": "worldstream.midnight-archive.result", "version": "0.1.0", "digest": "blake3:099bb9dc1b0e1fa76ebfe3d67d8b1c0e239e53ec72c88b65e0892bf79f268821"}, "publication": {"policy": "disabled", "attribution": "none", "public_output": "none", "suppression": "unhealthy_inconclusive_or_conflict"}}}$fixture$::jsonb as value;
insert into platform_store.activity_listing_revisions
select (jsonb_populate_record(null::platform_store.activity_listing_revisions,
  to_jsonb(existing) || jsonb_build_object(
    'listing_revision_digest', 'blake3:' || repeat('d8',32),
    'listing_key', 'worldstream.roster-coordination.fixture',
    'canonical_document', '\x' || encode(convert_to(source.value::text,'utf8'),'hex'),
    'seat_templates', source.value -> 'seats',
    'room_setup_configuration', source.value #> '{room_setup,configuration}'
  ))).*
from platform_store.activity_listing_revisions existing, fixture_source source
where existing.listing_key = 'worldstream.midnight-archive.internal-solo' limit 1;
insert into platform_store.platform_accounts(account_id) values
 ('d8000000-0000-4000-8000-000000000001'), ('d8000000-0000-4000-8000-000000000002'),
 ('d8000000-0000-4000-8000-000000000003');
create function pg_temp.launch(option_id text, account_suffix integer default 1, key text default 'request', fill text default 'disabled')
returns uuid language sql as $$
 select launch_request_id from platform_api.create_launch_request_v1(
   ('d8000000-0000-4000-8000-' || lpad(account_suffix::text,12,'0'))::uuid,
   'blake3:' || repeat('d8',32), 'roster-option-test', extensions.digest(key,'sha256'),
   convert_to(jsonb_build_object('roster_option', option_id)::text,'utf8'),
   extensions.digest(convert_to(jsonb_build_object('roster_option', option_id)::text,'utf8'),'sha256'),
   'worldstream/canonical-json/v1', fill, 'seat', 'lead', 'account_human');
$$;
grant select on fixture_source to service_role;
set local role service_role;
select throws_ok($$select pg_temp.launch('unknown')$$,'22023','unsupported_roster_option','unknown option rejected at database boundary');
select throws_ok($$select pg_temp.launch('first')$$,'22023','roster_fill_choice_mismatch','supplied option cannot skip reservation');
select throws_ok($$select pg_temp.launch('solo',1,'request','fill_unclaimed')$$,'22023','roster_fill_choice_mismatch','solo cannot request random fill');
create temporary table solo_launch as select pg_temp.launch('solo') as id;
select is(pg_temp.launch('solo'),(select id from solo_launch),'same request retains one launch');
select throws_ok($$select pg_temp.launch('first',1,'request','fill_unclaimed')$$,'23505','launch_idempotency_conflict','option change cannot rewrite immutable intent');
select throws_ok($$select pg_temp.launch('solo',1,'other')$$,'55000','pre_genesis_capacity_unavailable','account quota is preserved');
select is(jsonb_array_length(platform_api.read_launch_request_v1('d8000000-0000-4000-8000-000000000001',(select id from solo_launch)) -> 'seats'),1,'solo exposes exactly its selected seat');
select is(platform_api.read_launch_request_v1('d8000000-0000-4000-8000-000000000002',(select id from solo_launch)),null::jsonb,'wrong account cannot read private formation');
select throws_ok($$select platform_api.rotate_seat_invitation_v1('d8000000-0000-4000-8000-000000000001',(select id from solo_launch),'mira')$$,'22023','seat_unavailable','excluded seat cannot gain invitation');
select throws_ok($$update platform_store.launch_requests set canonical_launch_input=convert_to('{"roster_option":"both"}','utf8') where launch_request_id=(select id from solo_launch)$$,'42501','permission denied for table launch_requests','direct option mutation is rejected');
create temporary table supplied_launch as select pg_temp.launch('both',2,'supplied','fill_unclaimed') as id;
select throws_ok($$select platform_api.rotate_seat_invitation_v1('d8000000-0000-4000-8000-000000000002',(select id from supplied_launch),'mira')$$,'22023','seat_unavailable','selected supplied seat cannot become an account claim');
select is(platform_api.start_house_fill_v1('d8000000-0000-4000-8000-000000000003',(select id from supplied_launch)),null::jsonb,'wrong account cannot authorize supplied execution');
select platform_api.start_house_fill_v1('d8000000-0000-4000-8000-000000000002',(select id from supplied_launch));
reset role;
insert into platform_store.house_agent_host_approvals (
  host_installation_id,house_agent_revision_digest,agent_profile_revision_digest,runner_template_revision_digest,
  runner_executable_digest,named_credential_reference,approval_receipt_digest,available_for_new_assignments)
select 'roster-options-test', house_agent_revision_digest,'blake3:'||repeat('1',64),'blake3:'||repeat('2',64),
  'blake3:'||repeat('3',64),'openrouter-house',extensions.digest(house_agent_revision_digest,'sha256'),true
from platform_store.house_agent_revisions;
-- A transaction-only capacity window offsets retained local reservations.
alter table platform_store.house_runner_capacity_gates disable trigger protect_house_runner_capacity_gate_v1;
alter table platform_store.house_runner_capacity_gates drop constraint house_runner_gate_exact_limit;
update platform_store.house_runner_capacity_gates set hard_limit=2+(select count(*) from platform_store.house_runner_reservations where state in ('pending','ambiguous','succeeded') and released_at is null);
alter table platform_store.house_runner_capacity_gates enable trigger protect_house_runner_capacity_gate_v1;
alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations set claim_window_opened_at=claim_window_opened_at-interval '31 seconds',claim_window_closes_at=claim_window_closes_at-interval '31 seconds' where launch_request_id=(select id from supplied_launch);
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;
set local role service_role;
create temporary table selected as select platform_api.retain_house_fill_selection_v1((select id from supplied_launch),'roster-options-test') as value;
select is(platform_api.retain_house_fill_selection_v1((select id from supplied_launch),'roster-options-test'),(select value from selected),'retry retains exact selection and reservation identities');
select is((select count(*)::integer from platform_store.house_runner_reservations where launch_request_id=(select id from supplied_launch)),2,'both exact reservations are retained');
select is((select count(*)::integer from platform_store.house_agent_assignments where launch_request_id=(select id from supplied_launch)),0,'no partial assignments before all receipts');
select ok(not exists(select 1 from platform_store.house_runner_reservations reservation
 where launch_request_id=(select id from supplied_launch) and not exists(
  select 1 from fixture_source source,jsonb_array_elements(source.value #> '{launch_input_schema,roster_options,3,house_agent_assignments}') assignment
  where assignment ->> 'seat_id'=reservation.seat_id and assignment ->> 'house_agent_revision_digest'=reservation.house_agent_revision_digest)),
 'retained selection matches exact reviewed seat-to-revision mapping');
-- A crash after one successful receipt retains the exact successful reservation
-- and creates no partial Assignment set. The second terminal failure retires
-- the launch rather than selecting another revision.
do $$
declare reservation platform_store.house_runner_reservations%rowtype; receipt bytea;
begin
 select * into strict reservation from platform_store.house_runner_reservations
 where launch_request_id=(select id from supplied_launch) and seat_id='mira';
 receipt := convert_to(jsonb_build_object('outcome','succeeded','reservation_operation_id',reservation.reservation_operation_id,'runner_unit_id','roster-runner-mira')::text,'utf8');
 perform platform_api.record_house_runner_reservation_v1(reservation.reservation_operation_id,'succeeded','roster-runner-mira',receipt,extensions.digest(receipt,'sha256'),null);
end;
$$;
select is((platform_api.retain_house_fill_selection_v1((select id from supplied_launch),'roster-options-test')->>'state'),'reserving','restart resumes the same partial reservation operation');
select is((select count(*)::integer from platform_store.house_agent_assignments where launch_request_id=(select id from supplied_launch)),0,'one successful receipt does not publish a partial Assignment set');
select throws_ok($$select platform_api.complete_house_fill_v1((select id from supplied_launch))$$,'55000','house_fill_reservations_incomplete','completion cannot bypass a missing reservation');
do $$
declare reservation platform_store.house_runner_reservations%rowtype; receipt bytea;
begin
 select * into strict reservation from platform_store.house_runner_reservations
 where launch_request_id=(select id from supplied_launch) and seat_id='jonah';
 receipt := convert_to(jsonb_build_object('outcome','terminal_failed','reservation_operation_id',reservation.reservation_operation_id,'failure_code','capacity_unavailable')::text,'utf8');
 perform platform_api.record_house_runner_reservation_v1(reservation.reservation_operation_id,'terminal_failed',null,receipt,extensions.digest(receipt,'sha256'),'capacity_unavailable');
end;
$$;
select is((platform_api.retain_house_fill_selection_v1((select id from supplied_launch),'roster-options-test')->>'state'),'failed_pre_genesis','terminal reservation refusal never rerolls');
select is((select count(*)::integer from platform_store.house_agent_assignments where launch_request_id=(select id from supplied_launch)),0,'terminal pre-Genesis failure leaves no partial Assignment set');
select is((select count(*)::integer from platform_store.house_runner_reservations where launch_request_id=(select id from supplied_launch)),2,'failure preserves exactly the original two reservation identities');
create temporary table capacity_launch as select pg_temp.launch('both',3,'capacity','fill_unclaimed') as id;
select platform_api.start_house_fill_v1('d8000000-0000-4000-8000-000000000003',(select id from capacity_launch));
reset role;
alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations set claim_window_opened_at=claim_window_opened_at-interval '31 seconds',claim_window_closes_at=claim_window_closes_at-interval '31 seconds' where launch_request_id=(select id from capacity_launch);
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;
set local role service_role;
select is((platform_api.retain_house_fill_selection_v1((select id from capacity_launch),'roster-options-test')->>'failure_code'),'runner_capacity_unavailable','real retained capacity gate refuses a whole selection that cannot fit');
select is((select count(*)::integer from platform_store.house_runner_reservations where launch_request_id=(select id from capacity_launch)),0,'capacity refusal reserves no partial runner set');
select is((select count(*)::integer from platform_store.house_agent_assignments where launch_request_id=(select id from capacity_launch)),0,'capacity refusal creates no assignments');
create temporary table frozen_solo as select
 convert_to(jsonb_build_object('schema','worldstream/frozen-roster/v1','listing_revision_digest','blake3:'||repeat('d8',32),
   'members',jsonb_build_array(jsonb_build_object('seat_id','lead','participation','account_human','principal_reference','seat:lead','display_name','Expedition lead')))::text,'utf8') as roster,
 jsonb_build_object('schema','worldstream/room-setup/v2','pack',value -> 'pack','configuration',value #> '{room_setup,configuration}',
   'seats',jsonb_build_array(jsonb_build_object('label','lead','role','lead','required',true,'display_name','Expedition lead','principal',jsonb_build_object('reference','seat:lead','kind','human'))),
   'spectators',jsonb_build_array(jsonb_build_object('purpose','result_indexer','principal',jsonb_build_object('reference','worldstream:result-indexer','kind','agent'))),'operator_view',false) as setup
 from fixture_source;
create function pg_temp.freeze_solo(configuration jsonb default '{"scenario_id":"standard-v1"}',account_suffix integer default 1)
returns boolean language sql as $$
 select platform_api.freeze_launch_request_v1(
  ('d8000000-0000-4000-8000-'||lpad(account_suffix::text,12,'0'))::uuid,(select id from solo_launch),
  roster,extensions.digest(roster,'sha256'),convert_to((setup||jsonb_build_object('configuration',configuration))::text,'utf8'),
  'blake3:'||repeat('f',64),'roster-options-test','roster-options-solo') from frozen_solo;
$$;
select throws_ok($$select pg_temp.freeze_solo('{"scenario_id":"unreviewed"}')$$,'22023','room_setup_roster_option_mismatch','setup freeze independently rejects changed server configuration');
select is(pg_temp.freeze_solo('{"scenario_id":"standard-v1"}',2),false,'wrong account cannot freeze the selected setup');
select is(pg_temp.freeze_solo(),true,'the selected solo option freezes one complete setup');
select is(pg_temp.freeze_solo(),true,'identical setup retry resumes the same operation');
select throws_ok($$select pg_temp.freeze_solo('{"scenario_id":"unreviewed"}')$$,'23505','launch_freeze_conflict','frozen setup cannot be rewritten after restart');
select is((select room_setup_operation_id from platform_store.launch_requests where launch_request_id=(select id from solo_launch)),'roster-options-solo','one operation identity remains retained');
select * from finish();
rollback;
