begin;

create extension if not exists pgtap with schema extensions;
select plan(128);

select has_schema('platform_store', 'private store schema exists');
select has_schema('platform_api', 'RPC-only API schema exists');
select has_table('platform_store', 'platform_accounts', 'platform account table exists');
select has_table('platform_store', 'github_identities', 'GitHub identity table exists');
select has_table('platform_store', 'oauth_login_attempts', 'OAuth attempt table exists');
select has_table('platform_store', 'oauth_start_rate_limit', 'OAuth start limiter table exists');
select has_table('platform_store', 'account_mutation_rate_limits', 'account mutation limiter table exists');

select ok(
  (select relrowsecurity from pg_class where oid = 'platform_store.platform_accounts'::regclass),
  'platform accounts have RLS enabled'
);
select ok(
  (select relrowsecurity from pg_class where oid = 'platform_store.github_identities'::regclass),
  'GitHub identities have RLS enabled'
);
select ok(
  (select relrowsecurity from pg_class where oid = 'platform_store.oauth_login_attempts'::regclass),
  'OAuth attempts have RLS enabled'
);
select ok(
  (select relrowsecurity from pg_class where oid = 'platform_store.oauth_start_rate_limit'::regclass),
  'OAuth start limiter has RLS enabled'
);
select ok(
  (select relrowsecurity from pg_class where oid = 'platform_store.account_mutation_rate_limits'::regclass),
  'account mutation limiter has RLS enabled'
);

select ok(not has_schema_privilege('anon', 'platform_store', 'usage'), 'anon cannot use private schema');
select ok(not has_schema_privilege('authenticated', 'platform_store', 'usage'), 'authenticated cannot use private schema');
select ok(not has_schema_privilege('anon', 'platform_api', 'usage'), 'anon cannot use RPC schema');
select ok(not has_schema_privilege('authenticated', 'platform_api', 'usage'), 'authenticated cannot use RPC schema');
select ok(
  not exists (
    select 1 from aclexplode(coalesce(
      (select nspacl from pg_namespace where nspname = 'platform_store'),
      acldefault('n', (select nspowner from pg_namespace where nspname = 'platform_store'))
    )) where grantee = 0 and privilege_type = 'USAGE'
  ),
  'PUBLIC cannot use private schema'
);
select ok(
  not exists (
    select 1 from aclexplode(coalesce(
      (select nspacl from pg_namespace where nspname = 'platform_api'),
      acldefault('n', (select nspowner from pg_namespace where nspname = 'platform_api'))
    )) where grantee = 0 and privilege_type = 'USAGE'
  ),
  'PUBLIC cannot use RPC schema'
);

select ok(not has_table_privilege('anon', 'platform_store.platform_accounts', 'select'), 'anon cannot read accounts');
select ok(not has_table_privilege('anon', 'platform_store.github_identities', 'select'), 'anon cannot read identities');
select ok(not has_table_privilege('anon', 'platform_store.oauth_login_attempts', 'select'), 'anon cannot read OAuth attempts');
select ok(not has_table_privilege('authenticated', 'platform_store.platform_accounts', 'select'), 'authenticated cannot read accounts');
select ok(not has_table_privilege('authenticated', 'platform_store.github_identities', 'select'), 'authenticated cannot read identities');
select ok(not has_table_privilege('authenticated', 'platform_store.oauth_login_attempts', 'select'), 'authenticated cannot read OAuth attempts');
select ok(not has_table_privilege('anon', 'platform_store.oauth_start_rate_limit', 'select'), 'anon cannot read OAuth limiter state');
select ok(not has_table_privilege('authenticated', 'platform_store.oauth_start_rate_limit', 'select'), 'authenticated cannot read OAuth limiter state');
select ok(not has_table_privilege('anon', 'platform_store.account_mutation_rate_limits', 'select'), 'anon cannot read account limiter state');
select ok(not has_table_privilege('authenticated', 'platform_store.account_mutation_rate_limits', 'select'), 'authenticated cannot read account limiter state');
select is(
  (select count(*)::integer from information_schema.table_privileges
   where table_schema = 'platform_store' and grantee = 'PUBLIC'),
  0,
  'PUBLIC has no private table privileges'
);

