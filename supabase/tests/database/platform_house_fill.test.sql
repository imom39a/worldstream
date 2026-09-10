begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table('platform_store', 'house_agent_revisions', 'House Agent Revisions exist');
select has_table('platform_store', 'house_agent_host_approvals', 'House Host approvals exist');
select has_table('platform_store', 'house_runner_capacity_gates', 'House Runner gate exists');
select has_table('platform_store', 'house_fill_operations', 'House fill operations exist');
select has_table('platform_store', 'house_fill_candidate_evidence', 'House draws exist');
select has_table('platform_store', 'house_runner_reservations', 'House reservations exist');
select has_table('platform_store', 'house_agent_assignments', 'House assignments exist');

select is(
  (select count(*)::integer from platform_store.house_agent_revisions
   where house_agent_revision_digest in (
     'blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81',
     'blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a'
   )),
  2,
  'the retained Listing pool keeps its two original immutable revisions'
);
select is(
  (select hard_limit from platform_store.house_runner_capacity_gates
   where gate_kind = 'runner_unit'),
  4,
  'the single-Machine House Runner gate admits at most four units'
);
select ok(
  (select bool_and(classes.relrowsecurity)
   from pg_class classes
   join pg_namespace namespaces on namespaces.oid = classes.relnamespace
   where namespaces.nspname = 'platform_store'
     and classes.relname in (
       'house_agent_revisions',
       'house_agent_host_approvals',
       'house_runner_capacity_gates',
       'house_fill_operations',
       'house_fill_candidate_evidence',
       'house_runner_reservations',
       'house_agent_assignments'
     )),
  'every House coordination table has RLS enabled'
);
select ok(
  not has_table_privilege('anon', 'platform_store.house_agent_revisions', 'select')
  and not has_table_privilege('authenticated', 'platform_store.house_agent_revisions', 'select')
  and not has_table_privilege('anon', 'platform_store.house_runner_reservations', 'select')
  and not has_table_privilege('authenticated', 'platform_store.house_agent_assignments', 'select'),
  'browser roles cannot read House configuration, reservations, or assignments'
);
select has_function('platform_api', 'start_house_fill_v1', array['uuid', 'uuid']);
select has_function('platform_api', 'retain_house_fill_selection_v1', array['uuid', 'text']);
select has_function(
  'platform_api',
  'record_house_runner_reservation_v1',
  array['uuid', 'text', 'text', 'bytea', 'bytea', 'text']
);
select has_function('platform_api', 'complete_house_fill_v1', array['uuid']);
select has_function('platform_api', 'read_house_fill_v1', array['uuid', 'uuid']);
select has_function(
  'platform_api',
  'read_hosted_launch_material_v1',
  array['uuid', 'uuid']
);
select ok(
  has_function_privilege(
    'service_role',
    'platform_api.read_hosted_launch_material_v1(uuid, uuid)',
    'execute'
  )
  and not has_function_privilege(
    'authenticated',
    'platform_api.read_hosted_launch_material_v1(uuid, uuid)',
    'execute'
  )
  and not has_function_privilege(
    'anon',
    'platform_api.read_hosted_launch_material_v1(uuid, uuid)',
    'execute'
  ),
  'only the service role can read private hosted launch material'
);
select ok(
  not exists (
    select 1
    from pg_proc procedures
    join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
    where namespaces.nspname = 'platform_api' and procedures.prosecdef
  ),
  'House coordination adds no security-definer API function'
);

-- Preserve retained Runner reservations. The canonical four-unit policy is
-- asserted above; this transaction-only fixture gate is offset by existing
-- occupancy so the test can exercise its four fixture reservations without
-- deleting local state. The enclosing rollback restores the exact gate,
-- constraint, and trigger.
create temporary table house_runner_capacity_baseline as
select count(*)::integer as active_count
from platform_store.house_runner_reservations
where state in ('pending', 'ambiguous', 'succeeded') and released_at is null;
grant select on house_runner_capacity_baseline to service_role;

alter table platform_store.house_runner_capacity_gates
  disable trigger protect_house_runner_capacity_gate_v1;
alter table platform_store.house_runner_capacity_gates
  drop constraint house_runner_gate_exact_limit;
update platform_store.house_runner_capacity_gates gates
set hard_limit = (select active_count + 4 from house_runner_capacity_baseline)
where gates.gate_kind = 'runner_unit';
alter table platform_store.house_runner_capacity_gates
  enable trigger protect_house_runner_capacity_gate_v1;

