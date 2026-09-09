begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table(
  'platform_store', 'provisioning_abandonments',
  'pre-Genesis provisioning-abandonment evidence is retained privately'
);
select has_function(
  'platform_api', 'record_provisioning_abandonment_v1', array['uuid', 'bytea', 'bytea']
);
select ok(
  has_function_privilege(
    'service_role',
    'platform_api.record_provisioning_abandonment_v1(uuid, bytea, bytea)', 'execute'
  ) and not has_function_privilege(
    'authenticated',
    'platform_api.record_provisioning_abandonment_v1(uuid, bytea, bytea)', 'execute'
  ) and not has_table_privilege(
    'authenticated', 'platform_store.provisioning_abandonments', 'select'
  ),
  'only the server records or reads a provisioning-abandonment fence'
);
select ok(
  (select relrowsecurity
   from pg_class classes
   join pg_namespace namespaces on namespaces.oid = classes.relnamespace
   where namespaces.nspname = 'platform_store'
     and classes.relname = 'provisioning_abandonments'),
  'provisioning abandonment evidence has RLS enabled'
);

insert into platform_store.platform_accounts(account_id)
values ('97000000-0000-4000-8000-000000000001');

create temporary table provisioning_fixture as
select
  '97000000-0000-4000-8000-000000000101'::uuid as launch_request_id,
  (select listing_revision_digest
   from platform_store.activity_listing_revisions
   order by listing_revision_digest limit 1) as listing_revision_digest,
  (select house_agent_revision_digest
   from platform_store.house_agent_revisions
   order by house_agent_revision_digest limit 1) as house_agent_revision_digest,
  'blake3:' || repeat('a', 64) as launch_request_digest,
  'provisioning-abandonment-01'::text as operation_id,
  'provisioning-test-host'::text as host_installation_id,
  clock_timestamp() as sampled_at;
grant select on provisioning_fixture to service_role;

insert into platform_store.launch_requests (
  launch_request_id, creator_account_id, listing_revision_digest,
  idempotency_namespace, idempotency_key_digest, canonical_launch_input,
  launch_input_digest, canonicalizer_version, house_fill_choice,
  creator_access_choice, creator_seat_id, creator_participation_kind,
  state, expires_at, created_at, last_transition_at,
  roster_frozen_at, frozen_roster, frozen_roster_digest,
  frozen_room_setup_specification, frozen_room_setup_specification_digest,
  host_installation_id, room_setup_operation_id, host_mutation_started_at
)
select
  fixture.launch_request_id, '97000000-0000-4000-8000-000000000001',
  fixture.listing_revision_digest,
  'provisioning-abandonment-test',
  extensions.digest(convert_to('provisioning-abandonment-key', 'utf8'), 'sha256'),
  convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1', 'disabled', 'spectator', null, 'account_human',
  'provisioning', fixture.sampled_at + interval '24 hours', fixture.sampled_at,
  fixture.sampled_at, fixture.sampled_at, convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'), convert_to('{}', 'utf8'),
  'blake3:' || repeat('b', 64), fixture.host_installation_id, fixture.operation_id,
  fixture.sampled_at
from provisioning_fixture fixture;

insert into platform_store.capacity_reservations (
  launch_request_id, kind, reservation_class, controlling_account_id
)
values (
  '97000000-0000-4000-8000-000000000101',
  'pre_genesis', 'authorized', '97000000-0000-4000-8000-000000000001'
);

insert into platform_store.house_fill_operations (
  launch_request_id, listing_revision_digest, state,
  claim_window_opened_at, claim_window_closes_at, host_installation_id,
  selection_retained_at, candidate_set_digest, exclusion_set_digest,
  selected_assignment_count, completed_at
)
select
  fixture.launch_request_id, fixture.listing_revision_digest, 'assignments_complete',
  fixture.sampled_at - interval '60 seconds', fixture.sampled_at - interval '30 seconds',
  fixture.host_installation_id, fixture.sampled_at - interval '20 seconds',
  extensions.digest(convert_to('candidates', 'utf8'), 'sha256'),
  extensions.digest(convert_to('exclusions', 'utf8'), 'sha256'), 1,
  fixture.sampled_at - interval '10 seconds'
from provisioning_fixture fixture;

