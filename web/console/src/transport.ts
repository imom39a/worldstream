/** Browser-side boundary for the WorldStream 0.1 WebSocket protocol. */

export const WORLDSTREAM_PROTOCOL = "0.1" as const;
export const MAX_MESSAGE_BYTES = 524_288;
const MAX_STRING_BYTES = 16_384;
const MAX_JSON_DEPTH = 32;

const ULID_PATTERN = /^[0-9A-HJKMNP-TV-Z]{26}$/;

export type JsonPrimitive = boolean | number | string | null;
export type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
export type JsonObject = { [key: string]: JsonValue };

export type PrincipalKind = "human" | "agent";
export type ClientMode = "participant" | "spectator" | "operator" | "runner";

export const WORLDSTREAM_WS_SUBPROTOCOL = "worldstream.json.v0.1";
export const BROWSER_WS_TICKET_VERSION = "browser_ws_ticket.v1" as const;
const BROWSER_TICKET_PATTERN = /^wst1:[0-9a-f]{64}$/;
const REQUIRED_CAPABILITIES = ["cursor_ack", "projection_reset"] as const;

export interface ClientHelloBody {
  client_name: string;
  client_version: string;
  mode: ClientMode;
  supported_protocols: typeof WORLDSTREAM_PROTOCOL[];
  capabilities: string[];
}

export interface AuthenticatedPrincipal {
  principal_id: string;
  kind: PrincipalKind;
}

export interface ServerWelcomeBody {
  session_id: string;
  selected_protocol: typeof WORLDSTREAM_PROTOCOL;
  server_version: string;
  heartbeat_interval_ms: number;
  maximum_message_bytes: number;
  authenticated_principal: AuthenticatedPrincipal;
}

export interface RoomAttachBody {
  room_id: string;
  member_id: string;
  after_frame_seq?: number | null;
}

export interface RoomSyncAckBody {
  room_id: string;
  member_id: string;
  through_frame_head: number;
  sync_token: string;
}

export interface ObservationAckBody {
  room_id: string;
  member_id: string;
  through_frame_seq: number;
}

export interface ActionSubmitBody {
  room_id: string;
  member_id: string;
  action_id: string;
  based_on_room_seq: number;
  action_type: string;
  payload: JsonValue;
}

export type ServerBody = JsonValue;

export type MessageType =
  | "client.hello"
  | "server.welcome"
  | "room.attach"
  | "room.sync_ack"
  | "observation.ack"
  | "action.submit"
  | "room.attached"
  | "projection.reset"
  | "observation.deliver"
  | "observation.acked"
  | "room.sync_acked"
  | "action.accepted"
  | "action.rejected"
  | "error"
  | "server.ping"
  | "client.pong";

export interface ProtocolEnvelope<TType extends MessageType, TBody> {
  protocol: typeof WORLDSTREAM_PROTOCOL;
  type: TType;
  message_id: string;
  request_id?: string;
  body: TBody;
}

export type ClientHelloMessage = ProtocolEnvelope<"client.hello", ClientHelloBody>;
export type ServerWelcomeMessage = ProtocolEnvelope<"server.welcome", ServerWelcomeBody>;
export type RoomAttachMessage = ProtocolEnvelope<"room.attach", RoomAttachBody>;
export type RoomSyncAckMessage = ProtocolEnvelope<"room.sync_ack", RoomSyncAckBody>;
export type ObservationAckMessage = ProtocolEnvelope<"observation.ack", ObservationAckBody>;
export type ActionSubmitMessage = ProtocolEnvelope<"action.submit", ActionSubmitBody>;
export type ServerMessage = ProtocolEnvelope<
  Exclude<MessageType, ClientRequestMessage["type"] | "client.hello" | "server.welcome">,
  ServerBody
>;

export type ClientRequestMessage =
  | RoomAttachMessage
  | RoomSyncAckMessage
  | ObservationAckMessage
  | ActionSubmitMessage;

export type ProtocolMessage = ClientHelloMessage | ServerWelcomeMessage | ClientRequestMessage | ServerMessage;

