import { strict as assert } from "node:assert";
import { test } from "vitest";

import { encodeCanonical, type CanonicalObject } from "@worldstream/pack-sdk";

import {
  HostedFormationCoordinator,
  taggedSha256,
  type AccountParticipation,
  type FormationLaunchRecord,
  type GenesisReconciliation,
  type HostedFormationData,
  type HostedFormationGateway,
  type HostedLaunchMaterial,
  type HouseFillChoice,
  type HouseFillRecord,
  type LaunchCreationResult,
  type OwnedRunRecord,
} from "./hosted-formation.js";
import { AGENT_HEIST_LISTING_DIGEST } from "./hosted-catalog.js";

const ACCOUNT_ID = "10000000-0000-4000-8000-000000000001";
const LAUNCH_ID = "20000000-0000-4000-8000-000000000001";
const RUN_ID = "30000000-0000-4000-8000-000000000001";

class HumanFormationData implements HostedFormationData {
  material: HostedLaunchMaterial = {
    launchRequestId: LAUNCH_ID,
    listingRevisionDigest: AGENT_HEIST_LISTING_DIGEST,
    state: "collecting_roster",
    expiresAt: "2099-01-01T00:00:00.000Z",
    launchInputs: {},
    houseFillChoice: "disabled",
    creatorAccessChoice: "seat",
    creatorSeatId: "navigator",
    hostInstallationId: null,
    roomSetupOperationId: null,
    rosterFrozen: false,
    hostMutationStarted: false,
    claims: [
      {
        seatId: "navigator",
        displayName: "Navigator",
        participationKind: "account_human",
        principalReference: "account:creator:navigator",
      },
      {
        seatId: "insider",
        displayName: "Insider",
        participationKind: "account_external_agent",
        principalReference: "account:invitee:insider",
      },
    ],
    houseAssignments: [],
  };
  runId: string | null = null;
  freezeCalls = 0;
  authorizeCalls = 0;
  publicBindingRequest: CanonicalObject | null = null;
  publicBindingRecords = 0;

  async readHostedLaunchMaterial(accountId: string, launchRequestId: string) {
    return accountId === ACCOUNT_ID && launchRequestId === LAUNCH_ID ? this.material : null;
  }

  async freezeLaunch(input: {
    accountId: string;
    launchRequestId: string;
    frozenRoster: Uint8Array;
    frozenRosterDigest: Uint8Array;
    frozenRoomSetup: Uint8Array;
    frozenRoomSetupDigest: string;
    hostInstallationId: string;
    roomSetupOperationId: string;
  }) {
    assert.equal(input.accountId, ACCOUNT_ID);
    assert.equal(input.launchRequestId, LAUNCH_ID);
    assert.equal(input.frozenRosterDigest.byteLength, 32);
    assert.match(input.frozenRoomSetupDigest, /^blake3:[0-9a-f]{64}$/u);
    assert.equal(JSON.parse(new TextDecoder().decode(input.frozenRoster)).members.length, 2);
    assert.equal(JSON.parse(new TextDecoder().decode(input.frozenRoomSetup)).pack.id, "worldstream.agent-heist");
    this.freezeCalls += 1;
    this.material = {
      ...this.material,
      state: "host_authorized",
      rosterFrozen: true,
      hostInstallationId: input.hostInstallationId,
      roomSetupOperationId: input.roomSetupOperationId,
    };
    return true;
  }

  async authorizeHostMutation(
    accountId: string,
    launchRequestId: string,
    hostInstallationId: string,
    roomSetupOperationId: string,
  ) {
    assert.equal(accountId, ACCOUNT_ID);
    assert.equal(launchRequestId, LAUNCH_ID);
    assert.equal(hostInstallationId, this.material.hostInstallationId);
    assert.equal(roomSetupOperationId, this.material.roomSetupOperationId);
    this.authorizeCalls += 1;
    return true;
  }

  async recordGenesis(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ) {
    assert.equal(launchRequestId, LAUNCH_ID);
    assert.equal(evidenceDigest.byteLength, 32);
    assert.equal(JSON.parse(new TextDecoder().decode(canonicalEvidence)).schema, "worldstream/hosted-genesis-evidence/v1");
    const evidence = JSON.parse(new TextDecoder().decode(canonicalEvidence)) as Record<string, unknown>;
    this.publicBindingRequest = {
      schema: "worldstream/hosted-public-relay-bind-request/v1",
      public_run_id: "0123456789abcdef0123456789abcdef",
      activity_run_id: RUN_ID,
      host_installation_id: "hosted-preview-1",
      launch_request_id: LAUNCH_ID,
      listing_revision_digest: AGENT_HEIST_LISTING_DIGEST,
      launch_request_digest: String(evidence.launch_request_digest),
      room_setup_operation_id: String(evidence.room_setup_operation_id),
      room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      pack: {
        id: "worldstream.agent-heist",
        version: "0.2.0",
        digest: `blake3:${"d".repeat(64)}`,
      },
      relay_principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      relay_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
    };
    this.runId = RUN_ID;
    this.material = { ...this.material, state: "run_created" };
    return { runId: RUN_ID, reconciliationState: "ready" as const };
  }

  async readGenesisReconciliation(): Promise<GenesisReconciliation> {
    return {
      launchState: this.runId === null ? "host_authorized" : "run_created",
      runId: this.runId,
      reconciliationState: "ready",
      needsGenesisPull: false,
    };
  }

