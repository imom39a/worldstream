import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import {
  decodeCanonical,
  encodeCanonical,
  taggedBlake3,
  type CanonicalObject,
} from "@worldstream/pack-sdk";
import { readListingRevision } from "@worldstream/hosted-contract";
import { test, vi } from "vitest";

import {
  HttpHostedResultSourceClient,
  HttpHostedHouseRetirementClient,
  PinnedResultProjectorRegistry,
  ResultReconciliationRejectedError,
  ResultReconciliationUnavailableError,
  readHostedResultSourceEvidence,
  reconcileActivityResult,
  reconcileActivityResultCandidates,
  type HostedResultSourceClient,
  type HostedHouseRetirementClient,
  type HostedResultSourceEvidence,
  type ReconciliationWriteReceipt,
  type ResultReconciliationCandidate,
  type ResultReconciliationData,
  type ResultReconciliationState,
  type PrestartHouseRunnerRetirementCandidate,
  type TerminalReconciliationState,
  type TerminalHouseRunnerRetirementCandidate,
} from "./result-reconciliation.js";

const LISTING = "blake3:e3d401e783cec1ae4f911f682e8289054275dece60a0482b02f63e872f27dcc1";
const LAUNCH = `blake3:${"7".repeat(64)}`;
const RUN = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

function artifact(path: string): Uint8Array {
  return encodeCanonical(JSON.parse(
    readFileSync(new URL(`../../../${path}`, import.meta.url), "utf8"),
  ) as CanonicalObject);
}

function registry(): PinnedResultProjectorRegistry {
  return new PinnedResultProjectorRegistry([{
    listingBytes: artifact("config/hosted/listings/agent-heist-0.2.0.json"),
    projectorBytes: artifact("config/hosted/result-projectors/agent-heist-0.2.0.json"),
    runtimeBytes: artifact(
      "config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json",
    ),
    projectionSchemaBytes: artifact(
      "config/hosted/schemas/agent-heist-public-projection-v1.schema.json",
    ),
    outputSchemaBytes: artifact("config/hosted/schemas/result-summary-v1.schema.json"),
  }]);
}

function nonHeistContract() {
  const pack = {
    id: "worldstream.negotiate",
    version: "0.2.0",
    digest: `blake3:${"c".repeat(64)}`,
  } as const;
  const projectionSchema = encodeCanonical({
    schema: "worldstream.negotiate/public-projection/v1",
    type: "object",
    fields: ["status", "agreement", "participants"],
  });
  const outputSchema = encodeCanonical({
    schema: "worldstream.negotiate/result-summary/v1",
    fields: ["decision", "contract_id", "rounds"],
  });
  const projectorDocument = {
    schema: "worldstream/result-projector-revision/v1",
    projector_id: "worldstream.negotiate.result",
    version: "0.1.0",
    runtime: {
      id: "worldstream.result-projector.declarative",
      version: "1.0.0",
      digest: "blake3:7278757ac8f047330292e399ac3c8f4ed49225b05ecb6b997b75500784b0526e",
    },
    input: {
      pack,
      listing_schema: "worldstream/activity-listing-revision/v1",
      complete_head_schema: "worldstream/complete-head/v1",
      projection: {
        schema: "worldstream.negotiate/public-projection/v1",
        digest: taggedBlake3(projectionSchema),
      },
    },
    program: {
      schema: "worldstream/result-projector-program/v1",
      terminal: { field: "status", equals: "complete" },
      outcome_field: "agreement",
      summary_fields: [
        { output: "decision", source: "decision", kind: "enum", values: ["accepted", "rejected"] },
        { output: "contract_id", source: "contract_id", kind: "nullable_identifier", maximum_bytes: 128 },
        { output: "rounds", source: "rounds", kind: "integer", minimum: 0, maximum: 100 },
      ],
    },
    output: {
      schema: "worldstream.negotiate/result-summary/v1",
      schema_digest: taggedBlake3(outputSchema),
      canonicalizer: "worldstream/canonical-json/v1",
      maximum_bytes: 4096,
    },
    maximum_input_bytes: 262_144,
  } as const;
  const projectorBytes = encodeCanonical(projectorDocument);
  const listingBytes = encodeCanonical({
    schema: "worldstream/activity-listing-revision/v1",
    listing_id: "worldstream.negotiate.preview",
    version: "0.2.0",
    title: "Negotiate",
    description: "A reviewed non-Heist contract fixture.",
    catalog: { visibility: "public", review_status: "reviewed" },
    pack,
    client: {
      client_id: "worldstream.negotiate.web",
      release_digest: `sha256:${"1".repeat(64)}`,
      client_contract: "worldstream/activity-client-protocol/v1",
      surface_id: "negotiate-hosted-web",
    },
    launch_input_schema: {
      schema: "worldstream/launch-input-schema/v1",
      accepts: "none",
      defaults: {},
    },
    room_setup: { configuration: { agreement_type: "supply" } },
    seats: [
      { seat_id: "buyer", role: "buyer_agent", display_name: "Buyer Agent", required: true, allowed_participation: ["account_human", "account_external_agent"], allowed_house_agent_revisions: [] },
      { seat_id: "seller", role: "seller_agent", display_name: "Seller Agent", required: true, allowed_participation: ["account_human", "account_external_agent"], allowed_house_agent_revisions: [] },
      { seat_id: "approver", role: "buyer_approver", display_name: "Buyer Approver", required: true, allowed_participation: ["account_human", "account_external_agent"], allowed_house_agent_revisions: [] },
      { seat_id: "venue", role: "venue_signer", display_name: "Venue Signer", required: true, allowed_participation: ["account_human", "account_external_agent"], allowed_house_agent_revisions: [] },
    ],
    creator_access: "must_claim_seat",
    public_viewing_policy: "anonymous_by_link",
    pre_start_deadline_seconds: 300,
    result: {
      projection: { schema: "worldstream.negotiate/public-projection/v1", digest: taggedBlake3(projectionSchema) },
      projector: { id: "worldstream.negotiate.result", version: "0.1.0", digest: taggedBlake3(projectorBytes) },
      publication: {
        policy: "public_recent_results",
        attribution: "reviewed_pseudonymous_seats",
        public_output: "projector_summary_only",
        suppression: "unhealthy_inconclusive_or_conflict",
      },
    },
  });
  return {
    listing: readListingRevision(listingBytes),
    listingBytes,
    projectorBytes,
    runtimeBytes: artifact("config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json"),
    projectionSchemaBytes: projectionSchema,
    outputSchemaBytes: outputSchema,
    pack,
  } as const;
}

