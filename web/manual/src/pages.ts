import type { ManualPage } from "./manual";
import orientation from "./content/orientation.md?raw";
import quickstart from "./content/quickstart.md?raw";
import architecture from "./content/architecture.md?raw";
import domainModel from "./content/domain-model.md?raw";
import securityModel from "./content/security-model.md?raw";
import kernel from "./content/kernel.md?raw";
import activityClients from "./content/activity-clients.md?raw";
import activityPacks from "./content/activity-packs-overview.md?raw";
import negotiate from "./content/negotiate.md?raw";
import counter from "./content/counter.md?raw";
import agentHeist from "./content/agent-heist.md?raw";
import studioSetup from "./content/studio-setup.md?raw";
import studioWorkflows from "./content/studio-workflows.md?raw";
import agentsOverview from "./content/agents-overview.md?raw";
import mcp from "./content/mcp.md?raw";
import providers from "./content/providers.md?raw";
import managedHost from "./content/managed-host.md?raw";
import operations from "./content/operations-runbook.md?raw";
import configuration from "./content/configuration.md?raw";
import storage from "./content/storage.md?raw";
import observability from "./content/observability.md?raw";
import cli from "./content/cli.md?raw";
import httpApi from "./content/http-api.md?raw";
import repositoryMap from "./content/repository-map.md?raw";
import maintenance from "./content/maintenance.md?raw";
import testingRelease from "./content/testing-release.md?raw";
import troubleshooting from "./content/troubleshooting.md?raw";
import glossary from "./content/glossary.md?raw";
import manualMaintenance from "./content/manual-maintenance.md?raw";

export const manualPages: readonly ManualPage[] = [
  { route: "/orientation", title: "Developer orientation", summary: "Learn why WorldStream governs shared reality instead of orchestrating agent work, and how to read this manual.", group: "Start here", source: orientation },
  { route: "/quickstart", title: "Local quickstart", summary: "Install the pinned toolchain and bring up a verified local authority and Studio.", group: "Start here", source: quickstart },
  { route: "/concepts/architecture", title: "Architecture", summary: "Trace change from client or agent through Core, storage, projection, and delivery.", group: "Core concepts", source: architecture },
  { route: "/concepts/domain-model", title: "Domain model", summary: "Use WorldStream terminology precisely: Rooms, Participants, Memberships, Tasks, Actions, and Outcomes.", group: "Core concepts", source: domainModel },
  { route: "/concepts/security", title: "Security and authority", summary: "Understand credentials, privacy projections, authority binding, and trust boundaries.", group: "Core concepts", source: securityModel },
  { route: "/concepts/kernel", title: "Room Kernel", summary: "Build on ordered commits, deterministic reduction, scoped views, timers, and replay.", group: "Core concepts", source: kernel },
  { route: "/concepts/activity-clients", title: "Activity Clients", summary: "Keep Packs headless while independently released clients present authorized participant and spectator experiences.", group: "Core concepts", source: activityClients },
  { route: "/activity-packs/overview", title: "Activity Pack development", summary: "Implement a retained deterministic ruleset against the ActivityPackV1 contract.", group: "Activity Packs", source: activityPacks },
  { route: "/activity-packs/negotiate", title: "WorldStream Negotiate", summary: "Understand the pinned A202 formation Pack, exact approval, privacy, deadlines, and dual evidence.", group: "Activity Packs", source: negotiate },
  { route: "/activity-packs/counter", title: "Counter tutorial", summary: "Follow the smallest complete state, offer, action, observation, and replay example.", group: "Activity Packs", source: counter },
  { route: "/activity-packs/agent-heist", title: "Agent Heist", summary: "Run and dissect the visual demo/conformance Pack with roles, hidden state, timers, attention, and outcomes.", group: "Activity Packs", source: agentHeist },
  { route: "/studio/setup", title: "Studio setup", summary: "Install, configure, start, and verify the local Studio operator portal and Supervisor.", group: "Studio", source: studioSetup },
  { route: "/studio/workflows", title: "Studio workflows", summary: "Operate rooms, participants, agent assignments, backups, and daemon lifecycle safely.", group: "Studio", source: studioWorkflows },
  { route: "/agents/overview", title: "Agent integrations", summary: "Choose the correct boundary for independently hosted agents and model providers.", group: "Agent integration", source: agentsOverview },
  { route: "/agents/mcp", title: "Assignment MCP", summary: "Configure the seven assignment-scoped tools and implement the durable agent loop.", group: "Agent integration", source: mcp },
  { route: "/agents/providers", title: "Provider recipes", summary: "Connect Codex, ChatGPT, Claude, Grok, DeepSeek, and OpenClaw without overstating compatibility.", group: "Agent integration", source: providers },
  { route: "/agents/managed-host", title: "Managed Agent Host", summary: "Run the bounded loopback OpenAI-compatible reference host and understand its constraints.", group: "Agent integration", source: managedHost },
  { route: "/operations/runbook", title: "Local operations runbook", summary: "Start, stop, inspect, recover, and validate a local WorldStream authority.", group: "Operate", source: operations },
  { route: "/operations/configuration", title: "Configuration", summary: "Configure exact local profiles, ports, origins, secrets, and storage choices.", group: "Operate", source: configuration },
  { route: "/operations/storage", title: "Storage and recovery", summary: "Back up, restore, transfer, and verify SQLite and PostgreSQL authority state.", group: "Operate", source: storage },
  { route: "/operations/observability", title: "Observability", summary: "Use probes, diagnostics, audit evidence, and bounded logs without leaking authority material.", group: "Operate", source: observability },
  { route: "/reference/capabilities", title: "Capability explorer", summary: "Filter the complete implementation and roadmap inventory by area, interface, and status.", group: "Reference", source: "Interactive inventory of WorldStream capabilities and their current implementation status." },
  { route: "/reference/cli", title: "CLI reference", summary: "Find the daemon, administration, Supervisor, MCP, and Agent Host command surfaces.", group: "Reference", source: cli },
  { route: "/reference/http-api", title: "HTTP and WebSocket API", summary: "Understand the live API families, authentication boundary, concurrency, and streaming behavior.", group: "Reference", source: httpApi },
  { route: "/reference/repository-map", title: "Repository map", summary: "Locate the kernel, servers, SDKs, web applications, examples, schemas, and verification scripts.", group: "Reference", source: repositoryMap },
  { route: "/reference/glossary", title: "Glossary", summary: "Look up the exact language used by the kernel, Studio, storage, and agent boundaries.", group: "Reference", source: glossary },
  { route: "/maintainers/workflow", title: "Maintainer workflow", summary: "Make repository changes without breaking generated mirrors, compatibility, or local policy.", group: "Maintain", source: maintenance },
  { route: "/maintainers/testing-release", title: "Testing and release", summary: "Run the focused and full gates and prepare evidence for the eventual public v0.1.", group: "Maintain", source: testingRelease },
  { route: "/maintainers/troubleshooting", title: "Troubleshooting", summary: "Diagnose common build, authority, Studio, agent, storage, and recovery failures.", group: "Maintain", source: troubleshooting },
  { route: "/maintainers/manual", title: "Manual maintenance", summary: "Edit, verify, build, and publish this developer manual through GitHub or GitLab Pages.", group: "Maintain", source: manualMaintenance },
] as const;

export const navigationGroups = Array.from(new Set(manualPages.map((page) => page.group))).map((group) => ({
  label: group,
  items: manualPages.filter((page) => page.group === group),
}));