insert into platform_store.platform_accounts(account_id)
values
  ('20000000-0000-4000-8000-000000000001'),
  ('20000000-0000-4000-8000-000000000002'),
  ('20000000-0000-4000-8000-000000000003'),
  ('20000000-0000-4000-8000-000000000004');

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
  'house-test-host',
  revisions.house_agent_revision_digest,
  'blake3:' || repeat('1', 64),
  'blake3:' || repeat('2', 64),
  'blake3:' || repeat('3', 64),
  'openrouter-house',
  extensions.digest(convert_to(revisions.house_agent_revision_digest, 'utf8'), 'sha256'),
  true
from platform_store.house_agent_revisions revisions;

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
  'house-one-candidate',
  revisions.house_agent_revision_digest,
  'blake3:' || repeat('4', 64),
  'blake3:' || repeat('5', 64),
  'blake3:' || repeat('6', 64),
  'openrouter-house',
  extensions.digest(convert_to('one:' || revisions.house_agent_revision_digest, 'utf8'), 'sha256'),
  true
from platform_store.house_agent_revisions revisions
order by revisions.house_agent_revision_digest
limit 1;

set local role service_role;

create temporary table house_launch as
select * from platform_api.create_launch_request_v1(
  '20000000-0000-4000-8000-000000000001',
  'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
  'house-fill-test',
  decode(repeat('71', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'fill_unclaimed',
  'seat',
  'navigator',
  'account_human'
);

create temporary table late_invitation as
select * from platform_api.rotate_seat_invitation_v1(
  '20000000-0000-4000-8000-000000000001',
  (select launch_request_id from house_launch),
  'insider'
);

create temporary table started_fill as
select platform_api.start_house_fill_v1(
  '20000000-0000-4000-8000-000000000001',
  (select launch_request_id from house_launch)
) as value;

select is(
  ((select value from started_fill) ->> 'state'),
  'claim_window_open',
  'Start with House Agents opens one retained claim window'
);
select is(
  (select operations.claim_window_closes_at - operations.claim_window_opened_at
   from platform_store.house_fill_operations operations
   where operations.launch_request_id = (select launch_request_id from house_launch)),
  interval '30 seconds',
  'the final human-claim window is exactly 30 seconds'
);
select is(
  ((platform_api.start_house_fill_v1(
    '20000000-0000-4000-8000-000000000001',
    (select launch_request_id from house_launch)
  )) ->> 'house_fill_operation_id'),
  ((select value from started_fill) ->> 'house_fill_operation_id'),
  'an identical start retry returns the retained operation'
);

reset role;
alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations operations
set claim_window_opened_at = operations.claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = operations.claim_window_closes_at - interval '31 seconds'
where operations.launch_request_id = (select launch_request_id from house_launch);
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;
set local role service_role;

select throws_ok(
  format(
    'select * from platform_api.claim_invited_seat_v1(%L, decode(%L, ''hex''), %L)',
    '20000000-0000-4000-8000-000000000002',
    encode(extensions.digest(
      convert_to((select invitation_token from late_invitation), 'utf8'),
      'sha256'
    ), 'hex'),
    'account_human'
  ),
  '55000',
  'house_fill_claim_window_closed',
  'a late claimant cannot race the retained selection'
);

create temporary table retained_fill as
select platform_api.retain_house_fill_selection_v1(
  (select launch_request_id from house_launch),
  'house-test-host'
) as value;
select is(
  ((select value from retained_fill) ->> 'state'),
  'reserving',
  'selection retains exact Runner reservation work before any assignment'
);
select is(
  ((select value from retained_fill) ->> 'selected_assignment_count')::integer,
  2,
  'the two unclaimed eligible seats select at most two House revisions'
);
select is(
  (select count(*)::integer
   from platform_store.house_fill_candidate_evidence candidates
   join platform_store.house_fill_operations operations using (house_fill_operation_id)
   where operations.launch_request_id = (select launch_request_id from house_launch)
     and candidates.disposition = 'candidate'
     and octet_length(candidates.random_draw) = 32),
  2,
  'the exact candidate set and cryptographic draws are retained'
);
select is(
  (select count(distinct candidates.house_agent_revision_digest)::integer
   from platform_store.house_fill_candidate_evidence candidates
   join platform_store.house_fill_operations operations using (house_fill_operation_id)
   where operations.launch_request_id = (select launch_request_id from house_launch)
     and candidates.selected_seat_id is not null),
  2,
  'selection is without replacement within the launch lineage'
);
select is(
  (select count(*)::integer
   from platform_store.house_runner_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from house_launch)),
  2,
  'one stable reservation operation exists for every selected seat'
);
select is(
  (select count(*)::integer
   from platform_store.house_agent_assignments assignments
   where assignments.launch_request_id = (select launch_request_id from house_launch)),
  0,
  'selection never creates a partial assignment set'
);

create temporary table reservation_snapshot as
select jsonb_agg(jsonb_build_object(
  'operation', reservations.reservation_operation_id,
  'seat', reservations.seat_id,
  'revision', reservations.house_agent_revision_digest
) order by reservations.seat_id) as value
from platform_store.house_runner_reservations reservations
where reservations.launch_request_id = (select launch_request_id from house_launch);

select is(
  (select jsonb_agg(jsonb_build_object(
    'operation', reservations.reservation_operation_id,
    'seat', reservations.seat_id,
    'revision', reservations.house_agent_revision_digest
  ) order by reservations.seat_id)
   from platform_store.house_runner_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from house_launch)
     and platform_api.retain_house_fill_selection_v1(
       (select launch_request_id from house_launch),
       'house-test-host'
     ) is not null),
  (select value from reservation_snapshot),
  'a selection retry resumes the exact draw and reservation identities'
);
select throws_ok(
  format(
    'select platform_api.retain_house_fill_selection_v1(%L, %L)',
    (select launch_request_id from house_launch),
    'another-host'
  ),
  '23505',
  'house_fill_host_conflict',
  'the retained selection cannot move to another Host installation'
);

