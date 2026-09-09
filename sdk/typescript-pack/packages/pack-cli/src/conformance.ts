import { spawn } from "node:child_process";
import { pathToFileURL } from "node:url";

import { canonicalBytes, canonicalJson, taggedBlake3, type JsonValue } from "./canonical.js";
import { GENERATED_PACK_LIMITS, GENERATED_REJECTION_SCHEMA } from "./constants.js";
import { PackCliError, diagnostic } from "./diagnostics.js";
import type { ResolvedPackProject } from "./project.js";
import { emitForExecution } from "./compiler.js";

export interface RoleEvidence {
  readonly maximum: number;
  readonly minimum: number;
  readonly role: string;
}

export interface ActionEvidence {
  readonly actionType: string;
  readonly payloadSchema: JsonValue;
}

export interface EventEvidence {
  readonly eventType: string;
  readonly payloadSchema: JsonValue;
}

export interface BehavioralEvidence {
  readonly packId: string;
  readonly name: string;
  readonly version: string;
  readonly operations: readonly string[];
  readonly roles: readonly RoleEvidence[];
  readonly actions: readonly ActionEvidence[];
  readonly rejectionCodes: readonly string[];
  readonly events: readonly EventEvidence[];
  readonly attentionReasons: readonly string[];
  readonly configurationSchema: JsonValue;
  readonly stateSchema: JsonValue;
  readonly projectionSchemas: Readonly<Record<ActivityPackViewerClass, JsonValue>>;
  readonly observationSchemas: Readonly<Record<ActivityPackViewerClass, JsonValue>>;
  readonly declaresAudienceSchemas: boolean;
  readonly externalInputSchemas: Readonly<Record<string, JsonValue>>;
  readonly activityStartContract?: ActivityStartContractEvidence;
  readonly goldenFixture: JsonValue;
  readonly externalInputs: readonly JsonValue[];
  readonly acceptedActions: number;
  readonly declaredRejections: number;
  readonly deterministicRestart: boolean;
  readonly privateViewsDistinct: boolean;
  readonly crossRoleMutationHidden: boolean;
  readonly transcriptDigest: string;
  readonly retainedTranscriptDigest: string;
  readonly transcript: readonly JsonValue[];
}

/** Narrow declaration for one Host-owned pre-start ExternalInput. */
export interface ActivityStartContractEvidence {
  readonly contract: "worldstream/activity-start/v1";
  readonly preStartPhase: string;
  readonly inputType: string;
  readonly canonicalPayload: JsonValue;
}

type ActivityPackViewerClass =
  | "public"
  | "participant"
  | "operator"
  | "historical_public"
  | "historical_participant"
  | "historical_operator"
  | "final_reveal";

const VIEWER_CLASSES = [
  "public",
  "participant",
  "operator",
  "historical_public",
  "historical_participant",
  "historical_operator",
  "final_reveal",
] as const satisfies readonly ActivityPackViewerClass[];
const LEGACY_OBJECT_SCHEMA = { type: "object" } satisfies JsonValue;

type ActivityPackSchema = Record<string, JsonValue>;
type ActivityPackRoleDraft =
  | string
  | Readonly<{
      readonly role: string;
      readonly minimum: number;
      readonly maximum: number;
    }>;
type ActivityPackActionDraft =
  | string
  | Readonly<{
      readonly actionType: string;
      readonly payloadSchema: ActivityPackSchema;
    }>;
type ActivityPackEventDraft =
  | string
  | Readonly<{
      readonly eventType: string;
      readonly payloadSchema: ActivityPackSchema;
    }>;
interface ActivityPackDefinition {
  readonly descriptor: {
    readonly packId: string;
    readonly name: string;
    readonly version: string;
    readonly roles: readonly ActivityPackRoleDraft[];
    readonly actions: readonly ActivityPackActionDraft[];
    readonly rejectionCodes: readonly string[];
    readonly events: readonly ActivityPackEventDraft[];
    readonly attentionReasons: readonly string[];
    readonly configurationSchema?: ActivityPackSchema;
    readonly stateSchema?: ActivityPackSchema;
    readonly projectionSchemas?: Readonly<
      Partial<Record<ActivityPackViewerClass, ActivityPackSchema>>
    >;
    readonly observationSchemas?: Readonly<
      Partial<Record<ActivityPackViewerClass, ActivityPackSchema>>
    >;
    readonly externalInputSchemas?: Readonly<Record<string, ActivityPackSchema>>;
    readonly activityStartContract?: ActivityStartContractEvidence;
  };
  initialize(input: Record<string, JsonValue>): JsonValue;
  reduce(input: Record<string, JsonValue>): JsonValue;
  view(input: Record<string, JsonValue>): JsonValue;
  observe(input: Record<string, JsonValue>): JsonValue;
}

interface AudienceFixture {
  readonly memberId: string;
  readonly viewerType: "participant" | "public" | "operator" | "historical" | "final_reveal";
}

interface NormalizedDescriptor {
  readonly roles: readonly RoleEvidence[];
  readonly actions: readonly ActionEvidence[];
  readonly events: readonly EventEvidence[];
  readonly configurationSchema: JsonValue;
  readonly stateSchema: JsonValue;
  readonly projectionSchemas: Readonly<Record<ActivityPackViewerClass, JsonValue>>;
  readonly observationSchemas: Readonly<Record<ActivityPackViewerClass, JsonValue>>;
  readonly externalInputSchemas: Readonly<Record<string, JsonValue>>;
  readonly activityStartContract?: ActivityStartContractEvidence;
}

