/**
 * Calculates the part of the global active-Run gate that this fixture may
 * consume. Retained installations can already contain live Runs, so a probe
 * must never treat the fixed global limit as an empty test-only pool.
 */
export function fixtureAdmissionExpectation({ hardLimit, baselineActiveRuns, attempts }) {
  for (const [name, value] of Object.entries({ hardLimit, baselineActiveRuns, attempts })) {
    if (!Number.isInteger(value) || value < 0) {
      throw new TypeError(`${name} must be a non-negative integer`);
    }
  }

  const availableSlots = Math.max(0, hardLimit - baselineActiveRuns);
  const admitted = Math.min(attempts, availableSlots);
  return { admitted, rejected: attempts - admitted };
}

/**
 * Models the terminal result of this verifier's one House-fill selection
 * against the retained, global House Runner gate. The fixture never adjusts
 * that gate: its exact Launch Request either retains all selected Runner
 * reservations or fails before it creates any.
 */
export function fixtureHouseFillSelectionExpectation({
  hardLimit,
  baselineActiveReservations,
  selectedAssignments,
}) {
  for (const [name, value] of Object.entries({
    hardLimit,
    baselineActiveReservations,
    selectedAssignments,
  })) {
    if (!Number.isInteger(value) || value < 0) {
      throw new TypeError(`${name} must be a non-negative integer`);
    }
  }
  if (selectedAssignments === 0) {
    throw new TypeError("selectedAssignments must be positive for a House-fill selection");
  }

  if (baselineActiveReservations + selectedAssignments > hardLimit) {
    return { state: "failed_pre_genesis", failureCode: "runner_capacity_unavailable" };
  }
  return { state: "reserving", reservationCount: selectedAssignments };
}
