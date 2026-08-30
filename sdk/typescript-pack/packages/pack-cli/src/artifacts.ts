import {
  CANONICAL_CODEC_ID,
  EXECUTION_PROFILE_ID,
  HOST_CONTRACT_ID,
  OPERATION_CODEC_ID,
  REVISION_LOCK_ID,
  TOOLCHAIN_VERSIONS,
} from "./constants.js";
import { blake3Hex, canonicalBytes, taggedBlake3, type JsonValue } from "./canonical.js";
import type { BehavioralEvidence } from "./conformance.js";

export interface SchemaDocument {
  readonly canonical_schema: JsonValue;
  readonly schema_digest: string;
  readonly schema_id: string;
}

export interface GeneratedSemanticArtifacts {
  readonly schemas: JsonValue;
  readonly schemaIds: Readonly<Record<string, string>>;
  readonly actionDigests: Readonly<Record<string, string>>;
  readonly descriptorContent: JsonValue;
  readonly descriptorContentDigest: string;
  readonly schemaBundleDigest: string;
  readonly codecBundle: JsonValue;
  readonly codecBundleDigest: string;
  readonly dependencyLock: JsonValue;
  readonly dependencyLockDigest: string;
}

export interface FinalizedArtifacts {
  readonly revisionDigest: string;
  readonly revisionLock: JsonValue;
  readonly descriptor: JsonValue;
  readonly goldenCorpus: JsonValue;
  readonly conformance: JsonValue;
}

const VIEWER_CLASSES = [
  "public",
  "participant",
  "operator",
  "historical_public",
  "historical_participant",
  "historical_operator",
  "final_reveal",
] as const;