export class ProtocolValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ProtocolValidationError";
  }
}

export function encodeMessage(message: ProtocolMessage): string {
  validateMessage(message);

  let encoded: string;
  try {
    encoded = JSON.stringify(message);
  } catch (error) {
    throw new ProtocolValidationError(`message is not JSON encodable: ${errorMessage(error)}`);
  }

  if (encoded === undefined) {
    throw new ProtocolValidationError("message is not JSON encodable");
  }
  assertMessageSize(encoded);
  return encoded;
}

export function decodeMessage(raw: string): ProtocolMessage {
  if (typeof raw !== "string") {
    throw new ProtocolValidationError("message must be a JSON string");
  }
  assertMessageSize(raw);

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw) as unknown;
  } catch (error) {
    throw new ProtocolValidationError(`message is not valid JSON: ${errorMessage(error)}`);
  }

  validateMessage(parsed);
  return parsed;
}

export function validateServerWelcome(message: unknown): ServerWelcomeMessage {
  validateEnvelope(message, "server.welcome", validateServerWelcomeBody);
  return message as ServerWelcomeMessage;
}

export function createRequest<TType extends ClientRequestMessage["type"]>(
  type: TType,
  message_id: string,
  body: Extract<ClientRequestMessage, { type: TType }>["body"],
  request_id = message_id,
): Extract<ClientRequestMessage, { type: TType }> {
  const message = {
    protocol: WORLDSTREAM_PROTOCOL,
    type,
    message_id,
    request_id,
    body,
  } as Extract<ClientRequestMessage, { type: TType }>;
  validateMessage(message);
  return message;
}

export interface ActionSubmitRequestInput {
  message_id: string;
  request_id?: string;
  room_id: string;
  member_id: string;
  action_id: string;
  based_on_room_seq: number;
  action_type: string;
  payload: JsonValue;
}

/** Build the only wire shape the participant action form may submit. */
export function createActionSubmitRequest(input: ActionSubmitRequestInput): ActionSubmitMessage {
  return createRequest(
    "action.submit",
    input.message_id,
    {
      room_id: input.room_id,
      member_id: input.member_id,
      action_id: input.action_id,
      based_on_room_seq: input.based_on_room_seq,
      action_type: input.action_type,
      payload: input.payload,
    },
    input.request_id ?? input.message_id,
  );
}

export interface WorldStreamTransport {
  readonly state: "idle" | "open" | "closed";
  connect(): Promise<ServerWelcomeMessage>;
  send(message: ClientRequestMessage): Promise<void>;
  subscribe(listener: (message: ProtocolMessage) => void): () => void;
  close(): void;
  /** Resolves when an underlying network connection has observed its close. */
  waitForClose?(): Promise<void>;
}

