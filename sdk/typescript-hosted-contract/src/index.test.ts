import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { encodeCanonical, taggedBlake3, type CanonicalJson } from "@worldstream/pack-sdk";

import {
  deriveRoomSetup,
  projectResult,
  readHouseAgentRevision,
  readListingRevision,
  readResultProjectorRevision,
  resolveProjectorArtifacts,
  verifyListingClientRelease,
  verifyListingPack,
  verifyListingProjector,
  type ActivityClientReleaseIdentity,
  type ListingRevision,
  type ResolvedResultProjector,
  type ResultProjectorRevision,
} from "./index.js";

const repository = fileURLToPath(new URL("../../..", import.meta.url));

function document(path: string): CanonicalJson {
  return JSON.parse(readFileSync(`${repository}/${path}`, "utf8")) as CanonicalJson;
}

function canonical(path: string): Uint8Array { return encodeCanonical(document(path)); }

function mutable(path: string): Record<string, CanonicalJson> {
  return structuredClone(document(path)) as Record<string, CanonicalJson>;
}

function record(value: CanonicalJson | undefined): Record<string, CanonicalJson> {
  assert.ok(value !== null && value !== undefined && !Array.isArray(value) && typeof value === "object");
  return value as Record<string, CanonicalJson>;
}

function array(value: CanonicalJson | undefined): CanonicalJson[] {
  assert.ok(Array.isArray(value));
  return value as CanonicalJson[];
}

function contracts() {
  return {
    listing: readListingRevision(canonical("config/hosted/listings/agent-heist-0.2.0.json")),
    projector: readResultProjectorRevision(canonical("config/hosted/result-projectors/agent-heist-0.2.0.json")),
  };
}

function resolveProjector(projector: ResultProjectorRevision): ResolvedResultProjector {
  return resolveProjectorArtifacts(
    projector,
    canonical("config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json"),
    canonical("config/hosted/schemas/agent-heist-public-projection-v1.schema.json"),
    canonical("config/hosted/schemas/result-summary-v1.schema.json"),
  );
}

test("resolves every exact pre-genesis identity", () => {
  const { listing, projector } = contracts();
  const client = document("config/activity-clients/releases/agent-heist-web.json") as unknown as ActivityClientReleaseIdentity;
  verifyListingPack(listing, listing.value.pack);
  verifyListingClientRelease(listing, client);
  verifyListingProjector(listing, projector);
  resolveProjector(projector);
  assert.throws(() => verifyListingPack(listing, { ...listing.value.pack, id: "worldstream.wrong" }), /reference_mismatch/);
  assert.throws(() => verifyListingClientRelease(listing, { ...client, client_id: "worldstream.wrong.web" }), /reference_mismatch/);
  assert.throws(() => verifyListingClientRelease(listing, { ...client, surfaces: [{ surface_id: "wrong-surface" }] }), /reference_mismatch/);
});

test("validates the exact two-revision House Agent pool", () => {
  const cooperative = readHouseAgentRevision(canonical(
    "config/hosted/house-agents/cooperative-planner-1.json",
  ));
  const skeptical = readHouseAgentRevision(canonical(
    "config/hosted/house-agents/skeptical-auditor-1.json",
  ));
  const listing = readListingRevision(canonical("config/hosted/listings/agent-heist-0.3.0.json"));
  assert.equal(
    cooperative.digest,
    "blake3:a664f616c754f03b484f40b930822411aba8579325731c48ee0cd72302805e81",
  );
  assert.equal(
    skeptical.digest,
    "blake3:05639c75dcf556f45429bc5e0fcca8a7b7bcb002ced56c3a28af9a211930bb0a",
  );
  assert.equal(
    listing.digest,
    "blake3:d3f2c55783a791542945c8a8946a58184b35866f6548539e753edc7349881956",
  );
  assert.notEqual(cooperative.value.route.model_slug, skeptical.value.route.model_slug);
  assert.equal(cooperative.value.allowance.model_call_attempts, 10);
});