function nonHeistRegistry(): ReturnType<typeof registry> {
  const fixture = nonHeistContract();
  return new PinnedResultProjectorRegistry([fixture]);
}

function nonHeistEvidence(): HostedResultSourceEvidence {
  const fixture = nonHeistContract();
  const sourceHead = {
    room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    room_seq: 8,
    genesis_or_transition_hash: `blake3:${"9".repeat(64)}`,
    core_schema_version: "worldstream.core-room-state.v1",
    pack_digest: fixture.pack.digest,
    core_state_hash: `blake3:${"a".repeat(64)}`,
    activity_state_hash: `blake3:${"b".repeat(64)}`,
    authoritative_state_hash: `blake3:${"d".repeat(64)}`,
  } as const;
  const projection = {
    status: "complete",
    participants: [
      { role: "buyer_agent", present: true },
      { role: "seller_agent", present: true },
      { role: "buyer_approver", present: true },
      { role: "venue_signer", present: true },
    ],
    agreement: { decision: "accepted", contract_id: "contract-42", rounds: 3 },
  } satisfies CanonicalObject;
  const publicProjection = {
    projection_schema: "worldstream.negotiate/public-projection/v1",
    authorized_core: { access_mode: "spectator" },
    projection,
    action_offers: [],
  } satisfies CanonicalObject;
  const hash = projectionHash(publicProjection);
  return {
    schema: "worldstream/hosted-result-source-evidence/v1",
    host_installation_id: "hosted-test",
    launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    run_id: RUN,
    listing_revision_digest: fixture.listing.digest,
    launch_request_digest: LAUNCH,
    room_setup_operation_id: "hosted-result-01",
    room_id: sourceHead.room_id,
    pack: fixture.pack,
    result_indexer_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1",
    access_mode: "spectator",
    source_head: sourceHead,
    integrity_status: "healthy",
    integrity_generation: 7,
    projection_schema: publicProjection.projection_schema as string,
    public_projection: publicProjection,
    projection_hash: hash,
    replay: {
      verifier_revision: "worldstream.authorized-replay/v1",
      verified_head: sourceHead,
      projection_hash: hash,
      verification_receipt_digest: `sha256:${"6".repeat(64)}`,
    },
  };
}

function candidate(
  listingRevisionDigest = LISTING,
): ResultReconciliationCandidate {
  return {
    candidateKind: "result_source",
    launchRequestId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    runId: RUN,
    listingRevisionDigest,
    launchRequestDigest: LAUNCH,
    hostInstallationId: "hosted-test",
    roomSetupOperationId: "hosted-result-01",
  };
}

function fixture(name: string): CanonicalObject {
  return JSON.parse(
    readFileSync(
      new URL(`../../../fixtures/hosted-contract/valid/${name}`, import.meta.url),
      "utf8",
    ),
  ) as CanonicalObject;
}

function projectionHash(projection: CanonicalObject): string {
  return taggedBlake3(encodeCanonical({
    domain: "worldstream/projection-hash/v1",
    projection_schema: "worldstream.projection.v1",
    projection,
  }));
}

