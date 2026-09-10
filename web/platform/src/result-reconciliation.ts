import { createHash } from "node:crypto";
import { reportPlatformFailure } from "./diagnostics.js";

import {
  ContractViolation,
  projectResult,
  readListingRevision,
  readResultProjectorRevision,
  resolveProjectorArtifacts,
  verifyListingPack,
  verifyListingProjector,
  type ListingRevision,
  type PackReference,
  type ResolvedResultProjector,
} from "@worldstream/hosted-contract";
import {
  decodeCanonical,
  encodeCanonical,
  taggedBlake3,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

const MAX_EVIDENCE_BYTES = 262_144;
const MAX_GATEWAY_RESPONSE_BYTES = 384 * 1024;
const MAX_JSON_NODES = 4_096;
const MAX_JSON_DEPTH = 32;
const SHA256_PATTERN = /^sha256:[0-9a-f]{64}$/u;
const BLAKE3_PATTERN = /^blake3:[0-9a-f]{64}$/u;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const ULID_PATTERN = /^[0-9A-HJKMNP-TV-Z]{26}$/u;
const REFERENCE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;

export type ResultIntegrityStatus = "healthy" | "faulted" | "quarantined";

export interface ResultSourceHead {
  readonly room_id: string;
  readonly room_seq: number;
  readonly genesis_or_transition_hash: string;
  readonly core_schema_version: "worldstream.core-room-state.v1";
  readonly pack_digest: string;
  readonly core_state_hash: string;
  readonly activity_state_hash: string;
  readonly authoritative_state_hash: string;
}

export interface AuthorizedPublicProjection {
  readonly projection_schema: string;
  readonly authorized_core: CanonicalJson;
  readonly projection: CanonicalJson;
  readonly action_offers: readonly CanonicalJson[];
}

export interface ResultReplayEvidence {
  readonly verifier_revision: "worldstream.authorized-replay/v1";
  readonly verified_head: ResultSourceHead;
  readonly projection_hash: string;
  readonly verification_receipt_digest: string;
}

export interface HostedResultSourceEvidence {
  readonly schema: "worldstream/hosted-result-source-evidence/v1";
  readonly host_installation_id: string;
  readonly launch_request_id: string;
  readonly run_id: string;
  readonly listing_revision_digest: string;
  readonly launch_request_digest: string;
  readonly room_setup_operation_id: string;
  readonly room_id: string;
  readonly pack: PackReference;
  readonly result_indexer_membership_id: string;
  readonly access_mode: "spectator";
  readonly source_head: ResultSourceHead;
  readonly integrity_status: ResultIntegrityStatus;
  readonly integrity_generation: number;
  readonly projection_schema: string;
  readonly public_projection: AuthorizedPublicProjection;
  readonly projection_hash: string;
  readonly replay?: ResultReplayEvidence;
}

export interface ResultSourceRequest {
  readonly schema: "worldstream/hosted-result-source-request/v1";
  readonly run_id: string;
  readonly listing_revision_digest: string;
  readonly launch_request_digest: string;
  readonly room_setup_operation_id: string;
}

export interface ResultReconciliationCandidate {
  readonly candidateKind: "genesis" | "result_source";
  readonly launchRequestId: string;
  readonly runId: string | null;
  readonly listingRevisionDigest: string;
  readonly launchRequestDigest: string | null;
  readonly hostInstallationId: string;
  readonly roomSetupOperationId: string;
}

export interface TerminalReconciliationState {
  readonly terminalRecorded: boolean;
  readonly projectorStatus: "terminal_without_outcome" | "summary" | null;
  readonly reconciliationState: "pending" | "terminal" | "quarantined";
}

export interface ResultReconciliationState {
  readonly resultRecorded: boolean;
  readonly resultPayloadDigest: string | null;
  readonly integrityStatus: ResultIntegrityStatus | null;
  readonly integrityGeneration: number | null;
  readonly publishable: boolean;
}

export interface ReconciliationWriteReceipt {
  readonly disposition: "applied" | "duplicate" | "conflict" | "blocked";
  readonly safeCode: string;
}

/** Exact, server-derived Host cleanup material for one terminal House seat. */
export interface TerminalHouseRunnerRetirementCandidate {
  readonly hostInstallationId: string;
  readonly reservationOperationId: string;
  readonly launchRequestId: string;
  readonly houseAgentAssignmentId: string;
  readonly terminalEvidenceDigest: string;
}

/** Exact server-derived cleanup material after immutable pre-start abandonment. */
export interface PrestartHouseRunnerRetirementCandidate {
  readonly hostInstallationId: string;
  readonly reservationOperationId: string;
  readonly launchRequestId: string;
  readonly houseAgentAssignmentId: string;
  readonly abandonmentEvidenceDigest: string;
}

/** The narrow service-only Host request; it is never a browser payload. */
export interface HostedHouseRunnerRetirementRequest {
  readonly schema: "worldstream/house-runner-retirement-request/v1";
  readonly host_installation_id: string;
  readonly reservation_operation_id: string;
  readonly launch_request_id: string;
  readonly house_agent_assignment_id: string;
  readonly disposition: "run_terminal" | "pre_start_abandoned";
  readonly platform_evidence_digest: string;
}

export interface ResultReconciliationData {
  listCandidates(limit: number): Promise<readonly ResultReconciliationCandidate[]>;
  /**
   * Releases only active-Run reservations whose exact Run already has
   * immutable terminal evidence. This repairs retained coordination state;
   * it neither observes a Room nor classifies an Outcome.
   */
  reconcileTerminalActivityCapacity(limit: number): Promise<number>;
  /**
   * Returns terminal Runs whose exact House retirement receipts are still
   * absent. This is intentionally separate from result indexing: a Host
   * outage must not make a published result disappear from automatic retry.
   */
  listTerminalHouseRunnerRetirementRuns(limit: number): Promise<readonly string[]>;
  /** Independent retry lane; it remains eligible after result-free abandonment. */
  listPrestartHouseRunnerRetirementRuns(limit: number): Promise<readonly string[]>;
  /** DB-time-selected post-Genesis, pre-Lobby-abandonment candidates. */
  listPrestartAbandonmentLaunches(limit: number): Promise<readonly string[]>;
  markAttempt(launchRequestId: string): Promise<void>;
  readTerminal(runId: string): Promise<TerminalReconciliationState | null>;
  readResult(runId: string): Promise<ResultReconciliationState | null>;
  recordTerminal(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt>;
  recordTerminalConflict(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt>;
  readTerminalHouseRunnerRetirements(
    runId: string,
  ): Promise<readonly TerminalHouseRunnerRetirementCandidate[]>;
  recordTerminalHouseRunnerRetirement(input: {
    readonly runId: string;
    readonly candidate: TerminalHouseRunnerRetirementCandidate;
    readonly canonicalReceipt: Uint8Array;
    readonly receiptDigest: Uint8Array;
  }): Promise<boolean>;
  readPrestartHouseRunnerRetirements(
    runId: string,
  ): Promise<readonly PrestartHouseRunnerRetirementCandidate[]>;
  recordPrestartHouseRunnerRetirement(input: {
    readonly runId: string;
    readonly candidate: PrestartHouseRunnerRetirementCandidate;
    readonly canonicalReceipt: Uint8Array;
    readonly receiptDigest: Uint8Array;
  }): Promise<boolean>;
  recordResult(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
    payload: Uint8Array,
    payloadDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt>;
  recordIntegrity(
    runId: string,
    evidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<ReconciliationWriteReceipt>;
}

export interface HostedResultSourceClient {
  readResultSource(request: ResultSourceRequest): Promise<unknown>;
}

export interface HostedHouseRetirementClient {
  retireHouseRunner(request: HostedHouseRunnerRetirementRequest): Promise<CanonicalObject>;
}

export interface PinnedProjectorArtifactBundle {
  readonly listingBytes: Uint8Array;
  readonly projectorBytes: Uint8Array;
  readonly runtimeBytes: Uint8Array;
  readonly projectionSchemaBytes: Uint8Array;
  readonly outputSchemaBytes: Uint8Array;
}

interface PinnedProjector {
  readonly listing: ListingRevision;
  readonly resolved: ResolvedResultProjector;
}

/** Process-start registry of exact, reviewed projector artifacts. */
export class PinnedResultProjectorRegistry {
  readonly #byListing = new Map<string, PinnedProjector>();

  constructor(bundles: readonly PinnedProjectorArtifactBundle[]) {
    if (bundles.length === 0 || bundles.length > 64) {
      throw new ResultReconciliationRejectedError("invalid_projector_registry");
    }
    for (const bundle of bundles) {
      try {
        const listing = readListingRevision(bundle.listingBytes);
        const projector = readResultProjectorRevision(bundle.projectorBytes);
        verifyListingProjector(listing, projector);
        const resolved = resolveProjectorArtifacts(
          projector,
          bundle.runtimeBytes,
          bundle.projectionSchemaBytes,
          bundle.outputSchemaBytes,
        );
        if (this.#byListing.has(listing.digest)) {
          throw new ResultReconciliationRejectedError("duplicate_listing_projector");
        }
        this.#byListing.set(listing.digest, { listing, resolved });
      } catch (error) {
        if (error instanceof ResultReconciliationRejectedError) throw error;
        throw new ResultReconciliationRejectedError("invalid_projector_artifact", { cause: error });
      }
    }
  }

  resolve(listingRevisionDigest: string): PinnedProjector {
    const resolved = this.#byListing.get(listingRevisionDigest);
    if (resolved === undefined) {
      throw new ResultReconciliationUnavailableError("projector_artifact_unavailable");
    }
    return resolved;
  }
}

export class ResultReconciliationRejectedError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "ResultReconciliationRejectedError";
  }
}

export class ResultReconciliationUnavailableError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "ResultReconciliationUnavailableError";
  }
}

/** Literal service-only client for the Fly result-source route. */
export class HttpHostedResultSourceClient implements HostedResultSourceClient {
  readonly #endpoint: URL;
  readonly #serviceAuthority: string;
  readonly #timeoutMs: number;
  readonly #fetch: typeof fetch;

  constructor(input: {
    baseUrl: string;
    serviceAuthority: string;
    timeoutMs?: number;
    fetchImplementation?: typeof fetch;
  }) {
    const base = new URL(input.baseUrl);
    const local = base.hostname === "localhost" || base.hostname === "127.0.0.1";
    if (
      (!local && base.protocol !== "https:")
      || (local && !["http:", "https:"].includes(base.protocol))
      || base.username !== ""
      || base.password !== ""
      || base.search !== ""
      || base.hash !== ""
      || input.serviceAuthority.length < 32
      || input.serviceAuthority.length > 512
      || /\s/u.test(input.serviceAuthority)
    ) {
      throw new ResultReconciliationRejectedError("invalid_result_source_configuration");
    }
    // Outlast the Gateway's 25-second Controller budget, which contains both
    // the current Projection and Replay reads. This is not a health probe.
    const timeoutMs = input.timeoutMs ?? 30_000;
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 100 || timeoutMs > 30_000) {
      throw new ResultReconciliationRejectedError("invalid_result_source_timeout");
    }
    this.#endpoint = new URL("/v1/hosted/result-source-evidence", base);
    this.#serviceAuthority = input.serviceAuthority;
    this.#timeoutMs = timeoutMs;
    this.#fetch = input.fetchImplementation ?? fetch;
  }

  async readResultSource(request: ResultSourceRequest): Promise<unknown> {
    let response: Response;
    try {
      response = await this.#fetch(this.#endpoint, {
        method: "POST",
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.#serviceAuthority}`,
          "content-type": "application/json",
        },
        body: Buffer.from(encodeCanonical(request as unknown as CanonicalObject)),
        redirect: "error",
        signal: AbortSignal.timeout(this.#timeoutMs),
      });
    } catch (error) {
      reportPlatformFailure("fly_result_source", error);
      throw new ResultReconciliationUnavailableError("result_source_unavailable", {
        cause: error,
      });
    }
    if (!response.ok) {
      if ([400, 401, 403, 404, 409, 422].includes(response.status)) {
        throw new ResultReconciliationRejectedError("result_source_rejected");
      }
      reportPlatformFailure("fly_result_source", undefined, { status: response.status });
      throw new ResultReconciliationUnavailableError("result_source_unavailable");
    }
    const bytes = await readBoundedBody(response, MAX_GATEWAY_RESPONSE_BYTES);
    try {
      return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as unknown;
    } catch (error) {
      throw new ResultReconciliationRejectedError("invalid_result_source_response", {
        cause: error,
      });
    }
  }
}

