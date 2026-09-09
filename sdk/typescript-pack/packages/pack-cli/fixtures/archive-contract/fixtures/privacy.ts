import type { CanonicalJson } from "@worldstream/pack-sdk";

export const privacyFixture = {
  audiences: [
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2", viewer_type: "participant" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "public" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9", viewer_type: "operator" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "historical" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0", viewer_type: "historical" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9", viewer_type: "historical" },
    { member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8", viewer_type: "final_reveal" },
  ],
  forbiddenPublic: ["lead_secret", "mira_secret", "jonah_secret", "private_note"],
  mutations: [{
    activity_state: {
      inspections: 3,
      jonah_secret: "changed",
      lead_secret: "amber",
      mira_secret: "cobalt",
      phase: "active",
    },
    hidden_from: ["01ARZ3NDEKTSV4RRFFQ69G5FC0", "01ARZ3NDEKTSV4RRFFQ69G5FC1"],
  }],
  private_viewers: ["01ARZ3NDEKTSV4RRFFQ69G5FC0", "01ARZ3NDEKTSV4RRFFQ69G5FC1"],
} as const satisfies CanonicalJson;
