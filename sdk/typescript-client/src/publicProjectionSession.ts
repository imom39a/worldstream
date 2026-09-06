import type {
  ActivityClientStartup,
  AuthorizedRoomDeliveryBatch,
} from "./browserHandoff";
import type {
  HostedLiveActionInput,
  HostedLiveActionReceipt,
  HostedLiveSessionSnapshot,
  HostedLiveWaitOptions,
} from "./hostedLiveSession";
import {
  MAX_MESSAGE_BYTES,
  type WebSocketFactory,
  type WebSocketLike,
} from "./transport";

export const PUBLIC_PROJECTION_WS_SUBPROTOCOL =
  "worldstream.public-projection.v1" as const;
export const PUBLIC_PROJECTION_STREAM_VERSION =
  "worldstream/public-projection-stream/v1" as const;

const PUBLIC_STREAM_PATH = /^\/v1\/hosted\/public-runs\/[0-9a-f]{32}\/stream$/u;
const DIGEST = /^(?:blake3|sha256):[0-9a-f]{64}$/u;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}$/u;
const FORBIDDEN_PUBLIC_KEYS = new Set([
  "account_id",
  "activity_run_id",
  "bearer",
  "credential",
  "entry_selector",
  "final_reveal",
  "handoff",
  "host_authority",
  "invitation_token",
  "member_id",
  "membership_id",
  "principal_id",
  "provider_response",
  "replay",
  "room_id",
  "runner_authority",
  "secret_ref",
  "secret_reference",
  "service_scope_digest",
  "ticket",
  "token_hash",
]);
const MAX_WAITERS = 32;
const DEFAULT_CONNECT_TIMEOUT_MS = 20_000;

export interface PublicProjectionSessionControllerOptions {
  readonly streamUrl: string;
  readonly webSocketFactory?: WebSocketFactory;
  readonly connectTimeoutMs?: number;
}

/**
 * Thin anonymous observer for the dedicated public Projection protocol.
 * It starts empty, owns no Room authority, sends no application messages, and
 * cannot submit Actions. Pack-specific interpretation remains in the client.
 */
export class PublicProjectionSessionController {
  private current: HostedLiveSessionSnapshot = initialSnapshot();
  private readonly listeners = new Set<
    (snapshot: HostedLiveSessionSnapshot) => void
  >();
  private readonly streamUrl: string;
  private readonly socketFactory: WebSocketFactory;
  private readonly connectTimeoutMs: number;
  private socket: WebSocketLike | null = null;
  private startTask: Promise<HostedLiveSessionSnapshot> | null = null;
  private reconnectTask: Promise<HostedLiveSessionSnapshot> | null = null;
  private generation = 0;
  private lastFrameHead: number | null = null;
  private lastRoomSequence: number | null = null;
  private packDigest: string | null = null;
  private activeWaiters = 0;

  constructor(options: PublicProjectionSessionControllerOptions) {
    this.streamUrl = exactPublicStreamUrl(options.streamUrl);
    this.socketFactory = options.webSocketFactory ??
      ((url, protocols) => new WebSocket(url, [...protocols]));
    this.connectTimeoutMs = boundedTimeout(options.connectTimeoutMs);
  }

  get state(): HostedLiveSessionSnapshot {
    return this.current;
  }