export async function runBehavioralConformance(
  project: ResolvedPackProject,
): Promise<BehavioralEvidence> {
  const emitted = await emitForExecution(project, "conformance");
  const packModule = await freshImport<{ readonly default: ActivityPackDefinition }>(emitted.entrypoint);
  const goldenModule = await freshImport<{ readonly goldenFixture: JsonValue }>(emitted.golden);
  const privacyModule = await freshImport<{ readonly privacyFixture: JsonValue }>(emitted.privacy);
  const pack = packModule.default;
  const descriptor = validateDefinition(pack, project.entrypoint);
  const fixture = asRecord(goldenModule.goldenFixture, "goldenFixture");
  const privacy = asRecord(privacyModule.privacyFixture, "privacyFixture");
  const configuration = fixture.configuration;
  if (!matchesSchema(configuration, descriptor.configurationSchema)) {
    failTest("goldenFixture.configuration does not satisfy descriptor.configurationSchema");
  }
  const participants = fixtureParticipants(fixture);
  validateRoster(participants, descriptor.roles);
  const rosterCases = validateRosterCases(fixture, descriptor.roles);
  const accepted = asArray(fixture.accepted, "goldenFixture.accepted");
  const rejected = asRecord(fixture.rejected, "goldenFixture.rejected");
  const invalidActions = fixture.invalidActions === undefined
    ? []
    : asArray(fixture.invalidActions, "goldenFixture.invalidActions");
  const externalInputs = fixture.externalInputs === undefined
    ? []
    : asArray(fixture.externalInputs, "goldenFixture.externalInputs");
  const core = coreWith(participants);
  const initRequest = initializationRequest(configuration, core, fixture.created_at);
  const firstInit = deterministic("initialize", () => pack.initialize(initRequest));
  const secondInit = pack.initialize(initRequest);
  const deterministicRestart = canonicalJson(firstInit) === canonicalJson(secondInit);
  if (!deterministicRestart) failTest("initialize changed across a simulated restart");
  let state = asRecord(firstInit, "initialize output").initial_activity_state;
  assertMatchesSchema(state, descriptor.stateSchema, "initialize output");
  let scheduledTimers = applyTimerRequests(
    {},
    asArray(asRecord(firstInit, "initialize output").timer_requests, "initialize timer requests"),
  );
  const transcript: JsonValue[] = [firstInit];
  let sequence = 1;
  const start = descriptor.activityStartContract;
  if (start !== undefined) {
    if (asRecord(state, "initial Activity State").phase !== start.preStartPhase) {
      failTest("activityStartContract preStartPhase does not match the initialized root Activity State.phase");
    }
    const matching = externalInputs.filter((value) => {
      const input = asRecord(value, "Activity Start ExternalInput");
      return input.source_id === "01ARZ3NDEKTSV4RRFFQ69G5FH1" &&
        input.input_type === start.inputType &&
        canonicalJson(input.canonical_payload!) === canonicalJson(start.canonicalPayload) &&
        Array.isArray(input.immutable_resource_references) && input.immutable_resource_references.length === 0;
    });
    if (matching.length !== 1) {
      failTest("goldenFixture must contain exactly one exact Activity Start Contract ExternalInput");
    }
    if (externalInputs[0] !== matching[0]) {
      failTest("the exact Activity Start Contract ExternalInput must be the first external input");
    }
    const wrong = {
      ...asRecord(matching[0], "Activity Start ExternalInput"),
      source_id: "01ARZ3NDEKTSV4RRFFQ69G5FC7",
    } satisfies Record<string, JsonValue>;
    const rejectedWrongStart = deterministic("reduce wrong Activity Start", () =>
      pack.reduce(reduceRequest(state, core, scheduledTimers, externalInputStimulus(wrong), sequence)),
    );
    assertCleanDeclaredRejection(
      rejectedWrongStart,
      pack.descriptor.rejectionCodes,
      "wrong Activity Start output",
    );
    transcript.push(rejectedWrongStart);
  }

  const declaredRejection = deterministic("reduce rejection", () =>
    pack.reduce(reduceRequest(state, core, scheduledTimers, actionStimulus(rejected), sequence)),
  );
  if (asRecord(declaredRejection, "declared rejection").activity_disposition_type !== "reject") {
    failTest("the declared-rejection fixture was not rejected");
  }
  transcript.push(declaredRejection);

  for (const [index, inputValue] of externalInputs.entries()) {
    const input = asRecord(inputValue, `goldenFixture.externalInputs[${index}]`);
    const inputType = stringField(input, "input_type", "ExternalInput fixture");
    const schema = descriptor.externalInputSchemas[inputType];
    if (schema === undefined) failTest(`ExternalInput ${inputType} has no declared schema`);
    assertMatchesSchema(input.canonical_payload, schema, `ExternalInput ${inputType}`);
    const output = deterministic(`reduce ExternalInput ${index + 1}`, () =>
      pack.reduce(reduceRequest(state, core, scheduledTimers, externalInputStimulus(input), sequence)),
    );
    const disposition = asRecord(output, "ExternalInput reduce output");
    if (disposition.activity_disposition_type !== "apply") failTest(`ExternalInput ${inputType} did not apply`);
    state = disposition.next_activity_state;
    assertMatchesSchema(state, descriptor.stateSchema, `ExternalInput ${inputType} Activity State`);
    scheduledTimers = applyTimerRequests(
      scheduledTimers,
      asArray(disposition.timer_requests, "ExternalInput timer requests"),
    );
    transcript.push(output);
    sequence += 1;
    if (start !== undefined && input.source_id === "01ARZ3NDEKTSV4RRFFQ69G5FH1" &&
      input.input_type === start.inputType &&
      canonicalJson(input.canonical_payload!) === canonicalJson(start.canonicalPayload)) {
      if (asRecord(state, "started Activity State").phase === start.preStartPhase) {
        failTest("the declared Activity Start Contract did not leave its preStartPhase");
      }
      const repeated = deterministic("reduce repeated Activity Start", () =>
        pack.reduce(reduceRequest(state, core, scheduledTimers, externalInputStimulus(input), sequence)),
      );
      assertCleanDeclaredRejection(
        repeated,
        pack.descriptor.rejectionCodes,
        "repeated Activity Start output",
      );
      transcript.push(repeated);
    }
  }

  for (const [index, actionValue] of invalidActions.entries()) {
    const action = asRecord(actionValue, `goldenFixture.invalidActions[${index}]`);
    const definition = actionDefinition(descriptor, action, "invalid Action");
    if (matchesSchema(action.canonical_payload, definition.payloadSchema)) {
      failTest(`invalid Action ${definition.actionType} satisfies its declared payload schema`);
    }
  }

  let acceptedActions = 0;
  for (const [index, actionValue] of accepted.entries()) {
    const action = asRecord(actionValue, `goldenFixture.accepted[${index}]`);
    const definition = actionDefinition(descriptor, action, "accepted Action");
    assertMatchesSchema(action.canonical_payload, definition.payloadSchema, `accepted Action ${definition.actionType}`);
    const output = deterministic(`reduce accepted Action ${index + 1}`, () =>
      pack.reduce(reduceRequest(state, core, scheduledTimers, actionStimulus(action), sequence)),
    );
    const disposition = asRecord(output, "reduce output");
    if (disposition.activity_disposition_type !== "apply") failTest(`accepted Action ${index + 1} did not apply`);
    state = disposition.next_activity_state;
    assertMatchesSchema(state, descriptor.stateSchema, `accepted Action ${definition.actionType} Activity State`);
    scheduledTimers = applyTimerRequests(scheduledTimers, asArray(disposition.timer_requests, "reduce timer requests"));
    acceptedActions += 1;
    transcript.push(output);
    sequence += 1;
  }

  exerciseRosterCases(
    pack,
    descriptor,
    configuration,
    fixture.created_at,
    rosterCases,
    externalInputs,
    accepted,
  );

  const audiences = audienceFixtures(privacy, participants);
  const views = new Map<string, JsonValue>();
  for (const audience of audiences) {
    const view = deterministic(
      `${audience.viewerType} view`,
      () => pack.view(viewRequest(state, core, audience)),
    );
    const record = asRecord(view, `${audience.viewerType} view`);
    const schemaId = stringField(record, "projection_schema", `${audience.viewerType} view`);
    if (!isActivityPackViewerClass(schemaId)) {
      failTest(`${audience.viewerType} view uses undeclared projection schema ${schemaId}`);
    }
    assertMatchesSchema(
      record.projection,
      descriptor.projectionSchemas[schemaId],
      `${audience.viewerType} projection`,
    );
    views.set(audienceKey(audience), view);
  }
  const publicAudience = audiences.find((audience) => audience.viewerType === "public");
  if (publicAudience === undefined) failTest("privacyFixture.audiences must declare one public audience");
  assertForbidden(
    views.get(audienceKey(publicAudience))!,
    asArray(privacy.forbiddenPublic, "privacyFixture.forbiddenPublic"),
  );
  const privateAudiences = privateAudienceFixtures(privacy, audiences);
  const privateViews = privateAudiences.map((audience) => {
    const view = views.get(audienceKey(audience));
    if (view === undefined) failTest(`missing participant audience ${audience.memberId}`);
    return asRecord(asRecord(view, "participant view").projection, "participant projection");
  });
  const privateViewsDistinct = canonicalJson(privateViews[0]!) !== canonicalJson(privateViews[1]!);
  if (!privateViewsDistinct) failTest("the declared private audience views are not distinct");

  const mutations = privacy.mutations === undefined
    ? legacyPrivacyMutations(state, participants)
    : asArray(privacy.mutations, "privacyFixture.mutations");
  for (const [index, mutationValue] of mutations.entries()) {
    const mutation = asRecord(mutationValue, `privacyFixture.mutations[${index}]`);
    const mutatedState = mutation.activity_state;
    assertMatchesSchema(mutatedState, descriptor.stateSchema, `privacy mutation ${index + 1}`);
    if (mutatedState === undefined) failTest(`privacy mutation ${index + 1} has no Activity State`);
    if (canonicalJson(mutatedState) === canonicalJson(state!)) {
      failTest(`privacy mutation ${index + 1} does not change Activity State`);
    }
    const hiddenFrom = asArray(mutation.hidden_from, `privacyFixture.mutations[${index}].hidden_from`);
    const hiddenIds = new Set<string>();
    for (const memberId of hiddenFrom) {
      if (typeof memberId !== "string") failTest(`privacy mutation ${index + 1} has a non-string member ID`);
      hiddenIds.add(memberId);
      const audience = audiences.find(
        (candidate) => candidate.memberId === memberId && candidate.viewerType === "participant",
      );
      if (audience === undefined) failTest(`privacy mutation ${index + 1} names no participant audience`);
      const before = views.get(audienceKey(audience))!;
      const after = pack.view(viewRequest(mutatedState, core, audience));
      if (canonicalJson(before) !== canonicalJson(after)) {
        failTest(`privacy mutation ${index + 1} changed ${memberId}'s authorized view`);
      }
    }
    const owningAudiences = audiences.filter(
      (candidate) => candidate.viewerType === "participant" && !hiddenIds.has(candidate.memberId),
    );
    if (hiddenIds.size === 0 || owningAudiences.length === 0) {
      failTest(`privacy mutation ${index + 1} needs hidden and owning participant audiences`);
    }
    const owningViewChanged = owningAudiences.some((audience) => {
      const before = views.get(audienceKey(audience))!;
      const after = pack.view(viewRequest(mutatedState, core, audience));
      return canonicalJson(before) !== canonicalJson(after);
    });
    if (!owningViewChanged) {
      failTest(`privacy mutation ${index + 1} did not change its owning audience view`);
    }
  }
  if (
    mutations.length === 0 &&
    (privacy.mutations !== undefined || pack.descriptor.projectionSchemas !== undefined ||
      pack.descriptor.observationSchemas !== undefined)
  ) {
    failTest("privacyFixture must declare a real privacy mutation for Packs with audience schemas");
  }

  const observingAudience = privateAudiences[0]!;
  const observation = deterministic("observe", () => pack.observe({
    activity_after: state!,
    activity_before: asRecord(firstInit, "initialize output").initial_activity_state!,
    after_view: asRecord(views.get(audienceKey(observingAudience))!, "participant view"),
    core_after: core,
    core_before: core,
    ordered_domain_events: [],
    recorded_stimulus: actionStimulus(asRecord(accepted.at(-1), "last accepted Action")),
    viewer: viewerFor(observingAudience),
  }));
  if (!new Set(["unchanged", "reuse_after_view"]).has(String(asRecord(observation, "observation").action_offers))) {
    failTest("observe must use unchanged or reuse_after_view for Action Offers");
  }
  transcript.push(...views.values(), observation);

  if (emitted.customTest) await runCustomTest(emitted.customTest, project.root);
  const transcriptDigest = taggedBlake3(canonicalBytes(transcript));
  return {
    packId: pack.descriptor.packId,
    name: pack.descriptor.name,
    version: pack.descriptor.version,
    operations: ["descriptor", "initialize", "reduce", "view", "observe"],
    roles: descriptor.roles,
    actions: descriptor.actions,
    rejectionCodes: [...pack.descriptor.rejectionCodes],
    events: descriptor.events,
    attentionReasons: [...pack.descriptor.attentionReasons],
    configurationSchema: descriptor.configurationSchema,
    stateSchema: descriptor.stateSchema,
    projectionSchemas: descriptor.projectionSchemas,
    observationSchemas: descriptor.observationSchemas,
    declaresAudienceSchemas: pack.descriptor.projectionSchemas !== undefined ||
      pack.descriptor.observationSchemas !== undefined,
    externalInputSchemas: descriptor.externalInputSchemas,
    ...(descriptor.activityStartContract === undefined
      ? {}
      : { activityStartContract: descriptor.activityStartContract }),
    goldenFixture: fixture,
    externalInputs,
    acceptedActions,
    declaredRejections: 1,
    deterministicRestart,
    privateViewsDistinct,
    crossRoleMutationHidden: mutations.length > 0,
    transcriptDigest,
    retainedTranscriptDigest: retainedTranscriptDigest(fixture, transcriptDigest),
    transcript,
  };
}

