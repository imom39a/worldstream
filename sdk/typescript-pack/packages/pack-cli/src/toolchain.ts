import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { finalizeArtifacts, generateSemanticArtifacts } from "./artifacts.js";
import {
  finalizePackBundleTranscript,
  inspectPackBundle,
  writePackBundle,
} from "./bundle.js";
import { canonicalBytes, type JsonValue } from "./canonical.js";
import { checkTypeScript } from "./compiler.js";
import { runBehavioralConformance } from "./conformance.js";
import {
  EXECUTION_PROFILE_ID,
  HOST_CONTRACT_ID,
  OPERATION_CODEC_ID,
  REQUIRED_COMPONENT_EXPORTS,
  TOOLCHAIN_VERSIONS,
} from "./constants.js";
import { buildComponent } from "./component.js";
import { PackCliError, diagnostic } from "./diagnostics.js";
import { loadProject } from "./project.js";
import {
  createPromptReview,
  promotePromptReview,
  type PromptCreateOptions,
  type PromptPromotionReceipt,
  type PromptReviewReceipt,
} from "./prompt.js";
import { scaffoldProject } from "./scaffold.js";

export interface CheckReceipt {
  readonly command: "check";
  readonly contract: typeof HOST_CONTRACT_ID;
  readonly diagnostics: readonly [];
  readonly entrypoint: string;
  readonly operationCodec: typeof OPERATION_CODEC_ID;
  readonly status: "passed";
  readonly toolchain: typeof TOOLCHAIN_VERSIONS;
  readonly wit: string;
}

export interface TestReceipt {
  readonly acceptedActions: number;
  readonly command: "test";
  readonly declaredRejections: number;
  readonly deterministicRestart: boolean;
  readonly packId: string;
  readonly privateViewsDistinct: boolean;
  readonly status: "passed";
  readonly transcriptDigest: string;
}

export interface BuildReceipt {
  readonly bundle: string;
  readonly command: "build";
  readonly componentBytes: number;
  readonly componentExports: readonly string[];
  readonly componentImports: readonly string[];
  readonly executionProfile: typeof EXECUTION_PROFILE_ID;
  readonly packId: string;
  readonly revisionDigest: string;
  readonly status: "passed";
}

export interface InspectReceipt {
  readonly bundle: string;
  readonly bundleDigest: string;
  readonly command: "inspect";
  readonly members: readonly string[];
  readonly revisionDigest: string;
  readonly size: number;
  readonly status: "passed";
}

export interface CompleteProductionHostProof {
  readonly accepted_action: boolean;
  readonly bundle_digest: string;
  readonly declared_rejection: boolean;
  readonly pack_id: string;
  readonly private_views: number;
  readonly proof_type: "complete";
  readonly retained_old_revision: boolean;
  readonly revision_digest: string;
  readonly room_id: string;
  readonly roles: number;
  readonly status: "passed";
  readonly transcript_digest: string;
}

export interface NeedsFinalizationProductionHostProof {
  readonly actual_transcript_digest: string;
  readonly bundle_digest: string;
  readonly expected_transcript_digest: string;
  readonly pack_id: string;
  readonly proof_type: "needs_finalization";
  readonly revision_digest: string;
  readonly status: "needs_finalization";
}

export type ProductionHostProof =
  | CompleteProductionHostProof
  | NeedsFinalizationProductionHostProof;

export interface ProofReceipt {
  readonly acceptedAction: boolean;
  readonly bundleDigest: string;
  readonly command: "prove";
  readonly conformancePassed: boolean;
  readonly declaredRejection: boolean;
  readonly packId: string;
  readonly privateViews: number;
  readonly productionHost: "verified";
  readonly retainedOldRevision: boolean;
  readonly revisionDigest: string;
  readonly roles: number;
  readonly roomId: string;
  readonly status: "passed";
  readonly transcriptDigest: string;
}