  async readPublicRelayBindingCandidate(runId: string) {
    assert.equal(runId, RUN_ID);
    return this.publicBindingRequest;
  }

  async recordPublicRelayBinding(input: {
    runId: string;
    canonicalRequest: Uint8Array;
    requestDigest: Uint8Array;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }) {
    assert.equal(input.runId, RUN_ID);
    assert.equal(input.requestDigest.byteLength, 32);
    assert.equal(input.receiptDigest.byteLength, 32);
    assert.deepEqual(
      JSON.parse(new TextDecoder().decode(input.canonicalRequest)),
      this.publicBindingRequest,
    );
    assert.equal(
      JSON.parse(new TextDecoder().decode(input.canonicalReceipt)).bound,
      true,
    );
    this.publicBindingRecords += 1;
    return true;
  }

  async createLaunchRequest(_input: {
    accountId: string;
    listingRevisionDigest: string;
    idempotencyNamespace: string;
    idempotencyKeyDigest: Uint8Array;
    canonicalLaunchInput: Uint8Array;
    launchInputDigest: Uint8Array;
    houseFillChoice: HouseFillChoice;
    creatorAccessChoice: "seat" | "spectator";
    creatorSeatId: string | null;
  }): Promise<LaunchCreationResult> { throw new Error("unused"); }
  async readLaunchRequest(_accountId: string, _launchRequestId: string): Promise<FormationLaunchRecord | null> { throw new Error("unused"); }
  async rotateSeatInvitation(_accountId: string, _launchRequestId: string, _seatId: string): Promise<{ token: string; expiresAt: string } | null> { throw new Error("unused"); }
  async claimInvitedSeat(_accountId: string, _tokenDigest: Uint8Array, _participation: AccountParticipation): Promise<{ launchRequestId: string; seatId: string } | null> { throw new Error("unused"); }
  async releaseSeatClaim(_accountId: string, _launchRequestId: string, _seatId: string): Promise<boolean> { throw new Error("unused"); }
  async resetSeatClaim(_accountId: string, _launchRequestId: string, _seatId: string): Promise<boolean> { throw new Error("unused"); }
  async cancelLaunchRequest(_accountId: string, _launchRequestId: string): Promise<boolean> { throw new Error("unused"); }
  async startHouseFill(_accountId: string, _launchRequestId: string): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async readHouseFill(_accountId: string, _launchRequestId: string): Promise<HouseFillRecord | null> { return null; }
  async retainHouseFillSelection(_launchRequestId: string, _hostInstallationId: string): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async recordHouseRunnerReservation(_input: never): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async completeHouseFill(_launchRequestId: string): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async readOwnedRun(_accountId: string, _runId: string): Promise<OwnedRunRecord | null> { throw new Error("unused"); }
}

class RecordingGateway implements HostedFormationGateway {
  launches: CanonicalObject[] = [];
  publicBindings: CanonicalObject[] = [];

  async reserveHouseRunner(_request: CanonicalObject): Promise<CanonicalObject> {
    throw new Error("unused");
  }

  async launch(request: CanonicalObject) {
    this.launches.push(request);
    return { state: "launched" };
  }

  async readGenesisEvidence(request: CanonicalObject): Promise<CanonicalObject> {
    return {
      schema: "worldstream/hosted-genesis-evidence/v1",
      listing_revision_digest: String(request.listing_revision_digest),
      launch_request_digest: String(request.launch_request_digest),
      room_setup_operation_id: String(request.room_setup_operation_id),
    };
  }

  async bindPublicRelay(request: CanonicalObject): Promise<CanonicalObject> {
    this.publicBindings.push(request);
    return {
      schema: "worldstream/hosted-public-relay-bind-receipt/v1",
      public_run_id: String(request.public_run_id),
      activity_run_id: String(request.activity_run_id),
      binding_request_digest: taggedSha256(encodeCanonical(request)),
      bound: true,
    };
  }
}

test("the stateless coordinator freezes one reviewed roster and resumes the same Run", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");

  assert.deepEqual(await coordinator.advance(ACCOUNT_ID, LAUNCH_ID), {
    state: "run_created",
    retryAfterSeconds: null,
    runId: RUN_ID,
  });
  assert.equal(data.freezeCalls, 1);
  assert.equal(data.authorizeCalls, 1);
  assert.equal(gateway.launches.length, 1);
  assert.equal(gateway.publicBindings.length, 1);
  assert.equal(data.publicBindingRecords, 1);
  const request = gateway.launches[0];
  assert.equal(request?.schema, "worldstream/hosted-launch-request/v1");
  assert.equal(request?.listing_revision_digest, AGENT_HEIST_LISTING_DIGEST);
  assert.deepEqual(request?.house_runner_assignments, []);
  assert.equal(JSON.stringify(request).includes("provider"), false);

  assert.deepEqual(await coordinator.advance(ACCOUNT_ID, LAUNCH_ID), {
    state: "run_created",
    retryAfterSeconds: null,
    runId: RUN_ID,
  });
  assert.equal(data.freezeCalls, 1);
  assert.equal(gateway.launches.length, 1);
  assert.equal(gateway.publicBindings.length, 2);
  assert.equal(data.publicBindingRecords, 2);
});