export function retainedTranscriptDigest(
  fixture: Record<string, JsonValue>,
  localTranscriptDigest: string,
): string {
  const authored = fixture.expected_transcript_digest;
  if (authored === undefined) return localTranscriptDigest;
  if (typeof authored !== "string" || !/^blake3:[0-9a-f]{64}$/u.test(authored)) {
    throw new PackCliError(diagnostic(
      "WSP-GOLDEN-004",
      "expected_transcript_digest must be an exact tagged BLAKE3 identity",
      { detail: "Use the authoritative digest returned by the production WorldStream Core golden prover." },
    ));
  }
  return authored;
}

function validateDefinition(pack: ActivityPackDefinition, file: string): NormalizedDescriptor {
  if (!pack || typeof pack !== "object") {
    throw new PackCliError(diagnostic(
      "WSP-CONTRACT-001",
      "Entrypoint must default-export one ActivityPackDefinition",
      { file },
    ));
  }
  if (
    !pack.descriptor ||
    ["initialize", "reduce", "view", "observe"].some((name) => typeof pack[name as "initialize"] !== "function")
  ) {
    throw new PackCliError(diagnostic("WSP-CONTRACT-002", "Activity Pack definition is incomplete", { file }));
  }
  const { descriptor } = pack;
  const roles = descriptor.roles.map(normalizeRole);
  const actions = descriptor.actions.map(normalizeAction);
  const events = descriptor.events.map(normalizeEvent);
  if (
    !/^[a-z0-9]+(?:[.-][a-z0-9]+)+$/u.test(descriptor.packId) || descriptor.name.length === 0 ||
    descriptor.version.length === 0 || roles.length < 2 || actions.length === 0 ||
    descriptor.rejectionCodes.length === 0
  ) {
    throw new PackCliError(diagnostic(
      "WSP-CONTRACT-003",
      "Activity Pack descriptor draft is invalid",
      { file, detail: "Use a stable dotted pack ID, two or more Roles, Actions, and declared rejection codes." },
    ));
  }
  if (
    roles.some((role) => role.role.length === 0 || !Number.isSafeInteger(role.minimum) ||
      !Number.isSafeInteger(role.maximum) || role.minimum < 0 || role.maximum === 0 ||
      role.minimum > role.maximum) || !unique(roles.map((role) => role.role))
  ) {
    failTest("Role declarations must be unique and have coherent cardinalities");
  }
  if (
    !unique(actions.map((action) => action.actionType)) || !unique(events.map((event) => event.eventType)) ||
    !unique(descriptor.rejectionCodes) || !unique(descriptor.attentionReasons)
  ) {
    failTest("descriptor declarations must be unique and non-empty");
  }
  const configurationSchema = schemaOrLegacy(descriptor.configurationSchema, "configurationSchema");
  const stateSchema = schemaOrLegacy(descriptor.stateSchema, "stateSchema");
  const typedDeclarations = [
    ...actions.map((action) => [action.actionType, action.payloadSchema] as const),
    ...events.map((event) => [event.eventType, event.payloadSchema] as const),
  ];
  for (const [name, schema] of typedDeclarations) {
    assertSupportedSchema(schema as ActivityPackSchema, name);
  }
  const projectionSchemas = audienceSchemas(descriptor.projectionSchemas, "projectionSchemas");
  const observationSchemas = audienceSchemas(descriptor.observationSchemas, "observationSchemas");
  const externalInputSchemas: Record<string, JsonValue> = {};
  for (const [inputType, schema] of Object.entries(descriptor.externalInputSchemas ?? {})) {
    if (inputType.length === 0) failTest("externalInputSchemas must use non-empty input types");
    assertSupportedSchema(schema, `external input ${inputType}`);
    externalInputSchemas[inputType] = schema;
  }
  const activityStartContract = descriptor.activityStartContract;
  if (activityStartContract !== undefined) {
    if (
      activityStartContract.contract !== "worldstream/activity-start/v1" ||
      typeof activityStartContract.preStartPhase !== "string" ||
      activityStartContract.preStartPhase.length === 0 ||
      typeof activityStartContract.inputType !== "string" ||
      activityStartContract.inputType.length === 0 ||
      externalInputSchemas[activityStartContract.inputType] === undefined
    ) {
      failTest("activityStartContract must name one declared ExternalInput and a non-empty root phase");
    }
    canonicalJson(activityStartContract.canonicalPayload);
    if (
      new TextEncoder().encode(activityStartContract.preStartPhase).length > 256 ||
      new TextEncoder().encode(activityStartContract.inputType).length > 256 ||
      canonicalBytes(activityStartContract.canonicalPayload).length > 16_384
    ) {
      failTest("activityStartContract exceeds its bounded phase, input type, or payload size");
    }
    if (!matchesSchema(activityStartContract.canonicalPayload, externalInputSchemas[activityStartContract.inputType]!)) {
      failTest("activityStartContract.canonicalPayload does not satisfy its declared ExternalInput schema");
    }
  }
  return {
    roles,
    actions,
    events,
    configurationSchema,
    stateSchema,
    projectionSchemas,
    observationSchemas,
    externalInputSchemas,
    ...(activityStartContract === undefined ? {} : { activityStartContract }),
  };
}

