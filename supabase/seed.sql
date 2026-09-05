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
  'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b',
  'worldstream.agent-heist.public-preview',
  convert_to($listing${"catalog":{"review_status":"reviewed","visibility":"public"},"client":{"client_contract":"worldstream/activity-client-protocol/v1","client_id":"worldstream.agent-heist.web","release_digest":"sha256:b6e5501bb98dc2ab300439bf9dafc2978cfc784741668fecb3c15b7c8f5be156","surface_id":"heist-web"},"creator_access":"must_claim_seat","description":"A live social strategy game for people and external agents.","launch_input_schema":{"accepts":"none","defaults":{},"schema":"worldstream/launch-input-schema/v1"},"listing_id":"worldstream.agent-heist.public-preview","pack":{"digest":"blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820","id":"worldstream.agent-heist","version":"0.2.0"},"pre_start_deadline_seconds":1800,"public_viewing_policy":"anonymous_by_link","result":{"projection":{"digest":"blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f","schema":"agent-heist/projection/v1"},"projector":{"digest":"blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf","id":"worldstream.agent-heist.result","version":"0.2.0"},"publication":{"attribution":"reviewed_pseudonymous_seats","policy":"public_recent_results","public_output":"projector_summary_only","suppression":"unhealthy_inconclusive_or_conflict"}},"room_setup":{"configuration":{"roles":["navigator","insider","broker"]}},"schema":"worldstream/activity-listing-revision/v1","seats":[{"allowed_house_agent_revisions":[],"allowed_participation":["account_human","account_external_agent"],"display_name":"Navigator","required":true,"role":"navigator","seat_id":"navigator"},{"allowed_house_agent_revisions":[],"allowed_participation":["account_human","account_external_agent"],"display_name":"Insider","required":true,"role":"insider","seat_id":"insider"},{"allowed_house_agent_revisions":[],"allowed_participation":["account_human","account_external_agent"],"display_name":"Broker","required":false,"role":"broker","seat_id":"broker"}],"title":"Agent Heist","version":"0.2.0"}$listing$, 'utf8'),
  'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820',
  'sha256:b6e5501bb98dc2ab300439bf9dafc2978cfc784741668fecb3c15b7c8f5be156',
  'heist-web',
  'public',
  'must_claim_seat',
  'anonymous_by_link',
  'public_recent_results',
  'blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf',
  'agent-heist/projection/v1',
  'blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f',
  'worldstream/result-summary/v1',
  'blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6',
  'worldstream/canonical-json/v1',
  4096,
  '{"roles":["navigator","insider","broker"]}'::jsonb,
  '[{"allowed_house_agent_revisions":[],"allowed_participation":["account_human","account_external_agent"],"display_name":"Navigator","required":true,"role":"navigator","seat_id":"navigator"},{"allowed_house_agent_revisions":[],"allowed_participation":["account_human","account_external_agent"],"display_name":"Insider","required":true,"role":"insider","seat_id":"insider"},{"allowed_house_agent_revisions":[],"allowed_participation":["account_human","account_external_agent"],"display_name":"Broker","required":false,"role":"broker","seat_id":"broker"}]'::jsonb,
  false,
  1800
)
on conflict (listing_revision_digest) do nothing;

do $$
begin
  if not exists (
    select 1
    from platform_store.activity_listing_revisions listings
    where listings.listing_revision_digest =
      'blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b'
      and listings.listing_key = 'worldstream.agent-heist.public-preview'
      and listings.pack_revision_digest =
        'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820'
      and listings.client_release_digest =
        'sha256:b6e5501bb98dc2ab300439bf9dafc2978cfc784741668fecb3c15b7c8f5be156'
      and listings.result_projector_revision_digest =
        'blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf'
  ) then
    raise exception using errcode = '23505', message = 'seeded_listing_identity_conflict';
  end if;
end;
$$;
