import { canonicalStringify, encodeCanonical, taggedBlake3, type CanonicalObject } from "@worldstream/pack-sdk";
import { type ArchiveState, type CompanionRole, type ExtractionPreview, type Role, RuleRejection, asCanonical, reject } from "./model.js";
import { activeMiraMemberId, applyMiraResolution, prepareMiraResolution, type MiraResolution } from "./companions.js";

type ConflictCode = "shared_power" | "service_hatch" | "catalog_verifier" | "ineligible_contribution";
interface Conflict { readonly code: ConflictCode; readonly roles: readonly Role[] }
interface Reservation { readonly role: Role; readonly power: number; readonly interaction: "none" | "service_hatch" | "catalog_verifier" }
export interface TurnResolution {
  readonly status: "clear" | "conflict";
  readonly power_reserved: number;
  readonly prepared_roles: readonly CompanionRole[];
  readonly deferred_roles: readonly CompanionRole[];
  readonly unprepared_roles: readonly CompanionRole[];
  readonly reservations: readonly Reservation[];
  readonly conflicts: readonly Conflict[];
}
interface TurnWork { readonly view: TurnResolution; readonly mira: MiraResolution; readonly jonah: MiraResolution }
function noContribution(): MiraResolution { return { kind: "none", step: null, destination: null }; }

/** Reservations are derived from the retained staged Action and exact preparations.
 * No later resolver can choose a winner or spend resources hidden from this preview. */
export function prepareTurn(state: ArchiveState, core?: CanonicalObject): TurnWork {
  const resolutions = { mira: noContribution(), jonah: noContribution() };
  if (state.phase !== "active") return { ...resolutions, view: { status: "clear", power_reserved: 0,
    prepared_roles: [], deferred_roles: [], unprepared_roles: [], reservations: [], conflicts: [] } };
  const conflicts: Conflict[] = [];
  const prepared: CompanionRole[] = [];
  const deferred: CompanionRole[] = [];
  const unprepared: CompanionRole[] = [];
  const reservations: Reservation[] = [];
  const interaction = (kind: string): Reservation["interaction"] => kind === "open_service_hatch"
    ? "service_hatch" : kind === "use_verifier" ? "catalog_verifier" : "none";
  if (state.staged_action.kind !== "none") reservations.push({ role: "lead", power: state.staged_action.power_cost, interaction: interaction(state.staged_action.kind) });
  for (const role of ["mira", "jonah"] as const) {
    const companion = state[role];
    if (companion.presence !== "active") continue;
    if (companion.mode === "tasked") {
      if (companion.preparation.status === "deferred") deferred.push(role);
      else if (companion.preparation.status === "prepared") prepared.push(role);
      else unprepared.push(role);
    }
    try {
      resolutions[role] = prepareMiraResolution(state, state.staged_action, core, role);
      const step = resolutions[role].step;
      if (step !== null) reservations.push({ role, power: step.power_cost, interaction: interaction(step.step_type) });
    } catch (error) {
      if (!(error instanceof RuleRejection)) throw error;
      conflicts.push({ code: "ineligible_contribution", roles: [role] });
    }
  }
  const power = reservations.reduce((sum, item) => sum + item.power, 0);
  if (power > state.power_remaining) conflicts.push({ code: "shared_power", roles: reservations.filter((item) => item.power > 0).map((item) => item.role) });
  for (const unique of ["service_hatch", "catalog_verifier"] as const) {
    const roles = reservations.filter((item) => item.interaction === unique).map((item) => item.role);
    if (roles.length > 1) conflicts.push({ code: unique, roles });
  }
  return { ...resolutions, view: { status: conflicts.length === 0 ? "clear" : "conflict", power_reserved: power,
    prepared_roles: prepared, deferred_roles: deferred, unprepared_roles: unprepared, reservations, conflicts } };
}

export function resolveCompanions(before: ArchiveState, afterLead: ArchiveState, work: TurnWork): ArchiveState {
  if (work.view.status === "conflict") reject("turn_conflict", "resolve or defer the disclosed conflicting work before commitment");
  let state = afterLead;
  const executed = [...state.completed_crew_work];
  for (const role of ["mira", "jonah"] as const) {
    state = applyMiraResolution(before, state, work[role], role);
    const contribution = state[role].last_contribution;
    if (contribution.turn === before.turns_used + 1 && contribution.kind !== "none") {
      executed.push({ role, kind: contribution.kind, turn: contribution.turn });
    }
  }
  return { ...state, completed_crew_work: executed };
}

