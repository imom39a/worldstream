import { useEffect, useMemo, useRef, useState } from "react";

import {
  createActionSubmitRequest,
  createRequest,
  WebSocketWorldStreamTransport,
  type ActionSubmitMessage,
  type JsonValue,
  type ProtocolMessage,
  type BrowserTicketFetch,
  type WebSocketFactory,
  type WorldStreamTransport,
} from "./transport";
import type {
  ConsoleTransportState,
  DiscoveryStatus,
  FrameDeliveryState,
  HeistActionType,
  HeistFixture,
  HeistPhase,
  RolePresence,
  RoomHealthState,
  RuntimeStatus,
} from "./fixture";

export interface LiveSessionConfig {
  endpoint: string;
  clientName: string;
  clientVersion: string;
  roomId: string;
  memberId: string;
  bearer?: string;
  ticketUrl?: string;
  fetch?: BrowserTicketFetch;
  webSocketFactory?: WebSocketFactory;
  /** Recreates a bearer-owning transport after a stale-head resync. */
  transportFactory?: () => WorldStreamTransport;
}

export interface LiveReplayClient {
  fetch(roomId: string, atRoomSeq: number): Promise<JsonValue>;
}

export interface LiveSessionBootstrapTarget {
  __WORLDSTREAM_LIVE_SESSION__?: unknown;
}

const LIVE_CONFIG_VERSION = "worldstream.console.live.v1" as const;
const LIVE_CONFIG_BEARER = /^wsb1:[0-9a-f]{64}$/;
const LIVE_CONFIG_ID = /^[0-9A-HJKMNP-TV-Z]{26}$/;

