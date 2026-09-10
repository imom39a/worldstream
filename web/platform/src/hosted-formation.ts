import { createHash } from "node:crypto";
import { reportPlatformFailure } from "./diagnostics.js";

import { deriveRoomSetup } from "@worldstream/hosted-contract";
import {
  encodeCanonical,
  taggedBlake3,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

import {
  reviewedActivityByDigest,
  type ReviewedHostedActivity,
} from "./hosted-catalog.js";

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SAFE_REFERENCE_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u;
const SAFE_OPERATION_PATTERN = /^[a-z][a-z0-9-]{0,63}$/u;
const BLAKE3_PATTERN = /^blake3:[0-9a-f]{64}$/u;
const SHA256_PATTERN = /^sha256:[0-9a-f]{64}$/u;
const MAX_GATEWAY_RESPONSE_BYTES = 262_144;

export type AccountParticipation = "account_human" | "account_external_agent";
export type HouseFillChoice = "disabled" | "fill_unclaimed";

export interface LaunchCreationResult {
  readonly launchRequestId: string;
  readonly state: string;
  readonly expiresAt: string;
  readonly wasCreated: boolean;
}

export interface FormationSeatRecord {
  readonly seatId: string;
  readonly displayName: string;
  readonly required: boolean;
  readonly claimed: boolean;
  readonly claimedByRequester: boolean;
  readonly participationKind: AccountParticipation | null;
}

export interface FormationLaunchRecord {
  readonly launchRequestId: string;
  readonly listingRevisionDigest: string;
  readonly state: string;
  readonly expiresAt: string;
  readonly creatorAccessChoice: "seat" | "spectator";
  readonly creatorSeatId: string | null;
  readonly houseFillChoice: HouseFillChoice;
  readonly rosterFrozen: boolean;
  readonly canManage: boolean;
  readonly seats: readonly FormationSeatRecord[];
}

export interface HouseFillReservation {
  readonly reservationOperationId: string;
  readonly seatId: string;
  readonly houseAgentRevisionDigest: string;
  readonly state: "pending" | "ambiguous" | "succeeded" | "terminal_failed" | "released";
}

export interface HouseFillRecord {
  readonly state: "claim_window_open" | "reserving" | "assignments_complete" | "failed_pre_genesis";
  readonly claimWindowClosesAt: string;
  readonly failureCode: string | null;
  readonly reservations: readonly HouseFillReservation[];
  /** Safe labels used only for the waiting-room roster. */
  readonly assignments: readonly {
    readonly seatId: string;
    readonly displayName: string;
  }[];
}

interface HumanLaunchMember {
  readonly seatId: string;
  readonly displayName: string;
  readonly participationKind: AccountParticipation;
  readonly principalReference: string;
}

interface HouseLaunchMember {
  readonly assignmentId: string;
  readonly seatId: string;
  readonly displayName: string;
  readonly principalReference: string;
  readonly houseAgentRevisionDigest: string;
  readonly agentProfile: { readonly profileId: string; readonly revision: string };
  readonly runnerTemplate: { readonly templateId: string; readonly revision: string };
  readonly reservationReceipt: CanonicalObject;
}

export interface HostedLaunchMaterial {
  readonly launchRequestId: string;
  readonly listingRevisionDigest: string;
  readonly state: string;
  readonly expiresAt: string;
  readonly launchInputs: CanonicalObject;
  readonly houseFillChoice: HouseFillChoice;
  readonly creatorAccessChoice: "seat" | "spectator";
  readonly creatorSeatId: string | null;
  readonly hostInstallationId: string | null;
  readonly roomSetupOperationId: string | null;
  readonly rosterFrozen: boolean;
  readonly hostMutationStarted: boolean;
  readonly claims: readonly HumanLaunchMember[];
  readonly houseAssignments: readonly HouseLaunchMember[];
}

export interface GenesisReconciliation {
  readonly launchState: string;
  readonly runId: string | null;
  readonly reconciliationState: "pending" | "ready" | "quarantined";
  readonly needsGenesisPull: boolean;
}

/** Server-derived deadline work; never constructed from a browser payload. */
export interface PrestartAbandonmentCandidate {
  readonly runId: string;
  readonly launchRequestId: string;
  readonly listingRevisionDigest: string;
  readonly hostInstallationId: string;
  readonly roomSetupOperationId: string;
}

/** Server-selected creator closures that still need Host evidence. */
export interface LaunchClosureCandidate {
  readonly launchRequestId: string;
}

export interface OwnedRunRecord {
  readonly runId: string;
  readonly publicId: string | null;
  readonly reconciliationState: "ready" | "quarantined";
  readonly canEnter: boolean;
  readonly memberships: readonly {
    readonly purpose: "participant" | "creator_spectator";
    readonly seatId: string | null;
    readonly entrySelector: string | null;
  }[];
}

/** Private service-key data seam. No method accepts a browser-supplied account id. */
export interface HostedFormationData {
  createLaunchRequest(input: {
    accountId: string;
    listingRevisionDigest: string;
    idempotencyNamespace: string;
    idempotencyKeyDigest: Uint8Array;
    canonicalLaunchInput: Uint8Array;
    launchInputDigest: Uint8Array;
    houseFillChoice: HouseFillChoice;
    creatorAccessChoice: "seat" | "spectator";
    creatorSeatId: string | null;
  }): Promise<LaunchCreationResult>;
  readLaunchRequest(accountId: string, launchRequestId: string): Promise<FormationLaunchRecord | null>;
  rotateSeatInvitation(accountId: string, launchRequestId: string, seatId: string): Promise<{
    readonly token: string;
    readonly expiresAt: string;
  } | null>;
  claimInvitedSeat(accountId: string, tokenDigest: Uint8Array, participation: AccountParticipation): Promise<{
    readonly launchRequestId: string;
    readonly seatId: string;
  } | null>;
  releaseSeatClaim(accountId: string, launchRequestId: string, seatId: string): Promise<boolean>;
  resetSeatClaim(accountId: string, launchRequestId: string, seatId: string): Promise<boolean>;
  cancelLaunchRequest(accountId: string, launchRequestId: string): Promise<boolean>;
  requestLaunchClosure(input: {
    accountId: string;
    launchRequestId: string;
    hostInstallationId: string;
    canonicalRequest: Uint8Array;
    requestDigest: Uint8Array;
  }): Promise<boolean>;
  startHouseFill(accountId: string, launchRequestId: string): Promise<HouseFillRecord | null>;
  readHouseFill(accountId: string, launchRequestId: string): Promise<HouseFillRecord | null>;
  retainHouseFillSelection(launchRequestId: string, hostInstallationId: string): Promise<HouseFillRecord | null>;
  recordHouseRunnerReservation(input: {
    reservationOperationId: string;
    outcome: "succeeded" | "terminal_failed";
    runnerUnitId: string | null;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
    failureCode: string | null;
  }): Promise<HouseFillRecord | null>;
  completeHouseFill(launchRequestId: string): Promise<HouseFillRecord | null>;
  readHostedLaunchMaterial(accountId: string, launchRequestId: string): Promise<HostedLaunchMaterial | null>;
  /** Service-only lookup; returns only an already authorized, frozen operation. */
  readHostedRecoveryMaterial(launchRequestId: string): Promise<HostedLaunchMaterial | null>;
  freezeLaunch(input: {
    accountId: string;
    launchRequestId: string;
    frozenRoster: Uint8Array;
    frozenRosterDigest: Uint8Array;
    frozenRoomSetup: Uint8Array;
    frozenRoomSetupDigest: string;
    hostInstallationId: string;
    roomSetupOperationId: string;
  }): Promise<boolean>;
  authorizeHostMutation(accountId: string, launchRequestId: string, hostInstallationId: string, roomSetupOperationId: string): Promise<boolean>;
  readGenesisReconciliation(launchRequestId: string): Promise<GenesisReconciliation | null>;
  recordGenesis(launchRequestId: string, canonicalEvidence: Uint8Array, evidenceDigest: Uint8Array): Promise<{
    readonly runId: string;
    readonly reconciliationState: "ready" | "quarantined";
  } | null>;
  recordPrestartAbandonment(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean>;
  recordProvisioningAbandonment(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean>;
  recordLaunchClosure(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean>;
  listPendingLaunchClosures(limit: number): Promise<readonly LaunchClosureCandidate[]>;
  listPrestartAbandonmentCandidates(limit: number): Promise<readonly PrestartAbandonmentCandidate[]>;
  readPublicRelayBindingCandidate(runId: string): Promise<CanonicalObject | null>;
  recordPublicRelayBinding(input: {
    runId: string;
    canonicalRequest: Uint8Array;
    requestDigest: Uint8Array;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }): Promise<boolean>;
  readOwnedRun(accountId: string, runId: string): Promise<OwnedRunRecord | null>;
}

export interface HostedLaunchStatus {
  readonly state: string;
  readonly roomSetupComplete: boolean;
}

export interface HostedFormationGateway {
  reserveHouseRunner(request: CanonicalObject): Promise<CanonicalObject>;
  launch(request: CanonicalObject): Promise<HostedLaunchStatus>;
  readStatus(request: CanonicalObject): Promise<HostedLaunchStatus>;
  /** Service-only: fences this launch before Genesis or archives its Room. */
  closeLaunch(request: CanonicalObject): Promise<CanonicalObject>;
  /** Service-only: Host proves its retained Lobby task has not launched. */
  abandonPrestart(request: CanonicalObject): Promise<CanonicalObject>;
  /** Service-only: Host fences one exact setup operation before Genesis. */
  abandonProvisioning(request: CanonicalObject): Promise<CanonicalObject>;
  readGenesisEvidence(request: CanonicalObject): Promise<CanonicalObject>;
  bindPublicRelay(request: CanonicalObject): Promise<CanonicalObject>;
}

export class HostedFormationRejectedError extends Error {
  constructor(message = "hosted_formation_rejected") {
    super(message);
    this.name = "HostedFormationRejectedError";
  }
}

export class HostedFormationPendingError extends Error {
  constructor() {
    super("hosted_formation_pending");
    this.name = "HostedFormationPendingError";
  }
}

class HostedFormationNotFoundError extends HostedFormationRejectedError {}
class HostedFormationConflictError extends HostedFormationRejectedError {}

export class HostedFormationUnavailableError extends Error {
  constructor(message = "hosted_formation_unavailable", options?: ErrorOptions) {
    super(message, options);
    this.name = "HostedFormationUnavailableError";
  }
}

export interface FormationAdvanceResult {
  readonly state: "collecting" | "provisioning" | "reconciling" | "run_created" | "failed_pre_genesis";
  readonly retryAfterSeconds: number | null;
  readonly runId: string | null;
}

/** Stateless coordinator. Every retry reconstructs the same frozen documents. */
export class HostedFormationCoordinator {
  constructor(
    private readonly data: HostedFormationData,
    private readonly gateway: HostedFormationGateway,
    private readonly hostInstallationId: string,
    private readonly resolveReviewedActivity: (digest: string) => ReviewedHostedActivity | null = reviewedActivityByDigest,
  ) {
    if (!SAFE_REFERENCE_PATTERN.test(hostInstallationId)) {
      throw new HostedFormationRejectedError("invalid_host_installation");
    }
  }

  async advance(accountId: string, launchRequestId: string): Promise<FormationAdvanceResult> {
    let material = await this.requiredMaterial(accountId, launchRequestId);
    const reviewed = requiredReviewedActivity(material.listingRevisionDigest, this.resolveReviewedActivity);
    // A pre-start abandonment is a terminal, evidence-backed decision.  Do
    // not let the generic retry path treat its retained host mutation as an
    // invitation to resume or relaunch the Lobby.  The other terminal
    // states retain their existing failed-pre-Genesis response semantics.
    if (["closing", "closed_by_creator", "abandoned_prestart"].includes(material.state)) {
      throw new HostedFormationRejectedError("launch_unavailable");
    }
    if (["cancelled", "expired", "failed_pre_genesis"].includes(material.state)) {
      return { state: "failed_pre_genesis", retryAfterSeconds: null, runId: null };
    }
    if (material.hostMutationStarted) return this.resume(material);

    if (!material.rosterFrozen && material.houseFillChoice === "fill_unclaimed") {
      const house = await this.advanceHouseFill(accountId, material);
      if (house !== null) return house;
      material = await this.requiredMaterial(accountId, launchRequestId);
    }

    const documents = deriveFrozenDocuments(reviewed, material, this.hostInstallationId);
    if (!material.rosterFrozen) {
      const frozen = await this.data.freezeLaunch({
        accountId,
        launchRequestId,
        frozenRoster: documents.rosterBytes,
        frozenRosterDigest: sha256(documents.rosterBytes),
        frozenRoomSetup: documents.setupBytes,
        frozenRoomSetupDigest: taggedBlake3(documents.setupBytes),
        hostInstallationId: this.hostInstallationId,
        roomSetupOperationId: documents.roomSetupOperationId,
      });
      if (!frozen) throw new HostedFormationRejectedError();
      material = await this.requiredMaterial(accountId, launchRequestId);
    }
    requireFrozenIdentity(material, this.hostInstallationId, documents.roomSetupOperationId);
    if (!(await this.data.authorizeHostMutation(
      accountId,
      launchRequestId,
      this.hostInstallationId,
      documents.roomSetupOperationId,
    ))) {
      throw new HostedFormationRejectedError();
    }

    const status = await this.gateway.launch(documents.gatewayLaunchRequest);
    return this.recordObservedGenesis(launchRequestId, documents, status);
  }

  /** Repairs prior consent only. It cannot freeze a roster or authorize a launch. */
  async recover(launchRequestId: string): Promise<FormationAdvanceResult | null> {
    const material = await this.data.readHostedRecoveryMaterial(launchRequestId);
    return material === null || material.state === "closing" ? null : this.resume(material);
  }

  /**
   * Requests and completes the creator's one-way close for this launch lineage.
   * Capacity is released only after the Host returns a durable fence or an
   * archived canonical Room head.
   */
  async close(accountId: string, launchRequestId: string): Promise<boolean> {
    const material = await this.requiredMaterial(accountId, launchRequestId);
    if (material.state === "closed_by_creator") return true;
    const closureRequest = deriveClosureRequest(material);
    const requestBytes = encodeCanonical(closureRequest);
    if (!(await this.data.requestLaunchClosure({
      accountId,
      launchRequestId,
      hostInstallationId: this.hostInstallationId,
      canonicalRequest: requestBytes,
      requestDigest: sha256(requestBytes),
    }))) {
      throw new HostedFormationRejectedError("launch_close_rejected");
    }
    return this.completeClosure(launchRequestId);
  }

  /** Retries an already-authorized closure without browser authority. */
  async recoverClosure(launchRequestId: string): Promise<boolean> {
    return this.completeClosure(launchRequestId);
  }

  private async completeClosure(launchRequestId: string): Promise<boolean> {
    const material = await this.data.readHostedRecoveryMaterial(launchRequestId);
    // The request RPC is idempotent and a completed launch is intentionally no
    // longer exposed as recovery material.
    if (material === null) return true;
    if (material.state !== "closing") {
      throw new HostedFormationRejectedError("launch_close_not_authorized");
    }
    const closureRequest = deriveClosureRequest(material);
    const evidence = await this.gateway.closeLaunch(closureRequest);
    validateLaunchClosureEvidence(evidence, closureRequest, this.hostInstallationId);
    const evidenceBytes = encodeCanonical(evidence);
    if (!(await this.data.recordLaunchClosure(
      launchRequestId,
      evidenceBytes,
      sha256(evidenceBytes),
    ))) {
      throw new HostedFormationRejectedError("launch_close_rejected");
    }
    return true;
  }

  /**
   * Fences a Genesis-created Room before its Lobby launch. This is deliberately
   * separate from creator cancellation: it uses the exact already-frozen Host
   * operation and the Host's current task state, never a browser timestamp.
   */
  async abandonPrestart(launchRequestId: string): Promise<boolean> {
    const material = await this.data.readHostedRecoveryMaterial(launchRequestId);
    if (material === null || !["run_created", "abandoned_prestart"].includes(material.state)) {
      throw new HostedFormationRejectedError("prestart_abandonment_unavailable");
    }
    const documents = this.recoveryDocuments(material);
    const evidence = await this.gateway.abandonPrestart(documents.evidenceRequest);
    validatePrestartAbandonmentEvidence(evidence, documents.evidenceRequest);
    const evidenceBytes = encodeCanonical(evidence);
    if (!(await this.data.recordPrestartAbandonment(
      launchRequestId,
      evidenceBytes,
      sha256(evidenceBytes),
    ))) {
      throw new HostedFormationRejectedError("prestart_abandonment_rejected");
    }
    return true;
  }

  /**
   * The narrow pre-Genesis closure for a retained setup operation that the
   * Host proves absent. It is reachable only from `resume` after an exact
   * read 404 and exact replay rejection; this method itself never uses time
   * or a browser request as proof of absence.
   */
  async abandonProvisioning(
    material: HostedLaunchMaterial,
    documents: ReturnType<typeof deriveFrozenDocuments>,
    reconciliation: GenesisReconciliation | null,
  ): Promise<FormationAdvanceResult> {
    if (!['provisioning', 'reconciling'].includes(material.state) ||
        (reconciliation !== null && reconciliation.runId !== null)) {
      throw new HostedFormationRejectedError("provisioning_abandonment_unavailable");
    }
    let evidence: CanonicalObject;
    try {
      evidence = await this.gateway.abandonProvisioning(documents.evidenceRequest);
    } catch (error) {
      // A conflict can originate at the fixed Gateway boundary or from the
      // Host after the platform's second read. Either way it supplies no
      // absence proof, so there is no authority to release capacity. Pull
      // Genesis again and retain this launch for ordinary reconciliation.
      if (error instanceof HostedFormationConflictError) {
        return this.recordObservedGenesis(material.launchRequestId, documents, {
          state: "reconciling",
          roomSetupComplete: false,
        });
      }
      throw error;
    }
    validateProvisioningAbandonmentEvidence(
      evidence, documents.evidenceRequest, material.launchRequestId,
    );
    const evidenceBytes = encodeCanonical(evidence);
    if (!(await this.data.recordProvisioningAbandonment(
      material.launchRequestId,
      evidenceBytes,
      sha256(evidenceBytes),
    ))) {
      throw new HostedFormationRejectedError("provisioning_abandonment_rejected");
    }
    return { state: "failed_pre_genesis", retryAfterSeconds: null, runId: null };
  }

  async entryReady(launchRequestId: string): Promise<boolean> {
    const material = await this.data.readHostedRecoveryMaterial(launchRequestId);
    if (material === null) return false;
    const documents = this.recoveryDocuments(material);
    const reconciliation = await this.data.readGenesisReconciliation(launchRequestId);
    if (reconciliation?.reconciliationState !== "ready" || reconciliation.runId === null) return false;
    try {
      return (await this.gateway.readStatus(documents.evidenceRequest)).roomSetupComplete;
    } catch (error) {
      // Entry is disabled until a fresh authorized status confirms it. A
      // temporary Host outage is not proof that the frozen Room is absent and
      // must not make a caller create a replacement Launch Request.
      if (
        error instanceof HostedFormationUnavailableError ||
        error instanceof HostedFormationNotFoundError
      ) return false;
      throw error;
    }
  }

  private recoveryDocuments(material: HostedLaunchMaterial) {
    if (!material.hostMutationStarted || !material.rosterFrozen ||
        !["provisioning", "reconciling", "run_created", "abandoned_prestart"].includes(material.state)) {
      throw new HostedFormationRejectedError("launch_recovery_not_authorized");
    }
    const documents = deriveFrozenDocuments(
      requiredReviewedActivity(material.listingRevisionDigest, this.resolveReviewedActivity), material, this.hostInstallationId,
    );
    requireFrozenIdentity(material, this.hostInstallationId, documents.roomSetupOperationId);
    return documents;
  }

  private async resume(material: HostedLaunchMaterial): Promise<FormationAdvanceResult> {
    const documents = this.recoveryDocuments(material);
    const reconciliation = await this.data.readGenesisReconciliation(material.launchRequestId);
    if (reconciliation?.reconciliationState === "quarantined") {
      throw new HostedFormationRejectedError("launch_quarantined");
    }
    // Observe Genesis before any repair. A lost capability reply must not hide
    // an existing Room from platform correspondence.
    const observed = await this.recordObservedGenesis(material.launchRequestId, documents, {
      state: "reconciling", roomSetupComplete: false,
    });
    let status: HostedLaunchStatus;
    let exactStatusWasMissing = false;
    try {
      status = await this.gateway.readStatus(documents.evidenceRequest);
    } catch (error) {
      if (!(error instanceof HostedFormationNotFoundError)) throw error;
      exactStatusWasMissing = true;
      status = { state: "provisioning", roomSetupComplete: false };
    }
    // Genesis and Membership creation do not imply that every House process
    // has started. Resume the same frozen launch while its Lobby is waiting;
    // the Host retains process identities, allowances and the launch input.
    // Human entry remains available so synchronization can satisfy readiness.
    if (!status.roomSetupComplete || status.state === "waiting_for_readiness") {
      try {
        status = await this.gateway.launch(documents.gatewayLaunchRequest);
      } catch (error) {
        // This is deliberately a conjunction. A generic launch rejection,
        // timeout, or an observed Host operation remains recoverable and must
        // retain capacity. Only an exact missing read followed by rejection of
        // the same frozen replay can ask the Host to install its durable fence.
        if (exactStatusWasMissing &&
            ['provisioning', 'reconciling'].includes(material.state) &&
            (reconciliation === null || reconciliation.runId === null) &&
            error instanceof HostedFormationConflictError) {
          // A submit conflict does not prove that the Host still has no
          // operation. A same-identity create/resume may have crossed the
          // initial read and the replay can observe that retained operation
          // as a conflict. Re-read the same identity first; any observed
          // operation remains recoverable and retains capacity.
          try {
            status = await this.gateway.readStatus(documents.evidenceRequest);
          } catch (readError) {
            if (readError instanceof HostedFormationNotFoundError) {
              return this.abandonProvisioning(material, documents, reconciliation);
            }
            throw readError;
          }
          // A second Genesis observation also closes the small interval in
          // which the exact operation became visible after the first pull.
          return this.recordObservedGenesis(material.launchRequestId, documents, status);
        }
        throw error;
      }
    }
    if (!status.roomSetupComplete) return observed;
    return this.recordObservedGenesis(material.launchRequestId, documents, status);
  }

  private async recordObservedGenesis(
    launchRequestId: string,
    documents: ReturnType<typeof deriveFrozenDocuments>,
    status: HostedLaunchStatus,
  ): Promise<FormationAdvanceResult> {
    try {
      let evidence: CanonicalObject;
      try {
        evidence = await this.gateway.readGenesisEvidence(documents.evidenceRequest);
      } catch (error) {
        if (!status.roomSetupComplete && (error instanceof HostedFormationNotFoundError ||
            error instanceof HostedFormationUnavailableError)) {
          return { state: "reconciling", retryAfterSeconds: 2, runId: null };
        }
        throw error;
      }
      const evidenceBytes = encodeCanonical(evidence);
      const recorded = await this.data.recordGenesis(
        launchRequestId,
        evidenceBytes,
        sha256(evidenceBytes),
      );
      if (recorded === null || recorded.reconciliationState !== "ready") throw new HostedFormationRejectedError();
      if (!status.roomSetupComplete) {
        return { state: "reconciling", retryAfterSeconds: 2, runId: recorded.runId };
      }
      await this.ensurePublicRelayBinding(recorded.runId);
      return {
        state: "run_created",
        retryAfterSeconds: null,
        runId: recorded.runId,
      };
    } catch (error) {
      if (error instanceof HostedFormationPendingError) {
        return { state: "reconciling", retryAfterSeconds: 2, runId: null };
      }
      throw error;
    }
  }

  private async advanceHouseFill(
    accountId: string,
    material: HostedLaunchMaterial,
  ): Promise<FormationAdvanceResult | null> {
    let operation = await this.data.readHouseFill(accountId, material.launchRequestId);
    operation ??= await this.data.startHouseFill(accountId, material.launchRequestId);
    if (operation === null) throw new HostedFormationRejectedError();
    if (operation.state === "failed_pre_genesis") {
      return { state: "failed_pre_genesis", retryAfterSeconds: null, runId: null };
    }
    if (operation.state === "claim_window_open") {
      const remaining = Math.ceil((Date.parse(operation.claimWindowClosesAt) - Date.now()) / 1_000);
      if (remaining > 0) {
        return { state: "collecting", retryAfterSeconds: Math.max(1, remaining), runId: null };
      }
      operation = await this.data.retainHouseFillSelection(
        material.launchRequestId,
        this.hostInstallationId,
      );
      if (operation === null) throw new HostedFormationRejectedError();
      if (operation.state === "claim_window_open") {
        return { state: "collecting", retryAfterSeconds: 1, runId: null };
      }
    }
    if (operation.state === "failed_pre_genesis") {
      return { state: "failed_pre_genesis", retryAfterSeconds: null, runId: null };
    }
    if (operation.state === "reserving") {
      for (const reservation of operation.reservations) {
        if (reservation.state === "succeeded") continue;
        const request = houseReservationRequest(
          this.hostInstallationId,
          material,
          reservation,
        );
        const receipt = await this.gateway.reserveHouseRunner(request);
        validateHouseReceipt(receipt, request);
        const receiptBytes = encodeCanonical(receipt);
        operation = await this.data.recordHouseRunnerReservation({
          reservationOperationId: reservation.reservationOperationId,
          outcome: receipt.outcome === "succeeded" ? "succeeded" : "terminal_failed",
          runnerUnitId: nullableString(receipt.runner_unit_id),
          canonicalReceipt: receiptBytes,
          receiptDigest: sha256(receiptBytes),
          failureCode: nullableString(receipt.failure_code),
        });
        if (operation === null) throw new HostedFormationRejectedError();
        if (operation.state === "failed_pre_genesis") {
          return { state: "failed_pre_genesis", retryAfterSeconds: null, runId: null };
        }
      }
      operation = await this.data.completeHouseFill(material.launchRequestId);
      if (operation === null) throw new HostedFormationRejectedError();
    }
    if (operation.state !== "assignments_complete") {
      throw new HostedFormationRejectedError("house_fill_incomplete");
    }
    return null;
  }

  private async ensurePublicRelayBinding(runId: string): Promise<void> {
    const request = await this.data.readPublicRelayBindingCandidate(runId);
    if (request === null) return;
    const requestBytes = encodeCanonical(request);
    const requestDigest = sha256(requestBytes);
    const receipt = await this.gateway.bindPublicRelay(request);
    validatePublicRelayReceipt(receipt, request, requestDigest);
    const receiptBytes = encodeCanonical(receipt);
    if (!(await this.data.recordPublicRelayBinding({
      runId,
      canonicalRequest: requestBytes,
      requestDigest,
      canonicalReceipt: receiptBytes,
      receiptDigest: sha256(receiptBytes),
    }))) {
      throw new HostedFormationRejectedError("public_relay_binding_rejected");
    }
  }

  private async requiredMaterial(accountId: string, launchRequestId: string) {
    const material = await this.data.readHostedLaunchMaterial(accountId, launchRequestId);
    if (material === null) throw new HostedFormationRejectedError();
    return material;
  }
}

export class HttpHostedFormationGateway implements HostedFormationGateway {
  private readonly base: URL;
  private readonly timeoutMs: number;
  private readonly fetchImplementation: typeof fetch;

  constructor(input: {
    readonly baseUrl: string;
    readonly serviceAuthority: string;
    readonly timeoutMs?: number;
    readonly fetchImplementation?: typeof fetch;
  }) {
    this.base = serviceBase(input.baseUrl);
    if (
      input.serviceAuthority.length < 32 ||
      input.serviceAuthority.length > 512 ||
      !/^[\x21-\x7e]+$/u.test(input.serviceAuthority)
    ) {
      throw new HostedFormationRejectedError("invalid_gateway_configuration");
    }
    this.authority = input.serviceAuthority;
    this.timeoutMs = input.timeoutMs ?? 10_000;
    if (!Number.isSafeInteger(this.timeoutMs) || this.timeoutMs < 100 || this.timeoutMs > 30_000) {
      throw new HostedFormationRejectedError("invalid_gateway_timeout");
    }
    this.fetchImplementation = input.fetchImplementation ?? fetch;
  }

  private readonly authority: string;

  /**
   * Reads whether the live Gateway and its fixed Controller recognize one
   * exact reviewed Listing Revision. This request creates no launch state.
   */
  async activityAvailable(listingRevisionDigest: string): Promise<boolean> {
    if (!BLAKE3_PATTERN.test(listingRevisionDigest)) {
      throw new HostedFormationRejectedError("invalid_listing_identity");
    }
    let response: Response;
    try {
      response = await this.fetchImplementation(new URL(
        `/v1/hosted/activities/${encodeURIComponent(listingRevisionDigest)}/availability`,
        this.base,
      ), {
        method: "GET",
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.authority}`,
        },
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(this.timeoutMs),
      });
    } catch (error) {
      reportPlatformFailure("fly_formation", error);
      throw new HostedFormationUnavailableError(undefined, { cause: error });
    }
    if (!response.ok) {
      if ([400, 401, 403, 404, 409, 422].includes(response.status)) {
        throw new HostedFormationRejectedError("activity_unavailable");
      }
      reportPlatformFailure("fly_formation", undefined, { status: response.status });
      throw new HostedFormationUnavailableError();
    }
    const bytes = await boundedResponse(response, 4_096);
    let value: unknown;
    try {
      value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    } catch (error) {
      throw new HostedFormationUnavailableError("invalid_gateway_response", { cause: error });
    }
    if (
      value === null || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).length !== 3 ||
      (value as Record<string, unknown>).schema !== "worldstream/hosted-activity-availability/v1" ||
      (value as Record<string, unknown>).listing_revision_digest !== listingRevisionDigest ||
      typeof (value as Record<string, unknown>).available !== "boolean"
    ) {
      throw new HostedFormationUnavailableError("invalid_gateway_response");
    }
    return (value as Record<string, unknown>).available === true;
  }

  async reserveHouseRunner(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/house-runners/reserve", request, false);
  }

  async launch(request: CanonicalObject): Promise<HostedLaunchStatus> {
    return this.statusCall("/v1/hosted/launch", request);
  }

  async readStatus(request: CanonicalObject): Promise<HostedLaunchStatus> {
    return this.statusCall("/v1/hosted/evidence", request);
  }

  async closeLaunch(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/close", request, false);
  }

  async abandonPrestart(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/abandon-prestart", request, false);
  }

  async abandonProvisioning(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/abandon-provisioning", request, false);
  }

  private async statusCall(path: string, request: CanonicalObject): Promise<HostedLaunchStatus> {
    const response = await this.call(path, request, false);
    if (
      response.schema !== "worldstream/hosted-launch-status/v1" ||
      response.launch_request_digest !== request.launch_request_digest ||
      response.room_setup_operation_id !== request.room_setup_operation_id ||
      typeof response.room_setup_complete !== "boolean"
    ) {
      throw new HostedFormationUnavailableError("invalid_gateway_response");
    }
    return {
      state: requiredString(response.stage),
      roomSetupComplete: response.room_setup_complete,
    };
  }

  async readGenesisEvidence(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/genesis-evidence", request, true);
  }

  async bindPublicRelay(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/public-relays/bind", request, false);
  }

  private async call(path: string, body: CanonicalObject, pendingOnConflict: boolean): Promise<CanonicalObject> {
    let response: Response;
    try {
      response = await this.fetchImplementation(new URL(path, this.base), {
        method: "POST",
        headers: {
          accept: "application/json",
          authorization: `Bearer ${this.authority}`,
          "content-type": "application/json",
        },
        body: Buffer.from(encodeCanonical(body)),
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(this.timeoutMs),
      });
    } catch (error) {
      reportPlatformFailure("fly_formation", error);
      throw new HostedFormationUnavailableError(undefined, { cause: error });
    }
    if (!response.ok) {
      if (pendingOnConflict && response.status === 409) throw new HostedFormationPendingError();
      if (response.status === 404) throw new HostedFormationNotFoundError();
      if (response.status === 409) throw new HostedFormationConflictError();
      if ([400, 401, 403, 404, 409, 422].includes(response.status)) {
        throw new HostedFormationRejectedError();
      }
      reportPlatformFailure("fly_formation", undefined, { status: response.status });
      throw new HostedFormationUnavailableError();
    }
    const bytes = await boundedResponse(response, MAX_GATEWAY_RESPONSE_BYTES);
    try {
      const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
      return requiredCanonicalObject(value);
    } catch (error) {
      if (error instanceof HostedFormationUnavailableError) throw error;
      throw new HostedFormationUnavailableError("invalid_gateway_response", { cause: error });
    }
  }
}

function deriveFrozenDocuments(
  reviewed: ReviewedHostedActivity,
  material: HostedLaunchMaterial,
  hostInstallationId: string,
) {
  const creatorReference = material.creatorAccessChoice === "spectator"
    ? "worldstream:creator-spectator"
    : material.claims.find(({ seatId }) => seatId === material.creatorSeatId)?.principalReference;
  if (creatorReference === undefined) throw new HostedFormationRejectedError("creator_seat_unfilled");
  const launch = {
    schema: "worldstream/launch-request/v2",
    listing_revision_digest: material.listingRevisionDigest,
    inputs: material.launchInputs,
    creator: {
      participation: material.creatorAccessChoice,
      principal_reference: creatorReference,
    },
  } as const;
  const members = [
    ...material.claims.map((claim) => ({
      seat_id: claim.seatId,
      participation: claim.participationKind,
      principal_reference: claim.principalReference,
      display_name: claim.displayName,
    })),
    ...material.houseAssignments.map((assignment) => ({
      seat_id: assignment.seatId,
      participation: "house_agent_fill" as const,
      principal_reference: assignment.principalReference,
      display_name: assignment.displayName,
      house_agent_revision_digest: assignment.houseAgentRevisionDigest,
      agent_profile: {
        profile_id: assignment.agentProfile.profileId,
        revision: assignment.agentProfile.revision,
      },
      runner_template: {
        template_id: assignment.runnerTemplate.templateId,
        revision: assignment.runnerTemplate.revision,
      },
    })),
  ];
  const roster = {
    schema: "worldstream/frozen-roster/v1",
    listing_revision_digest: material.listingRevisionDigest,
    members,
  } as const;
  const launchBytes = encodeCanonical(launch);
  const rosterBytes = encodeCanonical(roster);
  const setupBytes = deriveRoomSetup(
    reviewed.listing,
    launchBytes,
    rosterBytes,
    [...reviewed.houseAgents.values()],
  );
  const roomSetupOperationId = material.roomSetupOperationId ??
    `launch-${material.launchRequestId.replaceAll("-", "")}`;
  if (!SAFE_OPERATION_PATTERN.test(roomSetupOperationId)) throw new HostedFormationRejectedError();
  const launchRequestDigest = taggedBlake3(launchBytes);
  const evidenceRequest = {
    schema: "worldstream/hosted-launch-evidence-request/v1",
    listing_revision_digest: material.listingRevisionDigest,
    launch_request_digest: launchRequestDigest,
    room_setup_operation_id: roomSetupOperationId,
  } as const;
  const gatewayLaunchRequest = {
    schema: "worldstream/hosted-launch-request/v1",
    listing_revision_digest: material.listingRevisionDigest,
    launch_request_digest: launchRequestDigest,
    launch_input_digest: taggedSha256(encodeCanonical(material.launchInputs)),
    frozen_roster_digest: taggedSha256(rosterBytes),
    room_setup_specification_digest: taggedBlake3(setupBytes),
    room_setup_operation_id: roomSetupOperationId,
    capacity_authorization: {
      schema: "worldstream/platform-capacity-authorization/v1",
      host_installation_id: hostInstallationId,
      reservation_reference: material.launchRequestId,
    },
    house_runner_assignments: material.houseAssignments.map((assignment) => ({
      house_agent_assignment_id: assignment.assignmentId,
      reservation_receipt: assignment.reservationReceipt,
    })),
    frozen_launch_request: launch,
    frozen_roster: roster,
    frozen_room_setup_specification: JSON.parse(new TextDecoder().decode(setupBytes)),
  } as unknown as CanonicalObject;
  return {
    evidenceRequest: evidenceRequest as unknown as CanonicalObject,
    gatewayLaunchRequest,
    roomSetupOperationId,
    rosterBytes,
    setupBytes,
  };
}

function deriveClosureRequest(material: HostedLaunchMaterial): CanonicalObject {
  const creatorReference = material.creatorAccessChoice === "spectator"
    ? "worldstream:creator-spectator"
    : material.creatorSeatId === null ? undefined : `seat:${material.creatorSeatId}`;
  if (creatorReference === undefined) {
    throw new HostedFormationRejectedError("creator_seat_unfilled");
  }
  const launch = {
    schema: "worldstream/launch-request/v2",
    listing_revision_digest: material.listingRevisionDigest,
    inputs: material.launchInputs,
    creator: {
      participation: material.creatorAccessChoice,
      principal_reference: creatorReference,
    },
  } as const;
  const roomSetupOperationId = material.roomSetupOperationId ??
    `launch-${material.launchRequestId.replaceAll("-", "")}`;
  if (!SAFE_OPERATION_PATTERN.test(roomSetupOperationId)) {
    throw new HostedFormationRejectedError("invalid_launch_identity");
  }
  return {
    schema: "worldstream/hosted-launch-closure-request/v1",
    launch_request_id: material.launchRequestId,
    listing_revision_digest: material.listingRevisionDigest,
    launch_request_digest: taggedBlake3(encodeCanonical(launch)),
    room_setup_operation_id: roomSetupOperationId,
  };
}

function houseReservationRequest(
  hostInstallationId: string,
  material: HostedLaunchMaterial,
  reservation: HouseFillReservation,
): CanonicalObject {
  return {
    schema: "worldstream/house-runner-reservation-request/v1",
    host_installation_id: hostInstallationId,
    reservation_operation_id: reservation.reservationOperationId,
    launch_request_id: material.launchRequestId,
    listing_revision_digest: material.listingRevisionDigest,
    seat_id: reservation.seatId,
    house_agent_revision_digest: reservation.houseAgentRevisionDigest,
  };
}

function validateHouseReceipt(receipt: CanonicalObject, request: CanonicalObject): void {
  if (
    receipt.schema !== "worldstream/house-runner-reservation-receipt/v1" ||
    receipt.host_installation_id !== request.host_installation_id ||
    receipt.reservation_operation_id !== request.reservation_operation_id ||
    receipt.launch_request_id !== request.launch_request_id ||
    receipt.listing_revision_digest !== request.listing_revision_digest ||
    receipt.seat_id !== request.seat_id ||
    receipt.house_agent_revision_digest !== request.house_agent_revision_digest ||
    (receipt.outcome !== "succeeded" && receipt.outcome !== "terminal_failed") ||
    !BLAKE3_PATTERN.test(requiredString(receipt.binding_digest)) ||
    !/^[0-9a-f]{64}$/u.test(requiredString(receipt.authentication_tag))
  ) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
  if (
    (receipt.outcome === "succeeded" &&
      (!SAFE_REFERENCE_PATTERN.test(requiredString(receipt.runner_unit_id)) || receipt.failure_code !== null)) ||
    (receipt.outcome === "terminal_failed" &&
      (receipt.runner_unit_id !== null || !SAFE_REFERENCE_PATTERN.test(requiredString(receipt.failure_code))))
  ) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
}

function validatePrestartAbandonmentEvidence(
  evidence: CanonicalObject,
  request: CanonicalObject,
): void {
  if (
    evidence.schema !== "worldstream/hosted-prestart-abandonment-evidence/v1" ||
    !SAFE_REFERENCE_PATTERN.test(requiredString(evidence.host_installation_id)) ||
    !UUID_PATTERN.test(requiredString(evidence.launch_request_id)) ||
    evidence.listing_revision_digest !== request.listing_revision_digest ||
    evidence.launch_request_digest !== request.launch_request_digest ||
    evidence.room_setup_operation_id !== request.room_setup_operation_id ||
    !SAFE_REFERENCE_PATTERN.test(requiredString(evidence.room_id)) ||
    evidence.lobby_launch_committed !== false ||
    !BLAKE3_PATTERN.test(requiredString(evidence.abandonment_fence_digest)) ||
    !/^[0-9a-f]{64}$/u.test(requiredString(evidence.authentication_tag)) ||
    Object.keys(evidence).sort().join(",") !==
      "abandonment_fence_digest,authentication_tag,host_installation_id,launch_request_digest,launch_request_id,listing_revision_digest,lobby_launch_committed,room_id,room_setup_operation_id,schema"
  ) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
}

function validateProvisioningAbandonmentEvidence(
  evidence: CanonicalObject,
  request: CanonicalObject,
  launchRequestId: string,
): void {
  if (
    evidence.schema !== "worldstream/hosted-provisioning-abandonment-evidence/v1" ||
    !SAFE_REFERENCE_PATTERN.test(requiredString(evidence.host_installation_id)) ||
    evidence.launch_request_id !== launchRequestId ||
    !UUID_PATTERN.test(requiredString(evidence.launch_request_id)) ||
    evidence.listing_revision_digest !== request.listing_revision_digest ||
    evidence.launch_request_digest !== request.launch_request_digest ||
    evidence.room_setup_operation_id !== request.room_setup_operation_id ||
    evidence.genesis_committed !== false ||
    !BLAKE3_PATTERN.test(requiredString(evidence.provisioning_fence_digest)) ||
    !/^[0-9a-f]{64}$/u.test(requiredString(evidence.authentication_tag)) ||
    Object.keys(evidence).sort().join(",") !==
      "authentication_tag,genesis_committed,host_installation_id,launch_request_digest,launch_request_id,listing_revision_digest,provisioning_fence_digest,room_setup_operation_id,schema"
  ) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
}

function validateLaunchClosureEvidence(
  evidence: CanonicalObject,
  request: CanonicalObject,
  hostInstallationId: string,
): void {
  const disposition = evidence.disposition;
  const roomId = evidence.room_id;
  const roomHead = evidence.room_head;
  const commonValid =
    evidence.schema === "worldstream/hosted-launch-closure-evidence/v1" &&
    evidence.host_installation_id === hostInstallationId &&
    evidence.launch_request_id === request.launch_request_id &&
    evidence.listing_revision_digest === request.listing_revision_digest &&
    evidence.launch_request_digest === request.launch_request_digest &&
    evidence.room_setup_operation_id === request.room_setup_operation_id &&
    BLAKE3_PATTERN.test(requiredString(evidence.closure_fence_digest)) &&
    /^[0-9a-f]{64}$/u.test(requiredString(evidence.authentication_tag)) &&
    Object.keys(evidence).sort().join(",") ===
      "authentication_tag,closure_fence_digest,disposition,host_installation_id,launch_request_digest,launch_request_id,listing_revision_digest,room_head,room_id,room_setup_operation_id,schema";
  const cancelled = disposition === "cancelled_before_genesis" && roomId === null && roomHead === null;
  const archived = disposition === "room_archived" &&
    typeof roomId === "string" &&
    /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/u.test(roomId) &&
    isArchivedRoomHead(roomHead, roomId);
  if (!commonValid || (!cancelled && !archived)) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
}

function isArchivedRoomHead(value: unknown, roomId: string): boolean {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const head = value as CanonicalObject;
  return head.room_id === roomId &&
    Number.isSafeInteger(head.room_seq) && Number(head.room_seq) > 0 &&
    head.core_schema_version === "worldstream.core-room-state.v1" &&
    [
      head.genesis_or_transition_hash,
      head.pack_digest,
      head.core_state_hash,
      head.activity_state_hash,
      head.authoritative_state_hash,
    ].every((digest) => typeof digest === "string" && BLAKE3_PATTERN.test(digest)) &&
    Object.keys(head).sort().join(",") ===
      "activity_state_hash,authoritative_state_hash,core_schema_version,core_state_hash,genesis_or_transition_hash,pack_digest,room_id,room_seq";
}

function validatePublicRelayReceipt(
  receipt: CanonicalObject,
  request: CanonicalObject,
  requestDigest: Uint8Array,
): void {
  if (
    receipt.schema !== "worldstream/hosted-public-relay-bind-receipt/v1" ||
    receipt.public_run_id !== request.public_run_id ||
    receipt.activity_run_id !== request.activity_run_id ||
    receipt.binding_request_digest !== `sha256:${Buffer.from(requestDigest).toString("hex")}` ||
    receipt.bound !== true ||
    Object.keys(receipt).sort().join(",") !==
      "activity_run_id,binding_request_digest,bound,public_run_id,schema"
  ) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
}

function requireFrozenIdentity(
  material: HostedLaunchMaterial,
  hostInstallationId: string,
  roomSetupOperationId: string,
): void {
  if (
    material.hostInstallationId !== hostInstallationId ||
    material.roomSetupOperationId !== roomSetupOperationId
  ) {
    throw new HostedFormationRejectedError("frozen_launch_mismatch");
  }
}

function requiredReviewedActivity(
  digest: string,
  resolve: (digest: string) => ReviewedHostedActivity | null,
): ReviewedHostedActivity {
  const reviewed = resolve(digest);
  if (reviewed === null) throw new HostedFormationRejectedError("listing_unavailable");
  return reviewed;
}

function serviceBase(value: string): URL {
  const url = new URL(value);
  const local = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (
    (!local && url.protocol !== "https:") ||
    (local && !["http:", "https:"].includes(url.protocol)) ||
    url.username !== "" ||
    url.password !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new HostedFormationRejectedError("invalid_gateway_configuration");
  }
  return url;
}

async function boundedResponse(response: Response, maximum: number): Promise<Uint8Array> {
  const reader = response.body?.getReader();
  if (reader === undefined) return new Uint8Array();
  const chunks: Uint8Array[] = [];
  let used = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    used += value.byteLength;
    if (used > maximum) {
      await reader.cancel();
      throw new HostedFormationUnavailableError("gateway_response_too_large");
    }
    chunks.push(value);
  }
  const result = new Uint8Array(used);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

function requiredCanonicalObject(value: unknown): CanonicalObject {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
  return value as CanonicalObject;
}

function requiredString(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new HostedFormationUnavailableError("invalid_gateway_response");
  }
  return value;
}

function nullableString(value: unknown): string | null {
  if (value === null) return null;
  return requiredString(value);
}

export function sha256(value: Uint8Array | string): Uint8Array {
  return createHash("sha256").update(value).digest();
}

export function taggedSha256(value: Uint8Array): string {
  return `sha256:${Buffer.from(sha256(value)).toString("hex")}`;
}

export function validLaunchIdentifier(value: string): boolean {
  return UUID_PATTERN.test(value);
}

export function validReleaseDigest(value: string): boolean {
  return BLAKE3_PATTERN.test(value) || SHA256_PATTERN.test(value);
}
