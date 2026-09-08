import {
  readHouseAgentRevision,
  readListingRevision,
  type HouseAgentRevision,
  type ListingRevision,
} from "@worldstream/hosted-contract";

import {
  agentHeistListingBase64,
  retainedAgentHeistListing02Base64,
  retainedAgentHeistListing03Base64,
  retainedAgentHeistListing04Base64,
  retainedAgentHeistListing05Base64,
  retainedAgentHeistListing06Base64,
  retainedAgentHeistListing07Base64,
  cooperativePlannerBase64,
  retainedCooperativePlanner1Base64,
  retainedCooperativePlanner2Base64,
  retainedCooperativePlanner3Base64,
  retainedSkepticalAuditor2Base64,
  retainedSkepticalAuditor1Base64,
  skepticalAuditorBase64,
} from "./hosted-artifacts.generated.js";

export const AGENT_HEIST_LISTING_DIGEST =
  "blake3:f7beef31cc1160418a103963b7d3884e5f3a71ca1e4fc5c1c0a09d7cb98c909a";

export interface PublicHostedActivity {
  readonly slug: "agent-heist" | "negotiate";
  readonly title: string;
  readonly description: string;
  readonly availability: "available" | "coming_soon" | "dependency_unavailable";
  readonly availabilityMessage: string;
  readonly seatSummary: string;
  readonly seats: readonly { readonly key: string; readonly label: string; readonly required: boolean }[];
  readonly creatorMaySpectate: boolean;
  readonly houseFillAvailable: boolean;
  readonly publicViewingAvailable: boolean;
  readonly resultPublication: string;
  readonly attribution: string;
  readonly clientPath: string | null;
  readonly houseTerms: {
    readonly exhibition: true;
    readonly maximumAgents: 2;
    readonly maximumCallsPerAgent: 10;
    readonly maximumInputTokensPerAgent: 120_000;
    readonly maximumOutputTokensPerAgent: 10_000;
    readonly callTimeoutSeconds: 60;
  } | null;
}

export interface ReviewedHostedActivity {
  readonly slug: "agent-heist";
  readonly listing: ListingRevision;
  readonly houseAgents: ReadonlyMap<string, HouseAgentRevision>;
  readonly public: PublicHostedActivity;
}

const listing = readListingRevision(decode(agentHeistListingBase64));
if (listing.digest !== AGENT_HEIST_LISTING_DIGEST) {
  throw new Error("reviewed_listing_identity_changed");
}
const houseAgents = [
  readHouseAgentRevision(decode(cooperativePlannerBase64)),
  readHouseAgentRevision(decode(skepticalAuditorBase64)),
];
const retainedHouseAgents = new Map([
  readHouseAgentRevision(decode(retainedCooperativePlanner1Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner2Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner3Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor2Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor1Base64)),
].map((revision) => [revision.digest, revision]));

const agentHeistPublic = Object.freeze({
  slug: "agent-heist",
  title: listing.value.title,
  description: listing.value.description,
  availability: "available",
  availabilityMessage: "Ready for live formation",
  seatSummary: "2 required seats · 1 optional seat",
  seats: listing.value.seats.map((seat, index) => ({
    key: `seat-${index + 1}`,
    label: seat.display_name,
    required: seat.required,
  })),
  creatorMaySpectate: false,
  houseFillAvailable: true,
  publicViewingAvailable: true,
  resultPublication: "Replay-verified summaries can appear in Recent Results.",
  attribution: "Results use reviewed seat names unless a participant opts in to a public profile.",
  clientPath: "/agent-heist-v4/hosted/",
  houseTerms: {
    exhibition: true,
    maximumAgents: 2,
    maximumCallsPerAgent: 10,
    maximumInputTokensPerAgent: 120_000,
    maximumOutputTokensPerAgent: 10_000,
    callTimeoutSeconds: 60,
  },
} satisfies PublicHostedActivity);

