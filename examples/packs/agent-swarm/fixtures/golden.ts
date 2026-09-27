import type { CanonicalObject } from "@worldstream/pack-sdk";

const coordinator = participant("01ARZ3NDEKTSV4RRFFQ69G5SA1", "human_coordinator", "human", "01ARZ3NDEKTSV4RRFFQ69G5SP1");
const worker = participant("01ARZ3NDEKTSV4RRFFQ69G5SA2", "worker", "agent", "01ARZ3NDEKTSV4RRFFQ69G5SP2");

export const goldenFixture = {
  accepted: [{
    action_type: "confirm_initial_setup",
    admitted_at: "2026-09-15T12:00:00Z",
    canonical_payload: { setup_revision: 2 },
    member_id: coordinator.member_id,
  }],
  configuration: {
    goal: "Prepare a concise comparison of two local fixture outputs.",
    constraints: ["Use only files under the approved working area."],
    acceptance_criteria: ["The comparison identifies both fixture outputs.", "Every conclusion names its local source."],
    approved_working_area: { root: "/tmp/agent-swarm-golden", resource_paths: ["inputs", "result"] },
    roster: [{
      member_key: "worker-a", label: "Fixture Worker A", moving_alias_acknowledged: false, provider: "codex",
      requested_model: "fixture-model-v1", requested_effort: "medium",
      configuration_state: "fixture_unavailable",
    }],
  },
  created_at: "2026-09-15T12:00:00Z",
  participants: [coordinator, worker],
  rejected: {
    action_type: "confirm_initial_setup",
    admitted_at: "2026-09-15T12:00:00Z",
    canonical_payload: { setup_revision: 1 },
    member_id: coordinator.member_id,
  },
} as const;

function participant(member_id: string, role: string, principal_kind: "agent" | "human", principal_id: string): CanonicalObject {
  return { access_mode: "participant", member_id, principal_id, principal_kind, role, standing: "enabled" };
}