/** Literal service-only client for the Fly House-retirement route. */
export class HttpHostedHouseRetirementClient implements HostedHouseRetirementClient {
  readonly #endpoint: URL;
  readonly #serviceAuthority: string;
  readonly #timeoutMs: number;
  readonly #fetch: typeof fetch;

  constructor(input: {
    baseUrl: string;
    serviceAuthority: string;
    timeoutMs?: number;
    fetchImplementation?: typeof fetch;
  }) {
    const base = new URL(input.baseUrl);
    const local = base.hostname === "localhost" || base.hostname === "127.0.0.1";
    if (
      (!local && base.protocol !== "https:")
      || (local && !["http:", "https:"].includes(base.protocol))
      || base.username !== ""
      || base.password !== ""
      || base.search !== ""
      || base.hash !== ""
      || input.serviceAuthority.length < 32
      || input.serviceAuthority.length > 512
      || /\s/u.test(input.serviceAuthority)
    ) {
      throw new ResultReconciliationRejectedError("invalid_house_retirement_configuration");
    }
    const timeoutMs = input.timeoutMs ?? 15_000;
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 100 || timeoutMs > 30_000) {
      throw new ResultReconciliationRejectedError("invalid_house_retirement_timeout");
    }
    this.#endpoint = new URL("/v1/hosted/house-runners/retire", base);
    this.#serviceAuthority = input.serviceAuthority;
    this.#timeoutMs = timeoutMs;
    this.#fetch = input.fetchImplementation ?? fetch;
  }

  async retireHouseRunner(request: HostedHouseRunnerRetirementRequest): Promise<CanonicalObject> {
    let response: Response;
    try {
      response = await this.#fetch(this.#endpoint, {
        method: "POST",
        headers: {
          authorization: `Bearer ${this.#serviceAuthority}`,
          "content-type": "application/json",
          accept: "application/json",
        },
        body: JSON.stringify(request),
        signal: AbortSignal.timeout(this.#timeoutMs),
      });
    } catch (error) {
      throw new ResultReconciliationUnavailableError("house_retirement_unavailable", { cause: error });
    }
    if (!response.ok) {
      if (response.status === 404 || (response.status >= 500 && response.status <= 599)) {
        throw new ResultReconciliationUnavailableError("house_retirement_unavailable");
      }
      throw new ResultReconciliationRejectedError("house_retirement_rejected");
    }
    const bytes = await readBoundedBody(response, MAX_GATEWAY_RESPONSE_BYTES);
    try {
      return record(JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)));
    } catch (error) {
      throw new ResultReconciliationRejectedError("invalid_house_retirement_response", { cause: error });
    }
  }
}

