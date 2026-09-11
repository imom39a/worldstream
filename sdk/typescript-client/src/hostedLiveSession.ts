import {
  ActivityClientHandoffError,
  type ActivityClientHandoffClient,
  type ActivityClientRoomHead,
  type ActivityClientSessionStatus,
  type ActivityClientStartup,
  type AuthorizedRoomDeliveryBatch,
} from "./browserHandoff";
import {
  MAX_MESSAGE_BYTES,
  WORLDSTREAM_PROTOCOL,
  WORLDSTREAM_WS_SUBPROTOCOL,
  decodeMessage,
  type JsonValue,
  type ProtocolMessage,
  type WebSocketFactory,
  type WebSocketLike,
} from "./transport";

const STREAM_PATH = "/v1/hosted/browser-stream";
const TICKET_PATTERN = /^wst1:[0-9a-f]{64}$/u;
const ULID_PATTERN = /^[0-9A-HJKMNP-TV-Z]{26}$/u;
const DIGEST_PATTERN = /^(?:blake3|sha256):[0-9a-f]{64}$/u;
const DEFAULT_CONNECT_TIMEOUT_MS = 20_000;
const DEFAULT_ACTION_TIMEOUT_MS = 15_000;
const DEFAULT_HEARTBEAT_GRACE_MS = 5_000;
const MAX_WAITERS = 32;

export type HostedLiveSessionStatus =
  | "idle"
  | "connecting"
  | "synchronizing"
  | "live"
  | "disconnected"
  | "setup_required"
  | "closed";

export interface HostedLiveAcceptedReceipt {
  readonly state: "accepted";
  readonly actionId: string;
  readonly duplicate: boolean;
  readonly roomHead: ActivityClientRoomHead;
}

export interface HostedLiveRejectedReceipt {
  readonly state: "rejected";
  readonly actionId: string;
  readonly code: string;
  readonly currentRoomSeq: number;
  readonly retryableWithSameActionId: boolean;
  readonly maySubmitRevisedAction: boolean;
  readonly duplicate: boolean;
}

export type HostedLiveActionReceipt =
  | HostedLiveAcceptedReceipt
  | HostedLiveRejectedReceipt;

export interface HostedLiveActionInput {
  readonly actionId: string;
  readonly basedOnRoomSeq: number;
  readonly actionType: string;
  readonly payload: JsonValue;
}

/**
 * Pack-neutral state exposed to an Activity Client. Routing identity remains
 * private to the controller and can never be selected by its consumer.
 */
export interface HostedLiveSessionSnapshot {
  readonly status: HostedLiveSessionStatus;
  readonly synchronized: boolean;
  readonly canAct: boolean;
  readonly deliveryBatch: AuthorizedRoomDeliveryBatch | null;
  readonly lastAcknowledgedFrameSeq: number | null;
  readonly actionReceipt: HostedLiveActionReceipt | null;
  readonly message: string | null;
}

export interface HostedLiveSessionAuthority {
  redeem(handoff: string): Promise<ActivityClientSessionStatus>;
  resume(): Promise<ActivityClientSessionStatus>;
  issueStreamTicket(afterFrameSeq: number | null): Promise<{
    readonly ticket: string;
    readonly expiresInMs: number;
  }>;
}

export interface HostedLiveSessionControllerOptions {
  readonly streamUrl: string;
  readonly authority: HostedLiveSessionAuthority;
  readonly webSocketFactory?: WebSocketFactory;
  readonly createMessageId?: () => string;
  readonly connectTimeoutMs?: number;
  readonly actionTimeoutMs?: number;
  readonly heartbeatGraceMs?: number;
}

interface BoundTarget {
  readonly roomId: string;
  readonly memberId: string;
  readonly accessMode: "participant" | "spectator";
  readonly pack: AuthorizedRoomDeliveryBatch["pack"];
  readonly cursor: number | null;
  roomHead: ActivityClientRoomHead;
  roomHealth: string;
}

interface SyncState {
  readonly kind: "retained_frames" | "projection_reset";
  readonly through: number;
  readonly cursorExclusive: number | null;
  readonly token: string;
  resetInstalled: boolean;
  acknowledgementRequestId: string | null;
  acknowledged: boolean;
}

interface PendingAction {
  readonly actionId: string;
  readonly requestId: string;
  readonly basedOnRoomSeq: number;
  readonly resolve: (receipt: HostedLiveActionReceipt) => void;
  readonly reject: (error: Error) => void;
  readonly timer: ReturnType<typeof setTimeout>;
}

interface PendingObservationAck {
  readonly requestId: string;
  readonly through: number;
}

export interface HostedLiveWaitOptions {
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
}

/**
 * React-free direct-stream controller shared by first-party Activity Clients.
 * It owns connection and synchronization mechanics but never interprets a
 * Pack Projection, invents an Action, or exposes a routing selector.
 */