function normalizeRole(value: ActivityPackRoleDraft): RoleEvidence {
  return typeof value === "string"
    ? { maximum: 1, minimum: 1, role: value }
    : { maximum: value.maximum, minimum: value.minimum, role: value.role };
}

function normalizeAction(value: ActivityPackActionDraft): ActionEvidence {
  return typeof value === "string"
    ? { actionType: value, payloadSchema: LEGACY_OBJECT_SCHEMA }
    : { actionType: value.actionType, payloadSchema: value.payloadSchema };
}

function normalizeEvent(value: ActivityPackEventDraft): EventEvidence {
  return typeof value === "string"
    ? { eventType: value, payloadSchema: LEGACY_OBJECT_SCHEMA }
    : { eventType: value.eventType, payloadSchema: value.payloadSchema };
}

function schemaOrLegacy(value: ActivityPackSchema | undefined, label: string): JsonValue {
  const schema = value ?? LEGACY_OBJECT_SCHEMA;
  assertSupportedSchema(schema, label);
  return schema;
}

function audienceSchemas(
  supplied: Readonly<Partial<Record<ActivityPackViewerClass, ActivityPackSchema>>> | undefined,
  label: string,
): Readonly<Record<ActivityPackViewerClass, JsonValue>> {
  const result = {} as Record<ActivityPackViewerClass, JsonValue>;
  for (const audience of VIEWER_CLASSES) {
    const schema = supplied?.[audience] ?? LEGACY_OBJECT_SCHEMA;
    assertSupportedSchema(schema, `${label}.${audience}`);
    result[audience] = schema;
  }
  return result;
}

