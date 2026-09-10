import type { CanonicalJson, CanonicalObject } from "@worldstream/pack-sdk";

import type { ArchiveState } from "../src/model.js";
import {
  applyLeadAction,
  initializeArchiveState,
  startArchive,
} from "../src/rules.js";
import { initialMiraState } from "../src/companions.js";

export const LEAD_MEMBER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
export const MIRA_MEMBER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC1";
export const JONAH_MEMBER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC2";

const lead = participant(
  LEAD_MEMBER_ID,
  "01ARZ3NDEKTSV4RRFFQ69G5FD0",
  "lead",
  "human",
);
const mira = participant(
  MIRA_MEMBER_ID,
  "01ARZ3NDEKTSV4RRFFQ69G5FD1",
  "mira",
  "agent",
);
const jonah = participant(
  JONAH_MEMBER_ID,
  "01ARZ3NDEKTSV4RRFFQ69G5FD2",
  "jonah",
  "agent",
);
const spectator = observer(
  "01ARZ3NDEKTSV4RRFFQ69G5FC8",
  "01ARZ3NDEKTSV4RRFFQ69G5FD8",
  "spectator",
);
const operator = observer(
  "01ARZ3NDEKTSV4RRFFQ69G5FC9",
  "01ARZ3NDEKTSV4RRFFQ69G5FD9",
  "operator",
);

export const bothObjectivesRouteActions = [
  stage("stage_move", { destination: "records" }),
  stage("commit_turn", {}),
  stage("stage_use_verifier", {}),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "conservation" }),
  stage("commit_turn", {}),
  stage("stage_accept_preservation_agreement", {}),
  stage("commit_turn", {}),
  stage("stage_prepare_collection", {}),
  stage("commit_turn", {}),
  stage("stage_energize_preservation_equipment", {}),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "vault" }),
  stage("commit_turn", {}),
  stage("stage_recover_candidate", { candidate_id: "ledger-violet" }),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "conservation" }),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "records" }),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "plant" }),
  stage("commit_turn", {}),
  stage("stage_protect_source_record", {}),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "records" }),
  stage("commit_turn", {}),
  stage("stage_move", { destination: "atrium" }),
  stage("commit_turn", {}),
  stage("stage_extract", {}),
  stage("prepare_extraction", {}),
  stage("acknowledge_extraction", { preview_revision: 30, left_behind_roles: [] }),
  stage("commit_turn", {}),
] as const;

export const goldenFixture = {
  accepted: bothObjectivesRouteActions.map((action, index) => ({
    ...action,
    admitted_at: `2026-09-09T12:00:${String(index + 2).padStart(2, "0")}Z`,
  })),
  configuration: { scenario_id: "standard-v1" },
  created_at: "2026-09-09T12:00:00Z",
  externalInputs: [{
    canonical_payload: { opened_by: "host" },
    immutable_resource_references: [],
    input_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6",
    input_type: "worldstream.midnight-archive/briefing-opened/v1",
    recorded_at: "2026-09-09T12:00:01Z",
    source_id: "01ARZ3NDEKTSV4RRFFQ69G5FH1",
  }],
  invalidActions: [
    {
      action_type: "stage_move",
      canonical_payload: { destination: 4 },
      member_id: LEAD_MEMBER_ID,
    },
    {
      action_type: "stage_wait",
      canonical_payload: { extra: true },
      member_id: LEAD_MEMBER_ID,
    },
    {
      action_type: "stage_recover_candidate",
      canonical_payload: { candidate_id: "ledger-unknown" },
      member_id: LEAD_MEMBER_ID,
    },
  ],
  participants: [lead, mira, jonah, spectator, operator],
  rosterCases: [
    { participants: [lead] },
    { participants: [lead, mira] },
    { participants: [lead, jonah] },
    { participants: [lead, mira, jonah] },
  ],
  invalidRosterCases: [
    { participants: [mira] },
    {
      participants: [
        lead,
        mira,
        participant(
          "01ARZ3NDEKTSV4RRFFQ69G5FCA",
          "01ARZ3NDEKTSV4RRFFQ69G5FDA",
          "mira",
          "agent",
        ),
      ],
    },
  ],
  rejected: {
    action_type: "stage_wait",
    canonical_payload: {},
    member_id: LEAD_MEMBER_ID,
  },
} as const satisfies CanonicalJson;

export function bothObjectivesRouteFinalState(): ArchiveState {
  let state = startArchive({
    ...initializeArchiveState({ scenario_id: "standard-v1" }),
    mira: initialMiraState(MIRA_MEMBER_ID),
    jonah: initialMiraState(JONAH_MEMBER_ID, "jonah"),
    starting_crew: [{ role: "lead", member_id: LEAD_MEMBER_ID }, { role: "mira", member_id: MIRA_MEMBER_ID }, { role: "jonah", member_id: JONAH_MEMBER_ID }],
  });
  const fixtureCore = {
    memberships: Object.fromEntries(
      [lead, mira, jonah, spectator, operator].map((membership) => [membership.member_id, membership]),
    ),
  } as CanonicalObject;
  for (const action of bothObjectivesRouteActions) {
    state = applyLeadAction(
      state,
      action.action_type,
      action.canonical_payload as CanonicalObject,
      fixtureCore,
    ).state;
  }
  return state;
}

function stage(actionType: string, canonicalPayload: CanonicalObject) {
  return {
    action_type: actionType,
    canonical_payload: canonicalPayload,
    member_id: LEAD_MEMBER_ID,
  };
}

function participant(
  memberId: string,
  principalId: string,
  role: "lead" | "mira" | "jonah",
  principalKind: "agent" | "human",
) {
  return {
    access_mode: "participant",
    member_id: memberId,
    principal_id: principalId,
    principal_kind: principalKind,
    role,
    standing: "enabled",
  } as const;
}

function observer(
  memberId: string,
  principalId: string,
  accessMode: "operator" | "spectator",
) {
  return {
    access_mode: accessMode,
    member_id: memberId,
    principal_id: principalId,
    principal_kind: "human",
    role: null,
    standing: "enabled",
  } as const;
}