export interface WebSocketLike {
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent<unknown>) => void) | null;
  onerror: ((event: Event) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export interface WebSocketFactoryOptions {
  /** Header-capable adapters may use this for the protocol Authorization header. */
  readonly headers: Readonly<Record<string, string>>;
}

export type WebSocketFactory = (
  url: string,
  protocols: readonly string[],
  options: WebSocketFactoryOptions,
) => WebSocketLike;

export interface BrowserTicketFetchResponse {
  readonly ok: boolean;
  json(): Promise<unknown>;
}

export type BrowserTicketFetch = (
  input: string,
  init?: RequestInit,
) => Promise<BrowserTicketFetchResponse>;

export interface WebSocketWorldStreamTransportOptions {
  url: string;
  clientName: string;
  clientVersion: string;
  mode?: ClientMode;
  capabilities?: readonly string[];
  bearer?: string;
  ticketUrl?: string;
  fetch?: BrowserTicketFetch;
  webSocketFactory?: WebSocketFactory;
  heartbeatGraceMs?: number;
  createMessageId?: () => string;
}

/**
 * A bounded browser WebSocket transport. Native browser connections obtain a
 * short-lived admission ticket over authenticated HTTP, then send only that
 * ticket as the first WebSocket frame. Header-capable adapters retain the
 * direct Authorization path for SDKs and extensions. Neither path puts the
 * bearer in the URL, subprotocol, error, close reason, or DOM.
 */
export class WebSocketWorldStreamTransport implements WorldStreamTransport {
  private currentState: WorldStreamTransport["state"] = "idle";
  private readonly listeners = new Set<(message: ProtocolMessage) => void>();
  private readonly options: Required<Pick<WebSocketWorldStreamTransportOptions, "mode" | "heartbeatGraceMs">> & Omit<WebSocketWorldStreamTransportOptions, "bearer">;
  private bearer: string | undefined;
  private admissionTicket: string | undefined;
  private socket: WebSocketLike | null = null;
  private welcome: ServerWelcomeMessage | null = null;
  private connectPromise: Promise<ServerWelcomeMessage> | null = null;
  private resolveConnect: ((welcome: ServerWelcomeMessage) => void) | null = null;
  private rejectConnect: ((error: Error) => void) | null = null;
  private heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
  private closePromise: Promise<void> = Promise.resolve();
  private resolveClose: (() => void) | null = null;
  private lastInboundAt = 0;
  private negotiatedMessageBytes = MAX_MESSAGE_BYTES;

  constructor(options: WebSocketWorldStreamTransportOptions) {
    if (!/^wss?:\/\//.test(options.url)) {
      throw new Error("WebSocket URL must use ws:// or wss://");
    }
    const { bearer, ...safeOptions } = options;
    this.bearer = bearer;
    this.options = { mode: "participant", heartbeatGraceMs: 5_000, ...safeOptions };
  }

  get state(): WorldStreamTransport["state"] {
    return this.currentState;
  }

  connect(): Promise<ServerWelcomeMessage> {
    if (this.currentState === "closed") return Promise.reject(new Error("transport is closed"));
    if (this.currentState === "open" && this.welcome) return Promise.resolve(this.welcome);
    if (this.connectPromise) return this.connectPromise;
    this.connectPromise = new Promise<ServerWelcomeMessage>((resolve, reject) => {
      this.resolveConnect = resolve;
      this.rejectConnect = reject;
      void this.establishConnection();
    });
    return this.connectPromise;
  }

  send(message: ClientRequestMessage): Promise<void> {
    if (this.currentState !== "open" || !this.socket) return Promise.reject(new Error("transport is not open"));
    try {
      const encoded = encodeMessage(message);
      assertMessageSize(encoded, this.negotiatedMessageBytes);
      this.socket.send(encoded);
      return Promise.resolve();
    } catch (error) {
      return Promise.reject(error);
    }
  }

  subscribe(listener: (message: ProtocolMessage) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  close(): void {
    if (this.currentState === "closed") return;
    this.beginCloseWait();
    this.currentState = "closed";
    this.clearHeartbeat();
    this.rejectConnect?.(new Error("transport closed"));
    this.rejectConnect = null;
    this.resolveConnect = null;
    this.connectPromise = null;
    this.bearer = undefined;
    this.admissionTicket = undefined;
    if (this.socket !== null) this.socket.close(1000, "client closed");
    else this.finishClose();
    this.socket = null;
    this.listeners.clear();
  }

  waitForClose(): Promise<void> {
    return this.closePromise;
  }

  private createHello(): ClientHelloMessage {
    const capabilities = [...new Set([...(this.options.capabilities ?? []), ...REQUIRED_CAPABILITIES])];
    return {
      protocol: WORLDSTREAM_PROTOCOL,
      type: "client.hello",
      message_id: this.messageId(),
      body: {
        client_name: this.options.clientName,
        client_version: this.options.clientVersion,
        mode: this.options.mode,
        supported_protocols: [WORLDSTREAM_PROTOCOL],
        capabilities,
      },
    };
  }

  private async establishConnection(): Promise<void> {
    try {
      const factory = this.options.webSocketFactory ?? ((url, protocols) => new WebSocket(url, [...protocols]));
      let admissionTicket: string | undefined;
      let headers: Readonly<Record<string, string>> = {};
      if (this.options.webSocketFactory) {
        const bearer = this.bearer;
        this.bearer = undefined;
        headers = bearer ? { Authorization: `Bearer ${bearer}` } : {};
      } else {
        admissionTicket = await this.issueBrowserTicket();
        this.admissionTicket = admissionTicket;
      }
      const hello = this.createHello();
      this.socket = factory(this.options.url, [WORLDSTREAM_WS_SUBPROTOCOL], { headers });
      this.socket.onopen = () => {
        try {
          if (admissionTicket !== undefined) this.socket?.send(admissionTicket);
          this.socket?.send(encodeMessage(hello));
        } catch (error) {
          this.fail(errorMessage(error));
        }
      };
      this.socket.onmessage = (event) => this.handleMessage(event.data);
      this.socket.onerror = () => this.fail("WebSocket connection failed");
      this.socket.onclose = () => {
        this.finishClose();
        this.fail("WebSocket connection closed");
      };
    } catch {
      this.fail("browser authorization failed");
    }
  }

  private async issueBrowserTicket(): Promise<string> {
    const bearer = this.bearer;
    this.bearer = undefined;
    if (!bearer) throw new Error("browser authorization failed");
    const ticketUrl = this.options.ticketUrl ?? deriveBrowserTicketUrl(this.options.url);
    const fetcher = this.options.fetch ?? fetch;
    try {
      const response = await fetcher(ticketUrl, {
        method: "POST",
        headers: {
          Accept: "application/json",
          Authorization: `Bearer ${bearer}`,
        },
        cache: "no-store",
        credentials: "omit",
      });
      if (!response.ok) throw new Error("browser authorization failed");
      const body = await response.json();
      if (!isBrowserTicketResponse(body)) throw new Error("browser authorization failed");
      return body.ticket;
    } catch {
      throw new Error("browser authorization failed");
    }
  }

  private handleMessage(raw: unknown): void {
    if (typeof raw !== "string") {
      this.fail("WebSocket delivered a non-text protocol message");
      return;
    }
    try {
      assertMessageSize(raw, this.negotiatedMessageBytes);
      const message = decodeMessage(raw);
      this.lastInboundAt = Date.now();
      if (message.type === "error" && !this.welcome) {
        this.fail("server rejected the WebSocket handshake");
        return;
      }
      if (message.type === "server.welcome") {
        if (this.currentState === "closed") return;
        this.currentState = "open";
        this.welcome = message;
        this.negotiatedMessageBytes = message.body.maximum_message_bytes;
        this.resolveConnect?.(message);
        this.resolveConnect = null;
        this.rejectConnect = null;
        this.startHeartbeat(message.body.heartbeat_interval_ms);
      } else if (message.type === "server.ping") {
        this.sendPong();
      }
      for (const listener of this.listeners) listener(message);
    } catch (error) {
      this.fail(error instanceof ProtocolValidationError ? error.message : "invalid server message");
    }
  }

  private sendPong(): void {
    if (this.currentState !== "open" || !this.socket) return;
    const pong: ProtocolMessage = {
      protocol: WORLDSTREAM_PROTOCOL,
      type: "client.pong",
      message_id: this.messageId(),
      body: {},
    };
    try { this.socket.send(encodeMessage(pong)); } catch { this.fail("WebSocket heartbeat failed"); }
  }

  private startHeartbeat(intervalMs: number): void {
    this.clearHeartbeat();
    this.lastInboundAt = Date.now();
    const check = () => {
      if (this.currentState !== "open") return;
      if (Date.now() - this.lastInboundAt > intervalMs + this.options.heartbeatGraceMs) {
        this.fail("WebSocket heartbeat timed out");
        return;
      }
      this.heartbeatTimer = setTimeout(check, intervalMs);
    };
    this.heartbeatTimer = setTimeout(check, intervalMs);
  }

  private clearHeartbeat(): void {
    if (this.heartbeatTimer !== null) clearTimeout(this.heartbeatTimer);
    this.heartbeatTimer = null;
  }

  private fail(reason: string): void {
    if (this.currentState === "closed") return;
    this.beginCloseWait();
    this.currentState = "closed";
    this.clearHeartbeat();
    this.rejectConnect?.(new Error(reason));
    this.rejectConnect = null;
    this.resolveConnect = null;
    this.connectPromise = null;
    this.bearer = undefined;
    this.admissionTicket = undefined;
    if (this.socket !== null) this.socket.close(1000, "transport failure");
    else this.finishClose();
    this.socket = null;
  }

  private beginCloseWait(): void {
    if (this.resolveClose !== null) return;
    this.closePromise = new Promise<void>((resolve) => {
      this.resolveClose = resolve;
    });
  }

  private finishClose(): void {
    const resolve = this.resolveClose;
    this.resolveClose = null;
    resolve?.();
  }

  private messageId(): string {
    return this.options.createMessageId?.() ?? createUlid();
  }
}

function deriveBrowserTicketUrl(webSocketUrl: string): string {
  const url = new URL(webSocketUrl);
  url.protocol = url.protocol === "wss:" ? "https:" : "http:";
  url.pathname = url.pathname.endsWith("/stream")
    ? `${url.pathname}/ticket`
    : `${url.pathname.replace(/\/$/, "")}/ticket`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

function isBrowserTicketResponse(value: unknown): value is { ticket: string } {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const body = value as Record<string, unknown>;
  return body.version === BROWSER_WS_TICKET_VERSION
    && typeof body.ticket === "string"
    && BROWSER_TICKET_PATTERN.test(body.ticket)
    && Number.isSafeInteger(body.expires_in_ms)
    && (body.expires_in_ms as number) > 0;
}

/**
 * A deterministic transport seam for console tests and future UI wiring.
 * `receive` is intentionally explicit: tests or a future WebSocket adapter
 * control server messages without this class implying a live connection.
 */
export class InMemoryWorldStreamTransport implements WorldStreamTransport {
  private currentState: WorldStreamTransport["state"] = "idle";
  private readonly listeners = new Set<(message: ProtocolMessage) => void>();
  private readonly sent: ClientRequestMessage[] = [];
  private readonly welcome: ServerWelcomeMessage;

  constructor(welcome: ServerWelcomeMessage) {
    this.welcome = validateServerWelcome(welcome);
  }

  get state(): WorldStreamTransport["state"] {
    return this.currentState;
  }

  get sentMessages(): readonly ClientRequestMessage[] {
    return this.sent;
  }

  connect(): Promise<ServerWelcomeMessage> {
    if (this.currentState === "closed") {
      return Promise.reject(new Error("transport is closed"));
    }
    this.currentState = "open";
    return Promise.resolve(this.welcome);
  }

  send(message: ClientRequestMessage): Promise<void> {
    if (this.currentState !== "open") {
      return Promise.reject(new Error("transport is not open"));
    }
    const validated = decodeMessage(encodeMessage(message));
    if (!isClientRequestMessage(validated)) {
      return Promise.reject(new ProtocolValidationError("transport accepts client requests only"));
    }
    this.sent.push(validated);
    return Promise.resolve();
  }

  receive(message: ProtocolMessage): void {
    if (this.currentState !== "open") {
      throw new Error("transport is not open");
    }
    const validated = decodeMessage(encodeMessage(message));
    for (const listener of this.listeners) {
      listener(validated);
    }
  }

  subscribe(listener: (message: ProtocolMessage) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  close(): void {
    this.currentState = "closed";
    this.listeners.clear();
  }
}

function validateMessage(message: unknown): asserts message is ProtocolMessage {
  assertRecord(message, "message");
  assertExactKeys(message, ["protocol", "type", "message_id", "body"], ["request_id"]);
  assertLiteral(message.protocol, WORLDSTREAM_PROTOCOL, "protocol");
  assertMessageType(message.type);
  assertUlid(message.message_id, "message_id");
  if (message.request_id !== undefined) {
    assertUlid(message.request_id, "request_id");
  }

  switch (message.type) {
    case "client.hello":
      validateClientHelloBody(message.body);
      return;
    case "server.welcome":
      validateServerWelcomeBody(message.body);
      return;
    case "room.attach":
      validateEnvelopeBody(message.body, "room.attach", validateRoomAttachBody);
      return;
    case "room.sync_ack":
      validateEnvelopeBody(message.body, "room.sync_ack", validateRoomSyncAckBody);
      return;
    case "observation.ack":
      validateEnvelopeBody(message.body, "observation.ack", validateObservationAckBody);
      return;
    case "action.submit":
      validateEnvelopeBody(message.body, "action.submit", validateActionSubmitBody);
      return;
    case "room.attached":
    case "projection.reset":
    case "observation.deliver":
    case "observation.acked":
    case "room.sync_acked":
    case "action.accepted":
    case "action.rejected":
    case "error":
      validateServerBody(message.type, message.body);
      return;
    case "server.ping":
    case "client.pong":
      assertRecord(message.body, `${message.type}.body`);
      assertExactKeys(message.body, []);
      return;
  }
}

function validateEnvelope<TType extends MessageType>(
  message: unknown,
  expectedType: TType,
  validateBody: (body: unknown) => void,
): void {
  assertRecord(message, "message");
  assertExactKeys(message, ["protocol", "type", "message_id", "body"], ["request_id"]);
  assertLiteral(message.protocol, WORLDSTREAM_PROTOCOL, "protocol");
  assertLiteral(message.type, expectedType, "type");
  assertUlid(message.message_id, "message_id");
  if (message.request_id !== undefined) {
    assertUlid(message.request_id, "request_id");
  }
  validateBody(message.body);
}

function validateEnvelopeBody(
  body: unknown,
  messageType: ClientRequestMessage["type"],
  validateBody: (body: unknown) => void,
): void {
  try {
    validateBody(body);
  } catch (error) {
    throw new ProtocolValidationError(`${messageType}.body: ${errorMessage(error)}`);
  }
}

function validateClientHelloBody(body: unknown): asserts body is ClientHelloBody {
  assertRecord(body, "client.hello.body");
  assertExactKeys(body, ["client_name", "client_version", "mode", "supported_protocols", "capabilities"]);
  assertBoundedString(body.client_name, "client_name", 256);
  assertBoundedString(body.client_version, "client_version", 128);
  assertOneOf(body.mode, ["participant", "spectator", "operator", "runner"], "mode");
  assertStringArray(body.supported_protocols, "supported_protocols", 16);
  if (!body.supported_protocols.includes(WORLDSTREAM_PROTOCOL)) {
    throw new ProtocolValidationError("supported_protocols must include protocol 0.1");
  }
  assertStringArray(body.capabilities, "capabilities", 64);
}

function validateServerWelcomeBody(body: unknown): asserts body is ServerWelcomeBody {
  assertRecord(body, "server.welcome.body");
  assertExactKeys(body, [
    "session_id",
    "selected_protocol",
    "server_version",
    "heartbeat_interval_ms",
    "maximum_message_bytes",
    "authenticated_principal",
  ]);
  assertUlid(body.session_id, "session_id");
  assertLiteral(body.selected_protocol, WORLDSTREAM_PROTOCOL, "selected_protocol");
  assertBoundedString(body.server_version, "server_version", 128);
  assertIntegerInRange(body.heartbeat_interval_ms, 1, 86_400_000, "heartbeat_interval_ms");
  assertIntegerInRange(body.maximum_message_bytes, 1, MAX_MESSAGE_BYTES, "maximum_message_bytes");
  assertRecord(body.authenticated_principal, "authenticated_principal");
  assertExactKeys(body.authenticated_principal, ["principal_id", "kind"]);
  // Principal identifiers are protocol strings backed by the server's
  // authenticated-principal contract. They are not transport session IDs and
  // must not be narrowed to ULIDs at this boundary.
  assertBoundedString(body.authenticated_principal.principal_id, "authenticated_principal.principal_id", 512);
  assertOneOf(body.authenticated_principal.kind, ["human", "agent"], "authenticated_principal.kind");
}

function validateRoomAttachBody(body: unknown): asserts body is RoomAttachBody {
  assertRecord(body, "room.attach.body");
  assertExactKeys(body, ["room_id", "member_id"], ["after_frame_seq"]);
  assertUlid(body.room_id, "room_id");
  assertUlid(body.member_id, "member_id");
  if (body.after_frame_seq !== undefined && body.after_frame_seq !== null) {
    assertIntegerInRange(body.after_frame_seq, 0, Number.MAX_SAFE_INTEGER, "after_frame_seq");
  }
}

function validateRoomSyncAckBody(body: unknown): asserts body is RoomSyncAckBody {
  assertRecord(body, "room.sync_ack.body");
  assertExactKeys(body, ["room_id", "member_id", "through_frame_head", "sync_token"]);
  assertUlid(body.room_id, "room_id");
  assertUlid(body.member_id, "member_id");
  assertIntegerInRange(body.through_frame_head, 0, Number.MAX_SAFE_INTEGER, "through_frame_head");
  assertBoundedString(body.sync_token, "sync_token", 4_096);
}

function validateObservationAckBody(body: unknown): asserts body is ObservationAckBody {
  assertRecord(body, "observation.ack.body");
  assertExactKeys(body, ["room_id", "member_id", "through_frame_seq"]);
  assertUlid(body.room_id, "room_id");
  assertUlid(body.member_id, "member_id");
  assertIntegerInRange(body.through_frame_seq, 0, Number.MAX_SAFE_INTEGER, "through_frame_seq");
}

function validateActionSubmitBody(body: unknown): asserts body is ActionSubmitBody {
  assertRecord(body, "action.submit.body");
  assertExactKeys(body, ["room_id", "member_id", "action_id", "based_on_room_seq", "action_type", "payload"]);
  assertUlid(body.room_id, "room_id");
  assertUlid(body.member_id, "member_id");
  assertUlid(body.action_id, "action_id");
  assertIntegerInRange(body.based_on_room_seq, 0, Number.MAX_SAFE_INTEGER, "based_on_room_seq");
  assertBoundedString(body.action_type, "action_type", 256);
  validateJsonValue(body.payload, "payload", 0);
}

function assertMessageType(value: unknown): asserts value is MessageType {
  assertOneOf(
    value,
    [
      "client.hello", "server.welcome", "room.attach", "room.sync_ack", "observation.ack", "action.submit",
      "room.attached", "projection.reset", "observation.deliver", "observation.acked", "room.sync_acked",
      "action.accepted", "action.rejected", "error", "server.ping", "client.pong",
    ],
    "type",
  );
}

function validateServerBody(type: MessageType, body: unknown): void {
  assertRecord(body, `${type}.body`);
  const required: Record<string, readonly string[]> = {
    "room.attached": ["room_id", "member_id", "principal_kind", "access_mode", "membership_status", "room_status", "room_health", "integrity_generation", "room_head", "cursor", "frame_head", "retained_floor", "sync_token", "sync", "pack"],
    "projection.reset": ["room_id", "member_id", "room_head", "room_health", "integrity_generation", "baseline_frame_head", "reset_reason", "projection_schema", "projection", "projection_hash"],
    "observation.deliver": ["room_id", "member_id", "frame_seq", "cause_room_seq", "frame_kind", "observation_schema", "observation", "frame_payload_hash"],
    "observation.acked": ["room_id", "member_id", "cursor"],
    "room.sync_acked": ["through_frame_head"],
    "action.accepted": ["room_id", "member_id", "action_id", "transition_id", "admitted_at", "room_head", "duplicate"],
    "action.rejected": ["room_id", "member_id", "action_id", "admitted_at", "code", "message", "current_room_seq", "action_offers", "retryable_with_same_action_id", "may_submit_revised_action", "duplicate", "details"],
    error: ["code", "message", "retryable"],
  };
  const optional = type === "room.attached" ? ["role"] : type === "error" ? ["details"] : [];
  assertExactKeys(body, required[type] ?? [], optional);
  for (const key of required[type] ?? []) {
    if (body[key] === null && ["cursor", "role"].includes(key)) continue;
    if (body[key] === undefined) throw new ProtocolValidationError(`${type}.body.${key} is required`);
  }
  for (const [key, value] of Object.entries(body)) {
    if (typeof value === "string") assertBoundedString(value, `${type}.body.${key}`, 16_384);
    else validateJsonValue(value, `${type}.body.${key}`, 0);
  }
}

function assertRecord(value: unknown, path: string): asserts value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new ProtocolValidationError(`${path} must be an object`);
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) {
    throw new ProtocolValidationError(`${path} must be a plain object`);
  }
}

function assertExactKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[] = [],
): void {
  const allowed = new Set([...required, ...optional]);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) {
      throw new ProtocolValidationError(`unknown field ${key}`);
    }
  }
  for (const key of required) {
    if (!(key in value)) {
      throw new ProtocolValidationError(`missing field ${key}`);
    }
  }
}