do $$
declare
  reservation record;
  receipt bytea;
begin
  for reservation in
    select reservations.*
    from platform_store.house_runner_reservations reservations
    where reservations.launch_request_id = (select launch_request_id from house_launch)
    order by reservations.seat_id
  loop
    receipt := convert_to(jsonb_build_object(
      'outcome', 'succeeded',
      'reservation_operation_id', reservation.reservation_operation_id,
      'runner_unit_id', 'runner-' || reservation.seat_id
    )::text, 'utf8');
    perform platform_api.record_house_runner_reservation_v1(
      reservation.reservation_operation_id,
      'succeeded',
      'runner-' || reservation.seat_id,
      receipt,
      extensions.digest(receipt, 'sha256'),
      null
    );
  end loop;
end;
$$;

select is(
  (select count(*)::integer
   from platform_store.house_agent_assignments assignments
   where assignments.launch_request_id = (select launch_request_id from house_launch)),
  0,
  'successful reservation receipts still do not create assignments one by one'
);
select is(
  (platform_api.complete_house_fill_v1(
    (select launch_request_id from house_launch)
  ) ->> 'state'),
  'assignments_complete',
  'one completion transaction publishes the full assignment set'
);
select is(
  (select count(*)::integer
   from platform_store.house_agent_assignments assignments
   where assignments.launch_request_id = (select launch_request_id from house_launch)),
  2,
  'every selected House seat receives one immutable assignment'
);

