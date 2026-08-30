import { spawn } from "node:child_process";
import { pathToFileURL } from "node:url";

import { canonicalBytes, canonicalJson, taggedBlake3, type JsonValue } from "./canonical.js";
import { PackCliError, diagnostic } from "./diagnostics.js";
import type { ResolvedPackProject } from "./project.js";
import { emitForExecution } from "./compiler.js";

interface PackDefinition {
  readonly descriptor: {
    readonly packId: string;
    readonly name: string;
    readonly version: string;
    readonly roles: readonly string[];
    readonly actions: readonly string[];
    readonly rejectionCodes: readonly string[];
    readonly events: readonly string[];
    readonly attentionReasons: readonly string[];
  };
  initialize(input: Record<string, JsonValue>): JsonValue;
  reduce(input: Record<string, JsonValue>): JsonValue;
  view(input: Record<string, JsonValue>): JsonValue;
  observe(input: Record<string, JsonValue>): JsonValue;
}

export interface BehavioralEvidence {
  readonly packId: string;
  readonly name: string;
  readonly version: string;
  readonly operations: readonly string[];
  readonly roles: readonly string[];
  readonly actions: readonly string[];
  readonly rejectionCodes: readonly string[];
  readonly events: readonly string[];
  readonly attentionReasons: readonly string[];
  readonly goldenFixture: JsonValue;
  readonly acceptedActions: number;
  readonly declaredRejections: number;
  readonly deterministicRestart: boolean;
  readonly privateViewsDistinct: boolean;
  readonly crossRoleMutationHidden: boolean;
  readonly transcriptDigest: string;
  readonly retainedTranscriptDigest: string;
  readonly transcript: readonly JsonValue[];
}

