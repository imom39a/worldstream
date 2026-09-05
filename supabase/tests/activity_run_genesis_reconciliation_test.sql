begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table('platform_store', 'activity_runs', 'Activity Runs exist');
select has_table(
  'platform_store',
  'activity_run_memberships',
  'immutable Run Membership correspondence exists'
);
select has_table(
  'platform_store',
  'reconciliation_receipts',
  'append-only reconciliation receipts exist'
);
select has_function('platform_api', 'record_genesis_v1', array['uuid', 'bytea', 'bytea']);
select has_function('platform_api', 'read_genesis_reconciliation_v1', array['uuid']);
select has_function('platform_api', 'read_owned_run_v1', array['uuid', 'uuid']);
select has_function(
  'platform_api',
  'resolve_owned_run_membership_v1',
  array['uuid', 'uuid', 'text']
);
select ok(
  (select bool_and(classes.relrowsecurity)
   from pg_class classes
   join pg_namespace namespaces on namespaces.oid = classes.relnamespace
   where namespaces.nspname = 'platform_store'
     and classes.relname in (
       'activity_runs',
       'activity_run_memberships',
       'reconciliation_receipts'
     )),
  'every Run and reconciliation table has RLS enabled'
);
select ok(
  not has_table_privilege('anon', 'platform_store.activity_runs', 'select')
  and not has_table_privilege('authenticated', 'platform_store.activity_runs', 'select')
  and not has_table_privilege(
    'anon',
    'platform_store.activity_run_memberships',
    'select'
  )
  and not has_table_privilege(
    'authenticated',
    'platform_store.activity_run_memberships',
    'select'
  ),
  'browser roles cannot read Run identity or Membership correspondence'
);
select ok(
  has_function_privilege(
    'service_role',
    'platform_api.resolve_owned_run_membership_v1(uuid,uuid,text)',
    'execute'
  )
  and not has_function_privilege(
    'anon',
    'platform_api.resolve_owned_run_membership_v1(uuid,uuid,text)',
    'execute'
  )
  and not has_function_privilege(
    'authenticated',
    'platform_api.resolve_owned_run_membership_v1(uuid,uuid,text)',
    'execute'
  ),
  'only the server role can resolve private Run Membership identity'
);
select ok(
  not exists (
    select 1
    from pg_proc procedures
    join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
    where namespaces.nspname = 'platform_api' and procedures.prosecdef
  ),
  'Genesis reconciliation adds no security-definer API function'
);

insert into platform_store.platform_accounts(account_id)
values
  ('30000000-0000-4000-8000-000000000001'),
  ('30000000-0000-4000-8000-000000000002');

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
  'blake3:' || repeat('a', 64),
  'worldstream.test.genesis-reconciliation',
  convert_to(
    jsonb_build_object(
      'pack', jsonb_build_object(
        'id', 'worldstream.test.genesis',
        'version', '1.0.0',
        'digest', 'blake3:' || repeat('b', 64)
      )
    )::text,
    'utf8'
  ),
  'blake3:' || repeat('b', 64),
  'sha256:' || repeat('c', 64),
  'test-web',
  'public',
  'must_claim_seat',
  'anonymous_by_link',
  'public_recent_results',
  'blake3:' || repeat('d', 64),
  'test/projection/v1',
  'blake3:' || repeat('e', 64),
  'test/result/v1',
  'blake3:' || repeat('f', 64),
  'worldstream/canonical-json/v1',
  1024,
  '{}'::jsonb,
  '[{"seat_id":"navigator","role":"navigator","display_name":"Navigator","required":true,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
  false,
  1800
);