export class HostedLiveSessionController {
  private current: HostedLiveSessionSnapshot = initialSnapshot();
  private readonly listeners = new Set<
    (snapshot: HostedLiveSessionSnapshot) => void
  >();
  private readonly streamUrl: string;
  private readonly authority: HostedLiveSessionAuthority;
  private readonly socketFactory: WebSocketFactory;
  private readonly createMessageId: () => string;
  private readonly connectTimeoutMs: number;
  private readonly actionTimeoutMs: number;
  private readonly heartbeatGraceMs: number;
  private startTask: Promise<HostedLiveSessionSnapshot> | null = null;
  private reconnectTask: Promise<HostedLiveSessionSnapshot> | null = null;
  private socket: WebSocketLike | null = null;
  private target: BoundTarget | null = null;
  private sync: SyncState | null = null;
  private generation = 0;
  private retainedPackDigest: string | null = null;
  private lastReceivedFrameSeq: number | null = null;
  private lastObservationRoomSeq: number | null = null;
  private readonly recentFrameHashes = new Map<number, string>();
  private observationAck: PendingObservationAck | null = null;
  private pendingObservationAckThrough: number | null = null;
  private pendingAction: PendingAction | null = null;
  private heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
  private heartbeatIntervalMs = 0;
  private lastInboundAt = 0;
  private activeWaiters = 0;
  private welcomed = false;
  private synchronizationBusy = false;

  constructor(options: HostedLiveSessionControllerOptions) {
    this.streamUrl = exactHostedStreamUrl(options.streamUrl);
    this.authority = options.authority;
    this.socketFactory =
      options.webSocketFactory ??
      ((url, protocols) => new WebSocket(url, [...protocols]));
    this.createMessageId = options.createMessageId ?? createUlid;
    this.connectTimeoutMs = boundedTimeout(
      options.connectTimeoutMs,
      DEFAULT_CONNECT_TIMEOUT_MS,
      60_000,
    );
    this.actionTimeoutMs = boundedTimeout(
      options.actionTimeoutMs,
      DEFAULT_ACTION_TIMEOUT_MS,
      60_000,
    );
    this.heartbeatGraceMs = boundedTimeout(
      options.heartbeatGraceMs,
      DEFAULT_HEARTBEAT_GRACE_MS,
      60_000,
    );
  }

  get state(): HostedLiveSessionSnapshot {
    return this.current;
  }