function sourceEvidence(name: string): HostedResultSourceEvidence {
  const input = fixture(name);
  const sourceHead = input.source_head as CanonicalObject;
  const pack = input.pack as CanonicalObject;
  const projected = input.public_projection;
  if (projected === undefined) throw new Error("fixture projection is required");
  const publicProjection = {
    projection_schema: String(input.projection_schema),
    authorized_core: { access_mode: "spectator" },
    projection: projected,
    action_offers: [],
  } satisfies CanonicalObject;
  const hash = projectionHash(publicProjection);
  return {
    schema: "worldstream/hosted-result-source-evidence/v1",
    host_installation_id: "hosted-test",
    launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    run_id: RUN,
    listing_revision_digest: String(input.listing_revision_digest),
    launch_request_digest: LAUNCH,
    room_setup_operation_id: "hosted-result-01",
    room_id: String(sourceHead.room_id),
    pack: {
      id: String(pack.id),
      version: String(pack.version),
      digest: String(pack.digest),
    },
    result_indexer_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1",
    access_mode: "spectator",
    source_head: sourceHead as unknown as HostedResultSourceEvidence["source_head"],
    integrity_status: "healthy",
    integrity_generation: 7,
    projection_schema: String(input.projection_schema),
    public_projection: publicProjection,
    projection_hash: hash,
    replay: {
      verifier_revision: "worldstream.authorized-replay/v1",
      verified_head: sourceHead as unknown as HostedResultSourceEvidence["source_head"],
      projection_hash: hash,
      verification_receipt_digest: `sha256:${"6".repeat(64)}`,
    },
  };
}

class MutableSource implements HostedResultSourceClient {
  constructor(public evidence: unknown) {}

  async readResultSource(): Promise<unknown> {
    return structuredClone(this.evidence);
  }
}

class MemoryData implements ResultReconciliationData {
  terminalCapacityReconciliationCalls = 0;
  terminalCapacityReleased = 0;

  async reconcileTerminalActivityCapacity(): Promise<number> {
    this.terminalCapacityReconciliationCalls += 1;
    return this.terminalCapacityReleased;
  }

  async markAttempt(_launchRequestId: string): Promise<void> {}
  terminal: TerminalReconciliationState = {
    terminalRecorded: false,
    projectorStatus: null,
    reconciliationState: "pending",
  };
  result: ResultReconciliationState = {
    resultRecorded: false,
    resultPayloadDigest: null,
    integrityStatus: null,
    integrityGeneration: null,
    publishable: false,
  };
  readonly writes: Array<{ kind: string; document: CanonicalObject }> = [];
  resultPayload: CanonicalObject | null = null;
  candidates: readonly ResultReconciliationCandidate[] = [];
  terminalHouseRetirementRuns: readonly string[] = [];
  terminalHouseRetirements: readonly TerminalHouseRunnerRetirementCandidate[] = [];
  prestartHouseRetirementRuns: readonly string[] = [];
  prestartHouseRetirements: readonly PrestartHouseRunnerRetirementCandidate[] = [];
  readonly retirementWrites: Array<{
    runId: string;
    candidate: TerminalHouseRunnerRetirementCandidate;
    receipt: CanonicalObject;
  }> = [];

  async listCandidates(): Promise<readonly ResultReconciliationCandidate[]> {
    return this.candidates;
  }

  async listTerminalHouseRunnerRetirementRuns(): Promise<readonly string[]> {
    return structuredClone(this.terminalHouseRetirementRuns);
  }

  async listPrestartHouseRunnerRetirementRuns(): Promise<readonly string[]> {
    return structuredClone(this.prestartHouseRetirementRuns);
  }

  async listPrestartAbandonmentLaunches(): Promise<readonly string[]> {
    return [];
  }

  async readTerminal(): Promise<TerminalReconciliationState> {
    return { ...this.terminal };
  }

  async readResult(): Promise<ResultReconciliationState> {
    return { ...this.result };
  }

  async readTerminalHouseRunnerRetirements(): Promise<readonly TerminalHouseRunnerRetirementCandidate[]> {
    return structuredClone(this.terminalHouseRetirements);
  }

  async readPrestartHouseRunnerRetirements(): Promise<readonly PrestartHouseRunnerRetirementCandidate[]> {
    return structuredClone(this.prestartHouseRetirements);
  }

  async recordTerminalHouseRunnerRetirement(input: {
    runId: string;
    candidate: TerminalHouseRunnerRetirementCandidate;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }): Promise<boolean> {
    assert.deepEqual(
      input.receiptDigest,
      createHash("sha256").update(input.canonicalReceipt).digest(),
    );
    this.retirementWrites.push({
      runId: input.runId,
      candidate: input.candidate,
      receipt: decodeCanonical<CanonicalObject>(input.canonicalReceipt),
    });
    this.terminalHouseRetirements = this.terminalHouseRetirements.filter(
      (candidate) => candidate.houseAgentAssignmentId !== input.candidate.houseAgentAssignmentId,
    );
    return true;
  }

