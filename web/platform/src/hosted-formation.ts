import { createHash } from "node:crypto";

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
  readonly state: "pending" | "ambiguous" | "succeeded" | "terminal_failed";
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
}

export interface HostedFormationGateway {
  reserveHouseRunner(request: CanonicalObject): Promise<CanonicalObject>;
  launch(request: CanonicalObject): Promise<HostedLaunchStatus>;
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
  ) {
    if (!SAFE_REFERENCE_PATTERN.test(hostInstallationId)) {
      throw new HostedFormationRejectedError("invalid_host_installation");
    }
  }

  async advance(accountId: string, launchRequestId: string): Promise<FormationAdvanceResult> {
    let material = await this.requiredMaterial(accountId, launchRequestId);
    const reviewed = requiredReviewedActivity(material.listingRevisionDigest);
    if (["cancelled", "expired", "failed_pre_genesis"].includes(material.state)) {
      return { state: "failed_pre_genesis", retryAfterSeconds: null, runId: null };
    }
    if (material.state === "run_created") return this.runCreated(launchRequestId);

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

    await this.gateway.launch(documents.gatewayLaunchRequest);
    try {
      const evidence = await this.gateway.readGenesisEvidence(documents.evidenceRequest);
      const evidenceBytes = encodeCanonical(evidence);
      const recorded = await this.data.recordGenesis(
        launchRequestId,
        evidenceBytes,
        sha256(evidenceBytes),
      );
      if (recorded === null) throw new HostedFormationRejectedError();
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

  private async runCreated(launchRequestId: string): Promise<FormationAdvanceResult> {
    const reconciliation = await this.data.readGenesisReconciliation(launchRequestId);
    if (reconciliation?.runId === null || reconciliation?.runId === undefined) {
      throw new HostedFormationRejectedError();
    }
    await this.ensurePublicRelayBinding(reconciliation.runId);
    return {
      state: "run_created",
      retryAfterSeconds: null,
      runId: reconciliation.runId,
    };
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

  async reserveHouseRunner(request: CanonicalObject): Promise<CanonicalObject> {
    return this.call("/v1/hosted/house-runners/reserve", request, false);
  }

  async launch(request: CanonicalObject): Promise<HostedLaunchStatus> {
    const response = await this.call("/v1/hosted/launch", request, false);
    if (
      response.schema !== "worldstream/hosted-launch-status/v1" ||
      response.launch_request_digest !== request.launch_request_digest ||
      response.room_setup_operation_id !== request.room_setup_operation_id
    ) {
      throw new HostedFormationUnavailableError("invalid_gateway_response");
    }
    return { state: requiredString(response.stage) };
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
      throw new HostedFormationUnavailableError(undefined, { cause: error });
    }
    if (!response.ok) {
      if (pendingOnConflict && response.status === 409) throw new HostedFormationPendingError();
      if ([400, 401, 403, 404, 409, 422].includes(response.status)) {
        throw new HostedFormationRejectedError();
      }
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
  const roomSetupOperationId = `launch-${material.launchRequestId.replaceAll("-", "")}`;
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

function requiredReviewedActivity(digest: string): ReviewedHostedActivity {
  const reviewed = reviewedActivityByDigest(digest);
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