  subscribe(listener: (snapshot: HostedLiveSessionSnapshot) => void): () => void {
    if (this.listeners.size >= 64) {
      throw new Error("Public Projection listener capacity is exhausted.");
    }
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  start(startup: ActivityClientStartup): Promise<HostedLiveSessionSnapshot> {
    if (startup.kind !== "direct") {
      this.setState({
        ...initialSnapshot(),
        status: "setup_required",
        message: "This public viewer does not accept participant authority.",
      });
      return Promise.resolve(this.current);
    }
    this.startTask ??= this.connect();
    return this.startTask;
  }

  reconnect(): Promise<HostedLiveSessionSnapshot> {
    if (this.current.status === "closed") {
      return Promise.reject(new Error("Public Projection session is closed."));
    }
    this.reconnectTask ??= this.connect().finally(() => {
      this.reconnectTask = null;
    });
    return this.reconnectTask;
  }

  submitAction(_input: HostedLiveActionInput): Promise<HostedLiveActionReceipt> {
    return Promise.reject(
      new Error("Anonymous public Projection sessions cannot submit Actions."),
    );
  }

  waitFor(
    predicate: (snapshot: HostedLiveSessionSnapshot) => boolean,
    options: HostedLiveWaitOptions = {},
  ): Promise<HostedLiveSessionSnapshot> {
    if (predicate(this.current)) return Promise.resolve(this.current);
    if (this.activeWaiters >= MAX_WAITERS) {
      return Promise.reject(new Error("Public Projection wait capacity is exhausted."));
    }
    const timeoutMs = boundedTimeout(options.timeoutMs, 30_000);
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
        () => finish({ error: new Error("Public Projection wait timed out.") }),
        timeoutMs,
      );
      if (options.signal?.aborted === true) abort();
      else options.signal?.addEventListener("abort", abort, { once: true });
    });
  }

  close(): void {
    if (this.current.status === "closed") return;
    this.generation += 1;
    this.closeSocket();
    this.resetSequence();
    this.setState({
      ...initialSnapshot(),
      status: "closed",
      message: "Public Projection session closed.",
    });
    this.listeners.clear();
  }

  private connect(): Promise<HostedLiveSessionSnapshot> {
    const generation = this.generation + 1;
    this.generation = generation;
    this.closeSocket();
    this.resetSequence();
    this.setState({
      ...initialSnapshot(),
      status: "connecting",
      message: "Connecting to the public Projection…",
    });

    return new Promise<HostedLiveSessionSnapshot>((resolve) => {
      let settled = false;
      const settle = () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(this.current);
      };
      const fail = () => {
        if (generation !== this.generation) return;
        this.failConnection();
        settle();
      };
      const timer = setTimeout(fail, this.connectTimeoutMs);
      let socket: WebSocketLike;
      try {
        socket = this.socketFactory(
          this.streamUrl,
          [PUBLIC_PROJECTION_WS_SUBPROTOCOL],
          { headers: {} },
        );
      } catch {
        fail();
        return;
      }
      this.socket = socket;
      socket.onopen = () => {
        if (generation !== this.generation || socket !== this.socket) return;
        this.update({
          status: "synchronizing",
          message: "Installing the authorized public Projection Reset…",
        });
      };
      socket.onmessage = (event) => {
        if (generation !== this.generation || socket !== this.socket) return;
        try {
          const batch = readPublicProjectionFrame(event.data);
          this.install(batch);
          if (this.current.status === "live") settle();
        } catch {
          fail();
        }
      };
      socket.onerror = fail;
      socket.onclose = fail;
    });
  }

  private install(batch: AuthorizedRoomDeliveryBatch): void {
    const delivery = batch.delivery[0];
    if (delivery === undefined) throw new Error("Public delivery is missing.");
    if (this.lastFrameHead === null) {
      if (delivery.kind !== "projection_reset") {
        throw new Error("Public stream did not begin with a Projection Reset.");
      }
      this.packDigest = batch.pack.digest;
      this.lastFrameHead = batch.frame_head;
      this.lastRoomSequence = batch.room_head.room_seq;
    } else {
      if (
        delivery.kind !== "observation" ||
        batch.pack.digest !== this.packDigest ||
        batch.frame_head !== this.lastFrameHead + 1 ||
        batch.room_head.room_seq < (this.lastRoomSequence ?? 0)
      ) {
        throw new Error("Public stream order or Pack identity changed.");
      }
      this.lastFrameHead = batch.frame_head;
      this.lastRoomSequence = batch.room_head.room_seq;
    }
    this.update({
      status: "live",
      synchronized: true,
      canAct: false,
      deliveryBatch: batch,
      actionReceipt: null,
      message: null,
    });
  }

  private failConnection(): void {
    if (this.current.status === "closed" || this.current.status === "setup_required") return;
    this.closeSocket();
    this.resetSequence();
    this.update({
      status: "disconnected",
      synchronized: false,
      canAct: false,
      deliveryBatch: null,
      message: "The public Projection is unavailable. Reconnect to try again.",
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
      socket.close(1000, "public viewer closed");
    } catch {
      // A detached browser transport cannot retain authority or state.
    }
  }

  private resetSequence(): void {
    this.lastFrameHead = null;
    this.lastRoomSequence = null;
    this.packDigest = null;
  }

  private update(patch: Partial<HostedLiveSessionSnapshot>): void {
    this.setState({ ...this.current, ...patch });
  }

  private setState(snapshot: HostedLiveSessionSnapshot): void {
    this.current = snapshot;
    for (const listener of this.listeners) {
      try {
        listener(snapshot);
      } catch {
        // Presentation failures cannot mutate the stream controller.
      }
    }
  }
}

