import type { CanonicalJson } from "@worldstream/pack-sdk";

export const privacyFixture = {
  audiences: [
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FN0", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FN1", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FN2", viewer_type: "public" },
  ],
  forbiddenPublic: ["role_guidance", "assessor_member_id", "arrival_version", "minimum_transfer_minutes"],
  mutations: [{
    activity_state: {
      connections: [{
        arrival_minute: 630,
        arrival_version: 2,
        connection_id: "C17",
        departure_minute: 650,
        departure_version: 1,
        minimum_transfer_minutes: 25,
      }],
      max_open_work: 16,
      objective: "Identify connections needing intervention and record current assessments.",
      open_work: [{
        connection_id: "C17",
        last_assessment: {
          assessor_member_id: "01ARZ3NDEKTSV4RRFFQ69G5FN1",
          arrival_version: 2,
          claim: "connection_at_risk",
          departure_version: 1,
          reason_code: "transfer_time_insufficient",
          validity: "current",
        },
        reason: "arrival_changed",
        revision: 2,
        status: "needs_assessment",
        work_id: "work-C17",
      }],
      phase: "monitoring",
      role_guidance: {
        analyst: "A private fixture mutation proves the analyst-owned guidance is audience-scoped.",
        reviewer: "Reviewers validate the current source revisions and record the bounded assessment conclusion.",
      },
      room_seq: 4,
    },
    hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5FN1"],
  }],
  private_viewers: ["01ARZ3NDEKTSV4RRFFQ69G5FN0", "01ARZ3NDEKTSV4RRFFQ69G5FN1"],
} as const satisfies CanonicalJson;
