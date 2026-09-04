import {
  decodeCanonical,
  encodeCanonical,
  taggedBlake3,
  type CanonicalJson,
  type CanonicalObject,
} from "@worldstream/pack-sdk";

import { ProjectorRuntimeViolation, interpretProjectorV1 } from "./runtimeV1.js";

const MAX_REVISION_BYTES = 262_144;
const MAX_JSON_NODES = 4_096;
const MAX_JSON_DEPTH = 32;
const MAX_SEATS = 32;
const MAX_RESULT_OUTPUT_BYTES = 16_384;
const PROJECTION_SCHEMA = "agent-heist/projection/v1";
const PROJECTION_SCHEMA_DIGEST = "blake3:a617f398abd1469d237ce4d832704459deddf44193267fab8bb0e1311e5faa8f";
const RESULT_SCHEMA = "worldstream/result-summary/v1";
const RESULT_SCHEMA_DIGEST = "blake3:816295fc5ac531886f090e59de24901633a60ed68af1510e133cead3d7eb35c6";
const RUNTIME_ID = "worldstream.result-projector.declarative";
const RUNTIME_VERSION = "1.0.0";
const RUNTIME_DIGEST = "blake3:7278757ac8f047330292e399ac3c8f4ed49225b05ecb6b997b75500784b0526e";
const RUNTIME_IMPLEMENTATIONS = {
  rust: {
    path: "crates/worldstream-hosted-contract/src/runtime_v1.rs",
    digest: "blake3:29abed9d256ae4978fb30090b4e8f1f1b61bd0e28653b54080c324c04cfb0f1f",
  },
  typescript: {
    path: "sdk/typescript-hosted-contract/src/runtimeV1.ts",
    digest: "blake3:375f96e77cec51ed81ffb83f22bc9187579dfaf00223917815bc947d07e30701",
  },
} as const;

export type ParticipationKind = "account_human" | "account_external_agent" | "house_agent_fill";

export interface PackReference {
  readonly id: string;
  readonly version: string;
  readonly digest: string;
}

interface AgentProfileReference {
  readonly profile_id: string;
  readonly revision: string;
}

interface RunnerTemplateReference {
  readonly template_id: string;
  readonly revision: string;
}

interface ListingSeat {
  readonly seat_id: string;
  readonly role: string;
  readonly display_name: string;
  readonly required: boolean;
  readonly allowed_participation: readonly ParticipationKind[];
  readonly allowed_house_agent_revisions: readonly string[];
}

interface SchemaReference {
  readonly schema: string;
  readonly digest: string;
}

export interface ListingRevisionValue {
  readonly schema: "worldstream/activity-listing-revision/v1";
  readonly listing_id: string;
  readonly version: string;
  readonly title: string;
  readonly description: string;
  readonly catalog: {
    readonly visibility: "public" | "unlisted" | "private";
    readonly review_status: "reviewed";
  };
  readonly pack: PackReference;
  readonly client: {
    readonly client_id: string;
    readonly release_digest: string;
    readonly client_contract: string;
    readonly surface_id: string;
  };
  readonly launch_input_schema: {
    readonly schema: "worldstream/launch-input-schema/v1";
    readonly accepts: "none";
    readonly defaults: Readonly<Record<string, never>>;
  };
  readonly room_setup: { readonly configuration: CanonicalJson };
  readonly seats: readonly ListingSeat[];
  readonly creator_access: "must_claim_seat" | "may_spectate";
  readonly public_viewing_policy: "disabled" | "anonymous_by_link";
  readonly pre_start_deadline_seconds: number;
  readonly result: {
    readonly projection: SchemaReference;
    readonly projector: { readonly id: string; readonly version: string; readonly digest: string };
    readonly publication: {
      readonly policy: "disabled" | "public_recent_results";
      readonly attribution: "none" | "reviewed_pseudonymous_seats";
      readonly public_output: "none" | "projector_summary_only";
      readonly suppression: "unhealthy_inconclusive_or_conflict";
    };
  };
}

interface EnumField {
  readonly output: string;
  readonly source: string;
  readonly kind: "enum";
  readonly values: readonly string[];
}

interface NullableIdentifierField {
  readonly output: string;
  readonly source: string;
  readonly kind: "nullable_identifier";
  readonly maximum_bytes: number;
}

interface IntegerField {
  readonly output: string;
  readonly source: string;
  readonly kind: "integer";
  readonly minimum: number;
  readonly maximum: number;
}

type SummaryField = EnumField | NullableIdentifierField | IntegerField;

