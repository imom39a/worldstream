begin;
create extension if not exists pgtap with schema extensions;
set local search_path = public, extensions;
select plan(5);

select is(
  (select encode(extensions.digest(canonical_document, 'sha256'), 'hex')
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b'),
  'c2d4739c2232b261d5710c6be67b57040a9aef00b38b68f52ae319e207a1ba63',
  'internal candidate stores exact canonical reviewed Listing bytes'
);
select is(
  (select catalog_visibility || ':' || public_viewing_policy || ':' || result_publication_policy
   from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b'),
  'unlisted:disabled:disabled', 'registration enables neither public discovery nor viewing nor results'
);
select is(
  (select room_setup_configuration from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b'),
  '{"scenario_id":"standard-v1"}'::jsonb, 'the initial solo candidate freezes Standard'
);
select is(
  (select jsonb_array_length(seat_templates) from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b'),
  1, 'solo launch declares only one seat'
);
select is(
  (select client_release_digest from platform_store.activity_listing_revisions
   where listing_revision_digest = 'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b'),
  'sha256:9be6d6548f2d62baba805ec461b27fbd1021ed0d61fbd0e8ffd1d779616bc534',
  'the internal candidate cannot silently select a different client release'
);

select * from finish();
rollback;