const reviewedAgentHeist = Object.freeze({
  slug: "agent-heist",
  listing,
  houseAgents: new Map(houseAgents.map((revision) => [revision.digest, revision])),
  public: agentHeistPublic,
} satisfies ReviewedHostedActivity);

// Discovery selects only the current revision. Retained formation and results
// must continue resolving the exact revision accepted before this deployment.
const retainedAgentHeist = [retainedAgentHeistListing02Base64, retainedAgentHeistListing03Base64, retainedAgentHeistListing04Base64, retainedAgentHeistListing05Base64, retainedAgentHeistListing06Base64, retainedAgentHeistListing07Base64]
  .map((bytes): ReviewedHostedActivity => {
    const retainedListing = readListingRevision(decode(bytes));
    const houseFillAvailable = retainedListing.value.seats.some(
      (seat) => seat.allowed_house_agent_revisions.length > 0,
    );
    const retainedClientPath = bytes === retainedAgentHeistListing07Base64 ? "/agent-heist-v4/hosted/" : bytes === retainedAgentHeistListing05Base64 || bytes === retainedAgentHeistListing06Base64
      ? "/agent-heist-v3/hosted/"
      : bytes === retainedAgentHeistListing04Base64 ? "/agent-heist-v2/hosted/" : null;
    return Object.freeze({
      ...reviewedAgentHeist,
      listing: retainedListing,
      houseAgents: new Map([...retainedHouseAgents].filter(([digest]) =>
        retainedListing.value.seats.some((seat) => seat.allowed_house_agent_revisions.includes(digest)))),
      public: Object.freeze({
        ...agentHeistPublic,
        availability: retainedClientPath !== null ? "available" : "dependency_unavailable",
        availabilityMessage: retainedClientPath !== null
          ? "Retained revision with its original client"
          : "This retained revision requires its original client artifact, which is not hosted here.",
        clientPath: retainedClientPath,
        houseFillAvailable,
        houseTerms: houseFillAvailable ? agentHeistPublic.houseTerms : null,
      }),
    });
  });
const reviewedByDigest = new Map(
  [reviewedAgentHeist, ...retainedAgentHeist].map((activity) => [activity.listing.digest, activity]),
);

const negotiatePublic = Object.freeze({
  slug: "negotiate",
  title: "Negotiate",
  description: "A multi-party contract negotiation activity for people and agents.",
  availability: "coming_soon",
  availabilityMessage: "A reviewed hosted release is not available yet.",
  seatSummary: "Participation model under review",
  seats: [],
  creatorMaySpectate: false,
  houseFillAvailable: false,
  publicViewingAvailable: false,
  resultPublication: "Not available",
  attribution: "Not available",
  clientPath: null,
  houseTerms: null,
} satisfies PublicHostedActivity);

export function listPublicHostedActivities(
  dependenciesAvailable: boolean,
): readonly PublicHostedActivity[] {
  return [
    dependenciesAvailable
      ? agentHeistPublic
      : {
          ...agentHeistPublic,
          availability: "dependency_unavailable",
          availabilityMessage: "Live room service is temporarily unavailable.",
        },
    negotiatePublic,
  ];
}

export function reviewedActivityBySlug(slug: string): ReviewedHostedActivity | null {
  return slug === reviewedAgentHeist.slug ? reviewedAgentHeist : null;
}

export function reviewedActivityByDigest(digest: string): ReviewedHostedActivity | null {
  return reviewedByDigest.get(digest) ?? null;
}

export function reviewedSeatId(activity: ReviewedHostedActivity, publicKey: string): string | null {
  const index = activity.public.seats.findIndex(({ key }) => key === publicKey);
  return index < 0 ? null : activity.listing.value.seats[index]?.seat_id ?? null;
}

export function reviewedSeatKey(activity: ReviewedHostedActivity, seatId: string): string | null {
  const index = activity.listing.value.seats.findIndex(({ seat_id }) => seat_id === seatId);
  return index < 0 ? null : activity.public.seats[index]?.key ?? null;
}

function decode(value: string): Uint8Array {
  return Buffer.from(value, "base64");
}
