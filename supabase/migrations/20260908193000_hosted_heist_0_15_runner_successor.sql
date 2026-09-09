-- Append-only correction: current 0.3 Heist formation needs its own exact
-- House Runner and Agent revisions. Earlier artifacts remain addressable.

do $house_revision$
declare
  source_document text;
  expected_document bytea;
  expected_value jsonb;
begin
  select convert_from(canonical_document, 'utf8') into strict source_document
  from platform_store.house_agent_revisions
  where house_agent_revision_digest = 'blake3:1665aa7c527861012829c237b1fdab760cbce918036a128d10ba06b7fa4cd098';
  expected_document := convert_to(
    replace(replace(replace(source_document,
      '"agent_profile":{"profile_id":"house-cooperative-planner","revision":"8"}',
      '"agent_profile":{"profile_id":"house-cooperative-planner","revision":"9"}'),
      '"runner_template":{"revision":"7","template_id":"openrouter-house"}',
      '"runner_template":{"revision":"8","template_id":"openrouter-house"}'),
      '"version":"8"}', '"version":"9"}'), 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.house_agent_revisions (house_agent_revision_digest, house_agent_key, display_name, canonical_document, model_slug, provider_slug, behavior_policy_id, behavior_policy_revision, agent_profile_id, agent_profile_revision, runner_template_id, runner_template_revision, accounting_tokenizer_id, accounting_tokenizer_revision, execution_allowance, tool_set)
  values ('blake3:1a5edc340f6a2e7d66db5710d069dcf3c849028147f00fa2d99db2cd30ca9399', expected_value ->> 'house_agent_id', expected_value ->> 'display_name', expected_document, expected_value #>> '{route,model_slug}', expected_value #>> '{route,provider_slug}', expected_value #>> '{behavior_policy,policy_id}', expected_value #>> '{behavior_policy,revision}', expected_value #>> '{agent_profile,profile_id}', expected_value #>> '{agent_profile,revision}', expected_value #>> '{runner_template,template_id}', expected_value #>> '{runner_template,revision}', expected_value #>> '{accounting_tokenizer,tokenizer_id}', expected_value #>> '{accounting_tokenizer,revision}', expected_value -> 'allowance', expected_value -> 'tools')
  on conflict (house_agent_revision_digest) do nothing;
  if not exists (select 1 from platform_store.house_agent_revisions retained where retained.house_agent_revision_digest = 'blake3:1a5edc340f6a2e7d66db5710d069dcf3c849028147f00fa2d99db2cd30ca9399' and retained.canonical_document = expected_document) then
    raise exception using errcode = '23505', message = 'hosted_house_revision_identity_conflict';
  end if;
end;
$house_revision$;

do $house_revision$
declare
  source_document text;
  expected_document bytea;
  expected_value jsonb;
begin
  select convert_from(canonical_document, 'utf8') into strict source_document
  from platform_store.house_agent_revisions
  where house_agent_revision_digest = 'blake3:5a826962c0f09c40a1b760d2c0eec9af216b9eb703b2e6f971fc24e32f0564d1';
  expected_document := convert_to(
    replace(replace(replace(source_document,
      '"agent_profile":{"profile_id":"house-skeptical-auditor","revision":"7"}',
      '"agent_profile":{"profile_id":"house-skeptical-auditor","revision":"8"}'),
      '"runner_template":{"revision":"7","template_id":"openrouter-house"}',
      '"runner_template":{"revision":"8","template_id":"openrouter-house"}'),
      '"version":"7"}', '"version":"8"}'), 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.house_agent_revisions (house_agent_revision_digest, house_agent_key, display_name, canonical_document, model_slug, provider_slug, behavior_policy_id, behavior_policy_revision, agent_profile_id, agent_profile_revision, runner_template_id, runner_template_revision, accounting_tokenizer_id, accounting_tokenizer_revision, execution_allowance, tool_set)
  values ('blake3:6146f54874747d6d4b16e06bca7872696f09c16dfe3b3177e5ab0c6c06034155', expected_value ->> 'house_agent_id', expected_value ->> 'display_name', expected_document, expected_value #>> '{route,model_slug}', expected_value #>> '{route,provider_slug}', expected_value #>> '{behavior_policy,policy_id}', expected_value #>> '{behavior_policy,revision}', expected_value #>> '{agent_profile,profile_id}', expected_value #>> '{agent_profile,revision}', expected_value #>> '{runner_template,template_id}', expected_value #>> '{runner_template,revision}', expected_value #>> '{accounting_tokenizer,tokenizer_id}', expected_value #>> '{accounting_tokenizer,revision}', expected_value -> 'allowance', expected_value -> 'tools')
  on conflict (house_agent_revision_digest) do nothing;
  if not exists (select 1 from platform_store.house_agent_revisions retained where retained.house_agent_revision_digest = 'blake3:6146f54874747d6d4b16e06bca7872696f09c16dfe3b3177e5ab0c6c06034155' and retained.canonical_document = expected_document) then
    raise exception using errcode = '23505', message = 'hosted_house_revision_identity_conflict';
  end if;
end;
$house_revision$;

do $listing_revision$
declare
  source_document text;
  expected_document bytea;
  expected_value jsonb;
begin
  select convert_from(canonical_document, 'utf8') into strict source_document
  from platform_store.activity_listing_revisions
  where listing_revision_digest = 'blake3:735865c4246282c89d37858c9ee9de99a05ae4557fdef5dca5ea365f90f031b0';
  expected_document := convert_to(
    replace(replace(replace(source_document,
      'blake3:1665aa7c527861012829c237b1fdab760cbce918036a128d10ba06b7fa4cd098',
      'blake3:1a5edc340f6a2e7d66db5710d069dcf3c849028147f00fa2d99db2cd30ca9399'),
      'blake3:5a826962c0f09c40a1b760d2c0eec9af216b9eb703b2e6f971fc24e32f0564d1',
      'blake3:6146f54874747d6d4b16e06bca7872696f09c16dfe3b3177e5ab0c6c06034155'),
      '"version":"0.14.0"}', '"version":"0.15.0"}'), 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.activity_listing_revisions (listing_revision_digest, listing_key, canonical_document, pack_revision_digest, client_release_digest, client_surface_id, catalog_visibility, creator_access, public_viewing_policy, result_publication_policy, result_projector_revision_digest, public_projection_schema, public_projection_schema_digest, result_output_schema, result_output_schema_digest, result_canonicalizer_version, result_output_max_bytes, room_setup_configuration, seat_templates, allow_multiple_seats_per_account, pre_start_deadline_seconds)
  values ('blake3:79400919b61a051f4de82824a9a7f6414be609609802ece11ca1137fc8fbdf65', 'worldstream.agent-heist.public-preview', expected_document, expected_value #>> '{pack,digest}', expected_value #>> '{client,release_digest}', expected_value #>> '{client,surface_id}', expected_value #>> '{catalog,visibility}', expected_value ->> 'creator_access', expected_value ->> 'public_viewing_policy', expected_value #>> '{result,publication,policy}', expected_value #>> '{result,projector,digest}', expected_value #>> '{result,projection,schema}', expected_value #>> '{result,projection,digest}', 'worldstream/result-summary/v1', 'blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6', 'worldstream/canonical-json/v1', 4096, expected_value #> '{room_setup,configuration}', expected_value -> 'seats', false, 1800)
  on conflict (listing_revision_digest) do nothing;
  if not exists (select 1 from platform_store.activity_listing_revisions retained where retained.listing_revision_digest = 'blake3:79400919b61a051f4de82824a9a7f6414be609609802ece11ca1137fc8fbdf65' and retained.canonical_document = expected_document) then
    raise exception using errcode = '23505', message = 'hosted_heist_0_15_listing_identity_conflict';
  end if;
end;
$listing_revision$;