export interface ToolchainOptions {
  readonly productionHostProver?: (bundlePath: string) => Promise<ProductionHostProof>;
}

export class WorldStreamPackToolchain {
  readonly #options: ToolchainOptions;

  constructor(options: ToolchainOptions = {}) {
    this.#options = options;
  }

  async new(directory: string): Promise<{ readonly command: "new"; readonly directory: string; readonly status: "created" }> {
    return { command: "new", directory: await scaffoldProject(directory), status: "created" };
  }

  async createPrompt(
    directory: string,
    options: PromptCreateOptions,
  ): Promise<PromptReviewReceipt> {
    return createPromptReview(directory, options);
  }

  async confirmPrompt(
    directory: string,
    reviewId: string,
  ): Promise<PromptPromotionReceipt> {
    return promotePromptReview(directory, reviewId);
  }

  async check(projectDirectory = process.cwd()): Promise<CheckReceipt> {
    const project = await loadProject(projectDirectory);
    await checkTypeScript(project);
    const generatedDir = join(project.root, ".worldstream", "generated");
    await mkdir(generatedDir, { recursive: true });
    const witSource = fileURLToPath(new URL("../assets/worldstream-activity-pack.wit", import.meta.url));
    const witTarget = join(generatedDir, "worldstream-activity-pack.wit");
    await writeFile(witTarget, await readFile(witSource));
    return {
      command: "check",
      contract: HOST_CONTRACT_ID,
      diagnostics: [],
      entrypoint: project.entrypoint,
      operationCodec: OPERATION_CODEC_ID,
      status: "passed",
      toolchain: TOOLCHAIN_VERSIONS,
      wit: witTarget,
    };
  }

  async test(projectDirectory = process.cwd()): Promise<TestReceipt> {
    const project = await loadProject(projectDirectory);
    const evidence = await runBehavioralConformance(project);
    return {
      acceptedActions: evidence.acceptedActions,
      command: "test",
      declaredRejections: evidence.declaredRejections,
      deterministicRestart: evidence.deterministicRestart,
      packId: evidence.packId,
      privateViewsDistinct: evidence.privateViewsDistinct,
      status: "passed",
      transcriptDigest: evidence.transcriptDigest,
    };
  }

  async build(projectDirectory = process.cwd()): Promise<BuildReceipt> {
    const project = await loadProject(projectDirectory);
    const evidence = await runBehavioralConformance(project);
    const semantic = generateSemanticArtifacts(evidence);
    const generatedDir = join(project.root, ".worldstream", "generated");
    const component = await buildComponent(
      project,
      generatedDir,
      semantic.descriptorContent,
      semantic.schemaIds,
      semantic.actionDigests,
    );
    const finalized = finalizeArtifacts(evidence, semantic, component.bytes);
    const members = new Map<string, Uint8Array>([
      ["codec-bundle.json", canonicalBytes(semantic.codecBundle)],
      ["conformance.json", canonicalBytes(finalized.conformance)],
      ["dependency-lock.json", canonicalBytes(semantic.dependencyLock)],
      ["descriptor.json", canonicalBytes(finalized.descriptor)],
      ["executor.component.wasm", component.bytes],
      ["golden-corpus.json", canonicalBytes(finalized.goldenCorpus)],
      ["revision-lock.json", canonicalBytes(finalized.revisionLock)],
      ["schemas.json", canonicalBytes(semantic.schemas)],
    ]);
    const bundle = writePackBundle(members, finalized.revisionDigest);
    await mkdir(dirname(project.output), { recursive: true });
    await writeFile(project.output, bundle);
    for (const [name, bytes] of members) await writeFile(join(generatedDir, name), bytes);
    const receipt: BuildReceipt = {
      bundle: project.output,
      command: "build",
      componentBytes: component.bytes.length,
      componentExports: component.exports,
      componentImports: component.imports,
      executionProfile: EXECUTION_PROFILE_ID,
      packId: evidence.packId,
      revisionDigest: finalized.revisionDigest,
      status: "passed",
    };
    await writeReceipt(join(project.root, ".worldstream", "build-receipt.json"), receipt);
    return receipt;
  }