  subscribe(listener: (snapshot: HostedLiveSessionSnapshot) => void): () => void {
    if (this.listeners.size >= 64) {
      throw new Error("Hosted live-session listener capacity is exhausted.");
    }
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Redeems or resumes the opaque Browser Activity Session exactly once. */
  start(startup: ActivityClientStartup): Promise<HostedLiveSessionSnapshot> {
    this.startTask ??= this.startOnce(startup);
    return this.startTask;
  }

  /** Revalidates the retained session and obtains a fresh one-use ticket. */
  reconnect(): Promise<HostedLiveSessionSnapshot> {
    if (this.current.status === "closed") {
      return Promise.reject(new Error("Hosted live session is closed."));
    }
    if (this.current.status === "setup_required") {
      return Promise.resolve(this.current);
    }
    this.reconnectTask ??= this.reconnectOnce().finally(() => {
      this.reconnectTask = null;
    });
    return this.reconnectTask;
  }

  /**
   * Submits one caller-created Action at the exact synchronized Room sequence.
   * The controller adds no offer semantics and sends no routing identifiers.
   */
  submitAction(input: HostedLiveActionInput): Promise<HostedLiveActionReceipt> {
    if (
      this.current.status !== "live" ||
      !this.current.canAct ||
      this.target?.accessMode !== "participant" ||
      this.pendingAction !== null
    ) {
      return Promise.reject(
        new Error("Hosted live session is not ready for an Action."),
      );
    }
    if (
      !ULID_PATTERN.test(input.actionId) ||
      !safeInteger(input.basedOnRoomSeq) ||
      input.basedOnRoomSeq !== this.target.roomHead.room_seq ||
      !boundedText(input.actionType, 256) ||
      !isJsonValue(input.payload, 0)
    ) {
      return Promise.reject(
        new Error("Hosted Action is not valid at the synchronized Head."),
      );
    }

    const requestId = this.nextMessageId();
    const message = encodeHostedRequest("action.submit", requestId, {
      action_id: input.actionId,
      based_on_room_seq: input.basedOnRoomSeq,
      action_type: input.actionType,
      payload: input.payload,
    });
    return new Promise<HostedLiveActionReceipt>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pendingAction?.requestId !== requestId) return;
        this.pendingAction = null;
        reject(new Error("Hosted Action receipt timed out."));
        this.failConnection(
          "Action outcome is uncertain. Reconnect before submitting again.",
        );
      }, this.actionTimeoutMs);
      this.pendingAction = {
        actionId: input.actionId,
        requestId,
        basedOnRoomSeq: input.basedOnRoomSeq,
        resolve,
        reject,
        timer,
      };
      try {
        this.send(message);
        this.update({ canAct: false, actionReceipt: null });
      } catch (error) {
        clearTimeout(timer);
        this.pendingAction = null;
        reject(asError(error, "Hosted Action could not be sent."));
        this.failConnection("Realtime connection was interrupted.");
      }
    });
  }

  /** Waits for a bounded state predicate without creating a polling loop. */
  waitFor(
    predicate: (snapshot: HostedLiveSessionSnapshot) => boolean,
    options: HostedLiveWaitOptions = {},
  ): Promise<HostedLiveSessionSnapshot> {
    if (predicate(this.current)) return Promise.resolve(this.current);
    if (this.activeWaiters >= MAX_WAITERS) {
      return Promise.reject(new Error("Hosted live-session wait capacity is exhausted."));
    }
    const timeoutMs = boundedTimeout(options.timeoutMs, 30_000, 60_000);
    this.activeWaiters += 1;
    return new Promise<HostedLiveSessionSnapshot>((resolve, reject) => {
      let settled = false;
      const finish = (
        outcome: { readonly value: HostedLiveSessionSnapshot } | { readonly error: Error },
      ) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        options.signal?.removeEventListener("abort", abort);
        unsubscribe();
        this.activeWaiters -= 1;
        if ("value" in outcome) resolve(outcome.value);
        else reject(outcome.error);
      };
      const abort = () => finish({ error: abortError() });
      const unsubscribe = this.subscribe((snapshot) => {
        if (predicate(snapshot)) finish({ value: snapshot });
      });
      const timer = setTimeout(
        () => finish({ error: new Error("Hosted live-session wait timed out.") }),
        timeoutMs,
      );
      if (options.signal?.aborted === true) abort();
      else options.signal?.addEventListener("abort", abort, { once: true });
    });
  }

  close(): void {
    if (this.current.status === "closed") return;
    this.generation += 1;
    this.clearHeartbeat();
    this.closeSocket();
    this.rejectPendingAction(new Error("Hosted live session closed."));
    this.target = null;
    this.sync = null;
    this.recentFrameHashes.clear();
    this.setState({
      ...initialSnapshot(),
      status: "closed",
      message: "Hosted live session closed.",
    });
    this.listeners.clear();
  }

  private async startOnce(
    startup: ActivityClientStartup,
  ): Promise<HostedLiveSessionSnapshot> {
    if (startup.kind === "invalid_handoff") {
      return this.requireSetup("This Activity Client handoff is invalid.");
    }
    if (startup.kind === "retained_error") {
      return this.applyAuthorityError(startup.error);
    }
    try {
      const status =
        startup.kind === "handoff"
          ? await this.authority.redeem(startup.handoff)
          : startup.kind === "retained"
            ? startup.status
            : await this.authority.resume();
      return this.applySessionStatus(status);
    } catch (error) {
      return this.applyAuthorityError(error);
    }
  }

  private async reconnectOnce(): Promise<HostedLiveSessionSnapshot> {
    this.update({
      status: "connecting",
      synchronized: false,
      canAct: false,
      message: "Revalidating the retained Activity Client session…",
    });
    try {
      return await this.applySessionStatus(await this.authority.resume());
    } catch (error) {
      return this.applyAuthorityError(error);
    }
  }

  private applySessionStatus(
    status: ActivityClientSessionStatus,
  ): Promise<HostedLiveSessionSnapshot> | HostedLiveSessionSnapshot {
    // A revalidated session can have a disconnected transport. Recovery still
    // needs a fresh, server-authorized ticket and a complete synchronization;
    // transport health alone must not prevent that recovery attempt.
    switch (status.state) {
      case "usable":
      case "disconnected":
        return this.connectStream();
    }
  }

  private async connectStream(): Promise<HostedLiveSessionSnapshot> {
    const deadline = Date.now() + this.connectTimeoutMs;
    let retryDelayMs = 100;
    while (true) {
      const result = await this.connectStreamAttempt(deadline);
      const generation = this.generation;
      if (!this.synchronizationBusy || result.status !== "disconnected") return result;
      const remainingMs = deadline - Date.now();
      if (remainingMs <= retryDelayMs) return result;
      // room_busy is an explicit retryable concurrency fence. Restart only
      // pre-Live synchronization, within the original connection deadline.
      // Never reuse a consumed ticket, handoff, or replay a domain Action.
      try {
        await this.waitFor((state) => state.status === "closed" || state.status === "setup_required", {
          timeoutMs: retryDelayMs,
        });
        return this.current;
      } catch {
        // Backoff elapsed. A close or superseding connection still wins.
      }
      if (this.generation !== generation || Date.now() >= deadline) return this.current;
      retryDelayMs = Math.min(retryDelayMs * 2, 1_000);
    }
  }

  private async connectStreamAttempt(deadline: number): Promise<HostedLiveSessionSnapshot> {
    const generation = this.generation + 1;
    this.generation = generation;
    this.synchronizationBusy = false;
    this.clearHeartbeat();
    this.closeSocket();
    this.sync = null;
    this.target = null;
    this.welcomed = false;
    this.observationAck = null;
    this.pendingObservationAckThrough = null;
    this.lastReceivedFrameSeq = this.current.lastAcknowledgedFrameSeq;
    this.lastObservationRoomSeq = null;
    this.recentFrameHashes.clear();
    this.update({
      status: "connecting",
      synchronized: false,
      canAct: false,
      actionReceipt: null,
      message: "Opening the direct realtime stream…",
    });

    let ticketValue = "";
    try {
      const ticketWait = new AbortController();
      const admission = await Promise.race([
        this.authority.issueStreamTicket(this.current.lastAcknowledgedFrameSeq),
        this.waitFor((state) => state.status === "closed" || state.status === "setup_required", {
          timeoutMs: Math.max(1, deadline - Date.now()),
          signal: ticketWait.signal,
        }).then(() => { throw new Error("Realtime admission was interrupted."); }),
      ]).finally(() => ticketWait.abort());
      if (
        this.generation !== generation ||
        Date.now() >= deadline ||
        !TICKET_PATTERN.test(admission.ticket) ||
        !safeInteger(admission.expiresInMs) ||
        admission.expiresInMs <= 0 ||
        admission.expiresInMs > 15_000
      ) {
        throw new Error("Hosted realtime admission was invalid.");
      }
      ticketValue = admission.ticket;
      const socket = this.socketFactory(
        this.streamUrl,
        [WORLDSTREAM_WS_SUBPROTOCOL],
        { headers: {} },
      );
      this.socket = socket;
      socket.onopen = () => {
        if (this.generation !== generation) return;
        try {
          socket.send(ticketValue);
          ticketValue = "";
          this.update({
            status: "synchronizing",
            message: "Waiting for an authorized Projection Reset or Catch-up…",
          });
        } catch {
          ticketValue = "";
          this.failConnection("Realtime admission failed.");
        }
      };
      socket.onmessage = (event) => {
        if (this.generation !== generation) return;
        this.handleInbound(event.data);
      };
      socket.onerror = () => {
        if (this.generation === generation) {
          this.failConnection("Realtime connection failed.");
        }
      };
      socket.onclose = () => {
        if (
          this.generation === generation &&
          this.current.status !== "closed" &&
          this.current.status !== "setup_required"
        ) {
          this.failConnection("Realtime connection closed. Reconnect to continue.");
        }
      };
      return await this.waitFor(
        (snapshot) =>
          snapshot.status === "live" ||
          snapshot.status === "disconnected" ||
          snapshot.status === "setup_required" ||
          snapshot.status === "closed",
        { timeoutMs: Math.max(1, deadline - Date.now()) },
      );
    } catch (error) {
      ticketValue = "";
      if (this.generation !== generation) return this.current;
      if (error instanceof ActivityClientHandoffError) {
        return this.applyAuthorityError(error);
      }
      this.failConnection("Realtime connection could not be established.");
      return this.current;
    }
  }

  private handleInbound(raw: unknown): void {
    if (typeof raw !== "string") {
      this.failConnection("Realtime protocol rejected a non-text frame.");
      return;
    }
    try {
      const message = decodeMessage(raw);
      this.lastInboundAt = Date.now();
      switch (message.type) {
        case "server.welcome":
          this.handleWelcome(message);
          break;
        case "room.attached":
          this.handleAttached(message);
          break;
        case "projection.reset":
          this.handleProjectionReset(message);
          break;
        case "observation.deliver":
          this.handleObservation(message);
          break;
        case "room.sync_acked":
          this.handleSyncAcknowledged(message);
          break;
        case "observation.acked":
          this.handleObservationAcknowledged(message);
          break;
        case "action.accepted":
        case "action.rejected":
          this.handleActionReceipt(message);
          break;
        case "server.ping":
          this.send(
            encodeHostedRequest("client.pong", this.nextMessageId(), {}),
          );
          break;
        case "error": {
          const error = record(message.body);
          this.synchronizationBusy = this.current.status === "synchronizing" &&
            error.code === "room_busy" && error.retryable === true;
          this.failConnection("WorldStream rejected the realtime operation.");
          break;
        }
        default:
          break;
      }
    } catch {
      this.failConnection("Realtime protocol validation failed.");
    }
  }

  private handleWelcome(message: ProtocolMessage): void {
    if (
      message.type !== "server.welcome" ||
      this.welcomed ||
      this.target !== null
    ) {
      throw new Error("Unexpected welcome.");
    }
    this.welcomed = true;
    this.heartbeatIntervalMs = message.body.heartbeat_interval_ms;
    this.lastInboundAt = Date.now();
    this.startHeartbeat();
  }

  private handleAttached(message: ProtocolMessage): void {
    if (
      message.type !== "room.attached" ||
      !this.welcomed ||
      this.target !== null
    ) {
      throw new Error("Unexpected attach.");
    }
    const body = record(message.body);
    const roomId = exactUlid(body.room_id);
    const memberId = exactUlid(body.member_id);
    const accessMode =
      body.access_mode === "participant" || body.access_mode === "spectator"
        ? body.access_mode
        : null;
    const role = body.role;
    const pack = packFrom(body.pack);
    const roomHead = roomHeadFrom(body.room_head, roomId);
    const frameHead = exactInteger(body.frame_head);
    const cursor = nullableInteger(body.cursor);
    const sync = syncFrom(body.sync, body.sync_token);
    if (
      roomId === null ||
      memberId === null ||
      accessMode === null ||
      pack === null ||
      roomHead === null ||
      frameHead === null ||
      cursor === undefined ||
      (cursor ?? -1) < (this.current.lastAcknowledgedFrameSeq ?? -1) ||
      sync === null ||
      body.membership_status !== "enabled" ||
      typeof body.room_health !== "string" ||
      (accessMode === "participant"
        ? typeof role !== "string" || !boundedText(role, 128)
        : role !== null) ||
      frameHead !== sync.through ||
      (sync.kind === "retained_frames" && cursor !== sync.cursorExclusive) ||
      (sync.kind === "retained_frames" && cursor !== this.current.lastAcknowledgedFrameSeq) ||
      (this.retainedPackDigest !== null &&
        this.retainedPackDigest !== pack.digest)
    ) {
      throw new Error("Attach was not exact.");
    }
    this.retainedPackDigest = pack.digest;
    this.target = {
      roomId,
      memberId,
      accessMode,
      pack,
      cursor,
      roomHead,
      roomHealth: body.room_health,
    };
    this.sync = sync;
    this.lastReceivedFrameSeq = sync.cursorExclusive ?? sync.through;
    this.update({
      status: "synchronizing",
      synchronized: false,
      canAct: false,
      message:
        sync.kind === "projection_reset"
          ? "Installing the authorized Projection Reset…"
          : "Installing retained observations…",
    });
    this.maybeSendSyncAcknowledgement();
  }

  private handleProjectionReset(message: ProtocolMessage): void {
    if (message.type !== "projection.reset") return;
    const target = this.requireTarget(message.body);
    const sync = this.sync;
    const body = record(message.body);
    const baseline = exactInteger(body.baseline_frame_head);
    const roomHead = roomHeadFrom(body.room_head, target.roomId);
    const projection = recordOrNull(body.projection);
    if (
      sync?.kind !== "projection_reset" ||
      baseline === null ||
      baseline !== sync.through ||
      roomHead === null ||
      roomHead.pack_digest !== target.pack.digest ||
      projection === null ||
      typeof body.room_health !== "string" ||
      !boundedText(body.room_health, 128) ||
      typeof body.projection_schema !== "string" ||
      typeof body.projection_hash !== "string" ||
      !DIGEST_PATTERN.test(body.projection_hash)
    ) {
      throw new Error("Projection Reset was not exact.");
    }
    target.roomHead = roomHead;
    target.roomHealth = body.room_health;
    sync.resetInstalled = true;
    this.lastReceivedFrameSeq = baseline;
    const safeBody = withoutRouting(body);
    this.update({
      deliveryBatch: {
        pack: target.pack,
        room_head: roomHead,
        frame_head: baseline,
        delivery: [{ kind: "projection_reset", body: safeBody }],
      },
      message: "Authorized Projection Reset installed; completing synchronization…",
      // A newer stored Cursor is safe to adopt only with the replacement
      // baseline installed. An interrupted Reset must request it again.
      lastAcknowledgedFrameSeq: target.cursor,
    });
    this.maybeSendSyncAcknowledgement();
  }

  private handleObservation(message: ProtocolMessage): void {
    if (message.type !== "observation.deliver") return;
    const target = this.requireTarget(message.body);
    const body = record(message.body);
    const frameSeq = exactInteger(body.frame_seq);
    const causeRoomSeq = exactInteger(body.cause_room_seq);
    const frameHash =
      typeof body.frame_payload_hash === "string" &&
      DIGEST_PATTERN.test(body.frame_payload_hash)
        ? body.frame_payload_hash
        : null;
    if (
      frameSeq === null ||
      causeRoomSeq === null ||
      frameHash === null ||
      recordOrNull(body.observation) === null ||
      this.sync === null ||
      this.lastReceivedFrameSeq === null
    ) {
      throw new Error("Observation was not exact.");
    }
    if (frameSeq <= this.lastReceivedFrameSeq) {
      if (this.recentFrameHashes.get(frameSeq) === frameHash) return;
      throw new Error("Observation replay conflicted.");
    }
    if (
      frameSeq !== this.lastReceivedFrameSeq + 1 ||
      (this.lastObservationRoomSeq !== null &&
        causeRoomSeq < this.lastObservationRoomSeq)
    ) {
      throw new Error("Observation order was not contiguous.");
    }
    this.lastReceivedFrameSeq = frameSeq;
    this.lastObservationRoomSeq = causeRoomSeq;
    this.recentFrameHashes.set(frameSeq, frameHash);
    while (this.recentFrameHashes.size > 256) {
      const oldest = this.recentFrameHashes.keys().next().value as
        | number
        | undefined;
      if (oldest === undefined) break;
      this.recentFrameHashes.delete(oldest);
    }
    const roomHead = { ...target.roomHead, room_seq: causeRoomSeq };
    target.roomHead = roomHead;
    const safeBody = withoutRouting(body);
    this.update({
      deliveryBatch: {
        pack: target.pack,
        room_head: roomHead,
        frame_head: frameSeq,
        delivery: [{ kind: "observation", body: safeBody }],
      },
      message:
        this.current.status === "live"
          ? null
          : "Installing retained observations…",
      canAct:
        this.current.status === "live" &&
        target.accessMode === "participant" &&
        target.roomHealth === "healthy",
    });
    this.maybeSendSyncAcknowledgement();
    if (this.sync.acknowledged) this.queueObservationAcknowledgement(frameSeq);
  }

  private handleSyncAcknowledged(message: ProtocolMessage): void {
    if (message.type !== "room.sync_acked" || this.sync === null) return;
    const body = record(message.body);
    if (
      exactInteger(body.through_frame_head) !== this.sync.through ||
      message.request_id !== this.sync.acknowledgementRequestId
    ) {
      throw new Error("Synchronization receipt did not match.");
    }
    this.sync.acknowledged = true;
    const canAct =
      this.target?.accessMode === "participant" &&
      this.target.roomHealth === "healthy";
    this.update({
      status: "live",
      synchronized: true,
      canAct,
      message: null,
    });
    if (
      this.lastReceivedFrameSeq !== null &&
      this.lastReceivedFrameSeq >
        (this.current.lastAcknowledgedFrameSeq ?? -1) &&
      this.recentFrameHashes.has(this.lastReceivedFrameSeq)
    ) {
      this.queueObservationAcknowledgement(this.lastReceivedFrameSeq);
    }
  }

  private handleObservationAcknowledged(message: ProtocolMessage): void {
    if (message.type !== "observation.acked" || this.observationAck === null) {
      return;
    }
    const target = this.requireTarget(message.body);
    const body = record(message.body);
    const cursor = exactInteger(body.cursor);
    if (
      target !== this.target ||
      message.request_id !== this.observationAck.requestId ||
      cursor !== this.observationAck.through
    ) {
      throw new Error("Observation acknowledgement did not match.");
    }
    this.observationAck = null;
    this.update({ lastAcknowledgedFrameSeq: cursor });
    const pending = this.pendingObservationAckThrough;
    this.pendingObservationAckThrough = null;
    if (pending !== null && pending > cursor) {
      this.queueObservationAcknowledgement(pending);
    }
  }

  private handleActionReceipt(message: ProtocolMessage): void {
    if (
      (message.type !== "action.accepted" &&
        message.type !== "action.rejected") ||
      this.pendingAction === null
    ) {
      return;
    }
    const pending = this.pendingAction;
    const target = this.requireTarget(message.body);
    const body = record(message.body);
    if (
      message.request_id !== pending.requestId ||
      body.action_id !== pending.actionId
    ) {
      return;
    }

    let receipt: HostedLiveActionReceipt;
    if (message.type === "action.accepted") {
      const roomHead = roomHeadFrom(body.room_head, target.roomId);
      if (roomHead === null || typeof body.duplicate !== "boolean") {
        throw new Error("Action receipt was not exact.");
      }
      target.roomHead = roomHead;
      receipt = {
        state: "accepted",
        actionId: pending.actionId,
        duplicate: body.duplicate,
        roomHead,
      };
    } else {
      const currentRoomSeq = exactInteger(body.current_room_seq);
      if (
        currentRoomSeq === null ||
        typeof body.code !== "string" ||
        !boundedText(body.code, 128) ||
        typeof body.retryable_with_same_action_id !== "boolean" ||
        typeof body.may_submit_revised_action !== "boolean" ||
        typeof body.duplicate !== "boolean"
      ) {
        throw new Error("Action rejection was not exact.");
      }
      receipt = {
        state: "rejected",
        actionId: pending.actionId,
        code: body.code,
        currentRoomSeq,
        retryableWithSameActionId: body.retryable_with_same_action_id,
        maySubmitRevisedAction: body.may_submit_revised_action,
        duplicate: body.duplicate,
      };
    }
    clearTimeout(pending.timer);
    this.pendingAction = null;
    pending.resolve(receipt);
    this.update({
      actionReceipt: receipt,
      canAct:
        receipt.state === "rejected" &&
        receipt.code === "stale_room_state"
          ? false
          : this.current.status === "live" &&
            target.accessMode === "participant" &&
            target.roomHealth === "healthy" &&
            (receipt.state === "rejected" ||
              this.current.deliveryBatch?.room_head.room_seq ===
                receipt.roomHead.room_seq),
      message:
        receipt.state === "rejected" &&
        receipt.code === "stale_room_state"
          ? "The Room advanced. Reconnect to synchronize before acting."
          : null,
    });
    if (
      receipt.state === "rejected" &&
      receipt.code === "stale_room_state"
    ) {
      this.failConnection(
        "The Room advanced. Reconnect to synchronize before acting.",
      );
    }
  }

  private requireTarget(bodyValue: unknown): BoundTarget {
    const target = this.target;
    const body = record(bodyValue);
    if (
      target === null ||
      body.room_id !== target.roomId ||
      body.member_id !== target.memberId
    ) {
      throw new Error("Realtime delivery target changed.");
    }
    return target;
  }

  private maybeSendSyncAcknowledgement(): void {
    const sync = this.sync;
    if (sync === null || sync.acknowledgementRequestId !== null) return;
    const ready =
      sync.kind === "projection_reset"
        ? sync.resetInstalled
        : this.lastReceivedFrameSeq !== null &&
          this.lastReceivedFrameSeq >= sync.through;
    if (!ready) return;
    const requestId = this.nextMessageId();
    sync.acknowledgementRequestId = requestId;
    this.send(
      encodeHostedRequest("room.sync_ack", requestId, {
        through_frame_head: sync.through,
        sync_token: sync.token,
      }),
    );
  }

  private queueObservationAcknowledgement(through: number): void {
    if (this.observationAck !== null) {
      this.pendingObservationAckThrough = Math.max(
        this.pendingObservationAckThrough ?? through,
        through,
      );
      return;
    }
    if (through <= (this.current.lastAcknowledgedFrameSeq ?? -1)) return;
    const requestId = this.nextMessageId();
    this.observationAck = { requestId, through };
    this.send(
      encodeHostedRequest("observation.ack", requestId, {
        through_frame_seq: through,
      }),
    );
  }

  private startHeartbeat(): void {
    this.clearHeartbeat();
    const check = () => {
      if (
        this.current.status === "closed" ||
        this.current.status === "disconnected" ||
        this.current.status === "setup_required"
      ) {
        return;
      }
      if (
        Date.now() - this.lastInboundAt >
        this.heartbeatIntervalMs + this.heartbeatGraceMs
      ) {
        this.failConnection("Realtime heartbeat timed out. Reconnect to continue.");
        return;
      }
      this.heartbeatTimer = setTimeout(check, this.heartbeatIntervalMs);
    };
    this.heartbeatTimer = setTimeout(check, this.heartbeatIntervalMs);
  }

  private clearHeartbeat(): void {
    if (this.heartbeatTimer !== null) clearTimeout(this.heartbeatTimer);
    this.heartbeatTimer = null;
  }

  private send(message: string): void {
    if (this.socket === null || this.socket.readyState !== 1) {
      throw new Error("Realtime socket is not open.");
    }
    this.socket.send(message);
  }

  private nextMessageId(): string {
    const value = this.createMessageId();
    if (!ULID_PATTERN.test(value)) {
      throw new Error("Activity Client message identity is invalid.");
    }
    return value;
  }

  private applyAuthorityError(error: unknown): HostedLiveSessionSnapshot {
    if (
      error instanceof ActivityClientHandoffError &&
      error.nextAction === "return_to_task_setup"
    ) {
      return this.requireSetup(error.message);
    }
    this.failConnection(
      error instanceof ActivityClientHandoffError
        ? error.message
        : "Activity Client session is unavailable.",
    );
    return this.current;
  }

  private requireSetup(message: string): HostedLiveSessionSnapshot {
    this.generation += 1;
    this.clearHeartbeat();
    this.closeSocket();
    this.rejectPendingAction(new Error("Activity Client authority is no longer valid."));
    this.target = null;
    this.sync = null;
    this.retainedPackDigest = null;
    this.setState({
      ...initialSnapshot(),
      status: "setup_required",
      message,
    });
    return this.current;
  }

  private failConnection(message: string): void {
    if (
      this.current.status === "closed" ||
      this.current.status === "setup_required"
    ) {
      return;
    }
    this.clearHeartbeat();
    this.closeSocket();
    this.rejectPendingAction(new Error(message));
    this.target = null;
    this.sync = null;
    this.observationAck = null;
    this.pendingObservationAckThrough = null;
    this.update({
      status: "disconnected",
      synchronized: false,
      canAct: false,
      message,
    });
  }

  private closeSocket(): void {
    const socket = this.socket;
    this.socket = null;
    if (socket === null) return;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onerror = null;
    socket.onclose = null;
    try {
      socket.close(1000, "client connection closed");
    } catch {
      // A detached adapter cannot retain controller authority.
    }
  }

  private rejectPendingAction(error: Error): void {
    const pending = this.pendingAction;
    this.pendingAction = null;
    if (pending === null) return;
    clearTimeout(pending.timer);
    pending.reject(error);
  }

  private update(
    patch: Partial<HostedLiveSessionSnapshot>,
  ): HostedLiveSessionSnapshot {
    return this.setState({ ...this.current, ...patch });
  }

  private setState(
    snapshot: HostedLiveSessionSnapshot,
  ): HostedLiveSessionSnapshot {
    this.current = snapshot;
    for (const listener of this.listeners) {
      try {
        listener(snapshot);
      } catch {
        // Consumer presentation failures do not mutate transport authority.
      }
    }
    return snapshot;
  }
}