export async function runBehavioralConformance(
  project: ResolvedPackProject,
): Promise<BehavioralEvidence> {
  const emitted = await emitForExecution(project, "conformance");
  const packModule = await freshImport<{ readonly default: PackDefinition }>(emitted.entrypoint);
  const goldenModule = await freshImport<{ readonly goldenFixture: JsonValue }>(emitted.golden);
  const privacyModule = await freshImport<{ readonly privacyFixture: JsonValue }>(emitted.privacy);
  const pack = packModule.default;
  validateDefinition(pack, project.entrypoint);
  const fixture = asRecord(goldenModule.goldenFixture, "goldenFixture");
  const privacy = asRecord(privacyModule.privacyFixture, "privacyFixture");
  const configuration = asRecord(fixture.configuration, "goldenFixture.configuration");
  const buyer = asRecord(fixture.buyer, "goldenFixture.buyer");
  const seller = asRecord(fixture.seller, "goldenFixture.seller");
  const participants = fixtureParticipants(fixture, buyer, seller);
  const accepted = asArray(fixture.accepted, "goldenFixture.accepted");
  const rejected = asRecord(fixture.rejected, "goldenFixture.rejected");
  const core = coreWith(participants);
  const initRequest = {
    configuration,
    created_at:
      typeof fixture.created_at === "string"
        ? fixture.created_at
        : "2026-08-30T12:00:00Z",
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
  const firstInit = deterministic("initialize", () => pack.initialize(initRequest));
  const secondInit = pack.initialize(initRequest);
  const deterministicRestart = canonicalJson(firstInit) === canonicalJson(secondInit);
  if (!deterministicRestart) failTest("initialize changed across a simulated restart");
  let state = asRecord(asRecord(firstInit, "initialize output").initial_activity_state, "initial state");
  let scheduledTimers = applyTimerRequests(
    {},
    asArray(asRecord(firstInit, "initialize output").timer_requests, "initialize timer requests"),
  );
  const transcript: JsonValue[] = [firstInit];
  let acceptedActions = 0;

  const earlyRejection = deterministic("reduce rejection", () =>
    pack.reduce(reduceRequest(state, core, scheduledTimers, rejected, 1)),
  );
  if (asRecord(earlyRejection, "declared rejection").activity_disposition_type !== "reject") {
    failTest("the declared-rejection fixture was not rejected");
  }
  transcript.push(earlyRejection);

  for (const [index, actionValue] of accepted.entries()) {
    const action = asRecord(actionValue, `goldenFixture.accepted[${index}]`);
    const output = deterministic(`reduce accepted action ${index + 1}`, () =>
      pack.reduce(reduceRequest(state, core, scheduledTimers, action, index + 1)),
    );
    const disposition = asRecord(output, "reduce output");
    if (disposition.activity_disposition_type !== "apply") {
      failTest(`accepted action ${index + 1} did not apply`);
    }
    state = asRecord(disposition.next_activity_state, "next_activity_state");
    scheduledTimers = applyTimerRequests(
      scheduledTimers,
      asArray(disposition.timer_requests, "reduce timer requests"),
    );
    acceptedActions += 1;
    transcript.push(output);
  }

  const buyerView = deterministic("buyer view", () =>
    pack.view(viewRequest(state, core, buyer)),
  );
  const sellerView = deterministic("seller view", () =>
    pack.view(viewRequest(state, core, seller)),
  );
  const publicView = deterministic("public view", () =>
    pack.view({
      activity_state: state,
      complete_head: emptyHead(),
      core,
      viewer: { member_id: "spectator-member", viewer_type: "public" },
    }),
  );
  assertForbidden(publicView, asArray(privacy.forbiddenPublic, "privacyFixture.forbiddenPublic"));
  const buyerProjection = asRecord(asRecord(buyerView, "buyer view").projection, "buyer projection");
  const sellerProjection = asRecord(asRecord(sellerView, "seller view").projection, "seller projection");
  const privateViewsDistinct = canonicalJson(buyerProjection) !== canonicalJson(sellerProjection);
  if (!privateViewsDistinct) failTest("buyer and seller private views are not distinct");

  const buyerMutated = { ...state, buyerCeiling: 999 };
  const sellerAfterBuyerMutation = pack.view(viewRequest(buyerMutated, core, seller));
  const sellerBefore = canonicalJson(sellerView);
  const sellerAfter = canonicalJson(sellerAfterBuyerMutation);
  const crossRoleMutationHidden = sellerBefore === sellerAfter;
  if (!crossRoleMutationHidden) {
    failTest("mutating buyer-only state changed the seller view");
  }

  const observeRequest = {
    activity_after: state,
    activity_before: asRecord(asRecord(firstInit, "initialize output").initial_activity_state, "initial state"),
    after_view: asRecord(buyerView, "buyer view"),
    core_after: core,
    core_before: core,
    ordered_domain_events: [],
    recorded_stimulus: actionStimulus(asRecord(accepted.at(-1), "last accepted action")),
    viewer: participantViewer(buyer),
  } satisfies Record<string, JsonValue>;
  const observation = deterministic("observe", () => pack.observe(observeRequest));
  const observationRecord = asRecord(observation, "observation");
  if (!new Set(["unchanged", "reuse_after_view"]).has(String(observationRecord.action_offers))) {
    failTest("observe must use unchanged or reuse_after_view for Action Offers");
  }
  transcript.push(buyerView, sellerView, publicView, observation);

  if (emitted.customTest) await runCustomTest(emitted.customTest, project.root);
  const transcriptDigest = taggedBlake3(canonicalBytes(transcript));
  return {
    packId: pack.descriptor.packId,
    name: pack.descriptor.name,
    version: pack.descriptor.version,
    operations: ["descriptor", "initialize", "reduce", "view", "observe"],
    roles: [...pack.descriptor.roles],
    actions: [...pack.descriptor.actions],
    rejectionCodes: [...pack.descriptor.rejectionCodes],
    events: [...pack.descriptor.events],
    attentionReasons: [...pack.descriptor.attentionReasons],
    goldenFixture: fixture,
    acceptedActions,
    declaredRejections: 1,
    deterministicRestart,
    privateViewsDistinct,
    crossRoleMutationHidden,
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
    throw new PackCliError(
      diagnostic(
        "WSP-GOLDEN-004",
        "expected_transcript_digest must be an exact tagged BLAKE3 identity",
        {
          detail:
            "Use the authoritative digest returned by the production WorldStream Core golden prover.",
        },
      ),
    );
  }
  return authored;
}

function validateDefinition(pack: PackDefinition, file: string): void {
  if (!pack || typeof pack !== "object") {
    throw new PackCliError(
      diagnostic("WSP-CONTRACT-001", "Entrypoint must default-export one ActivityPackDefinition", {
        file,
      }),
    );
  }
  const callbacks = ["initialize", "reduce", "view", "observe"] as const;
  if (!pack.descriptor || callbacks.some((name) => typeof pack[name] !== "function")) {
    throw new PackCliError(
      diagnostic("WSP-CONTRACT-002", "Activity Pack definition is incomplete", {
        file,
        detail: "Expected descriptor plus initialize, reduce, view, and observe callbacks.",
      }),
    );
  }
  const { descriptor } = pack;
  if (
    !/^[a-z0-9]+(?:[.-][a-z0-9]+)+$/u.test(descriptor.packId) ||
    descriptor.name.length === 0 ||
    descriptor.version.length === 0 ||
    descriptor.roles.length < 2 ||
    descriptor.actions.length === 0 ||
    descriptor.rejectionCodes.length === 0
  ) {
    throw new PackCliError(
      diagnostic("WSP-CONTRACT-003", "Activity Pack descriptor draft is invalid", {
        file,
        detail: "Use a stable dotted pack ID, two or more Roles, Actions, and declared rejection codes.",
      }),
    );
  }
  for (const values of [
    descriptor.roles,
    descriptor.actions,
    descriptor.rejectionCodes,
    descriptor.events,
    descriptor.attentionReasons,
  ]) {
    if (new Set(values).size !== values.length || values.some((value) => value.length === 0)) {
      failTest("descriptor declarations must be unique and non-empty");
    }
  }
}

function deterministic(label: string, invoke: () => JsonValue): JsonValue {
  const first = invoke();
  const second = invoke();
  if (canonicalJson(first) !== canonicalJson(second)) {
    failTest(`${label} returned different canonical results for identical input`);
  }
  return first;
}

function coreWith(participants: readonly Record<string, JsonValue>[]): JsonValue {
  const memberships: Record<string, JsonValue> = {};
  for (const participant of participants) {
    const memberId = String(participant.member_id);
    memberships[memberId] = membership(memberId, participant);
  }
  return {
    memberships,
    room_status: "active",
  };
}

function membership(memberId: string, participant: Record<string, JsonValue>): JsonValue {
  const accessMode = typeof participant.access_mode === "string"
    ? participant.access_mode
    : "participant";
  return {
    access_mode: accessMode,
    member_id: memberId,
    principal_id:
      typeof participant.principal_id === "string"
        ? participant.principal_id
        : `principal-${memberId}`,
    principal_kind:
      typeof participant.principal_kind === "string" ? participant.principal_kind : "agent",
    role: typeof participant.role === "string" ? participant.role : null,
    standing: typeof participant.standing === "string" ? participant.standing : "enabled",
  };
}

function fixtureParticipants(
  fixture: Record<string, JsonValue>,
  buyer: Record<string, JsonValue>,
  seller: Record<string, JsonValue>,
): Record<string, JsonValue>[] {
  if (fixture.participants === undefined) return [buyer, seller];
  return asArray(fixture.participants, "goldenFixture.participants").map((item, index) =>
    asRecord(item, `goldenFixture.participants[${index}]`),
  );
}

function reduceRequest(
  state: Record<string, JsonValue>,
  core: JsonValue,
  scheduledTimers: Record<string, JsonValue>,
  action: Record<string, JsonValue>,
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
    prior_activity_state: state,
    proposed_core_after: core,
    recorded_stimulus: actionStimulus(action),
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
    const timerId = String(request.timer_id);
    const kind = String(request.timer_request_type);
    const current = next[timerId] === undefined
      ? undefined
      : asRecord(next[timerId], `scheduled timer ${timerId}`);
    if (kind === "schedule_next") {
      if (current !== undefined) failTest(`timer ${timerId} was scheduled twice`);
      next[timerId] = {
        canonical_payload: request.canonical_payload!,
        generation: 1,
        scheduled_for: String(request.due),
        timer_id: timerId,
      };
    } else if (kind === "cancel_current") {
      if (current === undefined || current.generation !== request.expected_generation) {
        failTest(`timer ${timerId} cancellation generation changed`);
      }
      delete next[timerId];
    } else if (kind === "reschedule_current") {
      if (current === undefined || current.generation !== request.expected_generation) {
        failTest(`timer ${timerId} reschedule generation changed`);
      }
      next[timerId] = {
        canonical_payload: request.new_canonical_payload!,
        generation: Number(current.generation) + 1,
        scheduled_for: String(request.new_due),
        timer_id: timerId,
      };
    } else {
      failTest(`unknown timer request ${kind}`);
    }
  }
  return next;
}