export interface ResultProjectorRevisionValue {
  readonly schema: "worldstream/result-projector-revision/v1";
  readonly projector_id: string;
  readonly version: string;
  readonly runtime: {
    readonly id: "worldstream.result-projector.declarative";
    readonly version: "1.0.0";
    readonly digest: string;
  };
  readonly input: {
    readonly pack: PackReference;
    readonly listing_schema: "worldstream/activity-listing-revision/v1";
    readonly complete_head_schema: "worldstream/complete-head/v1";
    readonly projection: SchemaReference;
  };
  readonly program: {
    readonly schema: "worldstream/result-projector-program/v1";
    readonly terminal: { readonly field: string; readonly equals: string };
    readonly outcome_field: string;
    readonly summary_fields: readonly SummaryField[];
  };
  readonly output: {
    readonly schema: string;
    readonly schema_digest: string;
    readonly canonicalizer: "worldstream/canonical-json/v1";
    readonly maximum_bytes: number;
  };
  readonly maximum_input_bytes: number;
}

export interface ListingRevision {
  readonly value: ListingRevisionValue;
  readonly canonicalBytes: Uint8Array;
  readonly digest: string;
}

export interface ResultProjectorRevision {
  readonly value: ResultProjectorRevisionValue;
  readonly canonicalBytes: Uint8Array;
  readonly digest: string;
}

/** An executable projector obtained only after exact runtime and schema resolution. */
export interface ResolvedResultProjector {
  readonly revision: ResultProjectorRevision;
}

const validatedListings = new WeakSet<object>();
const validatedProjectors = new WeakSet<object>();
const resolvedProjectors = new WeakSet<object>();

export interface ActivityClientReleaseIdentity {
  readonly client_id: string;
  readonly release_digest: string;
  readonly client_contract: string;
  readonly surfaces: readonly { readonly surface_id: string }[];
}

export class ContractViolation extends Error {
  constructor(readonly code: "too_large" | "noncanonical" | "invalid_shape" | "unsupported" | "unbounded" | "reference_mismatch" | "output_too_large") {
    super(`hosted contract ${code}`);
    this.name = "ContractViolation";
  }
}

export function readListingRevision(bytes: Uint8Array): ListingRevision {
  const value = readCanonical(bytes, MAX_REVISION_BYTES);
  validateListing(value);
  const canonicalBytes = bytes.slice();
  const revision = Object.freeze({
    value: deepFreeze(value) as unknown as ListingRevisionValue,
    get canonicalBytes(): Uint8Array { return canonicalBytes.slice(); },
    digest: taggedBlake3(bytes),
  });
  validatedListings.add(revision);
  return revision;
}

export function readResultProjectorRevision(bytes: Uint8Array): ResultProjectorRevision {
  const value = readCanonical(bytes, MAX_REVISION_BYTES);
  validateProjector(value);
  const canonicalBytes = bytes.slice();
  const revision = Object.freeze({
    value: deepFreeze(value) as unknown as ResultProjectorRevisionValue,
    get canonicalBytes(): Uint8Array { return canonicalBytes.slice(); },
    digest: taggedBlake3(bytes),
  });
  validatedProjectors.add(revision);
  return revision;
}

export function verifyListingPack(listing: ListingRevision, pack: PackReference): void {
  requireValidatedListing(listing);
  if (!samePack(listing.value.pack, pack)) throw new ContractViolation("reference_mismatch");
}

export function verifyListingClientRelease(
  listing: ListingRevision,
  release: ActivityClientReleaseIdentity,
): void {
  requireValidatedListing(listing);
  const expected = listing.value.client;
  if (
    expected.client_id !== release.client_id
    || expected.release_digest !== release.release_digest
    || expected.client_contract !== release.client_contract
    || !release.surfaces.some((surface) => surface.surface_id === expected.surface_id)
  ) {
    throw new ContractViolation("reference_mismatch");
  }
}

export function verifyListingProjector(
  listing: ListingRevision,
  projector: ResultProjectorRevision,
): void {
  requireValidatedListing(listing);
  requireValidatedProjector(projector);
  const expected = listing.value.result.projector;
  if (
    expected.id !== projector.value.projector_id
    || expected.version !== projector.value.version
    || expected.digest !== projector.digest
    || !sameSchema(listing.value.result.projection, projector.value.input.projection)
    || !samePack(listing.value.pack, projector.value.input.pack)
    || projector.value.input.listing_schema !== listing.value.schema
  ) {
    throw new ContractViolation("reference_mismatch");
  }
}

export function resolveProjectorArtifacts(
  projector: ResultProjectorRevision,
  runtimeBytes: Uint8Array,
  projectionSchemaBytes: Uint8Array,
  outputSchemaBytes: Uint8Array,
): ResolvedResultProjector {
  const revision = readResultProjectorRevision(projector.canonicalBytes);
  if (revision.digest !== projector.digest) throw new ContractViolation("reference_mismatch");
  verifyRuntimeArtifact(revision.value.runtime, runtimeBytes);
  if (
    canonicalDocumentDigest(projectionSchemaBytes) !== revision.value.input.projection.digest
    || canonicalDocumentDigest(outputSchemaBytes) !== revision.value.output.schema_digest
  ) {
    throw new ContractViolation("reference_mismatch");
  }
  const resolved = Object.freeze({ revision });
  resolvedProjectors.add(resolved);
  return resolved;
}