create temporary table house_frozen_documents as
with listing as (
  select
    listings.*,
    convert_from(listings.canonical_document, 'utf8')::jsonb as document
  from platform_store.activity_listing_revisions listings
  where listings.listing_revision_digest =
    'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956'
), ordered_members as (
  select
    seats.position,
    case
      when claims.claim_id is not null then jsonb_build_object(
        'seat_id', claims.seat_id,
        'participation', claims.participation_kind,
        'principal_reference', claims.principal_reference,
        'display_name', seats.value ->> 'display_name'
      )
      else jsonb_build_object(
        'seat_id', assignments.seat_id,
        'participation', 'house_agent_fill',
        'principal_reference', assignments.setup_principal_reference,
        'display_name', revisions.display_name,
        'house_agent_revision_digest', assignments.house_agent_revision_digest,
        'agent_profile', jsonb_build_object(
          'profile_id', assignments.agent_profile_id,
          'revision', assignments.agent_profile_revision
        ),
        'runner_template', jsonb_build_object(
          'template_id', assignments.runner_template_id,
          'revision', assignments.runner_template_revision
        )
      )
    end as roster_member,
    case
      when claims.claim_id is not null then jsonb_build_object(
        'label', claims.seat_id,
        'role', seats.value ->> 'role',
        'required', (seats.value ->> 'required')::boolean,
        'display_name', seats.value ->> 'display_name',
        'principal', jsonb_build_object(
          'reference', claims.principal_reference,
          'kind', case claims.participation_kind
            when 'account_human' then 'human' else 'agent' end
        )
      )
      else jsonb_build_object(
        'label', assignments.seat_id,
        'role', seats.value ->> 'role',
        'required', (seats.value ->> 'required')::boolean,
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
      )
    end as setup_seat
  from listing
  cross join lateral jsonb_array_elements(listing.seat_templates)
    with ordinality seats(value, position)
  left join platform_store.seat_claims claims
    on claims.launch_request_id = (select launch_request_id from house_launch)
    and claims.seat_id = seats.value ->> 'seat_id'
    and claims.released_at is null
  left join platform_store.house_agent_assignments assignments
    on assignments.launch_request_id = (select launch_request_id from house_launch)
    and assignments.seat_id = seats.value ->> 'seat_id'
  left join platform_store.house_agent_revisions revisions
    on revisions.house_agent_revision_digest = assignments.house_agent_revision_digest
), documents as (
  select
    jsonb_build_object(
      'schema', 'worldstream/frozen-roster/v1',
      'listing_revision_digest', listing.listing_revision_digest,
      'members', (select jsonb_agg(roster_member order by position) from ordered_members)
    ) as roster,
    jsonb_build_object(
      'schema', 'worldstream/room-setup/v2',
      'pack', listing.document -> 'pack',
      'configuration', listing.room_setup_configuration,
      'seats', (select jsonb_agg(setup_seat order by position) from ordered_members),
      'spectators', jsonb_build_array(
        jsonb_build_object(
          'purpose', 'result_indexer',
          'principal', jsonb_build_object(
            'reference', 'worldstream:result-indexer', 'kind', 'agent'
          )
        ),
        jsonb_build_object(
          'purpose', 'public_relay',
          'principal', jsonb_build_object(
            'reference', 'worldstream:public-relay', 'kind', 'agent'
          )
        )
      ),
      'operator_view', false
    ) as setup
  from listing
)
select
  convert_to(roster::text, 'utf8') as roster,
  convert_to(
    jsonb_set(roster, '{members,1,agent_profile,revision}', '"wrong"'::jsonb)::text,
    'utf8'
  ) as mismatched_roster,
  convert_to(setup::text, 'utf8') as setup
from documents;

