import { strict as assert } from "node:assert";
import { test } from "vitest";

import { encodeCanonical, type CanonicalObject } from "@worldstream/pack-sdk";

import {
  HostedFormationCoordinator,
  HttpHostedFormationGateway,
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

  async readHostedRecoveryMaterial(launchRequestId: string) {
    return launchRequestId === LAUNCH_ID && this.material.hostMutationStarted ? this.material : null;
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
    this.material = { ...this.material, hostMutationStarted: true, state: "provisioning" };
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
  roomSetupComplete = true;
  observedState = "launched";

  async reserveHouseRunner(_request: CanonicalObject): Promise<CanonicalObject> {
    throw new Error("unused");
  }

  async launch(request: CanonicalObject) {
    this.launches.push(request);
    return { state: "launched", roomSetupComplete: this.roomSetupComplete };
  }

  async readStatus(_request: CanonicalObject) {
    return { state: this.observedState, roomSetupComplete: this.roomSetupComplete };
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

test("post-Genesis provisioning retries the same setup before offering Run entry", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  // Genesis evidence is available, but a capability reply was lost. The Host
  // must reconcile that exact setup before the platform advertises entry.
  gateway.roomSetupComplete = false;
  assert.deepEqual(await coordinator.advance(ACCOUNT_ID, LAUNCH_ID), {
    state: "reconciling", retryAfterSeconds: 2, runId: RUN_ID,
  });
  assert.equal(data.runId, RUN_ID);
  assert.equal(gateway.publicBindings.length, 0);
  assert.equal(await coordinator.entryReady(LAUNCH_ID), false);
  assert.equal((await coordinator.recover(LAUNCH_ID))?.state, "reconciling");
  assert.deepEqual(gateway.launches[0], gateway.launches[1]);
  gateway.roomSetupComplete = true;
  assert.equal((await coordinator.advance(ACCOUNT_ID, LAUNCH_ID)).state, "run_created");
  assert.equal(data.freezeCalls, 1);
  assert.equal(data.authorizeCalls, 1);
  assert.equal(await coordinator.entryReady(LAUNCH_ID), true);
  assert.equal(data.runId, RUN_ID);
});

test("a created Lobby still resumes its original startup while waiting for readiness", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  await coordinator.advance(ACCOUNT_ID, LAUNCH_ID);
  gateway.observedState = "waiting_for_readiness";
  assert.equal((await coordinator.recover(LAUNCH_ID))?.state, "run_created");
  assert.equal(gateway.launches.length, 2);
  assert.deepEqual(gateway.launches[1], gateway.launches[0]);
  assert.equal(data.freezeCalls, 1);
  assert.equal(data.authorizeCalls, 1);
  // Do not block the human entry needed to make the Lobby ready.
  assert.equal(await coordinator.entryReady(LAUNCH_ID), true);
  for (const state of ["launched", "needs_attention"]) {
    gateway.observedState = state;
    await coordinator.recover(LAUNCH_ID);
    assert.equal(gateway.launches.length, 2);
  }
});

test("recovery never authorizes an unstarted launch or changes its Host", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  assert.equal(await coordinator.recover(LAUNCH_ID), null);
  assert.equal(await coordinator.entryReady(LAUNCH_ID), false);
  assert.equal(data.authorizeCalls, 0);
  await coordinator.advance(ACCOUNT_ID, LAUNCH_ID);
  data.material = { ...data.material, hostInstallationId: "another-host" };
  await assert.rejects(() => coordinator.recover(LAUNCH_ID));
  assert.equal(gateway.launches.length, 1);
});

test("the HTTP formation boundary requires an explicit setup-complete flag", async () => {
  const request = {
    launch_request_digest: "fixture-digest",
    room_setup_operation_id: "fixture-operation",
  };
  for (const complete of [false, true, undefined, "true"]) {
    const gateway = new HttpHostedFormationGateway({
      baseUrl: "http://127.0.0.1:8080",
      serviceAuthority: "test-service-authority-with-at-least-32-characters",
      fetchImplementation: async () => new Response(JSON.stringify({
        schema: "worldstream/hosted-launch-status/v1",
        ...request,
        stage: "provisioning",
        room_setup_complete: complete,
      })),
    });
    if (typeof complete === "boolean") {
      assert.deepEqual(await gateway.launch(request), {
        state: "provisioning", roomSetupComplete: complete,
      });
    } else {
      await assert.rejects(() => gateway.launch(request), /invalid_gateway_response/u);
    }
  }
});

test("a lost first Host request is recovered with the original authorization and request", async () => {
  const data = new HumanFormationData();
  let unavailable = true;
  const launches: unknown[] = [];
  const recorder = new RecordingGateway();
  const gateway = new HttpHostedFormationGateway({
    baseUrl: "http://127.0.0.1:8080",
    serviceAuthority: "test-service-authority-with-at-least-32-characters",
    fetchImplementation: async (url, init) => {
      const path = new URL(String(url)).pathname;
      const body = JSON.parse(String(init?.body)) as CanonicalObject;
      if (path.endsWith("/launch")) {
        launches.push(body);
        if (unavailable) { unavailable = false; return new Response(null, { status: 503 }); }
        return Response.json({ schema: "worldstream/hosted-launch-status/v1",
          launch_request_digest: body.launch_request_digest, room_setup_operation_id: body.room_setup_operation_id,
          stage: "launched", room_setup_complete: true });
      }
      if (launches.length < 2) return new Response(null, { status: 404 });
      if (path.endsWith("/genesis-evidence")) return Response.json(await recorder.readGenesisEvidence(body));
      return Response.json(await recorder.bindPublicRelay(body));
    },
  });
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  await assert.rejects(() => coordinator.advance(ACCOUNT_ID, LAUNCH_ID), /unavailable/u);
  assert.equal((await coordinator.recover(LAUNCH_ID))?.state, "run_created");
  assert.equal(data.freezeCalls, 1);
  assert.equal(data.authorizeCalls, 1);
  assert.deepEqual(launches[0], launches[1]);
});