export function generateSemanticArtifacts(evidence: BehavioralEvidence): GeneratedSemanticArtifacts {
  const schemaIds: Record<string, string> = {};
  const documents: SchemaDocument[] = [];
  const add = (key: string, suffix: string, schema: JsonValue): SchemaDocument => {
    const schemaId = `${evidence.packId}/${suffix}/v1`;
    const document = {
      canonical_schema: schema,
      schema_digest: taggedBlake3(canonicalBytes(schema)),
      schema_id: schemaId,
    };
    schemaIds[key] = schemaId;
    documents.push(document);
    return document;
  };
  const objectSchema = { type: "object" } satisfies JsonValue;
  const configuration = add("configuration", "configuration", objectSchema);
  const state = add("state", "state", objectSchema);
  const publicProjection = add("projection:public", "public-projection", objectSchema);
  const participantProjection = add(
    "projection:participant",
    "participant-projection",
    objectSchema,
  );
  const publicObservation = add("observation:public", "public-observation", objectSchema);
  const participantObservation = add(
    "observation:participant",
    "participant-observation",
    objectSchema,
  );
  const rejection = add("rejection", "rejection-detail", objectSchema);
  const timerPayload = add("stimulus:timer_fired", "stimulus-timer-fired", objectSchema);
  const timerRequest = add("output:timer_request", "output-timer-request", objectSchema);
  const actionDigests: Record<string, string> = {};
  const actions = evidence.actions.map((action) => {
    const schema = add(`action:${action}`, `action-${action}`, objectSchema);
    actionDigests[action] = schema.schema_digest;
    return { action_type: action, payload_schema: reference(schema) };
  });
  const eventSchemas: Record<string, JsonValue> = {};
  for (const event of evidence.events) {
    const schema = add(`event:${event}`, `event-${event}`, objectSchema);
    eventSchemas[event] = reference(schema);
  }
  const outputSchemas: Record<string, JsonValue> = {};
  outputSchemas.timer_request = reference(timerRequest);
  for (const code of evidence.rejectionCodes) {
    outputSchemas[`rejection:${code}`] = reference(rejection);
  }
  for (const reason of evidence.attentionReasons) {
    const schema = add(`attention:${reason}`, `attention-${reason}`, objectSchema);
    outputSchemas[`attention:${reason}`] = reference(schema);
  }
  documents.sort((left, right) => left.schema_id.localeCompare(right.schema_id));
  const schemas: JsonValue = {
    schemas: documents.map((document) => ({
      canonical_schema: document.canonical_schema,
      schema_digest: document.schema_digest,
      schema_id: document.schema_id,
    })),
  };
  const schemaReferences = documents.map(reference);
  const schemaBundleDigest = taggedBlake3(
    canonicalBytes({ domain: "worldstream/pack-schema-bundle/v1", schemas: schemaReferences }),
  );
  const codecBundle = {
    codec_id: CANONICAL_CODEC_ID,
    kinds: [
      "configuration",
      "state",
      "stimulus",
      "disposition",
      "domain_event",
      "timer_request",
      "attention_signal",
      "view",
      "observation",
    ],
    retained_reader_versions: [1],
    writer_version: 1,
  } satisfies JsonValue;
  const codecBundleDigest = taggedBlake3(
    canonicalBytes({ domain: "worldstream/pack-codec-bundle/v1", bundle: codecBundle }),
  );
  const publicRef = reference(publicProjection);
  const participantRef = reference(participantProjection);
  const publicObservationRef = reference(publicObservation);
  const participantObservationRef = reference(participantObservation);
  const projectionSchemas: Record<string, JsonValue> = {};
  const observationSchemas: Record<string, JsonValue> = {};
  for (const viewer of VIEWER_CLASSES) {
    const isParticipant = viewer === "participant" || viewer === "historical_participant";
    projectionSchemas[viewer] = isParticipant ? participantRef : publicRef;
    observationSchemas[viewer] = isParticipant ? participantObservationRef : publicObservationRef;
  }
  const descriptorContent = {
    actions,
    attention_reasons: [...evidence.attentionReasons],
    canonical_codec: CANONICAL_CODEC_ID,
    configuration_schema: reference(configuration),
    event_schemas: eventSchemas,
    explanatory_version: evidence.version,
    host_contract: HOST_CONTRACT_ID,
    limits: {
      maximum_attention_signals: 16,
      maximum_collection_items: 256,
      maximum_events: 32,
      maximum_nesting: 32,
      maximum_observation_bytes: 65536,
      maximum_projection_bytes: 65536,
      maximum_state_bytes: 262144,
      maximum_text_bytes: 16384,
      maximum_timer_requests: 16,
    },
    name: evidence.name,
    observation_schemas: observationSchemas,
    output_schemas: outputSchemas,
    pack_id: evidence.packId,
    projection_schemas: projectionSchemas,
    rejection_codes: [...evidence.rejectionCodes],
    roles: evidence.roles.map((role) => ({ maximum: 1, minimum: 1, role })),
    state_schema: reference(state),
    stimulus_schemas: { timer_fired: reference(timerPayload) },
  } satisfies JsonValue;
  const dependencyLock = {
    dependency_lock_id: "worldstream/typescript-pack-dependency-lock/v1",
    operation_codec: OPERATION_CODEC_ID,
    packages: [
      { name: "@bytecodealliance/componentize-js", version: TOOLCHAIN_VERSIONS.componentizeJs },
      { name: "@bytecodealliance/jco", version: TOOLCHAIN_VERSIONS.jco },
      { name: "@noble/hashes", version: TOOLCHAIN_VERSIONS.nobleHashes },
      { name: "@worldstream/pack-cli", version: "0.1.0" },
      { name: "@worldstream/pack-sdk", version: "0.1.0" },
      { name: "typescript", version: TOOLCHAIN_VERSIONS.typescript },
    ],
    toolchain: {
      execution_profile_id: EXECUTION_PROFILE_ID,
      host_contract: HOST_CONTRACT_ID,
    },
  } satisfies JsonValue;
  return {
    schemas,
    schemaIds,
    actionDigests,
    descriptorContent,
    descriptorContentDigest: taggedBlake3(canonicalBytes(descriptorContent)),
    schemaBundleDigest,
    codecBundle,
    codecBundleDigest,
    dependencyLock,
    dependencyLockDigest: taggedBlake3(canonicalBytes(dependencyLock)),
  };
}

