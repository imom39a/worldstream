import type { AuthorizedRoomDeliveryBatch } from "@worldstream/client";

import type { MidnightArchiveActionType } from "./actionContract";
import {
  MIDNIGHT_ARCHIVE_PACK_ID,
  MIDNIGHT_ARCHIVE_PACK_REVISION,
  MIDNIGHT_ARCHIVE_PACK_VERSION,
} from "./config";
import type { MidnightArchiveReadyState } from "./liveAdapter";
import {
  readMidnightArchiveProjection,
  type MidnightArchiveProjection,
} from "./model";

export const digest = (character: string) => `blake3:${character.repeat(64)}`;

export function rawMira(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    presence: "absent",
    location: "none",
    mode: "unavailable",
    task: {
      status: "none",
      revision: 0,
      kind: "none",
      power_allowance: 0,
      power_spent: 0,
    },
    planning: {
      status: "not_requested",
      opportunity_revision: 0,
      plan_revision: 0,
      steps_total: 0,
      steps_completed: 0,
      deadline: "none",
    },
    preparation: { status: "none", for_turn: 0, summary: "none" },
    knowledge: { records: "unknown", conservation: "unknown", verifier_result: null },
    last_contribution: {
      turn: 0,
      kind: "none",
      summary: "No Mira contribution has completed.",
    },
    ...overrides,
  };
}

export function rawProjection(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  const carriedCandidate = overrides.carried_candidate ?? null;
  return {
    phase: "active",
    objective: "Recover the authentic ledger and return to the Atrium before the archive seals.",
    location: "records",
    turns_used: 1,
    turns_remaining: 15,
    power: 3,
    gates: { archive_gate: "closed", service_hatch: "closed" },
    map: {
      locations: [
        { id: "atrium", name: "Atrium", description: "The moonlit extraction point." },
        { id: "records", name: "Records", description: "Catalog terminals and stacks." },
        { id: "conservation", name: "Conservation", description: "A sealed restoration wing." },
        { id: "plant", name: "Plant", description: "Emergency power and service controls." },
        { id: "vault", name: "Vault", description: "The candidate ledgers wait here." },
      ],
      connections: [
        { from: "atrium", to: "records", gate: null },
        { from: "atrium", to: "conservation", gate: null },
        { from: "records", to: "conservation", gate: null },
        { from: "records", to: "plant", gate: null },
        { from: "conservation", to: "vault", gate: "archive_gate" },
        { from: "plant", to: "vault", gate: "service_hatch" },
      ],
    },
    candidates: [
      {
        candidate_id: "ledger-amber",
        evidence_assessment: "unknown",
        observed_evidence: [],
        label: "Amber Folio",
        visible_attributes: [
          { label: "Binding", value: "calfskin" },
          { label: "Marking", value: "compass_rose" },
          { label: "Year", value: "1891" },
        ],
      },
      {
        candidate_id: "ledger-cobalt",
        evidence_assessment: "unknown",
        observed_evidence: [],
        label: "Cobalt Register",
        visible_attributes: [
          { label: "Binding", value: "linen" },
          { label: "Marking", value: "split_star" },
          { label: "Year", value: "1891" },
        ],
      },
      {
        candidate_id: "ledger-violet",
        evidence_assessment: "unknown",
        observed_evidence: [],
        label: "Violet Ledger",
        visible_attributes: [
          { label: "Binding", value: "calfskin" },
          { label: "Marking", value: "split_star" },
          { label: "Year", value: "1904" },
        ],
      },
    ],
    mira: rawMira(),
    preservation_agreement: {
      speaker: "Archivist",
      statement: "Preserve the threatened collection and I will open the Conservation–Vault gate.",
      commitment: "not_accepted",
      conditions: [
        {
          condition_id: "lead_acceptance",
          label: "Human lead accepts this fixed agreement",
          status: "pending",
          turn_cost: 1,
          power_cost: 0,
        },
        {
          condition_id: "collection_preparation",
          label: "Prepare the threatened collection",
          status: "pending",
          turn_cost: 1,
          power_cost: 0,
        },
        {
          condition_id: "equipment_energized",
          label: "Energize the preservation equipment",
          status: "blocked",
          turn_cost: 1,
          power_cost: 1,
        },
      ],
    },
    optional_objectives: {
      collection_preserved: {
        label: "Preserve the threatened collection",
        status: "not_started",
        turn_cost: 2,
        power_cost: 1,
      },
      source_record_protected: {
        label: "Protect the source's identifying record",
        status: carriedCandidate === null ? "locked" : "available",
        turn_cost: 1,
        power_cost: 1,
      },
    },
    debrief: overrides.phase === "complete"
      ? {
        evidence_status: "none",
        message: "No authored source was inspected.",
        agreement_commitment: "not_accepted",
        optional_objectives: {
          collection_preserved: false,
          source_record_protected: false,
        },
      }
      : null,
    staged_action: null,
    carried_candidate: carriedCandidate,
    verifier_result: null,
    outcome: null,
    ...overrides,
  };
}

