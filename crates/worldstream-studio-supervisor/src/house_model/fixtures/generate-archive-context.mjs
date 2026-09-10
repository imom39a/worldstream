// Run from the repository root after Archive pack:prove, then retain stdout as
// archive-v5-model-context.json beside this file. The retained v4 fixture is
// immutable compatibility evidence. No provider or network calls.
import { readFile } from "node:fs/promises";
import assert from "node:assert/strict";
import pack from "../../../../../packs/midnight-archive/.worldstream/conformance/src/pack.js";
import { initializeArchiveState, startArchive, applyLeadAction } from "../../../../../packs/midnight-archive/.worldstream/conformance/src/rules.js";
import { initialMiraState, applyMiraLeadControl, submitMiraPlan } from "../../../../../packs/midnight-archive/.worldstream/conformance/src/companions.js";
import { authorizedView } from "../../../../../packs/midnight-archive/.worldstream/conformance/src/view.js";

assert.equal(pack.descriptor.schemaVersion, 5);
const root = new URL("../../../../../", import.meta.url);
const proof = JSON.parse(await readFile(new URL("packs/midnight-archive/evidence/production-proof-0.1.0-session-expiry.json", root)));
const listing = JSON.parse(await readFile(new URL("config/hosted/listings/midnight-archive-0.3.0.json", root)));
assert.equal(listing.pack.digest, proof.revisionDigest);
const schemas = JSON.parse(await readFile(new URL("packs/midnight-archive/.worldstream/generated/schemas.json", root)));
const plan = schemas.schemas.find((schema) => schema.schema_id === "worldstream.midnight-archive/action-submit_companion_plan/v5");
const projectionSchema = "worldstream.midnight-archive/participant-projection/v5";
const observationSchema = "worldstream.midnight-archive/participant-observation/v5";
for (const schemaId of [projectionSchema, observationSchema]) {
  const schema = schemas.schemas.find((entry) => entry.schema_id === schemaId);
  assert.ok(schema.canonical_schema.required.includes("session_deadline"));
}
assert.deepEqual(plan.canonical_schema, pack.descriptor.actions.find((action) => action.actionType === "submit_companion_plan").payloadSchema);
const core = { memberships: Object.fromEntries(["lead", "mira", "jonah"].map((role) => [`${role}-1`, {
  access_mode: "participant", member_id: `${role}-1`, principal_id: `${role}-principal`,
  principal_kind: role === "lead" ? "human" : "agent", role, standing: "enabled",
}])) };
const projections = {};
let eligibilityWindow;
for (const scenario_id of ["standard-v1", "low-reserve-v1"]) {
  let state = startArchive("2026-09-09T12:00:00Z", { ...initializeArchiveState({ scenario_id }),
    mira: initialMiraState("mira-1"), jonah: initialMiraState("jonah-1", "jonah"),
    starting_crew: ["lead", "mira", "jonah"].map((role) => ({ role, member_id: `${role}-1` })),
  });
  // Both public sources are known, so every candidate carries both evidence
  // rows. Crew movement, method costs, map and optional objectives are real.
  for (const [action, payload] of [["stage_move", { destination: "records" }], ["stage_inspect_records", {}],
    ["stage_move", { destination: "conservation" }], ["stage_inspect_conservation", {}]]) {
    state = applyLeadAction(state, action, payload, core).state;
    state = applyLeadAction(state, "commit_turn", {}, core).state;
  }
  projections[scenario_id] = {};
  for (const role of ["mira", "jonah"]) {
    let retained = state;
    for (let index = 0; index < 4; index++) {
      retained = applyMiraLeadControl(retained, `assign_${role}_task`, {
        task_kind: "open_service_hatch", power_allowance: role === "mira" ? 2 : 1,
      }, core, "2026-09-09T12:00:10Z", {}, role).state;
      retained = applyMiraLeadControl(retained, `request_${role}_plan`, {}, core, "2026-09-09T12:00:11Z", {}, role).state;
      retained = submitMiraPlan(retained, {
        task_revision: retained[role].task.revision, opportunity_revision: retained[role].opportunity.revision,
        dialogue: '"'.repeat(46) + 'aa',
        steps: [
          { step_type: "move", destination: "records", source_id: "none", power_cost: 0 },
          { step_type: "move", destination: "plant", source_id: "none", power_cost: 0 },
          { step_type: "open_service_hatch", destination: "none", source_id: "none", power_cost: role === "mira" ? 2 : 1 },
        ],
      }, core, `${role}-1`, "2026-09-09T12:00:12Z", {}, role).state;
    }
    let planning = applyMiraLeadControl(retained, `assign_${role}_task`, {
      task_kind: "open_service_hatch", power_allowance: role === "mira" ? 2 : 1,
    }, core, "2026-09-09T12:00:10Z", {}, role).state;
    planning = applyMiraLeadControl(planning, `request_${role}_plan`, {}, core,
      "2026-09-09T12:00:11Z", {}, role).state;
    const view = authorizedView(planning, core, { viewer_type: "participant", member_id: `${role}-1` });
    assert.equal(view.projection.session_deadline, "2026-09-10T12:00:00Z");
    assert.deepEqual(view.actionOffers.map((offer) => offer.actionType), ["submit_companion_plan"]);
    const window = view.actionOffers[0].eligibilityWindow;
    const exactWindow = { opens_at: window.opensAt, deadline: window.deadline };
    if (eligibilityWindow !== undefined) assert.deepEqual(exactWindow, eligibilityWindow);
    eligibilityWindow = exactWindow;
    projections[scenario_id][role] = view.projection;
  }
}
process.stdout.write(JSON.stringify({
  provenance: { pack_revision_digest: proof.revisionDigest, pack_bundle_digest: proof.bundleDigest,
    method: "Real Pack helpers: both-source lead route; four accepted three-step plans with utterances at the 192-byte double-JSON limit; then a fresh hatch planning opportunity for each Role." },
  eligibility_window: eligibilityWindow,
  projection_schema: projectionSchema,
  observation_schema: observationSchema,
  payload_schema: { schema_id: plan.schema_id, schema_digest: plan.schema_digest, schema: plan.canonical_schema },
  projections,
}, null, 2) + "\n");