select has_function('platform_api', 'begin_github_oauth_v1', array['bytea', 'bytea', 'bytea', 'text']);
select has_function('platform_api', 'consume_github_oauth_v1', array['bytea', 'bytea', 'text']);
select has_function('platform_api', 'purge_expired_oauth_attempts_v1', array['integer']);
select has_function('platform_api', 'sync_github_identity_v1', array['uuid', 'text']);
select has_function('platform_api', 'sync_github_identity_v1', array['uuid', 'text', 'text', 'text']);
select has_function('platform_api', 'sync_github_identity_v1', array['uuid', 'text', 'boolean']);
select has_function('platform_api', 'set_public_profile_v1', array['uuid', 'boolean']);
select has_function('platform_api', 'begin_account_erasure_v1', array['uuid']);
select has_function('platform_store', 'admit_oauth_start_v1', array[]::text[]);
select has_function('platform_store', 'admit_account_mutation_v1', array['uuid']);
select has_function('platform_store', 'write_github_identity_v1', array['uuid', 'text', 'text', 'text']);
select is(
  (select array_agg(signatures.signature order by signatures.signature)
   from (
     select procedures.proname || '(' || pg_catalog.oidvectortypes(procedures.proargtypes) || ')' as signature
     from pg_proc procedures
     join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
     where namespaces.nspname = 'platform_api'
   ) signatures),
  array[
    'authorize_host_mutation_v1(uuid, uuid, text, text)',
    'begin_account_erasure_v1(uuid)',
    'begin_github_oauth_v1(bytea, bytea, bytea, text)',
    'cancel_launch_request_v1(uuid, uuid)',
    'claim_invited_seat_v1(uuid, bytea, text)',
    'complete_house_fill_v1(uuid)',
    'consume_github_oauth_v1(bytea, bytea, text)',
    'create_launch_request_v1(uuid, text, text, bytea, bytea, bytea, text, text, text, text, text)',
    'expire_launch_request_v1(uuid)',
    'freeze_launch_request_v1(uuid, uuid, bytea, bytea, bytea, text, text, text)',
    'list_my_games_v1(uuid, timestamp with time zone, uuid, integer)',
    'list_pending_launch_closures_v1(integer)',
    'list_prestart_abandonment_candidates_v1(integer)',
    'list_prestart_house_runner_retirement_candidates_v1(integer)',
    'list_recent_results_v1(integer)',
    'list_reconciliation_candidates_v1(integer)',
    'list_terminal_house_runner_retirement_candidates_v1(integer)',
    'mark_reconciliation_attempt_v1(uuid)',
    'purge_expired_oauth_attempts_v1(integer)',
    'read_genesis_reconciliation_v1(uuid)',
    'read_hosted_launch_material_v1(uuid, uuid)',
    'read_hosted_operating_state_v1(text)',
    'read_hosted_recovery_material_v1(uuid)',
    'read_hosted_schema_head_v1()',
    'read_house_fill_v1(uuid, uuid)',
    'read_integrity_reconciliation_v1(uuid)',
    'read_launch_request_v1(uuid, uuid)',
    'read_owned_run_v1(uuid, uuid)',
    'read_prestart_house_runner_retirements_v1(uuid)',
    'read_public_relay_binding_candidate_v1(uuid)',
    'read_public_run_v1(text)',
    'read_result_reconciliation_v1(uuid)',
    'read_terminal_house_runner_retirements_v1(uuid)',
    'read_terminal_reconciliation_v1(uuid)',
    'reconcile_terminal_activity_capacity_v1(integer)',
    'record_genesis_v1(uuid, bytea, bytea)',
    'record_hosted_deployment_revision_v1(bytea, bytea, text, text, text, jsonb)',
    'record_hosted_recovery_checkpoint_v1(uuid, text, bytea, bytea, bytea, bytea, bytea)',
    'record_house_runner_reservation_v1(uuid, text, text, bytea, bytea, text)',
    'record_integrity_observation_v1(uuid, bytea, bytea)',
    'record_launch_closure_v1(uuid, bytea, bytea)',
    'record_prestart_abandonment_v1(uuid, bytea, bytea)',
    'record_prestart_house_runner_retirement_v1(uuid, uuid, bytea, bytea)',
    'record_provisioning_abandonment_v1(uuid, bytea, bytea)',
    'record_public_relay_binding_v1(uuid, bytea, bytea, bytea, bytea)',
    'record_result_v1(uuid, bytea, bytea, bytea, bytea)',
    'record_run_terminal_v1(uuid, bytea, bytea)',
    'record_terminal_conflict_v1(uuid, bytea, bytea)',
    'record_terminal_house_runner_retirement_v1(uuid, uuid, bytea, bytea)',
    'release_seat_claim_v1(uuid, uuid, text)',
    'request_launch_closure_v1(uuid, uuid, text, bytea, bytea)',
    'reset_seat_claim_v1(uuid, uuid, text)',
    'resolve_owned_run_membership_v1(uuid, uuid, text)',
    'retain_house_fill_selection_v1(uuid, text)',
    'rotate_seat_invitation_v1(uuid, uuid, text)',
    'set_hosted_operating_state_v1(text, boolean, boolean, boolean, boolean)',
    'set_public_profile_v1(uuid, boolean)',
    'start_house_fill_v1(uuid, uuid)',
    'sync_github_identity_v1(uuid, text)',
    'sync_github_identity_v1(uuid, text, boolean)',
    'sync_github_identity_v1(uuid, text, text, text)'
  ]::text[],
  'platform API contains exactly the reviewed RPC signatures'
);
select ok(
  not exists (
    select 1
    from pg_proc procedures
    join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
    where namespaces.nspname = 'platform_api' and procedures.prosecdef
  ),
  'every exposed platform API routine is security invoker'
);