export function deriveRoomSetup(
  listing: ListingRevision,
  launchBytes: Uint8Array,
  rosterBytes: Uint8Array,
): Uint8Array {
  requireValidatedListing(listing);
  const launch = closedRecord(readCanonical(launchBytes, 16_384), ["schema", "listing_revision_digest", "inputs"]);
  const roster = closedRecord(readCanonical(rosterBytes, 65_536), ["schema", "listing_revision_digest", "members"]);
  if (launch.schema !== "worldstream/launch-request/v1" || roster.schema !== "worldstream/frozen-roster/v1") {
    throw new ContractViolation("unsupported");
  }
  if (launch.listing_revision_digest !== listing.digest || roster.listing_revision_digest !== listing.digest) {
    throw new ContractViolation("reference_mismatch");
  }
  closedRecord(launch.inputs, []);
  const memberValues = array(roster.members);
  if (memberValues.length > listing.value.seats.length) throw new ContractViolation("invalid_shape");

  const members = new Map<string, FrozenMember>();
  const principalReferences = new Set<string>();
  for (const value of memberValues) {
    const member = parseFrozenMember(value);
    if (members.has(member.seat_id) || principalReferences.has(member.principal_reference)) {
      throw new ContractViolation("invalid_shape");
    }
    members.set(member.seat_id, member);
    principalReferences.add(member.principal_reference);
  }
  const seats: CanonicalJson[] = [];
  for (const listed of listing.value.seats) {
    const member = members.get(listed.seat_id);
    members.delete(listed.seat_id);
    if (listed.required && member === undefined) throw new ContractViolation("invalid_shape");
    const seat: Record<string, CanonicalJson> = {
      label: listed.seat_id,
      role: listed.role,
      required: listed.required,
      display_name: member?.display_name ?? listed.display_name,
    };
    if (member !== undefined) installParticipation(seat, listed, member);
    seats.push(seat);
  }
  if (members.size !== 0) throw new ContractViolation("invalid_shape");
  const bytes = encodeCanonical({
    schema: "worldstream/room-setup/v1",
    pack: listing.value.pack as unknown as CanonicalObject,
    configuration: listing.value.room_setup.configuration,
    seats,
    operator_view: false,
  });
  if (bytes.byteLength > MAX_REVISION_BYTES) throw new ContractViolation("output_too_large");
  return bytes;
}

interface FrozenMember {
  readonly seat_id: string;
  readonly participation: ParticipationKind;
  readonly principal_reference: string;
  readonly display_name: string;
  readonly house_agent_revision_digest?: string;
  readonly agent_profile?: AgentProfileReference;
  readonly runner_template?: RunnerTemplateReference;
}

function parseFrozenMember(value: CanonicalJson): FrozenMember {
  const item = record(value);
  const allowed = ["seat_id", "participation", "principal_reference", "display_name"];
  if (Object.hasOwn(item, "house_agent_revision_digest")) allowed.push("house_agent_revision_digest");
  if (Object.hasOwn(item, "agent_profile")) allowed.push("agent_profile");
  if (Object.hasOwn(item, "runner_template")) allowed.push("runner_template");
  closedKeys(item, allowed);
  const participation = participationKind(item.participation);
  const member: FrozenMember = {
    seat_id: seatLabel(item.seat_id),
    participation,
    principal_reference: publicReference(item.principal_reference, 128),
    display_name: text(item.display_name, 128),
    ...(item.house_agent_revision_digest !== null && item.house_agent_revision_digest !== undefined
      ? { house_agent_revision_digest: digest(item.house_agent_revision_digest, "blake3") }
      : {}),
    ...(item.agent_profile !== null && item.agent_profile !== undefined
      ? { agent_profile: agentProfileReference(item.agent_profile) }
      : {}),
    ...(item.runner_template !== null && item.runner_template !== undefined
      ? { runner_template: runnerTemplateReference(item.runner_template) }
      : {}),
  };
  const managed = participation === "house_agent_fill";
  if (managed !== (
    member.house_agent_revision_digest !== undefined
    && member.agent_profile !== undefined
    && member.runner_template !== undefined
  )) {
    throw new ContractViolation("invalid_shape");
  }
  return member;
}

function installParticipation(
  seat: Record<string, CanonicalJson>,
  listed: ListingSeat,
  member: FrozenMember,
): void {
  if (!listed.allowed_participation.includes(member.participation)) throw new ContractViolation("unsupported");
  seat.principal = {
    reference: member.principal_reference,
    kind: member.participation === "account_human" ? "human" : "agent",
  };
  if (member.participation === "account_external_agent") seat.assignment = { mode: "external" };
  if (member.participation === "house_agent_fill") {
    if (
      member.house_agent_revision_digest === undefined
      || member.agent_profile === undefined
      || member.runner_template === undefined
    ) {
      throw new ContractViolation("invalid_shape");
    }
    if (!listed.allowed_house_agent_revisions.includes(member.house_agent_revision_digest)) {
      throw new ContractViolation("reference_mismatch");
    }
    seat.assignment = {
      mode: "managed",
      agent_profile: member.agent_profile as unknown as CanonicalObject,
      runner_template: member.runner_template as unknown as CanonicalObject,
    };
  }
}