  async recordPrestartHouseRunnerRetirement(input: {
    runId: string;
    candidate: PrestartHouseRunnerRetirementCandidate;
    canonicalReceipt: Uint8Array;
    receiptDigest: Uint8Array;
  }): Promise<boolean> {
    assert.deepEqual(
      input.receiptDigest,
      createHash("sha256").update(input.canonicalReceipt).digest(),
    );
    this.prestartHouseRetirements = this.prestartHouseRetirements.filter(
      (candidate) => candidate.houseAgentAssignmentId !== input.candidate.houseAgentAssignmentId,
    );
    return true;
  }

  async recordTerminal(
    _runId: string,
    evidence: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    const document = decodeCanonical<CanonicalObject>(evidence);
    this.writes.push({ kind: "terminal", document });
    const status = String(document.projector_status) as "terminal_without_outcome" | "summary";
    if (this.terminal.terminalRecorded && this.terminal.projectorStatus !== status) {
      return { disposition: "conflict", safeCode: "terminal_evidence_conflict" };
    }
    const duplicate = this.terminal.terminalRecorded;
    this.terminal = {
      terminalRecorded: true,
      projectorStatus: status,
      reconciliationState: "terminal",
    };
    return {
      disposition: duplicate ? "duplicate" : "applied",
      safeCode: duplicate ? "terminal_already_recorded" : "terminal_recorded",
    };
  }

  async recordTerminalConflict(
    _runId: string,
    evidence: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    this.writes.push({
      kind: "terminal_conflict",
      document: decodeCanonical<CanonicalObject>(evidence),
    });
    return { disposition: "conflict", safeCode: "nonterminal_reversion" };
  }

  async recordResult(
    _runId: string,
    evidence: Uint8Array,
    _evidenceDigest: Uint8Array,
    payload: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    this.resultPayload = decodeCanonical<CanonicalObject>(payload);
    this.writes.push({
      kind: "result",
      document: decodeCanonical<CanonicalObject>(evidence),
    });
    const digest = `sha256:${createHash("sha256").update(payload).digest("hex")}`;
    if (this.result.resultRecorded && this.result.resultPayloadDigest !== digest) {
      return { disposition: "conflict", safeCode: "result_summary_conflict" };
    }
    const duplicate = this.result.resultRecorded;
    this.result = {
      resultRecorded: true,
      resultPayloadDigest: digest,
      integrityStatus: "healthy",
      integrityGeneration: 7,
      publishable: true,
    };
    return {
      disposition: duplicate ? "duplicate" : "applied",
      safeCode: duplicate ? "result_already_recorded" : "result_recorded",
    };
  }

  async recordIntegrity(
    _runId: string,
    evidence: Uint8Array,
  ): Promise<ReconciliationWriteReceipt> {
    const document = decodeCanonical<CanonicalObject>(evidence);
    this.writes.push({ kind: "integrity", document });
    const status = String(document.integrity_status) as HostedResultSourceEvidence["integrity_status"];
    const generation = Number(document.integrity_generation);
    const duplicate = this.result.integrityGeneration === generation
      && this.result.integrityStatus === status;
    this.result = {
      ...this.result,
      integrityStatus: status,
      integrityGeneration: generation,
      publishable: this.result.resultRecorded && status === "healthy",
    };
    if (duplicate) {
      return { disposition: "duplicate", safeCode: "integrity_already_recorded" };
    }
    if (status !== "healthy" && this.result.resultRecorded) {
      return { disposition: "applied", safeCode: "result_suppressed" };
    }
    if (status === "healthy" && this.result.resultRecorded) {
      return { disposition: "applied", safeCode: "result_reverified" };
    }
    return { disposition: "applied", safeCode: "integrity_recorded" };
  }
}

class MemoryHouseRetirement implements HostedHouseRetirementClient {
  readonly requests: Array<Parameters<HostedHouseRetirementClient["retireHouseRunner"]>[0]> = [];

  async retireHouseRunner(request: Parameters<HostedHouseRetirementClient["retireHouseRunner"]>[0]) {
    this.requests.push(structuredClone(request));
    return {
      schema: "worldstream/house-runner-retirement-receipt/v1",
      host_installation_id: request.host_installation_id,
      reservation_operation_id: request.reservation_operation_id,
      launch_request_id: request.launch_request_id,
      house_agent_assignment_id: request.house_agent_assignment_id,
      runner_unit_id: "house-test-01",
      disposition: request.disposition,
      platform_evidence_digest: request.platform_evidence_digest,
      stop_witness: `blake3:${"8".repeat(64)}`,
      authentication_tag: "9".repeat(64),
    } as CanonicalObject;
  }
}

class UnavailableHouseRetirement implements HostedHouseRetirementClient {
  calls = 0;

  async retireHouseRunner(): Promise<CanonicalObject> {
    this.calls += 1;
    throw new ResultReconciliationUnavailableError("fixture_house_retirement_outage");
  }
}

class RejectedHouseRetirement implements HostedHouseRetirementClient {
  async retireHouseRunner(): Promise<CanonicalObject> {
    throw new ResultReconciliationRejectedError("fixture_house_retirement_rejected");
  }
}