select ok(not has_function_privilege('anon', 'platform_api.begin_github_oauth_v1(bytea,bytea,bytea,text)', 'execute'), 'anon cannot begin OAuth');
select ok(not has_function_privilege('anon', 'platform_api.consume_github_oauth_v1(bytea,bytea,text)', 'execute'), 'anon cannot consume OAuth');
select ok(not has_function_privilege('anon', 'platform_api.purge_expired_oauth_attempts_v1(integer)', 'execute'), 'anon cannot purge OAuth');
select ok(not has_function_privilege('anon', 'platform_api.sync_github_identity_v1(uuid,text)', 'execute'), 'anon cannot resolve identities');
select ok(not has_function_privilege('anon', 'platform_api.sync_github_identity_v1(uuid,text,text,text)', 'execute'), 'anon cannot sync identities');
select ok(not has_function_privilege('anon', 'platform_api.sync_github_identity_v1(uuid,text,boolean)', 'execute'), 'anon cannot admit session refreshes');
select ok(not has_function_privilege('anon', 'platform_api.set_public_profile_v1(uuid,boolean)', 'execute'), 'anon cannot change profiles');
select ok(not has_function_privilege('anon', 'platform_api.begin_account_erasure_v1(uuid)', 'execute'), 'anon cannot erase accounts');
select ok(not has_function_privilege('anon', 'platform_store.admit_oauth_start_v1()', 'execute'), 'anon cannot use OAuth limiter');
select ok(not has_function_privilege('anon', 'platform_store.admit_account_mutation_v1(uuid)', 'execute'), 'anon cannot use account limiter');
select ok(not has_function_privilege('anon', 'platform_store.write_github_identity_v1(uuid,text,text,text)', 'execute'), 'anon cannot use the identity writer');

select ok(not has_function_privilege('authenticated', 'platform_api.begin_github_oauth_v1(bytea,bytea,bytea,text)', 'execute'), 'authenticated cannot begin OAuth');
select ok(not has_function_privilege('authenticated', 'platform_api.consume_github_oauth_v1(bytea,bytea,text)', 'execute'), 'authenticated cannot consume OAuth');
select ok(not has_function_privilege('authenticated', 'platform_api.purge_expired_oauth_attempts_v1(integer)', 'execute'), 'authenticated cannot purge OAuth');
select ok(not has_function_privilege('authenticated', 'platform_api.sync_github_identity_v1(uuid,text)', 'execute'), 'authenticated cannot resolve identities');
select ok(not has_function_privilege('authenticated', 'platform_api.sync_github_identity_v1(uuid,text,text,text)', 'execute'), 'authenticated cannot sync identities');
select ok(not has_function_privilege('authenticated', 'platform_api.sync_github_identity_v1(uuid,text,boolean)', 'execute'), 'authenticated cannot admit session refreshes');
select ok(not has_function_privilege('authenticated', 'platform_api.set_public_profile_v1(uuid,boolean)', 'execute'), 'authenticated cannot change profiles');
select ok(not has_function_privilege('authenticated', 'platform_api.begin_account_erasure_v1(uuid)', 'execute'), 'authenticated cannot erase accounts');
select ok(not has_function_privilege('authenticated', 'platform_store.admit_oauth_start_v1()', 'execute'), 'authenticated cannot use OAuth limiter');
select ok(not has_function_privilege('authenticated', 'platform_store.admit_account_mutation_v1(uuid)', 'execute'), 'authenticated cannot use account limiter');
select ok(not has_function_privilege('authenticated', 'platform_store.write_github_identity_v1(uuid,text,text,text)', 'execute'), 'authenticated cannot use the identity writer');

