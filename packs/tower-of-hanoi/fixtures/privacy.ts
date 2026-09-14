import type { CanonicalJson } from "@worldstream/pack-sdk";

export const privacyFixture = {
  audiences: [
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FH1", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FJ1", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FK1", viewer_type: "public" },
  ],
  forbiddenPublic: ["private_role", "member_notice"],
  mutations: [{
    activity_state: {
      board: { A: [3, 2], B: [], C: [1] },
      claim_assessments: { "01ARZ3NDEKTSV4RRFFQ69G5FJ1": "endorse" },
      completion_claim: { claimant_member_id: "01ARZ3NDEKTSV4RRFFQ69G5FH1", claim_round: 3, electorate_members: ["01ARZ3NDEKTSV4RRFFQ69G5FH1", "01ARZ3NDEKTSV4RRFFQ69G5FJ1"], quorum: 2, work_revision: 1 },
      disks: 3,
      last_move: { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FH1", move: { disk: 1, from: "A", to: "C" }, round: 2 },
      move_limit: 15,
      outcome: { moves: 1, status: "participant_accepted_completion" },
      phase: "complete",
      round: 4,
      work_revision: 1,
      member_notices: {
        "01ARZ3NDEKTSV4RRFFQ69G5FH1": "A private solver note changed after the session.",
        "01ARZ3NDEKTSV4RRFFQ69G5FJ1": "Choose legal work or review a current completion claim from the latest Projection.",
      },
      contributions_by_member: { "01ARZ3NDEKTSV4RRFFQ69G5FH1": 1, "01ARZ3NDEKTSV4RRFFQ69G5FJ1": 0 },
    },
    hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5FJ1"],
  }],
  private_viewers: ["01ARZ3NDEKTSV4RRFFQ69G5FH1", "01ARZ3NDEKTSV4RRFFQ69G5FJ1"],
} as const satisfies CanonicalJson;
