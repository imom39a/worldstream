import { strict as assert } from "node:assert";
import { test } from "vitest";

import { encodeCanonical, type CanonicalObject } from "@worldstream/pack-sdk";
import { readListingRevision } from "@worldstream/hosted-contract";

import {
  HostedFormationCoordinator,
  HostedFormationRejectedError,
  HostedFormationUnavailableError,
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
import {
  AGENT_HEIST_LISTING_DIGEST,
  type ReviewedHostedActivity,
} from "./hosted-catalog.js";

const ACCOUNT_ID = "10000000-0000-4000-8000-000000000001";
const LAUNCH_ID = "20000000-0000-4000-8000-000000000001";
const RUN_ID = "30000000-0000-4000-8000-000000000001";

type FormationFixture = {
  readonly listingRevisionDigest: string;
  readonly pack: { readonly id: string; readonly version: string; readonly digest: string };
  readonly creatorSeatId: string;
  readonly claims: HostedLaunchMaterial["claims"];
};

const DEFAULT_FORMATION_FIXTURE: FormationFixture = {
  listingRevisionDigest: AGENT_HEIST_LISTING_DIGEST,
  pack: { id: "worldstream.agent-heist", version: "0.2.0", digest: `blake3:${"d".repeat(64)}` },
  creatorSeatId: "navigator",
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
};

const NEGOTIATE_FORMATION_FIXTURE: FormationFixture = {
  listingRevisionDigest: `blake3:${"e".repeat(64)}`,
  pack: { id: "worldstream.negotiate", version: "0.2.0", digest: `blake3:${"f".repeat(64)}` },
  creatorSeatId: "buyer",
  claims: [
    {
      seatId: "buyer",
      displayName: "Buyer Agent",
      participationKind: "account_human",
      principalReference: "account:creator:buyer",
    },
    {
      seatId: "seller",
      displayName: "Seller Agent",
      participationKind: "account_external_agent",
      principalReference: "account:invitee:seller",
    },
    {
      seatId: "approver",
      displayName: "Buyer Approver",
      participationKind: "account_human",
      principalReference: "account:creator:approver",
    },
  ],
};

function reviewedFormationFixture(fixture: FormationFixture): ReviewedHostedActivity {
  const listing = readListingRevision(encodeCanonical({
    schema: "worldstream/activity-listing-revision/v1",
    listing_id: `${fixture.pack.id}.preview`,
    version: fixture.pack.version,
    title: fixture.pack.id === "worldstream.negotiate" ? "Negotiate" : "Agent Heist",
    description: "A reviewed contract fixture.",
    catalog: { visibility: "public", review_status: "reviewed" },
    pack: fixture.pack,
    client: {
      client_id: `${fixture.pack.id}.web`,
      release_digest: `sha256:${"1".repeat(64)}`,
      client_contract: "worldstream/activity-client-protocol/v1",
      surface_id: "fixture-web",
    },
    launch_input_schema: {
      schema: "worldstream/launch-input-schema/v1",
      accepts: "none",
      defaults: {},
    },
    room_setup: { configuration: { fixture: true } },
    seats: fixture.claims.map((claim) => ({
      seat_id: claim.seatId,
      role: claim.seatId,
      display_name: claim.displayName,
      required: true,
      allowed_participation: ["account_human", "account_external_agent"],
      allowed_house_agent_revisions: [],
    })),
    creator_access: "must_claim_seat",
    public_viewing_policy: "anonymous_by_link",
    pre_start_deadline_seconds: 300,
    result: {
      projection: { schema: "fixture/projection/v1", digest: `blake3:${"2".repeat(64)}` },
      projector: {
        id: `${fixture.pack.id}.result`,
        version: "0.1.0",
        digest: `blake3:${"3".repeat(64)}`,
      },
      publication: {
        policy: "public_recent_results",
        attribution: "reviewed_pseudonymous_seats",
        public_output: "projector_summary_only",
        suppression: "unhealthy_inconclusive_or_conflict",
      },
    },
  }));
  return {
    slug: fixture.pack.id === "worldstream.negotiate" ? "negotiate" : "agent-heist",
    listing,
    houseAgents: new Map(),
    public: {
      slug: fixture.pack.id === "worldstream.negotiate" ? "negotiate" : "agent-heist",
      title: listing.value.title,
      description: listing.value.description,
      availability: "available",
      availabilityMessage: "Ready for the contract fixture",
      seatSummary: `${listing.value.seats.length} required seats`,
      seats: listing.value.seats.map((seat) => ({
        key: seat.seat_id,
        label: seat.display_name,
        required: seat.required,
      })),
      creatorMaySpectate: false,
      houseFillAvailable: false,
      publicViewingAvailable: true,
      resultPublication: "Reviewed fixture result",
      attribution: "Reviewed fixture seats",
      clientPath: "/fixture/",
      publicViewerClientPath: "/fixture/",
      houseTerms: null,
    },
  } satisfies ReviewedHostedActivity;
}

class HumanFormationData implements HostedFormationData {
  material: HostedLaunchMaterial;
  constructor(private readonly fixture: FormationFixture = DEFAULT_FORMATION_FIXTURE) {
    this.material = {
    launchRequestId: LAUNCH_ID,
    listingRevisionDigest: fixture.listingRevisionDigest,
    state: "collecting_roster",
    expiresAt: "2099-01-01T00:00:00.000Z",
    launchInputs: {},
    houseFillChoice: "disabled",
    creatorAccessChoice: "seat",
    creatorSeatId: fixture.creatorSeatId,
    hostInstallationId: null,
    roomSetupOperationId: null,
    rosterFrozen: false,
    hostMutationStarted: false,
    claims: fixture.claims,
    houseAssignments: [],
    };
  }
  runId: string | null = null;
  freezeCalls = 0;
  authorizeCalls = 0;
  publicBindingRequest: CanonicalObject | null = null;
  publicBindingRecords = 0;
  prestartAbandonmentRecords = 0;
  provisioningAbandonmentRecords = 0;

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
    assert.equal(JSON.parse(new TextDecoder().decode(input.frozenRoster)).members.length, this.fixture.claims.length);
    assert.equal(JSON.parse(new TextDecoder().decode(input.frozenRoomSetup)).pack.id, this.fixture.pack.id);
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
      listing_revision_digest: this.fixture.listingRevisionDigest,
      launch_request_digest: String(evidence.launch_request_digest),
      room_setup_operation_id: String(evidence.room_setup_operation_id),
      room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      pack: this.fixture.pack,
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
  async recordPrestartAbandonment(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean> {
    assert.equal(launchRequestId, LAUNCH_ID);
    assert.equal(evidenceDigest.byteLength, 32);
    assert.equal(
      JSON.parse(new TextDecoder().decode(canonicalEvidence)).lobby_launch_committed,
      false,
    );
    this.prestartAbandonmentRecords += 1;
    this.material = { ...this.material, state: "abandoned_prestart" };
    return true;
  }
  async recordProvisioningAbandonment(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean> {
    assert.equal(launchRequestId, LAUNCH_ID);
    assert.equal(evidenceDigest.byteLength, 32);
    assert.equal(
      JSON.parse(new TextDecoder().decode(canonicalEvidence)).genesis_committed,
      false,
    );
    this.provisioningAbandonmentRecords += 1;
    this.material = { ...this.material, state: "failed_pre_genesis" };
    return true;
  }
  async listPrestartAbandonmentCandidates() { return []; }
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
  prestartAbandonments: CanonicalObject[] = [];
  provisioningAbandonments: CanonicalObject[] = [];
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

  async abandonPrestart(request: CanonicalObject): Promise<CanonicalObject> {
    this.prestartAbandonments.push(request);
    return {
      schema: "worldstream/hosted-prestart-abandonment-evidence/v1",
      host_installation_id: "hosted-preview-1",
      launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
      listing_revision_digest: String(request.listing_revision_digest),
      launch_request_digest: String(request.launch_request_digest),
      room_setup_operation_id: String(request.room_setup_operation_id),
      room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      lobby_launch_committed: false,
      abandonment_fence_digest: `blake3:${"a".repeat(64)}`,
      authentication_tag: "b".repeat(64),
    };
  }

  async abandonProvisioning(request: CanonicalObject): Promise<CanonicalObject> {
    this.provisioningAbandonments.push(request);
    return {
      schema: "worldstream/hosted-provisioning-abandonment-evidence/v1",
      host_installation_id: "hosted-preview-1",
      launch_request_id: LAUNCH_ID,
      listing_revision_digest: String(request.listing_revision_digest),
      launch_request_digest: String(request.launch_request_digest),
      room_setup_operation_id: String(request.room_setup_operation_id),
      genesis_committed: false,
      provisioning_fence_digest: `blake3:${"a".repeat(64)}`,
      authentication_tag: "b".repeat(64),
    };
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

test("the formation boundary accepts a reviewed non-Heist roster with different seats", async () => {
  const data = new HumanFormationData(NEGOTIATE_FORMATION_FIXTURE);
  const gateway = new RecordingGateway();
  const reviewed = reviewedFormationFixture(NEGOTIATE_FORMATION_FIXTURE);
  data.material = { ...data.material, listingRevisionDigest: reviewed.listing.digest };
  const coordinator = new HostedFormationCoordinator(
    data,
    gateway,
    "hosted-preview-1",
    (digest) => digest === reviewed.listing.digest ? reviewed : null,
  );

  assert.deepEqual(await coordinator.advance(ACCOUNT_ID, LAUNCH_ID), {
    state: "run_created",
    retryAfterSeconds: null,
    runId: RUN_ID,
  });
  const request = gateway.launches[0];
  assert.equal(request?.listing_revision_digest, reviewed.listing.digest);
  const setup = request?.frozen_room_setup_specification as {
    readonly pack: { readonly id: string };
    readonly seats: readonly { readonly label: string; readonly role: string }[];
  };
  assert.equal(setup.pack.id, "worldstream.negotiate");
  assert.deepEqual(setup.seats.map(({ label, role }) => ({ label, role })), [
    { label: "buyer", role: "buyer" },
    { label: "seller", role: "seller" },
    { label: "approver", role: "approver" },
  ]);
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

test("post-Genesis abandonment uses one exact Host operation and remains restart-idempotent", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  await coordinator.advance(ACCOUNT_ID, LAUNCH_ID);

  assert.equal(await coordinator.abandonPrestart(LAUNCH_ID), true);
  assert.equal(data.material.state, "abandoned_prestart");
  assert.equal(data.prestartAbandonmentRecords, 1);
  assert.equal(gateway.prestartAbandonments.length, 1);
  assert.equal(gateway.prestartAbandonments[0]?.room_setup_operation_id, gateway.launches[0]?.room_setup_operation_id);

  assert.equal(await coordinator.abandonPrestart(LAUNCH_ID), true);
  assert.equal(data.prestartAbandonmentRecords, 2);
  assert.equal(gateway.prestartAbandonments.length, 2);
  assert.deepEqual(gateway.prestartAbandonments[1], gateway.prestartAbandonments[0]);
});

test("abandoned pre-start Run is terminal and cannot resume Host mutation", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  await coordinator.advance(ACCOUNT_ID, LAUNCH_ID);
  await coordinator.abandonPrestart(LAUNCH_ID);

  const launchCount = gateway.launches.length;
  const bindingCount = gateway.publicBindings.length;
  await assert.rejects(
    () => coordinator.advance(ACCOUNT_ID, LAUNCH_ID),
    (error: unknown) =>
      error instanceof Error &&
      error.name === "HostedFormationRejectedError" &&
      error.message === "launch_unavailable",
  );
  assert.equal(gateway.launches.length, launchCount);
  assert.equal(gateway.publicBindings.length, bindingCount);
  assert.equal(data.prestartAbandonmentRecords, 1);
});

test("a temporary entry-status outage retains the known Genesis and closes entry", async () => {
  const data = new HumanFormationData();
  const gateway = new RecordingGateway();
  const coordinator = new HostedFormationCoordinator(data, gateway, "hosted-preview-1");
  await coordinator.advance(ACCOUNT_ID, LAUNCH_ID);
  gateway.readStatus = async () => {
    throw new HostedFormationUnavailableError();
  };

  assert.equal(await coordinator.entryReady(LAUNCH_ID), false);
  assert.equal(data.runId, RUN_ID);
  assert.equal(data.freezeCalls, 1);
  assert.equal(data.authorizeCalls, 1);
  assert.equal(gateway.launches.length, 1);
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

test("recovery fences only an exact missing read followed by an exact replay conflict", async () => {
  const data = new HumanFormationData();
  const initial = new RecordingGateway();
  await new HostedFormationCoordinator(data, initial, "hosted-preview-1")
    .advance(ACCOUNT_ID, LAUNCH_ID);
  data.material = { ...data.material, state: "provisioning" };
  data.runId = null;
  const paths: string[] = [];
  const gateway = new HttpHostedFormationGateway({
    baseUrl: "http://127.0.0.1:8080",
    serviceAuthority: "test-service-authority-with-at-least-32-characters",
    fetchImplementation: async (url, init) => {
      const path = new URL(String(url)).pathname;
      const body = JSON.parse(String(init?.body)) as CanonicalObject;
      paths.push(path);
      if (path.endsWith("/genesis-evidence") || path.endsWith("/evidence")) {
        return new Response(null, { status: 404 });
      }
      if (path.endsWith("/launch")) return new Response(null, { status: 409 });
      if (path.endsWith("/abandon-provisioning")) {
        return Response.json({
          schema: "worldstream/hosted-provisioning-abandonment-evidence/v1",
          host_installation_id: "hosted-preview-1",
          launch_request_id: LAUNCH_ID,
          listing_revision_digest: body.listing_revision_digest,
          launch_request_digest: body.launch_request_digest,
          room_setup_operation_id: body.room_setup_operation_id,
          genesis_committed: false,
          provisioning_fence_digest: `blake3:${"a".repeat(64)}`,
          authentication_tag: "b".repeat(64),
        });
      }
      throw new Error(`unexpected ${path}`);
    },
  });

  assert.deepEqual(await new HostedFormationCoordinator(data, gateway, "hosted-preview-1")
    .recover(LAUNCH_ID), {
    state: "failed_pre_genesis", retryAfterSeconds: null, runId: null,
  });
  assert.equal(data.provisioningAbandonmentRecords, 1);
  assert.deepEqual(paths, [
    "/v1/hosted/genesis-evidence",
    "/v1/hosted/evidence",
    "/v1/hosted/launch",
    "/v1/hosted/evidence",
    "/v1/hosted/abandon-provisioning",
  ]);
});

test("a replay conflict that finds the exact operation remains reconcilable", async () => {
  const data = new HumanFormationData();
  const initial = new RecordingGateway();
  await new HostedFormationCoordinator(data, initial, "hosted-preview-1")
    .advance(ACCOUNT_ID, LAUNCH_ID);
  data.material = { ...data.material, state: "provisioning" };
  data.runId = null;
  const paths: string[] = [];
  let evidenceReads = 0;
  const gateway = new HttpHostedFormationGateway({
    baseUrl: "http://127.0.0.1:8080",
    serviceAuthority: "test-service-authority-with-at-least-32-characters",
    fetchImplementation: async (url, init) => {
      const path = new URL(String(url)).pathname;
      const body = JSON.parse(String(init?.body)) as CanonicalObject;
      paths.push(path);
      if (path.endsWith("/genesis-evidence")) return new Response(null, { status: 404 });
      if (path.endsWith("/evidence")) {
        evidenceReads += 1;
        if (evidenceReads === 1) return new Response(null, { status: 404 });
        return Response.json({
          schema: "worldstream/hosted-launch-status/v1",
          launch_request_digest: body.launch_request_digest,
          room_setup_operation_id: body.room_setup_operation_id,
          stage: "provisioning",
          room_setup_complete: false,
        });
      }
      if (path.endsWith("/launch")) return new Response(null, { status: 409 });
      throw new Error(`unexpected ${path}`);
    },
  });

  assert.deepEqual(await new HostedFormationCoordinator(data, gateway, "hosted-preview-1")
    .recover(LAUNCH_ID), {
    state: "reconciling", retryAfterSeconds: 2, runId: null,
  });
  assert.equal(data.provisioningAbandonmentRecords, 0);
  assert.equal(data.material.state, "provisioning");
  assert.deepEqual(paths, [
    "/v1/hosted/genesis-evidence",
    "/v1/hosted/evidence",
    "/v1/hosted/launch",
    "/v1/hosted/evidence",
    "/v1/hosted/genesis-evidence",
  ]);
});

test("a Host refusal to fence an exact operation retains it for reconciliation", async () => {
  const data = new HumanFormationData();
  const initial = new RecordingGateway();
  await new HostedFormationCoordinator(data, initial, "hosted-preview-1")
    .advance(ACCOUNT_ID, LAUNCH_ID);
  data.material = { ...data.material, state: "provisioning" };
  data.runId = null;
  const paths: string[] = [];
  const gateway = new HttpHostedFormationGateway({
    baseUrl: "http://127.0.0.1:8080",
    serviceAuthority: "test-service-authority-with-at-least-32-characters",
    fetchImplementation: async (url) => {
      const path = new URL(String(url)).pathname;
      paths.push(path);
      if (path.endsWith("/genesis-evidence") || path.endsWith("/evidence")) {
        return new Response(null, { status: 404 });
      }
      if (path.endsWith("/launch") || path.endsWith("/abandon-provisioning")) {
        return new Response(null, { status: 409 });
      }
      throw new Error(`unexpected ${path}`);
    },
  });

  assert.deepEqual(await new HostedFormationCoordinator(data, gateway, "hosted-preview-1")
    .recover(LAUNCH_ID), {
    state: "reconciling", retryAfterSeconds: 2, runId: null,
  });
  assert.equal(data.provisioningAbandonmentRecords, 0);
  assert.equal(data.material.state, "provisioning");
  assert.deepEqual(paths, [
    "/v1/hosted/genesis-evidence",
    "/v1/hosted/evidence",
    "/v1/hosted/launch",
    "/v1/hosted/evidence",
    "/v1/hosted/abandon-provisioning",
    "/v1/hosted/genesis-evidence",
  ]);
});

test("a missing read without the exact replay conflict retains capacity for recovery", async () => {
  const data = new HumanFormationData();
  const initial = new RecordingGateway();
  await new HostedFormationCoordinator(data, initial, "hosted-preview-1")
    .advance(ACCOUNT_ID, LAUNCH_ID);
  data.material = { ...data.material, state: "provisioning" };
  data.runId = null;
  const gateway = new HttpHostedFormationGateway({
    baseUrl: "http://127.0.0.1:8080",
    serviceAuthority: "test-service-authority-with-at-least-32-characters",
    fetchImplementation: async (url) => {
      const path = new URL(String(url)).pathname;
      if (path.endsWith("/genesis-evidence") || path.endsWith("/evidence")) {
        return new Response(null, { status: 404 });
      }
      if (path.endsWith("/launch")) return new Response(null, { status: 503 });
      throw new Error(`unexpected ${path}`);
    },
  });
  await assert.rejects(
    () => new HostedFormationCoordinator(data, gateway, "hosted-preview-1").recover(LAUNCH_ID),
    /unavailable/u,
  );
  assert.equal(data.provisioningAbandonmentRecords, 0);
  assert.equal(data.material.state, "provisioning");
});

test("recorded Genesis is never converted into a provisioning abandonment", async () => {
  const data = new HumanFormationData();
  const initial = new RecordingGateway();
  await new HostedFormationCoordinator(data, initial, "hosted-preview-1")
    .advance(ACCOUNT_ID, LAUNCH_ID);
  const recorder = new RecordingGateway();
  const paths: string[] = [];
  const gateway = new HttpHostedFormationGateway({
    baseUrl: "http://127.0.0.1:8080",
    serviceAuthority: "test-service-authority-with-at-least-32-characters",
    fetchImplementation: async (url, init) => {
      const path = new URL(String(url)).pathname;
      const body = JSON.parse(String(init?.body)) as CanonicalObject;
      paths.push(path);
      if (path.endsWith("/genesis-evidence")) {
        return Response.json(await recorder.readGenesisEvidence(body));
      }
      if (path.endsWith("/evidence")) return new Response(null, { status: 404 });
      if (path.endsWith("/launch")) return new Response(null, { status: 409 });
      throw new Error(`unexpected ${path}`);
    },
  });
  await assert.rejects(
    () => new HostedFormationCoordinator(data, gateway, "hosted-preview-1").recover(LAUNCH_ID),
  );
  assert.equal(data.provisioningAbandonmentRecords, 0);
  assert.equal(data.material.state, "run_created");
  assert.deepEqual(paths, [
    "/v1/hosted/genesis-evidence",
    "/v1/hosted/evidence",
    "/v1/hosted/launch",
  ]);
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

test("a frozen v2 solo choice survives uncertain Genesis and coordinator restart without another setup", async () => {
  const fixture = { ...DEFAULT_FORMATION_FIXTURE, claims: DEFAULT_FORMATION_FIXTURE.claims.slice(0, 1) };
  const base = reviewedFormationFixture(DEFAULT_FORMATION_FIXTURE);
  const listing = readListingRevision(encodeCanonical({
    ...base.listing.value,
    seats: base.listing.value.seats.map((seat, index) => ({ ...seat, required: index === 0 })),
    launch_input_schema: { schema: "worldstream/launch-input-schema/v2", accepts: "roster_option", defaults: { roster_option: "solo" },
      roster_options: [{ option_id: "solo", label: "Solo", seat_ids: [fixture.creatorSeatId], configuration: { fixed: "solo" }, house_agent_assignments: [] }] },
  } as unknown as CanonicalObject));
  const reviewed = { ...base, listing };
  const data = new HumanFormationData(fixture);
  data.material = { ...data.material, listingRevisionDigest: listing.digest, launchInputs: { roster_option: "solo" } };
  const gateway = new RecordingGateway();
  gateway.roomSetupComplete = false;
  const makeCoordinator = () => new HostedFormationCoordinator(data, gateway, "hosted-preview-1", (digest) => digest === listing.digest ? reviewed : null);
  assert.equal((await makeCoordinator().advance(ACCOUNT_ID, LAUNCH_ID)).state, "reconciling");
  const retained = structuredClone(gateway.launches[0]!);
  assert.deepEqual((retained.frozen_launch_request as CanonicalObject).inputs, { roster_option: "solo" });
  assert.deepEqual((retained.frozen_room_setup_specification as CanonicalObject).configuration, { fixed: "solo" });
  assert.equal(((retained.frozen_room_setup_specification as CanonicalObject).seats as unknown[]).length, 1);
  await assert.rejects(() => makeCoordinator().advance("wrong-account", LAUNCH_ID), HostedFormationRejectedError);
  assert.equal((await makeCoordinator().recover(LAUNCH_ID))?.state, "reconciling");
  assert.deepEqual(gateway.launches[1], retained);
  gateway.roomSetupComplete = true;
  assert.equal((await makeCoordinator().recover(LAUNCH_ID))?.state, "run_created");
  assert.equal(data.freezeCalls, 1);
  assert.equal(data.authorizeCalls, 1);
  assert.equal(data.runId, RUN_ID);
  assert.ok(gateway.launches.every((launch) => JSON.stringify(launch) === JSON.stringify(retained)));
});
