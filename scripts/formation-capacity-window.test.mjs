import assert from "node:assert/strict";
import { test } from "node:test";
import {
  fixtureAdmissionExpectation,
  fixtureHouseFillSelectionExpectation,
} from "./formation-capacity-window.mjs";

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

test("a House-fill selection retains its own reservations only when retained runner capacity permits it", () => {
  assert.deepEqual(
    fixtureHouseFillSelectionExpectation({ hardLimit: 4, baselineActiveReservations: 2, selectedAssignments: 2 }),
    { state: "reserving", reservationCount: 2 },
  );
  assert.deepEqual(
    fixtureHouseFillSelectionExpectation({ hardLimit: 4, baselineActiveReservations: 3, selectedAssignments: 2 }),
    { state: "failed_pre_genesis", failureCode: "runner_capacity_unavailable" },
  );
});
