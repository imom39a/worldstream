import type {
  CanonicalJson,
  CanonicalObject,
  PackActionOffer,
} from "@worldstream/pack-sdk";

import type { ArchiveState, Role, StagedAction, VisibleCandidate } from "./model.js";
import { record, stringValue } from "./model.js";
import { authoredEvidenceSources, legalDestinations } from "./rules.js";

export type AudienceSchema =
  | "public"
  | "participant"
  | "operator"
  | "historical_public"
  | "historical_participant"
  | "historical_operator"
  | "final_reveal";

export interface AuthorizedArchiveView {
  readonly schema: AudienceSchema;
  readonly projection: CanonicalObject;
  readonly actionOffers: readonly PackActionOffer[];
}

function archiveMap(): CanonicalJson {
  return {
    locations: [
      { id: "atrium", name: "Atrium", description: "Arrival hall and the only extraction point." },
      { id: "records", name: "Records", description: "Intake shelves and a powered catalog verifier." },
      { id: "conservation", name: "Conservation", description: "Restoration benches beside the sealed archive gate." },
      { id: "plant", name: "Plant", description: "Building controls for the service hatch." },
      { id: "vault", name: "Vault", description: "Three ledger candidates await recovery." },
    ],
    connections: [
      { from: "atrium", to: "records", gate: null },
      { from: "atrium", to: "conservation", gate: null },
      { from: "records", to: "conservation", gate: null },
      { from: "records", to: "plant", gate: null },
      { from: "conservation", to: "vault", gate: "archive_gate" },
      { from: "plant", to: "vault", gate: "service_hatch" },
    ],
  };
}

export function authorizedView(
  state: ArchiveState,
  core: CanonicalObject,
  viewer: CanonicalObject,
): AuthorizedArchiveView {
  const role = viewerRole(core, viewer);
  const viewerType = stringValue(viewer.viewer_type, "viewer_type");
  const participant = role !== null && (viewerType === "participant" || viewerType === "historical");
  const schema = audienceSchema(core, viewer, participant);
  if (!participant) {
    return {
      schema,
      projection: {
        phase: state.phase,
        location: state.location,
        turns_used: state.turns_used,
        outcome: state.outcome.kind === "pending" ? null : { kind: state.outcome.kind },
      },
      actionOffers: [],
    };
  }
  return {
    schema,
    projection: participantProjection(state, role),
    actionOffers: viewerType === "participant" && role === "lead"
      ? leadActionOffers(state)
      : [],
  };
}

export function participantProjection(state: ArchiveState, role: Role): CanonicalObject {
  return {
    phase: state.phase,
    objective: `${state.objective} ${state.role_notes[role]}`,
    location: state.location,
    turns_used: state.turns_used,
    turns_remaining: state.turn_limit - state.turns_used,
    power: state.power_remaining,
    gates: {
      archive_gate: state.gates.conservation_vault_open ? "open" : "closed",
      service_hatch: state.gates.plant_vault_open ? "open" : "closed",
    },
    map: archiveMap(),
    candidates: state.candidates.map((candidate) => ({
      candidate_id: candidate.candidate_id,
      label: candidateLabel(candidate.candidate_id),
      visible_attributes: [
        { label: "Binding", value: candidate.binding },
        { label: "Marking", value: candidate.marking },
        { label: "Year", value: String(candidate.year) },
      ],
      ...candidateEvidence(state, candidate),
    })),
    staged_action: stagedProjection(state.staged_action),
    carried_candidate: state.carried_candidate_id === "none"
      ? null
      : state.carried_candidate_id,
    verifier_result: state.verifier_result === "none"
      ? null
      : { candidate_id: state.verifier_result, confidence: "verified" },
    preservation_agreement: preservationAgreementProjection(state),
    optional_objectives: optionalObjectivesProjection(state),
    debrief: debriefProjection(state),
    outcome: state.outcome.kind === "pending" ? null : { kind: state.outcome.kind },
  };
}

function debriefProjection(state: ArchiveState): CanonicalJson {
  if (state.outcome.kind === "pending") return null;
  const observed = Object.values(state.evidence).filter((status) => status === "observed").length;
  const evidence = observed === 0
    ? { evidence_status: "none", message: "No authored source was inspected." }
    : observed === 1
    ? {
      evidence_status: "partial",
      message: "Only one authored source was inspected; it did not uniquely identify a candidate.",
    }
    : {
      evidence_status: "complete",
      message: "Both authored sources were inspected and their intersection informed the recommendation.",
    };
  return {
    ...evidence,
    agreement_commitment: agreementCommitment(state),
    optional_objectives: {
      collection_preserved: state.collection_preservation === "preserved",
      source_record_protected: state.source_record_protected,
    },
  };
}

function agreementCommitment(state: ArchiveState): "not_accepted" | "accepted" | "honored" {
  if (state.preservation_agreement === "offered") return "not_accepted";
  return state.collection_preservation === "preserved" ? "honored" : "accepted";
}

function preservationAgreementProjection(state: ArchiveState): CanonicalObject {
  return {
    speaker: "Archivist",
    statement: "Preserve the threatened collection and I will open the Conservation–Vault gate.",
    commitment: agreementCommitment(state),
    conditions: [
      {
        condition_id: "lead_acceptance",
        label: "Human lead accepts this fixed agreement",
        status: state.preservation_agreement === "accepted" ? "complete" : "pending",
        turn_cost: 1,
        power_cost: 0,
      },
      {
        condition_id: "collection_preparation",
        label: "Prepare the threatened collection",
        status: state.collection_preservation === "unprepared" ? "pending" : "complete",
        turn_cost: 1,
        power_cost: 0,
      },
      {
        condition_id: "equipment_energized",
        label: "Energize the preservation equipment",
        status: state.collection_preservation === "unprepared"
          ? "blocked"
          : state.collection_preservation === "prepared"
          ? "pending"
          : "complete",
        turn_cost: 1,
        power_cost: 1,
      },
    ],
  };
}

