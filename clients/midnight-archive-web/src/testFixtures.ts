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

export function rawProjection(overrides: Record<string, unknown> = {}): Record<string, unknown> {
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
        label: "Amber Folio",
        visible_attributes: [
          { label: "Binding", value: "Oxidized brass" },
          { label: "Index mark", value: "Aster / 4" },
          { label: "Year", value: "1891" },
        ],
      },
      {
        candidate_id: "ledger-cobalt",
        label: "Cobalt Register",
        visible_attributes: [
          { label: "Binding", value: "Blue linen" },
          { label: "Index mark", value: "Meridian / 9" },
          { label: "Year", value: "1904" },
        ],
      },
      {
        candidate_id: "ledger-violet",
        label: "Violet Ledger",
        visible_attributes: [
          { label: "Binding", value: "Pale vellum" },
          { label: "Index mark", value: "Aster / 9" },
          { label: "Year", value: "1904" },
        ],
      },
    ],
    staged_action: null,
    carried_candidate: null,
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