create temporary table genesis_case_documents as
select
  convert_to(
    jsonb_build_object(
      'schema', 'worldstream/frozen-roster/v1',
      'listing_revision_digest', 'blake3:' || repeat('a', 64),
      'members', jsonb_build_array(jsonb_build_object(
        'seat_id', 'navigator',
        'participation', 'account_human',
        'principal_reference', 'seat:navigator',
        'display_name', 'Navigator'
      ))
    )::text,
    'utf8'
  ) as frozen_roster,
  convert_to(
    jsonb_build_object(
      'schema', 'worldstream/room-setup/v2',
      'pack', jsonb_build_object(
        'id', 'worldstream.test.genesis',
        'version', '1.0.0',
        'digest', 'blake3:' || repeat('b', 64)
      ),
      'configuration', '{}'::jsonb,
      'seats', jsonb_build_array(jsonb_build_object(
        'seat_id', 'navigator',
        'role', 'navigator',
        'principal_reference', 'seat:navigator'
      )),
      'spectators', jsonb_build_array(
        jsonb_build_object(
          'purpose', 'result_indexer',
          'principal_reference', 'result-indexer'
        ),
        jsonb_build_object(
          'purpose', 'public_relay',
          'principal_reference', 'public-relay'
        )
      ),
      'operator_view', false
    )::text,
    'utf8'
  ) as frozen_setup;

grant select on genesis_case_documents to service_role;

create function pg_temp.make_genesis_evidence(
  p_launch_request_id uuid,
  p_host_installation_id text,
  p_room_setup_operation_id text,
  p_room_id text,
  p_launch_digest_character text,
  p_hash_character text,
  p_participant_principal_id text,
  p_participant_membership_id text,
  p_indexer_principal_id text,
  p_indexer_membership_id text,
  p_relay_principal_id text,
  p_relay_membership_id text
)
returns jsonb
language sql
stable
as $$
  select jsonb_build_object(
    'schema', 'worldstream/hosted-genesis-evidence/v1',
    'host_installation_id', p_host_installation_id,
    'launch_request_id', p_launch_request_id,
    'listing_revision_digest', 'blake3:' || repeat('a', 64),
    'launch_request_digest', 'blake3:' || repeat(p_launch_digest_character, 64),
    'frozen_roster_digest',
      'sha256:' || encode(extensions.digest(documents.frozen_roster, 'sha256'), 'hex'),
    'room_setup_specification_digest', 'blake3:' || repeat('1', 64),
    'room_setup_operation_id', p_room_setup_operation_id,
    'room_id', p_room_id,
    'pack', jsonb_build_object(
      'id', 'worldstream.test.genesis',
      'version', '1.0.0',
      'digest', 'blake3:' || repeat('b', 64)
    ),
    'genesis_head', jsonb_build_object(
      'room_id', p_room_id,
      'room_seq', 0,
      'genesis_or_transition_hash', 'blake3:' || repeat(p_hash_character, 64),
      'core_schema_version', 'worldstream/core-state/v1',
      'pack_digest', 'blake3:' || repeat('b', 64),
      'core_state_hash', 'blake3:' || repeat('2', 64),
      'activity_state_hash', 'blake3:' || repeat('3', 64),
      'authoritative_state_hash', 'blake3:' || repeat('4', 64)
    ),
    'memberships', jsonb_build_array(
      jsonb_build_object(
        'access_mode', 'participant',
        'purpose', 'participant',
        'seat_id', 'navigator',
        'role', 'navigator',
        'principal_kind', 'human',
        'principal_id', p_participant_principal_id,
        'membership_id', p_participant_membership_id
      ),
      jsonb_build_object(
        'access_mode', 'spectator',
        'purpose', 'result_indexer',
        'seat_id', null,
        'role', null,
        'principal_kind', 'agent',
        'principal_id', p_indexer_principal_id,
        'membership_id', p_indexer_membership_id,
        'scopes', jsonb_build_array(
          'room:attach',
          'room:observe_public',
          'room:replay'
        )
      ),
      jsonb_build_object(
        'access_mode', 'spectator',
        'purpose', 'public_projection_relay',
        'seat_id', null,
        'role', null,
        'principal_kind', 'agent',
        'principal_id', p_relay_principal_id,
        'membership_id', p_relay_membership_id,
        'scopes', jsonb_build_array('room:attach', 'room:observe_public')
      )
    )
  )
  from pg_temp.genesis_case_documents documents;