/** Validate the one-shot in-memory bootstrap used by the real-browser smoke. */
export function readLiveSessionConfig(value: unknown): LiveSessionConfig | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
  const candidate = value as Record<string, unknown>;
  if (candidate.version !== LIVE_CONFIG_VERSION) return null;
  if (!isBoundedString(candidate.endpoint, 2048) || !/^wss?:\/\//.test(candidate.endpoint)) return null;
  if (!isBoundedString(candidate.clientName, 256) || !isBoundedString(candidate.clientVersion, 128)) return null;
  if (!isBoundedString(candidate.roomId, 26) || !LIVE_CONFIG_ID.test(candidate.roomId)) return null;
  if (!isBoundedString(candidate.memberId, 26) || !LIVE_CONFIG_ID.test(candidate.memberId)) return null;
  if (!isBoundedString(candidate.bearer, 69) || !LIVE_CONFIG_BEARER.test(candidate.bearer)) return null;
  if (candidate.ticketUrl !== undefined && (!isBoundedString(candidate.ticketUrl, 2048) || !/^https?:\/\//.test(candidate.ticketUrl))) return null;
  return {
    endpoint: candidate.endpoint,
    clientName: candidate.clientName,
    clientVersion: candidate.clientVersion,
    roomId: candidate.roomId,
    memberId: candidate.memberId,
    bearer: candidate.bearer,
    ticketUrl: candidate.ticketUrl,
  };
}

function isBoundedString(value: unknown, maximumBytes: number): value is string {
  return typeof value === "string" && value.length > 0 && new TextEncoder().encode(value).length <= maximumBytes;
}

/** Consume browser bootstrap material before React receives its config. */
export function consumeLiveSessionBootstrap(target: LiveSessionBootstrapTarget): {
  config: LiveSessionConfig | null;
  transport: WorldStreamTransport | null;
  replayClient: LiveReplayClient | null;
} {
  const config = readLiveSessionConfig(target.__WORLDSTREAM_LIVE_SESSION__);
  delete target.__WORLDSTREAM_LIVE_SESSION__;
  if (config === null) return { config: null, transport: null, replayClient: null };
  const bearer = config.bearer;
  const transportFactory = () => createLiveSessionTransport({ ...config, bearer });
  const transport = transportFactory();
  const replayClient = createLiveReplayClient(config.endpoint, bearer);
  const safeConfig: LiveSessionConfig = { ...config, bearer: undefined, transportFactory };
  return { config: safeConfig, transport, replayClient };
}

export type ActionLifecycleState = "idle" | "submitting" | "accepted" | "rejected" | "stale" | "resync-required";

export interface LiveActionState {
  state: ActionLifecycleState;
  actionId: string | null;
  requestId: string | null;
  basedOnRoomSeq: number | null;
  message: string | null;
}

export interface LiveSessionView {
  fixture: HeistFixture;
  status: "connecting" | "attached" | "live" | "error" | "disconnected";
  error: string | null;
  action: LiveActionState;
  submitAction: (input: { offerId: string; schemaDigest: string; actionType: string; payload: JsonValue; basedOnRoomSeq: number }) => Promise<void>;
  resync: () => Promise<void>;
}

interface SyncTracker {
  kind: "retained_frames" | "projection_reset";
  through: number;
  cursorExclusive: number | null;
  syncToken: string;
  receivedFrames: number[];
  resetInstalled: boolean;
  acknowledgementSent: boolean;
  acknowledged: boolean;
  observationAckThrough: number | null;
}

const LIVE_ID = "01J00000000000000000000000";
const IDLE_ACTION: LiveActionState = { state: "idle", actionId: null, requestId: null, basedOnRoomSeq: null, message: null };

export function initialLiveSessionView(fixture: HeistFixture): LiveSessionView {
  return makeInitialView(fixture);
}

/** Pure projection seam used by UI and deterministic session tests. */
export function projectLiveSessionMessage(current: LiveSessionView, message: ProtocolMessage, fixture: HeistFixture): LiveSessionView {
  return applyMessage(current, message, fixture);
}

export function projectLiveSessionMessageForSession(
  current: LiveSessionView,
  message: ProtocolMessage,
  fixture: HeistFixture,
  session: Pick<LiveSessionConfig, "roomId" | "memberId">,
): LiveSessionView {
  return applyMessage(current, message, fixture, session);
}

export function isCurrentActionOffer(
  fixture: HeistFixture,
  input: { offerId: string; schemaDigest: string; actionType: string },
): boolean {
  const offer = fixture.participant.offers.find((candidate) => candidate.id === input.offerId);
  return offer !== undefined && offer.actionType === input.actionType && offer.schemaDigest === input.schemaDigest;
}

export function latestObservationFrameToAck(receivedFrames: readonly number[], acknowledged: boolean): number | null {
  if (!acknowledged || receivedFrames.length === 0) return null;
  return Math.max(...receivedFrames);
}

/**
 * Install a replacement without allowing an adapter teardown failure to
 * strand the hook in a null-transport state.
 */
export function replaceLiveTransport(
  current: WorldStreamTransport,
  createReplacement: () => WorldStreamTransport,
): WorldStreamTransport {
  const replacement = createReplacement();
  try {
    current.close();
  } finally {
    return replacement;
  }
}

/** Keep the live-session transport construction small and independently testable. */
export function createLiveSessionTransport(config: LiveSessionConfig): WorldStreamTransport {
  return new WebSocketWorldStreamTransport({
    url: config.endpoint,
    clientName: config.clientName,
    clientVersion: config.clientVersion,
    bearer: config.bearer,
    ticketUrl: config.ticketUrl,
    fetch: config.fetch,
    webSocketFactory: config.webSocketFactory,
  });
}

function createLiveReplayClient(endpoint: string, bearer: string | undefined): LiveReplayClient {
  const httpEndpoint = endpoint.replace(/^ws(s?):\/\//, "http$1://").replace(/\/v1\/stream\/?$/, "");
  let currentBearer = bearer;
  return {
    async fetch(roomId, atRoomSeq) {
      if (!currentBearer) throw new Error("live replay authorization is unavailable");
      const url = `${httpEndpoint}/v1/rooms/${encodeURIComponent(roomId)}/replay?at_room_seq=${atRoomSeq}`;
      const response = await fetch(url, {
        method: "GET",
        headers: { Accept: "application/json", Authorization: `Bearer ${currentBearer}` },
        cache: "no-store",
        credentials: "omit",
      });
      if (!response.ok) throw new Error("live replay request failed");
      const value: unknown = await response.json();
      if (!isReplayResponse(value, roomId, atRoomSeq)) throw new Error("live replay response was invalid");
      return value as JsonValue;
    },
  };
}

export function useLiveSession(
  fixture: HeistFixture,
  config?: LiveSessionConfig,
  providedTransport?: WorldStreamTransport | null,
  providedReplayClient?: LiveReplayClient | null,
): LiveSessionView | null {
  const initialTransport = useMemo<WorldStreamTransport | null>(() => {
    if (providedTransport !== undefined) return providedTransport;
    if (config?.transportFactory) return config.transportFactory();
    if (!config) return null;
    return createLiveSessionTransport(config);
  }, [config, providedTransport]);
  const [transport, setTransport] = useState<WorldStreamTransport | null>(initialTransport);
  const [replayClient] = useState<LiveReplayClient | null>(providedReplayClient ?? null);
  const effectGeneration = useRef(0);
  const replayRequested = useRef<number | null>(null);
  const latestFixture = useRef(fixture);
  latestFixture.current = fixture;
  const [view, setView] = useState<LiveSessionView>(() => initialLiveSessionView(fixture));

  useEffect(() => {
    if (!config) return;
    if (!transport) return;
    const generation = effectGeneration.current + 1;
    effectGeneration.current = generation;
    let disposed = false;
    let sync: SyncTracker | null = null;
    const sendObservationAck = (frameSeq: number) => {
      if (!sync || !sync.acknowledged || sync.observationAckThrough !== null && frameSeq <= sync.observationAckThrough) return;
      sync.observationAckThrough = frameSeq;
      void transport.send(createRequest("observation.ack", nextId(), { room_id: config.roomId, member_id: config.memberId, through_frame_seq: frameSeq })).catch(() => {
        if (!disposed) setView((current) => makeErrorView(current, "Live session could not acknowledge the observation."));
      });
    };
    const update = (message: ProtocolMessage) => {
      if (disposed) return;
      setView((current) => {
        const next = applyMessage(current, message, latestFixture.current, config);
        if (replayClient && next.fixture.phase === "Complete" && next.fixture.replay.verification !== "Verified" && replayRequested.current !== next.fixture.roomSequence) {
          replayRequested.current = next.fixture.roomSequence;
          void replayClient.fetch(config.roomId, next.fixture.roomSequence).then((value) => {
            if (!disposed) setView((latest) => applyLiveReplay(latest, value));
          }).catch(() => {
            if (!disposed) setView((latest) => ({ ...latest, fixture: { ...latest.fixture, replay: { ...latest.fixture.replay, verification: "Unavailable while quarantined", historicalAuthorization: "Unavailable" } } }));
          });
        }
        return next;
      });
      if (message.type === "room.attached") sync = syncTrackerFromAttach(objectBody(message.body));
      if (message.type === "projection.reset" && sync?.kind === "projection_reset") {
        if (objectBody(message.body).baseline_frame_head === sync.through) sync.resetInstalled = true;
      }
      if (message.type === "observation.deliver") {
        const body = objectBody(message.body);
        if (typeof body.frame_seq === "number" && sync) sync.receivedFrames.push(body.frame_seq);
        if (typeof body.frame_seq === "number") sendObservationAck(body.frame_seq);
      }
      if (sync && !sync.acknowledgementSent && syncBatchReady(sync)) {
        sync.acknowledgementSent = true;
        void transport.send(createRequest("room.sync_ack", nextId(), { room_id: config.roomId, member_id: config.memberId, through_frame_head: sync.through, sync_token: sync.syncToken })).catch(() => {
          if (!disposed) setView((current) => makeErrorView(current, "Live session could not acknowledge the Room projection."));
        });
      }
      if (message.type === "room.sync_acked" && sync && objectBody(message.body).through_frame_head === sync.through) {
        sync.acknowledged = true;
        const through = latestObservationFrameToAck(sync.receivedFrames, sync.acknowledged);
        if (through !== null) sendObservationAck(through);
      }
    };
    const unsubscribe = transport.subscribe(update);
    setView(makeConnectingView(latestFixture.current));
    void transport.connect().then(async () => {
      if (disposed) return;
      try {
        await transport.send(createRequest("room.attach", nextId(), { room_id: config.roomId, member_id: config.memberId, after_frame_seq: null }));
      } catch {
        if (!disposed) setView((current) => makeErrorView(current, "Live session could not be attached."));
      }
    }, () => {
      if (!disposed) setView((current) => makeErrorView(current, "Live session could not be attached."));
    });
    return () => {
      disposed = true;
      unsubscribe();
      queueMicrotask(() => {
        if (effectGeneration.current === generation) {
          try { transport.close(); } catch { /* a replacement session may already own teardown */ }
        }
      });
    };
  }, [config, replayClient, transport]);

  if (!config || !transport) return null;
  return {
    ...view,
    submitAction: async ({ offerId, schemaDigest, actionType, payload, basedOnRoomSeq }) => {
      if (transport.state !== "open" || view.status !== "live") throw new Error("live session is not attached");
      if (view.fixture.runtime.recovery !== "Active" || view.fixture.runtime.roomHealth !== "Healthy" || view.fixture.runtime.frame.delivery !== "Live") throw new Error("live session is not at an exact synchronized head");
      if (view.action.state === "stale" || view.action.state === "resync-required") throw new Error("stale action requires a new synchronized head");
      if (basedOnRoomSeq !== view.fixture.roomSequence) throw new Error("action basis is stale; synchronize before submitting");
      if (!isCurrentActionOffer(view.fixture, { offerId, schemaDigest, actionType })) throw new Error("action offer is not the current installed offer");
      const message: ActionSubmitMessage = createActionSubmitRequest({ message_id: nextId(), room_id: config.roomId, member_id: config.memberId, action_id: nextId(), based_on_room_seq: basedOnRoomSeq, action_type: actionType, payload });
      setView((current) => ({ ...current, action: { state: "submitting", actionId: message.body.action_id, requestId: message.message_id, basedOnRoomSeq, message: "Action sent; waiting for a durable server receipt." } }));
      try { await transport.send(message); } catch (error) { setView((current) => makeErrorView(current, "Live session could not submit the Action.")); throw error; }
    },
    resync: async () => {
      if (!config?.transportFactory) throw new Error("live resync is not configured");
      setView((current) => ({ ...current, status: "connecting", error: null, action: { ...IDLE_ACTION, message: "Creating a fresh Session to resynchronize the Room Head." }, fixture: withRuntime(current.fixture, "Connecting", "Loading", "Awaiting attach", "A fresh Session is being created after a stale Action result.") }));
      // Install the replacement even if an adapter's close path throws or
      // takes time. The effect cleanup closes the old adapter as well.
      setTransport(replaceLiveTransport(transport, config.transportFactory));
    },
  };
}

function makeInitialView(fixture: HeistFixture): LiveSessionView {
  return { fixture, status: "disconnected", error: null, action: IDLE_ACTION, submitAction: async () => { throw new Error("live session is not configured"); }, resync: async () => { throw new Error("live resync is not configured"); } };
}

function makeConnectingView(fixture: HeistFixture): LiveSessionView {
  return { ...makeInitialView(fixture), status: "connecting", fixture: withRuntime(fixture, "Connecting", "Loading", "Awaiting attach", "Connecting to the configured WorldStream session.") };
}

function makeErrorView(current: LiveSessionView, error: string): LiveSessionView {
  return { ...current, status: "error", error, action: { ...current.action, state: current.action.state === "submitting" ? "resync-required" : current.action.state, message: error }, fixture: withRuntime(current.fixture, "Disconnected", "Inactive", "Unavailable", error) };
}

function applyMessage(
  current: LiveSessionView,
  message: ProtocolMessage,
  fixture: HeistFixture,
  session?: Pick<LiveSessionConfig, "roomId" | "memberId">,
): LiveSessionView {
  if (session && !messageTargetsSession(message, session)) {
    return makeErrorView(current, "Live session received a response for a different Room membership.");
  }
  if (message.type === "server.welcome") return { ...current, status: "connecting", error: null, fixture: withRuntime(fixture, "Connecting", "CatchingUp", "Awaiting attach", "Authenticated session established; requesting the Room projection.") };
  if (message.type === "room.attached") {
    const body = objectBody(message.body);
    const resetRequired = objectBody(body.sync).kind === "projection_reset";
    return { ...current, status: "attached", action: { ...current.action, state: "resync-required", message: "Install the captured projection or retained frames before submitting." }, fixture: withRuntime(fixture, "Attached", "CatchingUp", "Catching up", "Room attached; retained observations are being delivered.", body, "Pending", resetRequired ? "Required" : fixture.runtime.frame.reset) };
  }
  if (message.type === "projection.reset") {
    const body = objectBody(message.body);
    const authorized = applyAuthorizedProjection(current.fixture, body.projection);
    return { ...current, status: "attached", action: { ...current.action, state: "resync-required", message: "Projection reset installed; waiting for the Session sync barrier." }, fixture: withRuntime(authorized, "Attached", "CatchingUp", "Catching up", "The projection reset was installed; the Session sync barrier is pending.", body, "Pending", "Installed") };
  }
  if (message.type === "observation.deliver") {
    const body = objectBody(message.body);
    const isLive = current.status === "live";
    const authorized = applyAuthorizedProjection(current.fixture, body.observation);
    return { ...current, status: isLive ? "live" : "attached", fixture: withRuntime(authorized, isLive ? "Live" : "Attached", isLive ? "Active" : "CatchingUp", isLive ? "Live" : "Catching up", isLive ? "Live observations are arriving from the attached Room." : "Retained observations are being installed before live delivery.", body, current.fixture.runtime.frame.syncAck) };
  }
  if (message.type === "room.sync_acked") {
    const through = numberValue(objectBody(message.body).through_frame_head, -1);
    if (current.status !== "attached" && current.status !== "live") return current;
    if (through < 0) return current;
    // A Projection Reset establishes a fresh frame baseline. Its reset
    // acknowledgement is authoritative even when the prior Session's frame
    // head is still visible in the reducer snapshot.
    if (current.fixture.runtime.frame.head !== null && through !== current.fixture.runtime.frame.head && current.fixture.runtime.frame.reset !== "Installed") return current;
    const action = current.action.state === "stale" || current.action.state === "resync-required" ? { ...IDLE_ACTION, message: "Synchronized head installed; a new Action ID may be created from the current offers." } : current.action;
    const synchronized = withRuntime(current.fixture, "Live", "Active", "Live", "Room synchronization acknowledged; exact-head Actions are enabled.", objectBody(message.body), "Sent");
    return { ...current, status: "live", error: null, action, fixture: { ...synchronized, runtime: { ...synchronized.runtime, frame: { ...synchronized.runtime.frame, head: through } } } };
  }
  if (message.type === "observation.acked") return { ...current, fixture: { ...current.fixture, runtime: { ...current.fixture.runtime, frame: { ...current.fixture.runtime.frame, observationAck: "Sent" } } } };
  if (message.type === "action.accepted") {
    const body = objectBody(message.body);
    if (!matchesPendingAction(current.action, message, body)) return current;
    return { ...current, status: "live", action: { ...current.action, state: "accepted", message: body.duplicate === true ? "Duplicate receipt resolved to the original committed Action." : "Action accepted at the canonical Room Head." }, fixture: withRuntime(current.fixture, "Live", "Active", "Live", "Action committed; the UI is waiting for its resulting observation.", body) };
  }
  if (message.type === "action.rejected") {
    const body = objectBody(message.body);
    if (!matchesPendingAction(current.action, message, body)) return current;
    const currentRoomSeq = numberValue(body.current_room_seq, current.fixture.roomSequence);
    const stale = body.code === "stale_room_state" || currentRoomSeq !== current.fixture.roomSequence;
    const withOffers = Array.isArray(body.action_offers) ? withAuthorizedProjection(current.fixture, { action_offers: body.action_offers }) : current.fixture;
    const rejectedFixture = withRuntime({ ...withOffers, roomSequence: Math.max(withOffers.roomSequence, currentRoomSeq) }, "Live", "Active", "Live", stale ? "The server rejected this Action at a stale Head; synchronize and create a new Action ID." : "The server rejected the Action; no canonical state was changed.");
    return { ...current, status: "live", action: { ...current.action, state: stale ? "stale" : "rejected", message: stringValue(body.message) ?? (stale ? "Stale Room Head" : "Action rejected") }, fixture: rejectedFixture };
  }
  if (message.type === "error") {
    const body = objectBody(message.body);
    const health = healthFromWire(body.code);
    const error = "The live WorldStream session reported an error.";
    const errored = makeErrorView(current, error);
    return health === undefined ? errored : { ...errored, fixture: withRuntime(errored.fixture, "Disconnected", current.fixture.runtime.recovery, "Unavailable", error, body, current.fixture.runtime.frame.syncAck, current.fixture.runtime.frame.reset, health) };
  }
  return current;
}

function messageTargetsSession(message: ProtocolMessage, session: Pick<LiveSessionConfig, "roomId" | "memberId">): boolean {
  if (!["room.attached", "projection.reset", "observation.deliver", "observation.acked", "action.accepted", "action.rejected"].includes(message.type)) return true;
  const body = objectBody(message.body);
  return body.room_id === session.roomId && body.member_id === session.memberId;
}

function withRuntime(fixture: HeistFixture, transport: ConsoleTransportState, recovery: RuntimeStatus["recovery"], delivery: FrameDeliveryState, reason: string, body?: JsonValue, syncAck: RuntimeStatus["frame"]["syncAck"] = fixture.runtime.frame.syncAck, reset: RuntimeStatus["frame"]["reset"] = fixture.runtime.frame.reset, explicitHealth?: RoomHealthState): HeistFixture {
  const bodyRecord = objectBody(body);
  const roomHead = objectBody(bodyRecord.room_head);
  const cursor = nullableNumberValue(bodyRecord.cursor, fixture.runtime.frame.cursor);
  const frameHead = nullableNumberValue(bodyRecord.frame_head, fixture.runtime.frame.head);
  const roomSequence = numberValue(roomHead.room_seq, numberValue(bodyRecord.cause_room_seq, fixture.roomSequence));
  const health = explicitHealth ?? healthFromWire(bodyRecord.room_health) ?? fixture.runtime.roomHealth;
  const completeHead = stringValue(roomHead.genesis_or_transition_hash) ?? fixture.participant.exactHead;
  const replay = fixture.discovery.status === "Fixture only" ? { ...fixture.replay, verification: "Pending" as const, historicalAuthorization: "Unavailable" as const } : fixture.replay;
  const next: HeistFixture = { ...fixture, fixtureLabel: "Live WorldStream session · only authorized projection fields are applied", discovery: { ...fixture.discovery, status: "Live" as DiscoveryStatus }, roomSequence, participant: { ...fixture.participant, exactHead: abbreviate(completeHead) }, runtime: { ...fixture.runtime, transport, recovery, roomHealth: health, frame: { ...fixture.runtime.frame, delivery, cursor, head: frameHead, syncAck, reset }, reason }, replay };
  return withOperatorDiagnostics(next, bodyRecord, health, transport, cursor, frameHead);
}

function withOperatorDiagnostics(fixture: HeistFixture, body: Record<string, JsonValue>, health: RoomHealthState, transport: ConsoleTransportState, cursor: number | null, frameHead: number | null): HeistFixture {
  const roomHead = objectBody(body.room_head);
  const generation = numberValue(body.integrity_generation, fixture.operator.integrity.generation);
  const completeHead = stringValue(roomHead.genesis_or_transition_hash) ?? fixture.operator.integrity.completeHead;
  const coreHash = stringValue(roomHead.core_state_hash) ?? fixture.operator.integrity.coreHash;
  const activityHash = stringValue(roomHead.activity_state_hash) ?? fixture.operator.integrity.activityHash;
  const aggregateHash = stringValue(roomHead.authoritative_state_hash) ?? fixture.operator.integrity.aggregateHash;
  const live = transport !== "Unavailable";
  return { ...fixture, operator: { ...fixture.operator, session: live ? { ...fixture.operator.session, status: transport === "Disconnected" ? "Disconnected" : "Attached", cursor: cursor === null ? "not acknowledged" : `frame seq ${cursor}` } : fixture.operator.session, runner: live ? { availability: "Unavailable", runnerRef: "Separate /v1/runner/stream", activation: "Idle", invocation: "Not exposed by the Room stream" } : fixture.operator.runner, frame: live ? { ...fixture.operator.frame, delivery: frameDeliveryLabel(fixture.runtime.frame.delivery), headSequence: frameHead ?? fixture.operator.frame.headSequence, cursorSequence: cursor ?? fixture.operator.frame.cursorSequence } : fixture.operator.frame, timer: { ...fixture.operator.timer, phaseDeadline: fixture.phaseDeadline ?? "No deadline in current authorized projection", timerGeneration: `generation ${fixture.phaseGeneration}` }, integrity: { ...fixture.operator.integrity, state: health, generation, completeHead: abbreviate(completeHead), coreHash: abbreviate(coreHash), activityHash: abbreviate(activityHash), aggregateHash: abbreviate(aggregateHash), safeReason: health === "Healthy" ? "No incident recorded by this Session" : `Room is ${health.toLowerCase()}; canonical mutation is disabled` } } };
}

function applyAuthorizedProjection(fixture: HeistFixture, value: JsonValue): HeistFixture {
  const envelope = objectBody(value);
  const source = objectBody("observation" in envelope ? envelope.observation : envelope);
  const activity = objectBody("activity" in source ? source.activity : "projection" in source ? source.projection : source);
  const sourceOffers = Array.isArray(source.action_offers) ? source.action_offers : envelope.action_offers;
  const normalizedActivity = Array.isArray(activity.action_offers) || !Array.isArray(sourceOffers)
    ? activity
    : { ...activity, action_offers: sourceOffers };
  const withOffers = withAuthorizedProjection(fixture, normalizedActivity);
  const withPrivateClues = applyPrivateActivity(withOffers, normalizedActivity);
  return applyPublicActivity(withPrivateClues, normalizedActivity);
}

function withAuthorizedProjection(fixture: HeistFixture, value: JsonValue): HeistFixture {
  const projection = objectBody(value);
  if (!Array.isArray(projection.action_offers)) return fixture;
  const actionTypes: readonly HeistActionType[] = ["inspect_clue", "publish_clue", "offer_exchange", "accept_exchange", "propose_plan", "endorse_plan", "challenge_plan", "commit_move", "acknowledge_result"];
  const offers = projection.action_offers.flatMap((candidate, index) => {
    const offer = objectBody(candidate);
    if (typeof offer.action_type !== "string" || !actionTypes.includes(offer.action_type as HeistActionType) || typeof offer.payload_schema_digest !== "string") return [];
    const eligibility = objectBody(offer.eligibility_window);
    const window = typeof eligibility.opens_at === "string" && typeof eligibility.deadline === "string" ? `${eligibility.opens_at} ≤ admitted_at < ${eligibility.deadline}` : "Current synchronized Head";
    return [{ id: `${fixture.roomSequence}:${offer.action_type}:${index}`, label: offer.action_type.replaceAll("_", " "), actionType: offer.action_type as HeistActionType, schemaDigest: offer.payload_schema_digest, eligibility: window }];
  });
  return { ...fixture, participant: { ...fixture.participant, offers } };
}

function applyPrivateActivity(fixture: HeistFixture, value: JsonValue): HeistFixture {
  const activity = objectBody(value);
  if (!Array.isArray(activity.private_clues)) return fixture;
  const privateClues = activity.private_clues.flatMap((candidate) => {
    const clue = objectBody(candidate);
    const clueId = stringValue(clue.clue_id);
    const claimCode = stringValue(clue.claim_code);
    return clueId !== undefined && claimCode !== undefined ? [{ clueId, claimCode }] : [];
  });
  return { ...fixture, participant: { ...fixture.participant, privateClues } };
}

/** Whitelist public pack projection keys; private clues/commitments never enter the UI fixture. */
function applyPublicActivity(fixture: HeistFixture, value: JsonValue): HeistFixture {
  const activity = objectBody(value);
  let next = fixture;
  const phase = phaseFromWire(stringValue(activity.phase));
  if (isHeistPhase(phase)) {
    const phaseDeadline = activity.phase_deadline === null ? null : stringValue(activity.phase_deadline) ?? fixture.phaseDeadline;
    next = { ...next, phase, phaseGeneration: numberValue(activity.phase_generation, fixture.phaseGeneration), phaseDeadline, deadline: phaseDeadline ?? "—", phaseDescription: phase === "Complete" ? "The activity is terminal. Core Room status is separate from Activity Phase." : `Live authorized Activity Phase · ${phase}` };
  }
  if (Array.isArray(activity.seats)) {
    const seats = activity.seats.flatMap((candidate) => { const seat = objectBody(candidate); const role = roleFromWire(seat.role); return role === undefined || typeof seat.present !== "boolean" ? [] : [{ role, present: seat.present }]; });
    if (seats.length > 0) { const byRole = new Map(seats.map((seat) => [seat.role, seat.present])); next = { ...next, roles: next.roles.map((role) => ({ ...role, presence: byRole.get(role.role) ? "Present" : "Awaiting" })) }; }
  }
  if (Array.isArray(activity.public_claims)) {
    const clues = activity.public_claims.flatMap((candidate, index) => { const claim = objectBody(candidate); const id = stringValue(claim.clue_id); const code = stringValue(claim.claim_code); return id === undefined || code === undefined ? [] : [{ id, label: `Public claim ${index + 1}`, claim: code, state: "Published" as const }]; });
    next = { ...next, publicClues: clues };
  }
  if (Array.isArray(activity.plans)) {
    const endorsements = objectBody(activity.endorsements);
    const challenges = Array.isArray(activity.challenges) ? activity.challenges : [];
    const counts = new Map<string, number>();
    for (const value of Object.values(endorsements)) {
      if (typeof value === "string") counts.set(value, (counts.get(value) ?? 0) + 1);
    }
    const challengeCounts = new Map<string, number>();
    for (const candidate of challenges) {
      const challenge = objectBody(candidate);
      const planId = stringValue(challenge.plan_id);
      if (planId !== undefined) challengeCounts.set(planId, (challengeCounts.get(planId) ?? 0) + 1);
    }
    const plans = activity.plans.flatMap((candidate) => {
      const plan = objectBody(candidate);
      const id = stringValue(plan.plan_id);
      if (id === undefined) return [];
      const route = stringValue(plan.route) ?? "published route";
      const window = stringValue(plan.entry_window) ?? "published window";
      const challengeCount = challengeCounts.get(id) ?? 0;
      return [{ id, label: `${route} / ${window}`, endorsements: counts.get(id) ?? 0, challenges: challengeCount, status: challengeCount > 0 ? "Under review" as const : "Leading" as const }];
    });
    next = { ...next, publicPlans: plans };
    const publicChallenges = challenges.flatMap((candidate, index) => {
      const challenge = objectBody(candidate);
      const planId = stringValue(challenge.plan_id);
      const reason = stringValue(challenge.reason);
      if (planId === undefined || reason === undefined) return [];
      return [{ id: `challenge-${index + 1}`, target: planId, reason, state: "Open" as const }];
    });
    next = { ...next, publicChallenges };
  }
  if (typeof activity.commitment_count === "number" && Number.isSafeInteger(activity.commitment_count) && activity.commitment_count >= 0) next = { ...next, commitmentCount: { ...next.commitmentCount, submitted: Math.min(activity.commitment_count, next.commitmentCount.total) } };
  if (activity.outcome !== undefined) next = { ...next, result: safeAggregateResult(next.result, activity.outcome) };
  return next;
}

function safeAggregateResult(current: HeistFixture["result"], value: JsonValue): HeistFixture["result"] {
  const outcome = objectBody(value);
  const rawName = stringValue(outcome.outcome);
  const name = rawName === undefined ? undefined : rawName === "success" ? "Success" : rawName === "partial_failure" ? "Partial failure" : "Failure";
  const selected = stringValue(outcome.selected_plan_id);
  const counts = objectBody(outcome.vote_counts);
  const voteSummary = Object.entries(counts).flatMap(([plan, count]) => typeof count === "number" ? [`${plan} · ${count}`] : []).join(" · ") || current.voteSummary;
  const checks = objectBody(outcome.checks);
  const checkValues = Object.values(checks).filter((item): item is boolean => typeof item === "boolean");
  const checksTotal = checkValues.length > 0 ? checkValues.length : current.checksTotal;
  const checksPassed = checkValues.length > 0 ? checkValues.filter(Boolean).length : numberValue(outcome.score, current.checksPassed);
  return { ...current, availability: name === undefined ? current.availability : "Available", outcome: name ?? current.outcome, selectedPlan: selected ?? current.selectedPlan, voteSummary, checksPassed: Math.max(0, Math.min(checksTotal, checksPassed)), checksTotal, scoreLabel: name ?? current.scoreLabel };
}

function applyLiveReplay(current: LiveSessionView, value: JsonValue): LiveSessionView {
  const body = objectBody(value);
  const head = objectBody(body.room_head);
  const coreStateHash = stringValue(head.core_state_hash);
  const activityStateHash = stringValue(head.activity_state_hash);
  const aggregateStateHash = stringValue(head.authoritative_state_hash);
  const lineageHash = stringValue(head.genesis_or_transition_hash);
  if (coreStateHash === undefined || activityStateHash === undefined || aggregateStateHash === undefined || lineageHash === undefined) return current;
  const requested = numberValue(body.requested_room_seq, current.fixture.roomSequence);
  return { ...current, fixture: { ...current.fixture, replay: { ...current.fixture.replay, availableThrough: `Sequence ${requested}`, verification: "Verified", presentAuthorization: "Authorized", historicalAuthorization: "Public projection authorized", hashes: { coreStateHash, activityStateHash, aggregateStateHash, lineageHash, transitionHashes: [lineageHash] } } } };
}

function matchesPendingAction(action: LiveActionState, message: ProtocolMessage, body: Record<string, JsonValue>): boolean {
  if (action.actionId === null && action.requestId === null) return false;
  return body.action_id === action.actionId || message.request_id === action.requestId;
}

function syncTrackerFromAttach(body: Record<string, JsonValue>): SyncTracker | null {
  const branch = objectBody(body.sync); if (typeof body.sync_token !== "string") return null;
  if (branch.kind === "projection_reset" && typeof branch.baseline_frame_head === "number") return { kind: "projection_reset", through: branch.baseline_frame_head, cursorExclusive: null, syncToken: body.sync_token, receivedFrames: [], resetInstalled: false, acknowledgementSent: false, acknowledged: false, observationAckThrough: null };
  if (branch.kind === "retained_frames" && typeof branch.cursor_exclusive === "number" && typeof branch.through_frame_head === "number") return { kind: "retained_frames", through: branch.through_frame_head, cursorExclusive: branch.cursor_exclusive, syncToken: body.sync_token, receivedFrames: [], resetInstalled: false, acknowledgementSent: false, acknowledged: false, observationAckThrough: null };
  return null;
}

function syncBatchReady(sync: SyncTracker): boolean {
  if (sync.kind === "projection_reset") return sync.resetInstalled;
  const expected: number[] = []; for (let frame = (sync.cursorExclusive ?? -1) + 1; frame <= sync.through; frame += 1) expected.push(frame);
  return JSON.stringify([...new Set(sync.receivedFrames)].sort((a, b) => a - b)) === JSON.stringify(expected);
}

function objectBody(value: unknown): Record<string, JsonValue> { return value !== undefined && value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, JsonValue> : {}; }
function stringValue(value: JsonValue | undefined): string | undefined { return typeof value === "string" && value.length > 0 ? value : undefined; }
function numberValue(value: JsonValue | undefined, fallback: number): number { return typeof value === "number" && Number.isSafeInteger(value) ? value : fallback; }
function nullableNumberValue(value: JsonValue | undefined, fallback: number | null): number | null { return typeof value === "number" && Number.isSafeInteger(value) ? value : fallback; }
function healthFromWire(value: JsonValue | undefined): RoomHealthState | undefined { if (value === "healthy") return "Healthy"; if (value === "faulted" || value === "room_faulted") return "Faulted"; if (value === "quarantined" || value === "room_quarantined") return "Quarantined"; return undefined; }
function phaseFromWire(value: string | undefined): HeistPhase | undefined {
  if (value === undefined) return undefined;
  const normalized = value.replaceAll("_", " ").toLowerCase();
  const phases: Record<string, HeistPhase> = {
    briefing: "Briefing",
    negotiation: "Negotiation",
    commitment: "Commitment",
    resolution: "Resolution",
    result: "Result",
    complete: "Complete",
  };
  return phases[normalized];
}

function isHeistPhase(value: string | undefined): value is HeistPhase { return phaseFromWire(value) !== undefined; }
function roleFromWire(value: JsonValue | undefined): RolePresence["role"] | undefined { if (value === "navigator" || value === "Navigator") return "Navigator"; if (value === "insider" || value === "Insider") return "Insider"; if (value === "broker" || value === "Broker") return "Broker"; return undefined; }
function frameDeliveryLabel(value: FrameDeliveryState): "Live" | "Catching up" | "Reset required" { return value === "Live" ? "Live" : value === "Reset required" ? "Reset required" : "Catching up"; }
function abbreviate(value: string): string { return value.length <= 18 ? value : `${value.slice(0, 10)}···${value.slice(-6)}`; }
function isReplayResponse(value: unknown, roomId: string, atRoomSeq: number): boolean {
  const body = objectBody(value);
  const head = objectBody(body.room_head);
  const projection = objectBody(body.projection);
  return body.room_id === roomId && body.requested_room_seq === atRoomSeq && body.verification === "verified" && typeof head.room_id === "string" && head.room_id === roomId && typeof head.room_seq === "number" && Object.keys(projection).length > 0;
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
let sequence = 1;
function nextId(): string { let value = sequence; sequence += 1; let suffix = ""; do { suffix = CROCKFORD[value % 32] + suffix; value = Math.floor(value / 32); } while (value > 0); suffix = suffix.padStart(10, "0"); return `${LIVE_ID.slice(0, 16)}${suffix}`.slice(0, 26); }
