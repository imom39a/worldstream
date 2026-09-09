import type { CanonicalJson } from "@worldstream/pack-sdk";

import {
  JONAH_MEMBER_ID,
  LEAD_MEMBER_ID,
  MIRA_MEMBER_ID,
  technicalRouteFinalState,
} from "./golden.js";

const finalState = technicalRouteFinalState();
const mutatedState = {
  ...finalState,
  role_notes: {
    ...finalState.role_notes,
    jonah: "Jonah's private priority changed without altering shared facts.",
  },
};

export const privacyFixture = {
  audiences: [
    { member_id: LEAD_MEMBER_ID, viewer_type: "participant" },
    { member_id: MIRA_MEMBER_ID, viewer_type: "participant" },
    { member_id: JONAH_MEMBER_ID, viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "public" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9", viewer_type: "operator" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "historical" },
    { member_id: LEAD_MEMBER_ID, viewer_type: "historical" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9", viewer_type: "historical" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "final_reveal" },
  ],
  forbiddenPublic: [
    "truth_marker",
    "authentic_candidate_id",
    "verifier_result",
    "candidates",
    "carried_candidate",
    "staged_action",
    "role_notes",
  ],
  mutations: [{
    activity_state: mutatedState as unknown as CanonicalJson,
    hidden_from: [LEAD_MEMBER_ID, MIRA_MEMBER_ID],
  }],
  private_viewers: [LEAD_MEMBER_ID, MIRA_MEMBER_ID],
} as const satisfies CanonicalJson;
