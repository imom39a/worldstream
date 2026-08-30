import { agentHeistEvidence } from "./buildIdentity";

export type DemoCategory = "application" | "technical-fixture";
export type DemoAvailability = "available" | "preview" | "planned";
export type DemoExperience = "interactive-fixture" | "guided-replay" | "live-shared-room";
export type DemoPerspective = "participant" | "spectator" | "operator" | "integrator" | "pack-author";
export type DemoCapability =
  | "Action Offers"
  | "Attention"
  | "Evidence"
  | "Recorded Replay evidence"
  | "Scoped Projections"
  | "Sealed commitments"
  | "Semantic Time";
export type DemoThumbnailVariant = "agent-heist" | "negotiate";
export type DemoCategoryFilter = "all" | DemoCategory;
export type DemoAvailabilityFilter = "all" | DemoAvailability;
export type DemoExperienceFilter = "all" | DemoExperience;
export type DemoPerspectiveFilter = "all" | DemoPerspective;
export type DemoCapabilityFilter = "all" | DemoCapability;
export type DemoActivityPackFilter = "all" | "worldstream.agent-heist" | "worldstream.negotiate";

export interface DemoActivityPack {
  readonly id: Exclude<DemoActivityPackFilter, "all">;
  readonly label: string;
  readonly fixtureId?: string;
}

export interface DemoDefinition {
  readonly id: string;
  readonly title: string;
  readonly category: DemoCategory;
  readonly categoryLabel: string;
  readonly summary: string;
  readonly activityPack: DemoActivityPack;
  readonly capabilities: readonly DemoCapability[];
  readonly perspectives: readonly DemoPerspective[];
  readonly availability: DemoAvailability;
  readonly experience: DemoExperience;
  readonly experienceLabel: string;
  readonly route: string;
  readonly documentationUrl: string;
  readonly thumbnail: {
    readonly kind: "css-diagram";
    readonly variant: DemoThumbnailVariant;
    readonly description: string;
  };
  readonly backendRequirement: {
    readonly required: boolean;
    readonly label: string;
  };
  readonly buildIdentity?: {
    readonly packVersion: string;
    readonly revisionDigest: string;
  };
}

export interface DemoCatalogFilter {
  readonly activityPack: DemoActivityPackFilter;
  readonly availability: DemoAvailabilityFilter;
  readonly capability: DemoCapabilityFilter;
  readonly category: DemoCategoryFilter;
  readonly experience: DemoExperienceFilter;
  readonly perspective: DemoPerspectiveFilter;
  readonly query: string;
}

export const defaultDemoCatalogFilter: DemoCatalogFilter = Object.freeze({
  activityPack: "all",
  availability: "all",
  capability: "all",
  category: "all",
  experience: "all",
  perspective: "all",
  query: "",
});

export const demos: readonly DemoDefinition[] = [
  {
    id: "agent-heist",
    title: "Agent Heist",
    category: "technical-fixture",
    categoryLabel: "Recorded technical fixture",
    summary:
      "Inspect Projection privacy, Semantic Time, Attention summaries, sealed commitments, and recorded Replay evidence.",
    activityPack: {
      id: "worldstream.agent-heist",
      label: "Agent Heist",
      fixtureId: agentHeistEvidence.fixtureId,
    },
    capabilities: [
      "Scoped Projections",
      "Semantic Time",
      "Attention",
      "Sealed commitments",
      "Recorded Replay evidence",
    ],
    perspectives: ["participant", "spectator", "operator"],
    availability: "available",
    experience: "interactive-fixture",
    experienceLabel: "Interactive fixture",
    route: "/demos/agent-heist",
    documentationUrl: "/docs/agent-heist/",
    thumbnail: {
      kind: "css-diagram",
      variant: "agent-heist",
      description: "One Room connected to three scoped views.",
    },
    backendRequirement: {
      required: false,
      label: "No backend required",
    },
    buildIdentity: {
      packVersion: agentHeistEvidence.packVersion,
      revisionDigest: agentHeistEvidence.revisionDigest,
    },
  },
  {
    id: "negotiate",
    title: "WorldStream Negotiate",
    category: "application",
    categoryLabel: "Application Activity Pack",
    summary:
      "Inspect the planned approval and signing flow for four Roles. This demo needs a persistent WorldStream authority.",
    activityPack: {
      id: "worldstream.negotiate",
      label: "WorldStream Negotiate",
    },
    capabilities: ["Action Offers", "Scoped Projections", "Evidence", "Recorded Replay evidence"],
    perspectives: ["participant", "operator", "integrator"],
    availability: "planned",
    experience: "live-shared-room",
    experienceLabel: "Live shared Room",
    route: "/demos/negotiate/",
    documentationUrl: "/#planned-demo",
    thumbnail: {
      kind: "css-diagram",
      variant: "negotiate",
      description: "One planned Room with proposal, approval, and signing Roles.",
    },
    backendRequirement: {
      required: true,
      label: "Persistent authority required",
    },
  },
] as const;

