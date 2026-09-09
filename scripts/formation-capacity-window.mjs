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
