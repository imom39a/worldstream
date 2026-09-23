import type { CanonicalJson } from "@worldstream/pack-sdk";

export const privacyFixture = {
  audiences: [
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA1", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA2", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA9", viewer_type: "public" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA9", viewer_type: "operator" },
  ],
  forbiddenPublic: [
    "acceptance_criteria", "approved_working_area", "artifact", "candidates", "checks", "configuration_state", "constraints",
    "configuration_revision", "contributions", "directions", "findings", "goal", "handoffs", "human_coordinator_member_id", "local_path", "member_notice",
    "member_notices", "moving_alias_acknowledged", "provider", "requested_effort", "requested_model", "resource_conflicts", "resources", "reviews", "roster",
    "suggestions", "work_attempts", "work_items", "writebacks",
  ],
  mutations: [{
    activity_state: {
      acceptance_criteria: ["The comparison identifies both fixture outputs.", "Every conclusion names its local source."],
      approved_working_area: { root: "/tmp/agent-swarm-golden", resource_paths: ["inputs", "result"] },
      blockers: [], candidates: [], checks: [], confirmed_at_room_seq: 1, confirmed_by_member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA1",
      constraints: ["Use only files under the approved working area."], contributions: [],
      correction_failure_limit: 3, criteria_revision: 1, direction_revision: 0, directions: [], execution_epoch: 1, findings: [],
      goal: "Prepare a concise comparison of two local fixture outputs.", goal_revision: 1, handoffs: [],
      human_coordinator_member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA1", late_contributions: [],
      member_notices: {
        "01ARZ3NDEKTSV4RRFFQ69G5SA1": "The coordinator has a revised private setup reminder.",
        "01ARZ3NDEKTSV4RRFFQ69G5SA2": "Provider execution settings are reported by the local application; Pack state records requested settings without claiming provider success.",
      },
      phase: "open", problems: [], progress_review_interval_seconds: 300, progress_review_sequence: 0, outstanding_progress_reviews: [],
      resource_conflicts: [], resources: [], results: [], reviews: [], room_seq: 1,
      roster: [{
        configuration_revision: 1, configuration_state: "fixture_unavailable", label: "Fixture Worker A", member_id: "01ARZ3NDEKTSV4RRFFQ69G5SA2",
        member_key: "worker-a", moving_alias_acknowledged: false, provider: "codex", requested_effort: "medium", requested_model: "fixture-model-v1",
      }],
      setup_revision: 2, suggestions: [], work_attempts: [], work_items: [], writebacks: [],
    },
    hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5SA2"],
  }],
  private_viewers: ["01ARZ3NDEKTSV4RRFFQ69G5SA1", "01ARZ3NDEKTSV4RRFFQ69G5SA2"],
} as const satisfies CanonicalJson;
