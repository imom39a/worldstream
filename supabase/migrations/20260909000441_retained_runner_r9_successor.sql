-- Immutable r9 Runner successor. This only records reviewed metadata; it
-- grants no House approval or execution authority. r7/r8 remain retained
-- historical identities and cannot be rewritten to repair their image paths.

do $house_revision$
declare source_document text; expected_document bytea; expected_value jsonb;
begin
  select convert_from(canonical_document, 'utf8') into strict source_document
  from platform_store.house_agent_revisions
  where house_agent_revision_digest = 'blake3:1a5edc340f6a2e7d66db5710d069dcf3c849028147f00fa2d99db2cd30ca9399';
  expected_document := convert_to(replace(replace(replace(source_document,
    '"agent_profile":{"profile_id":"house-cooperative-planner","revision":"9"}',
    '"agent_profile":{"profile_id":"house-cooperative-planner","revision":"10"}'),
    '"runner_template":{"revision":"8","template_id":"openrouter-house"}',
    '"runner_template":{"revision":"9","template_id":"openrouter-house"}'),
    '"version":"9"}', '"version":"10"}'), 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.house_agent_revisions (house_agent_revision_digest, house_agent_key, display_name, canonical_document, model_slug, provider_slug, behavior_policy_id, behavior_policy_revision, agent_profile_id, agent_profile_revision, runner_template_id, runner_template_revision, accounting_tokenizer_id, accounting_tokenizer_revision, execution_allowance, tool_set)
  values ('blake3:f61c494b644621e55eacf32dafbbf61f9471160eb2b91181e21c03ef5ed82271', expected_value ->> 'house_agent_id', expected_value ->> 'display_name', expected_document, expected_value #>> '{route,model_slug}', expected_value #>> '{route,provider_slug}', expected_value #>> '{behavior_policy,policy_id}', expected_value #>> '{behavior_policy,revision}', expected_value #>> '{agent_profile,profile_id}', expected_value #>> '{agent_profile,revision}', expected_value #>> '{runner_template,template_id}', expected_value #>> '{runner_template,revision}', expected_value #>> '{accounting_tokenizer,tokenizer_id}', expected_value #>> '{accounting_tokenizer,revision}', expected_value -> 'allowance', expected_value -> 'tools')
  on conflict (house_agent_revision_digest) do nothing;
  if not exists (select 1 from platform_store.house_agent_revisions where house_agent_revision_digest = 'blake3:f61c494b644621e55eacf32dafbbf61f9471160eb2b91181e21c03ef5ed82271' and canonical_document = expected_document) then
    raise exception using errcode = '23505', message = 'retained_runner_r9_identity_conflict';
  end if;
end;
$house_revision$;

do $house_revision$
declare source_document text; expected_document bytea; expected_value jsonb;
begin
  select convert_from(canonical_document, 'utf8') into strict source_document
  from platform_store.house_agent_revisions
  where house_agent_revision_digest = 'blake3:6146f54874747d6d4b16e06bca7872696f09c16dfe3b3177e5ab0c6c06034155';
  expected_document := convert_to(replace(replace(replace(source_document,
    '"agent_profile":{"profile_id":"house-skeptical-auditor","revision":"8"}',
    '"agent_profile":{"profile_id":"house-skeptical-auditor","revision":"9"}'),
    '"runner_template":{"revision":"8","template_id":"openrouter-house"}',
    '"runner_template":{"revision":"9","template_id":"openrouter-house"}'),
    '"version":"8"}', '"version":"9"}'), 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.house_agent_revisions (house_agent_revision_digest, house_agent_key, display_name, canonical_document, model_slug, provider_slug, behavior_policy_id, behavior_policy_revision, agent_profile_id, agent_profile_revision, runner_template_id, runner_template_revision, accounting_tokenizer_id, accounting_tokenizer_revision, execution_allowance, tool_set)
  values ('blake3:c794a69db2c617724483aaa86e0b55f1d694936c4735932b75c2ae5cacdcfecd', expected_value ->> 'house_agent_id', expected_value ->> 'display_name', expected_document, expected_value #>> '{route,model_slug}', expected_value #>> '{route,provider_slug}', expected_value #>> '{behavior_policy,policy_id}', expected_value #>> '{behavior_policy,revision}', expected_value #>> '{agent_profile,profile_id}', expected_value #>> '{agent_profile,revision}', expected_value #>> '{runner_template,template_id}', expected_value #>> '{runner_template,revision}', expected_value #>> '{accounting_tokenizer,tokenizer_id}', expected_value #>> '{accounting_tokenizer,revision}', expected_value -> 'allowance', expected_value -> 'tools')
  on conflict (house_agent_revision_digest) do nothing;
  if not exists (select 1 from platform_store.house_agent_revisions where house_agent_revision_digest = 'blake3:c794a69db2c617724483aaa86e0b55f1d694936c4735932b75c2ae5cacdcfecd' and canonical_document = expected_document) then
    raise exception using errcode = '23505', message = 'retained_runner_r9_identity_conflict';
  end if;
end;
$house_revision$;

do $listing_revision$
declare source_document text; expected_document bytea; expected_value jsonb;
begin
  select convert_from(canonical_document, 'utf8') into strict source_document
  from platform_store.activity_listing_revisions
  where listing_revision_digest = 'blake3:79400919b61a051f4de82824a9a7f6414be609609802ece11ca1137fc8fbdf65';
  expected_document := convert_to(replace(replace(replace(source_document,
    'blake3:1a5edc340f6a2e7d66db5710d069dcf3c849028147f00fa2d99db2cd30ca9399',
    'blake3:f61c494b644621e55eacf32dafbbf61f9471160eb2b91181e21c03ef5ed82271'),
    'blake3:6146f54874747d6d4b16e06bca7872696f09c16dfe3b3177e5ab0c6c06034155',
    'blake3:c794a69db2c617724483aaa86e0b55f1d694936c4735932b75c2ae5cacdcfecd'),
    '"version":"0.15.0"}', '"version":"0.16.0"}'), 'utf8');
  expected_value := convert_from(expected_document, 'utf8')::jsonb;
  insert into platform_store.activity_listing_revisions (listing_revision_digest, listing_key, canonical_document, pack_revision_digest, client_release_digest, client_surface_id, catalog_visibility, creator_access, public_viewing_policy, result_publication_policy, result_projector_revision_digest, public_projection_schema, public_projection_schema_digest, result_output_schema, result_output_schema_digest, result_canonicalizer_version, result_output_max_bytes, room_setup_configuration, seat_templates, allow_multiple_seats_per_account, pre_start_deadline_seconds)
  values ('blake3:ab6d61d35786e51e3849c68467aad664bd35cb41f299b86d1bcce3f52e4249db', 'worldstream.agent-heist.public-preview', expected_document, expected_value #>> '{pack,digest}', expected_value #>> '{client,release_digest}', expected_value #>> '{client,surface_id}', expected_value #>> '{catalog,visibility}', expected_value ->> 'creator_access', expected_value ->> 'public_viewing_policy', expected_value #>> '{result,publication,policy}', expected_value #>> '{result,projector,digest}', expected_value #>> '{result,projection,schema}', expected_value #>> '{result,projection,digest}', 'worldstream/result-summary/v1', 'blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6', 'worldstream/canonical-json/v1', 4096, expected_value #> '{room_setup,configuration}', expected_value -> 'seats', false, 1800)
  on conflict (listing_revision_digest) do nothing;
  if not exists (select 1 from platform_store.activity_listing_revisions where listing_revision_digest = 'blake3:ab6d61d35786e51e3849c68467aad664bd35cb41f299b86d1bcce3f52e4249db' and canonical_document = expected_document) then
    raise exception using errcode = '23505', message = 'retained_runner_r9_listing_identity_conflict';
  end if;
end;
$listing_revision$;