function dependencies(source: MutableSource, data = new MemoryData()) {
  return { source, data, houseRetirement: new MemoryHouseRetirement(), projectors: registry() };
}

test("nonterminal Projection creates no terminal or result row", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-nonterminal-input.json"));
  const data = new MemoryData();
  const report = await reconcileActivityResult(candidate(), dependencies(source, data));
  assert.equal(report.outcome, "not_terminal");
  assert.deepEqual(data.writes, []);
});

test("terminal without outcome releases terminal flow without inventing a result", async () => {
  const source = new MutableSource(
    sourceEvidence("agent-heist-terminal-without-outcome-input.json"),
  );
  const data = new MemoryData();
  const report = await reconcileActivityResult(candidate(), dependencies(source, data));
  assert.equal(report.outcome, "terminal_without_outcome");
  assert.equal(data.writes.filter(({ kind }) => kind === "terminal").length, 1);
  assert.equal(data.writes.filter(({ kind }) => kind === "result").length, 0);
});

test("healthy terminal summary is projected and published from Replay-equal evidence", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  const report = await reconcileActivityResult(candidate(), dependencies(source, data));
  assert.equal(report.outcome, "result_recorded");
  assert.equal(data.result.resultRecorded, true);
  const result = data.writes.find(({ kind }) => kind === "result")?.document;
  assert.equal(result?.replay_projection_hash, result?.projection_hash);
  assert.equal(result?.integrity_status, "healthy");
});

test("shared reconciliation projects a reviewed non-Heist result with a different shape", async () => {
  const source = new MutableSource(nonHeistEvidence());
  const data = new MemoryData();
  const report = await reconcileActivityResult(
    candidate(nonHeistContract().listing.digest),
    { source, data, houseRetirement: new MemoryHouseRetirement(), projectors: nonHeistRegistry() },
  );
  assert.equal(report.outcome, "result_recorded");
  assert.deepEqual(data.resultPayload, {
    schema: "worldstream.negotiate/result-summary/v1",
    decision: "accepted",
    contract_id: "contract-42",
    rounds: 3,
  });
});

test("disabled publication records terminal evidence without indexing an Outcome", async () => {
  const fixture = nonHeistContract();
  const listingBytes = encodeCanonical({ ...fixture.listing.value, result: {
    ...fixture.listing.value.result,
    publication: { policy: "disabled", attribution: "none", public_output: "none", suppression: "unhealthy_inconclusive_or_conflict" },
  }} as unknown as CanonicalObject);
  const listing = readListingRevision(listingBytes);
  const evidence = { ...nonHeistEvidence(), listing_revision_digest: listing.digest };
  const { replay: _replay, ...withoutReplay } = evidence;
  for (const selected of [evidence, withoutReplay]) {
    const data = new MemoryData();
    const report = await reconcileActivityResult(candidate(listing.digest), {
      source: new MutableSource(selected), data, houseRetirement: new MemoryHouseRetirement(),
      projectors: new PinnedResultProjectorRegistry([{ ...fixture, listingBytes }]),
    });
    assert.equal(report.outcome, "terminal_private");
    assert.equal(data.terminal.terminalRecorded, true);
    assert.equal(data.writes.some(({ kind }) => kind === "result"), false);
    assert.equal(data.resultPayload, null);
  }
});

test("only retained terminal evidence can trigger exact House retirement", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  data.terminalHouseRetirements = [{
    hostInstallationId: "hosted-test",
    reservationOperationId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launchRequestId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    houseAgentAssignmentId: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    terminalEvidenceDigest: `sha256:${"e".repeat(64)}`,
  }];
  const deps = dependencies(source, data);
  await reconcileActivityResult(candidate(), deps);
  assert.deepEqual(deps.houseRetirement.requests, [{
    schema: "worldstream/house-runner-retirement-request/v1",
    host_installation_id: "hosted-test",
    reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    disposition: "run_terminal",
    platform_evidence_digest: `sha256:${"e".repeat(64)}`,
  }]);
  assert.equal(data.retirementWrites.length, 1);
});

test("a Host retirement outage does not delay the durable result and retries automatically", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  data.terminalHouseRetirements = [{
    hostInstallationId: "hosted-test",
    reservationOperationId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launchRequestId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    houseAgentAssignmentId: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    terminalEvidenceDigest: `sha256:${"e".repeat(64)}`,
  }];
  data.terminalHouseRetirementRuns = [RUN];
  const unavailable = new UnavailableHouseRetirement();
  const deps = { source, data, houseRetirement: unavailable, projectors: registry() };

  const report = await reconcileActivityResult(candidate(), deps);
  assert.equal(report.outcome, "result_recorded");
  assert.equal(data.result.resultRecorded, true);
  assert.equal(data.retirementWrites.length, 0);

  await reconcileActivityResultCandidates(deps, 1);
  assert.equal(unavailable.calls, 2);
  assert.equal(data.retirementWrites.length, 0);

  const recovered = new MemoryHouseRetirement();
  await reconcileActivityResultCandidates({ ...deps, houseRetirement: recovered }, 1);
  assert.equal(recovered.requests.length, 1);
  assert.equal(data.retirementWrites.length, 1);
});