function optionalObjectivesProjection(state: ArchiveState): CanonicalObject {
  return {
    collection_preserved: {
      label: "Preserve the threatened collection",
      status: state.collection_preservation === "unprepared"
        ? "not_started"
        : state.collection_preservation === "prepared"
        ? "prepared"
        : "complete",
      turn_cost: 2,
      power_cost: 1,
    },
    source_record_protected: {
      label: "Protect the source's identifying record",
      status: state.source_record_protected
        ? "complete"
        : state.carried_candidate_id === "none"
        ? "locked"
        : "available",
      turn_cost: 1,
      power_cost: 1,
    },
  };
}

function candidateEvidence(state: ArchiveState, candidate: VisibleCandidate): CanonicalObject {
  const observed = authoredEvidenceSources().filter((source) => state.evidence[source.source_id] === "observed");
  const observedEvidence = observed.map((source) => ({
    source_id: source.source_id,
    source_label: source.source_label,
    attribute_label: source.attribute === "binding" ? "Binding" : source.attribute === "marking" ? "Marking" : "Year",
    observed_value: String(source.value),
    candidate_value: String(candidate[source.attribute]),
    relation: candidate[source.attribute] === source.value ? "matches" : "does_not_match",
  }));
  const recommendation = observedEvidence.length === 2 && observedEvidence.every((evidence) => evidence.relation === "matches");
  return {
    evidence_assessment: recommendation ? "recommended" : observedEvidence.length > 0 ? "observed" : "unknown",
    observed_evidence: observedEvidence,
  };
}

function stagedProjection(staged: StagedAction): CanonicalJson {
  if (staged.kind === "none") return null;
  const common = {
    action_type: `stage_${staged.kind}`,
    turn_cost: staged.turn_cost,
    power_cost: staged.power_cost,
  };
  if (staged.kind === "move") return { ...common, destination: staged.destination };
  if (staged.kind === "recover_candidate") {
    return { ...common, candidate_id: staged.candidate_id };
  }
  return common;
}

function leadActionOffers(state: ArchiveState): PackActionOffer[] {
  if (state.phase !== "active") return [];
  const offers: PackActionOffer[] = [];
  if (legalDestinations(state).length > 0) offers.push(offer("stage_move"));
  if (state.location === "records" && state.evidence.records === "unknown") {
    offers.push(offer("stage_inspect_records"));
  }
  if (state.location === "conservation" && state.evidence.conservation === "unknown") {
    offers.push(offer("stage_inspect_conservation"));
  }
  if (state.location === "conservation" && state.preservation_agreement === "offered") {
    offers.push(offer("stage_accept_preservation_agreement"));
  }
  if (state.location === "conservation" && state.collection_preservation === "unprepared") {
    offers.push(offer("stage_prepare_collection"));
  }
  if (
    state.location === "conservation" &&
    state.collection_preservation === "prepared" &&
    state.power_remaining >= 1
  ) {
    offers.push(offer("stage_energize_preservation_equipment"));
  }
  if (
    state.location === "records" &&
    state.verifier_result === "none" &&
    state.power_remaining >= 1
  ) {
    offers.push(offer("stage_use_verifier"));
  }
  if (
    state.location === "plant" &&
    !state.gates.plant_vault_open &&
    state.power_remaining >= 2
  ) {
    offers.push(offer("stage_open_service_hatch"));
  }
  if (state.location === "vault") offers.push(offer("stage_recover_candidate"));
  if (
    state.location === "plant" &&
    state.carried_candidate_id !== "none" &&
    !state.source_record_protected &&
    state.power_remaining >= 1
  ) {
    offers.push(offer("stage_protect_source_record"));
  }
  if (state.location === "atrium") offers.push(offer("stage_extract"));
  offers.push(offer("stage_wait"));
  if (state.staged_action.kind !== "none") offers.push(offer("commit_turn"));
  return offers;
}

function offer(actionType: string): PackActionOffer {
  return { actionType, eligibilityWindow: null };
}

function candidateLabel(candidateId: string): string {
  if (candidateId === "ledger-amber") return "Amber Ledger";
  if (candidateId === "ledger-cobalt") return "Cobalt Ledger";
  return "Violet Ledger";
}

function viewerRole(core: CanonicalObject, viewer: CanonicalObject): Role | null {
  if (
    viewer.viewer_type !== "participant" &&
    viewer.viewer_type !== "historical"
  ) return null;
  if (typeof viewer.member_id !== "string") return null;
  const memberships = record(core.memberships, "core.memberships");
  const membership = record(memberships[viewer.member_id], "viewer membership");
  return membership.role === "lead" || membership.role === "mira" || membership.role === "jonah"
    ? membership.role
    : null;
}

function audienceSchema(
  core: CanonicalObject,
  viewer: CanonicalObject,
  participant: boolean,
): AudienceSchema {
  if (viewer.viewer_type === "participant") return "participant";
  if (viewer.viewer_type === "operator") return "operator";
  if (viewer.viewer_type === "final_reveal") return "final_reveal";
  if (viewer.viewer_type !== "historical") return "public";
  const memberships = record(core.memberships, "core.memberships");
  const membership = record(memberships[String(viewer.member_id)], "viewer membership");
  if (membership.access_mode === "operator") return "historical_operator";
  return participant ? "historical_participant" : "historical_public";
}
