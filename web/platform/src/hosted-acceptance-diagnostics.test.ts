import { strict as assert } from "node:assert";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "vitest";
import { collectHouseWaitDiagnostic, houseWaitDiagnostic } from "./hosted-acceptance-diagnostics.js";

test("House wait diagnostics retain only bounded counts and explicitly allowed codes", () => {
  const secret = "never-log-prompt-session-room-or-provider-text";
  const diagnostic = houseWaitDiagnostic({ sessionStatus: "live", phase: "negotiation",
    plans: [{ endorsements: 1, planId: secret, route: secret }],
    ledger: { schema: "worldstream/house-allowance-ledger@1", assignments: {
      [secret]: { attempts: { [secret]: { state: "provider_failed", failure_code: "route_mismatch", response: secret },
        second: { state: secret, failure_code: secret } } },
    } },
    operations: [{ state: "needs_attention", assignment_id: secret, failure: { code: "host_exited", message: secret } }],
    providerMetrics: { house_completion_count: 2, response: secret },
  });
  assert.equal(diagnostic.endorsement_count, 1);
  assert.equal(diagnostic.provider_house_completion_count, 2);
  assert.equal(diagnostic.attempt_states.provider_failed, 1);
  assert.equal(diagnostic.attempt_failure_codes.route_mismatch, 1);
  assert.equal(diagnostic.attempt_failure_codes.unknown, 1);
  assert.equal(diagnostic.retained_process_failure_codes.host_exited, 1);
  assert.ok(!JSON.stringify(diagnostic).includes(secret));
  const hostile = houseWaitDiagnostic({ sessionStatus: secret, phase: secret,
    plans: Array.from({ length: 100 }, () => ({ endorsements: Number.MAX_SAFE_INTEGER })),
    ledger: null, operations: secret, providerMetrics: { house_completion_count: Infinity } });
  assert.equal(hostile.phase, "unknown");
  assert.equal(hostile.plan_count, 12);
  assert.equal(hostile.endorsement_count, 0);
  assert.equal(hostile.provider_house_completion_count, null);
  assert.ok(!JSON.stringify(hostile).includes(secret));
});

test("missing retained evidence and provider failures remain unavailable without exposing errors", async () => {
  const directory = await mkdtemp(join(tmpdir(), "house-diagnostic-"));
  try {
    const diagnostic = await collectHouseWaitDiagnostic({ sessionStatus: "live", phase: "negotiation", plans: [],
      stateDirectory: directory, providerMetrics: async () => { throw new Error("private provider content"); } });
    assert.equal(diagnostic.ledger_status, "unavailable");
    assert.equal(diagnostic.retained_operations_status, "unavailable");
    assert.equal(diagnostic.provider_house_completion_count, null);
    assert.ok(!JSON.stringify(diagnostic).includes("private"));
  } finally { await rm(directory, { recursive: true, force: true }); }
});
