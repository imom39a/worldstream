begin;
create extension if not exists pgtap with schema extensions;
set local search_path = public, extensions;
select plan(12);

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

select * from finish();
rollback;
