begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table(
  'platform_store',
  'hosted_operating_state',
  'the hosted installation has a durable operational gate'
);
select has_table(
  'platform_store',
  'hosted_deployment_revisions',
  'exact hosted deployments are retained'
);
select has_table(
  'platform_store',
  'hosted_recovery_checkpoint_events',
  'paired checkpoint evidence is append-only'
);
select ok(
  (select bool_and(classes.relrowsecurity)
   from pg_class classes
   join pg_namespace namespaces on namespaces.oid = classes.relnamespace
   where namespaces.nspname = 'platform_store'
     and classes.relname in (
       'hosted_operating_state',
       'hosted_deployment_revisions',
       'hosted_recovery_checkpoint_events'
     )),
  'every hosted operational table has RLS enabled'
);
select ok(
  not has_table_privilege('anon', 'platform_store.hosted_operating_state', 'select')
  and not has_table_privilege(
    'authenticated', 'platform_store.hosted_deployment_revisions', 'select'
  )
  and not has_table_privilege(
    'anon', 'platform_store.hosted_recovery_checkpoint_events', 'select'
  ),
  'browser roles cannot read operational or recovery records'
);
select has_function(
  'platform_api',
  'set_hosted_operating_state_v1',
  array['text', 'boolean', 'boolean', 'boolean', 'boolean']
);
select has_function(
  'platform_api',
  'record_hosted_deployment_revision_v1',
  array['bytea', 'bytea', 'text', 'text', 'text', 'jsonb']
);
select has_function(
  'platform_api',
  'record_hosted_recovery_checkpoint_v1',
  array['uuid', 'text', 'bytea', 'bytea', 'bytea', 'bytea', 'bytea']
);
select ok(
  has_function_privilege(
    'service_role',
    'platform_api.set_hosted_operating_state_v1(text,boolean,boolean,boolean,boolean)',
    'execute'
  )
  and not has_function_privilege(
    'authenticated',
    'platform_api.set_hosted_operating_state_v1(text,boolean,boolean,boolean,boolean)',
    'execute'
  )
  and not has_function_privilege(
    'anon',
    'platform_api.record_hosted_deployment_revision_v1(bytea,bytea,text,text,text,jsonb)',
    'execute'
  ),
  'only the service role can mutate hosted operations'
);
select ok(
  not exists (
    select 1
    from pg_proc procedures
    join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
    where namespaces.nspname in ('platform_api', 'platform_store')
      and procedures.proname like '%hosted%'
      and procedures.prosecdef
  ),
  'hosted operations add no security-definer function'
);

insert into platform_store.platform_accounts(account_id)
values ('81000000-0000-4000-8000-000000000001');

set local role service_role;

select is(
  platform_api.read_hosted_operating_state_v1('fly-primary') ->> 'launches_open',
  'true',
  'the preview starts open for launch formation'
);

create temporary table hosted_deployment_document(document bytea not null);
insert into hosted_deployment_document(document)
values (convert_to('{"schema":"worldstream/hosted-deployment-revision/v1"}', 'utf8'));
grant select on hosted_deployment_document to service_role;

create temporary table recorded_deployment as
select platform_api.record_hosted_deployment_revision_v1(
  extensions.digest(document, 'sha256'),
  document,
  repeat('a', 40),
  'sha256:' || repeat('b', 64),
  '20260906034201',
  jsonb_build_array('blake3:' || repeat('c', 64))
) as receipt
from hosted_deployment_document;
select is(
  (select receipt ->> 'was_created' from recorded_deployment),
  'true',
  'the first exact deployment record is retained'
);
select is(
  (select platform_api.record_hosted_deployment_revision_v1(
    extensions.digest(document, 'sha256'),
    document,
    repeat('a', 40),
    'sha256:' || repeat('b', 64),
    '20260906034201',
    jsonb_build_array('blake3:' || repeat('c', 64))
  ) ->> 'was_created'
  from hosted_deployment_document),
  'false',
  'an identical deployment record retry is idempotent'
);
select throws_ok(
  $$select platform_api.record_hosted_deployment_revision_v1(
    decode(repeat('11', 32), 'hex'), convert_to('{}', 'utf8'), repeat('a', 40),
    'sha256:' || repeat('b', 64), '20260906034201', '{}'::jsonb
  )$$,
  '22023',
  'invalid_hosted_deployment_revision',
  'a non-array Listing inventory is rejected by the service contract'
);
select throws_ok(
  $$select platform_api.record_hosted_deployment_revision_v1(
    extensions.digest(document, 'sha256'), document, repeat('d', 40),
    'sha256:' || repeat('b', 64), '20260906034201',
    jsonb_build_array('blake3:' || repeat('c', 64))
  ) from hosted_deployment_document$$,
  '23505',
  'hosted_deployment_revision_conflict',
  'one deployment digest cannot be rebound to different evidence'
);

