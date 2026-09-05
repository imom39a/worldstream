begin;

create extension if not exists pgtap with schema extensions;
select no_plan();

select has_table('platform_store', 'activity_listing_revisions', 'deployed Listing copies exist');
select has_table('platform_store', 'launch_requests', 'Launch Requests exist');
select has_table('platform_store', 'capacity_gates', 'capacity gate exists');
select has_table('platform_store', 'capacity_reservations', 'capacity reservations exist');
select has_table('platform_store', 'seat_invitations', 'Seat Invitations exist');
select has_table('platform_store', 'seat_claims', 'Seat Claims exist');

select ok(
  (select bool_and(classes.relrowsecurity)
   from pg_class classes
   join pg_namespace namespaces on namespaces.oid = classes.relnamespace
   where namespaces.nspname = 'platform_store'
     and classes.relname in (
       'activity_listing_revisions',
       'launch_requests',
       'capacity_gates',
       'capacity_reservations',
       'seat_invitations',
       'seat_claims'
     )),
  'every formation table has RLS enabled'
);
select ok(
  not has_table_privilege('anon', 'platform_store.launch_requests', 'select')
  and not has_table_privilege('authenticated', 'platform_store.launch_requests', 'select')
  and not has_table_privilege('anon', 'platform_store.seat_claims', 'select')
  and not has_table_privilege('authenticated', 'platform_store.seat_claims', 'select'),
  'browser roles cannot read formation tables'
);

select has_function(
  'platform_api',
  'create_launch_request_v1',
  array['uuid', 'text', 'text', 'bytea', 'bytea', 'bytea', 'text', 'text', 'text', 'text', 'text']
);
select has_function('platform_api', 'read_launch_request_v1', array['uuid', 'uuid']);
select has_function('platform_api', 'cancel_launch_request_v1', array['uuid', 'uuid']);
select has_function('platform_api', 'expire_launch_request_v1', array['uuid']);
select has_function('platform_api', 'rotate_seat_invitation_v1', array['uuid', 'uuid', 'text']);
select has_function('platform_api', 'claim_invited_seat_v1', array['uuid', 'bytea', 'text']);
select has_function('platform_api', 'release_seat_claim_v1', array['uuid', 'uuid', 'text']);
select has_function('platform_api', 'reset_seat_claim_v1', array['uuid', 'uuid', 'text']);
select has_function(
  'platform_api',
  'freeze_launch_request_v1',
  array['uuid', 'uuid', 'bytea', 'bytea', 'bytea', 'text', 'text', 'text']
);
select has_function(
  'platform_api',
  'authorize_host_mutation_v1',
  array['uuid', 'uuid', 'text', 'text']
);
select ok(
  not exists (
    select 1
    from pg_proc procedures
    join pg_namespace namespaces on namespaces.oid = procedures.pronamespace
    where namespaces.nspname = 'platform_api' and procedures.prosecdef
  ),
  'formation adds no security-definer API function'
);
select ok(
  has_function_privilege(
    'service_role',
    'platform_api.create_launch_request_v1(uuid,text,text,bytea,bytea,bytea,text,text,text,text,text)',
    'execute'
  )
  and not has_function_privilege(
    'anon',
    'platform_api.create_launch_request_v1(uuid,text,text,bytea,bytea,bytea,text,text,text,text,text)',
    'execute'
  )
  and not has_function_privilege(
    'authenticated',
    'platform_api.create_launch_request_v1(uuid,text,text,bytea,bytea,bytea,text,text,text,text,text)',
    'execute'
  ),
  'only the server role can execute launch creation'
);
select is(
  (select hard_limit from platform_store.capacity_gates where gate_kind = 'active_run'),
  10,
  'the migration owns one ten-Run capacity gate'
);
select throws_ok(
  $$update platform_store.capacity_gates set created_at = clock_timestamp()
    where gate_kind = 'active_run'$$,
  '23000',
  'immutable_formation_row',
  'the row-lock privilege cannot mutate the capacity gate'
);

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
  'blake3:1111111111111111111111111111111111111111111111111111111111111111',
  'worldstream.test.single-seat',
  convert_to('{}', 'utf8'),
  'blake3:2222222222222222222222222222222222222222222222222222222222222222',
  'sha256:3333333333333333333333333333333333333333333333333333333333333333',
  'test-web',
  'private',
  'must_claim_seat',
  'disabled',
  'disabled',
  'blake3:4444444444444444444444444444444444444444444444444444444444444444',
  'test/projection/v1',
  'blake3:5555555555555555555555555555555555555555555555555555555555555555',
  'test/result/v1',
  'blake3:6666666666666666666666666666666666666666666666666666666666666666',
  'worldstream/canonical-json/v1',
  1024,
  '{}'::jsonb,
  '[{"seat_id":"host","role":"host","display_name":"Host","required":true,"allowed_participation":["account_human"],"allowed_house_agent_revisions":[]}]'::jsonb,
  false,
  1800
);