export type ResultReconciliationOutcome =
  | "not_terminal"
  | "terminal_without_outcome"
  | "result_recorded"
  | "result_confirmed"
  | "result_suppressed"
  | "result_reverified"
  | "blocked"
  | "conflict";

export interface ResultReconciliationReport {
  readonly runId: string;
  readonly outcome: ResultReconciliationOutcome;
  readonly safeCode: string;
}

export interface ResultReconcilerDependencies {
  readonly source: HostedResultSourceClient;
  readonly houseRetirement: HostedHouseRetirementClient;
  readonly data: ResultReconciliationData;
  readonly projectors: PinnedResultProjectorRegistry;
}

/** Reconciles one Run without interpreting any Pack-specific field. */
export async function reconcileActivityResult(
  candidate: ResultReconciliationCandidate,
  dependencies: ResultReconcilerDependencies,
): Promise<ResultReconciliationReport> {
  const request = candidateRequest(candidate);
  const [terminal, result, untrustedEvidence] = await Promise.all([
    dependencies.data.readTerminal(request.run_id),
    dependencies.data.readResult(request.run_id),
    dependencies.source.readResultSource(request),
  ]);
  const evidence = readHostedResultSourceEvidence(untrustedEvidence);
  validateEvidenceRequestBinding(evidence, request, candidate);
  const pinned = dependencies.projectors.resolve(evidence.listing_revision_digest);
  try {
    verifyListingPack(pinned.listing, evidence.pack);
  } catch (error) {
    throw new ResultReconciliationRejectedError("result_source_pack_mismatch", { cause: error });
  }
  if (
    pinned.listing.value.result.projection.schema !== evidence.projection_schema
    || pinned.resolved.revision.digest
      !== pinned.listing.value.result.projector.digest
  ) {
    throw new ResultReconciliationRejectedError("result_source_projector_mismatch");
  }
  const projectorInput = encodeCanonical({
    schema: "worldstream/result-projector-input/v1",
    listing_revision_digest: pinned.listing.digest,
    projector_revision_digest: pinned.resolved.revision.digest,
    pack: evidence.pack as unknown as CanonicalObject,
    projection_schema: evidence.projection_schema,
    source_head: evidence.source_head as unknown as CanonicalObject,
    public_projection: evidence.public_projection.projection,
  });
  let projected: ProjectorOutput;
  try {
    projected = readProjectorOutput(projectResult(pinned.listing, pinned.resolved, projectorInput));
  } catch (error) {
    if (error instanceof ContractViolation) {
      throw new ResultReconciliationRejectedError("result_projection_rejected", { cause: error });
    }
    throw error;
  }
  const hostEvidenceBytes = encodeCanonical(evidence as unknown as CanonicalObject);
  const hostEvidenceDigest = taggedSha256(hostEvidenceBytes);

  if (projected.status === "not_terminal") {
    if (terminal?.terminalRecorded !== true) {
      return { runId: request.run_id, outcome: "not_terminal", safeCode: "not_terminal" };
    }
    const conflict = canonicalDocument({
      schema: "worldstream/platform-terminal-conflict/v1",
      run_id: request.run_id,
      listing_revision_digest: evidence.listing_revision_digest,
      result_projector_revision_digest: pinned.resolved.revision.digest,
      conflict_code: "nonterminal_reversion",
      source_head: evidence.source_head as unknown as CanonicalObject,
      integrity_status: evidence.integrity_status,
      integrity_generation: evidence.integrity_generation,
      projection_hash: evidence.projection_hash,
      host_evidence_digest: hostEvidenceDigest.tagged,
    });
    const receipt = await dependencies.data.recordTerminalConflict(
      request.run_id,
      conflict.bytes,
      conflict.digest,
    );
    return report(request.run_id, "conflict", receipt);
  }

  const terminalDocument = canonicalDocument({
    schema: "worldstream/platform-terminal-observation/v1",
    run_id: request.run_id,
    listing_revision_digest: evidence.listing_revision_digest,
    result_projector_revision_digest: pinned.resolved.revision.digest,
    projector_status: projected.status,
    host_installation_id: evidence.host_installation_id,
    room_id: evidence.room_id,
    result_indexer_membership_id: evidence.result_indexer_membership_id,
    source_head: evidence.source_head as unknown as CanonicalObject,
    integrity_status: evidence.integrity_status,
    integrity_generation: evidence.integrity_generation,
    projection_schema: evidence.projection_schema,
    projection_hash: evidence.projection_hash,
    host_evidence_digest: hostEvidenceDigest.tagged,
  });
  const terminalReceipt = await dependencies.data.recordTerminal(
    request.run_id,
    terminalDocument.bytes,
    terminalDocument.digest,
  );
  if (terminalReceipt.disposition === "conflict") {
    return report(request.run_id, "conflict", terminalReceipt);
  }

  if (projected.status === "terminal_without_outcome") {
    await recordCurrentIntegrity(dependencies.data, evidence, hostEvidenceDigest.tagged, null);
    await attemptTerminalHouseRunnerRetirement(request.run_id, dependencies);
    return {
      runId: request.run_id,
      outcome: "terminal_without_outcome",
      safeCode: terminalReceipt.safeCode,
    };
  }

  const summary = canonicalDocument(projected.summary);
  const replayVerified = evidence.integrity_status === "healthy" && evidence.replay !== undefined;
  if (!replayVerified) {
    const integrityReceipt = await recordCurrentIntegrity(
      dependencies.data,
      evidence,
      hostEvidenceDigest.tagged,
      null,
    );
    const outcome = report(
      request.run_id,
      result?.resultRecorded === true ? "result_suppressed" : "blocked",
      integrityReceipt,
    );
    await attemptTerminalHouseRunnerRetirement(request.run_id, dependencies);
    return outcome;
  }

  const summaryDigest = `sha256:${Buffer.from(summary.digest).toString("hex")}`;
  if (result?.resultRecorded === true) {
    if (result.resultPayloadDigest === summaryDigest) {
      const integrityReceipt = await recordCurrentIntegrity(
        dependencies.data,
        evidence,
        hostEvidenceDigest.tagged,
        summaryDigest,
      );
      const outcome = integrityReceipt.safeCode === "result_reverified"
        ? "result_reverified"
        : "result_confirmed";
      const reportValue = report(request.run_id, outcome, integrityReceipt);
      await attemptTerminalHouseRunnerRetirement(request.run_id, dependencies);
      return reportValue;
    }
  }

  const replay = evidence.replay;
  if (replay === undefined) {
    throw new ResultReconciliationRejectedError("replay_evidence_missing");
  }
  const resultDocument = canonicalDocument({
    schema: "worldstream/platform-indexed-result-evidence/v1",
    run_id: request.run_id,
    listing_revision_digest: evidence.listing_revision_digest,
    result_projector_revision_digest: pinned.resolved.revision.digest,
    result_output_schema: pinned.resolved.revision.value.output.schema,
    result_output_schema_digest: pinned.resolved.revision.value.output.schema_digest,
    result_canonicalizer_version: pinned.resolved.revision.value.output.canonicalizer,
    result_indexer_membership_id: evidence.result_indexer_membership_id,
    source_head: evidence.source_head as unknown as CanonicalObject,
    integrity_status: evidence.integrity_status,
    integrity_generation: evidence.integrity_generation,
    projection_hash: evidence.projection_hash,
    replay_verifier_revision: replay.verifier_revision,
    replay_verified_head: replay.verified_head as unknown as CanonicalObject,
    replay_projection_hash: replay.projection_hash,
    replay_receipt_digest: replay.verification_receipt_digest,
    host_evidence_digest: hostEvidenceDigest.tagged,
    summary_digest: summaryDigest,
  });
  const resultReceipt = await dependencies.data.recordResult(
    request.run_id,
    resultDocument.bytes,
    resultDocument.digest,
    summary.bytes,
    summary.digest,
  );
  if (resultReceipt.disposition === "conflict") {
    // The new summary conflicts, but the already-retained terminal evidence is
    // still sufficient for exact Host cleanup.
    await attemptTerminalHouseRunnerRetirement(request.run_id, dependencies);
    return report(request.run_id, "conflict", resultReceipt);
  }
  const reportValue = report(
    request.run_id,
    resultReceipt.disposition === "duplicate" ? "result_confirmed" : "result_recorded",
    resultReceipt,
  );
  await attemptTerminalHouseRunnerRetirement(request.run_id, dependencies);
  return reportValue;
}