$$;

set local role service_role;

create temporary table first_launch as
select * from platform_api.create_launch_request_v1(
  '30000000-0000-4000-8000-000000000001',
  'blake3:' || repeat('a', 64),
  'genesis-test',
  decode(repeat('11', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'navigator',
  'account_human'
);

select ok(
  platform_api.freeze_launch_request_v1(
    '30000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    (select frozen_roster from genesis_case_documents),
    extensions.digest((select frozen_roster from genesis_case_documents), 'sha256'),
    (select frozen_setup from genesis_case_documents),
    'blake3:' || repeat('1', 64),
    'genesis-test-host',
    'launch-first'
  ),
  'the first Launch freezes one exact roster and hosted setup'
);
select ok(
  platform_api.authorize_host_mutation_v1(
    '30000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    'genesis-test-host',
    'launch-first'
  ),
  'capacity is authorized before the first Host mutation'
);

create temporary table first_evidence as
with evidence(value) as (
  select pg_temp.make_genesis_evidence(
    (select launch_request_id from first_launch),
    'genesis-test-host',
    'launch-first',
    '01J00000000000000000000000',
    '5',
    '6',
    '01J00000000000000000000001',
    '01J00000000000000000000002',
    '01J00000000000000000000003',
    '01J00000000000000000000004',
    '01J00000000000000000000005',
    '01J00000000000000000000006'
  )
)
select
  value,
  convert_to(value::text, 'utf8') as bytes,
  extensions.digest(convert_to(value::text, 'utf8'), 'sha256') as digest
from evidence;

create temporary table first_record as
select * from platform_api.record_genesis_v1(
  (select launch_request_id from first_launch),
  (select bytes from first_evidence),
  (select digest from first_evidence)
);

select is(
  (select reconciliation_state from first_record),
  'ready'::text,
  'authorized Fly-proven Genesis creates a ready Run'
);
select is(
  (select disposition from first_record),
  'applied'::text,
  'the first valid Genesis is applied'
);
select is(
  (select count(*)::integer
   from platform_store.activity_runs
   where launch_request_id = (select launch_request_id from first_launch)),
  1,
  'one Launch Request maps to exactly one Activity Run'
);
select is(
  (select count(*)::integer
   from platform_store.activity_run_memberships
   where activity_run_id = (select activity_run_id from first_record)),
  3,
  'participant, result indexer, and public relay are promoted atomically'
);
select matches(
  (select public_id from first_record),
  '^[0-9a-f]{32}$',
  'a shareable Listing receives a random post-Genesis public ID'
);
select ok(
  (select canonical_genesis_evidence = (select bytes from first_evidence)
   from platform_store.activity_runs
   where activity_run_id = (select activity_run_id from first_record)),
  'the immutable Run retains the exact canonical Genesis evidence bytes'
);
select ok(
  (select activity_run_id = (select activity_run_id from first_record)
   from platform_store.capacity_reservations
   where launch_request_id = (select launch_request_id from first_launch)
     and kind = 'active_run'
     and released_at is null),
  'the authorized active capacity row is attached to the Run'
);
select ok(
  (select released_at is not null and release_reason = 'genesis_observed'
   from platform_store.capacity_reservations
   where launch_request_id = (select launch_request_id from first_launch)
     and kind = 'pre_genesis'),
  'Genesis releases only pre-Genesis capacity'
);
select is(
  (select state from platform_store.launch_requests
   where launch_request_id = (select launch_request_id from first_launch)),
  'run_created'::text,
  'Genesis advances the Launch Request to run_created'
);

create temporary table first_identity as
select memberships.principal_id, memberships.membership_id, memberships.entry_selector
from platform_store.activity_run_memberships memberships
where memberships.activity_run_id = (select activity_run_id from first_record)
  and memberships.seat_id = 'navigator';

create temporary table first_owned as
select platform_api.read_owned_run_v1(
  '30000000-0000-4000-8000-000000000001',
  (select activity_run_id from first_record)
) as value;
select ok(
  ((select value from first_owned) ->> 'can_enter')::boolean
  and jsonb_array_length((select value from first_owned) -> 'memberships') = 1
  and ((select value from first_owned) -> 'memberships' -> 0 ->> 'entry_selector')
    ~ '^[0-9a-f]{32}$',
  'an owned Run returns only the account-controlled entry selector'
);
select ok(
  (select value::text from first_owned) !~ 'principal_id|membership_id|room_id|host_installation_id',
  'the owned-Run DTO contains no private WorldStream correspondence'
);
select is(
  platform_api.resolve_owned_run_membership_v1(
    '30000000-0000-4000-8000-000000000001',
    (select activity_run_id from first_record),
    (select entry_selector from first_identity)
  ) ->> 'membership_id',
  (select membership_id from first_identity),
  'account plus Run plus opaque selector resolves the exact Membership'
);
select is(
  platform_api.resolve_owned_run_membership_v1(
    '30000000-0000-4000-8000-000000000002',
    (select activity_run_id from first_record),
    (select entry_selector from first_identity)
  ),
  null::jsonb,
  'the selector grants nothing to a different authenticated account'
);
select is(
  platform_api.resolve_owned_run_membership_v1(
    '30000000-0000-4000-8000-000000000001',
    (select activity_run_id from first_record),
    repeat('0', 32)
  ),
  null::jsonb,
  'a browser-invented selector cannot choose a Principal or Membership'
);

create temporary table exact_retry as
select * from platform_api.record_genesis_v1(
  (select launch_request_id from first_launch),
  (select bytes from first_evidence),
  (select digest from first_evidence)
);
select ok(
  (select activity_run_id = (select activity_run_id from first_record)
     and disposition = 'duplicate'
     and safe_code = 'genesis_already_recorded'
   from exact_retry),
  'an exact retry returns the retained Run without replacing identities'
);

reset role;
alter table platform_store.activity_run_memberships
  disable trigger protect_activity_run_membership_v1;
delete from platform_store.activity_run_memberships
where activity_run_id = (select activity_run_id from first_record)
  and purpose = 'result_indexer';
alter table platform_store.activity_run_memberships
  enable trigger protect_activity_run_membership_v1;
set local role service_role;

create temporary table repaired_retry as
select * from platform_api.record_genesis_v1(
  (select launch_request_id from first_launch),
  (select bytes from first_evidence),
  (select digest from first_evidence)
);
select ok(
  (select activity_run_id = (select activity_run_id from first_record)
     and disposition = 'applied'
     and safe_code = 'membership_correspondence_repaired'
   from repaired_retry),
  'an exact retry repairs missing immutable correspondence on the same Run'
);
select ok(
  (select count(*) = 3
   from platform_store.activity_run_memberships
   where activity_run_id = (select activity_run_id from first_record))
  and (select entry_selector from first_identity) = (
    select memberships.entry_selector
    from platform_store.activity_run_memberships memberships
    where memberships.activity_run_id = (select activity_run_id from first_record)
      and memberships.seat_id = 'navigator'
  ),
  'read repair restores the complete set without rotating retained selectors'
);

create temporary table conflicting_evidence as
with changed(value) as (
  select jsonb_set(
    jsonb_set(
      (select value from first_evidence),
      '{room_id}',
      to_jsonb('01J00000000000000000000007'::text)
    ),
    '{genesis_head,room_id}',
    to_jsonb('01J00000000000000000000007'::text)
  )
)
select
  value,
  convert_to(value::text, 'utf8') as bytes,
  extensions.digest(convert_to(value::text, 'utf8'), 'sha256') as digest
from changed;
create temporary table conflict_record as
select * from platform_api.record_genesis_v1(
  (select launch_request_id from first_launch),
  (select bytes from conflicting_evidence),
  (select digest from conflicting_evidence)
);
select ok(
  (select reconciliation_state = 'quarantined'
     and disposition = 'conflict'
     and safe_code = 'genesis_identity_conflict'
   from conflict_record),
  'contradictory Room identity appends a conflict quarantine'
);
select ok(
  (select count(*) = 1
   from platform_store.activity_runs
   where launch_request_id = (select launch_request_id from first_launch))
  and (select room_id = '01J00000000000000000000000'
       from platform_store.activity_runs
       where activity_run_id = (select activity_run_id from first_record)),
  'conflicting evidence never overwrites the first immutable mapping'
);
select ok(
  ((platform_api.read_owned_run_v1(
    '30000000-0000-4000-8000-000000000001',
    (select activity_run_id from first_record)
  )) ->> 'reconciliation_state') = 'quarantined'
  and not ((platform_api.read_owned_run_v1(
    '30000000-0000-4000-8000-000000000001',
    (select activity_run_id from first_record)
  )) ->> 'can_enter')::boolean
  and platform_api.resolve_owned_run_membership_v1(
    '30000000-0000-4000-8000-000000000001',
    (select activity_run_id from first_record),
    (select entry_selector from first_identity)
  ) is null,
  'quarantine suppresses every later owned entry resolution'
);
select is(
  platform_api.read_genesis_reconciliation_v1(
    (select launch_request_id from first_launch)
  ) ->> 'reconciliation_state',
  'quarantined'::text,
  'reconciliation status derives permanent quarantine from conflict receipts'
);

create temporary table unauthorized_launch as
select * from platform_api.create_launch_request_v1(
  '30000000-0000-4000-8000-000000000002',
  'blake3:' || repeat('a', 64),
  'genesis-test',
  decode(repeat('22', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'navigator',
  'account_human'
);
select ok(
  platform_api.freeze_launch_request_v1(
    '30000000-0000-4000-8000-000000000002',
    (select launch_request_id from unauthorized_launch),
    (select frozen_roster from genesis_case_documents),
    extensions.digest((select frozen_roster from genesis_case_documents), 'sha256'),
    (select frozen_setup from genesis_case_documents),
    'blake3:' || repeat('1', 64),
    'genesis-test-host',
    'launch-recovery'
  ),
  'the recovery fixture freezes before the simulated authorization loss'
);
create temporary table unauthorized_evidence as
with evidence(value) as (
  select pg_temp.make_genesis_evidence(
    (select launch_request_id from unauthorized_launch),
    'genesis-test-host',
    'launch-recovery',
    '01J00000000000000000000008',
    '7',
    '8',
    '01J00000000000000000000009',
    '01J0000000000000000000000A',
    '01J0000000000000000000000B',
    '01J0000000000000000000000C',
    '01J0000000000000000000000D',
    '01J0000000000000000000000E'
  )
)
select
  value,
  convert_to(value::text, 'utf8') as bytes,
  extensions.digest(convert_to(value::text, 'utf8'), 'sha256') as digest
from evidence;
create temporary table unauthorized_record as
select * from platform_api.record_genesis_v1(
  (select launch_request_id from unauthorized_launch),
  (select bytes from unauthorized_evidence),
  (select digest from unauthorized_evidence)
);
select ok(
  (select reconciliation_state = 'quarantined'
     and disposition = 'blocked'
     and safe_code = 'authorization_recovery_required'
   from unauthorized_record),
  'Fly-proven Genesis is retained but quarantined after authorization conflict'
);
select is(
  (select count(*)::integer
   from platform_store.activity_run_memberships
   where activity_run_id = (select activity_run_id from unauthorized_record)),
  3,
  'authorization conflict still preserves the exact private Membership set'
);
select ok(
  (select reservation_class = 'quarantined_recovery'
     and released_at is null
     and activity_run_id = (select activity_run_id from unauthorized_record)
   from platform_store.capacity_reservations
   where launch_request_id = (select launch_request_id from unauthorized_launch)
     and kind = 'active_run'),
  'an existing unauthorized Room receives recovery capacity without a replacement'
);
select ok(
  (select public_id ~ '^[0-9a-f]{32}$'
   from platform_store.activity_runs
   where activity_run_id = (select activity_run_id from unauthorized_record))
  and platform_api.resolve_owned_run_membership_v1(
    '30000000-0000-4000-8000-000000000002',
    (select activity_run_id from unauthorized_record),
    (select entry_selector
     from platform_store.activity_run_memberships
     where activity_run_id = (select activity_run_id from unauthorized_record)
       and seat_id = 'navigator')
  ) is null,
  'a recovery Run retains its stable public identity but remains unenterable'
);

reset role;
update platform_store.capacity_reservations
set released_at = clock_timestamp(),
    release_reason = 'projector_terminal'
where activity_run_id = (select activity_run_id from first_record)
  and kind = 'active_run'
  and released_at is null;
set local role service_role;

create temporary table second_launch as
select * from platform_api.create_launch_request_v1(
  '30000000-0000-4000-8000-000000000001',
  'blake3:' || repeat('a', 64),
  'genesis-test',
  decode(repeat('33', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'navigator',
  'account_human'
);
select ok(
  platform_api.freeze_launch_request_v1(
    '30000000-0000-4000-8000-000000000001',
    (select launch_request_id from second_launch),
    (select frozen_roster from genesis_case_documents),
    extensions.digest((select frozen_roster from genesis_case_documents), 'sha256'),
    (select frozen_setup from genesis_case_documents),
    'blake3:' || repeat('1', 64),
    'genesis-test-host',
    'launch-second'
  )
  and platform_api.authorize_host_mutation_v1(
    '30000000-0000-4000-8000-000000000001',
    (select launch_request_id from second_launch),
    'genesis-test-host',
    'launch-second'
  ),
  'the same account can start a later Run after its earlier capacity is released'
);
create temporary table second_evidence as
with evidence(value) as (
  select pg_temp.make_genesis_evidence(
    (select launch_request_id from second_launch),
    'genesis-test-host',
    'launch-second',
    '01J0000000000000000000000H',
    '8',
    '9',
    '01J0000000000000000000000J',
    '01J0000000000000000000000K',
    '01J0000000000000000000000M',
    '01J0000000000000000000000N',
    '01J0000000000000000000000P',
    '01J0000000000000000000000Q'
  )
)
select
  value,
  convert_to(value::text, 'utf8') as bytes,
  extensions.digest(convert_to(value::text, 'utf8'), 'sha256') as digest
from evidence;
create temporary table second_record as
select * from platform_api.record_genesis_v1(
  (select launch_request_id from second_launch),
  (select bytes from second_evidence),
  (select digest from second_evidence)
);
select ok(
  (select reconciliation_state = 'ready' and disposition = 'applied'
   from second_record)
  and (
    select count(*) = 2 and count(distinct memberships.principal_id) = 2
      and count(distinct memberships.entry_selector) = 2
    from platform_store.activity_run_memberships memberships
    where memberships.controlling_account_id =
      '30000000-0000-4000-8000-000000000001'
      and memberships.purpose = 'participant'
  ),
  'one account receives distinct run-scoped Principals and selectors across Runs'
);

reset role;
select throws_ok(
  format(
    'update platform_store.activity_runs set room_id = %L where activity_run_id = %L',
    '01J0000000000000000000000F',
    (select activity_run_id from first_record)
  ),
  '23000',
  'immutable_activity_run_row',
  'even the migration owner cannot mutate a retained Run through normal SQL'
);
select throws_ok(
  format(
    'update platform_store.activity_run_memberships set principal_id = %L where activity_run_id = %L and seat_id = %L',
    '01J0000000000000000000000G',
    (select activity_run_id from first_record),
    'navigator'
  ),
  '23000',
  'immutable_activity_run_row',
  'Run Membership correspondence cannot be rewritten'
);

select * from finish();
rollback;