select throws_ok(
  $$select platform_api.freeze_launch_request_v1(
    '20000000-0000-4000-8000-000000000001',
    (select launch_request_id from house_launch),
    (select mismatched_roster from house_frozen_documents),
    extensions.digest((select mismatched_roster from house_frozen_documents), 'sha256'),
    (select setup from house_frozen_documents),
    'blake3:' || repeat('f', 64),
    'house-test-host',
    'house-launch-operation'
  )$$,
  '22023',
  'frozen_roster_mismatch',
  'freeze rejects a House member whose exact Profile reference changed'
);
select ok(
  platform_api.freeze_launch_request_v1(
    '20000000-0000-4000-8000-000000000001',
    (select launch_request_id from house_launch),
    (select roster from house_frozen_documents),
    extensions.digest((select roster from house_frozen_documents), 'sha256'),
    (select setup from house_frozen_documents),
    'blake3:' || repeat('f', 64),
    'house-test-host',
    'house-launch-operation'
  ),
  'freeze accepts the complete exact human and House Agent roster'
);
select ok(
  platform_api.freeze_launch_request_v1(
    '20000000-0000-4000-8000-000000000001',
    (select launch_request_id from house_launch),
    (select roster from house_frozen_documents),
    extensions.digest((select roster from house_frozen_documents), 'sha256'),
    (select setup from house_frozen_documents),
    'blake3:' || repeat('f', 64),
    'house-test-host',
    'house-launch-operation'
  ),
  'the exact mixed-roster freeze is idempotent'
);
select ok(
  (select bool_and(
     assignments.execution_allowance = '{
       "call_timeout_seconds":60,
       "concurrent_calls":1,
       "input_tokens_per_call":12000,
       "model_call_attempts":10,
       "output_tokens_per_call":1000,
       "total_input_tokens":120000,
       "total_output_tokens":10000
     }'::jsonb
     and octet_length(assignments.reservation_receipt_digest) = 32
     and assignments.setup_principal_reference =
       'house:' || assignments.launch_request_id::text || ':' || assignments.seat_id
   )
   from platform_store.house_agent_assignments assignments
   where assignments.launch_request_id = (select launch_request_id from house_launch)),
  'assignments copy the fixed allowance, receipt evidence, and server-derived Principal reference'
);
select is(
  (platform_api.read_hosted_launch_material_v1(
    '20000000-0000-4000-8000-000000000001',
    (select launch_request_id from house_launch)
  ) ->> 'version'),
  'platform_hosted_launch_material.v1',
  'the service coordinator receives the versioned launch material contract'
);
select is(
  jsonb_array_length(platform_api.read_hosted_launch_material_v1(
    '20000000-0000-4000-8000-000000000001',
    (select launch_request_id from house_launch)
  ) -> 'house_assignments'),
  2,
  'the coordinator receives every retained House assignment'
);
select is(
  platform_api.read_hosted_launch_material_v1(
    '20000000-0000-4000-8000-000000000002',
    (select launch_request_id from house_launch)
  ),
  null,
  'another account cannot resolve private launch material'
);
select lives_ok(
  format(
    'select platform_api.complete_house_fill_v1(%L)',
    (select launch_request_id from house_launch)
  ),
  'completion is idempotent'
);
select throws_ok(
  format(
    'update platform_store.house_agent_assignments set model_slug = %L where launch_request_id = %L',
    'openrouter/auto',
    (select launch_request_id from house_launch)
  ),
  '42501',
  'permission denied for table house_agent_assignments',
  'the service boundary cannot update Assignment model or provider columns'
);
reset role;
select throws_ok(
  format(
    'update platform_store.house_agent_assignments set model_slug = %L where launch_request_id = %L',
    'openrouter/auto',
    (select launch_request_id from house_launch)
  ),
  '23000',
  'immutable_formation_row',
  'the storage trigger also rejects an Assignment model swap'
);
set local role service_role;