export function projectResult(
  listing: ListingRevision,
  resolved: ResolvedResultProjector,
  inputBytes: Uint8Array,
): Uint8Array {
  if (!resolvedProjectors.has(resolved)) throw new ContractViolation("unsupported");
  const revision = resolved.revision;
  verifyListingProjector(listing, revision);
  const input = closedRecord(readCanonical(inputBytes, revision.value.maximum_input_bytes), [
    "schema", "listing_revision_digest", "projector_revision_digest", "pack", "projection_schema", "source_head", "public_projection",
  ]);
  if (input.schema !== "worldstream/result-projector-input/v1") throw new ContractViolation("unsupported");
  const pack = packReference(input.pack);
  const sourceHead = validateCompleteHead(input.source_head);
  if (
    input.listing_revision_digest !== listing.digest
    || input.projector_revision_digest !== revision.digest
    || !samePack(pack, revision.value.input.pack)
    || input.projection_schema !== revision.value.input.projection.schema
    || sourceHead.pack_digest !== pack.digest
  ) {
    throw new ContractViolation("reference_mismatch");
  }
  validateAgentHeistPublicProjection(input.public_projection);
  let output: CanonicalObject;
  try {
    output = interpretProjectorV1(revision.value, input.public_projection);
  } catch (error) {
    if (error instanceof ProjectorRuntimeViolation) throw new ContractViolation("invalid_shape");
    throw error;
  }
  validateResultOutput(output);
  const bytes = encodeCanonical(output);
  if (bytes.byteLength > revision.value.output.maximum_bytes) throw new ContractViolation("output_too_large");
  return bytes;
}

function validateListing(value: CanonicalJson): void {
  const listing = closedRecord(value, [
    "schema", "listing_id", "version", "title", "description", "catalog", "pack", "client", "launch_input_schema", "room_setup", "seats", "creator_access", "public_viewing_policy", "pre_start_deadline_seconds", "result",
  ]);
  if (listing.schema !== "worldstream/activity-listing-revision/v1") throw new ContractViolation("unsupported");
  identifier(listing.listing_id, 128);
  version(listing.version);
  text(listing.title, 128);
  text(listing.description, 1_024);
  packReference(listing.pack);
  const catalog = closedRecord(listing.catalog, ["visibility", "review_status"]);
  enumValue(catalog.visibility, ["public", "unlisted", "private"]);
  if (catalog.review_status !== "reviewed") throw new ContractViolation("unsupported");
  const client = closedRecord(listing.client, ["client_id", "release_digest", "client_contract", "surface_id"]);
  identifier(client.client_id, 128);
  digest(client.release_digest, "sha256");
  identifier(client.client_contract, 128);
  identifier(client.surface_id, 128);
  const inputSchema = closedRecord(listing.launch_input_schema, ["schema", "accepts", "defaults"]);
  if (inputSchema.schema !== "worldstream/launch-input-schema/v1" || inputSchema.accepts !== "none") {
    throw new ContractViolation("unsupported");
  }
  closedRecord(inputSchema.defaults, []);
  const setup = closedRecord(listing.room_setup, ["configuration"]);
  validateJson(setup.configuration);
  enumValue(listing.creator_access, ["must_claim_seat", "may_spectate"]);
  enumValue(listing.public_viewing_policy, ["disabled", "anonymous_by_link"]);
  const deadline = integer(listing.pre_start_deadline_seconds);
  if (deadline < 60 || deadline > 86_400) throw new ContractViolation("unbounded");
  validateListingResult(listing.result);

  const seats = array(listing.seats);
  if (seats.length === 0 || seats.length > MAX_SEATS) throw new ContractViolation("unbounded");
  const seatIds = new Set<string>();
  for (const value of seats) {
    const seat = closedRecord(value, ["seat_id", "role", "display_name", "required", "allowed_participation", "allowed_house_agent_revisions"]);
    const seatId = seatLabel(seat.seat_id);
    publicReference(seat.role, 128);
    text(seat.display_name, 128);
    boolean(seat.required);
    if (seatIds.has(seatId)) throw new ContractViolation("invalid_shape");
    seatIds.add(seatId);
    const kinds = array(seat.allowed_participation).map(participationKind);
    if (kinds.length === 0 || kinds.length > 3 || new Set(kinds).size !== kinds.length) {
      throw new ContractViolation("invalid_shape");
    }
    const house = array(seat.allowed_house_agent_revisions).map((item) => digest(item, "blake3"));
    if ((kinds.includes("house_agent_fill") !== (house.length > 0)) || house.length > 32 || new Set(house).size !== house.length) {
      throw new ContractViolation("invalid_shape");
    }
  }
}