function readPublicProjectionFrame(raw: unknown): AuthorizedRoomDeliveryBatch {
  if (typeof raw !== "string" || new TextEncoder().encode(raw).byteLength > MAX_MESSAGE_BYTES) {
    throw new Error("Public Projection frame is invalid.");
  }
  const parsed = JSON.parse(raw) as unknown;
  rejectForbiddenKeys(parsed, 0);
  const envelope = exactRecord(parsed, ["version", "batch"]);
  if (envelope.version !== PUBLIC_PROJECTION_STREAM_VERSION) {
    throw new Error("Public Projection version is unsupported.");
  }
  const batch = exactRecord(envelope.batch, [
    "pack", "room_head", "frame_head", "delivery",
  ]);
  const pack = readPack(batch.pack);
  const roomHead = readRoomHead(batch.room_head);
  const frameHead = integer(batch.frame_head);
  if (
    pack.digest !== roomHead.pack_digest ||
    !Array.isArray(batch.delivery) ||
    batch.delivery.length !== 1
  ) {
    throw new Error("Public Projection batch is invalid.");
  }
  const wireDelivery = exactRecord(batch.delivery[0], ["kind", "body"]);
  let delivery: AuthorizedRoomDeliveryBatch["delivery"][number];
  if (wireDelivery.kind === "projection_reset") {
    delivery = { kind: "projection_reset", body: readReset(wireDelivery.body, roomHead, frameHead) };
  } else if (wireDelivery.kind === "observation") {
    delivery = { kind: "observation", body: readObservation(wireDelivery.body, roomHead, frameHead) };
  } else {
    throw new Error("Public Projection delivery kind is invalid.");
  }
  return { pack, room_head: roomHead, frame_head: frameHead, delivery: [delivery] };
}

function readPack(value: unknown): AuthorizedRoomDeliveryBatch["pack"] {
  const pack = exactRecord(value, ["id", "version", "digest"]);
  if (
    typeof pack.id !== "string" || !IDENTIFIER.test(pack.id) ||
    typeof pack.version !== "string" || pack.version.length === 0 || pack.version.length > 128 ||
    typeof pack.digest !== "string" || !DIGEST.test(pack.digest)
  ) throw new Error("Public Pack identity is invalid.");
  return { id: pack.id, version: pack.version, digest: pack.digest };
}

function readRoomHead(value: unknown): AuthorizedRoomDeliveryBatch["room_head"] {
  const head = exactRecord(value, [
    "room_seq", "genesis_or_transition_hash", "core_schema_version", "pack_digest",
    "core_state_hash", "activity_state_hash", "authoritative_state_hash",
  ]);
  const digests = [
    head.genesis_or_transition_hash,
    head.pack_digest,
    head.core_state_hash,
    head.activity_state_hash,
    head.authoritative_state_hash,
  ];
  if (
    typeof head.core_schema_version !== "string" ||
    head.core_schema_version.length === 0 || head.core_schema_version.length > 128 ||
    digests.some((value) => typeof value !== "string" || !DIGEST.test(value))
  ) throw new Error("Public Room Head is invalid.");
  return {
    room_seq: integer(head.room_seq),
    genesis_or_transition_hash: head.genesis_or_transition_hash as string,
    core_schema_version: head.core_schema_version,
    pack_digest: head.pack_digest as string,
    core_state_hash: head.core_state_hash as string,
    activity_state_hash: head.activity_state_hash as string,
    authoritative_state_hash: head.authoritative_state_hash as string,
  };
}