select ok(
  not exists (
    select 1
    from (values
      ('platform_api.list_my_games_v1(uuid,timestamptz,uuid,integer)'),
      ('platform_api.list_prestart_abandonment_candidates_v1(integer)'),
      ('platform_api.list_prestart_house_runner_retirement_candidates_v1(integer)'),
      ('platform_api.list_terminal_house_runner_retirement_candidates_v1(integer)'),
      ('platform_api.read_prestart_house_runner_retirements_v1(uuid)'),
      ('platform_api.read_terminal_house_runner_retirements_v1(uuid)'),
      ('platform_api.record_prestart_abandonment_v1(uuid,bytea,bytea)'),
      ('platform_api.record_prestart_house_runner_retirement_v1(uuid,uuid,bytea,bytea)'),
      ('platform_api.record_terminal_house_runner_retirement_v1(uuid,uuid,bytea,bytea)')
    ) reviewed(function_signature)
    where has_function_privilege('anon', reviewed.function_signature, 'execute')
       or has_function_privilege('authenticated', reviewed.function_signature, 'execute')
  ),
  'browser roles cannot execute the reviewed My Games or House retirement RPCs'
);
select ok(
  not exists (
    select 1
    from (values
      ('platform_api.list_my_games_v1(uuid,timestamptz,uuid,integer)'),
      ('platform_api.list_prestart_abandonment_candidates_v1(integer)'),
      ('platform_api.list_prestart_house_runner_retirement_candidates_v1(integer)'),
      ('platform_api.list_terminal_house_runner_retirement_candidates_v1(integer)'),
      ('platform_api.read_prestart_house_runner_retirements_v1(uuid)'),
      ('platform_api.read_terminal_house_runner_retirements_v1(uuid)'),
      ('platform_api.record_prestart_abandonment_v1(uuid,bytea,bytea)'),
      ('platform_api.record_prestart_house_runner_retirement_v1(uuid,uuid,bytea,bytea)'),
      ('platform_api.record_terminal_house_runner_retirement_v1(uuid,uuid,bytea,bytea)')
    ) reviewed(function_signature)
    where not has_function_privilege('service_role', reviewed.function_signature, 'execute')
  ),
  'service role can execute every reviewed My Games or House retirement RPC'
);

select ok(has_function_privilege('service_role', 'platform_api.begin_github_oauth_v1(bytea,bytea,bytea,text)', 'execute'), 'service role can begin OAuth');
select ok(has_function_privilege('service_role', 'platform_api.consume_github_oauth_v1(bytea,bytea,text)', 'execute'), 'service role can consume OAuth');
select ok(has_function_privilege('service_role', 'platform_api.purge_expired_oauth_attempts_v1(integer)', 'execute'), 'service role can purge OAuth');
select ok(has_function_privilege('service_role', 'platform_api.sync_github_identity_v1(uuid,text)', 'execute'), 'service role can resolve identities');
select ok(has_function_privilege('service_role', 'platform_api.sync_github_identity_v1(uuid,text,text,text)', 'execute'), 'service role can sync identities');
select ok(has_function_privilege('service_role', 'platform_api.sync_github_identity_v1(uuid,text,boolean)', 'execute'), 'service role can admit session refreshes');
select ok(has_function_privilege('service_role', 'platform_api.set_public_profile_v1(uuid,boolean)', 'execute'), 'service role can change profiles');
select ok(has_function_privilege('service_role', 'platform_api.begin_account_erasure_v1(uuid)', 'execute'), 'service role can erase accounts');
select ok(has_function_privilege('service_role', 'platform_store.admit_oauth_start_v1()', 'execute'), 'service role can use OAuth limiter');
select ok(has_function_privilege('service_role', 'platform_store.admit_account_mutation_v1(uuid)', 'execute'), 'service role can use account limiter');
select ok(has_function_privilege('service_role', 'platform_store.write_github_identity_v1(uuid,text,text,text)', 'execute'), 'service role can use the identity writer');
select is(
  (select count(*)::integer from information_schema.routine_privileges
   where routine_schema in ('platform_store', 'platform_api') and grantee = 'PUBLIC'),
  0,
  'PUBLIC has no application function privileges'
);

