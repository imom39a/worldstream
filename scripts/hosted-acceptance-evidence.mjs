import { constants } from "node:fs";
import { mkdir, open, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

export const HOSTED_ACCEPTANCE_SCHEMA =
  "worldstream/hosted-preview-acceptance-evidence/v1";

export const LOCAL_ACCEPTANCE_CHECKS = Object.freeze([
  "clean_candidate_revision",
  "signed_in_human",
  "invited_external_browser_agent",
  "reviewed_house_agent",
  "anonymous_spectator",
  "direct_gateway_websocket",
  "webmcp_read_wait_commit",
  "disconnect_and_catch_up",
  "retained_room_restart_and_reentry",
  "replay_verified_terminal_result",
  "negative_security_cutline",
]);

export const DEPLOYED_ACCEPTANCE_CHECKS = Object.freeze([
  "real_github_oauth",
  "invited_external_browser_agent",
  "reviewed_openrouter_house_agent",
  "anonymous_spectator",
  "direct_fly_websocket_over_five_minutes",
  "no_vercel_websocket_route",
  "chrome_desktop_human_path",
  "ios_safari_human_and_spectator_path",
  "chatgpt_desktop_webmcp",
  "disconnect_and_catch_up",
  "fly_restart_and_reentry",
  "replay_verified_terminal_result",
  "recent_results_publication",
  "negative_security_cutline",
]);

const DIGEST = /^(?:blake3|sha256):[0-9a-f]{64}$/u;
const COMMIT = /^[0-9a-f]{40}$/u;
const TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/u;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}$/u;
const FORBIDDEN_KEYS = new Set([
  "access_token",
  "authorization",
  "bearer",
  "credential",
  "entry_selector",
  "final_reveal",
  "handoff",
  "invitation_token",
  "member_id",
  "membership_id",
  "private_clues",
  "principal_id",
  "provider_response",
  "refresh_token",
  "room_id",
  "secret",
  "secret_reference",
  "service_authority",
  "session_cookie",
  "ticket",
]);
const FORBIDDEN_VALUES = [
  /\b(?:wsh1|wss1|wst1|wsb1):[0-9a-f]{64}\b/iu,
  /\beyJ[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\b/u,
  /\b(?:sk-or-v1[-_]|sb_secret_)[A-Za-z0-9_-]{16,}\b/u,
  /-----BEGIN [A-Z ]+PRIVATE KEY-----/u,
];

export function validateHostedAcceptanceEvidence(value, expectedKind = undefined) {
  exactObject(value, [
    "schema",
    "candidate_kind",
    "outcome",
    "commit",
    "recorded_at",
    "deployment",
    "checks",
    "metrics",
    "redaction",
  ]);
  if (value.schema !== HOSTED_ACCEPTANCE_SCHEMA) invalid("evidence_schema_invalid");
  if (!['local', 'deployed'].includes(value.candidate_kind)) {
    invalid("candidate_kind_invalid");
  }
  if (expectedKind !== undefined && value.candidate_kind !== expectedKind) {
    invalid("candidate_kind_mismatch");
  }
  if (!['passed', 'blocked'].includes(value.outcome)) invalid("outcome_invalid");
  if (typeof value.commit !== "string" || !COMMIT.test(value.commit)) {
    invalid("commit_invalid");
  }
  if (typeof value.recorded_at !== "string" || !TIMESTAMP.test(value.recorded_at)) {
    invalid("recorded_at_invalid");
  }
  validateDeployment(value.deployment, value.candidate_kind);
  validateChecks(value.checks, value.candidate_kind, value.outcome);
  validateMetrics(value.metrics, value.candidate_kind, value.outcome);
  if (value.outcome === "passed") {
    if (/^0+$/u.test(value.commit)) invalid("placeholder_candidate_identity");
    for (const identity of Object.values(value.deployment)) {
      if (/^(?:replace-with-|placeholder|unknown|pending)|^(?:(?:sha256|blake3):)?0+$/iu.test(identity)) {
        invalid("placeholder_candidate_identity");
      }
    }
    if (!/^\d{14}$/u.test(value.deployment.schema_head)) invalid("schema_head_invalid");
    if (value.candidate_kind === "deployed" && (
      !/^dpl_[A-Za-z0-9]+$/u.test(value.deployment.platform_revision) ||
      value.deployment.gateway_revision !== value.commit
    )) invalid("deployed_revision_invalid");
  }
  exactObject(value.redaction, ["private_projections_retained", "credentials_retained"]);
  if (
    value.redaction.private_projections_retained !== false ||
    value.redaction.credentials_retained !== false
  ) {
    invalid("redaction_attestation_invalid");
  }
  scanForSecrets(value);
  return value;
}

export async function readHostedAcceptanceEvidence(path, expectedKind = undefined) {
  const text = await readFile(path, "utf8");
  if (Buffer.byteLength(text) > 256 * 1024) invalid("evidence_too_large");
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    invalid("evidence_json_invalid");
  }
  return validateHostedAcceptanceEvidence(value, expectedKind);
}