insert into platform_store.platform_accounts(account_id)
values
  ('10000000-0000-4000-8000-000000000001'),
  ('10000000-0000-4000-8000-000000000002'),
  ('10000000-0000-4000-8000-000000000003'),
  ('10000000-0000-4000-8000-000000000004'),
  ('10000000-0000-4000-8000-000000000005');

set local role service_role;

create temporary table first_launch as
select * from platform_api.create_launch_request_v1(
  '10000000-0000-4000-8000-000000000001',
  'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
  'formation-test',
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
  (select was_created and launch_state = 'collecting_roster' from first_launch),
  'launch creation atomically creates one collecting request'
);
select is(
  (select first_launch.expires_at - launches.created_at
   from first_launch
   join platform_store.launch_requests launches using (launch_request_id)),
  interval '24 hours',
  'Launch Requests have an exact 24-hour lifetime'
);
select is(
  (select count(*)::integer
   from platform_store.capacity_reservations reservations
   join first_launch using (launch_request_id)
   where reservations.kind = 'pre_genesis' and reservations.released_at is null),
  1,
  'launch creation atomically acquires pre-Genesis capacity'
);
select ok(
  (select claims.seat_id = 'navigator'
     and claims.role = 'navigator'
     and claims.principal_reference = 'seat:navigator'
   from platform_store.seat_claims claims
   join first_launch using (launch_request_id)
   where claims.released_at is null),
  'the creator receives only the reviewed seat, Role, and server-derived Principal reference'
);

select ok(
  (select not was_created
   from platform_api.create_launch_request_v1(
     '10000000-0000-4000-8000-000000000001',
     'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
     'formation-test',
     decode(repeat('11', 32), 'hex'),
     convert_to('{}', 'utf8'),
     extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
     'worldstream/canonical-json/v1',
     'disabled',
     'seat',
     'navigator',
     'account_human'
   )),
  'an exact idempotent launch retry returns the retained request'
);
select throws_ok(
  $$select * from platform_api.create_launch_request_v1(
    '10000000-0000-4000-8000-000000000001',
    'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
    'formation-test', decode(repeat('11', 32), 'hex'), convert_to('{"changed":true}', 'utf8'),
    extensions.digest(convert_to('{"changed":true}', 'utf8'), 'sha256'),
    'worldstream/canonical-json/v1', 'disabled', 'seat', 'navigator', 'account_human')$$,
  '23505',
  'launch_idempotency_conflict',
  'a reused key with changed canonical input conflicts'
);
select throws_ok(
  $$select * from platform_api.create_launch_request_v1(
    '10000000-0000-4000-8000-000000000001',
    'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
    'another-key', decode(repeat('12', 32), 'hex'), convert_to('{}', 'utf8'),
    extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    'worldstream/canonical-json/v1', 'disabled', 'seat', 'navigator', 'account_human')$$,
  '55000',
  'pre_genesis_capacity_unavailable',
  'one account cannot own two pending launches'
);
select throws_ok(
  $$select * from platform_api.create_launch_request_v1(
    '10000000-0000-4000-8000-000000000002',
    'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
    'bad-choice', decode(repeat('13', 32), 'hex'), convert_to('{}', 'utf8'),
    extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    'worldstream/canonical-json/v1', 'disabled', 'spectator', null, 'account_human')$$,
  '22023',
  'creator_must_claim_seat',
  'the Listing prevents an unreviewed creator-spectator choice'
);
select throws_ok(
  $$select * from platform_api.create_launch_request_v1(
    '10000000-0000-4000-8000-000000000002',
    'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
    'bad-seat', decode(repeat('14', 32), 'hex'), convert_to('{}', 'utf8'),
    extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
    'worldstream/canonical-json/v1', 'disabled', 'seat', 'unlisted', 'account_human')$$,
  '22023',
  'creator_seat_not_allowed',
  'a caller cannot invent a seat or Role'
);

