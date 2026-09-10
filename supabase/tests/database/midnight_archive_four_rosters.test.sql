begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

create temporary table archive_identity (
  listing_digest text primary key,
  mira_digest text not null,
  jonah_digest text not null
);
insert into archive_identity values (
  'blake3:cc1c92ebc6ba7cccc9474186ff8107cf97f6bd0ce2676c6d1a2aa203c2a62d35',
  'blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde',
  'blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0'
);

select is(
  (select convert_from(canonical_document, 'utf8')::jsonb #>> '{version}'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = (select listing_digest from archive_identity)),
  '0.3.0',
  'the exact four-roster Archive Listing is registered'
);
select is(
  (select convert_from(canonical_document, 'utf8')::jsonb #>> '{launch_input_schema,schema}'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = (select listing_digest from archive_identity)),
  'worldstream/launch-input-schema/v3',
  'the registered Archive Listing uses the described closed roster schema'
);
select is(
  (select jsonb_agg(jsonb_build_object(
      'id', option ->> 'option_id',
      'seats', option -> 'seat_ids',
      'house', option -> 'house_agent_assignments',
      'configuration', option -> 'configuration'
    ) order by ordinality)
   from platform_store.activity_listing_revisions listings
   cross join lateral jsonb_array_elements(
     convert_from(listings.canonical_document, 'utf8')::jsonb #> '{launch_input_schema,roster_options}'
   ) with ordinality options(option, ordinality)
   where listings.listing_revision_digest = (select listing_digest from archive_identity)),
  jsonb_build_array(
    jsonb_build_object('id', 'solo', 'seats', jsonb_build_array('lead'), 'house', '[]'::jsonb,
      'configuration', jsonb_build_object('scenario_id', 'standard-v1')),
    jsonb_build_object('id', 'mira', 'seats', jsonb_build_array('lead', 'mira'), 'house', jsonb_build_array(
      jsonb_build_object('seat_id', 'mira', 'house_agent_revision_digest', (select mira_digest from archive_identity))),
      'configuration', jsonb_build_object('scenario_id', 'standard-v1')),
    jsonb_build_object('id', 'jonah', 'seats', jsonb_build_array('lead', 'jonah'), 'house', jsonb_build_array(
      jsonb_build_object('seat_id', 'jonah', 'house_agent_revision_digest', (select jonah_digest from archive_identity))),
      'configuration', jsonb_build_object('scenario_id', 'standard-v1')),
    jsonb_build_object('id', 'full-crew', 'seats', jsonb_build_array('lead', 'mira', 'jonah'), 'house', jsonb_build_array(
      jsonb_build_object('seat_id', 'mira', 'house_agent_revision_digest', (select mira_digest from archive_identity)),
      jsonb_build_object('seat_id', 'jonah', 'house_agent_revision_digest', (select jonah_digest from archive_identity))),
      'configuration', jsonb_build_object('scenario_id', 'standard-v1'))
  ),
  'the registered Listing retains the four exact reviewed choices'
);
select is(
  (select count(*)::integer from platform_store.house_agent_revisions
   where house_agent_revision_digest in (
     (select mira_digest from archive_identity),
     (select jonah_digest from archive_identity)
   ) and execution_allowance = '{
     "call_timeout_seconds":60,
     "concurrent_calls":1,
     "input_tokens_per_call":12000,
     "model_call_attempts":10,
     "output_tokens_per_call":1000,
     "total_input_tokens":120000,
     "total_output_tokens":10000
   }'::jsonb),
  2,
  'Mira and Jonah are registered with their exact bounded allowances'
);

insert into platform_store.platform_accounts(account_id) values
  ('d2090000-0000-4000-8000-000000000001'),
  ('d2090000-0000-4000-8000-000000000002'),
  ('d2090000-0000-4000-8000-000000000003'),
  ('d2090000-0000-4000-8000-000000000004'),
  ('d2090000-0000-4000-8000-000000000005'),
  ('d2090000-0000-4000-8000-000000000006'),
  ('d2090000-0000-4000-8000-000000000007');

create function pg_temp.archive_launch(
  option_id text,
  account_suffix integer,
  request_key text default null,
  fill_choice text default null
)
returns uuid
language plpgsql
as $$
declare
  canonical_input bytea := convert_to(jsonb_build_object('roster_option', option_id)::text, 'utf8');
  result uuid;
begin
  select launch_request_id into strict result
  from platform_api.create_launch_request_v1(
    ('d2090000-0000-4000-8000-' || lpad(account_suffix::text, 12, '0'))::uuid,
    (select listing_digest from archive_identity),
    'archive-four-rosters',
    extensions.digest(coalesce(request_key, option_id), 'sha256'),
    canonical_input,
    extensions.digest(canonical_input, 'sha256'),
    'worldstream/canonical-json/v1',
    coalesce(fill_choice, case when option_id = 'solo' then 'disabled' else 'fill_unclaimed' end),
    'seat',
    'lead',
    'account_human'
  );
  return result;
end;
$$;

create temporary table archive_launches (
  option_id text primary key,
  account_id uuid not null,
  launch_request_id uuid not null unique,
  expected_seats text[] not null,
  expected_house_seats text[] not null
);
grant select on archive_identity to service_role;
grant select, insert on archive_launches to service_role;

set local role service_role;
insert into archive_launches values
  ('solo', 'd2090000-0000-4000-8000-000000000001', pg_temp.archive_launch('solo', 1), array['lead'], array[]::text[]),
  ('mira', 'd2090000-0000-4000-8000-000000000002', pg_temp.archive_launch('mira', 2), array['lead', 'mira'], array['mira']),
  ('jonah', 'd2090000-0000-4000-8000-000000000003', pg_temp.archive_launch('jonah', 3), array['lead', 'jonah'], array['jonah']),
  ('full-crew', 'd2090000-0000-4000-8000-000000000004', pg_temp.archive_launch('full-crew', 4), array['lead', 'mira', 'jonah'], array['mira', 'jonah']);

select ok(
  (select bool_and(pg_temp.archive_launch(option_id, right(account_id::text, 1)::integer) = launch_request_id)
   from archive_launches),
  'repeating each exact request returns its retained Launch Request'
);
select throws_ok(
  $$select pg_temp.archive_launch('mira', 1, 'solo')$$,
  '23505',
  'launch_idempotency_conflict',
  'an existing idempotency key cannot change the selected roster'
);
select throws_ok(
  $$select pg_temp.archive_launch('solo', 6, 'solo-random-fill', 'fill_unclaimed')$$,
  '22023',
  'house_fill_not_allowed',
  'the public create RPC rejects a supplied Runner request for the v3 solo choice before insert'
);
select throws_ok(
  $$select pg_temp.archive_launch('mira', 7, 'mira-without-fill', 'disabled')$$,
  '22023',
  'roster_fill_choice_mismatch',
  'the v3 insert guard rejects a supplied choice that silently omits its exact Runner'
);
select ok(
  (select bool_and(
    (select array_agg(seat ->> 'seat_id' order by ordinality)
     from jsonb_array_elements(platform_api.read_launch_request_v1(
       launches.account_id, launches.launch_request_id
     ) -> 'seats') with ordinality seats(seat, ordinality)) = launches.expected_seats
  ) from archive_launches launches),
  'every waiting-room read exposes only the seats selected by its option'
);
select is(
  (select count(*)::integer
   from platform_store.seat_claims claims
   join archive_launches launches using (launch_request_id)
   where claims.seat_id <> 'lead' or claims.released_at is not null),
  0,
  'absent optional Roles never acquire a claim'
);
select is(
  (select count(*)::integer from platform_store.house_agent_assignments assignments
   join archive_launches launches using (launch_request_id)),
  0,
  'no House Assignment exists before an exact complete reservation set'
);

select platform_api.start_house_fill_v1(account_id, launch_request_id)
from archive_launches
where option_id <> 'solo'
order by option_id;

reset role;
insert into platform_store.house_agent_host_approvals (
  host_installation_id,
  house_agent_revision_digest,
  agent_profile_revision_digest,
  runner_template_revision_digest,
  runner_executable_digest,
  named_credential_reference,
  approval_receipt_digest,
  available_for_new_assignments
)
select
  'archive-four-rosters-test',
  revisions.house_agent_revision_digest,
  'blake3:' || repeat('1', 64),
  'blake3:' || repeat('2', 64),
  'blake3:' || repeat('3', 64),
  'hosted-openrouter',
  extensions.digest(convert_to('archive-four:' || revisions.house_agent_revision_digest, 'utf8'), 'sha256'),
  true
from platform_store.house_agent_revisions revisions
where revisions.house_agent_revision_digest in (
  (select mira_digest from archive_identity),
  (select jonah_digest from archive_identity)
);

create temporary table archive_runner_capacity_baseline as
select count(*)::integer as active_count
from platform_store.house_runner_reservations
where state in ('pending', 'ambiguous', 'succeeded') and released_at is null;
grant select on archive_runner_capacity_baseline to service_role;
alter table platform_store.house_runner_capacity_gates disable trigger protect_house_runner_capacity_gate_v1;
alter table platform_store.house_runner_capacity_gates drop constraint house_runner_gate_exact_limit;
update platform_store.house_runner_capacity_gates
set hard_limit = (select active_count + 4 from archive_runner_capacity_baseline)
where gate_kind = 'runner_unit';
alter table platform_store.house_runner_capacity_gates enable trigger protect_house_runner_capacity_gate_v1;

alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations operations
set claim_window_opened_at = operations.claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = operations.claim_window_closes_at - interval '31 seconds'
from archive_launches launches
where operations.launch_request_id = launches.launch_request_id
  and launches.option_id <> 'solo';
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;

set local role service_role;
create temporary table archive_selections as
select launches.launch_request_id,
       platform_api.retain_house_fill_selection_v1(
         launches.launch_request_id,
         'archive-four-rosters-test'
       ) as value
from archive_launches launches
where launches.option_id <> 'solo'
order by launches.option_id;

select ok(
  (select bool_and(
    value ->> 'state' = 'reserving'
    and (value ->> 'selected_assignment_count')::integer = cardinality(launches.expected_house_seats)
  ) from archive_selections selections
  join archive_launches launches using (launch_request_id)),
  'each supplied choice reserves its exact reviewed number of House seats'
);
select ok(
  (select bool_and(
    platform_api.retain_house_fill_selection_v1(
      selections.launch_request_id,
      'archive-four-rosters-test'
    ) = selections.value
  ) from archive_selections selections),
  'selection retries preserve the exact reservation operation identities'
);
select ok(
  not exists (
    select 1
    from platform_store.house_runner_reservations reservations
    join archive_launches launches using (launch_request_id)
    where not (reservations.seat_id = any(launches.expected_house_seats))
  ),
  'Mira-only and Jonah-only choices never reserve the absent optional Role'
);

do $$
declare
  reservation platform_store.house_runner_reservations%rowtype;
  receipt bytea;
begin
  for reservation in
    select reservations.*
    from platform_store.house_runner_reservations reservations
    join archive_launches launches using (launch_request_id)
    order by reservations.launch_request_id, reservations.seat_id
  loop
    receipt := convert_to(jsonb_build_object(
      'outcome', 'succeeded',
      'reservation_operation_id', reservation.reservation_operation_id,
      'runner_unit_id', 'archive-' || replace(reservation.launch_request_id::text, '-', '') || '-' || reservation.seat_id
    )::text, 'utf8');
    perform platform_api.record_house_runner_reservation_v1(
      reservation.reservation_operation_id,
      'succeeded',
      'archive-' || replace(reservation.launch_request_id::text, '-', '') || '-' || reservation.seat_id,
      receipt,
      extensions.digest(receipt, 'sha256'),
      null
    );
  end loop;
end;
$$;

select platform_api.complete_house_fill_v1(launch_request_id)
from archive_launches
where option_id <> 'solo'
order by option_id;

select ok(
  not exists (
    (select launches.option_id, expected.seat_id,
       case expected.seat_id
         when 'mira' then (select mira_digest from archive_identity)
         else (select jonah_digest from archive_identity)
       end as revision_digest
     from archive_launches launches
     cross join lateral unnest(launches.expected_house_seats) expected(seat_id))
    except
    (select launches.option_id, assignments.seat_id, assignments.house_agent_revision_digest
     from platform_store.house_agent_assignments assignments
     join archive_launches launches using (launch_request_id))
  ) and not exists (
    (select launches.option_id, assignments.seat_id, assignments.house_agent_revision_digest
     from platform_store.house_agent_assignments assignments
     join archive_launches launches using (launch_request_id))
    except
    (select launches.option_id, expected.seat_id,
       case expected.seat_id
         when 'mira' then (select mira_digest from archive_identity)
         else (select jonah_digest from archive_identity)
       end
     from archive_launches launches
     cross join lateral unnest(launches.expected_house_seats) expected(seat_id))
  ),
  'completed formation publishes exactly the selected Archive Assignments'
);
select ok(
  (select bool_and(
    assignments.setup_principal_reference =
      'house:' || assignments.launch_request_id::text || ':' || assignments.seat_id
    and assignments.execution_allowance = revisions.execution_allowance
    and assignments.execution_allowance = '{
      "call_timeout_seconds":60,
      "concurrent_calls":1,
      "input_tokens_per_call":12000,
      "model_call_attempts":10,
      "output_tokens_per_call":1000,
      "total_input_tokens":120000,
      "total_output_tokens":10000
    }'::jsonb
  ) from platform_store.house_agent_assignments assignments
  join platform_store.house_agent_revisions revisions using (house_agent_revision_digest)
  join archive_launches launches using (launch_request_id)),
  'every Assignment copies its immutable allowance and uses a Launch-scoped Principal reference'
);
select is(
  (select count(distinct setup_principal_reference)::integer
   from platform_store.house_agent_assignments assignments
   join archive_launches launches using (launch_request_id)),
  4,
  'separate Launch Requests receive fresh House Principal references'
);

reset role;
create temporary table archive_frozen_documents as
with selected_seats as (
  select launches.option_id,
         launches.account_id,
         launches.launch_request_id,
         convert_from(listings.canonical_document, 'utf8')::jsonb -> 'pack' as pack,
         seats.value as seat,
         seats.ordinality
  from archive_launches launches
  join platform_store.launch_requests requests using (launch_request_id)
  join platform_store.activity_listing_revisions listings
    on listings.listing_revision_digest = requests.listing_revision_digest
  cross join lateral jsonb_array_elements(
    platform_store.selected_listing_seats_v2(listings, requests.canonical_launch_input)
  ) with ordinality seats(value, ordinality)
), members as (
  select selected.option_id,
         selected.account_id,
         selected.launch_request_id,
         selected.pack,
         selected.ordinality,
         case when claims.claim_id is not null then jsonb_build_object(
           'seat_id', claims.seat_id,
           'display_name', selected.seat ->> 'display_name',
           'participation', claims.participation_kind,
           'principal_reference', claims.principal_reference
         ) else jsonb_build_object(
           'seat_id', assignments.seat_id,
           'display_name', revisions.display_name,
           'participation', 'house_agent_fill',
           'principal_reference', assignments.setup_principal_reference,
           'house_agent_revision_digest', assignments.house_agent_revision_digest,
           'agent_profile', jsonb_build_object(
             'profile_id', assignments.agent_profile_id,
             'revision', assignments.agent_profile_revision
           ),
           'runner_template', jsonb_build_object(
             'template_id', assignments.runner_template_id,
             'revision', assignments.runner_template_revision
           )
         ) end as roster_member,
         case when claims.claim_id is not null then jsonb_build_object(
           'label', claims.seat_id,
           'role', selected.seat ->> 'role',
           'required', (selected.seat ->> 'required')::boolean,
           'display_name', selected.seat ->> 'display_name',
           'principal', jsonb_build_object(
             'reference', claims.principal_reference,
             'kind', case claims.participation_kind
               when 'account_human' then 'human' else 'agent' end
           )
         ) else jsonb_build_object(
           'label', assignments.seat_id,
           'role', selected.seat ->> 'role',
           'required', (selected.seat ->> 'required')::boolean,
           'display_name', revisions.display_name,
           'principal', jsonb_build_object(
             'reference', assignments.setup_principal_reference,
             'kind', 'agent'
           ),
           'assignment', jsonb_build_object(
             'mode', 'managed',
             'agent_profile', jsonb_build_object(
               'profile_id', assignments.agent_profile_id,
               'revision', assignments.agent_profile_revision
             ),
             'runner_template', jsonb_build_object(
               'template_id', assignments.runner_template_id,
               'revision', assignments.runner_template_revision
             )
           )
         ) end as setup_seat
  from selected_seats selected
  left join platform_store.seat_claims claims
    on claims.launch_request_id = selected.launch_request_id
    and claims.seat_id = selected.seat ->> 'seat_id'
    and claims.released_at is null
  left join platform_store.house_agent_assignments assignments
    on assignments.launch_request_id = selected.launch_request_id
    and assignments.seat_id = selected.seat ->> 'seat_id'
  left join platform_store.house_agent_revisions revisions
    on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
), documents as (
  select option_id,
         account_id,
         launch_request_id,
         jsonb_build_object(
           'schema', 'worldstream/frozen-roster/v1',
           'listing_revision_digest', (select listing_digest from archive_identity),
           'members', jsonb_agg(roster_member order by ordinality)
         ) as roster,
         jsonb_build_object(
           'schema', 'worldstream/room-setup/v2',
           'pack', pack,
           'configuration', jsonb_build_object('scenario_id', 'standard-v1'),
           'seats', jsonb_agg(setup_seat order by ordinality),
           'spectators', jsonb_build_array(jsonb_build_object(
             'purpose', 'result_indexer',
             'principal', jsonb_build_object(
               'reference', 'worldstream:result-indexer',
               'kind', 'agent'
             )
           )),
           'operator_view', false
         ) as setup
  from members
  group by option_id, account_id, launch_request_id, pack
)
select option_id,
       account_id,
       launch_request_id,
       convert_to(roster::text, 'utf8') as roster,
       convert_to(setup::text, 'utf8') as setup
from documents;
create function pg_temp.freeze_archive_configuration(p_option_id text, p_configuration jsonb)
returns boolean
language sql
as $$
  select platform_api.freeze_launch_request_v1(
    account_id,
    launch_request_id,
    roster,
    extensions.digest(roster, 'sha256'),
    convert_to((
      convert_from(setup, 'utf8')::jsonb
      || jsonb_build_object('configuration', p_configuration)
    )::text, 'utf8'),
    'blake3:' || repeat('c', 64),
    'archive-four-rosters-test',
    'archive-roster-v3-mismatch'
  )
  from archive_frozen_documents
  where option_id = p_option_id;
$$;
grant select on archive_frozen_documents to service_role;
grant execute on function pg_temp.freeze_archive_configuration(text, jsonb) to service_role;

set local role service_role;
select throws_ok(
  $$select pg_temp.freeze_archive_configuration(
    'full-crew', '{"scenario_id":"low-reserve-v1"}'::jsonb
  )$$,
  '22023',
  'room_setup_roster_option_mismatch',
  'a v3 setup freeze independently rejects a configuration outside the selected option'
);
select ok(
  (select bool_and(platform_api.freeze_launch_request_v1(
    account_id,
    launch_request_id,
    roster,
    extensions.digest(roster, 'sha256'),
    setup,
    'blake3:' || repeat('d', 64),
    'archive-four-rosters-test',
    'archive-roster-' || option_id
  )) from archive_frozen_documents),
  'all four v3 roster choices freeze their exact selected members and Room Setup'
);
select ok(
  (select bool_and(platform_api.freeze_launch_request_v1(
    account_id,
    launch_request_id,
    roster,
    extensions.digest(roster, 'sha256'),
    setup,
    'blake3:' || repeat('d', 64),
    'archive-four-rosters-test',
    'archive-roster-' || option_id
  )) from archive_frozen_documents),
  'an exact v3 freeze retry retains the original setup operation'
);
reset role;
select ok(
  (select bool_and(
    requests.roster_frozen_at is not null
    and convert_from(requests.frozen_roster, 'utf8')::jsonb -> 'members'
      = convert_from(documents.roster, 'utf8')::jsonb -> 'members'
    and convert_from(requests.frozen_room_setup_specification, 'utf8')::jsonb -> 'seats'
      = convert_from(documents.setup, 'utf8')::jsonb -> 'seats'
  ) from archive_frozen_documents documents
  join platform_store.launch_requests requests using (launch_request_id)),
  'the database retains the exact v3 members and selected Room seats'
);
set local role service_role;

create temporary table archive_material_snapshots as
select launches.launch_request_id,
       platform_api.read_hosted_launch_material_v1(
         launches.account_id,
         launches.launch_request_id
       ) as material
from archive_launches launches;
grant select on archive_material_snapshots to service_role;
select ok(
  (select bool_and(
    platform_api.read_hosted_launch_material_v1(
      launches.account_id,
      launches.launch_request_id
    ) = snapshots.material
    and snapshots.material -> 'launch_inputs' = jsonb_build_object('roster_option', launches.option_id)
    and jsonb_array_length(snapshots.material -> 'claims') = 1
    and jsonb_array_length(snapshots.material -> 'house_assignments') = cardinality(launches.expected_house_seats)
  ) from archive_launches launches
  join archive_material_snapshots snapshots using (launch_request_id)),
  'returning to all four formations preserves selected seats, Assignments, receipts, and allowances'
);

create temporary table failed_archive_launch as
select pg_temp.archive_launch('full-crew', 5, 'capacity-failure') as launch_request_id;
select platform_api.start_house_fill_v1(
  'd2090000-0000-4000-8000-000000000005',
  (select launch_request_id from failed_archive_launch)
);
reset role;
alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations
set claim_window_opened_at = claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = claim_window_closes_at - interval '31 seconds'
where launch_request_id = (select launch_request_id from failed_archive_launch);
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;
set local role service_role;
select is(
  platform_api.retain_house_fill_selection_v1(
    (select launch_request_id from failed_archive_launch),
    'archive-four-rosters-test'
  ) ->> 'failure_code',
  'runner_capacity_unavailable',
  'an exact full-crew selection that cannot fit fails before Genesis'
);
select is(
  (select count(*)::integer from platform_store.house_runner_reservations
   where launch_request_id = (select launch_request_id from failed_archive_launch)),
  0,
  'pre-Genesis capacity failure creates no partial reservation set'
);
select is(
  (select count(*)::integer from platform_store.house_agent_assignments
   where launch_request_id = (select launch_request_id from failed_archive_launch)),
  0,
  'pre-Genesis capacity failure creates no partial Assignment set'
);

select * from finish();
rollback;