/**
 * Retires only assignments returned by the terminal-evidence-gated platform
 * read. This belongs after `recordTerminal`: a live Run has no material to
 * send to Fly. A failed call leaves the immutable terminal evidence intact,
 * so a later bounded reconciliation retries the same idempotent request.
 */
async function retireTerminalHouseRunners(
  runId: string,
  dependencies: ResultReconcilerDependencies,
): Promise<void> {
  const candidates = await dependencies.data.readTerminalHouseRunnerRetirements(runId);
  if (candidates.length > 2) {
    throw new ResultReconciliationRejectedError("terminal_house_retirement_limit_exceeded");
  }
  for (const candidate of candidates) {
    const request = retirementRequest(candidate);
    const rawReceipt = await dependencies.houseRetirement.retireHouseRunner(request);
    validateRetirementReceipt(rawReceipt, request);
    const canonicalReceipt = encodeCanonical(rawReceipt);
    const recorded = await dependencies.data.recordTerminalHouseRunnerRetirement({
      runId,
      candidate,
      canonicalReceipt,
      receiptDigest: sha256(canonicalReceipt),
    });
    if (!recorded) {
      throw new ResultReconciliationRejectedError("terminal_house_retirement_record_rejected");
    }
  }
}

/** Same exact Host primitive, with abandonment evidence as the sole release authority. */
async function retirePrestartHouseRunners(
  runId: string,
  dependencies: ResultReconcilerDependencies,
): Promise<void> {
  const candidates = await dependencies.data.readPrestartHouseRunnerRetirements(runId);
  if (candidates.length > 2) {
    throw new ResultReconciliationRejectedError("prestart_house_retirement_limit_exceeded");
  }
  for (const candidate of candidates) {
    const request = prestartRetirementRequest(candidate);
    const rawReceipt = await dependencies.houseRetirement.retireHouseRunner(request);
    validateRetirementReceipt(rawReceipt, request);
    const canonicalReceipt = encodeCanonical(rawReceipt);
    const recorded = await dependencies.data.recordPrestartHouseRunnerRetirement({
      runId,
      candidate,
      canonicalReceipt,
      receiptDigest: sha256(canonicalReceipt),
    });
    if (!recorded) {
      throw new ResultReconciliationRejectedError("prestart_house_retirement_record_rejected");
    }
  }
}

