-- Add the game-themed Heist client as a new immutable Listing.
-- Earlier Listing and Client Release identities remain unchanged.
do $revision$
declare
  expected_document constant bytea := convert_to($listing${"catalog":{"review_status":"reviewed","visibility":"public"},"client":{"client_contract":"worldstream/activity-client-protocol/v1","client_id":"worldstream.agent-heist.web","release_digest":"sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9","surface_id":"heist-hosted-web"},"creator_access":"must_claim_seat","description":"A live social strategy game for people, external agents, and bounded platform-supplied House Agents.","launch_input_schema":{"accepts":"none","defaults":{},"schema":"worldstream/launch-input-schema/v1"},"listing_id":"worldstream.agent-heist.public-preview","pack":{"digest":"blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820","id":"worldstream.agent-heist","version":"0.2.0"},"pre_start_deadline_seconds":1800,"public_viewing_policy":"anonymous_by_link","result":{"projection":{"digest":"blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f","schema":"agent-heist/projection/v1"},"projector":{"digest":"blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf","id":"worldstream.agent-heist.result","version":"0.2.0"},"publication":{"attribution":"reviewed_pseudonymous_seats","policy":"public_recent_results","public_output":"projector_summary_only","suppression":"unhealthy_inconclusive_or_conflict"}},"room_setup":{"configuration":{"roles":["navigator","insider","broker"]}},"schema":"worldstream/activity-listing-revision/v1","seats":[{"allowed_house_agent_revisions":["blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81","blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"],"allowed_participation":["account_human","account_external_agent","house_agent_fill"],"display_name":"Navigator","required":true,"role":"navigator","seat_id":"navigator"},{"allowed_house_agent_revisions":["blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81","blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"],"allowed_participation":["account_human","account_external_agent","house_agent_fill"],"display_name":"Insider","required":true,"role":"insider","seat_id":"insider"},{"allowed_house_agent_revisions":["blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81","blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a"],"allowed_participation":["account_human","account_external_agent","house_agent_fill"],"display_name":"Broker","required":false,"role":"broker","seat_id":"broker"}],"title":"Agent Heist","version":"0.5.0"}$listing$, 'utf8');
  expected_value constant jsonb := convert_from(expected_document, 'utf8')::jsonb;
begin
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
    'blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1',
    'worldstream.agent-heist.house-preview',
    expected_document,
    'blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820',
    'sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9',
    'heist-hosted-web',
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
    expected_value #> '{room_setup,configuration}',
    expected_value -> 'seats',
    false,
    1800
  ) on conflict (listing_revision_digest) do nothing;

  if not exists (
    select 1 from platform_store.activity_listing_revisions retained
    where retained.listing_revision_digest is not distinct from ('blake3:9553f4fa320aa6901d0a03870f5c19ce4342d271efd2ef90d4fd287395f6cef1')
      and retained.listing_key is not distinct from ('worldstream.agent-heist.house-preview')
      and retained.canonical_document is not distinct from (expected_document)
      and retained.pack_revision_digest is not distinct from ('blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820')
      and retained.client_release_digest is not distinct from ('sha256:8075408d0420d3eec514d01c428a2b8b17559c388e57a7067c05cc9f95a3b5d9')
      and retained.client_surface_id is not distinct from ('heist-hosted-web')
      and retained.catalog_visibility is not distinct from ('public')
      and retained.creator_access is not distinct from ('must_claim_seat')
      and retained.public_viewing_policy is not distinct from ('anonymous_by_link')
      and retained.result_publication_policy is not distinct from ('public_recent_results')
      and retained.result_projector_revision_digest is not distinct from ('blake3:421f83957b54e5a8b4be10084e6900cfd977b938c79f7edb6dacbefe8cff4ddf')
      and retained.public_projection_schema is not distinct from ('agent-heist/projection/v1')
      and retained.public_projection_schema_digest is not distinct from ('blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f')
      and retained.result_output_schema is not distinct from ('worldstream/result-summary/v1')
      and retained.result_output_schema_digest is not distinct from ('blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6')
      and retained.result_canonicalizer_version is not distinct from ('worldstream/canonical-json/v1')
      and retained.result_output_max_bytes is not distinct from (4096)
      and retained.room_setup_configuration is not distinct from (expected_value #> '{room_setup,configuration}')
      and retained.seat_templates is not distinct from (expected_value -> 'seats')
      and retained.allow_multiple_seats_per_account is not distinct from (false)
      and retained.pre_start_deadline_seconds is not distinct from (1800)
  ) then
    raise exception using errcode = '23505', message = 'hosted_heist_listing_identity_conflict';
  end if;
end;
$revision$;
