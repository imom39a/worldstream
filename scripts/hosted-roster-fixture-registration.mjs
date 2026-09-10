import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { encodeCanonical } from "../sdk/typescript-pack/packages/pack-sdk/dist/index.js";
import { readRosterFixture, rosterFixtureDigest } from "./hosted-roster-fixture.mjs";

const quote = (value) => `'${String(value).replaceAll("'", "''")}'`;
const document = (value) => `decode('${Buffer.from(encodeCanonical(value)).toString("hex")}','hex')`;
const asJson = (value) => `${quote(JSON.stringify(value))}::jsonb`;

/** Local fixture registration only, after exact real Host imports have succeeded. */
export async function rosterFixtureRegistrationSql({ stateDirectory, installationId, importReceipt, environment }) {
  assert.equal(environment.WORLDSTREAM_ROSTER_FIXTURE_QUALIFICATION, "visible-local-only");
  assert.equal(environment.WORLDSTREAM_DEPLOYMENT_ENVIRONMENT, "development");
  assert.notEqual(environment.NODE_ENV, "production");
  assert.notEqual(environment.VERCEL_ENV, "production");
  assert.match(installationId, /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u);
  const { fixture, listing, house } = await readRosterFixture();
  const hex = (value) => Buffer.from(value).toString("hex");
  const profile = JSON.parse(await readFile(join(stateDirectory, "agent-profiles/revisions", hex(fixture.profile_id), `${hex(fixture.version)}.json`), "utf8"));
  const template = JSON.parse(await readFile(join(stateDirectory, "runner-templates/installed", `${fixture.template_id}--${fixture.version}.json`), "utf8"));
  assert.equal(profile.profile_id, fixture.profile_id);
  assert.equal(profile.revision, fixture.version);
  assert.deepEqual(profile.host_contract.runner_template, house.value.runner_template);
  assert.equal(template.template_id, fixture.template_id);
  assert.equal(template.revision, fixture.version);
  assert.equal(importReceipt.status, "complete");
  assert.ok(importReceipt.import_apply?.created_agent_profiles?.some((p) => p.profile_id === fixture.profile_id)
    || importReceipt.import_apply?.reused_agent_profiles?.some((p) => p.profile_id === fixture.profile_id));
  const receipt = { schema: "worldstream/local-roster-fixture-approval/v1", execution: fixture.execution,
    installation_id: installationId, house_agent_digest: house.digest, profile_digest: rosterFixtureDigest(profile),
    template_digest: rosterFixtureDigest(template), executable_digest: `blake3:${template.executable.blake3}` };
  const receiptHash = createHash("sha256").update(encodeCanonical(receipt)).digest("hex");
  const h = house.value;
  const approval = [installationId, house.digest, receipt.profile_digest, receipt.template_digest, receipt.executable_digest, "hosted-openrouter"];
  return `begin;
do $fixture$ begin
  if current_setting('worldstream.development_seed',true) is distinct from 'visible-local-only' then
    raise exception 'local_fixture_registration_forbidden';
  end if;
end $fixture$;
insert into platform_store.activity_listing_revisions
select (jsonb_populate_record(base,jsonb_build_object(
  'listing_revision_digest',${quote(listing.digest)},'listing_key',${quote(listing.value.listing_id)},
  'canonical_document',${document(listing.value)},'seat_templates',${asJson(listing.value.seats)}))).*
from platform_store.activity_listing_revisions base where base.listing_revision_digest=${quote(fixture.base_listing_digest)}
on conflict (listing_revision_digest) do nothing;
insert into platform_store.house_agent_revisions (house_agent_revision_digest,house_agent_key,display_name,canonical_document,
model_slug,provider_slug,behavior_policy_id,behavior_policy_revision,agent_profile_id,agent_profile_revision,
runner_template_id,runner_template_revision,accounting_tokenizer_id,accounting_tokenizer_revision,execution_allowance,tool_set)
values (${[house.digest,h.house_agent_id,h.display_name].map(quote).join(",")},${document(h)},
${[h.route.model_slug,h.route.provider_slug,h.behavior_policy.policy_id,h.behavior_policy.revision,h.agent_profile.profile_id,h.agent_profile.revision,
  h.runner_template.template_id,h.runner_template.revision,h.accounting_tokenizer.tokenizer_id,h.accounting_tokenizer.revision].map(quote).join(",")},${asJson(h.allowance)},'[]'::jsonb)
on conflict (house_agent_revision_digest) do nothing;
insert into platform_store.house_agent_host_approvals(host_installation_id,house_agent_revision_digest,agent_profile_revision_digest,
runner_template_revision_digest,runner_executable_digest,named_credential_reference,approval_receipt_digest,available_for_new_assignments)
values (${approval.map(quote).join(",")},decode('${receiptHash}','hex'),true)
on conflict (host_installation_id,house_agent_revision_digest) do nothing;
-- Ordinary local seeding disables non-current House availability. The explicit
-- fixture opt-in may restore this exact reviewed binding, never a revoked or
-- mismatched approval, and never an Assignment or allowance.
update platform_store.house_agent_host_approvals set available_for_new_assignments=true
where host_installation_id=${quote(installationId)} and house_agent_revision_digest=${quote(house.digest)}
  and agent_profile_revision_digest=${quote(receipt.profile_digest)} and runner_template_revision_digest=${quote(receipt.template_digest)}
  and runner_executable_digest=${quote(receipt.executable_digest)} and approval_receipt_digest=decode('${receiptHash}','hex')
  and named_credential_reference='hosted-openrouter' and revoked_at is null;
do $fixture$ begin
  if not exists(select 1 from platform_store.activity_listing_revisions where listing_revision_digest=${quote(listing.digest)} and canonical_document=${document(listing.value)})
    or not exists(select 1 from platform_store.house_agent_revisions where house_agent_revision_digest=${quote(house.digest)} and canonical_document=${document(h)})
    or not exists(select 1 from platform_store.house_agent_host_approvals where host_installation_id=${quote(installationId)}
      and house_agent_revision_digest=${quote(house.digest)} and agent_profile_revision_digest=${quote(receipt.profile_digest)}
      and runner_template_revision_digest=${quote(receipt.template_digest)} and runner_executable_digest=${quote(receipt.executable_digest)}
      and named_credential_reference='hosted-openrouter'
      and approval_receipt_digest=decode('${receiptHash}','hex') and available_for_new_assignments and revoked_at is null)
  then raise exception 'immutable_local_fixture_registration_conflict'; end if;
end $fixture$;
commit;`;
}