function validateListingResult(value: CanonicalJson | undefined): void {
  const result = closedRecord(value, ["projection", "projector", "publication"]);
  schemaReference(result.projection);
  projectorReference(result.projector);
  const publication = closedRecord(result.publication, ["policy", "attribution", "public_output", "suppression"]);
  const policy = enumValue(publication.policy, ["disabled", "public_recent_results"]);
  const attribution = enumValue(publication.attribution, ["none", "reviewed_pseudonymous_seats"]);
  const output = enumValue(publication.public_output, ["none", "projector_summary_only"]);
  if (publication.suppression !== "unhealthy_inconclusive_or_conflict") throw new ContractViolation("unsupported");
  if (!(
    (policy === "disabled" && attribution === "none" && output === "none")
    || (policy === "public_recent_results" && attribution === "reviewed_pseudonymous_seats" && output === "projector_summary_only")
  )) {
    throw new ContractViolation("invalid_shape");
  }
}

function validateProjector(value: CanonicalJson): void {
  const projector = closedRecord(value, ["schema", "projector_id", "version", "runtime", "input", "program", "output", "maximum_input_bytes"]);
  if (projector.schema !== "worldstream/result-projector-revision/v1") {
    throw new ContractViolation("unsupported");
  }
  resolveRuntime(projector.runtime);
  identifier(projector.projector_id, 128);
  version(projector.version);
  const input = closedRecord(projector.input, ["pack", "listing_schema", "complete_head_schema", "projection"]);
  packReference(input.pack);
  if (input.listing_schema !== "worldstream/activity-listing-revision/v1" || input.complete_head_schema !== "worldstream/complete-head/v1") {
    throw new ContractViolation("unsupported");
  }
  const projection = schemaReference(input.projection);
  if (projection.schema !== PROJECTION_SCHEMA || projection.digest !== PROJECTION_SCHEMA_DIGEST) throw new ContractViolation("unsupported");
  const program = closedRecord(projector.program, ["schema", "terminal", "outcome_field", "summary_fields"]);
  if (program.schema !== "worldstream/result-projector-program/v1") throw new ContractViolation("unsupported");
  const terminal = closedRecord(program.terminal, ["field", "equals"]);
  if (jsonKey(terminal.field) !== "phase" || text(terminal.equals, 64) !== "complete" || jsonKey(program.outcome_field) !== "outcome") {
    throw new ContractViolation("invalid_shape");
  }
  const fields = array(program.summary_fields);
  if (fields.length === 0 || fields.length > 32) throw new ContractViolation("unbounded");
  const outputs = new Set<string>();
  const sources = new Set<string>();
  for (const field of fields) validateSummaryField(field, outputs, sources);
  if (!["outcome", "reason", "score", "selected_plan_id"].every((item) => outputs.has(item)) || outputs.size !== 4) {
    throw new ContractViolation("invalid_shape");
  }
  const output = closedRecord(projector.output, ["schema", "schema_digest", "canonicalizer", "maximum_bytes"]);
  if (
    identifier(output.schema, 128) !== RESULT_SCHEMA
    || digest(output.schema_digest, "blake3") !== RESULT_SCHEMA_DIGEST
    || output.canonicalizer !== "worldstream/canonical-json/v1"
  ) {
    throw new ContractViolation("unsupported");
  }
  const maximumInput = integer(projector.maximum_input_bytes);
  const maximumOutput = integer(output.maximum_bytes);
  if (maximumInput < 1 || maximumInput > MAX_REVISION_BYTES || maximumOutput < 128 || maximumOutput > MAX_RESULT_OUTPUT_BYTES) {
    throw new ContractViolation("unbounded");
  }
}

function validateSummaryField(value: CanonicalJson, outputs: Set<string>, sources: Set<string>): void {
  const field = record(value);
  const kind = stringValue(field.kind);
  const keys = kind === "enum"
    ? ["output", "source", "kind", "values"]
    : kind === "nullable_identifier"
      ? ["output", "source", "kind", "maximum_bytes"]
      : kind === "integer"
        ? ["output", "source", "kind", "minimum", "maximum"]
        : [];
  if (keys.length === 0) throw new ContractViolation("unsupported");
  closedKeys(field, keys);
  const output = jsonKey(field.output);
  const source = jsonKey(field.source);
  if (outputs.has(output) || sources.has(source)) throw new ContractViolation("invalid_shape");
  outputs.add(output);
  sources.add(source);
  if (kind === "enum") {
    const values = array(field.values).map((item) => text(item, 128));
    if (values.length === 0 || values.length > 32 || new Set(values).size !== values.length) throw new ContractViolation("invalid_shape");
  } else if (kind === "nullable_identifier") {
    const maximum = integer(field.maximum_bytes);
    if (maximum < 1 || maximum > 128) throw new ContractViolation("unbounded");
  } else {
    const minimum = integer(field.minimum);
    const maximum = integer(field.maximum);
    if (minimum < 0 || minimum > maximum || maximum > 4_294_967_295) throw new ContractViolation("unbounded");
  }
}