function assertSupportedSchema(value: ActivityPackSchema, label: string, depth = 0): void {
  if (depth > 64) failTest(`${label} schema nesting exceeds 64`);
  const record = asRecord(value, `${label} schema`);
  const supported = new Set([
    "type", "const", "enum", "minimum", "maximum", "minLength", "maxLength", "minItems",
    "maxItems", "properties", "required", "additionalProperties", "items", "description",
  ]);
  for (const keyword of Object.keys(record)) {
    if (!supported.has(keyword)) failTest(`${label} uses unsupported schema keyword ${keyword}`);
  }
  if (record.description !== undefined && typeof record.description !== "string") {
    failTest(`${label}.description must be a string`);
  }
  const type = record.type;
  if (type !== undefined && (typeof type !== "string" || !SCHEMA_TYPES.has(type))) {
    failTest(`${label}.type is unsupported`);
  }
  if (record.enum !== undefined) {
    const values = asArray(record.enum, `${label}.enum`);
    if (values.length === 0) failTest(`${label}.enum must not be empty`);
    for (const [index, candidate] of values.entries()) {
      if (values.slice(0, index).some((previous) => canonicalJson(previous) === canonicalJson(candidate))) {
        failTest(`${label}.enum values must be unique`);
      }
    }
  }
  const minimum = schemaInteger(record, "minimum", label);
  const maximum = schemaInteger(record, "maximum", label);
  if ((minimum !== undefined || maximum !== undefined) && type !== "integer") {
    failTest(`${label}.minimum and maximum require type integer`);
  }
  if (minimum !== undefined && maximum !== undefined && minimum > maximum) {
    failTest(`${label}.minimum must not exceed maximum`);
  }
  const minLength = schemaNonnegativeInteger(record, "minLength", label);
  const maxLength = schemaNonnegativeInteger(record, "maxLength", label);
  if ((minLength !== undefined || maxLength !== undefined) && type !== "string") {
    failTest(`${label}.minLength and maxLength require type string`);
  }
  if (minLength !== undefined && maxLength !== undefined && minLength > maxLength) {
    failTest(`${label}.minLength must not exceed maxLength`);
  }
  const minItems = schemaNonnegativeInteger(record, "minItems", label);
  const maxItems = schemaNonnegativeInteger(record, "maxItems", label);
  if ((minItems !== undefined || maxItems !== undefined || record.items !== undefined) && type !== "array") {
    failTest(`${label} item constraints require type array`);
  }
  if (minItems !== undefined && maxItems !== undefined && minItems > maxItems) {
    failTest(`${label}.minItems must not exceed maxItems`);
  }
  if (record.items !== undefined) {
    assertSupportedSchema(asRecord(record.items, `${label}.items`), `${label}.items`, depth + 1);
  }
  const hasObjectKeywords = record.properties !== undefined || record.required !== undefined ||
    record.additionalProperties !== undefined;
  if (hasObjectKeywords && type !== "object") {
    failTest(`${label} object constraints require type object`);
  }
  const properties = record.properties === undefined
    ? undefined
    : asRecord(record.properties, `${label}.properties`);
  for (const [name, nested] of Object.entries(properties ?? {})) {
    if (name.length === 0) failTest(`${label}.properties must have non-empty names`);
    assertSupportedSchema(asRecord(nested, `${label}.properties.${name}`), `${label}.properties.${name}`, depth + 1);
  }
  if (record.required !== undefined) {
    const required = asArray(record.required, `${label}.required`);
    const uniqueRequired = new Set<string>();
    for (const field of required) {
      if (typeof field !== "string" || field.length === 0) {
        failTest(`${label}.required must contain non-empty strings`);
      }
      if (uniqueRequired.has(field)) failTest(`${label}.required entries must be unique`);
      uniqueRequired.add(field);
      if (properties?.[field] === undefined) {
        failTest(`${label}.required ${field} has no property schema`);
      }
    }
  }
  if (record.additionalProperties !== undefined && typeof record.additionalProperties !== "boolean") {
    failTest(`${label}.additionalProperties must be boolean`);
  }
}

function matchesSchema(value: JsonValue | undefined, schema: JsonValue): boolean {
  if (value === undefined) return false;
  const record = asRecord(schema, "schema");
  if (record.const !== undefined && canonicalJson(record.const) !== canonicalJson(value)) {
    return false;
  }
  if (Array.isArray(record.enum) && !record.enum.some(
    (candidate) => canonicalJson(candidate) === canonicalJson(value),
  )) return false;
  if (record.type === "object") {
    if (!isRecord(value)) return false;
    const properties = record.properties === undefined
      ? {}
      : asRecord(record.properties, "schema.properties");
    const required = record.required === undefined ? [] : asArray(record.required, "schema.required");
    if (required.some((field) => typeof field !== "string" || value[field] === undefined)) return false;
    if (
      record.additionalProperties === false &&
      Object.keys(value).some((field) => properties[field] === undefined)
    ) return false;
    return Object.entries(properties).every(
      ([field, propertySchema]) => value[field] === undefined || matchesSchema(value[field], propertySchema),
    );
  }
  if (record.type === "array") return Array.isArray(value) &&
    (typeof record.minItems !== "number" || value.length >= record.minItems) &&
    (typeof record.maxItems !== "number" || value.length <= record.maxItems) &&
    (record.items === undefined || value.every((item) => matchesSchema(item, record.items!)));
  if (record.type === "string") {
    if (typeof value !== "string") return false;
    const bytes = new TextEncoder().encode(value).length;
    return (typeof record.maxLength !== "number" || bytes <= record.maxLength) &&
      (typeof record.minLength !== "number" || bytes >= record.minLength);
  }
  if (record.type === "integer") return typeof value === "number" && Number.isSafeInteger(value) &&
    (typeof record.minimum !== "number" || value >= record.minimum) &&
    (typeof record.maximum !== "number" || value <= record.maximum);
  if (record.type === "boolean") return typeof value === "boolean";
  if (record.type === "null") return value === null;
  return record.type === undefined;
}

const SCHEMA_TYPES = new Set(["null", "boolean", "integer", "string", "array", "object"]);

function schemaInteger(record: Record<string, JsonValue>, keyword: string, label: string): number | undefined {
  const value = record[keyword];
  if (value === undefined) return undefined;
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    failTest(`${label}.${keyword} must be an integer`);
  }
  return value;
}