create temporary table recorded_checkpoint as
select platform_api.record_hosted_recovery_checkpoint_v1(
  '82000000-0000-4000-8000-000000000001',
  'created',
  extensions.digest(document, 'sha256'),
  decode(repeat('11', 32), 'hex'),
  decode(repeat('22', 32), 'hex'),
  decode(repeat('33', 32), 'hex'),
  decode(repeat('44', 32), 'hex')
) as receipt
from hosted_deployment_document;
select is(
  (select receipt ->> 'was_created' from recorded_checkpoint),
  'true',
  'the paired checkpoint creation event is retained'
);
select is(
  (select platform_api.record_hosted_recovery_checkpoint_v1(
    '82000000-0000-4000-8000-000000000001',
    'isolated_restore_verified',
    extensions.digest(document, 'sha256'),
    decode(repeat('11', 32), 'hex'),
    decode(repeat('22', 32), 'hex'),
    decode(repeat('33', 32), 'hex'),
    decode(repeat('44', 32), 'hex')
  ) ->> 'event_kind'
  from hosted_deployment_document),
  'isolated_restore_verified',
  'a distinct restore-drill event is append-only evidence'
);
select throws_ok(
  $$select platform_api.record_hosted_recovery_checkpoint_v1(
    '82000000-0000-4000-8000-000000000001', 'unknown', decode(repeat('11', 32), 'hex'),
    decode(repeat('11', 32), 'hex'), decode(repeat('22', 32), 'hex'),
    decode(repeat('33', 32), 'hex'), decode(repeat('44', 32), 'hex')
  )$$,
  '22023',
  'invalid_hosted_checkpoint_event',
  'checkpoint evidence accepts only the closed event vocabulary'
);

select is(
  platform_api.set_hosted_operating_state_v1(
    'fly-primary', false, false, true, false
  ) ->> 'maintenance_mode',
  'true',
  'maintenance closes both admission paths'
);
select throws_ok(
  $$select * from platform_api.create_launch_request_v1(
    '81000000-0000-4000-8000-000000000001',
    'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
    'maintenance-test', decode(repeat('81', 32), 'hex'), convert_to('{}', 'utf8'),
    extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    'worldstream/canonical-json/v1', 'disabled', 'seat', 'navigator', 'account_human')$$,
  '55000',
  'hosted_launches_closed',
  'maintenance rejects new Launch Requests inside the durable transaction'
);
select throws_ok(
  $$insert into platform_store.house_fill_operations (
    launch_request_id, listing_revision_digest, claim_window_closes_at
  ) values (
    '83000000-0000-4000-8000-000000000001',
    'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
    clock_timestamp() + interval '30 seconds'
  )$$,
  '55000',
  'hosted_house_fill_closed',
  'maintenance rejects a new House-fill operation before external work'
);
select throws_ok(
  $$select platform_api.set_hosted_operating_state_v1(
    'fly-primary', true, false, false, true
  )$$,
  '22023',
  'invalid_hosted_operating_state',
  'recovery cannot be fenced while launches remain open'
);
select is(
  platform_api.set_hosted_operating_state_v1(
    'fly-primary', false, false, false, true
  ) ->> 'recovery_fenced',
  'true',
  'a restored authority remains fenced before audit'
);
select is(
  platform_api.set_hosted_operating_state_v1(
    'fly-primary', true, true, false, false
  ) ->> 'launches_open',
  'true',
  'an explicit audited transition reopens the preview'
);

reset role;
select throws_ok(
  $$update platform_store.hosted_deployment_revisions
    set image_digest = 'sha256:' || repeat('e', 64)$$,
  '23000',
  'immutable_hosted_evidence',
  'deployment evidence cannot be rewritten'
);

select * from finish();
rollback;
