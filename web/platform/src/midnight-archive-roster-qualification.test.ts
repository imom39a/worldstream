import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { test } from "vitest";

import {
  readHouseAgentRevision,
  readListingRevision,
  resolveRosterOption,
  type HouseAgentRevision,
} from "@worldstream/hosted-contract";
import { encodeCanonical, type CanonicalObject } from "@worldstream/pack-sdk";

import {
  HostedFormationCoordinator,
  HostedFormationRejectedError,
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
import type { ReviewedHostedActivity } from "./hosted-catalog.js";

const ACCOUNT_ID = "10000000-0000-4000-8000-000000000001";
const HOST_ID = "archive-roster-qualification";
const MIRA_DIGEST = "blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde";
const JONAH_DIGEST = "blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0";

function artifact(path: string): CanonicalObject {
  return JSON.parse(readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8")) as CanonicalObject;
}

const listing = readListingRevision(encodeCanonical(artifact("config/hosted/listings/midnight-archive-0.4.0.json")));
const houseAgents = [
  readHouseAgentRevision(encodeCanonical(artifact("config/hosted/house-agents/mira-1.json"))),
  readHouseAgentRevision(encodeCanonical(artifact("config/hosted/house-agents/jonah-1.json"))),
];
const reviewed: ReviewedHostedActivity = {
  slug: "midnight-archive",
  listing,
  houseAgents: new Map(houseAgents.map((revision) => [revision.digest, revision])),
  public: {
    slug: "midnight-archive",
    title: listing.value.title,
    description: listing.value.description,
    availability: "available",
    availabilityMessage: "Qualification fixture",
    seatSummary: "1 lead · up to 2 supplied specialists",
    participationKinds: ["human"],
    seats: listing.value.seats.map((seat) => ({
      key: seat.seat_id,
      label: seat.display_name,
      required: seat.required,
    })),
    creatorMaySpectate: false,
    houseFillAvailable: true,
    publicViewingAvailable: false,
    resultPublication: "Private",
    attribution: "None",
    clientPath: "/midnight-archive-v14/hosted/",
    publicViewerClientPath: null,
    houseTerms: {
      exhibition: true,
      includedAtNoCharge: true,
      maximumAgents: 2,
      maximumCallsPerAgent: 10,
      maximumInputTokensPerAgent: 120_000,
      maximumOutputTokensPerAgent: 10_000,
      callTimeoutSeconds: 60,
    },
  },
};

const expectations = [
  { option: "solo", seats: ["lead"], house: [] },
  { option: "mira", seats: ["lead", "mira"], house: ["mira"] },
  { option: "jonah", seats: ["lead", "jonah"], house: ["jonah"] },
  { option: "full-crew", seats: ["lead", "mira", "jonah"], house: ["mira", "jonah"] },
] as const;

function uuid(lineage: number, kind: 2 | 3): string {
  return `${kind}0000000-0000-4000-8000-${lineage.toString().padStart(12, "0")}`;
}

function houseRevision(seatId: "mira" | "jonah"): HouseAgentRevision {
  const digest = seatId === "mira" ? MIRA_DIGEST : JONAH_DIGEST;
  const revision = reviewed.houseAgents.get(digest);
  assert.ok(revision);
  return revision;
}

function assignment(launchId: string, seatId: "mira" | "jonah", index: number) {
  const revision = houseRevision(seatId);
  return {
    assignmentId: uuid(index + 10, 2),
    seatId,
    displayName: revision.value.display_name,
    principalReference: `house:${launchId}:${seatId}`,
    houseAgentRevisionDigest: revision.digest,
    agentProfile: {
      profileId: revision.value.agent_profile.profile_id,
      revision: revision.value.agent_profile.revision,
    },
    runnerTemplate: {
      templateId: revision.value.runner_template.template_id,
      revision: revision.value.runner_template.revision,
    },
    reservationReceipt: {
      schema: "worldstream/house-runner-reservation-receipt/v1",
      reservation_operation_id: `reserve-${index}-${seatId}`,
      runner_unit_id: `archive-${index}-${seatId}`,
      outcome: "succeeded",
    } satisfies CanonicalObject,
  };
}

class RosterData implements HostedFormationData {
  material: HostedLaunchMaterial;
  readonly runId: string;
  freezeCalls = 0;
  authorizeCalls = 0;
  genesisRecords = 0;
  frozenRoster: CanonicalObject | null = null;
  frozenSetup: CanonicalObject | null = null;
  readonly closureRequests: CanonicalObject[] = [];
  readonly closureEvidence: CanonicalObject[] = [];
  failHouseFill = false;

  constructor(
    readonly option: (typeof expectations)[number],
    readonly launchId: string,
    lineage: number,
  ) {
    this.runId = uuid(lineage, 3);
    this.material = {
      launchRequestId: launchId,
      listingRevisionDigest: listing.digest,
      state: "collecting_roster",
      expiresAt: "2099-01-01T00:00:00.000Z",
      launchInputs: { roster_option: option.option },
      houseFillChoice: option.house.length === 0 ? "disabled" : "fill_unclaimed",
      creatorAccessChoice: "seat",
      creatorSeatId: "lead",
      hostInstallationId: null,
      roomSetupOperationId: null,
      rosterFrozen: false,
      hostMutationStarted: false,
      claims: [{
        seatId: "lead",
        displayName: "Expedition lead",
        participationKind: "account_human",
        principalReference: `account:${launchId}:lead`,
      }],
      houseAssignments: option.house.map((seatId, index) => assignment(launchId, seatId, lineage * 10 + index)),
    };
  }

  async readHostedLaunchMaterial(accountId: string, launchRequestId: string) {
    return accountId === ACCOUNT_ID && launchRequestId === this.launchId ? this.material : null;
  }

  async readHostedRecoveryMaterial(launchRequestId: string) {
    return launchRequestId === this.launchId &&
      (this.material.hostMutationStarted || this.material.state === "closing")
      ? this.material
      : null;
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
    assert.equal(input.launchRequestId, this.launchId);
    this.freezeCalls += 1;
    this.frozenRoster = JSON.parse(new TextDecoder().decode(input.frozenRoster)) as CanonicalObject;
    this.frozenSetup = JSON.parse(new TextDecoder().decode(input.frozenRoomSetup)) as CanonicalObject;
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
    assert.equal(launchRequestId, this.launchId);
    assert.equal(hostInstallationId, HOST_ID);
    assert.equal(roomSetupOperationId, this.material.roomSetupOperationId);
    this.authorizeCalls += 1;
    this.material = { ...this.material, state: "provisioning", hostMutationStarted: true };
    return true;
  }

  async readGenesisReconciliation(): Promise<GenesisReconciliation> {
    return {
      launchState: this.genesisRecords === 0 ? "provisioning" : "run_created",
      runId: this.genesisRecords === 0 ? null : this.runId,
      reconciliationState: "ready",
      needsGenesisPull: false,
    };
  }

  async recordGenesis(launchRequestId: string, canonicalEvidence: Uint8Array, evidenceDigest: Uint8Array) {
    assert.equal(launchRequestId, this.launchId);
    assert.equal(evidenceDigest.byteLength, 32);
    assert.equal(
      (JSON.parse(new TextDecoder().decode(canonicalEvidence)) as CanonicalObject).schema,
      "worldstream/hosted-genesis-evidence/v1",
    );
    this.genesisRecords += 1;
    this.material = { ...this.material, state: "run_created" };
    return { runId: this.runId, reconciliationState: "ready" as const };
  }

  async readHouseFill(): Promise<HouseFillRecord | null> {
    if (this.option.house.length === 0) return null;
    return this.failHouseFill ? {
      state: "failed_pre_genesis",
      claimWindowClosesAt: "2000-01-01T00:00:00.000Z",
      failureCode: "runner_capacity_unavailable",
      reservations: [],
      assignments: [],
    } : {
      state: "assignments_complete",
      claimWindowClosesAt: "2000-01-01T00:00:00.000Z",
      failureCode: null,
      reservations: [],
      assignments: this.option.house.map((seatId) => ({ seatId, displayName: houseRevision(seatId).value.display_name })),
    };
  }

  async readPublicRelayBindingCandidate() { return null; }
  async createLaunchRequest(_input: {
    accountId: string; listingRevisionDigest: string; idempotencyNamespace: string;
    idempotencyKeyDigest: Uint8Array; canonicalLaunchInput: Uint8Array; launchInputDigest: Uint8Array;
    houseFillChoice: HouseFillChoice; creatorAccessChoice: "seat" | "spectator"; creatorSeatId: string | null;
  }): Promise<LaunchCreationResult> { throw new Error("unused"); }
  async readLaunchRequest(_accountId: string, _launchRequestId: string): Promise<FormationLaunchRecord | null> { throw new Error("unused"); }
  async rotateSeatInvitation(_accountId: string, _launchRequestId: string, _seatId: string): Promise<{ token: string; expiresAt: string } | null> { throw new Error("unused"); }
  async claimInvitedSeat(_accountId: string, _tokenDigest: Uint8Array, _participation: AccountParticipation): Promise<{ launchRequestId: string; seatId: string } | null> { throw new Error("unused"); }
  async releaseSeatClaim(_accountId: string, _launchRequestId: string, _seatId: string): Promise<boolean> { throw new Error("unused"); }
  async resetSeatClaim(_accountId: string, _launchRequestId: string, _seatId: string): Promise<boolean> { throw new Error("unused"); }
  async cancelLaunchRequest(_accountId: string, _launchRequestId: string): Promise<boolean> { throw new Error("unused"); }
  async requestLaunchClosure(input: {
    accountId: string;
    launchRequestId: string;
    hostInstallationId: string;
    canonicalRequest: Uint8Array;
    requestDigest: Uint8Array;
  }): Promise<boolean> {
    assert.equal(input.accountId, ACCOUNT_ID);
    assert.equal(input.launchRequestId, this.launchId);
    assert.equal(input.hostInstallationId, HOST_ID);
    assert.equal(input.requestDigest.byteLength, 32);
    const request = JSON.parse(new TextDecoder().decode(input.canonicalRequest)) as CanonicalObject;
    assert.equal(request.launch_request_id, this.launchId);
    this.closureRequests.push(request);
    this.material = { ...this.material, state: "closing" };
    return true;
  }
  async startHouseFill(_accountId: string, _launchRequestId: string): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async retainHouseFillSelection(_launchRequestId: string, _hostInstallationId: string): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async recordHouseRunnerReservation(_input: never): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async completeHouseFill(_launchRequestId: string): Promise<HouseFillRecord | null> { throw new Error("unused"); }
  async recordPrestartAbandonment(_launchRequestId: string, _canonicalEvidence: Uint8Array, _evidenceDigest: Uint8Array): Promise<boolean> { throw new Error("unused"); }
  async recordProvisioningAbandonment(_launchRequestId: string, _canonicalEvidence: Uint8Array, _evidenceDigest: Uint8Array): Promise<boolean> { throw new Error("unused"); }
  async recordLaunchClosure(
    launchRequestId: string,
    canonicalEvidence: Uint8Array,
    evidenceDigest: Uint8Array,
  ): Promise<boolean> {
    assert.equal(launchRequestId, this.launchId);
    assert.equal(evidenceDigest.byteLength, 32);
    const evidence = JSON.parse(new TextDecoder().decode(canonicalEvidence)) as CanonicalObject;
    assert.equal(evidence.schema, "worldstream/hosted-launch-closure-evidence/v1");
    this.closureEvidence.push(evidence);
    this.material = { ...this.material, state: "closed_by_creator" };
    return true;
  }
  async listPendingLaunchClosures(limit: number) {
    if (limit <= 0 || this.material.state !== "closing") return [];
    return [{ launchRequestId: this.launchId }].slice(0, limit);
  }
  async listPrestartAbandonmentCandidates(_limit: number) { return []; }
  async recordPublicRelayBinding(_input: {
    runId: string; canonicalRequest: Uint8Array; requestDigest: Uint8Array;
    canonicalReceipt: Uint8Array; receiptDigest: Uint8Array;
  }): Promise<boolean> { throw new Error("unused"); }
  async readOwnedRun(_accountId: string, _runId: string): Promise<OwnedRunRecord | null> { throw new Error("unused"); }
}

class RosterGateway implements HostedFormationGateway {
  readonly launches: CanonicalObject[] = [];
  readonly launchClosures: CanonicalObject[] = [];
  roomSetupComplete = false;

  async launch(request: CanonicalObject) {
    this.launches.push(structuredClone(request));
    return { state: this.roomSetupComplete ? "launched" as const : "waiting_for_readiness" as const, roomSetupComplete: this.roomSetupComplete, lobbyLaunchCommitted: this.roomSetupComplete, retryable: true, terminalBeforeGenesis: false };
  }

  async readStatus() {
    return { state: this.roomSetupComplete ? "launched" as const : "waiting_for_readiness" as const, roomSetupComplete: this.roomSetupComplete, lobbyLaunchCommitted: this.roomSetupComplete, retryable: true, terminalBeforeGenesis: false };
  }

  async readGenesisEvidence(request: CanonicalObject): Promise<CanonicalObject> {
    return {
      schema: "worldstream/hosted-genesis-evidence/v1",
      listing_revision_digest: String(request.listing_revision_digest),
      launch_request_digest: String(request.launch_request_digest),
      room_setup_operation_id: String(request.room_setup_operation_id),
    };
  }

  async closeLaunch(request: CanonicalObject): Promise<CanonicalObject> {
    this.launchClosures.push(structuredClone(request));
    const roomExists = this.launches.length > 0;
    const roomId = roomExists ? "01ARZ3NDEKTSV4RRFFQ69G5FAV" : null;
    return {
      schema: "worldstream/hosted-launch-closure-evidence/v1",
      host_installation_id: HOST_ID,
      launch_request_id: String(request.launch_request_id),
      listing_revision_digest: String(request.listing_revision_digest),
      launch_request_digest: String(request.launch_request_digest),
      room_setup_operation_id: String(request.room_setup_operation_id),
      disposition: roomExists ? "room_archived" : "cancelled_before_genesis",
      room_id: roomId,
      room_head: roomExists ? {
        room_id: roomId,
        room_seq: 2,
        genesis_or_transition_hash: `blake3:${"1".repeat(64)}`,
        core_schema_version: "worldstream.core-room-state.v1",
        pack_digest: `blake3:${"2".repeat(64)}`,
        core_state_hash: `blake3:${"3".repeat(64)}`,
        activity_state_hash: `blake3:${"4".repeat(64)}`,
        authoritative_state_hash: `blake3:${"5".repeat(64)}`,
      } : null,
      closure_fence_digest: `blake3:${"a".repeat(64)}`,
      authentication_tag: "b".repeat(64),
    };
  }

  async reserveHouseRunner(_request: CanonicalObject): Promise<CanonicalObject> { throw new Error("unused"); }
  async abandonPrestart(_request: CanonicalObject): Promise<CanonicalObject> { throw new Error("unused"); }
  async abandonProvisioning(_request: CanonicalObject): Promise<CanonicalObject> { throw new Error("unused"); }
  async bindPublicRelay(_request: CanonicalObject): Promise<CanonicalObject> { throw new Error("unused"); }
}

test("the production Listing exposes exactly four bounded Archive rosters and exact House identities", () => {
  assert.equal(listing.value.version, "0.4.0");
  assert.equal(listing.value.launch_input_schema.schema, "worldstream/launch-input-schema/v3");
  assert.deepEqual(houseAgents.map(({ digest }) => digest), [MIRA_DIGEST, JONAH_DIGEST]);
  assert.deepEqual(
    expectations.map(({ option }) => resolveRosterOption(listing, { roster_option: option })).map((option) => ({
      id: option?.option_id,
      seats: option?.seat_ids,
      house: option?.house_agent_assignments.map((assignment) => assignment.seat_id),
      scenario: option?.configuration,
      hasDescription: option !== null && "description" in option && Boolean(option.description),
    })),
    expectations.map((expected) => ({
      id: expected.option,
      seats: expected.seats,
      house: expected.house,
      scenario: { scenario_id: "standard-v1" },
      hasDescription: true,
    })),
  );
  assert.throws(() => resolveRosterOption(listing, { roster_option: "full-crew", prompt: "replace policy" }));
});

test("all four exact rosters survive post-Genesis readiness retries without changing seats or House lineage", async () => {
  const principalReferences = new Set<string>();
  for (const [index, expected] of expectations.entries()) {
    const launchId = uuid(index + 1, 2);
    const data = new RosterData(expected, launchId, index + 1);
    const gateway = new RosterGateway();
    const coordinator = () => new HostedFormationCoordinator(
      data,
      gateway,
      HOST_ID,
      (digest) => digest === listing.digest ? reviewed : null,
    );

    assert.deepEqual(await coordinator().advance(ACCOUNT_ID, launchId), {
      state: "reconciling",
      retryAfterSeconds: 2,
      runId: data.runId,
    });
    assert.equal(data.freezeCalls, 1);
    assert.equal(data.authorizeCalls, 1);
    assert.equal(data.genesisRecords, 1);
    assert.equal(data.material.state, "run_created");
    assert.ok(data.frozenRoster);
    assert.ok(data.frozenSetup);
    assert.deepEqual(
      (data.frozenRoster.members as CanonicalObject[]).map((member) => member.seat_id),
      expected.seats,
    );
    assert.deepEqual(
      (data.frozenSetup.seats as CanonicalObject[]).map((seat) => seat.label),
      expected.seats,
    );
    assert.deepEqual(data.frozenSetup.configuration, { scenario_id: "standard-v1" });
    assert.deepEqual(
      (data.frozenRoster.members as CanonicalObject[])
        .filter((member) => member.participation === "house_agent_fill")
        .map((member) => member.seat_id),
      expected.house,
    );
    for (const member of (data.frozenRoster.members as CanonicalObject[]).filter(
      (candidate) => candidate.participation === "house_agent_fill",
    )) {
      const reference = String(member.principal_reference);
      assert.equal(reference, `house:${launchId}:${String(member.seat_id)}`);
      assert.equal(principalReferences.has(reference), false, "House principal reference escaped its Run lineage");
      principalReferences.add(reference);
    }

    const firstRequest = gateway.launches[0];
    assert.ok(firstRequest);
    const retainedRequest = structuredClone(firstRequest);
    assert.deepEqual(
      (retainedRequest.frozen_launch_request as CanonicalObject).inputs,
      { roster_option: expected.option },
    );
    assert.equal(
      ((retainedRequest.house_runner_assignments as CanonicalObject[]) ?? []).length,
      expected.house.length,
    );
    assert.equal((await coordinator().recover(launchId))?.state, "reconciling");
    assert.deepEqual(gateway.launches[1], retainedRequest);
    gateway.roomSetupComplete = true;
    assert.deepEqual(await coordinator().recover(launchId), {
      state: "run_created",
      retryAfterSeconds: null,
      runId: data.runId,
    });
    assert.equal(data.freezeCalls, 1);
    assert.equal(data.authorizeCalls, 1);
    assert.ok(data.genesisRecords >= 1);
    assert.equal(data.material.state, "run_created");
    assert.ok(gateway.launches.every((request) => JSON.stringify(request) === JSON.stringify(retainedRequest)));
    await assert.rejects(() => coordinator().advance("wrong-account", launchId), HostedFormationRejectedError);
  }
  assert.equal(principalReferences.size, 4);
});

test("a supplied-roster failure before Genesis creates no setup, Run, or partial Assignment", async () => {
  const expected = expectations[3];
  const launchId = uuid(9, 2);
  const data = new RosterData(expected, launchId, 9);
  data.failHouseFill = true;
  data.material = { ...data.material, houseAssignments: [] };
  const gateway = new RosterGateway();
  const coordinator = new HostedFormationCoordinator(
    data,
    gateway,
    HOST_ID,
    (digest) => digest === listing.digest ? reviewed : null,
  );

  assert.deepEqual(await coordinator.advance(ACCOUNT_ID, launchId), {
    state: "failed_pre_genesis",
    retryAfterSeconds: null,
    runId: null,
  });
  assert.equal(data.material.houseAssignments.length, 0);
  assert.equal(data.freezeCalls, 0);
  assert.equal(data.authorizeCalls, 0);
  assert.equal(data.genesisRecords, 0);
  assert.deepEqual(gateway.launches, []);
});

test("the current Agent Heist Listing keeps its existing House-fill contract", () => {
  const heist = readListingRevision(encodeCanonical(artifact("config/hosted/listings/agent-heist-0.24.0.json")));
  assert.equal(heist.value.launch_input_schema.schema, "worldstream/launch-input-schema/v1");
  assert.equal(heist.value.launch_input_schema.accepts, "none");
  assert.deepEqual(
    heist.value.seats.map((seat) => ({
      id: seat.seat_id,
      required: seat.required,
      houseFill: seat.allowed_participation.includes("house_agent_fill"),
    })),
    [
      { id: "navigator", required: true, houseFill: true },
      { id: "insider", required: true, houseFill: true },
      { id: "broker", required: false, houseFill: true },
    ],
  );
});