function schemaNonnegativeInteger(
  record: Record<string, JsonValue>,
  keyword: string,
  label: string,
): number | undefined {
  const value = schemaInteger(record, keyword, label);
  if (value !== undefined && value < 0) failTest(`${label}.${keyword} must be a non-negative integer`);
  return value;
}

function assertMatchesSchema(value: JsonValue | undefined, schema: JsonValue, label: string): void {
  if (!matchesSchema(value, schema)) failTest(`${label} does not satisfy its declared schema`);
}

function actionDefinition(
  descriptor: NormalizedDescriptor,
  action: Record<string, JsonValue>,
  label: string,
): ActionEvidence {
  const actionType = stringField(action, "action_type", label);
  const definition = descriptor.actions.find((candidate) => candidate.actionType === actionType);
  if (definition === undefined) failTest(`${label} ${actionType} has no declared schema`);
  return definition;
}

function validateRoster(participants: readonly Record<string, JsonValue>[], roles: readonly RoleEvidence[]): void {
  const violation = rosterViolation(participants, roles);
  if (violation !== undefined) failTest(violation);
}

function validateRosterCases(
  fixture: Record<string, JsonValue>,
  roles: readonly RoleEvidence[],
): readonly (readonly Record<string, JsonValue>[])[] {
  const validCases = fixture.rosterCases === undefined
    ? []
    : asArray(fixture.rosterCases, "goldenFixture.rosterCases");
  const rosters = validCases.map((value, index) => {
    const roster = asRecord(value, `goldenFixture.rosterCases[${index}]`);
    const participants = asArray(roster.participants, `goldenFixture.rosterCases[${index}].participants`)
      .map((participant, participantIndex) => asRecord(
        participant,
        `goldenFixture.rosterCases[${index}].participants[${participantIndex}]`,
      ));
    validateRoster(participants, roles);
    return participants;
  });

  const invalidCases = fixture.invalidRosterCases === undefined
    ? []
    : asArray(fixture.invalidRosterCases, "goldenFixture.invalidRosterCases");
  for (const [index, value] of invalidCases.entries()) {
    const roster = asRecord(value, `goldenFixture.invalidRosterCases[${index}]`);
    const participants = asArray(roster.participants, `goldenFixture.invalidRosterCases[${index}].participants`)
      .map((participant, participantIndex) => asRecord(
        participant,
        `goldenFixture.invalidRosterCases[${index}].participants[${participantIndex}]`,
      ));
    if (rosterViolation(participants, roles) === undefined) {
      failTest(`goldenFixture.invalidRosterCases[${index}] unexpectedly satisfies role cardinality`);
    }
  }
  return rosters;
}

function exerciseRosterCases(
  pack: ActivityPackDefinition,
  descriptor: NormalizedDescriptor,
  configuration: JsonValue | undefined,
  createdAt: JsonValue | undefined,
  rosters: readonly (readonly Record<string, JsonValue>[])[],
  externalInputs: readonly JsonValue[],
  accepted: readonly JsonValue[],
): void {
  for (const [index, roster] of rosters.entries()) {
    const core = coreWith(roster);
    const initialized = asRecord(
      pack.initialize(initializationRequest(configuration, core, createdAt)),
      `roster case ${index + 1} initialize output`,
    );
    let state = initialized.initial_activity_state;
    assertMatchesSchema(state, descriptor.stateSchema, `roster case ${index + 1} Activity State`);
    let timers = applyTimerRequests(
      {},
      asArray(initialized.timer_requests, `roster case ${index + 1} initialize timer requests`),
    );
    let sequence = 1;
    for (const inputValue of externalInputs) {
      const input = asRecord(inputValue, `roster case ${index + 1} ExternalInput`);
      const output = asRecord(
        pack.reduce(reduceRequest(state, core, timers, externalInputStimulus(input), sequence)),
        `roster case ${index + 1} ExternalInput output`,
      );
      if (output.activity_disposition_type !== "apply") {
        failTest(`roster case ${index + 1} ExternalInput did not apply`);
      }
      state = output.next_activity_state;
      timers = applyTimerRequests(
        timers,
        asArray(output.timer_requests, `roster case ${index + 1} ExternalInput timer requests`),
      );
      sequence += 1;
    }
    const rosterMemberIds = new Set(
      roster.map((participant) => stringField(participant, "member_id", "roster participant")),
    );
    const actionValue = accepted.find((value) => {
      const action = asRecord(value, `roster case ${index + 1} Action`);
      return rosterMemberIds.has(stringField(action, "member_id", "roster Action"));
    });
    if (actionValue === undefined) failTest(`roster case ${index + 1} has no relevant accepted Action`);
    const action = asRecord(actionValue, `roster case ${index + 1} Action`);
    const definition = actionDefinition(descriptor, action, `roster case ${index + 1} Action`);
    assertMatchesSchema(
      action.canonical_payload,
      definition.payloadSchema,
      `roster case ${index + 1} Action ${definition.actionType}`,
    );
    const output = asRecord(
      pack.reduce(reduceRequest(state, core, timers, actionStimulus(action), sequence)),
      `roster case ${index + 1} Action output`,
    );
    if (output.activity_disposition_type !== "apply") {
      failTest(`roster case ${index + 1} relevant Action did not apply`);
    }
    const participant = roster[0]!;
    const memberId = stringField(participant, "member_id", "roster participant");
    const view = pack.view(viewRequest(output.next_activity_state, core, {
      memberId,
      viewerType: "participant",
    }));
    const projection = asRecord(
      asRecord(view, `roster case ${index + 1} view`).projection,
      `roster case ${index + 1} projection`,
    );
    assertMatchesSchema(projection, descriptor.projectionSchemas.participant, `roster case ${index + 1} projection`);
  }
}

function rosterViolation(
  participants: readonly Record<string, JsonValue>[],
  roles: readonly RoleEvidence[],
): string | undefined {
  const counts = new Map(roles.map((role) => [role.role, 0]));
  for (const participant of participants) {
    if ((participant.access_mode ?? "participant") !== "participant") continue;
    const role = participant.role;
    if (typeof role !== "string" || !counts.has(role)) {
      return "participant fixture uses an undeclared Role";
    }
    counts.set(role, counts.get(role)! + 1);
  }
  for (const role of roles) {
    const count = counts.get(role.role)!;
    if (count < role.minimum || count > role.maximum) {
      return `participant fixture violates ${role.role} cardinality ${role.minimum}..${role.maximum}`;
    }
  }
  return undefined;
}