create temporary table first_view as
select platform_api.read_launch_request_v1(
  '10000000-0000-4000-8000-000000000001',
  (select launch_request_id from first_launch)
) as value;
select ok(
  (select value ->> 'version' = 'platform_launch_request.v1'
     and value ->> 'state' = 'collecting_roster'
     and value ? 'seats'
     and not (value ? 'creator_account_id')
     and not (value ? 'host_installation_id')
     and not (value ? 'room_setup_operation_id')
   from first_view),
  'the bounded launch DTO omits account and Host identities'
);
select is(
  platform_api.read_launch_request_v1(
    '10000000-0000-4000-8000-000000000003',
    (select launch_request_id from first_launch)
  ),
  null::jsonb,
  'an unrelated account cannot read a Launch Request'
);

create temporary table incomplete_roster as
select
  convert_to(jsonb_build_object(
    'schema', 'worldstream/frozen-roster/v1',
    'listing_revision_digest', 'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
    'members', jsonb_build_array(jsonb_build_object(
      'seat_id', 'navigator',
      'participation', 'account_human',
      'principal_reference', 'seat:navigator'
    ))
  )::text, 'utf8') as roster,
  convert_to(jsonb_build_object(
    'schema', 'worldstream/room-setup/v2',
    'pack', jsonb_build_object(
      'digest', 'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820'
    ),
    'seats', '[]'::jsonb,
    'spectators', jsonb_build_array(
      jsonb_build_object('purpose', 'result_indexer'),
      jsonb_build_object('purpose', 'public_relay')
    ),
    'operator_view', false
  )::text, 'utf8') as setup;
select throws_ok(
  format(
    $query$select platform_api.freeze_launch_request_v1(
      '10000000-0000-4000-8000-000000000001', %L, decode(%L, 'hex'),
      decode(%L, 'hex'), decode(%L, 'hex'),
      'blake3:%s', 'host-local', '01ARZ3NDEKTSV4RRFFQ69G5FB3')$query$,
    (select launch_request_id from first_launch),
    encode((select roster from incomplete_roster), 'hex'),
    encode(extensions.digest((select roster from incomplete_roster), 'sha256'), 'hex'),
    encode((select setup from incomplete_roster), 'hex'),
    repeat('a', 64)
  ),
  '55000',
  'required_seat_unfilled',
  'roster freeze rejects a missing required seat'
);

create temporary table first_invitation as
select * from platform_api.rotate_seat_invitation_v1(
  '10000000-0000-4000-8000-000000000001',
  (select launch_request_id from first_launch),
  'insider'
);
select ok(
  (select generation = 1
     and invitation_token ~ '^[0-9a-f]{64}$'
     and expires_at = (select expires_at from first_launch)
   from first_invitation),
  'the creator receives one 256-bit invitation token for the reviewed seat'
);
select ok(
  not exists (
    select 1
    from information_schema.columns
    where table_schema = 'platform_store'
      and table_name = 'seat_invitations'
      and column_name = 'invitation_token'
  ),
  'the raw invitation token has no storage column'
);

create temporary table rotated_invitation as
select * from platform_api.rotate_seat_invitation_v1(
  '10000000-0000-4000-8000-000000000001',
  (select launch_request_id from first_launch),
  'insider'
);
select ok(
  (select generation = 2 from rotated_invitation)
  and (
    select revoked_at is not null
    from platform_store.seat_invitations invitations
    join first_invitation original
      on invitations.launch_request_id = (select launch_request_id from first_launch)
     and invitations.seat_id = original.seat_id
     and invitations.generation = original.generation
  ),
  'rotation atomically revokes the earlier generation'
);
select throws_ok(
  format(
    $query$select * from platform_api.claim_invited_seat_v1(
      '10000000-0000-4000-8000-000000000002', decode(%L, 'hex'),
      'account_external_agent')$query$,
    encode(extensions.digest(convert_to((select invitation_token from first_invitation), 'utf8'), 'sha256'), 'hex')
  ),
  '55000',
  'invitation_unavailable',
  'a rotated invitation cannot claim a seat'
);