test("result-free pre-start abandonment retries the same exact House retirement", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-nonterminal-input.json"));
  const data = new MemoryData();
  data.prestartHouseRetirementRuns = [RUN];
  data.prestartHouseRetirements = [{
    hostInstallationId: "hosted-test",
    reservationOperationId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launchRequestId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    houseAgentAssignmentId: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    abandonmentEvidenceDigest: `sha256:${"a".repeat(64)}`,
  }];
  const unavailable = new UnavailableHouseRetirement();
  const deps = { source, data, houseRetirement: unavailable, projectors: registry() };

  await reconcileActivityResultCandidates(deps, 1);
  assert.equal(unavailable.calls, 1);
  assert.equal(data.prestartHouseRetirements.length, 1);

  const recovered = new MemoryHouseRetirement();
  await reconcileActivityResultCandidates({ ...deps, houseRetirement: recovered }, 1);
  assert.deepEqual(recovered.requests, [{
    schema: "worldstream/house-runner-retirement-request/v1",
    host_installation_id: "hosted-test",
    reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    disposition: "pre_start_abandoned",
    platform_evidence_digest: `sha256:${"a".repeat(64)}`,
  }]);
  assert.equal(data.prestartHouseRetirements.length, 0);
});

test("a rejected House retirement proof remains closed and observable", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  data.terminalHouseRetirements = [{
    hostInstallationId: "hosted-test",
    reservationOperationId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launchRequestId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    houseAgentAssignmentId: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    terminalEvidenceDigest: `sha256:${"e".repeat(64)}`,
  }];
  const deps = {
    source,
    data,
    houseRetirement: new RejectedHouseRetirement(),
    projectors: registry(),
  };

  await assert.rejects(() => reconcileActivityResult(candidate(), deps), ResultReconciliationRejectedError);
  assert.equal(data.result.resultRecorded, true);
  assert.equal(data.retirementWrites.length, 0);
});

test("duplicate and later confirming Heads do not create a second result", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  const deps = dependencies(source, data);
  await reconcileActivityResult(candidate(), deps);
  const duplicate = await reconcileActivityResult(candidate(), deps);
  assert.equal(duplicate.outcome, "result_confirmed");

  const later = structuredClone(source.evidence) as HostedResultSourceEvidence;
  const nextHead = { ...later.source_head, room_seq: later.source_head.room_seq + 1 };
  source.evidence = {
    ...later,
    source_head: nextHead,
    integrity_generation: later.integrity_generation + 1,
    replay: { ...later.replay!, verified_head: nextHead },
  };
  const confirmed = await reconcileActivityResult(candidate(), deps);
  assert.equal(confirmed.outcome, "result_reverified");
  assert.equal(data.writes.filter(({ kind }) => kind === "result").length, 1);
});

test("divergent later summary records conflict without replacing the first", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  const deps = dependencies(source, data);
  await reconcileActivityResult(candidate(), deps);
  const changed = structuredClone(source.evidence) as HostedResultSourceEvidence;
  const projection = structuredClone(changed.public_projection) as {
    projection_schema: string;
    authorized_core: CanonicalObject;
    projection: CanonicalObject;
    action_offers: CanonicalObject[];
  };
  const outcome = projection.projection.outcome as Record<string, unknown>;
  outcome.score = 4;
  const hash = projectionHash(projection as unknown as CanonicalObject);
  source.evidence = {
    ...changed,
    public_projection: projection,
    projection_hash: hash,
    replay: { ...changed.replay!, projection_hash: hash },
  };
  const report = await reconcileActivityResult(candidate(), deps);
  assert.equal(report.outcome, "conflict");
  assert.equal(report.safeCode, "result_summary_conflict");
  assert.equal(data.writes.filter(({ kind }) => kind === "result").length, 2);
});

test("later nonterminal state after terminal is a permanent conflict", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  const deps = dependencies(source, data);
  await reconcileActivityResult(candidate(), deps);
  source.evidence = sourceEvidence("agent-heist-nonterminal-input.json");
  const report = await reconcileActivityResult(candidate(), deps);
  assert.equal(report.outcome, "conflict");
  assert.equal(data.writes.at(-1)?.kind, "terminal_conflict");
});

test("unhealthy evidence suppresses an existing result and never republishes it", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-terminal-input.json"));
  const data = new MemoryData();
  const deps = dependencies(source, data);
  await reconcileActivityResult(candidate(), deps);
  const unhealthy = structuredClone(source.evidence) as HostedResultSourceEvidence;
  const { replay: _ignoredReplay, ...withoutReplay } = unhealthy;
  source.evidence = {
    ...withoutReplay,
    integrity_status: "faulted",
    integrity_generation: unhealthy.integrity_generation + 1,
  };
  const report = await reconcileActivityResult(candidate(), deps);
  assert.equal(report.outcome, "result_suppressed");
  assert.equal(data.result.publishable, false);
  assert.equal(data.writes.filter(({ kind }) => kind === "result").length, 1);
});

