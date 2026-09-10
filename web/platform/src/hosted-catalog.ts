import {
  readHouseAgentRevision,
  readListingRevision,
  type HouseAgentRevision,
  type ListingRevision,
} from "@worldstream/hosted-contract";

import {
  midnightArchiveListingBase64,
  agentHeistListingBase64,
  retainedAgentHeistListing023Base64,
  retainedAgentHeistListing022Base64,
  retainedAgentHeistListing021Base64,
  retainedAgentHeistListing020Base64,
  retainedAgentHeistListing019Base64,
  retainedAgentHeistListing018Base64,
  retainedAgentHeistListing017Base64,
  retainedAgentHeistListing016Base64,
  retainedAgentHeistListing015Base64,
  retainedAgentHeistListing014Base64,
  retainedAgentHeistListing013Base64,
  retainedAgentHeistListing012Base64,
  retainedAgentHeistListing011Base64,
  cooperativePlannerBase64,
  retainedCooperativePlanner16Base64,
  retainedCooperativePlanner15Base64,
  retainedCooperativePlanner14Base64,
  retainedCooperativePlanner13Base64,
  retainedCooperativePlanner12Base64,
  retainedCooperativePlanner11Base64,
  retainedCooperativePlanner10Base64,
  retainedCooperativePlanner9Base64,
  retainedCooperativePlanner8Base64,
  retainedCooperativePlanner7Base64,
  retainedSkepticalAuditor6Base64,
  retainedAgentHeistListing010Base64,
  retainedCooperativePlanner6Base64,
  retainedSkepticalAuditor5Base64,
  retainedAgentHeistListing02Base64,
  retainedAgentHeistListing03Base64,
  retainedAgentHeistListing04Base64,
  retainedAgentHeistListing05Base64,
  retainedAgentHeistListing06Base64,
  retainedAgentHeistListing07Base64,
  retainedAgentHeistListing08Base64,
  retainedAgentHeistListing09Base64,
  retainedCooperativePlanner1Base64,
  retainedCooperativePlanner2Base64,
  retainedCooperativePlanner3Base64,
  retainedCooperativePlanner4Base64,
  retainedCooperativePlanner5Base64,
  retainedSkepticalAuditor2Base64,
  retainedSkepticalAuditor3Base64,
  retainedSkepticalAuditor4Base64,
  retainedSkepticalAuditor1Base64,
  skepticalAuditorBase64,
  retainedSkepticalAuditor15Base64,
  retainedSkepticalAuditor14Base64,
  retainedSkepticalAuditor13Base64,
  retainedSkepticalAuditor12Base64,
  retainedSkepticalAuditor11Base64,
  retainedSkepticalAuditor10Base64,
  retainedSkepticalAuditor9Base64,
  retainedSkepticalAuditor8Base64,
  retainedSkepticalAuditor7Base64,
} from "./hosted-artifacts.generated.js";

export const AGENT_HEIST_LISTING_DIGEST =
  "blake3:71805434c2530094d3a575336cb0a44d71b411ccb089e37f142d9764af860397";

export interface PublicHostedActivity {
  readonly slug: string;
  readonly title: string;
  readonly description: string;
  readonly availability: "available" | "coming_soon" | "dependency_unavailable";
  readonly availabilityMessage: string;
  readonly seatSummary: string;
  readonly participationKinds?: readonly ("human" | "external_agent")[];
  readonly seats: readonly { readonly key: string; readonly label: string; readonly required: boolean }[];
  readonly creatorMaySpectate: boolean;
  readonly houseFillAvailable: boolean;
  readonly publicViewingAvailable: boolean;
  readonly resultPublication: string;
  readonly attribution: string;
  readonly clientPath: string | null;
  /**
   * An exact retained client release that also understands the public
   * projection contract. This is deliberately distinct from participant
   * entry: an older participant client must never be silently upgraded just
   * because somebody opens a public Run URL.
   */
  readonly publicViewerClientPath: string | null;
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
  /**
   * The reviewed activity identity is broader than the current discovery
   * catalog.  Retained and test-only reviewed activities must be able to use
   * the same formation contract without pretending to be Agent Heist.
   */
  readonly slug: PublicHostedActivity["slug"];
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
  // A retained Listing may have been formed with the then-current House
  // revision. Keep current immutable revisions available to recovery even
  // after discovery advances to a newer Listing.
  readHouseAgentRevision(decode(cooperativePlannerBase64)),
  readHouseAgentRevision(decode(skepticalAuditorBase64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner16Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor15Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner15Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor14Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner14Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor13Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner13Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor12Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner12Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor11Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner11Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor10Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner10Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor9Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner9Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor8Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner8Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor7Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner7Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor6Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner6Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor5Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner1Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner2Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner3Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner4Base64)),
  readHouseAgentRevision(decode(retainedCooperativePlanner5Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor2Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor3Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor4Base64)),
  readHouseAgentRevision(decode(retainedSkepticalAuditor1Base64)),
].map((revision) => [revision.digest, revision]));