select ok(has_table_privilege('service_role', 'platform_store.platform_accounts', 'select'), 'service role can read accounts');
select ok(has_table_privilege('service_role', 'platform_store.platform_accounts', 'insert'), 'service role can insert accounts');
select ok(not has_table_privilege('service_role', 'platform_store.platform_accounts', 'update'), 'service role has no broad account update');
select ok(has_column_privilege('service_role', 'platform_store.platform_accounts', 'erased_at', 'update'), 'service role can tombstone accounts');
select ok(not has_column_privilege('service_role', 'platform_store.platform_accounts', 'created_at', 'update'), 'service role cannot rewrite account creation time');
select ok(not has_table_privilege('service_role', 'platform_store.platform_accounts', 'delete'), 'service role cannot delete account tombstones');
select ok(has_table_privilege('service_role', 'platform_store.github_identities', 'select'), 'service role can read identities');
select ok(has_table_privilege('service_role', 'platform_store.github_identities', 'insert'), 'service role can link identities');
select ok(not has_table_privilege('service_role', 'platform_store.github_identities', 'update'), 'service role has no broad identity update');
select ok(has_column_privilege('service_role', 'platform_store.github_identities', 'refreshed_at', 'update'), 'service role can refresh identities');
select ok(not has_column_privilege('service_role', 'platform_store.github_identities', 'provider_subject', 'update'), 'service role cannot rewrite provider subjects');
select ok(not has_table_privilege('service_role', 'platform_store.github_identities', 'delete'), 'service role cannot directly delete identities');
select ok(has_table_privilege('service_role', 'platform_store.oauth_login_attempts', 'select'), 'service role can read OAuth attempts');
select ok(has_table_privilege('service_role', 'platform_store.oauth_login_attempts', 'insert'), 'service role can insert OAuth attempts');
select ok(not has_table_privilege('service_role', 'platform_store.oauth_login_attempts', 'update'), 'service role has no broad OAuth-attempt update');
select ok(has_column_privilege('service_role', 'platform_store.oauth_login_attempts', 'consumed_at', 'update'), 'service role can consume OAuth attempts');
select ok(not has_column_privilege('service_role', 'platform_store.oauth_login_attempts', 'expires_at', 'update'), 'service role cannot rewrite OAuth expiry');
select ok(has_table_privilege('service_role', 'platform_store.oauth_login_attempts', 'delete'), 'service role can purge OAuth attempts');
select ok(has_table_privilege('service_role', 'platform_store.oauth_start_rate_limit', 'select'), 'service role can execute the invoker OAuth limiter');
select ok(has_table_privilege('service_role', 'platform_store.account_mutation_rate_limits', 'select'), 'service role can execute the invoker account limiter');

select has_index(
  'platform_store',
  'oauth_login_attempts',
  'oauth_login_attempts_expiry_idx',
  'OAuth expiry lookup is indexed'
);

create temporary table begun_oauth as
select * from platform_api.begin_github_oauth_v1(
  decode(repeat('aa', 32), 'hex'),
  decode(repeat('cc', 64), 'hex'),
  decode(repeat('bb', 32), 'hex'),
  '/activities/heist'
);
select is((select count(*)::integer from begun_oauth), 1, 'OAuth attempt starts once');
select is(
  (select expires_at - created_at from platform_store.oauth_login_attempts limit 1),
  interval '10 minutes',
  'OAuth attempt has exact ten-minute lifetime'
);
select is(
  (select count(*)::integer from platform_api.consume_github_oauth_v1(
    decode(repeat('aa', 32), 'hex'), decode(repeat('bb', 32), 'hex'), '/changed'
  )),
  0,
  'changed return target cannot consume attempt'
);
select is(
  (select count(*)::integer from platform_api.consume_github_oauth_v1(
    decode(repeat('aa', 32), 'hex'), decode(repeat('bb', 32), 'hex'), '/activities/heist'
  )),
  1,
  'matching OAuth attempt is consumed'
);
select is(
  (select count(*)::integer from platform_api.consume_github_oauth_v1(
    decode(repeat('aa', 32), 'hex'), decode(repeat('bb', 32), 'hex'), '/activities/heist'
  )),
  0,
  'OAuth attempt replay is rejected'
);

