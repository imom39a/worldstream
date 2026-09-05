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
  (select count(*)::integer from platform_store.house_agent_revisions),
  2,
  'the reviewed MVP House pool has exactly two immutable revisions'
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
select ok(
  not exists (
    select 1
    from pg_proc procedures
    join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
    where namespaces.nspname = 'platform_api' and procedures.prosecdef
  ),
  'House coordination adds no security-definer API function'
);

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
  'blake3:66926f7d6c88d0799ec0671a4230141272843e98ed4297dabd0d18cb64ada447',
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
  'blake3:66926f7d6c88d0799ec0671a4230141272843e98ed4297dabd0d18cb64ada447',
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
  'blake3:66926f7d6c88d0799ec0671a4230141272843e98ed4297dabd0d18cb64ada447',
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

select finish();
rollback;