  async inspect(bundleOrProject = process.cwd()): Promise<InspectReceipt> {
    const bundle = bundleOrProject.endsWith(".wspack")
      ? resolve(bundleOrProject)
      : (await loadProject(bundleOrProject)).output;
    const inspection = await inspectPackBundle(bundle);
    return {
      bundle,
      bundleDigest: inspection.bundleDigest,
      command: "inspect",
      members: inspection.memberNames,
      revisionDigest: inspection.manifest.revision_digest,
      size: inspection.size,
      status: "passed",
    };
  }

  async prove(projectDirectory = process.cwd()): Promise<ProofReceipt> {
    const project = await loadProject(projectDirectory);
    const build = await this.build(project.root);
    let inspection = await this.inspect(build.bundle);
    const prover = this.#options.productionHostProver ?? productionHostFromEnvironment();
    if (!prover) {
      const pending = {
        bundle: build.bundle,
        command: "prove",
        reason: "production_host_unavailable",
        revisionDigest: build.revisionDigest,
        status: "pending",
      } satisfies JsonValue;
      await writeFile(
        join(project.root, ".worldstream", "proof-receipt.json"),
        canonicalBytes(pending),
      );
      throw new PackCliError(
        diagnostic("WSP-PROVE-001", "Production Component Host proof is unavailable", {
          detail: "The bundle, deterministic corpus, and canonical inspection passed; no production host prover was configured.",
          hint: "Set WORLDSTREAM_PACK_HOST to the production prover executable, then run pack:prove again.",
        }),
      );
    }
    let host = await prover(build.bundle);
    let finalizedTranscript: string | undefined;
    if (host.proof_type === "needs_finalization") {
      validateNeedsFinalizationProof(host, build, inspection);
      finalizedTranscript = host.actual_transcript_digest;
      inspection = await finalizePackBundleTranscript(
        build.bundle,
        build.revisionDigest,
        host.expected_transcript_digest,
        host.actual_transcript_digest,
      ).then((item) => ({
        bundle: build.bundle,
        bundleDigest: item.bundleDigest,
        command: "inspect" as const,
        members: item.memberNames,
        revisionDigest: item.manifest.revision_digest,
        size: item.size,
        status: "passed" as const,
      }));
      host = await prover(build.bundle);
      if (host.proof_type === "needs_finalization") {
        throw new PackCliError(
          diagnostic("WSP-PROVE-006", "Production host requested golden finalization twice", {
            detail: JSON.stringify(host),
            hint: "The second proof must execute the exact finalized bundle without recompiling it.",
          }),
        );
      }
    }
    validateHostProof(host, build, inspection, finalizedTranscript);
    const receipt: ProofReceipt = {
      acceptedAction: host.accepted_action,
      bundleDigest: inspection.bundleDigest,
      command: "prove",
      conformancePassed: true,
      declaredRejection: host.declared_rejection,
      packId: build.packId,
      privateViews: host.private_views,
      productionHost: "verified",
      retainedOldRevision: host.retained_old_revision,
      revisionDigest: build.revisionDigest,
      roles: host.roles,
      roomId: host.room_id,
      status: "passed",
      transcriptDigest: host.transcript_digest,
    };
    await writeReceipt(join(project.root, ".worldstream", "first-success-receipt.json"), receipt);
    return receipt;
  }
}

function productionHostFromEnvironment(): ((bundlePath: string) => Promise<ProductionHostProof>) | undefined {
  const executable = process.env.WORLDSTREAM_PACK_HOST;
  if (!executable) return undefined;
  return async (bundlePath) => {
    const output = await collectProcess(executable, ["pack", "prove", bundlePath, "--json"]);
    let parsed: unknown;
    try {
      parsed = JSON.parse(output);
    } catch (error) {
      throw new PackCliError(
        diagnostic("WSP-PROVE-002", "Production host returned a non-JSON proof", {
          detail: error instanceof Error ? error.message : String(error),
        }),
      );
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new PackCliError(
        diagnostic("WSP-PROVE-002", "Production host returned a non-object proof"),
      );
    }
    return parsed as ProductionHostProof;
  };
}

