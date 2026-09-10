-- Internal candidate registration only: no public discovery, Host approval or deployment.
do $listing_revision$
declare expected_document bytea; expected_value jsonb;
begin
  expected_document := convert_to($artifact${"catalog":{"review_status":"reviewed","visibility":"unlisted"},"client":{"client_contract":"worldstream/activity-client-protocol/v1","client_id":"worldstream.midnight-archive.web","release_digest":"sha256:9be6d6548f2d62baba805ec461b27fbd1021ed0d61fbd0e8ffd1d779616bc534","surface_id":"midnight-archive-hosted-web"},"creator_access":"must_claim_seat","description":"Identify the authentic ledger, gain access to the vault, and escape through the Atrium. Solo Standard expedition: 16 turns and 3 power charges; reading costs no turns.","launch_input_schema":{"accepts":"none","defaults":{},"schema":"worldstream/launch-input-schema/v1"},"listing_id":"worldstream.midnight-archive.internal-solo","pack":{"digest":"blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64","id":"worldstream.midnight-archive","version":"0.1.0"},"pre_start_deadline_seconds":1800,"public_viewing_policy":"disabled","result":{"projection":{"digest":"blake3:d6042c76a9f09bc4bc1ea3fac8c443bc6f7f154f39aed4cd7aad5b60b956949f","schema":"worldstream.midnight-archive/public-projection/v3"},"projector":{"digest":"blake3:099bb9dc1b0e1fa76ebfe3d67d8b1c0e239e53ec72c88b65e0892bf79f268821","id":"worldstream.midnight-archive.result","version":"0.1.0"},"publication":{"attribution":"none","policy":"disabled","public_output":"none","suppression":"unhealthy_inconclusive_or_conflict"}},"room_setup":{"configuration":{"scenario_id":"standard-v1"}},"schema":"worldstream/activity-listing-revision/v1","seats":[{"allowed_house_agent_revisions":[],"allowed_participation":["account_human"],"display_name":"Expedition lead","required":true,"role":"lead","seat_id":"lead"}],"title":"Midnight Archive","version":"0.1.0"}$artifact$, 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.activity_listing_revisions (
    listing_revision_digest, listing_key, canonical_document, pack_revision_digest,
    client_release_digest, client_surface_id, catalog_visibility, creator_access,
    public_viewing_policy, result_publication_policy, result_projector_revision_digest,
    public_projection_schema, public_projection_schema_digest, result_output_schema,
    result_output_schema_digest, result_canonicalizer_version, result_output_max_bytes,
    room_setup_configuration, seat_templates, allow_multiple_seats_per_account, pre_start_deadline_seconds
  ) values (
    'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b', expected_value ->> 'listing_id', expected_document,
    expected_value #>> '{pack,digest}', expected_value #>> '{client,release_digest}',
    expected_value #>> '{client,surface_id}', expected_value #>> '{catalog,visibility}',
    expected_value ->> 'creator_access', expected_value ->> 'public_viewing_policy',
    expected_value #>> '{result,publication,policy}', expected_value #>> '{result,projector,digest}',
    expected_value #>> '{result,projection,schema}', expected_value #>> '{result,projection,digest}',
    'midnight-archive/terminal-summary/v1', 'blake3:dc4772ec95b72de7fe6d937a51844d9d1ff48cd01c01a2227408ef474b8d792e',
    'worldstream/canonical-json/v1', 1024, expected_value #> '{room_setup,configuration}',
    expected_value -> 'seats', false, 1800
  ) on conflict (listing_revision_digest) do nothing;
  if not exists (
    select 1 from platform_store.activity_listing_revisions
    where listing_revision_digest = 'blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b' and canonical_document = expected_document
  ) then raise exception using errcode = '23505', message = 'internal_listing_identity_conflict';
  end if;
end;
$listing_revision$;