test("missing Replay blocks a new summary result", async () => {
  const evidence = sourceEvidence("agent-heist-terminal-input.json");
  const { replay: _ignoredReplay, ...withoutReplay } = evidence;
  const source = new MutableSource(withoutReplay);
  const data = new MemoryData();
  const report = await reconcileActivityResult(candidate(), dependencies(source, data));
  assert.equal(report.outcome, "blocked");
  assert.equal(data.writes.filter(({ kind }) => kind === "result").length, 0);
});

test("wrong hash, widened schema, and oversized evidence fail before persistence", async () => {
  const valid = sourceEvidence("agent-heist-terminal-input.json");
  const invalidValues: unknown[] = [
    { ...valid, projection_hash: `blake3:${"0".repeat(64)}` },
    { ...valid, unexpected_private_field: true },
    {
      ...valid,
      public_projection: {
        ...valid.public_projection,
        projection: { padding: "x".repeat(263_000) },
      },
    },
  ];
  for (const value of invalidValues) {
    const data = new MemoryData();
    await assert.rejects(
      reconcileActivityResult(
        candidate(),
        dependencies(new MutableSource(value), data),
      ),
      ResultReconciliationRejectedError,
    );
    assert.deepEqual(data.writes, []);
  }
});

test("host and launch identity mismatches fail before persistence", async () => {
  const valid = sourceEvidence("agent-heist-terminal-input.json");
  for (const value of [
    { ...valid, host_installation_id: "different-host" },
    { ...valid, launch_request_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc" },
  ]) {
    const data = new MemoryData();
    await assert.rejects(
      reconcileActivityResult(candidate(), dependencies(new MutableSource(value), data)),
      ResultReconciliationRejectedError,
    );
    assert.deepEqual(data.writes, []);
  }
});

test("missing or malformed pinned projector artifacts fail closed", async () => {
  const unknownListing = `blake3:${"0".repeat(64)}`;
  const unknownEvidence = {
    ...sourceEvidence("agent-heist-terminal-input.json"),
    listing_revision_digest: unknownListing,
  };
  await assert.rejects(
    reconcileActivityResult(
      candidate(unknownListing),
      dependencies(new MutableSource(unknownEvidence)),
    ),
    ResultReconciliationUnavailableError,
  );
  assert.throws(
    () => new PinnedResultProjectorRegistry([{
      listingBytes: new TextEncoder().encode("{}"),
      projectorBytes: new TextEncoder().encode("{}"),
      runtimeBytes: new TextEncoder().encode("{}"),
      projectionSchemaBytes: new TextEncoder().encode("{}"),
      outputSchemaBytes: new TextEncoder().encode("{}"),
    }]),
    ResultReconciliationRejectedError,
  );
});

test("candidate passes are hard-bounded and ignore the separate Genesis lane", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-nonterminal-input.json"));
  const data = new MemoryData();
  data.candidates = [{
    ...candidate(),
    candidateKind: "genesis",
    runId: null,
    launchRequestDigest: null,
  }];
  assert.deepEqual(
    await reconcileActivityResultCandidates(dependencies(source, data), 1),
    [],
  );
  assert.equal(data.terminalCapacityReconciliationCalls, 1);
  await assert.rejects(
    reconcileActivityResultCandidates(dependencies(source, data), 101),
    ResultReconciliationRejectedError,
  );
});

test("candidate reconciliation rejects an unbounded terminal-capacity repair result", async () => {
  const source = new MutableSource(sourceEvidence("agent-heist-nonterminal-input.json"));
  const data = new MemoryData();
  data.terminalCapacityReleased = 2;
  await assert.rejects(
    reconcileActivityResultCandidates(dependencies(source, data), 1),
    ResultReconciliationRejectedError,
  );
});

test("default result request outlasts the two-read Gateway budget", async () => {
  const timeout = vi.spyOn(AbortSignal, "timeout").mockReturnValue(new AbortController().signal);
  try {
    const client = new HttpHostedResultSourceClient({
      baseUrl: "https://host.example",
      serviceAuthority: "synthetic-service-authority-at-least-32-bytes",
      fetchImplementation: async () => Response.json(sourceEvidence("agent-heist-terminal-input.json")),
    });
    await client.readResultSource({schema: "worldstream/hosted-result-source-request/v1", run_id: RUN,
      listing_revision_digest: LISTING, launch_request_digest: LAUNCH, room_setup_operation_id: "hosted-result-01"});
    assert.deepEqual(timeout.mock.calls, [[30_000]]);
  } finally { timeout.mockRestore(); }
});

