import type { CanonicalObject } from "@worldstream/pack-sdk";

const analyst = participant("late-analyst", "analyst");
const reviewer = participant("late-reviewer", "reviewer");

export const goldenFixture = {
  accepted: [
    {
      action_type: "record_assessment",
      admitted_at: "2026-09-12T15:00:00Z",
      canonical_payload: {
        arrival_version: 1,
        claim: "connection_feasible",
        departure_version: 1,
        expected_work_revision: 1,
        reason_code: "transfer_time_sufficient",
        work_id: "work-C17",
      },
      member_id: analyst.member_id,
    },
    {
      action_type: "record_source_update",
      admitted_at: "2026-09-12T15:01:00Z",
      canonical_payload: {
        arrival_minute: 630,
        arrival_version: 2,
        connection_id: "C17",
        departure_minute: 650,
        departure_version: 1,
        minimum_transfer_minutes: 25,
      },
      member_id: analyst.member_id,
    },
    {
      action_type: "record_assessment",
      admitted_at: "2026-09-12T15:02:00Z",
      canonical_payload: {
        arrival_version: 2,
        claim: "connection_at_risk",
        departure_version: 1,
        expected_work_revision: 2,
        reason_code: "transfer_time_insufficient",
        work_id: "work-C17",
      },
      member_id: reviewer.member_id,
    },
  ],
  configuration: { max_open_work: 16 },
  created_at: "2026-09-12T15:00:00Z",
  participants: [analyst, reviewer],
  rejected: {
    action_type: "record_assessment",
    admitted_at: "2026-09-12T15:00:00Z",
    canonical_payload: {
      arrival_version: 1,
      claim: "connection_feasible",
      departure_version: 1,
      expected_work_revision: 99,
      reason_code: "transfer_time_sufficient",
      work_id: "work-C17",
    },
    member_id: reviewer.member_id,
  },
} as const;

function participant(member_id: string, role: string): CanonicalObject {
  return {
    access_mode: "participant",
    member_id,
    principal_id: `principal-${member_id}`,
    principal_kind: "agent",
    role,
    standing: "enabled",
  };
}