function initializationRequest(
  configuration: JsonValue | undefined,
  core: JsonValue,
  createdAt: JsonValue | undefined,
): Record<string, JsonValue> {
  return {
    configuration: configuration!,
    created_at: typeof createdAt === "string" ? createdAt : "2026-08-30T12:00:00Z",
    deterministic_context: {
      next_room_sequence: 0,
      pack_digest: `blake3:${"0".repeat(64)}`,
      room_seed: "00000000000000000000000000000000",
    },
    initial_core_state: core,
    pack_digest: `blake3:${"0".repeat(64)}`,
    room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
    room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
  };
}

function coreWith(participants: readonly Record<string, JsonValue>[]): JsonValue {
  return {
    memberships: Object.fromEntries(participants.map((participant) => {
      const memberId = stringField(participant, "member_id", "participant");
      return [memberId, {
        access_mode: participant.access_mode ?? "participant",
        member_id: memberId,
        principal_id: participant.principal_id ?? `principal-${memberId}`,
        principal_kind: participant.principal_kind ?? "agent",
        role: participant.role ?? null,
        standing: participant.standing ?? "enabled",
      }];
    })),
    room_status: "active",
  };
}

function fixtureParticipants(fixture: Record<string, JsonValue>): Record<string, JsonValue>[] {
  if (fixture.participants !== undefined) {
    return asArray(fixture.participants, "goldenFixture.participants").map((item, index) =>
      asRecord(item, `goldenFixture.participants[${index}]`));
  }
  return [asRecord(fixture.buyer, "goldenFixture.buyer"), asRecord(fixture.seller, "goldenFixture.seller")];
}

function audienceFixtures(
  privacy: Record<string, JsonValue>,
  participants: readonly Record<string, JsonValue>[],
): AudienceFixture[] {
  if (privacy.audiences !== undefined) return asArray(
    privacy.audiences,
    "privacyFixture.audiences",
  ).map((value, index) => {
    const audience = asRecord(value, `privacyFixture.audiences[${index}]`);
    const viewerType = stringField(audience, "viewer_type", `privacyFixture.audiences[${index}]`);
    if (!isViewerType(viewerType)) failTest(`privacyFixture.audiences[${index}] has invalid viewer_type`);
    return {
      memberId: stringField(audience, "member_id", `privacyFixture.audiences[${index}]`),
      viewerType,
    };
  });
  return [
    ...participants
      .filter((participant) => (participant.access_mode ?? "participant") === "participant")
      .slice(0, 2)
      .map((participant) => ({
        memberId: stringField(participant, "member_id", "participant"),
        viewerType: "participant" as const,
      })),
    { memberId: "spectator-member", viewerType: "public" },
  ];
}

function privateAudienceFixtures(
  privacy: Record<string, JsonValue>,
  audiences: readonly AudienceFixture[],
): AudienceFixture[] {
  const ids = privacy.private_viewers === undefined
    ? audiences.filter((audience) => audience.viewerType === "participant").slice(0, 2)
      .map((audience) => audience.memberId)
    : asArray(privacy.private_viewers, "privacyFixture.private_viewers");
  if (ids.length < 2) failTest("privacyFixture must declare at least two participant audiences");
  return ids.map((id, index) => {
    if (typeof id !== "string") failTest(`privacyFixture.private_viewers[${index}] must be a member ID`);
    const audience = audiences.find(
      (candidate) => candidate.memberId === id && candidate.viewerType === "participant",
    );
    if (audience === undefined) failTest(`privacyFixture.private_viewers[${index}] is not a participant audience`);
    return audience;
  });
}

function legacyPrivacyMutations(
  state: JsonValue | undefined,
  participants: readonly Record<string, JsonValue>[],
): JsonValue[] {
  if (!isRecord(state) || !Object.hasOwn(state, "buyerCeiling")) return [];
  const seller = participants.find((participant) => participant.role === "seller");
  return seller === undefined
    ? []
    : [{
        activity_state: { ...state, buyerCeiling: 999 },
        hidden_from: [stringField(seller, "member_id", "seller")],
      }];
}

function reduceRequest(
  state: JsonValue | undefined,
  core: JsonValue,
  scheduledTimers: Record<string, JsonValue>,
  stimulus: JsonValue,
  sequence: number,
): Record<string, JsonValue> {
  return {
    core_before: core,
    deterministic_context: {
      next_room_sequence: sequence,
      pack_digest: `blake3:${"0".repeat(64)}`,
      room_seed: "00000000000000000000000000000000",
    },
    next_room_seq: sequence,
    prior_activity_state: state!,
    proposed_core_after: core,
    recorded_stimulus: stimulus,
    scheduled_timers: scheduledTimers,
  };
}

function applyTimerRequests(
  previous: Record<string, JsonValue>,
  requests: readonly JsonValue[],
): Record<string, JsonValue> {
  const next = { ...previous };
  for (const [index, item] of requests.entries()) {
    const request = asRecord(item, `timer requests[${index}]`);
    const timerId = stringField(request, "timer_id", `timer requests[${index}]`);
    const current = next[timerId] === undefined
      ? undefined
      : asRecord(next[timerId], `scheduled timer ${timerId}`);
    if (request.timer_request_type === "schedule_next") {
      if (current !== undefined) failTest(`timer ${timerId} was scheduled twice`);
      next[timerId] = {
        canonical_payload: request.canonical_payload!,
        generation: 1,
        scheduled_for: String(request.due),
        timer_id: timerId,
      };
    } else if (request.timer_request_type === "cancel_current") {
      if (current === undefined || current.generation !== request.expected_generation) {
        failTest(`timer ${timerId} cancellation generation changed`);
      }
      delete next[timerId];
    } else if (request.timer_request_type === "reschedule_current") {
      if (current === undefined || current.generation !== request.expected_generation) {
        failTest(`timer ${timerId} reschedule generation changed`);
      }
      next[timerId] = {
        canonical_payload: request.new_canonical_payload!,
        generation: Number(current.generation) + 1,
        scheduled_for: String(request.new_due),
        timer_id: timerId,
      };
    } else failTest(`unknown timer request ${String(request.timer_request_type)}`);
  }
  return next;
}

function actionStimulus(action: Record<string, JsonValue>): JsonValue {
  return {
    action_id: "01H00000000000000000000000",
    action_type: stringField(action, "action_type", "Action fixture"),
    admitted_at: typeof action.admitted_at === "string" ? action.admitted_at : "2026-01-01T00:00:00Z",
    canonical_payload: action.canonical_payload!,
    exact_basis_head: emptyHead(),
    member_id: stringField(action, "member_id", "Action fixture"),
    payload_schema_digest: `blake3:${"0".repeat(64)}`,
    stimulus_type: "participant_action",
  };
}