function assertLiteral<T>(value: unknown, expected: T, path: string): asserts value is T {
  if (value !== expected) {
    throw new ProtocolValidationError(`${path} must be ${String(expected)}`);
  }
}

function assertOneOf<T extends string>(value: unknown, values: readonly T[], path: string): asserts value is T {
  if (typeof value !== "string" || !values.includes(value as T)) {
    throw new ProtocolValidationError(`${path} is not supported`);
  }
}

function assertUlid(value: unknown, path: string): asserts value is string {
  if (typeof value !== "string" || !ULID_PATTERN.test(value)) {
    throw new ProtocolValidationError(`${path} must be a 26-character Crockford ULID`);
  }
}

function assertBoundedString(value: unknown, path: string, maximumBytes: number): asserts value is string {
  if (typeof value !== "string" || value.length === 0) {
    throw new ProtocolValidationError(`${path} must be a non-empty string`);
  }
  if (byteLength(value) > Math.min(maximumBytes, MAX_STRING_BYTES)) {
    throw new ProtocolValidationError(`${path} exceeds its size bound`);
  }
}

function assertStringArray(value: unknown, path: string, maximumItems: number): asserts value is string[] {
  if (!Array.isArray(value) || value.length > maximumItems) {
    throw new ProtocolValidationError(`${path} must be a bounded string array`);
  }
  value.forEach((item, index) => assertBoundedString(item, `${path}[${index}]`, 256));
}