/**
 * Result evidence and its public index are durable before Host cleanup is
 * attempted. A transient Host failure is therefore non-fatal to the visible
 * result. `listTerminalHouseRunnerRetirementRuns` provides the bounded,
 * automatic retry path until the immutable receipt has been recorded.
 */
async function attemptTerminalHouseRunnerRetirement(
  runId: string,
  dependencies: ResultReconcilerDependencies,
): Promise<void> {
  try {
    await retireTerminalHouseRunners(runId, dependencies);
  } catch (error) {
    if (!(error instanceof ResultReconciliationUnavailableError)) throw error;
  }
}

function retirementRequest(
  candidate: TerminalHouseRunnerRetirementCandidate,
): HostedHouseRunnerRetirementRequest {
  if (
    !REFERENCE_PATTERN.test(candidate.hostInstallationId)
    || !UUID_PATTERN.test(candidate.reservationOperationId)
    || !UUID_PATTERN.test(candidate.launchRequestId)
    || !UUID_PATTERN.test(candidate.houseAgentAssignmentId)
    || !SHA256_PATTERN.test(candidate.terminalEvidenceDigest)
  ) {
    throw new ResultReconciliationRejectedError("invalid_terminal_house_retirement_candidate");
  }
  return {
    schema: "worldstream/house-runner-retirement-request/v1",
    host_installation_id: candidate.hostInstallationId,
    reservation_operation_id: candidate.reservationOperationId,
    launch_request_id: candidate.launchRequestId,
    house_agent_assignment_id: candidate.houseAgentAssignmentId,
    disposition: "run_terminal",
    platform_evidence_digest: candidate.terminalEvidenceDigest,
  };
}

function prestartRetirementRequest(
  candidate: PrestartHouseRunnerRetirementCandidate,
): HostedHouseRunnerRetirementRequest {
  if (
    !REFERENCE_PATTERN.test(candidate.hostInstallationId)
    || !UUID_PATTERN.test(candidate.reservationOperationId)
    || !UUID_PATTERN.test(candidate.launchRequestId)
    || !UUID_PATTERN.test(candidate.houseAgentAssignmentId)
    || !SHA256_PATTERN.test(candidate.abandonmentEvidenceDigest)
  ) {
    throw new ResultReconciliationRejectedError("invalid_prestart_house_retirement_candidate");
  }
  return {
    schema: "worldstream/house-runner-retirement-request/v1",
    host_installation_id: candidate.hostInstallationId,
    reservation_operation_id: candidate.reservationOperationId,
    launch_request_id: candidate.launchRequestId,
    house_agent_assignment_id: candidate.houseAgentAssignmentId,
    disposition: "pre_start_abandoned",
    platform_evidence_digest: candidate.abandonmentEvidenceDigest,
  };
}

function validateRetirementReceipt(
  value: CanonicalObject,
  request: HostedHouseRunnerRetirementRequest,
): void {
  exactKeys(value, [
    "schema",
    "host_installation_id",
    "reservation_operation_id",
    "launch_request_id",
    "house_agent_assignment_id",
    "runner_unit_id",
    "disposition",
    "platform_evidence_digest",
    "stop_witness",
    "authentication_tag",
  ]);
  const runnerUnitId = reference(value.runner_unit_id);
  const stopWitness = digest(value.stop_witness, BLAKE3_PATTERN);
  const authenticationTag = hexTag(value.authentication_tag);
  if (
    value.schema !== "worldstream/house-runner-retirement-receipt/v1"
    || value.host_installation_id !== request.host_installation_id
    || value.reservation_operation_id !== request.reservation_operation_id
    || value.launch_request_id !== request.launch_request_id
    || value.house_agent_assignment_id !== request.house_agent_assignment_id
    || value.disposition !== request.disposition
    || value.platform_evidence_digest !== request.platform_evidence_digest
    || !REFERENCE_PATTERN.test(runnerUnitId)
    || !BLAKE3_PATTERN.test(stopWitness)
    || !/^[0-9a-f]{64}$/u.test(authenticationTag)
  ) {
    throw new ResultReconciliationRejectedError("terminal_house_retirement_receipt_mismatch");
  }
}