create temporary table failed_launch as
select * from platform_api.create_launch_request_v1(
  '20000000-0000-4000-8000-000000000003',
  'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
  'house-fill-failure',
  decode(repeat('72', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'fill_unclaimed',
  'seat',
  'navigator',
  'account_human'
);
select platform_api.start_house_fill_v1(
  '20000000-0000-4000-8000-000000000003',
  (select launch_request_id from failed_launch)
);
reset role;
alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations operations
set claim_window_opened_at = operations.claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = operations.claim_window_closes_at - interval '31 seconds'
where operations.launch_request_id = (select launch_request_id from failed_launch);
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;
set local role service_role;
select platform_api.retain_house_fill_selection_v1(
  (select launch_request_id from failed_launch),
  'house-test-host'
);

do $$
declare
  reservation platform_store.house_runner_reservations%rowtype;
  receipt bytea;
begin
  select reservations.* into strict reservation
  from platform_store.house_runner_reservations reservations
  where reservations.launch_request_id = (select launch_request_id from failed_launch)
  order by reservations.seat_id
  limit 1;
  receipt := convert_to(jsonb_build_object(
    'failure_code', 'runner_unavailable',
    'outcome', 'terminal_failed',
    'reservation_operation_id', reservation.reservation_operation_id
  )::text, 'utf8');
  perform platform_api.record_house_runner_reservation_v1(
    reservation.reservation_operation_id,
    'terminal_failed',
    null,
    receipt,
    extensions.digest(receipt, 'sha256'),
    'runner_unavailable'
  );
end;
$$;
select is(
  (select launches.state from platform_store.launch_requests launches
   where launches.launch_request_id = (select launch_request_id from failed_launch)),
  'failed_pre_genesis',
  'a terminal Runner reservation failure ends the Launch before Genesis'
);
select is(
  (select count(*)::integer from platform_store.house_agent_assignments assignments
   where assignments.launch_request_id = (select launch_request_id from failed_launch)),
  0,
  'a terminal Runner reservation failure creates no assignment'
);
select is(
  (select count(*)::integer from platform_store.capacity_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from failed_launch)
     and reservations.released_at is null),
  0,
  'proven pre-Genesis failure releases platform launch capacity'
);

create temporary table insufficient_launch as
select * from platform_api.create_launch_request_v1(
  '20000000-0000-4000-8000-000000000004',
  'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
  'house-fill-insufficient',
  decode(repeat('73', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'fill_unclaimed',
  'seat',
  'navigator',
  'account_human'
);
select platform_api.start_house_fill_v1(
  '20000000-0000-4000-8000-000000000004',
  (select launch_request_id from insufficient_launch)
);
reset role;
alter table platform_store.house_fill_operations disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations operations
set claim_window_opened_at = operations.claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = operations.claim_window_closes_at - interval '31 seconds'
where operations.launch_request_id = (select launch_request_id from insufficient_launch);
alter table platform_store.house_fill_operations enable trigger protect_house_fill_operation_v1;
set local role service_role;
select is(
  (platform_api.retain_house_fill_selection_v1(
    (select launch_request_id from insufficient_launch),
    'house-one-candidate'
  ) ->> 'state'),
  'failed_pre_genesis',
  'an insufficient reviewed candidate set fails without reroll or fallback'
);
select is(
  (select count(*)::integer from platform_store.house_runner_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from insufficient_launch)),
  0,
  'insufficient candidates create no partial Runner reservation set'
);

-- A terminal Run can free only the exact retained Runner unit after its Host
-- receipt is durably recorded. These assertions deliberately exercise the
-- RPC boundary, not direct table updates.
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
)
select
  '24000000-0000-4000-8000-000000000001',
  launches.launch_request_id,
  launches.listing_revision_digest,
  launches.creator_account_id,
  'house-test-host',
  'house-launch-operation',
  '01ARZ3NDEKTSV4RRFFQ69G5FBC',
  'blake3:' || repeat('a', 64),
  'worldstream.test.house-retirement',
  '1.0.0',
  'blake3:' || repeat('b', 64),
  0,
  'blake3:' || repeat('c', 64),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  repeat('d', 32),
  'exhibition_platform_house_agents',
  'ready'
from platform_store.launch_requests launches
where launches.launch_request_id = (select launch_request_id from house_launch);

create temporary table terminal_house_retirement_fixture as
select
  assignments.house_agent_assignment_id,
  assignments.execution_allowance,
  reservations.reservation_operation_id,
  reservations.runner_unit_id,
  reservations.reservation_receipt_digest,
  convert_to(jsonb_build_object(
    'schema', 'worldstream/house-runner-retirement-receipt/v1',
    'host_installation_id', 'house-test-host',
    'reservation_operation_id', reservations.reservation_operation_id,
    'launch_request_id', assignments.launch_request_id,
    'house_agent_assignment_id', assignments.house_agent_assignment_id,
    'runner_unit_id', reservations.runner_unit_id,
    'disposition', 'run_terminal',
    'platform_evidence_digest', 'sha256:' || repeat('e', 64),
    'stop_witness', 'blake3:' || repeat('f', 64),
    'authentication_tag', repeat('1', 64)
  )::text, 'utf8') as canonical_receipt
from platform_store.house_agent_assignments assignments
join platform_store.house_runner_reservations reservations
  on reservations.reservation_operation_id = assignments.reservation_operation_id
where assignments.launch_request_id = (select launch_request_id from house_launch)
order by assignments.house_agent_assignment_id;

select is(
  platform_api.record_terminal_house_runner_retirement_v1(
    '24000000-0000-4000-8000-000000000001',
    (select house_agent_assignment_id from terminal_house_retirement_fixture limit 1),
    (select canonical_receipt from terminal_house_retirement_fixture limit 1),
    extensions.digest(
      (select canonical_receipt from terminal_house_retirement_fixture limit 1), 'sha256'
    )
  ),
  false,
  'a nonterminal Run cannot release a House reservation'
);
select is(
  (select count(*)::integer from platform_store.house_runner_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from house_launch)
     and reservations.state = 'succeeded' and reservations.released_at is null),
  2,
  'nonterminal retirement leaves both retained units live'
);

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
  terminal_evidence_digest
)
select
  '24000000-0000-4000-8000-000000000001',
  launches.listing_revision_digest,
  'blake3:' || repeat('1', 64),
  'terminal_without_outcome',
  '01ARZ3NDEKTSV4RRFFQ69G5FBD',
  '{}'::jsonb,
  1,
  'blake3:' || repeat('2', 64),
  'healthy',
  1,
  extensions.digest(convert_to('host-evidence', 'utf8'), 'sha256'),
  convert_to('{}', 'utf8'),
  decode(repeat('ee', 32), 'hex')