insert into platform_store.house_runner_reservations (
  house_fill_operation_id, launch_request_id, seat_id, house_agent_revision_digest,
  state, runner_unit_id, reservation_receipt, reservation_receipt_digest,
  reserved_at, last_transition_at
)
select
  operations.house_fill_operation_id, fixture.launch_request_id, 'house-seat',
  fixture.house_agent_revision_digest, 'succeeded', 'provisioning-test-runner',
  convert_to('{}', 'utf8'), extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  fixture.sampled_at, fixture.sampled_at
from provisioning_fixture fixture
join platform_store.house_fill_operations operations
  on operations.launch_request_id = fixture.launch_request_id;

create temporary table provisioning_evidence as
select convert_to(jsonb_build_object(
  'schema', 'worldstream/hosted-provisioning-abandonment-evidence/v1',
  'host_installation_id', fixture.host_installation_id,
  'listing_revision_digest', fixture.listing_revision_digest,
  'launch_request_id', fixture.launch_request_id,
  'launch_request_digest', fixture.launch_request_digest,
  'room_setup_operation_id', fixture.operation_id,
  'genesis_committed', false,
  'provisioning_fence_digest', 'blake3:' || repeat('c', 64),
  'authentication_tag', repeat('d', 64)
)::text, 'utf8') as value
from provisioning_fixture fixture;
grant select on provisioning_evidence to service_role;

set local role service_role;
select ok(
  not platform_api.record_provisioning_abandonment_v1(
    '97000000-0000-4000-8000-000000000101',
    convert_to(replace(
      convert_from((select value from provisioning_evidence), 'utf8'),
      '97000000-0000-4000-8000-000000000101',
      '97000000-0000-4000-8000-000000000102'
    ), 'utf8'),
    extensions.digest(convert_to(replace(
      convert_from((select value from provisioning_evidence), 'utf8'),
      '97000000-0000-4000-8000-000000000101',
      '97000000-0000-4000-8000-000000000102'
    ), 'utf8'), 'sha256')
  ),
  'an exact-operation fence cannot be replayed with another platform launch request id'
);
select ok(
  platform_api.record_provisioning_abandonment_v1(
    '97000000-0000-4000-8000-000000000101',
    (select value from provisioning_evidence),
    extensions.digest((select value from provisioning_evidence), 'sha256')
  ),
  'exact Host pre-Genesis evidence atomically fences the retained launch'
);
select ok(
  (select launches.state = 'failed_pre_genesis'
    and not exists (
      select 1 from platform_store.activity_runs runs
      where runs.launch_request_id = launches.launch_request_id
    )
    and not exists (
      select 1 from platform_store.capacity_reservations reservations
      where reservations.launch_request_id = launches.launch_request_id
        and reservations.released_at is null
    )
    and not exists (
      select 1 from platform_store.house_runner_reservations reservations
      where reservations.launch_request_id = launches.launch_request_id
        and reservations.state = 'succeeded'
        and reservations.released_at is null
    )
    and exists (
      select 1 from platform_store.house_runner_reservations reservations
      where reservations.launch_request_id = launches.launch_request_id
        and reservations.state = 'released'
        and reservations.release_receipt_digest = extensions.digest(
          (select value from provisioning_evidence), 'sha256'
        )
    )
   from platform_store.launch_requests launches
   where launches.launch_request_id = '97000000-0000-4000-8000-000000000101'),
  'the fence releases launch and succeeded House reservation capacity without inventing a Run'
);
select ok(
  platform_api.record_provisioning_abandonment_v1(
    '97000000-0000-4000-8000-000000000101',
    (select value from provisioning_evidence),
    extensions.digest((select value from provisioning_evidence), 'sha256')
  ),
  'the exact fence retry is idempotent after an interrupted caller restart'
);
select is(
  (
    select count(*)::integer
    from platform_store.provisioning_abandonments abandonments
    where abandonments.launch_request_id = (
      select fixture.launch_request_id from provisioning_fixture fixture
    )
      and abandonments.abandonment_evidence_digest = extensions.digest(
        (select value from provisioning_evidence), 'sha256'
      )
      and abandonments.canonical_abandonment_evidence = (
        select value from provisioning_evidence
      )
  ), 1,
  'an idempotent retry retains one exact provisioning-abandonment receipt for its launch'
);

select * from finish();
rollback;