export function filterDemos(
  catalog: readonly DemoDefinition[],
  filter: DemoCatalogFilter,
): readonly DemoDefinition[] {
  const terms = filter.query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);

  return catalog.filter((demo) => {
    if (filter.category !== "all" && demo.category !== filter.category) return false;
    if (filter.activityPack !== "all" && demo.activityPack.id !== filter.activityPack) return false;
    if (filter.availability !== "all" && demo.availability !== filter.availability) return false;
    if (filter.capability !== "all" && !demo.capabilities.includes(filter.capability)) return false;
    if (filter.experience !== "all" && demo.experience !== filter.experience) return false;
    if (filter.perspective !== "all" && !demo.perspectives.includes(filter.perspective)) return false;

    const searchableText = [
      demo.title,
      demo.categoryLabel,
      demo.summary,
      demo.activityPack.id,
      demo.activityPack.label,
      demo.availability,
      demo.experience,
      demo.experienceLabel,
      demo.backendRequirement.label,
      ...demo.capabilities,
      ...demo.perspectives,
    ]
      .join(" ")
      .toLocaleLowerCase();

    return terms.every((term) => searchableText.includes(term));
  });
}

export function getDemoById(
  catalog: readonly DemoDefinition[],
  id: string,
): DemoDefinition | undefined {
  return catalog.find((demo) => demo.id === id);
}

export interface DemoCatalogFacets {
  readonly activityPacks: readonly { readonly id: DemoActivityPack["id"]; readonly label: string }[];
  readonly availabilities: readonly DemoAvailability[];
  readonly capabilities: readonly DemoCapability[];
  readonly categories: readonly { readonly id: DemoCategory; readonly label: string }[];
  readonly experiences: readonly { readonly id: DemoExperience; readonly label: string }[];
  readonly perspectives: readonly DemoPerspective[];
}

export function deriveDemoCatalogFacets(catalog: readonly DemoDefinition[]): DemoCatalogFacets {
  return {
    activityPacks: uniqueBy(
      catalog.map((demo) => ({ id: demo.activityPack.id, label: demo.activityPack.label })),
      (item) => item.id,
    ),
    availabilities: uniqueSorted(catalog.map((demo) => demo.availability)),
    capabilities: uniqueSorted(catalog.flatMap((demo) => demo.capabilities)),
    categories: uniqueBy(
      catalog.map((demo) => ({ id: demo.category, label: demo.categoryLabel })),
      (item) => item.id,
    ),
    experiences: uniqueBy(
      catalog.map((demo) => ({ id: demo.experience, label: demo.experienceLabel })),
      (item) => item.id,
    ),
    perspectives: uniqueSorted(catalog.flatMap((demo) => demo.perspectives)),
  };
}

function uniqueSorted<Value extends string>(values: readonly Value[]): readonly Value[] {
  return [...new Set(values)].sort((left, right) => left.localeCompare(right));
}

function uniqueBy<Value>(values: readonly Value[], key: (value: Value) => string): readonly Value[] {
  return [...new Map(values.map((value) => [key(value), value])).values()]
    .sort((left, right) => key(left).localeCompare(key(right)));
}
