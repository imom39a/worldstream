import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import {
  decodeCanonical,
  encodeCanonical,
  taggedBlake3,
  type CanonicalObject,
} from "@worldstream/pack-sdk";
import { test } from "vitest";

import {
  HttpHostedResultSourceClient,
  PinnedResultProjectorRegistry,
  ResultReconciliationRejectedError,
  ResultReconciliationUnavailableError,
  readHostedResultSourceEvidence,
  reconcileActivityResult,
  reconcileActivityResultCandidates,
  type HostedResultSourceClient,
  type HostedResultSourceEvidence,
  type ReconciliationWriteReceipt,
  type ResultReconciliationCandidate,
  type ResultReconciliationData,
  type ResultReconciliationState,
  type TerminalReconciliationState,
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
  candidates: readonly ResultReconciliationCandidate[] = [];

  async listCandidates(): Promise<readonly ResultReconciliationCandidate[]> {
    return this.candidates;
  }

  async readTerminal(): Promise<TerminalReconciliationState> {
    return { ...this.terminal };
  }

  async readResult(): Promise<ResultReconciliationState> {
    return { ...this.result };
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

function dependencies(source: MutableSource, data = new MemoryData()) {
  return { source, data, projectors: registry() };
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
  await assert.rejects(
    reconcileActivityResultCandidates(dependencies(source, data), 101),
    ResultReconciliationRejectedError,
  );
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