select is(
  (select accepted_count from platform_store.oauth_start_rate_limit where singleton),
  1,
  'OAuth start admission is part of attempt creation'
);
update platform_store.oauth_start_rate_limit set accepted_count = 120 where singleton;
select is(
  (select count(*)::integer from platform_api.begin_github_oauth_v1(
    decode(repeat('44', 32), 'hex'), decode(repeat('55', 64), 'hex'),
    decode(repeat('66', 32), 'hex'), '/activities/heist'
  )),
  0,
  'OAuth attempt creation is rejected at the shared limit'
);

with timing as (select clock_timestamp() - interval '11 minutes' as created_at)
insert into platform_store.oauth_login_attempts (
  state_digest,
  encrypted_pkce_verifier,
  browser_nonce_digest,
  return_target,
  created_at,
  expires_at
) select
  decode(repeat('11', 32), 'hex'),
  decode(repeat('22', 64), 'hex'),
  decode(repeat('33', 32), 'hex'),
  '/activities/heist',
  timing.created_at,
  timing.created_at + interval '10 minutes'
from timing;
select is(
  (select count(*)::integer from platform_api.consume_github_oauth_v1(
    decode(repeat('11', 32), 'hex'), decode(repeat('33', 32), 'hex'), '/activities/heist'
  )),
  0,
  'expired OAuth attempt is rejected'
);

insert into platform_store.platform_accounts(account_id)
values ('00000000-0000-4000-8000-000000000001');
select throws_ok(
  $$update platform_store.platform_accounts
    set created_at = created_at + interval '1 second'
    where account_id = '00000000-0000-4000-8000-000000000001'$$,
  '23000',
  'immutable_platform_account',
  'account identity and creation time are immutable'
);
select throws_ok(
  $$select * from platform_api.begin_github_oauth_v1(
    decode(repeat('dd', 32), 'hex'), decode(repeat('ee', 64), 'hex'),
    decode(repeat('ff', 32), 'hex'), 'https://evil.example'
  )$$,
  '23514',
  null,
  'external OAuth return targets are rejected'
);

insert into auth.users(id) values ('00000000-0000-4000-8000-000000000010');
create temporary table identity_account_count_before_sync as
select count(*)::integer as account_count from platform_store.platform_accounts;

