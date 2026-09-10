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
    field_assay: { steps_completed: 0, result: null },
    last_contribution: {
      turn: 0,
      kind: "none",
      summary: "No Mira contribution has completed.",
    },
    ...overrides,
  };
}

export function rawJonah(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ...rawMira({
      last_contribution: {
        turn: 0,
        kind: "none",
        summary: "No Jonah contribution has completed.",
      },
    }),
    ...overrides,
  };
}

export function rawProjection(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  const phase = (overrides.phase ?? "active") as string;
  const outcomeKind = (overrides.outcome as Record<string, unknown> | null)?.kind;
  const defaultTerminalCandidate = phase === "complete"
    && (outcomeKind === "success" || outcomeKind === "partial_extraction" || outcomeKind === "wrong_ledger")
    ? "ledger-violet" : null;
  const carriedCandidate = Object.hasOwn(overrides, "carried_candidate")
    ? overrides.carried_candidate : defaultTerminalCandidate;
  const mira = (overrides.mira ?? rawMira()) as Record<string, unknown>;
  const jonah = (overrides.jonah ?? rawJonah()) as Record<string, unknown>;
  const stagedAction = (overrides.staged_action ?? null) as Record<string, unknown> | null;
  const turnsUsed = (overrides.turns_used ?? 1) as number;
  const power = (overrides.power ?? 3) as number;
  const startingRoles = [
    "lead",
    ...(mira.presence === "absent" ? [] : ["mira"]),
    ...(jonah.presence === "absent" ? [] : ["jonah"]),
  ];
  const turnResolution = defaultTurnResolution(mira, jonah, stagedAction, power);
  const terminal = phase === "complete";
  const exhausted = (overrides.outcome as Record<string, unknown> | null)?.kind === "exhausted_inside";
  const terminalExtracted = terminal && !exhausted ? startingRoles : [];
  const terminalLeft = terminal ? startingRoles.filter((role) => !terminalExtracted.includes(role)) : [];
  return {
    phase,
    objective: "Recover the authentic ledger and return to the Atrium before the archive seals.",
    location: "records",
    turns_used: turnsUsed,
    turns_remaining: 15,
    power,
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
    mira,
    jonah,
    turn_resolution: turnResolution,
    extraction: terminal && !exhausted ? {
      status: "acknowledged",
      revision: 1,
      for_turn: turnsUsed,
      extracted_roles: terminalExtracted,
      left_behind_roles: terminalLeft.filter((role) => role !== "lead"),
    } : {
      status: "none", revision: 0, for_turn: 0, extracted_roles: [], left_behind_roles: [],
    },
    crew_debrief: {
      starting_roles: startingRoles,
      extracted_roles: terminalExtracted,
      left_behind_roles: terminalLeft,
      completed_work: [],
    },
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
    staged_action: stagedAction,
    carried_candidate: carriedCandidate,
    verifier_result: null,
    outcome: null,
    ...overrides,
  };
}

function defaultTurnResolution(
  mira: Record<string, unknown>,
  jonah: Record<string, unknown>,
  staged: Record<string, unknown> | null,
  power: number,
) {
  const companions = { mira, jonah } as const;
  const prepared_roles: string[] = [];
  const deferred_roles: string[] = [];
  const unprepared_roles: string[] = [];
  const reservations: Array<{ role: string; power: number; interaction: string }> = [];
  if (staged !== null) reservations.push({
    role: "lead",
    power: Number(staged.power_cost),
    interaction: staged.action_type === "stage_open_service_hatch" ? "service_hatch"
      : staged.action_type === "stage_use_verifier" ? "catalog_verifier" : "none",
  });
  for (const role of ["mira", "jonah"] as const) {
    const companion = companions[role];
    const preparation = companion.preparation as Record<string, unknown>;
    const tasked = companion.presence === "active" && companion.mode === "tasked";
    if (!tasked) continue;
    if (preparation.status === "prepared") {
      prepared_roles.push(role);
      const summary = String(preparation.summary);
      reservations.push({
        role,
        power: summary.includes("catalog verifier") ? 1
          : summary.includes("service hatch") ? (role === "mira" ? 2 : 1) : 0,
        interaction: summary.includes("catalog verifier") ? "catalog_verifier"
          : summary.includes("service hatch") ? "service_hatch" : "none",
      });
    } else if (preparation.status === "deferred") deferred_roles.push(role);
    else unprepared_roles.push(role);
  }
  const conflicts: Array<{ code: string; roles: string[] }> = [];
  const reserved = reservations.reduce((sum, item) => sum + item.power, 0);
  if (reserved > power) conflicts.push({ code: "shared_power", roles: reservations.filter((item) => item.power > 0).map((item) => item.role) });
  for (const interaction of ["service_hatch", "catalog_verifier"]) {
    const roles = reservations.filter((item) => item.interaction === interaction).map((item) => item.role);
    if (roles.length > 1) conflicts.push({ code: interaction, roles });
  }
  return {
    status: conflicts.length === 0 ? "clear" : "conflict",
    power_reserved: reserved,
    prepared_roles,
    deferred_roles,
    unprepared_roles,
    reservations,
    conflicts,
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
