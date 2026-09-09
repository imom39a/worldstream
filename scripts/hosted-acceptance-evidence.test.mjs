import assert from "node:assert/strict";
import { test } from "node:test";

import {
  HOSTED_ACCEPTANCE_SCHEMA,
  LOCAL_ACCEPTANCE_CHECKS,
  DEPLOYED_ACCEPTANCE_CHECKS,
  REQUIRED_RENDERED_CLIENT_CHECKS,
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
    metrics: { provider_calls: 3, maximum_direct_push_seconds: 8 },
    qualification: {
      match_matrix: [1, 2, 3].map((match) => ({
        match,
        mode: match === 3 ? "people_only" : "house_backed",
        outcome: match === 3 ? "failure" : "success",
        provider_call_delta: match === 3 ? 0 : 1,
        capacity_released: true,
        history_retained: match > 1,
        disconnect_and_catch_up: match === 1,
        restart_and_reentry: match === 1,
        no_actions: match === 3,
      })),
      retained_history_count: 2,
      consumed_allowance_observed: true,
      fresh_setup: { status: "passed" },
      retained_upgrade: { status: "passed" },
      ordinary_restart: { status: "passed" },
      populated_recovery: "deferred_not_verified",
      rendered_client: {
        schema: "worldstream/hosted-rendered-browser-journey/v1",
        outcome: "passed",
        completed: true,
        checks: [...REQUIRED_RENDERED_CLIENT_CHECKS],
        provider: "local_fake_provider_only",
      },
    },
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

test("passed local evidence requires the accepted three-match scenario distribution", () => {
  const invalidMode = localEvidence();
  invalidMode.qualification.match_matrix[0].mode = "people_only";
  assert.throws(() => validateHostedAcceptanceEvidence(invalidMode), /qualification_scenario_invalid/u);

  const invalidOutcome = localEvidence();
  invalidOutcome.qualification.match_matrix[2].outcome = "success";
  assert.throws(() => validateHostedAcceptanceEvidence(invalidOutcome), /qualification_scenario_invalid/u);

  const providerCallOnPeopleOnly = localEvidence();
  providerCallOnPeopleOnly.qualification.match_matrix[2].provider_call_delta = 1;
  assert.throws(() => validateHostedAcceptanceEvidence(providerCallOnPeopleOnly), /qualification_scenario_invalid/u);

  const missingRestartProof = localEvidence();
  missingRestartProof.qualification.match_matrix[0].restart_and_reentry = false;
  assert.throws(() => validateHostedAcceptanceEvidence(missingRestartProof), /qualification_scenario_invalid/u);
});

test("passed evidence requires the complete rendered-browser proof", () => {
  const evidence = localEvidence();
  evidence.qualification.rendered_client.checks = evidence.qualification.rendered_client.checks
    .filter((check) => check !== "rendered_room_start");
  assert.throws(() => validateHostedAcceptanceEvidence(evidence), /rendered_client_checks_incomplete/u);
});

test("passed evidence rejects template identity even when every check is green", () => {
  const evidence = localEvidence();
  evidence.candidate_kind = "deployed";
  evidence.checks = Object.fromEntries(DEPLOYED_ACCEPTANCE_CHECKS.map((name) => [name, { status: "passed" }]));
  evidence.metrics.maximum_direct_push_seconds = 301;
  evidence.outcome = "blocked";
  evidence.qualification.fresh_setup.status = "blocked";
  evidence.qualification.retained_upgrade.status = "blocked";
  evidence.qualification.ordinary_restart.status = "blocked";
  evidence.checks = Object.fromEntries(DEPLOYED_ACCEPTANCE_CHECKS.map((name) => [name, { status: "blocked" }]));
  evidence.deployment.platform_revision = "dpl_fixture123";
  evidence.deployment.gateway_revision = evidence.commit;
  evidence.qualification.rendered_client.provider = "openrouter/production";
  assert.doesNotThrow(() => validateHostedAcceptanceEvidence(evidence));
  evidence.outcome = "passed";
  evidence.metrics.provider_calls = 1;
  evidence.qualification.fresh_setup.status = "passed";
  evidence.qualification.retained_upgrade.status = "passed";
  evidence.qualification.ordinary_restart.status = "passed";
  evidence.checks = Object.fromEntries(DEPLOYED_ACCEPTANCE_CHECKS.map((name) => [name, { status: "passed" }]));
  evidence.qualification.rendered_client.provider = "local_fake_provider_only";
  assert.throws(() => validateHostedAcceptanceEvidence(evidence), /rendered_client_provider_invalid/u);
  evidence.qualification.rendered_client.provider = "openrouter/production";
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