export async function writeHostedAcceptanceEvidence(path, evidence) {
  validateHostedAcceptanceEvidence(evidence);
  const selected = resolve(path);
  await mkdir(dirname(selected), { recursive: true, mode: 0o700 });
  const handle = await open(
    selected,
    constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY,
    0o600,
  );
  try {
    await handle.writeFile(`${JSON.stringify(evidence, null, 2)}\n`);
    await handle.sync();
  } finally {
    await handle.close();
  }
  return selected;
}

function validateDeployment(value, kind) {
  exactObject(value, [
    "platform_revision",
    "gateway_revision",
    "schema_head",
    "listing_revision_digest",
    "pack_digest",
    "client_release_digest",
    "projector_digest",
  ]);
  for (const field of [
    "platform_revision",
    "gateway_revision",
    "schema_head",
  ]) {
    if (typeof value[field] !== "string" || !IDENTIFIER.test(value[field])) {
      invalid(`${field}_invalid`);
    }
  }
  for (const field of [
    "listing_revision_digest",
    "pack_digest",
    "client_release_digest",
    "projector_digest",
  ]) {
    if (typeof value[field] !== "string" || !DIGEST.test(value[field])) {
      invalid(`${field}_invalid`);
    }
  }
  if (kind === "deployed" && value.platform_revision === "local") {
    invalid("deployed_revision_invalid");
  }
}

function validateChecks(value, kind, outcome) {
  if (!plainObject(value)) invalid("checks_invalid");
  const expected = kind === "local" ? LOCAL_ACCEPTANCE_CHECKS : DEPLOYED_ACCEPTANCE_CHECKS;
  if (
    Object.keys(value).length !== expected.length ||
    expected.some((name) => !Object.prototype.hasOwnProperty.call(value, name))
  ) {
    invalid("checks_incomplete");
  }
  for (const name of expected) {
    exactObject(value[name], ["status"], ["note"]);
    if (!['passed', 'blocked'].includes(value[name].status)) invalid("check_status_invalid");
    if (
      value[name].note !== undefined &&
      (typeof value[name].note !== "string" || value[name].note.length > 256)
    ) {
      invalid("check_note_invalid");
    }
  }
  const allPassed = expected.every((name) => value[name].status === "passed");
  if ((outcome === "passed") !== allPassed) invalid("outcome_checks_disagree");
}

function validateMetrics(value, kind, outcome) {
  exactObject(value, ["provider_calls", "maximum_direct_push_seconds"]);
  if (
    !Number.isSafeInteger(value.provider_calls) ||
    value.provider_calls < 0 ||
    value.provider_calls > 10 ||
    !Number.isSafeInteger(value.maximum_direct_push_seconds) ||
    value.maximum_direct_push_seconds < 0 ||
    value.maximum_direct_push_seconds > 86_400
  ) {
    invalid("metrics_invalid");
  }
  if (
    kind === "deployed" && outcome === "passed" &&
    (value.provider_calls !== 1 || value.maximum_direct_push_seconds <= 300)
  ) {
    invalid("deployed_metrics_invalid");
  }
}

function scanForSecrets(value, key = "") {
  if (FORBIDDEN_KEYS.has(key.toLowerCase())) invalid("forbidden_evidence_field");
  if (typeof value === "string") {
    if (FORBIDDEN_VALUES.some((pattern) => pattern.test(value))) {
      invalid("credential_like_evidence_value");
    }
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) scanForSecrets(item);
    return;
  }
  if (plainObject(value)) {
    for (const [childKey, child] of Object.entries(value)) scanForSecrets(child, childKey);
  }
}

function exactObject(value, required, optional = []) {
  if (!plainObject(value)) invalid("object_shape_invalid");
  const allowed = new Set([...required, ...optional]);
  if (
    required.some((key) => !Object.prototype.hasOwnProperty.call(value, key)) ||
    Object.keys(value).some((key) => !allowed.has(key))
  ) {
    invalid("object_shape_invalid");
  }
}

function plainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function invalid(code) {
  throw new Error(code);
}