/** Structural helper for the concrete cookie-backed authority client. */
export function hostedLiveAuthority(
  client: Pick<
    ActivityClientHandoffClient,
    "redeem" | "resume" | "issueStreamTicket"
  >,
): HostedLiveSessionAuthority {
  return client;
}

function initialSnapshot(): HostedLiveSessionSnapshot {
  return {
    status: "idle",
    synchronized: false,
    canAct: false,
    deliveryBatch: null,
    lastAcknowledgedFrameSeq: null,
    actionReceipt: null,
    message: null,
  };
}

function syncFrom(value: unknown, tokenValue: unknown): SyncState | null {
  const source = recordOrNull(value);
  if (source === null || !boundedText(tokenValue, 4_096)) return null;
  if (source.kind === "projection_reset") {
    const through = exactInteger(source.baseline_frame_head);
    return through === null
      ? null
      : {
          kind: "projection_reset",
          through,
          cursorExclusive: null,
          token: tokenValue,
          resetInstalled: false,
          acknowledgementRequestId: null,
          acknowledged: false,
        };
  }
  if (source.kind === "retained_frames") {
    const cursorExclusive = exactInteger(source.cursor_exclusive);
    const through = exactInteger(source.through_frame_head);
    return cursorExclusive === null || through === null || through < cursorExclusive
      ? null
      : {
          kind: "retained_frames",
          through,
          cursorExclusive,
          token: tokenValue,
          resetInstalled: false,
          acknowledgementRequestId: null,
          acknowledged: false,
        };
  }
  return null;
}

