import type { CanonicalJson } from "@worldstream/pack-sdk";

const lead = {
  access_mode: "participant",
  member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0",
  principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD0",
  principal_kind: "human",
  role: "lead",
  standing: "enabled",
} as const;

const mira = {
  access_mode: "participant",
  member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1",
  principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD1",
  principal_kind: "agent",
  role: "mira",
  standing: "enabled",
} as const;

const jonah = {
  access_mode: "participant",
  member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2",
  principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD2",
  principal_kind: "agent",
  role: "jonah",
  standing: "enabled",
} as const;

const spectator = {
  access_mode: "spectator",
  member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8",
  principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD8",
  principal_kind: "human",
  role: null,
  standing: "enabled",
} as const;

const operator = {
  access_mode: "operator",
  member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9",
  principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD9",
  principal_kind: "human",
  role: null,
  standing: "enabled",
} as const;

export const goldenFixture = {
  accepted: [lead, mira, jonah].map(({ member_id }) => ({
    action_type: "inspect",
    admitted_at: "2026-08-30T12:00:01Z",
    canonical_payload: { target: "records" },
    member_id,
  })),
  configuration: { archive_id: "midnight" },
  created_at: "2026-08-30T12:00:00Z",
  externalInputs: [{
    canonical_payload: { opened_by: "host" },
    immutable_resource_references: [],
    input_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6",
    input_type: "archive.fixture/briefing-opened/v1",
    recorded_at: "2026-08-30T12:00:00Z",
    source_id: "01ARZ3NDEKTSV4RRFFQ69G5FH1",
  }],
  invalidActions: [{
    action_type: "inspect",
    canonical_payload: { target: 4 },
    member_id: lead.member_id,
  }, {
    action_type: "inspect",
    canonical_payload: {},
    member_id: lead.member_id,
  }, {
    action_type: "inspect",
    canonical_payload: { extra: true, target: "records" },
    member_id: lead.member_id,
  }],
  participants: [lead, mira, jonah, spectator, operator],
  rosterCases: [
    { participants: [lead] },
    { participants: [lead, mira] },
    { participants: [lead, jonah] },
    { participants: [lead, mira, jonah] },
  ],
  invalidRosterCases: [
    { participants: [mira] },
    { participants: [lead, mira, { ...mira, member_id: "01ARZ3NDEKTSV4RRFFQ69G5FCA" }] },
  ],
  rejected: {
    action_type: "inspect",
    canonical_payload: { target: "records" },
    member_id: lead.member_id,
  },
} as const satisfies CanonicalJson;
