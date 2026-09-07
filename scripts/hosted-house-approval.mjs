import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, mkdir, open, realpath, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

// Use the workspace's already pinned dependency without requiring generated SDK dist.
const require = createRequire(new URL("../sdk/typescript-pack/packages/pack-sdk/package.json", import.meta.url));
const { blake3 } = await import(pathToFileURL(require.resolve("@noble/hashes/blake3.js")));
const RECIPE = "worldstream/operator-house-approval/v1";
const ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;
const BLAKE3 = /^blake3:[0-9a-f]{64}$/u;
const SHA256 = /^sha256:[0-9a-f]{64}$/u;

export function canonicalBytes(value) {
  let nodes = 0;
  function encode(item, depth) {
    if (++nodes > 4096 || depth > 32) throw new Error("approval_json_unbounded");
    if (item === null || typeof item === "boolean" || typeof item === "string") return JSON.stringify(item);
    if (typeof item === "number" && Number.isSafeInteger(item)) return String(item);
    if (Array.isArray(item)) return `[${item.map((entry) => encode(entry, depth + 1)).join(",")}]`;
    if (item !== null && typeof item === "object") {
      return `{${Object.keys(item).sort().map((key) => `${JSON.stringify(key)}:${encode(item[key], depth + 1)}`).join(",")}}`;
    }
    throw new Error("approval_json_invalid");
  }
  return Buffer.from(encode(value, 0));
}

const digest = (bytes) => `blake3:${Buffer.from(blake3(bytes)).toString("hex")}`;
const sha256 = (bytes) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