function validateNeedsFinalizationProof(
  proof: NeedsFinalizationProductionHostProof,
  build: BuildReceipt,
  inspection: InspectReceipt,
): void {
  const keys = [
    "actual_transcript_digest",
    "bundle_digest",
    "expected_transcript_digest",
    "pack_id",
    "proof_type",
    "revision_digest",
    "status",
  ];
  if (
    !hasExactKeys(proof, keys) ||
    proof.status !== "needs_finalization" ||
    proof.pack_id !== build.packId ||
    proof.revision_digest !== build.revisionDigest ||
    proof.bundle_digest !== inspection.bundleDigest ||
    !isTaggedBlake3(proof.expected_transcript_digest) ||
    !isTaggedBlake3(proof.actual_transcript_digest) ||
    proof.expected_transcript_digest === proof.actual_transcript_digest
  ) {
    throw new PackCliError(
      diagnostic("WSP-PROVE-007", "Production host finalization proof is invalid", {
        detail: JSON.stringify(proof),
      }),
    );
  }
}

function validateHostProof(
  proof: CompleteProductionHostProof,
  build: BuildReceipt,
  inspection: InspectReceipt,
  finalizedTranscript?: string,
): void {
  const keys = [
    "accepted_action",
    "bundle_digest",
    "declared_rejection",
    "pack_id",
    "private_views",
    "proof_type",
    "retained_old_revision",
    "revision_digest",
    "roles",
    "room_id",
    "status",
    "transcript_digest",
  ];
  if (
    !hasExactKeys(proof, keys) ||
    proof.proof_type !== "complete" ||
    proof.status !== "passed" ||
    proof.pack_id !== build.packId ||
    proof.bundle_digest !== inspection.bundleDigest ||
    proof.revision_digest !== build.revisionDigest ||
    !isTaggedBlake3(proof.transcript_digest) ||
    (finalizedTranscript !== undefined && proof.transcript_digest !== finalizedTranscript) ||
    proof.accepted_action !== true ||
    proof.declared_rejection !== true ||
    proof.private_views < 2 ||
    proof.retained_old_revision !== true ||
    proof.roles < 2 ||
    typeof proof.room_id !== "string" ||
    proof.room_id.length === 0
  ) {
    throw new PackCliError(
      diagnostic("WSP-PROVE-003", "Production host proof is incomplete", {
        detail: JSON.stringify(proof),
        hint: "The smoke must create a real two-Role Room, apply and reject Actions, prove private views, and retain the old revision.",
      }),
    );
  }
}

function hasExactKeys(value: object, expected: readonly string[]): boolean {
  return JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...expected].sort());
}

function isTaggedBlake3(value: unknown): value is string {
  return typeof value === "string" && /^blake3:[0-9a-f]{64}$/u.test(value);
}

async function collectProcess(executable: string, args: readonly string[]): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    const child = spawn(executable, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => (stderr += chunk));
    child.once("error", reject);
    child.once("exit", (code) => {
      if (code === 0) resolve(stdout);
      else reject(new Error(`production host exited ${code ?? "by signal"}: ${stderr.trim()}`));
    });
  }).catch((error: unknown) => {
    throw new PackCliError(
      diagnostic("WSP-PROVE-004", "Production host proof command failed", {
        detail: error instanceof Error ? error.message : String(error),
      }),
    );
  });
}

async function writeReceipt(path: string, receipt: object): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, canonicalBytes(receipt as JsonValue));
}

export const EXPECTED_COMPONENT_EXPORTS = REQUIRED_COMPONENT_EXPORTS;