function validateCompleteHead(value: CanonicalJson | undefined): { readonly pack_digest: string } {
  const head = closedRecord(value, [
    "room_id", "room_seq", "genesis_or_transition_hash", "core_schema_version", "pack_digest", "core_state_hash", "activity_state_hash", "authoritative_state_hash",
  ]);
  publicReference(head.room_id, 128);
  boundedInteger(head.room_seq, 0, Number.MAX_SAFE_INTEGER);
  for (const field of ["genesis_or_transition_hash", "pack_digest", "core_state_hash", "activity_state_hash", "authoritative_state_hash"] as const) {
    digest(head[field], "blake3");
  }
  if (head.core_schema_version !== "worldstream.core-room-state.v1") throw new ContractViolation("unsupported");
  return { pack_digest: stringValue(head.pack_digest) };
}

function validateAgentHeistPublicProjection(value: CanonicalJson | undefined): void {
  const projection = closedRecord(value, [
    "phase", "phase_generation", "phase_start", "phase_deadline", "seats", "public_claims", "plans", "endorsements", "challenges", "commitment_count", "outcome",
  ]);
  enumValue(projection.phase, ["lobby", "briefing", "negotiation", "commitment", "resolution", "result", "complete"]);
  boundedInteger(projection.phase_generation, 0, 4_294_967_295);
  timestamp(projection.phase_start);
  if (projection.phase_deadline !== null) timestamp(projection.phase_deadline);
  boundedArray(projection.seats, 32, (item) => {
    const seat = closedRecord(item, ["role", "present"]);
    role(seat.role);
    boolean(seat.present);
  });
  boundedArray(projection.public_claims, 64, (item) => {
    const claim = closedRecord(item, ["clue_id", "claim_code"]);
    publicReference(claim.clue_id, 128);
    publicReference(claim.claim_code, 128);
  });
  boundedArray(projection.plans, 64, (item) => {
    const plan = closedRecord(item, ["plan_id", "proposer_role", "created_room_seq", "route", "entry_window", "required_tool", "extraction"]);
    publicReference(plan.plan_id, 128);
    role(plan.proposer_role);
    boundedInteger(plan.created_room_seq, 0, Number.MAX_SAFE_INTEGER);
    enumValue(plan.route, ["canal", "service", "roof"]);
    enumValue(plan.entry_window, ["late", "early", "middle"]);
    enumValue(plan.required_tool, ["disguise", "thermal_key", "jammer"]);
    enumValue(plan.extraction, ["van", "boat", "motorbike"]);
  });
  const endorsements = record(projection.endorsements);
  if (Object.keys(endorsements).length > 32) throw new ContractViolation("unbounded");
  for (const [key, planId] of Object.entries(endorsements)) {
    role(key);
    publicReference(planId, 128);
  }
  boundedArray(projection.challenges, 64, (item) => {
    const challenge = closedRecord(item, ["role", "plan_id", "reason"]);
    role(challenge.role);
    publicReference(challenge.plan_id, 128);
    enumValue(challenge.reason, ["route_conflict", "timing_conflict", "tool_conflict", "extraction_conflict"]);
  });
  boundedInteger(projection.commitment_count, 0, 32);
  if (projection.outcome === undefined) throw new ContractViolation("invalid_shape");
  if (projection.outcome !== null) validatePublicOutcome(projection.outcome);
}

function validatePublicOutcome(value: CanonicalJson): void {
  const outcome = closedRecord(value, ["outcome", "selected_plan_id", "vote_counts", "missing_roles", "checks", "score", "reason"]);
  enumValue(outcome.outcome, ["success", "partial_failure", "failure"]);
  if (outcome.selected_plan_id !== null) publicReference(outcome.selected_plan_id, 128);
  const votes = record(outcome.vote_counts);
  if (Object.keys(votes).length > 64) throw new ContractViolation("unbounded");
  for (const [planId, count] of Object.entries(votes)) {
    publicReference(planId, 128);
    boundedInteger(count, 0, 32);
  }
  const roles = boundedArray(outcome.missing_roles, 3, role);
  if (new Set(roles.map(stringValue)).size !== roles.length) throw new ContractViolation("invalid_shape");
  if (outcome.checks !== null) {
    const checks = closedRecord(outcome.checks, ["route", "entry_window", "required_tool", "extraction", "resource_contributed"]);
    for (const field of Object.values(checks)) boolean(field);
  }
  boundedInteger(outcome.score, 0, 5);
  enumValue(outcome.reason, ["no_strict_majority", "scored_selected_plan"]);
}

function validateResultOutput(value: CanonicalObject): void {
  const status = stringValue(value.status);
  if (status === "not_terminal" || status === "terminal_without_outcome") {
    closedKeys(value, ["status"]);
    return;
  }
  if (status !== "summary") throw new ContractViolation("invalid_shape");
  closedKeys(value, ["status", "summary"]);
  const summary = closedRecord(value.summary, ["schema", "outcome", "selected_plan_id", "score", "reason"]);
  if (summary.schema !== RESULT_SCHEMA) throw new ContractViolation("invalid_shape");
  enumValue(summary.outcome, ["success", "partial_failure", "failure"]);
  if (summary.selected_plan_id !== null) publicReference(summary.selected_plan_id, 128);
  boundedInteger(summary.score, 0, 5);
  enumValue(summary.reason, ["no_strict_majority", "scored_selected_plan"]);
}