export function emptyExtraction(revision = 0): ExtractionPreview {
  return { status: "none", revision, for_turn: 0, contribution_fingerprint: "none", extracted_roles: [], left_behind_roles: [] };
}

export function invalidateExtraction(state: ArchiveState): ArchiveState {
  // Once resolved, extraction is a historical fact, not a reusable preparation.
  if (state.phase === "complete") return state;
  return { ...state, extraction: emptyExtraction(state.extraction.revision + 1) };
}

function contributionFingerprint(state: ArchiveState, core?: CanonicalObject): string {
  return taggedBlake3(encodeCanonical(asCanonical({ domain: "worldstream.midnight-archive/extraction-contributions/v2",
    turn: state.turns_used + 1, staged: state.staged_action,
    mira: state.mira, jonah: state.jonah, gates: state.gates, power: state.power_remaining,
    roster: state.starting_crew, current_core: core ?? null })));
}

function extractedSet(state: ArchiveState, core: CanonicalObject | undefined, work: TurnWork): { extracted: Role[]; left: CompanionRole[] } {
  const after = resolveCompanions(state, state, work);
  const extracted: Role[] = ["lead"];
  const left: CompanionRole[] = [];
  for (const member of state.starting_crew) {
    if (member.role === "lead") continue;
    if (core !== undefined && activeMiraMemberId(core, member.role) === member.member_id &&
      after[member.role].member_id === member.member_id && after[member.role].location === "atrium") extracted.push(member.role);
    else left.push(member.role);
  }
  return { extracted, left };
}

export function prepareExtraction(state: ArchiveState, core?: CanonicalObject): ArchiveState {
  if (state.phase !== "active" || state.location !== "atrium" || state.staged_action.kind !== "extract") {
    reject("illegal_action", "stage extraction in the Atrium before preparing its preview");
  }
  const work = prepareTurn(state, core);
  const { extracted, left } = extractedSet(state, core, work);
  return { ...state, extraction: { status: "prepared", revision: state.extraction.revision + 1,
    for_turn: state.turns_used + 1, contribution_fingerprint: contributionFingerprint(state, core),
    extracted_roles: extracted, left_behind_roles: left } };
}

export function acknowledgeExtraction(state: ArchiveState, payload: CanonicalObject, core?: CanonicalObject): ArchiveState {
  if (Object.keys(payload).sort().join(",") !== "left_behind_roles,preview_revision" || !Array.isArray(payload.left_behind_roles)) {
    reject("invalid_payload", "extraction acknowledgement requires the exact preview revision and left-behind set");
  }
  if (state.extraction.status !== "prepared" || payload.preview_revision !== state.extraction.revision ||
    canonicalStringify(payload.left_behind_roles) !== canonicalStringify([...state.extraction.left_behind_roles]) ||
    state.extraction.contribution_fingerprint !== contributionFingerprint(state, core)) {
    reject("extraction_preview_stale", "acknowledge exactly the current extraction preview and starting crew left behind");
  }
  return { ...state, extraction: { ...state.extraction, status: "acknowledged" } };
}

export function validateExtractionCommit(state: ArchiveState, core?: CanonicalObject): void {
  if (state.extraction.status !== "acknowledged") reject("extraction_preview_required", "prepare and acknowledge the exact extraction preview before committing");
  if (state.extraction.for_turn !== state.turns_used + 1 ||
    state.extraction.contribution_fingerprint !== contributionFingerprint(state, core)) reject("extraction_preview_stale", "the extraction contribution changed after acknowledgement");
}

export function crewDebrief(state: ArchiveState): CanonicalObject {
  const extracted = state.phase === "complete" && state.outcome.kind !== "exhausted_inside" ? state.extraction.extracted_roles : [];
  return asCanonical({ starting_roles: state.starting_crew.map((member) => member.role), extracted_roles: extracted,
    left_behind_roles: state.phase === "complete" ? state.starting_crew.filter((member) => !extracted.includes(member.role)).map((member) => member.role) : [],
    completed_work: state.completed_crew_work.map((item) => ({ ...item })) });
}