create temporary table first_identity_sync as
select * from platform_api.sync_github_identity_v1(
  '00000000-0000-4000-8000-000000000010',
  'github-subject-10',
  'octocat',
  'https://avatars.githubusercontent.com/u/10'
);
select is(
  (select accepted_count from platform_store.account_mutation_rate_limits
   where auth_user_id = '00000000-0000-4000-8000-000000000010'),
  1,
  'account mutation admission is part of verified identity sync'
);
update platform_store.account_mutation_rate_limits
set accepted_count = 32
where auth_user_id = '00000000-0000-4000-8000-000000000010';
select is(
  (select count(*)::integer from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000010',
    'github-subject-10',
    'octocat',
    'https://avatars.githubusercontent.com/u/10'
  )),
  0,
  'account mutation is rejected at the shared limit'
);
select is((select count(*)::integer from first_identity_sync), 1, 'verified GitHub identity creates one account');
select is(
  (select count(*)::integer from platform_store.platform_accounts),
  (select account_count + 1 from identity_account_count_before_sync),
  'identity sync adds exactly one account'
);
select is(
  (select provider_subject from platform_store.github_identities where auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'github-subject-10',
  'provider subject is retained as the identity key'
);
update platform_store.account_mutation_rate_limits
set accepted_count = 0
where auth_user_id = '00000000-0000-4000-8000-000000000010';
select is(
  (select account_id from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000010',
    'github-subject-10',
    'octocat-renamed',
    'https://avatars.githubusercontent.com/u/10'
  )),
  (select account_id from first_identity_sync),
  'identity refresh preserves the Platform Account'
);
select is(
  (select count(*)::integer from platform_store.platform_accounts),
  (select account_count + 1 from identity_account_count_before_sync),
  'identity refresh does not duplicate accounts'
);
create temporary table identity_before_resolve as
select github_login, avatar_url, refreshed_at
from platform_store.github_identities
where auth_user_id = '00000000-0000-4000-8000-000000000010';
create temporary table resolved_identity as
select * from platform_api.sync_github_identity_v1(
  '00000000-0000-4000-8000-000000000010',
  'github-subject-10'
);
select ok(
  (select count(*) = 1 from resolved_identity)
  and (select before.github_login = identities.github_login
         and before.avatar_url = identities.avatar_url
         and before.refreshed_at = identities.refreshed_at
       from identity_before_resolve before
       cross join platform_store.github_identities identities
       where identities.auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'session identity resolution returns the account without refreshing retained identity data'
);
update platform_store.account_mutation_rate_limits
set accepted_count = 32
where auth_user_id = '00000000-0000-4000-8000-000000000010';
select is(
  (select admitted from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000010',
    'github-subject-10',
    true
  )),
  false,
  'session refresh is rejected before token rotation at the shared account limit'
);
update platform_store.account_mutation_rate_limits
set accepted_count = 0
where auth_user_id = '00000000-0000-4000-8000-000000000010';
create temporary table admitted_session_refresh as
select * from platform_api.sync_github_identity_v1(
  '00000000-0000-4000-8000-000000000010',
  'github-subject-10',
  true
);
select ok(
  (select count(*) = 1 and bool_and(admitted) from admitted_session_refresh)
  and (select accepted_count = 1
       from platform_store.account_mutation_rate_limits
       where auth_user_id = '00000000-0000-4000-8000-000000000010')
  and (select before.github_login = identities.github_login
         and before.avatar_url = identities.avatar_url
       from identity_before_resolve before
       cross join platform_store.github_identities identities
       where identities.auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'session refresh preserves current profile data and reserves one durable mutation allowance'
);
create temporary table cleared_identity_sync as
select * from platform_api.sync_github_identity_v1(
  '00000000-0000-4000-8000-000000000010',
  'github-subject-10',
  null,
  null
);
select ok(
  (select count(*) = 1 from cleared_identity_sync)
  and (select github_login is null and avatar_url is null
       from platform_store.github_identities
       where auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'identity sync clears stale optional presentation data when GitHub omits both values'
);
select throws_ok(
  $$select * from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000010',
    'different-github-subject',
    'octocat',
    'https://avatars.githubusercontent.com/u/10'
  )$$,
  '23505',
  'github_identity_conflict',
  'one Auth user cannot be rebound to a different GitHub subject'
);
insert into auth.users(id) values ('00000000-0000-4000-8000-000000000011');
select throws_ok(
  $$select * from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000011',
    'github-subject-10',
    'octocat',
    'https://avatars.githubusercontent.com/u/10'
  )$$,
  '23505',
  'github_identity_conflict',
  'one GitHub subject cannot be rebound to a different Auth user'
);
select ok(
  platform_api.set_public_profile_v1('00000000-0000-4000-8000-000000000010', true),
  'profile opt-in succeeds for an active verified identity'
);
select ok(
  (select public_profile_enabled from platform_store.github_identities where auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'profile opt-in is retained'
);
select is(
  (select count(*)::integer from platform_api.begin_account_erasure_v1('00000000-0000-4000-8000-000000000010')),
  1,
  'erasure start returns one stable account tombstone'
);
select ok(
  (select erasure_requested_at is not null
     and not public_profile_enabled
     and github_login is null
     and avatar_url is null
   from platform_store.github_identities
   where auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'erasure start disables publication and scrubs optional profile data'
);
create temporary table limiter_before_erased_refresh as
select accepted_count
from platform_store.account_mutation_rate_limits
where auth_user_id = '00000000-0000-4000-8000-000000000010';
select ok(
  (select not admitted from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000010',
    'github-subject-10',
    true
  ))
  and (select before.accepted_count = limits.accepted_count
       from limiter_before_erased_refresh before
       cross join platform_store.account_mutation_rate_limits limits
       where limits.auth_user_id = '00000000-0000-4000-8000-000000000010'),
  'an erased identity cannot refresh or consume durable admission capacity'
);
select throws_ok(
  $$select * from platform_api.sync_github_identity_v1(
    '00000000-0000-4000-8000-000000000010',
    'github-subject-10',
    'octocat',
    'https://avatars.githubusercontent.com/u/10'
  )$$,
  '55000',
  'account_erasure_pending',
  'a tombstoned Platform Account cannot be reactivated'
);

select * from finish();
rollback;