create temporary table insider_claim as
select * from platform_api.claim_invited_seat_v1(
  '10000000-0000-4000-8000-000000000002',
  extensions.digest(
    convert_to((select invitation_token from rotated_invitation), 'utf8'),
    'sha256'
  ),
  'account_external_agent'
);
select ok(
  (select seat_id = 'insider'
     and role = 'insider'
     and participation_kind = 'account_external_agent'
   from insider_claim),
  'an eligible authenticated account claims the exact invited seat once'
);
select throws_ok(
  format(
    $query$select * from platform_api.claim_invited_seat_v1(
      '10000000-0000-4000-8000-000000000003', decode(%L, 'hex'),
      'account_human')$query$,
    encode(extensions.digest(convert_to((select invitation_token from rotated_invitation), 'utf8'), 'sha256'), 'hex')
  ),
  '55000',
  'invitation_unavailable',
  'a consumed invitation cannot be replayed by another claimant'
);
select ok(
  (select (value -> 'seats' -> 1 ->> 'claimed_by_requester')::boolean
   from (
     select platform_api.read_launch_request_v1(
       '10000000-0000-4000-8000-000000000002',
       (select launch_request_id from first_launch)
     ) as value
   ) claimant_view),
  'a current claimant may read the bounded formation DTO'
);
select ok(
  platform_api.release_seat_claim_v1(
    '10000000-0000-4000-8000-000000000002',
    (select launch_request_id from first_launch),
    'insider'
  ),
  'a claimant may release their own unfrozen claim'
);
select is(
  platform_api.release_seat_claim_v1(
    '10000000-0000-4000-8000-000000000003',
    (select launch_request_id from first_launch),
    'navigator'
  ),
  false,
  'another account cannot release the creator claim'
);

create temporary table third_invitation as
select * from platform_api.rotate_seat_invitation_v1(
  '10000000-0000-4000-8000-000000000001',
  (select launch_request_id from first_launch),
  'insider'
);
select is(
  (select count(*)::integer from platform_api.claim_invited_seat_v1(
    '10000000-0000-4000-8000-000000000002',
    extensions.digest(convert_to((select invitation_token from third_invitation), 'utf8'), 'sha256'),
    'account_external_agent'
  )),
  1,
  'a released seat can be reclaimed through a fresh generation'
);
select ok(
  platform_api.reset_seat_claim_v1(
    '10000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    'insider'
  ),
  'the creator can reset an unfrozen claim'
);

create temporary table fourth_invitation as
select * from platform_api.rotate_seat_invitation_v1(
  '10000000-0000-4000-8000-000000000001',
  (select launch_request_id from first_launch),
  'insider'
);
select is(
  (select count(*)::integer from platform_api.claim_invited_seat_v1(
    '10000000-0000-4000-8000-000000000002',
    extensions.digest(convert_to((select invitation_token from fourth_invitation), 'utf8'), 'sha256'),
    'account_external_agent'
  )),
  1,
  'the reset seat has exactly one new winner'
);

create temporary table broker_invitation as
select * from platform_api.rotate_seat_invitation_v1(
  '10000000-0000-4000-8000-000000000001',
  (select launch_request_id from first_launch),
  'broker'
);
select throws_ok(
  format(
    $query$select * from platform_api.claim_invited_seat_v1(
      '10000000-0000-4000-8000-000000000002', decode(%L, 'hex'),
      'account_human')$query$,
    encode(extensions.digest(convert_to((select invitation_token from broker_invitation), 'utf8'), 'sha256'), 'hex')
  ),
  '55000',
  'account_seat_limit',
  'the default Listing permits one seat per account'
);

create temporary table complete_frozen_payload as
select
  convert_to(jsonb_build_object(
    'schema', 'worldstream/frozen-roster/v1',
    'listing_revision_digest', 'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
    'members', jsonb_build_array(
      jsonb_build_object(
        'seat_id', 'navigator',
        'participation', 'account_human',
        'principal_reference', 'seat:navigator'
      ),
      jsonb_build_object(
        'seat_id', 'insider',
        'participation', 'account_external_agent',
        'principal_reference', 'seat:insider'
      )
    )
  )::text, 'utf8') as roster,
  (select setup from incomplete_roster) as setup;