/** Offline operator evidence only. No vault, network, provider call, or database access. */
export async function prepareHouseApprovals(options) {
  if (!ID.test(options.installationId) || !ID.test(options.credentialId)
      || !/^[0-9a-f]{40}([0-9a-f]{24})?$/u.test(options.sourceRevision)
      || !SHA256.test(options.imageDigest)
      || !Array.isArray(options.houseRevisions) || options.houseRevisions.length < 1 || options.houseRevisions.length > 32) {
    throw new Error("approval_options_invalid");
  }
  const state = await privateDirectory(options.controllerState);
  const report = await jsonFile(options.importReceipt, true);
  const applied = report.import_apply;
  if (report.status !== "complete" || report.code !== "complete"
      || applied?.schema !== "worldstream/initialization-import-apply/v1"
      || !BLAKE3.test(applied.digest) || applied.services_started !== false) {
    throw new Error("approval_import_receipt_invalid");
  }
  const profiles = [...requiredArray(applied.created_agent_profiles), ...requiredArray(applied.reused_agent_profiles)];
  const runners = [...requiredArray(applied.created_runner_templates), ...requiredArray(applied.reused_runner_templates)];
  const providers = [...requiredArray(applied.created_provider_credentials), ...requiredArray(applied.reused_provider_credentials)];
  if (!providers.includes(options.credentialId)) throw new Error("approval_credential_not_reviewed");
  const provider = await jsonFile(await child(state, `model-provider-credentials/installed/${options.credentialId}.json`), true);
  exactKeys(provider, ["schema", "credential_id", "display_name", "provider", "secret"]);
  exactKeys(provider.secret, ["kind", "reference"]);
  if (provider.schema !== "worldstream/model-provider-credential/v1"
      || provider.credential_id !== options.credentialId || provider.provider !== "openrouter"
      || provider.secret.kind !== "model_provider" || !/^[0-9a-f]{64}$/u.test(provider.secret.reference)) {
    throw new Error("approval_credential_metadata_invalid");
  }
  const executable = await fileBytes(options.runnerBinary, 256 * 1024 * 1024, false);
  const executableDigest = digest(executable);
  const receipts = [];
  for (const housePath of options.houseRevisions) {
    const house = await jsonFile(housePath, false);
    if (house.schema !== "worldstream/house-agent-revision/v1"
        || !ID.test(house.agent_profile?.profile_id) || !ID.test(house.agent_profile?.revision)
        || !ID.test(house.runner_template?.template_id) || !ID.test(house.runner_template?.revision)) {
      throw new Error("approval_house_revision_invalid");
    }
    const hex = (value) => Buffer.from(value).toString("hex");
    const profile = await jsonFile(await child(state,
      `agent-profiles/revisions/${hex(house.agent_profile.profile_id)}/${hex(house.agent_profile.revision)}.json`), true);
    const runner = await jsonFile(await child(state,
      `runner-templates/installed/${house.runner_template.template_id}--${house.runner_template.revision}.json`), true);
    exactKeys(profile, ["schema", "profile_id", "revision", "display_name", "non_secret_configuration", "secret_settings", "host_contract"]);
    exactKeys(profile.host_contract, ["kind", "host_contract_revision", "runner_template"]);
    if (profile.schema !== "worldstream/studio-agent-profile/v1"
        || profile.profile_id !== house.agent_profile.profile_id || profile.revision !== house.agent_profile.revision
        || profile.host_contract.kind !== "managed_house_openrouter" || profile.host_contract.host_contract_revision !== "1"
        || !same(profile.host_contract.runner_template, house.runner_template)
        || !same(profile.non_secret_configuration, {})
        || !profiles.some((identity) => same(identity, house.agent_profile))) {
      throw new Error("approval_profile_binding_invalid");
    }
    if (!same(profile.secret_settings, [{ key: "MODEL_PROVIDER_TOKEN", kind: "model_provider", reference: provider.secret.reference }])) {
      throw new Error("approval_profile_credential_mismatch");
    }
    exactKeys(runner, ["schema", "template_id", "revision", "display_name", "executable", "compatibility", "capacity", "health", "non_secret_environment", "secret_environment", "instances"]);
    exactKeys(runner.executable, ["path", "blake3"]);
    if (runner.schema !== "worldstream/runner-template/v1"
        || runner.template_id !== house.runner_template.template_id || runner.revision !== house.runner_template.revision
        || runner.executable.path !== "/usr/local/bin/worldstream-managed-agent-host"
        || `blake3:${runner.executable.blake3}` !== executableDigest
        || !same(runner.secret_environment, [])
        || !same(runner.non_secret_environment, { WORLDSTREAM_RUNNER_MODE: "hosted-house" })
        || !runners.some((reviewed) => same(reviewed, runner))) {
      throw new Error("approval_runner_binding_invalid");
    }
    receipts.push({
      schema: RECIPE,
      hash_recipe: "canonical-json-v1-profile-and-template-blake3;raw-executable-blake3;canonical-receipt-sha256",
      host_installation_id: options.installationId,
      source_revision: options.sourceRevision,
      image_digest: options.imageDigest,
      house_agent_revision_digest: digest(canonicalBytes(house)),
      agent_profile: house.agent_profile,
      agent_profile_revision_digest: digest(canonicalBytes(profile)),
      runner_template: house.runner_template,
      runner_template_revision_digest: digest(canonicalBytes(runner)),
      runner_executable_digest: executableDigest,
      named_credential_reference: options.credentialId,
      initialization_import_review_digest: applied.digest,
      initialization_apply_receipt_sha256: sha256(canonicalBytes(report)),
      authority: "operator_observation_not_runtime_attestation",
    });
  }
  receipts.sort((left, right) => left.house_agent_revision_digest.localeCompare(right.house_agent_revision_digest));
  if (new Set(receipts.map((value) => value.house_agent_revision_digest)).size !== receipts.length) {
    throw new Error("approval_house_revision_duplicate");
  }
  const evidence = receipts.map((receipt) => ({ receipt, approval_receipt_digest: sha256(canonicalBytes(receipt)) }));
  return { evidence, approvalSql: approvalSql(evidence, false), activationSql: approvalSql(evidence, true) };
}

function approvalSql(evidence, activate) {
  const quote = (value) => `'${value.replaceAll("'", "''")}'`;
  return `-- ${RECIPE}: operator-reviewed DML, never a migration or automatic startup step.\nbegin;\n${evidence.map(({ receipt: r, approval_receipt_digest: receiptDigest }) => {
    const fields = ["host_installation_id", "house_agent_revision_digest", "agent_profile_revision_digest", "runner_template_revision_digest", "runner_executable_digest", "named_credential_reference"];
    const values = fields.map((key) => quote(r[key]));
    const receiptValue = `decode(${quote(receiptDigest.slice(7))}, 'hex')`;
    const predicates = fields.map((key, index) => `${key} is not distinct from ${values[index]}`);
    return `do $approval$\nbegin\n${activate ? "" : `  insert into platform_store.house_agent_host_approvals (${fields.join(", ")}, approval_receipt_digest, available_for_new_assignments)\n  values (${values.join(", ")}, ${receiptValue}, false)\n  on conflict (host_installation_id, house_agent_revision_digest) do nothing;\n`}
  if not exists (select 1 from platform_store.house_agent_host_approvals where ${predicates.join(" and ")} and approval_receipt_digest is not distinct from ${receiptValue} and revoked_at is null) then
    raise exception using errcode = '23505', message = 'house_approval_identity_conflict';
  end if;${activate ? `\n  update platform_store.house_agent_host_approvals set available_for_new_assignments = true, availability_checked_at = clock_timestamp()\n  where host_installation_id = ${values[0]} and house_agent_revision_digest = ${values[1]} and approval_receipt_digest = ${receiptValue} and revoked_at is null;` : ""}
end;\n$approval$;`;
  }).join("\n")}\ncommit;\n`;
}