from platform_store.launch_requests launches
where launches.launch_request_id = (select launch_request_id from house_launch);

select is(
  platform_api.record_terminal_house_runner_retirement_v1(
    '24000000-0000-4000-8000-000000000099',
    (select house_agent_assignment_id from terminal_house_retirement_fixture limit 1),
    (select canonical_receipt from terminal_house_retirement_fixture limit 1),
    extensions.digest(
      (select canonical_receipt from terminal_house_retirement_fixture limit 1), 'sha256'
    )
  ),
  false,
  'a receipt cannot release its reservation through a different Run'
);
select is(
  platform_api.record_terminal_house_runner_retirement_v1(
    '24000000-0000-4000-8000-000000000001',
    '24000000-0000-4000-8000-000000000099',
    (select canonical_receipt from terminal_house_retirement_fixture limit 1),
    extensions.digest(
      (select canonical_receipt from terminal_house_retirement_fixture limit 1), 'sha256'
    )
  ),
  false,
  'a receipt cannot release its reservation through a different Assignment'
);
select is(
  platform_api.record_terminal_house_runner_retirement_v1(
    '24000000-0000-4000-8000-000000000001',
    (select house_agent_assignment_id from terminal_house_retirement_fixture limit 1),
    (select canonical_receipt from terminal_house_retirement_fixture limit 1),
    extensions.digest(
      (select canonical_receipt from terminal_house_retirement_fixture limit 1), 'sha256'
    )
  ),
  true,
  'the exact terminal Host receipt releases its one matching reservation'
);
select is(
  (select count(*)::integer from platform_store.house_runner_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from house_launch)
     and reservations.state = 'released'),
  1,
  'a signed-shape receipt releases only one retained unit'
);
select is(
  (select count(*)::integer from platform_store.house_runner_reservations reservations
   where reservations.launch_request_id = (select launch_request_id from house_launch)
     and reservations.state = 'succeeded' and reservations.released_at is null),
  1,
  'the other retained unit is not released by a sibling receipt'
);
select is(
  platform_api.record_terminal_house_runner_retirement_v1(
    '24000000-0000-4000-8000-000000000001',
    (select house_agent_assignment_id from terminal_house_retirement_fixture limit 1),
    (select canonical_receipt from terminal_house_retirement_fixture limit 1),
    extensions.digest(
      (select canonical_receipt from terminal_house_retirement_fixture limit 1), 'sha256'
    )
  ),
  true,
  'an exact duplicate receipt is idempotent'
);
select is(
  platform_api.record_terminal_house_runner_retirement_v1(
    '24000000-0000-4000-8000-000000000001',
    (select house_agent_assignment_id from terminal_house_retirement_fixture limit 1),
    convert_to(
      jsonb_set(
        convert_from((select canonical_receipt from terminal_house_retirement_fixture limit 1), 'utf8')::jsonb,
        '{authentication_tag}', to_jsonb(repeat('2', 64))
      )::text,
      'utf8'
    ),
    extensions.digest(
      convert_to(
        jsonb_set(
          convert_from((select canonical_receipt from terminal_house_retirement_fixture limit 1), 'utf8')::jsonb,
          '{authentication_tag}', to_jsonb(repeat('2', 64))
        )::text,
        'utf8'
      ),
      'sha256'
    )
  ),
  false,
  'a conflicting receipt cannot replace the retained exact receipt'
);
select ok(
  (select bool_and(assignments.execution_allowance = fixture.execution_allowance)
   from platform_store.house_agent_assignments assignments
   join terminal_house_retirement_fixture fixture
     on fixture.house_agent_assignment_id = assignments.house_agent_assignment_id)
  and (select bool_and(reservations.reservation_receipt_digest = fixture.reservation_receipt_digest)
       from platform_store.house_runner_reservations reservations
       join terminal_house_retirement_fixture fixture
         on fixture.reservation_operation_id = reservations.reservation_operation_id),
  'retirement never changes retained Assignment allowance or reservation history'
);

-- An expired House claim window still permits the creator's existing, pre-Host
-- cancellation path.  It releases only the live claim and the Launch Request
-- capacity, while preserving the cancelled Launch Request as history.
set local role service_role;

