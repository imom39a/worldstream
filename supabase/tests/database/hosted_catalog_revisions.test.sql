begin;
create extension if not exists pgtap with schema extensions;
set local search_path = public, extensions;
select plan(23);

select is(
  (select encode(extensions.digest(canonical_document, 'sha256'), 'hex')
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:3cdaaa7b2402b816ded0b36d5419f405b1be1428b37c89155a805d39bf826069'),
  '25994354df316bf56b85a57da9ad6b4ee96510bab3fd16945a8616fbaced7c00',
  'the r13 successor Listing stores the checked canonical artifact bytes'
);
select is(
  (select convert_from(canonical_document, 'utf8')::jsonb ->> 'version'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:3cdaaa7b2402b816ded0b36d5419f405b1be1428b37c89155a805d39bf826069'),
  '0.21.0',
  'the r13 successor Listing is the current immutable revision'
);
select is(
  (select convert_from(canonical_document, 'utf8')::jsonb ->> 'version'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:1cf75abcb30d77fdbe0abc5e39813a315bea6900c61e9b49c51b84d995335d74'),
  '0.20.0',
  'the r12 Listing remains retained after current discovery advances'
);
select is(
  (select convert_from(canonical_document, 'utf8')::jsonb ->> 'version'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:5b0993de4c858771cce34b16cb25e03b2bf509cbe16cd1ce7249a789ea8c426f'),
  '0.18.0',
  'the r10 Listing remains retained after current discovery advances'
);
select is(
  (select convert_from(canonical_document, 'utf8')::jsonb ->> 'version'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:350beff2dbb28d495a5355ac19a7580f8494589bc0d5c6fec1c521be86a7cf38'),
  '0.17.0',
  'the r9 Listing remains retained after current discovery advances'
);
select is(
  (select string_agg(runner_template_revision, ',' order by house_agent_key)
   from platform_store.house_agent_revisions
   where house_agent_revision_digest in (
     'blake3:f61c494b644621e55eacf32dafbbf61f9471160eb2b91181e21c03ef5ed82271',
     'blake3:c794a69db2c617724483aaa86e0b55f1d694936c4735932b75c2ae5cacdcfecd'
   )),
  '9,9',
  'the r9 House revisions remain immutable for retained Assignments'
);

select is(
  (select count(*)::integer from platform_store.activity_listing_revisions
   where listing_revision_digest in (
     'blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1',
     'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956',
     'blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1'
     , 'blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80'
   )), 4, 'all immutable hosted Listing revisions remain available'
);
select is(
  (select convert_from(canonical_document, 'utf8')::jsonb ->> 'version'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1'),
  '0.5.0', 'the retained design Listing keeps its original version'
);
select is(
  (select client_release_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1'),
  'sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9',
  'current Listing pins the game-themed client'
);
select is(
  (select client_surface_id from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1'),
  'heist-hosted-web', 'hosted Listing selects only the authenticated hosted entrypoint'
);
select is(
  (select client_release_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956'),
  'sha256:88f76571d3d5736f0ca52740d619761f9ad6dcb9112792e806d8072c2a983148',
  'retained Listing still pins its original client'
);
select throws_ok(
  $$update platform_store.activity_listing_revisions
    set client_release_digest = 'sha256:88f76571d3d5736f0ca52740d619761f9ad6dcb9112792e806d8072c2a983148'
    where listing_revision_digest = 'blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1'$$,
  '23000', 'immutable_formation_row', 'current Listing cannot silently rebind its client'
);

select is(
  (select client_release_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:04edc964d5cbc1bc5efa422ac856305d55cec609a6ae5c5c1814c8389b776f80'),
  'sha256:493260d162be2bdfda28e71b6e4f94d5ef5c11e7f03adfbee3ab9295bdf4594b',
  'the previous design retains its exact client release'
);

select is(
  (select convert_from(canonical_document, 'utf8')::jsonb ->> 'version'
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782'),
  '0.6.0', 'the Granite successor is a separate immutable Listing'
);
select is(
  (select client_release_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782'),
  'sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9',
  'the House successor retains the exact existing v3 client'
);
select is(
  (select agent_profile_revision from platform_store.house_agent_revisions
   where house_agent_revision_digest = 'blake3:134c19dbbd0b80bf2af98d095c8feca5026a00137c1077c16ccea5de95100ce4'),
  '2', 'the new Planner uses a new exact Host profile revision'
);
select is(
  (select model_slug from platform_store.house_agent_revisions
   where house_agent_revision_digest = 'blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81'),
  'qwen/qwen3.8-flash-20260826', 'the retained Planner route is not rewritten'
);
select ok(
  (select count(*) = 2
      and count(distinct revisions.behavior_policy_id) = 2
      and bool_and(revisions.model_slug = 'ibm-granite/granite-4.2-8b-20260831')
      and bool_and(revisions.provider_slug = 'deepinfra/bf16')
   from platform_store.house_agent_revisions revisions
   where revisions.house_agent_revision_digest in (
     select jsonb_array_elements_text(seats.value -> 'allowed_house_agent_revisions')
     from platform_store.activity_listing_revisions listings,
          jsonb_array_elements(listings.seat_templates) seats(value)
     where listings.listing_revision_digest = 'blake3:48f76e8c1336e8f50fb6952cd0f2ff4c47cc8c372594db01422a67bca9363782'
   )),
  'the current Listing allows two distinct strategies on the exact Granite route'
);

select is(
  (select pack_revision_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630'),
  'blake3:4e4c970403f29a8448a1a3bcf7a96c030df713499730288f324c7e200d160b2d',
  'clock-safe Listing pins the new exact Pack'
);
select is(
  (select client_release_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630'),
  'sha256:12714052c8e1cac59a95b0439e8e86bf17766c8f689f5782d1dd5d4c0efd4cd6',
  'clock-safe Listing pins the separate v4 client'
);
select is(
  (select result_projector_revision_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630'),
  'blake3:f676cab8007a374db5510d66f534697472c53ea4b2719900bfecce53786f7563',
  'clock-safe results use their own exact projector'
);
select ok(
  (select count(*) = 2 and bool_and(runner_template_revision = '2')
      and bool_and((execution_allowance ->> 'model_call_attempts')::integer = 10)
      and bool_and(model_slug = 'ibm-granite/granite-4.2-8b-20260831')
   from platform_store.house_agent_revisions
   where house_agent_revision_digest in (
     'blake3:b6f273c3da48b9636bf0107e1768e46cc7ab3d52618e0100791c724baff774ef',
     'blake3:0ecf354d30e0034ec8e43cd70f99484eca86403a5eef82079552a4e905ba89ca'
   )), 'clock-safe House definitions change compatibility, not model or call budget'
);
select throws_ok(
  $$update platform_store.activity_listing_revisions
    set pack_revision_digest = 'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820'
    where listing_revision_digest = 'blake3:48c397a32632896d66beb9ae7f8a6d090338800187c56eb0593997b80bd2b630'$$,
  '23000', 'immutable_formation_row', 'clock-safe Listing cannot silently change Pack rules'
);

select * from finish();
rollback;
