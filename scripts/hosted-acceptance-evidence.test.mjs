import assert from "node:assert/strict";
import { test } from "node:test";

import {
  HOSTED_ACCEPTANCE_SCHEMA,
  LOCAL_ACCEPTANCE_CHECKS,
  DEPLOYED_ACCEPTANCE_CHECKS,
  validateHostedAcceptanceEvidence,
} from "./hosted-acceptance-evidence.mjs";

function localEvidence() {
  return {
    schema: HOSTED_ACCEPTANCE_SCHEMA,
    candidate_kind: "local",
    outcome: "passed",
    commit: "a".repeat(40),
    recorded_at: "2026-09-06T12:00:00.000Z",
    deployment: {
      platform_revision: "local",
      gateway_revision: "local",
      schema_head: "20260904000000",
      listing_revision_digest: `blake3:${"1".repeat(64)}`,
      pack_digest: `blake3:${"2".repeat(64)}`,
      client_release_digest: `sha256:${"3".repeat(64)}`,
      projector_digest: `blake3:${"4".repeat(64)}`,
    },
    checks: Object.fromEntries(
      LOCAL_ACCEPTANCE_CHECKS.map((name) => [name, { status: "passed" }]),
    ),
    metrics: { provider_calls: 1, maximum_direct_push_seconds: 8 },
    redaction: { private_projections_retained: false, credentials_retained: false },
  };
}

test("acceptance evidence requires every exact check and matching outcome", () => {
  assert.doesNotThrow(() => validateHostedAcceptanceEvidence(localEvidence(), "local"));
  const missing = localEvidence();
  delete missing.checks.signed_in_human;
  assert.throws(() => validateHostedAcceptanceEvidence(missing), /checks_incomplete/u);
  const blocked = localEvidence();
  blocked.checks.reviewed_house_agent = { status: "blocked" };
  assert.throws(() => validateHostedAcceptanceEvidence(blocked), /outcome_checks_disagree/u);
  const dirty = localEvidence();
  dirty.checks.clean_candidate_revision = { status: "blocked" };
  assert.throws(() => validateHostedAcceptanceEvidence(dirty), /outcome_checks_disagree/u);
  dirty.outcome = "blocked";
  assert.doesNotThrow(() => validateHostedAcceptanceEvidence(dirty));
});

test("passed evidence rejects template identity even when every check is green", () => {
  const evidence = localEvidence();
  evidence.candidate_kind = "deployed";
  evidence.checks = Object.fromEntries(DEPLOYED_ACCEPTANCE_CHECKS.map((name) => [name, { status: "passed" }]));
  evidence.metrics.maximum_direct_push_seconds = 301;
  evidence.deployment.platform_revision = "dpl_fixture123";
  evidence.deployment.gateway_revision = evidence.commit;
  assert.doesNotThrow(() => validateHostedAcceptanceEvidence(evidence));
  const exactlyFiveMinutes = structuredClone(evidence);
  exactlyFiveMinutes.metrics.maximum_direct_push_seconds = 300;
  assert.throws(() => validateHostedAcceptanceEvidence(exactlyFiveMinutes), /deployed_metrics_invalid/u);
  for (const field of Object.keys(evidence.deployment)) {
    const invalid = structuredClone(evidence);
    invalid.deployment[field] = field.endsWith("digest") ? `sha256:${"0".repeat(64)}` : "replace-with-value";
    assert.throws(() => validateHostedAcceptanceEvidence(invalid));
  }
  evidence.commit = "0".repeat(40);
  assert.throws(() => validateHostedAcceptanceEvidence(evidence), /placeholder/u);
});

test("acceptance evidence rejects credentials and private Projection fields", () => {
  const credential = localEvidence();
  credential.checks.signed_in_human.note = `wst1:${"a".repeat(64)}`;
  assert.throws(() => validateHostedAcceptanceEvidence(credential), /credential_like/u);
  const privateProjection = localEvidence();
  privateProjection.checks.signed_in_human.private_clues = [];
  assert.throws(() => validateHostedAcceptanceEvidence(privateProjection), /object_shape_invalid/u);
  for (const prefix of ["sk-or-v1-", "sb_secret_"]) {
    const providerCredential = localEvidence();
    providerCredential.checks.signed_in_human.note = `${prefix}${"a".repeat(64)}`;
    assert.throws(() => validateHostedAcceptanceEvidence(providerCredential), /credential_like/u);
  }
});
