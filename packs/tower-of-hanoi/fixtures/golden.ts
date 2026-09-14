import type { CanonicalObject } from "@worldstream/pack-sdk";

const solver = participant("01ARZ3NDEKTSV4RRFFQ69G5FH1", "solver", "01ARZ3NDEKTSV4RRFFQ69G5FP1");
const secondSolver = participant("01ARZ3NDEKTSV4RRFFQ69G5FJ1", "solver", "01ARZ3NDEKTSV4RRFFQ69G5FQ1");

/** An arbitrary non-goal board: this corpus proves mechanics, never a known path. */
export const goldenFixture = {
  accepted: [
    {
      action_type: "move_disk",
      admitted_at: "2026-09-13T12:00:00Z",
      canonical_payload: { disk: 1, from: "A", to: "C" },
      member_id: solver.member_id,
    },
    {
      action_type: "post_completion_claim",
      admitted_at: "2026-09-13T12:00:01Z",
      canonical_payload: { work_revision: 1 },
      member_id: solver.member_id,
    },
    {
      action_type: "assess_claim",
      admitted_at: "2026-09-13T12:00:02Z",
      canonical_payload: { assessment: "endorse", claim_round: 3, work_revision: 1 },
      member_id: secondSolver.member_id,
    },
  ],
  configuration: { disks: 3, move_limit: 15 },
  created_at: "2026-09-13T12:00:00Z",
  participants: [solver, secondSolver],
  rejected: {
    action_type: "move_disk",
    admitted_at: "2026-09-13T12:00:00Z",
    canonical_payload: { disk: 2, from: "A", to: "C" },
    member_id: solver.member_id,
  },
} as const;

function participant(member_id: string, role: string, principal_id: string): CanonicalObject {
  return { access_mode: "participant", member_id, principal_kind: "agent", principal_id, role, standing: "enabled" };
}