/** Runs one bounded reconciliation pass and ignores Genesis candidates. */
export async function reconcileActivityResultCandidates(
  dependencies: ResultReconcilerDependencies,
  limit = 100,
): Promise<readonly ResultReconciliationReport[]> {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
    throw new ResultReconciliationRejectedError("invalid_candidate_limit");
  }
  await reconcileTerminalActivityCapacity(dependencies, limit);
  const candidates = await dependencies.data.listCandidates(limit);
  if (candidates.length > limit || candidates.length > 100) {
    throw new ResultReconciliationRejectedError("candidate_limit_exceeded");
  }
  const reports: ResultReconciliationReport[] = [];
  for (const candidate of candidates) {
    if (candidate.candidateKind === "result_source") {
      await dependencies.data.markAttempt(candidate.launchRequestId);
      reports.push(await reconcileActivityResult(candidate, dependencies));
    }
  }
  await reconcileTerminalHouseRunnerRetirementCandidates(dependencies, limit);
  await reconcilePrestartHouseRunnerRetirementCandidates(dependencies, limit);
  return reports;
}

/**
 * Repairs only retained active-Run capacity for Runs with immutable terminal
 * evidence. It is intentionally independent of source polling: a completed
 * result may no longer be a result-source candidate, yet must never retain
 * admission capacity because of an older recovery transition.
 */
export async function reconcileTerminalActivityCapacity(
  dependencies: ResultReconcilerDependencies,
  limit = 100,
): Promise<number> {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
    throw new ResultReconciliationRejectedError("invalid_candidate_limit");
  }
  const released = await dependencies.data.reconcileTerminalActivityCapacity(limit);
  if (!Number.isSafeInteger(released) || released < 0 || released > limit) {
    throw new ResultReconciliationRejectedError("invalid_terminal_capacity_reconciliation_result");
  }
  return released;
}

/** A bounded retry lane for exact House cleanup after result-free abandonment. */
export async function reconcilePrestartHouseRunnerRetirementCandidates(
  dependencies: ResultReconcilerDependencies,
  limit = 100,
): Promise<void> {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
    throw new ResultReconciliationRejectedError("invalid_candidate_limit");
  }
  const retirementRuns = await dependencies.data.listPrestartHouseRunnerRetirementRuns(limit);
  if (retirementRuns.length > limit || retirementRuns.length > 100) {
    throw new ResultReconciliationRejectedError("prestart_house_retirement_candidate_limit_exceeded");
  }
  for (const runId of retirementRuns) {
    if (!UUID_PATTERN.test(runId)) {
      throw new ResultReconciliationRejectedError("invalid_prestart_house_retirement_run");
    }
    try {
      await retirePrestartHouseRunners(runId, dependencies);
    } catch (error) {
      if (!(error instanceof ResultReconciliationUnavailableError)) throw error;
    }
  }
}

/** A bounded independent retry lane for terminal Host cleanup receipts. */
export async function reconcileTerminalHouseRunnerRetirementCandidates(
  dependencies: ResultReconcilerDependencies,
  limit = 100,
): Promise<void> {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) {
    throw new ResultReconciliationRejectedError("invalid_candidate_limit");
  }
  const retirementRuns = await dependencies.data.listTerminalHouseRunnerRetirementRuns(limit);
  if (retirementRuns.length > limit || retirementRuns.length > 100) {
    throw new ResultReconciliationRejectedError("terminal_house_retirement_candidate_limit_exceeded");
  }
  for (const runId of retirementRuns) {
    if (!UUID_PATTERN.test(runId)) {
      throw new ResultReconciliationRejectedError("invalid_terminal_house_retirement_run");
    }
    await attemptTerminalHouseRunnerRetirement(runId, dependencies);
  }
}

function candidateRequest(candidate: ResultReconciliationCandidate): ResultSourceRequest {
  if (
    candidate.candidateKind !== "result_source"
    || candidate.runId === null
    || candidate.launchRequestDigest === null
    || !UUID_PATTERN.test(candidate.runId)
    || !UUID_PATTERN.test(candidate.launchRequestId)
    || !BLAKE3_PATTERN.test(candidate.listingRevisionDigest)
    || !BLAKE3_PATTERN.test(candidate.launchRequestDigest)
    || !REFERENCE_PATTERN.test(candidate.hostInstallationId)
    || !REFERENCE_PATTERN.test(candidate.roomSetupOperationId)
  ) {
    throw new ResultReconciliationRejectedError("invalid_result_candidate");
  }
  return {
    schema: "worldstream/hosted-result-source-request/v1",
    run_id: candidate.runId,
    listing_revision_digest: candidate.listingRevisionDigest,
    launch_request_digest: candidate.launchRequestDigest,
    room_setup_operation_id: candidate.roomSetupOperationId,
  };
}

function validateEvidenceRequestBinding(
  evidence: HostedResultSourceEvidence,
  request: ResultSourceRequest,
  candidate: ResultReconciliationCandidate,
): void {
  if (
    evidence.host_installation_id !== candidate.hostInstallationId
    || evidence.launch_request_id !== candidate.launchRequestId
    || evidence.run_id !== request.run_id
    || evidence.listing_revision_digest !== request.listing_revision_digest
    || evidence.launch_request_digest !== request.launch_request_digest
    || evidence.room_setup_operation_id !== request.room_setup_operation_id
  ) {
    throw new ResultReconciliationRejectedError("result_source_identity_mismatch");
  }
}

interface ProjectorOutputNotTerminal {
  readonly status: "not_terminal";
}

interface ProjectorOutputTerminalWithoutOutcome {
  readonly status: "terminal_without_outcome";
}

interface ProjectorOutputSummary {
  readonly status: "summary";
  readonly summary: CanonicalObject;
}

type ProjectorOutput =
  | ProjectorOutputNotTerminal
  | ProjectorOutputTerminalWithoutOutcome
  | ProjectorOutputSummary;

function readProjectorOutput(bytes: Uint8Array): ProjectorOutput {
  const value = record(decodeCanonical(bytes));
  if (value.status === "not_terminal") {
    exactKeys(value, ["status"]);
    return { status: "not_terminal" };
  }
  if (value.status === "terminal_without_outcome") {
    exactKeys(value, ["status"]);
    return { status: "terminal_without_outcome" };
  }
  if (value.status === "summary") {
    exactKeys(value, ["status", "summary"]);
    return { status: "summary", summary: record(value.summary) };
  }
  throw new ResultReconciliationRejectedError("invalid_projector_output");
}