function actionStimulus(action: Record<string, JsonValue>): JsonValue {
  return {
    action_id: "01H00000000000000000000000",
    action_type: String(action.action_type),
    admitted_at:
      typeof action.admitted_at === "string"
        ? action.admitted_at
        : "2026-01-01T00:00:00Z",
    canonical_payload: asRecord(action.canonical_payload, "action payload"),
    exact_basis_head: emptyHead(),
    member_id: String(action.member_id),
    payload_schema_digest: `blake3:${"0".repeat(64)}`,
    stimulus_type: "participant_action",
  };
}

function viewRequest(
  state: Record<string, JsonValue>,
  core: JsonValue,
  member: Record<string, JsonValue>,
): Record<string, JsonValue> {
  return {
    activity_state: state,
    complete_head: emptyHead(),
    core,
    viewer: participantViewer(member),
  };
}

function participantViewer(member: Record<string, JsonValue>): JsonValue {
  return { member_id: String(member.member_id), viewer_type: "participant" };
}

function emptyHead(): JsonValue {
  const zero = `blake3:${"0".repeat(64)}`;
  return {
    activity_state_hash: zero,
    authoritative_state_hash: zero,
    core_state_hash: zero,
    room_seq: 0,
  };
}

function assertForbidden(value: JsonValue, names: readonly JsonValue[]): void {
  const text = canonicalJson(value);
  for (const name of names) {
    if (typeof name === "string" && text.includes(name)) {
      failTest(`public view leaks forbidden field ${name}`);
    }
  }
}

function asRecord(value: JsonValue | undefined, label: string): Record<string, JsonValue> {
  if (value === null || value === undefined || Array.isArray(value) || typeof value !== "object") {
    failTest(`${label} must be an object`);
  }
  return value as Record<string, JsonValue>;
}

function asArray(value: JsonValue | undefined, label: string): JsonValue[] {
  if (!Array.isArray(value)) failTest(`${label} must be an array`);
  return value;
}

function failTest(detail: string): never {
  throw new PackCliError(
    diagnostic("WSP-CONFORMANCE-001", "Activity Pack behavioral conformance failed", {
      detail,
      hint: "Run `worldstream-pack test` after fixing the named callback or fixture.",
    }),
  );
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
    child.once("exit", (code) => {
      if (code === 0) resolve();
      else reject(new Error(`custom tests exited with status ${code ?? "signal"}`));
    });
  }).catch((error: unknown) => {
    throw new PackCliError(
      diagnostic("WSP-CONFORMANCE-002", "Custom Activity Pack tests failed", {
        file: path,
        detail: error instanceof Error ? error.message : String(error),
      }),
    );
  });
}