select ok(
  (select platform_api.freeze_launch_request_v1(
    '10000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    roster,
    extensions.digest(roster, 'sha256'),
    setup,
    'blake3:' || repeat('a', 64),
    'host-local',
    '01ARZ3NDEKTSV4RRFFQ69G5FB3'
  ) from complete_frozen_payload),
  'the complete exact roster and Room Setup Specification freeze once'
);
select ok(
  (select platform_api.freeze_launch_request_v1(
    '10000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    roster,
    extensions.digest(roster, 'sha256'),
    setup,
    'blake3:' || repeat('a', 64),
    'host-local',
    '01ARZ3NDEKTSV4RRFFQ69G5FB3'
  ) from complete_frozen_payload),
  'an exact freeze retry returns the retained frozen launch'
);
select throws_ok(
  format(
    $query$select platform_api.freeze_launch_request_v1(
      '10000000-0000-4000-8000-000000000001', %L, decode(%L, 'hex'),
      decode(%L, 'hex'), decode(%L, 'hex'),
      'blake3:%s', 'host-local', 'DIFFERENT')$query$,
    (select launch_request_id from first_launch),
    encode((select roster from complete_frozen_payload), 'hex'),
    encode(extensions.digest((select roster from complete_frozen_payload), 'sha256'), 'hex'),
    encode((select setup from complete_frozen_payload), 'hex'),
    repeat('a', 64)
  ),
  '23505',
  'launch_freeze_conflict',
  'a changed freeze retry conflicts'
);
select is(
  platform_api.release_seat_claim_v1(
    '10000000-0000-4000-8000-000000000002',
    (select launch_request_id from first_launch),
    'insider'
  ),
  false,
  'claims cannot be released after roster freeze'
);
select throws_ok(
  format(
    $query$update platform_store.seat_claims set released_at = clock_timestamp(),
      release_reason = 'self_released'
      where launch_request_id = %L and seat_id = 'insider' and released_at is null$query$,
    (select launch_request_id from first_launch)
  ),
  '23000',
  'launch_roster_frozen',
  'the table trigger also rejects post-freeze claim mutation'
);

select ok(
  platform_api.authorize_host_mutation_v1(
    '10000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    'host-local',
    '01ARZ3NDEKTSV4RRFFQ69G5FB3'
  ),
  'Host mutation authorization atomically acquires active-Run capacity'
);
select ok(
  platform_api.authorize_host_mutation_v1(
    '10000000-0000-4000-8000-000000000001',
    (select launch_request_id from first_launch),
    'host-local',
    '01ARZ3NDEKTSV4RRFFQ69G5FB3'
  ),
  'an exact Host authorization retry reuses the retained capacity reservation'
);
select is(
  (select count(*)::integer
   from platform_store.capacity_reservations reservations
   join first_launch using (launch_request_id)
   where reservations.released_at is null),
  2,
  'ambiguous pre-Genesis Host state retains both reservations'
);
select throws_ok(
  format(
    $query$select platform_api.authorize_host_mutation_v1(
      '10000000-0000-4000-8000-000000000001', %L,
      'host-local', 'DIFFERENT')$query$,
    (select launch_request_id from first_launch)
  ),
  '55000',
  'frozen_launch_mismatch',
  'a caller cannot retarget the frozen Host operation'
);
select throws_ok(
  format(
    $query$select platform_api.cancel_launch_request_v1(
      '10000000-0000-4000-8000-000000000001', %L)$query$,
    (select launch_request_id from first_launch)
  ),
  '55000',
  'launch_cannot_cancel',
  'an ambiguous Host mutation cannot release capacity through cancellation'
);