test("House Agent revisions reject tools, fallback policy, and mutable allowance", () => {
  const source = mutable("config/hosted/house-agents/cooperative-planner-1.json");
  for (const mutation of ["tools", "route", "allowance"] as const) {
    const value = structuredClone(source);
    if (mutation === "tools") value.tools = ["browser"];
    if (mutation === "route") record(value.route).zero_data_retention = false;
    if (mutation === "allowance") record(value.allowance).model_call_attempts = 11;
    assert.throws(() => readHouseAgentRevision(encodeCanonical(value)), /invalid_shape/);
  }
});

test("artifact verification requires canonical duplicate-free bytes", () => {
  const { projector } = contracts();
  assert.throws(() => resolveProjectorArtifacts(
    projector,
    canonical("config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json"),
    new TextEncoder().encode('{"$id":1,"$id":2}'),
    canonical("config/hosted/schemas/result-summary-v1.schema.json"),
  ), /noncanonical/);
});

test("projection rejects a forged unverified handle", () => {
  const { listing, projector } = contracts();
  assert.throws(
    () => projectResult(
      listing,
      { revision: projector },
      canonical("fixtures/hosted-contract/valid/agent-heist-terminal-input.json"),
    ),
    /unsupported/,
  );
});

test("artifact resolution reconstructs the projector and rejects a forged revision identity", () => {
  const { listing, projector } = contracts();
  const forgedValue = structuredClone(projector.value) as unknown as Record<string, CanonicalJson>;
  record(record(forgedValue.program).terminal).equals = "never";
  const forged = {
    value: forgedValue as unknown as ResultProjectorRevision["value"],
    canonicalBytes: projector.canonicalBytes,
    digest: projector.digest,
  };
  assert.throws(() => verifyListingProjector(listing, forged), /unsupported/);
  const resolved = resolveProjector(forged);
  assert.equal(resolved.revision.value.program.terminal.equals, "complete");
  const output = projectResult(
    listing,
    resolved,
    canonical("fixtures/hosted-contract/valid/agent-heist-terminal-input.json"),
  );
  assert.equal((JSON.parse(new TextDecoder().decode(output)) as { status: string }).status, "summary");

  assert.throws(
    () => resolveProjector({ ...forged, digest: `blake3:${"0".repeat(64)}` }),
    /reference_mismatch/,
  );
});

test("runtime artifact binds the exact Rust and TypeScript interpreter sources", () => {
  const artifact = mutable("config/hosted/result-projector-runtimes/declarative-runtime-1.0.0.json");
  for (const value of array(artifact.implementations)) {
    const implementation = record(value);
    assert.equal(
      implementation.digest,
      taggedBlake3(readFileSync(`${repository}/${String(implementation.path)}`)),
    );
  }
});

test("validated revisions are immutable and defend canonical bytes", () => {
  const { listing } = contracts();
  assert.throws(() => { (listing.value as unknown as { title: string }).title = "mutated"; }, TypeError);
  const bytes = listing.canonicalBytes;
  bytes[0] = 0;
  assert.notEqual(listing.canonicalBytes[0], 0);
});

test("every listing consumer rejects a forged structural revision", () => {
  const { listing, projector } = contracts();
  const forgedValue = structuredClone(listing.value) as unknown as Record<string, CanonicalJson>;
  record(array(forgedValue.seats)[0]).role = "attacker";
  const forged = {
    value: forgedValue as unknown as ListingRevision["value"],
    canonicalBytes: listing.canonicalBytes,
    digest: listing.digest,
  };
  const client = document("config/activity-clients/releases/agent-heist-web.json") as unknown as ActivityClientReleaseIdentity;
  const resolved = resolveProjector(projector);
  const rejects = [
    () => verifyListingPack(forged, forged.value.pack),
    () => verifyListingClientRelease(forged, client),
    () => verifyListingProjector(forged, projector),
    () => deriveRoomSetup(
      forged,
      canonical("fixtures/hosted-contract/valid/agent-heist-launch-request.json"),
      canonical("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json"),
    ),
    () => projectResult(
      forged,
      resolved,
      canonical("fixtures/hosted-contract/valid/agent-heist-terminal-input.json"),
    ),
  ];
  for (const reject of rejects) assert.throws(reject, /unsupported/);
});