export function finalizeArtifacts(
  evidence: BehavioralEvidence,
  semantic: GeneratedSemanticArtifacts,
  componentBytes: Uint8Array,
): FinalizedArtifacts {
  const componentDigest = taggedBlake3(componentBytes);
  const revisionLock = {
    canonical_codec: CANONICAL_CODEC_ID,
    codec_bundle_digest: semantic.codecBundleDigest,
    descriptor_digest: semantic.descriptorContentDigest,
    deterministic_dependency_lock_digest: semantic.dependencyLockDigest,
    deterministic_static_data_digests: [],
    explanatory_version: evidence.version,
    host_contract: HOST_CONTRACT_ID,
    pack_id: evidence.packId,
    revision_lock_id: REVISION_LOCK_ID,
    rule_source_digest: componentDigest,
    schema_bundle_digest: semantic.schemaBundleDigest,
  } satisfies JsonValue;
  const revisionDigest = taggedBlake3(canonicalBytes(revisionLock));
  const descriptor = {
    ...(semantic.descriptorContent as Record<string, JsonValue>),
    revision_digest: revisionDigest,
  } satisfies JsonValue;
  const fixture = evidence.goldenFixture as Record<string, JsonValue>;
  const buyer = fixture.buyer as Record<string, JsonValue>;
  const seller = fixture.seller as Record<string, JsonValue>;
  const participants = Array.isArray(fixture.participants)
    ? fixture.participants as Record<string, JsonValue>[]
    : [buyer, seller];
  const createdAt = typeof fixture.created_at === "string"
    ? fixture.created_at
    : "2026-08-30T12:00:00Z";
  const actions = fixture.accepted as JsonValue[];
  const goldenParticipants = withGoldenAccessMemberships(participants);
  const goldenCorpus = {
    actions: actions.map((item, index) => {
      const action = item as Record<string, JsonValue>;
      return {
        action_id: `01ARZ3NDEKTSV4RRFFQ69G5F${String(index + 2).padStart(2, "0")}`,
        action_type: String(action.action_type),
        admitted_at:
          typeof action.admitted_at === "string"
            ? action.admitted_at
            : `2026-08-30T12:00:0${index + 1}Z`,
        canonical_payload: action.canonical_payload!,
        member_id: String(action.member_id),
        payload_schema_digest: semantic.actionDigests[String(action.action_type)]!,
      };
    }),
    corpus_id: "worldstream/pack-golden-corpus/v1",
    expected_transcript_digest: evidence.retainedTranscriptDigest,
    genesis: {
      configuration: fixture.configuration!,
      created_at: createdAt,
      initial_core_state: {
        memberships: Object.fromEntries(
          goldenParticipants.map((participant, index) => {
            const memberId = String(participant.member_id);
            return [memberId, goldenMembership(participant, index)];
          }),
        ),
        room_status: "active",
      },
      pack_digest: revisionDigest,
      room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    },
    viewers: goldenViewers(goldenParticipants),
  } satisfies JsonValue;
  const goldenCorpusDigest = taggedBlake3(canonicalBytes(goldenCorpus));
  const conformance = {
    conformance_id: "worldstream/pack-conformance/v1",
    execution_profile_id: EXECUTION_PROFILE_ID,
    executor_component_digest: componentDigest,
    golden_corpus_digest: goldenCorpusDigest,
    passed: true,
    revision_digest: revisionDigest,
  } satisfies JsonValue;
  return { revisionDigest, revisionLock, descriptor, goldenCorpus, conformance };
}

function reference(document: SchemaDocument): JsonValue {
  return { schema_digest: document.schema_digest, schema_id: document.schema_id };
}

function goldenMembership(participant: Record<string, JsonValue>, index: number): JsonValue {
  const memberId = String(participant.member_id);
  const defaultPrincipal = `01ARZ3NDEKTSV4RRFFQ69G5FD${index}`;
  return {
    access_mode:
      typeof participant.access_mode === "string" ? participant.access_mode : "participant",
    member_id: memberId,
    principal_id:
      typeof participant.principal_id === "string" ? participant.principal_id : defaultPrincipal,
    principal_kind:
      typeof participant.principal_kind === "string" ? participant.principal_kind : "agent",
    role: typeof participant.role === "string" ? participant.role : null,
    standing: typeof participant.standing === "string" ? participant.standing : "enabled",
  };
}

function withGoldenAccessMemberships(
  participants: readonly Record<string, JsonValue>[],
): Record<string, JsonValue>[] {
  const result = participants.map((participant) => ({ ...participant }));
  if (!result.some((participant) => participant.access_mode === "spectator")) {
    result.push({
      access_mode: "spectator",
      member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC8",
      principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD8",
      principal_kind: "human",
      role: null,
      standing: "enabled",
    });
  }
  if (!result.some((participant) => participant.access_mode === "operator")) {
    result.push({
      access_mode: "operator",
      member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC9",
      principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD9",
      principal_kind: "human",
      role: null,
      standing: "enabled",
    });
  }
  return result;
}

function goldenViewers(
  memberships: readonly Record<string, JsonValue>[],
): JsonValue[] {
  const participants = memberships.filter(
    (membership) => (membership.access_mode ?? "participant") === "participant",
  );
  const spectator = memberships.find((membership) => membership.access_mode === "spectator")!;
  const operator = memberships.find((membership) => membership.access_mode === "operator")!;
  const firstParticipant = participants[0]!;
  return [
    ...participants.map((participant) => ({
      available_after_action: 0,
      denied_before_detail: null,
      kind: "participant",
      member_id: String(participant.member_id),
    })),
    {
      available_after_action: 0,
      denied_before_detail: null,
      kind: "public",
      member_id: String(spectator.member_id),
    },
    {
      available_after_action: 0,
      denied_before_detail: null,
      kind: "operator",
      member_id: String(operator.member_id),
    },
    {
      available_after_action: 0,
      denied_before_detail: null,
      kind: "historical",
      member_id: String(spectator.member_id),
    },
    {
      available_after_action: 0,
      denied_before_detail: null,
      kind: "historical",
      member_id: String(firstParticipant.member_id),
    },
    {
      available_after_action: 0,
      denied_before_detail: null,
      kind: "historical",
      member_id: String(operator.member_id),
    },
    {
      available_after_action: 0,
      denied_before_detail: null,
      kind: "final_reveal",
      member_id: String(spectator.member_id),
    },
  ];
}