function readReset(
  value: unknown,
  head: AuthorizedRoomDeliveryBatch["room_head"],
  frameHead: number,
): Record<string, unknown> {
  const body = exactRecord(value, [
    "room_head", "room_health", "integrity_generation", "baseline_frame_head",
    "reset_reason", "projection_schema", "projection", "projection_hash",
  ]);
  const bodyHead = readRoomHead(body.room_head);
  const projection = exactRecord(body.projection, ["core", "activity", "action_offers"]);
  const core = record(projection.core);
  if (
    JSON.stringify(bodyHead) !== JSON.stringify(head) ||
    integer(body.baseline_frame_head) !== frameHead ||
    integer(body.integrity_generation) < 0 ||
    !boundedString(body.room_health, 128) ||
    !boundedString(body.reset_reason, 128) ||
    !boundedString(body.projection_schema, 256) ||
    typeof body.projection_hash !== "string" || !DIGEST.test(body.projection_hash) ||
    core.access_mode !== "spectator" || core.viewer_class !== "public" ||
    core.standing !== "enabled" || core.role !== null ||
    !Array.isArray(projection.action_offers) || projection.action_offers.length !== 0
  ) throw new Error("Public Projection Reset is invalid.");
  return body;
}

function readObservation(
  value: unknown,
  head: AuthorizedRoomDeliveryBatch["room_head"],
  frameHead: number,
): Record<string, unknown> {
  const body = exactRecord(value, [
    "frame_seq", "cause_room_seq", "frame_kind", "observation_schema",
    "observation", "frame_payload_hash",
  ]);
  const observation = record(body.observation);
  if (
    integer(body.frame_seq) !== frameHead ||
    integer(body.cause_room_seq) !== head.room_seq ||
    !boundedString(body.frame_kind, 128) ||
    !boundedString(body.observation_schema, 256) ||
    typeof body.frame_payload_hash !== "string" || !DIGEST.test(body.frame_payload_hash) ||
    (Object.prototype.hasOwnProperty.call(observation, "action_offers") &&
      (!Array.isArray(observation.action_offers) || observation.action_offers.length !== 0))
  ) throw new Error("Public Observation is invalid.");
  return body;
}

function rejectForbiddenKeys(value: unknown, depth: number): void {
  if (depth > 32) throw new Error("Public Projection is too deeply nested.");
  if (Array.isArray(value)) {
    if (value.length > 512) throw new Error("Public Projection collection is too large.");
    for (const item of value) rejectForbiddenKeys(item, depth + 1);
    return;
  }
  if (value === null || typeof value !== "object") return;
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length > 256) throw new Error("Public Projection object is too large.");
  for (const [key, child] of entries) {
    if (FORBIDDEN_PUBLIC_KEYS.has(key)) throw new Error("Private routing material was rejected.");
    rejectForbiddenKeys(child, depth + 1);
  }
}

function exactRecord(value: unknown, keys: readonly string[]): Record<string, unknown> {
  const source = record(value);
  const present = Object.keys(source);
  if (present.length !== keys.length || keys.some((key) => !(key in source))) {
    throw new Error("Public Projection shape is invalid.");
  }
  return source;
}

function record(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Public Projection object is invalid.");
  }
  return value as Record<string, unknown>;
}

function integer(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new Error("Public Projection sequence is invalid.");
  }
  return value as number;
}

function boundedString(value: unknown, maximum: number): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= maximum &&
    !/[\u0000-\u001f\u007f]/u.test(value);
}

function exactPublicStreamUrl(value: string): string {
  const url = new URL(value);
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (
    url.username !== "" || url.password !== "" || url.search !== "" || url.hash !== "" ||
    !PUBLIC_STREAM_PATH.test(url.pathname) ||
    (loopback ? !["ws:", "wss:"].includes(url.protocol) : url.protocol !== "wss:") ||
    value !== url.toString()
  ) throw new Error("Public Projection stream URL is invalid.");
  return value;
}

function boundedTimeout(value: number | undefined, fallback = DEFAULT_CONNECT_TIMEOUT_MS): number {
  if (value === undefined) return fallback;
  if (!Number.isSafeInteger(value) || value < 1 || value > 60_000) {
    throw new Error("Public Projection timeout is invalid.");
  }
  return value;
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

function abortError(): Error {
  const error = new Error("Public Projection wait was aborted.");
  error.name = "AbortError";
  return error;
}