test("derives exact downstream setup without operator privilege", () => {
  const { listing } = contracts();
  const setup = deriveRoomSetup(
    listing,
    canonical("fixtures/hosted-contract/valid/agent-heist-launch-request.json"),
    canonical("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json"),
  );
  assert.deepEqual(setup, canonical("fixtures/hosted-contract/expected/agent-heist-room-setup.json"));
  const value = JSON.parse(new TextDecoder().decode(setup)) as {
    operator_view: boolean;
    schema: string;
    spectators: Array<{ purpose: string; role?: string; scopes?: string[] }>;
  };
  assert.equal(value.schema, "worldstream/room-setup/v2");
  assert.equal(value.operator_view, false);
  assert.deepEqual(value.spectators.map(({ purpose }) => purpose), ["result_indexer", "public_relay"]);
  assert.equal(value.spectators[0]?.role, undefined);
  assert.equal(value.spectators[0]?.scopes, undefined);
});

test("setup rejects duplicate or invalid public principals", () => {
  const { listing } = contracts();
  const launch = canonical("fixtures/hosted-contract/valid/agent-heist-launch-request.json");
  const roster = mutable("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
  const members = array(roster.members).map(record);
  members[1]!.principal_reference = members[0]!.principal_reference!;
  assert.throws(() => deriveRoomSetup(listing, launch, encodeCanonical(roster)), /invalid_shape/);
  members[1]!.principal_reference = "agent/invalid";
  assert.throws(() => deriveRoomSetup(listing, launch, encodeCanonical(roster)), /invalid_shape/);
  const reserved = mutable("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
  array(reserved.members).map(record)[0]!.principal_reference = "worldstream:result-indexer";
  assert.throws(() => deriveRoomSetup(listing, launch, encodeCanonical(reserved)), /invalid_shape/);
});

test("browser launch DTO cannot select server-owned contracts", () => {
  const { listing } = contracts();
  const roster = canonical("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
  for (const field of ["pack", "role", "seat", "client", "client_url", "projector", "room_setup"]) {
    const launch = mutable("fixtures/hosted-contract/valid/agent-heist-launch-request.json");
    launch[field] = "not-browser-owned";
    assert.throws(() => deriveRoomSetup(listing, encodeCanonical(launch), roster), /invalid_shape/);
  }
});

test("creator participation is frozen and mutually exclusive", () => {
  const { listing: mustClaimListing } = contracts();
  const rosterBytes = canonical("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
  const invalidSeatLaunch = mutable("fixtures/hosted-contract/valid/agent-heist-launch-request.json");
  record(invalidSeatLaunch.creator).principal_reference = "github:not-in-roster";
  assert.throws(
    () => deriveRoomSetup(mustClaimListing, encodeCanonical(invalidSeatLaunch), rosterBytes),
    /invalid_shape/,
  );

  const listingValue = mutable("config/hosted/listings/agent-heist-0.2.0.json");
  listingValue.creator_access = "may_spectate";
  const listing = readListingRevision(encodeCanonical(listingValue));
  const launch = mutable("fixtures/hosted-contract/valid/agent-heist-launch-request.json");
  launch.listing_revision_digest = listing.digest;
  const roster = mutable("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
  roster.listing_revision_digest = listing.digest;

  const seated = JSON.parse(new TextDecoder().decode(deriveRoomSetup(
    listing,
    encodeCanonical(launch),
    encodeCanonical(roster),
  ))) as { spectators: Array<{ purpose: string }> };
  assert.equal(seated.spectators.some(({ purpose }) => purpose === "creator"), false);

  launch.creator = {
    participation: "spectator",
    principal_reference: "worldstream:creator-spectator",
  };
  record(array(roster.members)[0]).principal_reference = "github:2002";
  const spectating = JSON.parse(new TextDecoder().decode(deriveRoomSetup(
    listing,
    encodeCanonical(launch),
    encodeCanonical(roster),
  ))) as { spectators: Array<{ purpose: string; principal: { reference: string; kind: string } }> };
  assert.ok(spectating.spectators.some(({ purpose, principal }) =>
    purpose === "creator"
    && principal.reference === "worldstream:creator-spectator"
    && principal.kind === "human"
  ));

  const forbiddenLaunch = mutable("fixtures/hosted-contract/valid/agent-heist-launch-request.json");
  forbiddenLaunch.creator = launch.creator;
  assert.throws(
    () => deriveRoomSetup(mustClaimListing, encodeCanonical(forbiddenLaunch), rosterBytes),
    /invalid_shape/,
  );
  record(array(roster.members)[0]).principal_reference = "worldstream:creator-spectator";
  assert.throws(
    () => deriveRoomSetup(listing, encodeCanonical(launch), encodeCanonical(roster)),
    /invalid_shape/,
  );
});

test("reviewed house fill uses the existing managed assignment shape", () => {
  const listing = readListingRevision(canonical("config/hosted/listings/agent-heist-0.3.0.json"));
  const houseAgent = readHouseAgentRevision(
    canonical("config/hosted/house-agents/cooperative-planner-1.json"),
  );
  const launch = mutable("fixtures/hosted-contract/valid/agent-heist-launch-request.json");
  launch.listing_revision_digest = listing.digest;
  const roster = mutable("fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
  roster.listing_revision_digest = listing.digest;
  array(roster.members)[1] = {
    seat_id: "insider",
    participation: "house_agent_fill",
    principal_reference: "house:insider-1",
    display_name: "Cooperative Planner",
    house_agent_revision_digest: houseAgent.digest,
    agent_profile: { profile_id: "house-cooperative-planner", revision: "1" },
    runner_template: { template_id: "openrouter-house", revision: "1" },
  };
  assert.throws(
    () => deriveRoomSetup(listing, encodeCanonical(launch), encodeCanonical(roster)),
    /reference_mismatch/,
  );
  const output = deriveRoomSetup(
    listing,
    encodeCanonical(launch),
    encodeCanonical(roster),
    [houseAgent],
  );
  const setup = JSON.parse(new TextDecoder().decode(output)) as { seats: Array<{ assignment?: { mode: string } }> };
  assert.equal(setup.seats[1]?.assignment?.mode, "managed");

  record(array(roster.members)[1]!).agent_profile = {
    profile_id: "house-cooperative-planner",
    revision: "2",
  };
  assert.throws(
    () => deriveRoomSetup(listing, encodeCanonical(launch), encodeCanonical(roster), [houseAgent]),
    /reference_mismatch/,
  );
});

test("projects exactly three bounded states including lobby", () => {
  const { listing, projector } = contracts();
  const resolved = resolveProjector(projector);
  const cases = [
    ["agent-heist-nonterminal-input.json", "not_terminal"],
    ["agent-heist-terminal-without-outcome-input.json", "terminal_without_outcome"],
    ["agent-heist-terminal-input.json", "summary"],
  ] as const;
  for (const [fixture, status] of cases) {
    const output = projectResult(listing, resolved, canonical(`fixtures/hosted-contract/valid/${fixture}`));
    assert.equal((JSON.parse(new TextDecoder().decode(output)) as { status: string }).status, status);
    assert.ok(output.byteLength <= projector.value.output.maximum_bytes);
    if (status === "summary") assert.deepEqual(output, canonical("fixtures/hosted-contract/expected/agent-heist-result-summary.json"));
  }
});

test("projector requires listing, projector, Pack, schema, and complete Head identity", () => {
  const { listing, projector } = contracts();
  const resolved = resolveProjector(projector);
  for (const mutation of ["listing", "projector", "pack", "head_pack", "projection"] as const) {
    const input = mutable("fixtures/hosted-contract/valid/agent-heist-terminal-input.json");
    if (mutation === "listing") input.listing_revision_digest = `blake3:${"0".repeat(64)}`;
    if (mutation === "projector") input.projector_revision_digest = `blake3:${"0".repeat(64)}`;
    if (mutation === "pack") record(input.pack).digest = `blake3:${"0".repeat(64)}`;
    if (mutation === "head_pack") record(input.source_head).pack_digest = `blake3:${"0".repeat(64)}`;
    if (mutation === "projection") input.projection_schema = "agent-heist/projection/v2";
    assert.throws(() => projectResult(listing, resolved, encodeCanonical(input)), /reference_mismatch/);
  }
});

test("projector rejects private, unknown, or unreviewed public fields", () => {
  const { listing, projector } = contracts();
  const resolved = resolveProjector(projector);
  for (const mutation of ["private", "outcome", "phase"] as const) {
    const input = mutable("fixtures/hosted-contract/valid/agent-heist-terminal-input.json");
    const projection = record(input.public_projection);
    if (mutation === "private") projection.fixture = { secret: true };
    if (mutation === "outcome") record(projection.outcome).outcome = "spectacular";
    if (mutation === "phase") projection.phase = "unknown";
    assert.throws(() => projectResult(listing, resolved, encodeCanonical(input)));
  }
});

interface WireCase {
  readonly name: string;
  readonly target: "listing" | "projector";
  readonly operation: "replace" | "prefix_space";
  readonly from?: string;
  readonly to?: string;
}

function transformedWire(item: WireCase): Uint8Array {
  const path = item.target === "listing"
    ? "config/hosted/listings/agent-heist-0.2.0.json"
    : "config/hosted/result-projectors/agent-heist-0.2.0.json";
  const bytes = canonical(path);
  if (item.operation === "prefix_space") return Uint8Array.from([32, ...bytes]);
  assert.notEqual(item.from, undefined);
  assert.notEqual(item.to, undefined);
  const text = new TextDecoder().decode(bytes);
  assert.equal(text.split(item.from!).length - 1, 1, `corpus replacement is not unique: ${item.name}`);
  return new TextEncoder().encode(text.replace(item.from!, item.to!));
}

test("TypeScript consumes the shared exact wire rejection corpus", () => {
  const corpus = document("fixtures/hosted-contract/corpus.json") as unknown as {
    listing_digest: string;
    projector_digest: string;
    wire_cases: readonly WireCase[];
  };
  const { listing, projector } = contracts();
  assert.equal(listing.digest, corpus.listing_digest);
  assert.equal(projector.digest, corpus.projector_digest);
  for (const item of corpus.wire_cases) {
    const read = item.target === "listing" ? readListingRevision : readResultProjectorRevision;
    assert.throws(() => read(transformedWire(item)), /hosted contract/, item.name);
  }
});

test("semantic contract changes change identity and break the listing pin", () => {
  const { listing, projector } = contracts();
  const changedListing = mutable("config/hosted/listings/agent-heist-0.2.0.json");
  changedListing.title = "Agent Heist!";
  assert.notEqual(readListingRevision(encodeCanonical(changedListing)).digest, listing.digest);

  const changedProjector = mutable("config/hosted/result-projectors/agent-heist-0.2.0.json");
  const fields = array(record(changedProjector.program).summary_fields).map(record);
  fields[0]!.values = ["failure", "partial_failure", "success"];
  const changed = readResultProjectorRevision(encodeCanonical(changedProjector));
  assert.notEqual(changed.digest, projector.digest);
  assert.throws(() => verifyListingProjector(listing, changed), /reference_mismatch/);
});