function packReference(value: CanonicalJson | undefined): PackReference {
  const item = closedRecord(value, ["id", "version", "digest"]);
  return { id: identifier(item.id, 128), version: version(item.version), digest: digest(item.digest, "blake3") };
}

function schemaReference(value: CanonicalJson | undefined): SchemaReference {
  const item = closedRecord(value, ["schema", "digest"]);
  return { schema: identifier(item.schema, 128), digest: digest(item.digest, "blake3") };
}

function projectorReference(value: CanonicalJson | undefined): void {
  const item = closedRecord(value, ["id", "version", "digest"]);
  identifier(item.id, 128);
  version(item.version);
  digest(item.digest, "blake3");
}

function agentProfileReference(value: CanonicalJson | undefined): AgentProfileReference {
  const item = closedRecord(value, ["profile_id", "revision"]);
  return { profile_id: publicReference(item.profile_id, 128), revision: publicReference(item.revision, 128) };
}

function runnerTemplateReference(value: CanonicalJson | undefined): RunnerTemplateReference {
  const item = closedRecord(value, ["template_id", "revision"]);
  return { template_id: publicReference(item.template_id, 128), revision: publicReference(item.revision, 128) };
}

function samePack(left: PackReference, right: PackReference): boolean {
  return left.id === right.id && left.version === right.version && left.digest === right.digest;
}

function sameSchema(left: SchemaReference, right: SchemaReference): boolean {
  return left.schema === right.schema && left.digest === right.digest;
}

function requireValidatedListing(listing: ListingRevision): void {
  if (!validatedListings.has(listing)) throw new ContractViolation("unsupported");
}

function requireValidatedProjector(projector: ResultProjectorRevision): void {
  if (!validatedProjectors.has(projector)) throw new ContractViolation("unsupported");
}

function readCanonical(bytes: Uint8Array, maximumBytes: number): CanonicalJson {
  if (bytes.byteLength > maximumBytes) throw new ContractViolation("too_large");
  try { return decodeCanonical(bytes); } catch { throw new ContractViolation("noncanonical"); }
}

function canonicalDocumentDigest(bytes: Uint8Array): string {
  if (bytes.byteLength > MAX_REVISION_BYTES) throw new ContractViolation("too_large");
  try {
    decodeCanonical(bytes);
    return taggedBlake3(bytes);
  } catch {
    throw new ContractViolation("noncanonical");
  }
}

function verifyRuntimeArtifact(
  reference: ResultProjectorRevisionValue["runtime"],
  bytes: Uint8Array,
): void {
  resolveRuntime(reference);
  if (canonicalDocumentDigest(bytes) !== reference.digest) {
    throw new ContractViolation("reference_mismatch");
  }
  const artifact = closedRecord(readCanonical(bytes, MAX_REVISION_BYTES), [
    "schema", "runtime_id", "version", "implementations", "semantics",
  ]);
  if (
    artifact.schema !== "worldstream/result-projector-runtime-artifact/v1"
    || artifact.runtime_id !== reference.id
    || artifact.version !== reference.version
  ) {
    throw new ContractViolation("reference_mismatch");
  }
  const semantics = closedRecord(artifact.semantics, [
    "input", "terminal", "missing_outcome", "summary", "canonicalization", "capabilities",
  ]);
  for (const description of Object.values(semantics)) text(description, 1_024);

  const implementations = array(artifact.implementations);
  if (implementations.length !== Object.keys(RUNTIME_IMPLEMENTATIONS).length) {
    throw new ContractViolation("invalid_shape");
  }
  const seen = new Set<string>();
  for (const value of implementations) {
    const implementation = closedRecord(value, ["language", "path", "digest"]);
    const language = stringValue(implementation.language);
    if (seen.has(language)) throw new ContractViolation("invalid_shape");
    seen.add(language);
    if (language !== "rust" && language !== "typescript") throw new ContractViolation("unsupported");
    const expected = RUNTIME_IMPLEMENTATIONS[language];
    digest(implementation.digest, "blake3");
    if (implementation.path !== expected.path || implementation.digest !== expected.digest) {
      throw new ContractViolation("reference_mismatch");
    }
  }
}

function resolveRuntime(value: CanonicalJson | ResultProjectorRevisionValue["runtime"] | undefined): "declarative_v1" {
  const runtime = closedRecord(value as CanonicalJson | undefined, ["id", "version", "digest"]);
  identifier(runtime.id, 128);
  version(runtime.version);
  digest(runtime.digest, "blake3");
  if (runtime.id === RUNTIME_ID && runtime.version === RUNTIME_VERSION && runtime.digest === RUNTIME_DIGEST) {
    return "declarative_v1";
  }
  throw new ContractViolation("unsupported");
}