test("HTTP result source uses only the fixed service route and bounds responses", async () => {
  let observedUrl = "";
  let observedAuthorization = "";
  const evidence = sourceEvidence("agent-heist-terminal-input.json");
  const client = new HttpHostedResultSourceClient({
    baseUrl: "https://host.example/internal/ignored",
    serviceAuthority: "service-authority-value-with-at-least-32-bytes",
    fetchImplementation: async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      observedUrl = request.url;
      observedAuthorization = request.headers.get("authorization") ?? "";
      return Response.json(evidence);
    },
  });
  assert.deepEqual(
    await client.readResultSource({
      schema: "worldstream/hosted-result-source-request/v1",
      run_id: RUN,
      listing_revision_digest: LISTING,
      launch_request_digest: LAUNCH,
      room_setup_operation_id: "hosted-result-01",
    }),
    evidence,
  );
  assert.equal(observedUrl, "https://host.example/v1/hosted/result-source-evidence");
  assert.equal(
    observedAuthorization,
    "Bearer service-authority-value-with-at-least-32-bytes",
  );

  const oversized = new HttpHostedResultSourceClient({
    baseUrl: "https://host.example",
    serviceAuthority: "service-authority-value-with-at-least-32-bytes",
    fetchImplementation: async () => new Response("x", {
      status: 200,
      headers: { "content-length": String(385 * 1024) },
    }),
  });
  await assert.rejects(
    oversized.readResultSource({
      schema: "worldstream/hosted-result-source-request/v1",
      run_id: RUN,
      listing_revision_digest: LISTING,
      launch_request_digest: LAUNCH,
      room_setup_operation_id: "hosted-result-01",
    }),
    ResultReconciliationRejectedError,
  );
});

test("House retirement uses only its fixed service route", async () => {
  let observedUrl = "";
  const client = new HttpHostedHouseRetirementClient({
    baseUrl: "https://host.example/internal/ignored",
    serviceAuthority: "service-authority-value-with-at-least-32-bytes",
    fetchImplementation: async (input, init) => {
      const request = input instanceof Request ? input : new Request(input, init);
      observedUrl = request.url;
      return Response.json({
        schema: "worldstream/house-runner-retirement-receipt/v1",
        host_installation_id: "hosted-test",
        reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        runner_unit_id: "house-test-01",
        disposition: "run_terminal",
        platform_evidence_digest: `sha256:${"e".repeat(64)}`,
        stop_witness: `blake3:${"8".repeat(64)}`,
        authentication_tag: "9".repeat(64),
      });
    },
  });
  await client.retireHouseRunner({
    schema: "worldstream/house-runner-retirement-request/v1",
    host_installation_id: "hosted-test",
    reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    disposition: "run_terminal",
    platform_evidence_digest: `sha256:${"e".repeat(64)}`,
  });
  assert.equal(observedUrl, "https://host.example/v1/hosted/house-runners/retire");
});

test("House retirement retries only transport and 5xx failures", async () => {
  const request = {
    schema: "worldstream/house-runner-retirement-request/v1" as const,
    host_installation_id: "hosted-test",
    reservation_operation_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
    launch_request_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    house_agent_assignment_id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
    disposition: "run_terminal" as const,
    platform_evidence_digest: `sha256:${"e".repeat(64)}`,
  };
  for (const status of [400, 401, 403, 409, 422, 429]) {
    const client = new HttpHostedHouseRetirementClient({
      baseUrl: "https://host.example",
      serviceAuthority: "service-authority-value-with-at-least-32-bytes",
      fetchImplementation: async () => new Response("rejected", { status }),
    });
    await assert.rejects(() => client.retireHouseRunner(request), ResultReconciliationRejectedError);
  }
  const missing = new HttpHostedHouseRetirementClient({
    baseUrl: "https://host.example",
    serviceAuthority: "service-authority-value-with-at-least-32-bytes",
    fetchImplementation: async () => new Response("missing", { status: 404 }),
  });
  await assert.rejects(
    () => missing.retireHouseRunner(request),
    ResultReconciliationUnavailableError,
  );
  const unavailable = new HttpHostedHouseRetirementClient({
    baseUrl: "https://host.example",
    serviceAuthority: "service-authority-value-with-at-least-32-bytes",
    fetchImplementation: async () => new Response("unavailable", { status: 503 }),
  });
  await assert.rejects(
    () => unavailable.retireHouseRunner(request),
    ResultReconciliationUnavailableError,
  );
});

test("the shared reconciler contains no Pack-specific field branch", () => {
  const source = readFileSync(new URL("./result-reconciliation.ts", import.meta.url), "utf8");
  for (const forbidden of ["agent-heist", ".phase", "[\"phase\"]", ".winner", ".score"]) {
    assert.equal(source.includes(forbidden), false, forbidden);
  }
});

test("the independent evidence reader accepts the frozen valid shape", () => {
  const evidence = sourceEvidence("agent-heist-terminal-input.json");
  assert.deepEqual(readHostedResultSourceEvidence(evidence), evidence);
});