function packFrom(
  value: unknown,
): AuthorizedRoomDeliveryBatch["pack"] | null {
  const source = recordOrNull(value);
  if (
    source === null ||
    !exactKeys(source, ["id", "version", "digest"]) ||
    !boundedText(source.id, 256) ||
    !boundedText(source.version, 128) ||
    typeof source.digest !== "string" ||
    !DIGEST_PATTERN.test(source.digest)
  ) {
    return null;
  }
  return { id: source.id, version: source.version, digest: source.digest };
}

function roomHeadFrom(
  value: unknown,
  expectedRoomId: string | null,
): ActivityClientRoomHead | null {
  const source = recordOrNull(value);
  if (
    source === null ||
    !exactKeys(source, [
      "room_id",
      "room_seq",
      "genesis_or_transition_hash",
      "core_schema_version",
      "pack_digest",
      "core_state_hash",
      "activity_state_hash",
      "authoritative_state_hash",
    ]) ||
    exactUlid(source.room_id) === null ||
    (expectedRoomId !== null && source.room_id !== expectedRoomId) ||
    exactInteger(source.room_seq) === null ||
    !boundedText(source.core_schema_version, 128)
  ) {
    return null;
  }
  for (const key of [
    "genesis_or_transition_hash",
    "pack_digest",
    "core_state_hash",
    "activity_state_hash",
    "authoritative_state_hash",
  ] as const) {
    if (typeof source[key] !== "string" || !DIGEST_PATTERN.test(source[key])) {
      return null;
    }
  }
  return {
    room_seq: source.room_seq as number,
    genesis_or_transition_hash: source.genesis_or_transition_hash as string,
    core_schema_version: source.core_schema_version,
    pack_digest: source.pack_digest as string,
    core_state_hash: source.core_state_hash as string,
    activity_state_hash: source.activity_state_hash as string,
    authoritative_state_hash: source.authoritative_state_hash as string,
  };
}