create temporary table expired_cancel_launch as
select * from platform_api.create_launch_request_v1(
  '20000000-0000-4000-8000-000000000002',
  'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
  'expired-house-cancel',
  decode(repeat('72', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'fill_unclaimed',
  'seat',
  'navigator',
  'account_human'
);

select platform_api.start_house_fill_v1(
  '20000000-0000-4000-8000-000000000002',
  (select launch_request_id from expired_cancel_launch)
);

reset role;
alter table platform_store.house_fill_operations
  disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations operations
set claim_window_opened_at = operations.claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = operations.claim_window_closes_at - interval '31 seconds'
where operations.launch_request_id = (select launch_request_id from expired_cancel_launch);
alter table platform_store.house_fill_operations
  enable trigger protect_house_fill_operation_v1;
set local role service_role;

select ok(
  platform_api.cancel_launch_request_v1(
    '20000000-0000-4000-8000-000000000002',
    (select launch_request_id from expired_cancel_launch)
  ),
  'the creator can cancel an expired, pre-Host House-fill launch'
);
select ok(
  (
    select launches.state = 'cancelled'
      and exists (
        select 1
        from platform_store.seat_claims claims
        where claims.launch_request_id = launches.launch_request_id
          and claims.released_at is not null
          and claims.release_reason = 'launch_cancelled'
      )
      and exists (
        select 1
        from platform_store.capacity_reservations reservations
        where reservations.launch_request_id = launches.launch_request_id
          and reservations.released_at is not null
          and reservations.release_reason = 'cancelled'
      )
    from platform_store.launch_requests launches
    where launches.launch_request_id = (select launch_request_id from expired_cancel_launch)
  ),
  'expired cancellation preserves Launch Request history and releases its claim and capacity'
);
reset role;
select throws_ok(
  format(
    $query$update platform_store.seat_claims
      set controlling_account_id = %L
      where launch_request_id = %L and seat_id = 'navigator'$query$,
    '20000000-0000-4000-8000-000000000003',
    (select launch_request_id from expired_cancel_launch)
  ),
  '23000',
  'immutable_seat_claim',
  'expired cancellation never permits an identity mutation'
);
set local role service_role;
select throws_ok(
  format(
    $query$update platform_store.seat_claims
      set released_at = null, release_reason = null
      where launch_request_id = %L and seat_id = 'navigator'$query$,
    (select launch_request_id from expired_cancel_launch)
  ),
  '23000',
  'immutable_seat_claim',
  'expired cancellation never permits a released claim to reactivate'
);

-- The existing frozen-roster guard remains ahead of the narrow exception.
create temporary table expired_guarded_launch as
select * from platform_api.create_launch_request_v1(
  '20000000-0000-4000-8000-000000000003',
  'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
  'expired-house-guarded',
  decode(repeat('73', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'fill_unclaimed',
  'seat',
  'navigator',
  'account_human'
);

select platform_api.start_house_fill_v1(
  '20000000-0000-4000-8000-000000000003',
  (select launch_request_id from expired_guarded_launch)
);

reset role;
alter table platform_store.house_fill_operations
  disable trigger protect_house_fill_operation_v1;
update platform_store.house_fill_operations operations
set claim_window_opened_at = operations.claim_window_opened_at - interval '31 seconds',
    claim_window_closes_at = operations.claim_window_closes_at - interval '31 seconds'
where operations.launch_request_id = (select launch_request_id from expired_guarded_launch);
alter table platform_store.house_fill_operations
  enable trigger protect_house_fill_operation_v1;
update platform_store.launch_requests launches
set roster_frozen_at = clock_timestamp(),
    frozen_roster = convert_to('{}', 'utf8'),
    frozen_roster_digest = extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    frozen_room_setup_specification = convert_to('{}', 'utf8'),
    frozen_room_setup_specification_digest = 'blake3:' || repeat('f', 64),
    host_installation_id = 'house-test-host',
    room_setup_operation_id = 'expired-house-guarded'
where launches.launch_request_id = (select launch_request_id from expired_guarded_launch);
set local role service_role;

select throws_ok(
  format(
    $query$update platform_store.seat_claims
      set released_at = clock_timestamp(), release_reason = 'launch_cancelled'
      where launch_request_id = %L and seat_id = 'navigator'$query$,
    (select launch_request_id from expired_guarded_launch)
  ),
  '23000',
  'launch_roster_frozen',
  'a frozen roster cannot release a claim after the House window closes'
);

select finish();
rollback;
