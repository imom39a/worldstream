export type CapabilityStatus = "Implemented" | "Reference" | "Design only" | "Deferred";

export interface Capability {
  name: string;
  area: string;
  interface: string;
  status: CapabilityStatus;
  description: string;
  route: string;
}

export interface CapabilityFilters {
  query: string;
  area: string;
  interface: string;
  status: string;
}

export function filterCapabilities(
  items: readonly Capability[],
  filters: CapabilityFilters,
): Capability[] {
  const query = filters.query.trim().toLocaleLowerCase();
  return items.filter((item) => {
    const text = `${item.name} ${item.area} ${item.interface} ${item.status} ${item.description}`.toLocaleLowerCase();
    return (
      (query === "" || text.includes(query)) &&
      (filters.area === "All" || item.area === filters.area) &&
      (filters.interface === "All" || item.interface === filters.interface) &&
      (filters.status === "All" || item.status === filters.status)
    );
  });
}

export const capabilities: readonly Capability[] = [
  { name: "Ordered room commits", area: "Kernel", interface: "Rust", status: "Implemented", description: "Validate and atomically append authoritative shared-state changes.", route: "/concepts/kernel" },
  { name: "Deterministic reduction", area: "Kernel", interface: "Rust", status: "Implemented", description: "Rebuild room state by replaying canonical events through an Activity Pack.", route: "/concepts/kernel" },
  { name: "Scoped participant views", area: "Kernel", interface: "Rust", status: "Implemented", description: "Project one authoritative state into visibility-safe views per participant.", route: "/concepts/security" },
  { name: "Observation streams", area: "Kernel", interface: "WebSocket", status: "Implemented", description: "Deliver ordered, resumable view frames without exposing hidden state.", route: "/reference/http-api" },
  { name: "Action offers", area: "Kernel", interface: "HTTP", status: "Implemented", description: "Expose the exact currently legal operations and preconditions for a participant.", route: "/concepts/domain-model" },
  { name: "Idempotent action submission", area: "Kernel", interface: "HTTP", status: "Implemented", description: "Deduplicate retries with stable operation identifiers.", route: "/reference/http-api" },
  { name: "Virtual-time timers", area: "Kernel", interface: "Rust", status: "Implemented", description: "Schedule deterministic future changes independent of wall-clock replay.", route: "/concepts/kernel" },
  { name: "Attention and activation", area: "Kernel", interface: "Rust", status: "Implemented", description: "Create durable, cursor-bound work for independently hosted agents.", route: "/concepts/domain-model" },
  { name: "Outcome lifecycle", area: "Kernel", interface: "Rust", status: "Implemented", description: "Record terminal room results as authoritative domain facts.", route: "/concepts/domain-model" },
  { name: "SQLite authority store", area: "Storage", interface: "CLI", status: "Implemented", description: "Run the complete local authority in one owner-protected SQLite database bound to its authority secret.", route: "/operations/storage" },
  { name: "PostgreSQL authority store", area: "Storage", interface: "CLI", status: "Implemented", description: "Operate a PostgreSQL-backed authority with migration and lifecycle commands.", route: "/operations/storage" },
  { name: "Backup and restore", area: "Storage", interface: "CLI", status: "Implemented", description: "Create, inspect, restore, and verify authority backups.", route: "/operations/storage" },
  { name: "SQLite-to-PostgreSQL transfer", area: "Storage", interface: "CLI", status: "Implemented", description: "Perform the supported one-way offline transfer into an empty PostgreSQL target.", route: "/operations/storage" },
  { name: "Authority-bound bootstrap", area: "Security", interface: "CLI", status: "Implemented", description: "Fail closed when a database and authority secret do not belong together.", route: "/concepts/security" },
  { name: "Assignment bearer tokens", area: "Security", interface: "HTTP", status: "Implemented", description: "Bind an agent credential to one assignment rather than the full room.", route: "/concepts/security" },
  { name: "Audit trail", area: "Operations", interface: "HTTP", status: "Implemented", description: "Inspect durable authority decisions and administrative actions.", route: "/operations/observability" },
  { name: "Health, readiness, and version probes", area: "Operations", interface: "HTTP", status: "Implemented", description: "Distinguish process liveness, authority readiness, and protocol identity.", route: "/operations/runbook" },
  { name: "Structured diagnostics", area: "Operations", interface: "CLI", status: "Implemented", description: "Collect bounded local evidence for troubleshooting without copying the authority store.", route: "/operations/observability" },
  { name: "Studio operator portal", area: "Studio", interface: "Browser", status: "Implemented", description: "Configure and supervise the local daemon through a loopback-only control plane.", route: "/studio/setup" },
  { name: "Room and participant workflows", area: "Studio", interface: "Browser", status: "Implemented", description: "Create rooms, manage participants, and issue scoped assignment credentials.", route: "/studio/workflows" },
  { name: "Participant Console", area: "Studio", interface: "Browser", status: "Implemented", description: "Exercise participant-facing observations and actions from a local web client.", route: "/studio/workflows" },
  { name: "Investigation Room", area: "Studio", interface: "Browser", status: "Design only", description: "A proposed forensic workflow; no executable is shipped yet.", route: "/studio/workflows" },
  { name: "Activity Pack contract", area: "Activity Packs", interface: "Rust", status: "Implemented", description: "Provide descriptor, initialize, reduce, view, and observe operations.", route: "/activity-packs/overview" },
  { name: "Counter example pack", area: "Activity Packs", interface: "Rust", status: "Reference", description: "The smallest test-oriented example of deterministic state and actions.", route: "/activity-packs/counter" },
  { name: "Agent Heist pack", area: "Activity Packs", interface: "Rust", status: "Reference", description: "A richer bundled example covering roles, hidden information, timers, and outcomes.", route: "/activity-packs/agent-heist" },
  { name: "Portable untrusted pack ABI", area: "Activity Packs", interface: "ABI", status: "Deferred", description: "Dynamic third-party package loading is intentionally outside local v0.1.", route: "/activity-packs/overview" },
  { name: "Assignment MCP server", area: "Agents", interface: "MCP", status: "Implemented", description: "Expose seven assignment-scoped tools over local stdio.", route: "/agents/mcp" },
  { name: "OpenAI Codex and ChatGPT desktop recipe", area: "Agents", interface: "MCP", status: "Reference", description: "Connect an MCP-capable local host to one assignment; provider-specific acceptance is not yet checked in.", route: "/agents/providers" },
  { name: "Claude Code recipe", area: "Agents", interface: "MCP", status: "Reference", description: "Register the stdio assignment server locally; provider-specific acceptance is not yet checked in.", route: "/agents/providers" },
  { name: "OpenClaw runner", area: "Agents", interface: "MCP", status: "Reference", description: "Use OpenClaw's MCP integration with the same assignment-scoped server.", route: "/agents/providers" },
  { name: "Managed reference Agent Host", area: "Agents", interface: "HTTP", status: "Implemented", description: "Run a bounded loopback OpenAI-compatible model loop with durable activation handling.", route: "/agents/managed-host" },
  { name: "Grok adapter boundary", area: "Agents", interface: "HTTP", status: "Design only", description: "A trusted loopback adapter or external runner is required; none is shipped or accepted yet.", route: "/agents/providers" },
  { name: "DeepSeek adapter boundary", area: "Agents", interface: "HTTP", status: "Design only", description: "A trusted loopback adapter or external runner is required; none is shipped or accepted yet.", route: "/agents/providers" },
  { name: "Native Anthropic adapter", area: "Agents", interface: "HTTP", status: "Deferred", description: "The managed host does not natively speak the Anthropic Messages API; use MCP or a translator.", route: "/agents/providers" },
  { name: "worldstreamd", area: "CLI", interface: "CLI", status: "Implemented", description: "Run the authoritative WorldStream daemon.", route: "/reference/cli" },
  { name: "worldstreamctl", area: "CLI", interface: "CLI", status: "Implemented", description: "Administer SQLite and PostgreSQL stores, backups, and transfers.", route: "/reference/cli" },
  { name: "Studio Supervisor", area: "CLI", interface: "CLI", status: "Implemented", description: "Own the bounded local daemon process lifecycle for Studio.", route: "/reference/cli" },
  { name: "Python SDK", area: "SDK", interface: "Python", status: "Implemented", description: "Use typed clients and examples against the local HTTP authority.", route: "/reference/repository-map" },
  { name: "First-party TypeScript web clients", area: "Studio", interface: "TypeScript", status: "Implemented", description: "Use the internal typed protocol clients that power Studio and the Participant Console; no public TypeScript SDK is shipped.", route: "/reference/repository-map" },
  { name: "GitLab Pages manual", area: "Documentation", interface: "Browser", status: "Reference", description: "Build this local-first manual as a static artifact ready for GitLab Pages.", route: "/maintainers/manual" },
] as const;