function withoutRouting(source: Record<string, unknown>): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(source)) {
    if (key === "room_id" || key === "member_id" || key === "principal_id") {
      continue;
    }
    if (key === "room_head") {
      const head = recordOrNull(value);
      if (head !== null) {
        const { room_id: _roomId, ...safeHead } = head;
        result[key] = safeHead;
      }
      continue;
    }
    result[key] = value;
  }
  return result;
}

function encodeHostedRequest(
  type:
    | "room.sync_ack"
    | "observation.ack"
    | "action.submit"
    | "client.pong",
  messageId: string,
  body: Record<string, JsonValue>,
): string {
  const value = JSON.stringify({
    protocol: WORLDSTREAM_PROTOCOL,
    type,
    message_id: messageId,
    request_id: messageId,
    body,
  });
  if (new TextEncoder().encode(value).byteLength > MAX_MESSAGE_BYTES) {
    throw new Error("Hosted protocol message is too large.");
  }
  return value;
}

function exactHostedStreamUrl(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new TypeError("Hosted stream URL is invalid.");
  }
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (
    url.pathname !== STREAM_PATH ||
    url.search !== "" ||
    url.hash !== "" ||
    url.username !== "" ||
    url.password !== "" ||
    (loopback ? !["ws:", "wss:"].includes(url.protocol) : url.protocol !== "wss:")
  ) {
    throw new TypeError("Hosted stream URL must be the fixed direct Fly path.");
  }
  return url.toString();
}