function deepFreeze(value: CanonicalJson): CanonicalJson {
  if (Array.isArray(value)) {
    for (const item of value) deepFreeze(item);
    return Object.freeze(value);
  }
  if (value !== null && typeof value === "object") {
    for (const item of Object.values(value)) deepFreeze(item);
    return Object.freeze(value);
  }
  return value;
}

function closedRecord(value: CanonicalJson | undefined, keys: readonly string[]): CanonicalObject {
  const item = record(value);
  closedKeys(item, keys);
  return item;
}

function closedKeys(item: CanonicalObject, keys: readonly string[]): void {
  const actual = Object.keys(item);
  if (actual.length !== keys.length || actual.some((key) => !keys.includes(key))) throw new ContractViolation("invalid_shape");
}

function record(value: CanonicalJson | undefined): CanonicalObject {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") throw new ContractViolation("invalid_shape");
  return value as CanonicalObject;
}

function array(value: CanonicalJson | undefined): readonly CanonicalJson[] {
  if (!Array.isArray(value)) throw new ContractViolation("invalid_shape");
  return value;
}

function boundedArray<T>(value: CanonicalJson | undefined, maximum: number, check: (item: CanonicalJson) => T): readonly T[] {
  const values = array(value);
  if (values.length > maximum) throw new ContractViolation("unbounded");
  return values.map(check);
}

function stringValue(value: CanonicalJson | undefined): string {
  if (typeof value !== "string") throw new ContractViolation("invalid_shape");
  return value;
}

function boolean(value: CanonicalJson | undefined): boolean {
  if (typeof value !== "boolean") throw new ContractViolation("invalid_shape");
  return value;
}

function integer(value: CanonicalJson | undefined): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) throw new ContractViolation("invalid_shape");
  return value;
}

function boundedInteger(value: CanonicalJson | undefined, minimum: number, maximum: number): number {
  const selected = integer(value);
  if (selected < minimum || selected > maximum) throw new ContractViolation("unbounded");
  return selected;
}

function identifier(value: CanonicalJson | undefined, maximumBytes: number): string {
  const item = stringValue(value);
  if (utf8Length(item) > maximumBytes || !/^[A-Za-z0-9][A-Za-z0-9._:/-]*$/u.test(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function publicReference(value: CanonicalJson | undefined, maximumBytes: number): string {
  const item = stringValue(value);
  if (utf8Length(item) > maximumBytes || !/^[A-Za-z0-9][A-Za-z0-9._:-]*$/u.test(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function jsonKey(value: CanonicalJson | undefined): string { return publicReference(value, 128); }

function seatLabel(value: CanonicalJson | undefined): string {
  const item = stringValue(value);
  if (utf8Length(item) > 64 || !/^[a-z0-9][a-z0-9-]*$/u.test(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function version(value: CanonicalJson | undefined): string {
  const item = stringValue(value);
  if (utf8Length(item) > 32 || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/u.test(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function text(value: CanonicalJson | undefined, maximumBytes: number): string {
  const item = stringValue(value);
  if (item.length === 0 || utf8Length(item) > maximumBytes || /\p{Cc}/u.test(item)) throw new ContractViolation("unbounded");
  return item;
}

function timestamp(value: CanonicalJson | undefined): string {
  const item = text(value, 64);
  if (!/^[0-9T:Z.+-]+$/u.test(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function role(value: CanonicalJson | undefined): string {
  return enumValue(value, ["navigator", "insider", "broker"]);
}

function enumValue(value: CanonicalJson | undefined, allowed: readonly string[]): string {
  const item = stringValue(value);
  if (!allowed.includes(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function digest(value: CanonicalJson | undefined, algorithm: "blake3" | "sha256"): string {
  const item = stringValue(value);
  if (!new RegExp(`^${algorithm}:[0-9a-f]{64}$`, "u").test(item)) throw new ContractViolation("invalid_shape");
  return item;
}

function participationKind(value: CanonicalJson | undefined): ParticipationKind {
  const item = stringValue(value);
  if (item !== "account_human" && item !== "account_external_agent" && item !== "house_agent_fill") {
    throw new ContractViolation("unsupported");
  }
  return item;
}

function utf8Length(value: string): number { return new TextEncoder().encode(value).byteLength; }

function validateJson(value: CanonicalJson | undefined): void {
  const remaining = { value: MAX_JSON_NODES };
  if (!validateJsonNode(value, 0, remaining)) throw new ContractViolation("unbounded");
}

function validateJsonNode(value: CanonicalJson | undefined, depth: number, remaining: { value: number }): boolean {
  if (value === undefined || depth > MAX_JSON_DEPTH || remaining.value === 0) return false;
  remaining.value -= 1;
  if (value === null || typeof value === "boolean" || typeof value === "number") return true;
  if (typeof value === "string") return utf8Length(value) <= 4_096 && !/\p{Cc}/u.test(value);
  if (Array.isArray(value)) return value.every((item) => validateJsonNode(item, depth + 1, remaining));
  return Object.entries(value).every(([key, item]) => /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u.test(key) && validateJsonNode(item, depth + 1, remaining));
}
