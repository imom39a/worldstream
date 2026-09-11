import { readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputPath = join(
  repositoryRoot,
  "web/platform/src/hosted-artifacts.generated.ts",
);
const sources = {
  midnightArchiveListing: "config/hosted/listings/midnight-archive-0.4.0.json",
  retainedMidnightArchiveListing03: "config/hosted/listings/midnight-archive-0.3.0.json",
  retainedMidnightArchiveListing02: "config/hosted/listings/midnight-archive-0.2.0.json",
  retainedMidnightArchiveListing01: "config/hosted/listings/midnight-archive-0.1.0.json",
  midnightArchiveResultProjector: "config/hosted/result-projectors/midnight-archive-0.2.0.json",
  retainedMidnightArchiveResultProjector01: "config/hosted/result-projectors/midnight-archive-0.1.0.json",
  midnightArchivePublicProjectionSchema: "config/hosted/schemas/midnight-archive-public-projection-v2.schema.json",
  retainedMidnightArchivePublicProjectionSchema01: "config/hosted/schemas/midnight-archive-public-projection-v1.schema.json",
  midnightArchiveTerminalSummarySchema: "config/hosted/schemas/midnight-archive-terminal-summary-v1.schema.json",
  agentHeistListing: "config/hosted/listings/agent-heist-0.28.0.json",
  retainedAgentHeistListing027: "config/hosted/listings/agent-heist-0.27.0.json",
  retainedAgentHeistListing026: "config/hosted/listings/agent-heist-0.26.0.json",
  retainedAgentHeistListing025: "config/hosted/listings/agent-heist-0.25.0.json",
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
  midnightArchiveMira: "config/hosted/house-agents/mira-1.json",
  midnightArchiveJonah: "config/hosted/house-agents/jonah-1.json",
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
    return { name, path, value, base64: Buffer.from(canonical).toString("base64") };
  }),
);

function oneArtifact(predicate, kind) {
  const matches = entries.filter(predicate);
  if (matches.length !== 1) throw new Error(`expected one exact ${kind} artifact`);
  return `${matches[0].name}Base64`;
}

// Reviewed schema correspondence. Runtime registration independently checks
// canonical digests, so this lookup cannot grant compatibility or approval.
const schemaArtifacts = {
  "agent-heist/projection/v1": "agentHeistPublicProjectionSchema",
  "worldstream/result-summary/v1": "resultSummarySchema",
  "worldstream.midnight-archive/public-projection/v3": "retainedMidnightArchivePublicProjectionSchema01",
  "worldstream.midnight-archive/public-projection/v5": "midnightArchivePublicProjectionSchema",
  "midnight-archive/terminal-summary/v1": "midnightArchiveTerminalSummarySchema",
};

const projectorBundles = entries.filter(({ path }) => path.includes("/listings/")).map(({ name, value: listing }) => {
  const projectorEntry = entries.find(({ value }) => value.projector_id === listing.result.projector.id && value.version === listing.result.projector.version);
  if (projectorEntry === undefined) throw new Error(`missing projector for ${name}`);
  const projector = projectorEntry.value;
  const projectionName = oneArtifact(({ name }) => name === schemaArtifacts[listing.result.projection.schema], "projection");
  const outputName = oneArtifact(({ name }) => name === schemaArtifacts[projector.output.schema], "output");
  const runtimeName = oneArtifact(({ value }) => value.runtime_id === projector.runtime.id && value.version === projector.runtime.version, "runtime");
  return { name, projectorName: `${projectorEntry.name}Base64`, projectionName, outputName, runtimeName };
});

const generated = `// Generated by scripts/generate-hosted-platform-artifacts.mjs.
// Edit the reviewed files under config/hosted and regenerate this module.
${entries
  .map(
    ({ name, path, base64 }) =>
      `/** Canonical bytes copied from ${path}. */\nexport const ${name}Base64 = ${JSON.stringify(base64)};`,
  )
  .join("\n\n")}

/** Exact artifact correspondences for every retained reviewed Listing. */
export const hostedProjectorArtifactBundles = [
${projectorBundles.map(({ name, projectorName, projectionName, outputName, runtimeName }) =>
  `  { listingBase64: ${name}Base64, projectorBase64: ${projectorName}, runtimeBase64: ${runtimeName}, projectionSchemaBase64: ${projectionName}, outputSchemaBase64: ${outputName} },`
).join("\n")}
] as const;
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