function record(value: unknown): Record<string, unknown> {
  const result = recordOrNull(value);
  if (result === null) throw new Error("Expected protocol object.");
  return result;
}

function recordOrNull(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function exactKeys(
  value: Record<string, unknown>,
  keys: readonly string[],
): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

function exactUlid(value: unknown): string | null {
  return typeof value === "string" && ULID_PATTERN.test(value) ? value : null;
}

function exactInteger(value: unknown): number | null {
  return safeInteger(value) ? value : null;
}

function nullableInteger(value: unknown): number | null | undefined {
  return value === null ? null : exactInteger(value) ?? undefined;
}

function safeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function boundedText(value: unknown, maximum: number): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    new TextEncoder().encode(value).byteLength <= maximum
  );
}

function isJsonValue(value: unknown, depth: number): value is JsonValue {
  if (depth > 32) return false;
  if (
    value === null ||
    typeof value === "boolean" ||
    typeof value === "string"
  ) {
    return typeof value !== "string" || value.length <= 16_384;
  }
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) {
    return value.every((item) => isJsonValue(item, depth + 1));
  }
  const object = recordOrNull(value);
  return (
    object !== null &&
    Object.values(object).every((item) => isJsonValue(item, depth + 1))
  );
}

function boundedTimeout(
  value: number | undefined,
  fallback: number,
  maximum: number,
): number {
  if (value === undefined) return fallback;
  if (!Number.isSafeInteger(value) || value <= 0 || value > maximum) {
    throw new TypeError("Hosted live-session timeout is outside its bound.");
  }
  return value;
}

function abortError(): Error {
  const error = new Error("Hosted live-session wait was aborted.");
  error.name = "AbortError";
  return error;
}

function asError(error: unknown, fallback: string): Error {
  return error instanceof Error ? error : new Error(fallback);
}

function createUlid(): string {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[0] = (bytes[0] ?? 0) & 0x3f;
  let value = bytes.reduce(
    (result, byte) => (result << 8n) | BigInt(byte),
    0n,
  );
  let encoded = "";
  for (let index = 0; index < 26; index += 1) {
    encoded = alphabet[Number(value & 31n)] + encoded;
    value >>= 5n;
  }
  return encoded;
}