export async function writeApprovalFiles(outputDirectory, prepared) {
  const output = resolve(outputDirectory);
  await privateDirectory(dirname(output));
  await mkdir(output, { mode: 0o700 }); // Must be a new directory, never reuse or overwrite.
  await writeFile(join(output, "receipts.json"), `${JSON.stringify(prepared.evidence, null, 2)}\n`, { flag: "wx", mode: 0o600 });
  await writeFile(join(output, "approve.sql"), prepared.approvalSql, { flag: "wx", mode: 0o600 });
  await writeFile(join(output, "activate.sql"), prepared.activationSql, { flag: "wx", mode: 0o600 });
}

async function privateDirectory(path) {
  if (!isAbsolute(path)) throw new Error("approval_path_must_be_absolute");
  const actual = await realpath(path);
  const metadata = await lstat(path);
  if (actual !== path || !metadata.isDirectory() || metadata.isSymbolicLink() || (metadata.mode & 0o077) !== 0) {
    throw new Error("approval_directory_not_private");
  }
  return path;
}

async function child(root, suffix) {
  const path = join(root, suffix);
  // No symlink traversal, even for a parent directory within a captured tree.
  if (await realpath(path) !== path) throw new Error("approval_symlink_forbidden");
  return path;
}

async function fileBytes(path, maximum, privateFile) {
  if (!isAbsolute(path) || await realpath(path) !== path) throw new Error("approval_file_path_invalid");
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const metadata = await handle.stat();
    if (!metadata.isFile() || metadata.size < 1 || metadata.size > maximum
        || (privateFile && (metadata.mode & 0o077) !== 0)) throw new Error("approval_file_invalid");
    return await handle.readFile();
  } finally { await handle.close(); }
}

async function jsonFile(path, privateFile) {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(await fileBytes(resolve(path), 65_536, privateFile));
  const value = JSON.parse(text);
  // Retained serde JSON uses JSON.stringify-compatible lexemes. Reject duplicate
  // keys and alternate ambiguous encodings before canonicalizing metadata.
  let compact = "", inString = false, escaped = false;
  for (const character of text) {
    if (inString) {
      compact += character;
      if (escaped) escaped = false;
      else if (character === "\\") escaped = true;
      else if (character === '"') inString = false;
    } else if (character === '"') { inString = true; compact += character; }
    else if (!/[\t\n\r ]/u.test(character)) compact += character;
  }
  if (JSON.stringify(value) !== compact) throw new Error("approval_json_ambiguous");
  canonicalBytes(value);
  return value;
}

function exactKeys(value, keys) {
  if (value === null || typeof value !== "object" || Array.isArray(value)
      || !same(Object.keys(value).sort(), [...keys].sort())) throw new Error("approval_metadata_shape_invalid");
}
function requiredArray(value) {
  if (!Array.isArray(value) || value.length > 256) throw new Error("approval_import_receipt_invalid");
  return value;
}
function same(left, right) { return canonicalBytes(left).equals(canonicalBytes(right)); }

async function main() {
  const names = new Map([
    ["--controller-state", "controllerState"], ["--runner-binary", "runnerBinary"],
    ["--import-receipt", "importReceipt"], ["--installation-id", "installationId"],
    ["--credential-id", "credentialId"], ["--source-revision", "sourceRevision"],
    ["--image-digest", "imageDigest"], ["--output-dir", "outputDirectory"],
  ]);
  const options = { houseRevisions: [] };
  const args = process.argv.slice(2);
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index], value = args[index + 1];
    if (!value) throw new Error("approval_argument_invalid");
    if (name === "--house-revision") options.houseRevisions.push(resolve(value));
    else if (names.has(name) && options[names.get(name)] === undefined) options[names.get(name)] = value;
    else throw new Error("approval_argument_invalid");
  }
  if ([...names.values()].some((name) => typeof options[name] !== "string")) throw new Error("approval_argument_missing");
  const prepared = await prepareHouseApprovals(options);
  await writeApprovalFiles(options.outputDirectory, prepared);
  process.stdout.write(`Prepared ${prepared.evidence.length} private House approval receipts. No database change or provider call was made.\n`);
}

if (process.argv[1] !== undefined && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => { process.stderr.write("House approval preparation failed closed; inspect the selected metadata and recipe.\n"); process.exitCode = 1; });
}