async function recordCurrentIntegrity(
  data: ResultReconciliationData,
  evidence: HostedResultSourceEvidence,
  hostEvidenceDigest: string,
  resultPayloadDigest: string | null,
): Promise<ReconciliationWriteReceipt> {
  const replay = evidence.integrity_status === "healthy" && evidence.replay !== undefined
    ? evidence.replay as unknown as CanonicalObject
    : null;
  const document = canonicalDocument({
    schema: "worldstream/platform-run-integrity-observation/v1",
    run_id: evidence.run_id,
    listing_revision_digest: evidence.listing_revision_digest,
    result_indexer_membership_id: evidence.result_indexer_membership_id,
    source_head: evidence.source_head as unknown as CanonicalObject,
    integrity_status: evidence.integrity_status,
    integrity_generation: evidence.integrity_generation,
    projection_hash: evidence.projection_hash,
    host_evidence_digest: hostEvidenceDigest,
    replay,
    result_payload_digest: resultPayloadDigest,
  });
  return data.recordIntegrity(evidence.run_id, document.bytes, document.digest);
}

function report(
  runId: string,
  outcome: ResultReconciliationOutcome,
  receipt: ReconciliationWriteReceipt,
): ResultReconciliationReport {
  return { runId, outcome, safeCode: receipt.safeCode };
}

function canonicalDocument(value: CanonicalObject): {
  readonly bytes: Uint8Array;
  readonly digest: Uint8Array;
} {
  const bytes = encodeCanonical(value);
  return { bytes, digest: sha256(bytes) };
}

function taggedSha256(value: Uint8Array): { readonly tagged: string; readonly bytes: Uint8Array } {
  const bytes = sha256(value);
  return { tagged: `sha256:${Buffer.from(bytes).toString("hex")}`, bytes };
}

function sha256(value: Uint8Array): Uint8Array {
  return createHash("sha256").update(value).digest();
}

function projectionHash(projection: AuthorizedPublicProjection): string {
  return taggedBlake3(encodeCanonical({
    domain: "worldstream/projection-hash/v1",
    projection_schema: "worldstream.projection.v1",
    projection: projection as unknown as CanonicalObject,
  }));
}

/** Independently validates the complete Fly evidence object and frozen hash. */
export function readHostedResultSourceEvidence(value: unknown): HostedResultSourceEvidence {
  assertJsonBounds(value);
  const evidence = record(value);
  exactKeys(
    evidence,
    [
      "schema",
      "host_installation_id",
      "launch_request_id",
      "run_id",
      "listing_revision_digest",
      "launch_request_digest",
      "room_setup_operation_id",
      "room_id",
      "pack",
      "result_indexer_membership_id",
      "access_mode",
      "source_head",
      "integrity_status",
      "integrity_generation",
      "projection_schema",
      "public_projection",
      "projection_hash",
    ],
    ["replay"],
  );
  const pack = readPack(evidence.pack);
  const sourceHead = readHead(evidence.source_head);
  const publicProjection = readAuthorizedProjection(evidence.public_projection);
  const replay = evidence.replay === undefined ? undefined : readReplay(evidence.replay);
  const result: HostedResultSourceEvidence = {
    schema: literal(
      evidence.schema,
      "worldstream/hosted-result-source-evidence/v1",
    ),
    host_installation_id: reference(evidence.host_installation_id),
    launch_request_id: uuid(evidence.launch_request_id),
    run_id: uuid(evidence.run_id),
    listing_revision_digest: digest(evidence.listing_revision_digest, BLAKE3_PATTERN),
    launch_request_digest: digest(evidence.launch_request_digest, BLAKE3_PATTERN),
    room_setup_operation_id: reference(evidence.room_setup_operation_id),
    room_id: ulid(evidence.room_id),
    pack,
    result_indexer_membership_id: ulid(evidence.result_indexer_membership_id),
    access_mode: literal(evidence.access_mode, "spectator"),
    source_head: sourceHead,
    integrity_status: enumValue(evidence.integrity_status, [
      "healthy",
      "faulted",
      "quarantined",
    ]),
    integrity_generation: safeInteger(evidence.integrity_generation),
    projection_schema: identifier(evidence.projection_schema),
    public_projection: publicProjection,
    projection_hash: digest(evidence.projection_hash, BLAKE3_PATTERN),
    ...(replay === undefined ? {} : { replay }),
  };
  const bytes = encodeCanonical(result as unknown as CanonicalObject);
  if (bytes.byteLength > MAX_EVIDENCE_BYTES) {
    throw new ResultReconciliationRejectedError("result_source_evidence_too_large");
  }
  if (
    result.room_id !== result.source_head.room_id
    || result.pack.digest !== result.source_head.pack_digest
    || result.projection_schema !== result.public_projection.projection_schema
    || result.projection_hash !== projectionHash(result.public_projection)
    || (result.replay !== undefined && (
      !sameHead(result.replay.verified_head, result.source_head)
      || result.replay.projection_hash !== result.projection_hash
    ))
  ) {
    throw new ResultReconciliationRejectedError("result_source_evidence_mismatch");
  }
  return result;
}

function readPack(value: unknown): PackReference {
  const pack = record(value);
  exactKeys(pack, ["id", "version", "digest"]);
  return {
    id: identifier(pack.id),
    version: boundedString(pack.version, 64),
    digest: digest(pack.digest, BLAKE3_PATTERN),
  };
}

function readHead(value: unknown): ResultSourceHead {
  const head = record(value);
  exactKeys(head, [
    "room_id",
    "room_seq",
    "genesis_or_transition_hash",
    "core_schema_version",
    "pack_digest",
    "core_state_hash",
    "activity_state_hash",
    "authoritative_state_hash",
  ]);
  return {
    room_id: ulid(head.room_id),
    room_seq: safeInteger(head.room_seq),
    genesis_or_transition_hash: digest(head.genesis_or_transition_hash, BLAKE3_PATTERN),
    core_schema_version: literal(
      head.core_schema_version,
      "worldstream.core-room-state.v1",
    ),
    pack_digest: digest(head.pack_digest, BLAKE3_PATTERN),
    core_state_hash: digest(head.core_state_hash, BLAKE3_PATTERN),
    activity_state_hash: digest(head.activity_state_hash, BLAKE3_PATTERN),
    authoritative_state_hash: digest(head.authoritative_state_hash, BLAKE3_PATTERN),
  };
}

