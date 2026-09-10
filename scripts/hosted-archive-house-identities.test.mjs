import { strict as assert } from "node:assert";
import { mkdtemp, mkdir, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";

import {
  encodeCanonical,
  taggedBlake3,
} from "../sdk/typescript-pack/packages/pack-sdk/dist/index.js";
import { readHouseAgentRevision } from "../sdk/typescript-hosted-contract/dist/index.js";
import {
  renderArchiveHouseAgentProfiles,
  renderArchiveHouseRunnerTemplate,
} from "./hosted-runtime.mjs";
import { prepareHouseApprovals } from "./hosted-house-approval.mjs";

const allowance = {
  call_timeout_seconds: 60,
  concurrent_calls: 1,
  input_tokens_per_call: 12_000,
  model_call_attempts: 10,
  output_tokens_per_call: 1_000,
  total_input_tokens: 120_000,
  total_output_tokens: 10_000,
};

const route = {
  completion_token_parameter: "max_tokens",
  data_collection: "deny",
  gateway: "openrouter",
  maximum_completion_price: "0.00000025",
  maximum_prompt_price: "0.00000006",
  model_slug: "ibm-granite/granite-4.2-8b-20260831",
  provider_slug: "deepinfra/bf16",
  zero_data_retention: true,
};

async function json(path) {
  return JSON.parse(await readFile(new URL(`../${path}`, import.meta.url), "utf8"));
}

async function house(name) {
  const value = await json(`config/hosted/house-agents/${name}-1.json`);
  return readHouseAgentRevision(encodeCanonical(value));
}

test("Mira and Jonah are exact distinct House revisions with reviewed behavior policies", async () => {
  const [mira, jonah, miraPolicy, jonahPolicy] = await Promise.all([
    house("mira"),
    house("jonah"),
    json("config/hosted/house-policies/mira-1.json"),
    json("config/hosted/house-policies/jonah-1.json"),
  ]);
  assert.equal(
    mira.digest,
    "blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde",
  );
  assert.equal(
    jonah.digest,
    "blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0",
  );
  assert.notEqual(mira.digest, jonah.digest);
  assert.deepEqual(mira.value.behavior_policy, miraPolicy);
  assert.deepEqual(jonah.value.behavior_policy, jonahPolicy);
  assert.deepEqual(mira.value.agent_profile, {
    profile_id: "house-midnight-archive-mira",
    revision: "1",
  });
  assert.deepEqual(jonah.value.agent_profile, {
    profile_id: "house-midnight-archive-jonah",
    revision: "1",
  });
  for (const revision of [mira, jonah]) {
    assert.deepEqual(revision.value.route, route);
    assert.deepEqual(revision.value.allowance, allowance);
    assert.deepEqual(revision.value.runner_template, {
      revision: "1",
      template_id: "openrouter-house-archive",
    });
    assert.deepEqual(revision.value.accounting_tokenizer, {
      revision: "1",
      tokenizer_id: "worldstream.utf8-byte-accounting",
    });
    assert.deepEqual(revision.value.tools, []);
    assert.equal(revision.value.version, "1");
  }
});

test("profile sources and Runner template preserve the Host approval boundary", async () => {
  const profiles = renderArchiveHouseAgentProfiles();
  assert.deepEqual(profiles, {
    mira: await json("config/hosted/house-agent-profiles/mira-1.json"),
    jonah: await json("config/hosted/house-agent-profiles/jonah-1.json"),
  });
  assert.equal(
    taggedBlake3(encodeCanonical(profiles.mira)),
    "blake3:074c3df5883f252ad0ce8c8db3cba2d2ed31b44a5dd265b2e67000b359a2a9fd",
  );
  assert.equal(
    taggedBlake3(encodeCanonical(profiles.jonah)),
    "blake3:ee202951744bb38bc0cbf5262db24a4d8762badd4ed8c8379e45d0b6c60fd730",
  );
  const manifest = renderArchiveHouseRunnerTemplate(
    "/var/lib/worldstream/retained-runner-executables/blake3-a/worldstream-managed-agent-host",
    "a".repeat(64),
  );
  assert.deepEqual(manifest.executable, {
    path: "/var/lib/worldstream/retained-runner-executables/blake3-a/worldstream-managed-agent-host",
    blake3: "a".repeat(64),
  });
  assert.deepEqual(manifest.compatibility, [{
    activity_pack_id: "worldstream.midnight-archive",
    exact_revisions: ["0.1.0"],
  }]);
  assert.equal(manifest.template_id, "openrouter-house-archive");
  assert.equal(manifest.revision, "1");
  assert.deepEqual(manifest.secret_environment, []);
  assert.deepEqual(manifest.instances, [{
    instance_id: "hosted-archive-house-01",
    health_address: "127.0.0.1:9608",
  }]);
  for (const profile of Object.values(profiles)) {
    assert.equal(profile.managed_provider_credential_id, "hosted-openrouter");
    assert.deepEqual(profile.non_secret_configuration, {});
    assert.deepEqual(profile.host_contract, {
      kind: "managed_house_openrouter",
      host_contract_revision: "1",
      runner_template: { template_id: manifest.template_id, revision: manifest.revision },
    });
    assert.equal(Object.hasOwn(profile, "secret_settings"), false);
  }
});

test("approval preparation binds both exact companions to installed Host records without activating them", async (t) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), "worldstream-archive-house-")));
  t.after(() => rm(root, { recursive: true, force: true }));
  const state = join(root, "studio");
  const binary = Buffer.from("exact Archive House Runner test executable");
  const binaryDigest = taggedBlake3(binary).slice("blake3:".length);
  const binaryPath = join(root, "worldstream-managed-agent-host");
  await writeFile(binaryPath, binary, { mode: 0o700 });
  const template = renderArchiveHouseRunnerTemplate(
    "/usr/local/bin/worldstream-managed-agent-host",
    binaryDigest,
  );
  const reference = "a".repeat(64);
  const profileSources = renderArchiveHouseAgentProfiles();
  const installedProfiles = Object.values(profileSources).map((source) => ({
    schema: "worldstream/studio-agent-profile/v1",
    profile_id: source.profile_id,
    revision: source.revision,
    display_name: source.display_name,
    non_secret_configuration: source.non_secret_configuration,
    secret_settings: [{
      key: "MODEL_PROVIDER_TOKEN",
      kind: "model_provider",
      reference,
    }],
    host_contract: source.host_contract,
  }));
  const writeJson = async (path, value) => {
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
    await writeFile(path, `${JSON.stringify(value)}\n`, { mode: 0o600 });
  };
  for (const profile of installedProfiles) {
    await writeJson(
      join(
        state,
        "agent-profiles/revisions",
        Buffer.from(profile.profile_id).toString("hex"),
        `${Buffer.from(profile.revision).toString("hex")}.json`,
      ),
      profile,
    );
  }
  await writeJson(
    join(state, "runner-templates/installed/openrouter-house-archive--1.json"),
    template,
  );
  await writeJson(
    join(state, "model-provider-credentials/installed/hosted-openrouter.json"),
    {
      schema: "worldstream/model-provider-credential/v1",
      credential_id: "hosted-openrouter",
      display_name: "Hosted OpenRouter",
      provider: "openrouter",
      secret: { kind: "model_provider", reference },
    },
  );
  const report = {
    status: "complete",
    code: "complete",
    import_apply: {
      schema: "worldstream/initialization-import-apply/v1",
      digest: `blake3:${"b".repeat(64)}`,
      services_started: false,
      created_agent_profiles: installedProfiles.map(({ profile_id, revision }) => ({
        profile_id,
        revision,
      })),
      reused_agent_profiles: [],
      created_runner_templates: [template],
      reused_runner_templates: [],
      created_provider_credentials: ["hosted-openrouter"],
      reused_provider_credentials: [],
    },
  };
  const receiptPath = join(root, "import-apply.json");
  await writeJson(receiptPath, report);
  const prepared = await prepareHouseApprovals({
    controllerState: state,
    runnerBinary: binaryPath,
    importReceipt: receiptPath,
    houseRevisions: [
      resolve("config/hosted/house-agents/mira-1.json"),
      resolve("config/hosted/house-agents/jonah-1.json"),
    ],
    credentialId: "hosted-openrouter",
    installationId: "archive-house-test",
    sourceRevision: "c".repeat(40),
    imageDigest: `sha256:${"d".repeat(64)}`,
  });
  assert.deepEqual(
    prepared.evidence.map(({ receipt }) => receipt.house_agent_revision_digest).sort(),
    [
      "blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde",
      "blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0",
    ],
  );
  assert.equal(new Set(prepared.evidence.map(({ receipt }) => receipt.runner_template_revision_digest)).size, 1);
  assert.equal(new Set(prepared.evidence.map(({ receipt }) => receipt.runner_executable_digest)).size, 1);
  assert.match(prepared.approvalSql, /available_for_new_assignments\)\n  values \([^\n]+, false\)/u);
  assert.doesNotMatch(prepared.approvalSql, /set available_for_new_assignments = true/u);
  assert.match(prepared.activationSql, /set available_for_new_assignments = true/u);
  assert.equal(JSON.stringify(prepared).includes(reference), false);
});
