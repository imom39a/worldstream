import assert from "node:assert/strict";
import { test } from "node:test";
import { fixtureAdmissionExpectation } from "./formation-capacity-window.mjs";

test("a retained active-Run baseline leaves only its free capacity for the fixture", () => {
  assert.deepEqual(
    fixtureAdmissionExpectation({ hardLimit: 10, baselineActiveRuns: 2, attempts: 11 }),
    { admitted: 8, rejected: 3 },
  );
});

test("a full retained gate admits no fixture Runs", () => {
  assert.deepEqual(
    fixtureAdmissionExpectation({ hardLimit: 10, baselineActiveRuns: 10, attempts: 11 }),
    { admitted: 0, rejected: 11 },
  );
});