function assertIntegerInRange(value: unknown, minimum: number, maximum: number, path: string): asserts value is number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum || value > maximum) {
    throw new ProtocolValidationError(`${path} must be an integer in range`);
  }
}

function validateJsonValue(value: unknown, path: string, depth: number): asserts value is JsonValue {
  if (depth > MAX_JSON_DEPTH) {
    throw new ProtocolValidationError(`${path} exceeds the JSON nesting bound`);
  }
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    if (typeof value === "string" && byteLength(value) > MAX_STRING_BYTES) {
      throw new ProtocolValidationError(`${path} exceeds the string size bound`);
    }
    return;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new ProtocolValidationError(`${path} must contain finite numbers`);
    }
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => validateJsonValue(item, `${path}[${index}]`, depth + 1));
    return;
  }
  assertRecord(value, path);
  for (const [key, child] of Object.entries(value)) {
    validateJsonValue(child, `${path}.${key}`, depth + 1);
  }
}

function assertMessageSize(raw: string, maximum = MAX_MESSAGE_BYTES): void {
  if (byteLength(raw) > maximum) {
    throw new ProtocolValidationError(`message exceeds ${maximum}-byte limit`);
  }
}

function byteLength(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "unknown validation error";
}

function createUlid(): string {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  const bytes = new Uint8Array(16);
  if (typeof crypto !== "undefined" && typeof crypto.getRandomValues === "function") crypto.getRandomValues(bytes);
  else for (let index = 0; index < bytes.length; index += 1) bytes[index] = Math.floor(Math.random() * 256);
  let value = "0";
  for (let index = 0; index < 25; index += 1) value += alphabet[bytes[index % bytes.length] % alphabet.length];
  return value;
}

function isClientRequestMessage(message: ProtocolMessage): message is ClientRequestMessage {
  return ["room.attach", "room.sync_ack", "observation.ack", "action.submit"].includes(message.type as ClientRequestMessage["type"]);
}