function externalInputStimulus(input: Record<string, JsonValue>): JsonValue {
  return {
    canonical_payload: input.canonical_payload!,
    immutable_resource_references: input.immutable_resource_references ?? [],
    input_id: stringField(input, "input_id", "ExternalInput fixture"),
    input_type: stringField(input, "input_type", "ExternalInput fixture"),
    recorded_at: typeof input.recorded_at === "string" ? input.recorded_at : "2026-01-01T00:00:00Z",
    source_id: stringField(input, "source_id", "ExternalInput fixture"),
    stimulus_type: "external_input",
  };
}

function viewRequest(
  state: JsonValue | undefined,
  core: JsonValue,
  audience: AudienceFixture,
): Record<string, JsonValue> {
  return { activity_state: state!, complete_head: emptyHead(), core, viewer: viewerFor(audience) };
}

function viewerFor(audience: AudienceFixture): JsonValue {
  return { member_id: audience.memberId, viewer_type: audience.viewerType };
}
function audienceKey(audience: AudienceFixture): string {
  return `${audience.viewerType}:${audience.memberId}`;
}
function isViewerType(value: string): value is AudienceFixture["viewerType"] {
  return ["participant", "public", "operator", "historical", "final_reveal"].includes(value);
}
function isActivityPackViewerClass(value: string): value is ActivityPackViewerClass {
  return (VIEWER_CLASSES as readonly string[]).includes(value);
}
function emptyHead(): JsonValue {
  const zero = `blake3:${"0".repeat(64)}`;
  return { activity_state_hash: zero, authoritative_state_hash: zero, core_state_hash: zero, room_seq: 0 };
}
function assertForbidden(value: JsonValue, names: readonly JsonValue[]): void {
  const text = canonicalJson(value);
  for (const name of names) {
    if (typeof name === "string" && text.includes(name)) {
      failTest(`public view leaks forbidden field ${name}`);
    }
  }
}
function unique(values: readonly string[]): boolean {
  return values.every((value) => value.length > 0) && new Set(values).size === values.length;
}
function stringField(record: Record<string, JsonValue>, field: string, label: string): string {
  if (typeof record[field] !== "string" || record[field].length === 0) {
    failTest(`${label}.${field} must be a non-empty string`);
  }
  return record[field] as string;
}
function assertCleanDeclaredRejection(
  value: JsonValue,
  declaredCodes: readonly string[],
  label: string,
): void {
  const rejection = asRecord(value, label);
  if (
    Object.keys(rejection).sort().join(",") !==
      "activity_disposition_type,bounded_safe_details,declared_code" ||
    rejection.activity_disposition_type !== "reject" ||
    typeof rejection.declared_code !== "string" ||
    !declaredCodes.includes(rejection.declared_code)
  ) {
    failTest(`${label} must be one schema-valid declared rejection`);
  }
  assertMatchesSchema(
    rejection.bounded_safe_details,
    GENERATED_REJECTION_SCHEMA,
    `${label}.bounded_safe_details`,
  );
  if (canonicalBytes(rejection.bounded_safe_details!).length > GENERATED_PACK_LIMITS.maximumObservationBytes) {
    failTest(`${label}.bounded_safe_details exceeds the generated observation byte limit`);
  }
  assertGeneratedValueBounds(rejection.bounded_safe_details!, 1, `${label}.bounded_safe_details`);
}
function assertGeneratedValueBounds(value: JsonValue, depth: number, label: string): void {
  if (depth > GENERATED_PACK_LIMITS.maximumNesting) {
    failTest(`${label} exceeds the generated nesting limit`);
  }
  if (typeof value === "string") {
    if (new TextEncoder().encode(value).length > GENERATED_PACK_LIMITS.maximumTextBytes) {
      failTest(`${label} exceeds the generated text byte limit`);
    }
    return;
  }
  if (Array.isArray(value)) {
    if (value.length > GENERATED_PACK_LIMITS.maximumCollectionItems) {
      failTest(`${label} exceeds the generated collection limit`);
    }
    for (const item of value) assertGeneratedValueBounds(item, depth + 1, label);
    return;
  }
  if (isRecord(value)) {
    const entries = Object.entries(value);
    if (entries.length > GENERATED_PACK_LIMITS.maximumCollectionItems) {
      failTest(`${label} exceeds the generated collection limit`);
    }
    for (const [key, child] of entries) {
      if (new TextEncoder().encode(key).length > GENERATED_PACK_LIMITS.maximumTextBytes) {
        failTest(`${label} contains a key that exceeds the generated text byte limit`);
      }
      assertGeneratedValueBounds(child, depth + 1, label);
    }
  }
}
function isRecord(value: JsonValue | undefined): value is Record<string, JsonValue> {
  return value !== null && value !== undefined && !Array.isArray(value) && typeof value === "object";
}
function asRecord(value: JsonValue | undefined, label: string): Record<string, JsonValue> {
  if (!isRecord(value)) failTest(`${label} must be an object`);
  return value;
}
function asArray(value: JsonValue | undefined, label: string): JsonValue[] {
  if (!Array.isArray(value)) failTest(`${label} must be an array`);
  return value;
}
function deterministic(label: string, invoke: () => JsonValue): JsonValue {
  const first = invoke();
  const second = invoke();
  if (canonicalJson(first) !== canonicalJson(second)) {
    failTest(`${label} returned different canonical results for identical input`);
  }
  return first;
}
function failTest(detail: string): never {
  throw new PackCliError(diagnostic(
    "WSP-CONFORMANCE-001",
    "Activity Pack behavioral conformance failed",
    { detail, hint: "Run `worldstream-pack test` after fixing the named callback or fixture." },
  ));
}
async function freshImport<T>(path: string): Promise<T> {
  const url = pathToFileURL(path);
  url.searchParams.set("worldstream", `${Date.now()}-${Math.random()}`);
  return (await import(url.href)) as T;
}
async function runCustomTest(path: string, cwd: string): Promise<void> {
  await new Promise<void>((resolve, reject) => {
    const childEnvironment = { ...process.env };
    delete childEnvironment.NODE_TEST_CONTEXT;
    const child = spawn(process.execPath, ["--test", path], {
      cwd,
      env: childEnvironment,
      stdio: "inherit",
    });
    child.once("error", reject);
    child.once("exit", (code) => code === 0
      ? resolve()
      : reject(new Error(`custom tests exited with status ${code ?? "signal"}`)));
  }).catch((error: unknown) => {
    throw new PackCliError(diagnostic(
      "WSP-CONFORMANCE-002",
      "Custom Activity Pack tests failed",
      { file: path, detail: error instanceof Error ? error.message : String(error) },
    ));
  });
}
