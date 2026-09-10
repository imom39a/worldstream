import { readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputPath = join(
  repositoryRoot,
  "web/platform/src/hosted-artifacts.generated.ts",
);
const sources = {
  agentHeistListing: "config/hosted/listings/agent-heist-0.25.0.json",
  retainedAgentHeistListing024: "config/hosted/listings/agent-heist-0.24.0.json",
  retainedAgentHeistListing023: "config/hosted/listings/agent-heist-0.23.0.json",
  retainedAgentHeistListing022: "config/hosted/listings/agent-heist-0.22.0.json",
  retainedAgentHeistListing021: "config/hosted/listings/agent-heist-0.21.0.json",
  retainedAgentHeistListing020: "config/hosted/listings/agent-heist-0.20.0.json",
  retainedAgentHeistListing019: "config/hosted/listings/agent-heist-0.19.0.json",
  retainedAgentHeistListing018: "config/hosted/listings/agent-heist-0.18.0.json",
  retainedAgentHeistListing017: "config/hosted/listings/agent-heist-0.17.0.json",
  retainedAgentHeistListing016: "config/hosted/listings/agent-heist-0.16.0.json",
  retainedAgentHeistListing015: "config/hosted/listings/agent-heist-0.15.0.json",
  retainedAgentHeistListing014: "config/hosted/listings/agent-heist-0.14.0.json",
  retainedAgentHeistListing013: "config/hosted/listings/agent-heist-0.13.0.json",
  retainedAgentHeistListing012: "config/hosted/listings/agent-heist-0.12.0.json",
  retainedAgentHeistListing011: "config/hosted/listings/agent-heist-0.11.0.json",
  retainedAgentHeistListing010: "config/hosted/listings/agent-heist-0.10.0.json",
  retainedAgentHeistListing09: "config/hosted/listings/agent-heist-0.9.0.json",
  retainedAgentHeistListing08: "config/hosted/listings/agent-heist-0.8.0.json",
  retainedAgentHeistListing07: "config/hosted/listings/agent-heist-0.7.0.json",
  retainedAgentHeistListing06: "config/hosted/listings/agent-heist-0.6.0.json",
  retainedAgentHeistListing02: "config/hosted/listings/agent-heist-0.2.0.json",
  retainedAgentHeistListing03: "config/hosted/listings/agent-heist-0.3.0.json",
  retainedAgentHeistListing04: "config/hosted/listings/agent-heist-0.4.0.json",
  retainedAgentHeistListing05: "config/hosted/listings/agent-heist-0.5.0.json",
  cooperativePlanner: "config/hosted/house-agents/cooperative-planner-17.json",
  retainedCooperativePlanner16: "config/hosted/house-agents/cooperative-planner-16.json",
  retainedCooperativePlanner15: "config/hosted/house-agents/cooperative-planner-15.json",
  retainedCooperativePlanner14: "config/hosted/house-agents/cooperative-planner-14.json",
  retainedCooperativePlanner13: "config/hosted/house-agents/cooperative-planner-13.json",
  retainedCooperativePlanner12: "config/hosted/house-agents/cooperative-planner-12.json",
  retainedCooperativePlanner11: "config/hosted/house-agents/cooperative-planner-11.json",
  retainedCooperativePlanner10: "config/hosted/house-agents/cooperative-planner-10.json",
  retainedCooperativePlanner9: "config/hosted/house-agents/cooperative-planner-9.json",
  retainedCooperativePlanner8: "config/hosted/house-agents/cooperative-planner-8.json",
  retainedCooperativePlanner7: "config/hosted/house-agents/cooperative-planner-7.json",
  retainedCooperativePlanner6: "config/hosted/house-agents/cooperative-planner-6.json",
  retainedCooperativePlanner5: "config/hosted/house-agents/cooperative-planner-5.json",
  retainedCooperativePlanner4: "config/hosted/house-agents/cooperative-planner-4.json",
  retainedCooperativePlanner3: "config/hosted/house-agents/cooperative-planner-3.json",
  retainedCooperativePlanner2: "config/hosted/house-agents/cooperative-planner-2.json",
  retainedCooperativePlanner1: "config/hosted/house-agents/cooperative-planner-1.json",
  skepticalAuditor: "config/hosted/house-agents/skeptical-auditor-16.json",
  retainedSkepticalAuditor15: "config/hosted/house-agents/skeptical-auditor-15.json",
  retainedSkepticalAuditor14: "config/hosted/house-agents/skeptical-auditor-14.json",
  retainedSkepticalAuditor13: "config/hosted/house-agents/skeptical-auditor-13.json",
  retainedSkepticalAuditor12: "config/hosted/house-agents/skeptical-auditor-12.json",
  retainedSkepticalAuditor11: "config/hosted/house-agents/skeptical-auditor-11.json",
  retainedSkepticalAuditor10: "config/hosted/house-agents/skeptical-auditor-10.json",
  retainedSkepticalAuditor9: "config/hosted/house-agents/skeptical-auditor-9.json",
  retainedSkepticalAuditor8: "config/hosted/house-agents/skeptical-auditor-8.json",
  retainedSkepticalAuditor7: "config/hosted/house-agents/skeptical-auditor-7.json",
  retainedSkepticalAuditor6: "config/hosted/house-agents/skeptical-auditor-6.json",
  retainedSkepticalAuditor5: "config/hosted/house-agents/skeptical-auditor-5.json",
  retainedSkepticalAuditor4: "config/hosted/house-agents/skeptical-auditor-4.json",
  retainedSkepticalAuditor3: "config/hosted/house-agents/skeptical-auditor-3.json",
  retainedSkepticalAuditor2: "config/hosted/house-agents/skeptical-auditor-2.json",
  retainedSkepticalAuditor1: "config/hosted/house-agents/skeptical-auditor-1.json",
  agentHeistResultProjector:
    "config/hosted/result-projectors/agent-heist-0.5.0.json",
  retainedAgentHeistResultProjector04:
    "config/hosted/result-projectors/agent-heist-0.4.0.json",
  retainedAgentHeistResultProjector03:
    "config/hosted/result-projectors/agent-heist-0.3.0.json",
  retainedAgentHeistResultProjector02:
    "config/hosted/result-projectors/agent-heist-0.2.0.json",
  declarativeResultProjectorRuntime:
    "config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json",
  agentHeistPublicProjectionSchema:
    "config/hosted/schemas/agent-heist-public-projection-v1.schema.json",
  resultSummarySchema: "config/hosted/schemas/result-summary-v1.schema.json",
};

const entries = await Promise.all(
  Object.entries(sources).map(async ([name, path]) => {
    const value = JSON.parse(await readFile(join(repositoryRoot, path), "utf8"));
    const canonical = canonicalStringify(value);
    return { name, path, base64: Buffer.from(canonical).toString("base64") };
  }),
);

const generated = `// Generated by scripts/generate-hosted-platform-artifacts.mjs.
// Edit the reviewed files under config/hosted and regenerate this module.
${entries
  .map(
    ({ name, path, base64 }) =>
      `/** Canonical bytes copied from ${path}. */\nexport const ${name}Base64 = ${JSON.stringify(base64)};`,
  )
  .join("\n\n")}
`;

if (process.argv.includes("--check")) {
  const current = await readFile(outputPath, "utf8").catch(() => "");
  if (current !== generated) {
    throw new Error("hosted platform artifacts are stale; run pnpm hosted-artifacts:generate");
  }
} else {
  await writeFile(outputPath, generated);
}

function canonicalStringify(value) {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value)) throw new Error("non-canonical number");
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalStringify).join(",")}]`;
  if (typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalStringify(value[key])}`)
      .join(",")}}`;
  }
  throw new Error("unsupported JSON value");
}
