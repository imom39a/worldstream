export type DemoCategory = "application" | "conformance";
export type DemoCategoryFilter = "all" | DemoCategory;
export type DemoAvailability = "available" | "planned";
export type DemoExperience = "fixture" | "live";

export interface DemoDefinition {
  readonly id: string;
  readonly title: string;
  readonly category: DemoCategory;
  readonly categoryLabel: string;
  readonly summary: string;
  readonly capabilities: readonly string[];
  readonly availability: DemoAvailability;
  readonly experience: DemoExperience;
  readonly route: string;
}

export interface DemoCatalogFilter {
  readonly category: DemoCategoryFilter;
  readonly query: string;
}

export const demos: readonly DemoDefinition[] = [
  {
    id: "agent-heist",
    title: "Agent Heist",
    category: "conformance",
    categoryLabel: "Visual conformance Activity Pack",
    summary:
      "Inspect how three Agent Participants receive scoped Projections, respond to timers, and commit sealed selections in one deterministic fixture.",
    capabilities: [
      "Projection privacy",
      "Semantic Time",
      "Agent attention",
      "Recovery and Replay",
    ],
    availability: "available",
    experience: "fixture",
    route: "/demos/agent-heist",
  },
  {
    id: "negotiate",
    title: "WorldStream Negotiate",
    category: "application",
    categoryLabel: "Application Activity Pack",
    summary:
      "Inspect the planned approval and signing flow for four Roles. This demo needs a persistent WorldStream authority.",
    capabilities: [
      "Action Offers",
      "Scoped Projections",
      "Evidence",
      "Recovery and Replay",
    ],
    availability: "planned",
    experience: "live",
    route: "/demos/negotiate",
  },
] as const;

export function filterDemos(
  catalog: readonly DemoDefinition[],
  filter: DemoCatalogFilter,
): readonly DemoDefinition[] {
  const terms = filter.query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);

  return catalog.filter((demo) => {
    if (filter.category !== "all" && demo.category !== filter.category) {
      return false;
    }

    const searchableText = [
      demo.title,
      demo.categoryLabel,
      demo.summary,
      demo.availability,
      demo.experience,
      ...demo.capabilities,
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