create temporary table cancelled_launch as
select * from platform_api.create_launch_request_v1(
  '10000000-0000-4000-8000-000000000004',
  'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
  'cancel-test',
  decode(repeat('21', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'navigator',
  'account_human'
);
select ok(
  platform_api.cancel_launch_request_v1(
    '10000000-0000-4000-8000-000000000004',
    (select launch_request_id from cancelled_launch)
  ),
  'the creator can cancel before the Host boundary'
);
select ok(
  (select launches.state = 'cancelled'
     and reservations.released_at is not null
     and reservations.release_reason = 'cancelled'
   from platform_store.launch_requests launches
   join platform_store.capacity_reservations reservations using (launch_request_id)
   where launches.launch_request_id = (select launch_request_id from cancelled_launch)
     and reservations.kind = 'pre_genesis'),
  'cancellation releases only its retained capacity with a typed reason'
);

reset role;
with sampled as (
  select clock_timestamp() - interval '25 hours' as created_at
)
insert into platform_store.launch_requests (
  launch_request_id,
  creator_account_id,
  listing_revision_digest,
  idempotency_namespace,
  idempotency_key_digest,
  canonical_launch_input,
  launch_input_digest,
  canonicalizer_version,
  house_fill_choice,
  creator_access_choice,
  creator_seat_id,
  creator_participation_kind,
  state,
  expires_at,
  created_at,
  last_transition_at
) select
  '20000000-0000-4000-8000-000000000001',
  '10000000-0000-4000-8000-000000000005',
  'blake3:1111111111111111111111111111111111111111111111111111111111111111',
  'expired-test',
  decode(repeat('31', 32), 'hex'),
  convert_to('{}', 'utf8'),
  extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
  'worldstream/canonical-json/v1',
  'disabled',
  'seat',
  'host',
  'account_human',
  'collecting_roster',
  sampled.created_at + interval '24 hours',
  sampled.created_at,
  sampled.created_at
from sampled;
insert into platform_store.capacity_reservations (
  launch_request_id,
  kind,
  controlling_account_id,
  reserved_at
) values (
  '20000000-0000-4000-8000-000000000001',
  'pre_genesis',
  '10000000-0000-4000-8000-000000000005',
  clock_timestamp() - interval '25 hours'
);
set local role service_role;
select ok(
  platform_api.expire_launch_request_v1('20000000-0000-4000-8000-000000000001'),
  'database time expires an eligible Launch Request'
);
select ok(
  (select launches.state = 'expired'
     and reservations.released_at is not null
     and reservations.release_reason = 'expired'
   from platform_store.launch_requests launches
   join platform_store.capacity_reservations reservations using (launch_request_id)
   where launches.launch_request_id = '20000000-0000-4000-8000-000000000001'),
  'expiry atomically releases pre-Genesis capacity'
);

reset role;
insert into platform_store.platform_accounts(account_id)
select ('30000000-0000-4000-8000-' || lpad(series::text, 12, '0'))::uuid
from generate_series(1, 10) series;
create temporary table global_capacity_candidate(launch_request_id uuid primary key);
grant select, insert on global_capacity_candidate to service_role;
set local role service_role;
do $$
declare
  series integer;
  account_id uuid;
  launch_id uuid;
  roster bytea;
  setup bytea;
begin
  for series in 1..10 loop
    account_id := ('30000000-0000-4000-8000-' || lpad(series::text, 12, '0'))::uuid;
    select created.launch_request_id
    into launch_id
    from platform_api.create_launch_request_v1(
      account_id,
      'blake3:1111111111111111111111111111111111111111111111111111111111111111',
      'gate-test',
      extensions.digest(convert_to('gate-' || series::text, 'utf8'), 'sha256'),
      convert_to('{}', 'utf8'),
      extensions.digest(convert_to('{}', 'utf8'), 'sha256'),
      'worldstream/canonical-json/v1',
      'disabled',
      'seat',
      'host',
      'account_human'
    ) created;
    roster := convert_to(jsonb_build_object(
      'schema', 'worldstream/frozen-roster/v1',
      'listing_revision_digest', 'blake3:1111111111111111111111111111111111111111111111111111111111111111',
      'members', jsonb_build_array(jsonb_build_object(
        'seat_id', 'host',
        'participation', 'account_human',
        'principal_reference', 'seat:host'
      ))
    )::text, 'utf8');
    setup := convert_to(jsonb_build_object(
      'schema', 'worldstream/room-setup/v2',
      'pack', jsonb_build_object(
        'digest', 'blake3:2222222222222222222222222222222222222222222222222222222222222222'
      ),
      'seats', '[]'::jsonb,
      'spectators', jsonb_build_array(jsonb_build_object('purpose', 'result_indexer')),
      'operator_view', false
    )::text, 'utf8');
    perform platform_api.freeze_launch_request_v1(
      account_id,
      launch_id,
      roster,
      extensions.digest(roster, 'sha256'),
      setup,
      'blake3:' || repeat('7', 64),
      'host-local',
      'gate-operation-' || series::text
    );
    if series <= 9 then
      perform platform_api.authorize_host_mutation_v1(
        account_id,
        launch_id,
        'host-local',
        'gate-operation-' || series::text
      );
    else
      insert into global_capacity_candidate values (launch_id);
    end if;
  end loop;
end;
$$;
select is(
  (select count(*)::integer
   from platform_store.capacity_reservations
   where kind = 'active_run' and released_at is null),
  10,
  'the fixed lock gate admits exactly ten active Run reservations globally'
);
select throws_ok(
  format(
    $query$select platform_api.authorize_host_mutation_v1(
      '30000000-0000-4000-8000-000000000010', %L,
      'host-local', 'gate-operation-10')$query$,
    (select launch_request_id from global_capacity_candidate)
  ),
  '55000',
  'global_active_run_capacity_unavailable',
  'the eleventh active Run reservation fails closed'
);
select is(
  (select count(*)::integer
   from platform_store.capacity_reservations
   where kind = 'active_run' and released_at is null),
  10,
  'capacity rejection does not create a partial reservation'
);

select * from finish();
rollback;