const agentHeistPublic = Object.freeze({
  slug: "agent-heist",
  title: listing.value.title,
  description: listing.value.description,
  availability: "available",
  availabilityMessage: "Ready for live formation",
  seatSummary: "2 required seats · 1 optional seat",
  participationKinds: ["human", "external_agent"],
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
  clientPath: "/agent-heist-v7/hosted/",
  publicViewerClientPath: "/agent-heist-v7/hosted/",
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
const retainedAgentHeist = [retainedAgentHeistListing02Base64, retainedAgentHeistListing03Base64, retainedAgentHeistListing04Base64, retainedAgentHeistListing05Base64, retainedAgentHeistListing06Base64, retainedAgentHeistListing07Base64, retainedAgentHeistListing08Base64, retainedAgentHeistListing09Base64, retainedAgentHeistListing010Base64, retainedAgentHeistListing011Base64, retainedAgentHeistListing012Base64, retainedAgentHeistListing013Base64, retainedAgentHeistListing014Base64, retainedAgentHeistListing015Base64, retainedAgentHeistListing016Base64, retainedAgentHeistListing017Base64, retainedAgentHeistListing018Base64, retainedAgentHeistListing019Base64, retainedAgentHeistListing020Base64, retainedAgentHeistListing021Base64, retainedAgentHeistListing022Base64, retainedAgentHeistListing023Base64]
  .map((bytes): ReviewedHostedActivity => {
    const retainedListing = readListingRevision(decode(bytes));
    const houseFillAvailable = retainedListing.value.seats.some(
      (seat) => seat.allowed_house_agent_revisions.length > 0,
    );
    const retainedClientPath = bytes === retainedAgentHeistListing014Base64 || bytes === retainedAgentHeistListing019Base64 || bytes === retainedAgentHeistListing020Base64 || bytes === retainedAgentHeistListing021Base64 || bytes === retainedAgentHeistListing022Base64 || bytes === retainedAgentHeistListing023Base64 ? "/agent-heist-v6/hosted/" : bytes === retainedAgentHeistListing012Base64 || bytes === retainedAgentHeistListing013Base64 ? "/agent-heist-v5/hosted/" : (bytes === retainedAgentHeistListing07Base64 || bytes === retainedAgentHeistListing08Base64 || bytes === retainedAgentHeistListing09Base64 || bytes === retainedAgentHeistListing010Base64 || bytes === retainedAgentHeistListing011Base64) ? "/agent-heist-v4/hosted/" : bytes === retainedAgentHeistListing05Base64 || bytes === retainedAgentHeistListing06Base64
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
        publicViewerClientPath: null,
        houseFillAvailable,
        houseTerms: houseFillAvailable ? agentHeistPublic.houseTerms : null,
      }),
    });
  });
const archiveListing = readListingRevision(decode(midnightArchiveListingBase64));
export const MIDNIGHT_ARCHIVE_LISTING_DIGEST = archiveListing.digest;
const reviewedMidnightArchive: ReviewedHostedActivity = Object.freeze({
  slug: "midnight-archive",
  listing: archiveListing,
  houseAgents: new Map<string, HouseAgentRevision>(),
  public: Object.freeze({
    slug: "midnight-archive",
    title: archiveListing.value.title,
    description: archiveListing.value.description,
    availability: "available",
    availabilityMessage: "Internal solo candidate",
    seatSummary: "Solo · 16 turns · 3 power charges",
    participationKinds: ["human"] as const,
    seats: [{ key: "seat-1", label: "Expedition lead", required: true }],
    creatorMaySpectate: false,
    houseFillAvailable: false,
    publicViewingAvailable: false,
    resultPublication: "Private debrief in the activity; no public result publication.",
    attribution: "Anonymous viewing is disabled.",
    clientPath: "/midnight-archive-v10/hosted/",
    publicViewerClientPath: null,
    houseTerms: null,
  }),
});
const currentReviewedActivities = [reviewedAgentHeist, reviewedMidnightArchive];
const reviewedBySlug = new Map(currentReviewedActivities.map((activity) => [activity.slug, activity]));
const reviewedByDigest = new Map(
  [...currentReviewedActivities, ...retainedAgentHeist].map((activity) => [activity.listing.digest, activity]),
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
  publicViewerClientPath: null,
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
  return reviewedBySlug.get(slug) ?? null;
}

/** Nonpublic discovery requires both authentication and an exact deployment opt-in. */
export function reviewedInternalActivities(allowedDigests: readonly string[]): readonly ReviewedHostedActivity[] {
  const allowed = new Set(allowedDigests);
  return currentReviewedActivities.filter((activity) =>
    activity.listing.value.catalog.visibility !== "public" && allowed.has(activity.listing.digest));
}

export function reviewedActivityByDigest(
  digest: string,
  additionalActivities: readonly ReviewedHostedActivity[] = [],
): ReviewedHostedActivity | null {
  return additionalActivities.find((activity) => activity.listing.digest === digest)
    ?? reviewedByDigest.get(digest) ?? null;
}

/**
 * Selects a reviewed public viewer release without inspecting a Pack ID. The
 * catalog owns this correspondence, so callers cannot choose a different
 * client or replace a retained release with the current one.
 */
export function reviewedPublicViewerClientPath(
  client: Pick<PublicHostedActivity, "publicViewerClientPath">,
): string | null {
  return client.publicViewerClientPath;
}

/** Public option labels and opaque seat keys; exact configuration stays server-side. */
export function publicRosterOptions(activity: ReviewedHostedActivity) {
  const schema = activity.listing.value.launch_input_schema;
  if (schema.accepts === "none") return {};
  return {
    rosterOptions: schema.roster_options.map((option) => ({
      key: option.option_id,
      label: option.label,
      seatKeys: option.seat_ids.map((id) => reviewedSeatKey(activity, id)!),
      creatorSeatKeys: option.seat_ids.filter((id) =>
        !option.house_agent_assignments.some((assignment) => assignment.seat_id === id)
        && activity.listing.value.seats.some((seat) => seat.seat_id === id && seat.allowed_participation.includes("account_human")))
        .map((id) => reviewedSeatKey(activity, id)!),
      suppliedAgents: option.house_agent_assignments.length,
    })),
    defaultRosterOption: schema.defaults.roster_option,
  };
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