function readAuthorizedProjection(value: unknown): AuthorizedPublicProjection {
  const projection = record(value);
  exactKeys(projection, [
    "projection_schema",
    "authorized_core",
    "projection",
    "action_offers",
  ]);
  if (!Array.isArray(projection.action_offers)) {
    throw new ResultReconciliationRejectedError("invalid_public_projection");
  }
  return {
    projection_schema: identifier(projection.projection_schema),
    authorized_core: projection.authorized_core as CanonicalJson,
    projection: projection.projection as CanonicalJson,
    action_offers: projection.action_offers as CanonicalJson[],
  };
}

function readReplay(value: unknown): ResultReplayEvidence {
  const replay = record(value);
  exactKeys(replay, [
    "verifier_revision",
    "verified_head",
    "projection_hash",
    "verification_receipt_digest",
  ]);
  return {
    verifier_revision: literal(
      replay.verifier_revision,
      "worldstream.authorized-replay/v1",
    ),
    verified_head: readHead(replay.verified_head),
    projection_hash: digest(replay.projection_hash, BLAKE3_PATTERN),
    verification_receipt_digest: digest(
      replay.verification_receipt_digest,
      SHA256_PATTERN,
    ),
  };
}

function sameHead(left: ResultSourceHead, right: ResultSourceHead): boolean {
  return Buffer.from(encodeCanonical(left as unknown as CanonicalObject)).equals(
    Buffer.from(encodeCanonical(right as unknown as CanonicalObject)),
  );
}

function record(value: unknown): Record<string, CanonicalJson> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ResultReconciliationRejectedError("invalid_object");
  }
  return value as Record<string, CanonicalJson>;
}

function exactKeys(
  value: Record<string, CanonicalJson>,
  required: readonly string[],
  optional: readonly string[] = [],
): void {
  const allowed = new Set([...required, ...optional]);
  if (
    required.some((key) => !Object.hasOwn(value, key))
    || Object.keys(value).some((key) => !allowed.has(key))
  ) {
    throw new ResultReconciliationRejectedError("invalid_object_keys");
  }
}

function boundedString(value: unknown, maximum: number): string {
  if (
    typeof value !== "string"
    || value.length === 0
    || Buffer.byteLength(value) > maximum
    || /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    throw new ResultReconciliationRejectedError("invalid_string");
  }
  return value;
}

function identifier(value: unknown): string {
  const result = boundedString(value, 128);
  if (!/^[A-Za-z0-9][A-Za-z0-9._:/-]*$/u.test(result)) {
    throw new ResultReconciliationRejectedError("invalid_identifier");
  }
  return result;
}

function reference(value: unknown): string {
  if (typeof value !== "string" || !REFERENCE_PATTERN.test(value)) {
    throw new ResultReconciliationRejectedError("invalid_reference");
  }
  return value;
}

function uuid(value: unknown): string {
  if (typeof value !== "string" || !UUID_PATTERN.test(value)) {
    throw new ResultReconciliationRejectedError("invalid_uuid");
  }
  return value;
}

function ulid(value: unknown): string {
  if (typeof value !== "string" || !ULID_PATTERN.test(value)) {
    throw new ResultReconciliationRejectedError("invalid_ulid");
  }
  return value;
}

function digest(value: unknown, pattern: RegExp): string {
  if (typeof value !== "string" || !pattern.test(value)) {
    throw new ResultReconciliationRejectedError("invalid_digest");
  }
  return value;
}

function hexTag(value: unknown): string {
  const result = boundedString(value, 64);
  if (!/^[0-9a-f]{64}$/u.test(result)) {
    throw new ResultReconciliationRejectedError("invalid_authentication_tag");
  }
  return result;
}

function literal<const T extends string>(value: unknown, expected: T): T {
  if (value !== expected) throw new ResultReconciliationRejectedError("unsupported_value");
  return expected;
}

function enumValue<const T extends string>(value: unknown, choices: readonly T[]): T {
  if (typeof value !== "string" || !choices.includes(value as T)) {
    throw new ResultReconciliationRejectedError("unsupported_value");
  }
  return value as T;
}

function safeInteger(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new ResultReconciliationRejectedError("invalid_integer");
  }
  return value;
}

function assertJsonBounds(value: unknown): void {
  let nodes = 0;
  const visit = (item: unknown, depth: number): void => {
    nodes += 1;
    if (nodes > MAX_JSON_NODES || depth > MAX_JSON_DEPTH) {
      throw new ResultReconciliationRejectedError("result_source_evidence_unbounded");
    }
    if (item === null || typeof item === "boolean") return;
    if (typeof item === "number") {
      if (!Number.isSafeInteger(item)) {
        throw new ResultReconciliationRejectedError("invalid_json_number");
      }
      return;
    }
    if (typeof item === "string") {
      if (Buffer.byteLength(item) > MAX_EVIDENCE_BYTES) {
        throw new ResultReconciliationRejectedError("result_source_evidence_unbounded");
      }
      return;
    }
    if (Array.isArray(item)) {
      for (const child of item) visit(child, depth + 1);
      return;
    }
    if (typeof item === "object") {
      for (const [key, child] of Object.entries(item)) {
        if (Buffer.byteLength(key) > 256) {
          throw new ResultReconciliationRejectedError("result_source_evidence_unbounded");
        }
        visit(child, depth + 1);
      }
      return;
    }
    throw new ResultReconciliationRejectedError("invalid_json_value");
  };
  visit(value, 0);
}

async function readBoundedBody(response: Response, maximum: number): Promise<Uint8Array> {
  const declared = response.headers.get("content-length");
  if (declared !== null && Number(declared) > maximum) {
    throw new ResultReconciliationRejectedError("result_source_response_too_large");
  }
  if (response.body === null) {
    throw new ResultReconciliationRejectedError("result_source_response_empty");
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > maximum) {
      await reader.cancel();
      throw new ResultReconciliationRejectedError("result_source_response_too_large");
    }
    chunks.push(value);
  }
  const result = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}