export function projection(overrides: Record<string, unknown> = {}): MidnightArchiveProjection {
  const parsed = readMidnightArchiveProjection(rawProjection(overrides));
  if (parsed === null) throw new Error("invalid test Projection");
  return parsed;
}

export function offer(actionType: MidnightArchiveActionType, index = 0) {
  return {
    offerId: `7:${actionType}:${index}`,
    actionType,
    schemaDigest: digest(String((index + 1) % 10)),
    eligibility: "Current synchronized turn",
  } as const;
}

export function readyState(
  projectionValue: MidnightArchiveProjection = projection(),
  actions: readonly MidnightArchiveActionType[] = [
    "stage_move",
    "stage_use_verifier",
    "stage_wait",
  ],
): MidnightArchiveReadyState {
  return {
    kind: "ready",
    pack: {
      id: MIDNIGHT_ARCHIVE_PACK_ID,
      version: MIDNIGHT_ARCHIVE_PACK_VERSION,
      digest: MIDNIGHT_ARCHIVE_PACK_REVISION,
    },
    roomHead: {
      genesisOrTransitionHash: digest("8"),
      authoritativeStateHash: digest("9"),
    },
    roomSequence: 7,
    frameHead: 11,
    authorization: { accessMode: "participant", role: "lead" },
    projection: projectionValue,
    offers: actions.map(offer),
  };
}

export function authorizedBatch({
  projectionValue = rawProjection(),
  actions = ["stage_move", "stage_use_verifier", "stage_wait"] as const,
  kind = "projection_reset",
  roomSequence = 7,
}: {
  readonly projectionValue?: Record<string, unknown>;
  readonly actions?: readonly MidnightArchiveActionType[];
  readonly kind?: "projection_reset" | "observation";
  readonly roomSequence?: number;
} = {}): AuthorizedRoomDeliveryBatch {
  const action_offers = actions.map((action_type, index) => ({
    action_type,
    payload_schema_digest: digest(String((index + 1) % 10)),
    eligibility_window: null,
  }));
  return {
    pack: {
      id: MIDNIGHT_ARCHIVE_PACK_ID,
      version: MIDNIGHT_ARCHIVE_PACK_VERSION,
      digest: MIDNIGHT_ARCHIVE_PACK_REVISION,
    },
    room_head: {
      room_seq: roomSequence,
      genesis_or_transition_hash: digest("8"),
      core_schema_version: "worldstream.core-room-state.v1",
      pack_digest: MIDNIGHT_ARCHIVE_PACK_REVISION,
      core_state_hash: digest("7"),
      activity_state_hash: digest("6"),
      authoritative_state_hash: digest("9"),
    },
    frame_head: 11,
    delivery: kind === "projection_reset" ? [{
      kind,
      body: {
        projection: {
          core: {
            access_mode: "participant",
            viewer_class: "participant",
            standing: "enabled",
            role: "lead",
          },
          activity: projectionValue,
          action_offers,
        },
      },
    }] : [{
      kind,
      body: { observation: { ...projectionValue, action_offers } },
    }],
  };
}
