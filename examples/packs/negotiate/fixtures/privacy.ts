import type { CanonicalJson } from "@worldstream/pack-sdk";

export const privacyFixture = {
  forbiddenPublic: [
    "agreement_canonical_json",
    "buyer_acceptance_binding",
    "candidate_canonical_json",
    "current_proposal",
    "evidence",
    "pending_approval",
    "recorded_approval",
    "signatures",
  ],
} as const satisfies CanonicalJson;
