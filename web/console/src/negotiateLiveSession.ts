import { useEffect, useMemo, useRef, useState } from "react";

import {
  createLiveSessionTransport,
  replaceLiveTransport,
  type LiveSessionConfig,
} from "./liveSession";
import {
  readNegotiateConsoleBootstrap,
  type NegotiateActionOffer,
  type NegotiateConsoleBootstrap,
} from "./negotiate";
import {
  createActionSubmitRequest,
  createRequest,
  type JsonObject,
  type JsonValue,
  type ProtocolMessage,
  type WorldStreamTransport,
} from "./transport";

export interface NegotiateActionReceipt {
  readonly state: "idle" | "submitting" | "accepted" | "rejected" | "stale";
  readonly action_id: string | null;
  readonly code: string | null;
  readonly message: string | null;
}

export interface NegotiateLiveView {
  readonly session: NegotiateConsoleBootstrap;
  readonly receipt: NegotiateActionReceipt;
  readonly error: string | null;
}

export interface NegotiateLiveController extends NegotiateLiveView {
  readonly submitPreparedAction: (
    offer: NegotiateActionOffer,
    payload: JsonValue,
  ) => Promise<void>;
  readonly reconnect: () => void;
}

interface SyncTracker {
  readonly kind: "retained_frames" | "projection_reset";
  readonly through: number;
  readonly cursorExclusive: number | null;
  readonly syncToken: string;
  receivedFrames: number[];
  resetInstalled: boolean;
  acknowledgementSent: boolean;
  syncAcknowledged: boolean;
  observationAckThrough: number | null;
}

const IDLE_RECEIPT: NegotiateActionReceipt = {
  state: "idle",
  action_id: null,
  code: null,
  message: null,
};

export function initialNegotiateLiveView(
  session: NegotiateConsoleBootstrap,
): NegotiateLiveView {
  return {
    session: { ...session, connection: "connecting", action_offers: [] },
    receipt: IDLE_RECEIPT,
    error: null,
  };
}

export function projectNegotiateMessage(
  current: NegotiateLiveView,
  message: ProtocolMessage,
  target: Pick<LiveSessionConfig, "roomId" | "memberId">,
): NegotiateLiveView {
  if (!messageTargetsSession(message, target)) {
    return closedError(current, "WorldStream returned data for another Room Membership.");
  }
  if (message.type === "server.welcome") {
    return updateConnection(current, "connecting", null);
  }
  if (message.type === "room.attached") {
    const body = objectBody(message.body);
    const pack = objectBody(body.pack);
    if (pack.id !== "worldstream.negotiate") {
      return closedError(current, "The attached Room is not a WorldStream Negotiate Room.");
    }
    const roomHead = objectBody(body.room_head);
    const catchingUp = updateConnection(current, "catching_up", null);
    return {
      ...catchingUp,
      session: {
        ...catchingUp.session,
        room_sequence: safeSequence(roomHead.room_seq, current.session.room_sequence),
      },
    };
  }
  if (message.type === "projection.reset") {
    const body = objectBody(message.body);
    const projectionEnvelope = objectBody(body.projection);
    return installAuthorizedProjection(
      updateRoomSequence(current, body),
      projectionEnvelope,
    );
  }
  if (message.type === "observation.deliver") {
    const body = objectBody(message.body);
    const installed = installAuthorizedProjection(
      updateRoomSequence(current, body),
      objectBody(body.observation),
    );
    return current.session.connection === "live"
      ? updateConnection(installed, "live", null)
      : installed;
  }
  if (message.type === "room.sync_acked") {
    return {
      ...updateConnection(current, "live", null),
      receipt:
        current.receipt.state === "stale"
          ? { ...IDLE_RECEIPT, message: "Current heads synchronized; prepare a new Action." }
          : current.receipt,
    };
  }
  if (message.type === "action.accepted") {
    const body = objectBody(message.body);
    if (body.action_id !== current.receipt.action_id) return current;
    const advanced = updateRoomSequence(current, body);
    return {
      ...advanced,
      session: { ...advanced.session, action_offers: [] },
      receipt: {
        state: "accepted",
        action_id: current.receipt.action_id,
        code: null,
        message:
          body.duplicate === true
            ? "Duplicate submission resolved to the original receipt."
            : "Action accepted at the authoritative Room Head.",
      },
    };
  }
  if (message.type === "action.rejected") {
    const body = objectBody(message.body);
    if (body.action_id !== current.receipt.action_id) return current;
    const currentSequence = safeSequence(
      body.current_room_seq,
      current.session.room_sequence,
    );
    const code = safeCode(body.code) ?? "action_rejected";
    const staleRoom =
      code === "stale_room_state" ||
      code === "stale_room_head" ||
      currentSequence !== current.session.room_sequence;
    const staleA202 = code === "stale_transaction_head" || code === "stale_session_head";
    const withOffers = Array.isArray(body.action_offers)
      ? installOffers(current, body.action_offers)
      : current;
    return {
      ...withOffers,
      session: {
        ...withOffers.session,
        connection: staleRoom ? "catching_up" : withOffers.session.connection,
        room_sequence: Math.max(withOffers.session.room_sequence, currentSequence),
      },
      receipt: {
        state: staleRoom || staleA202 ? "stale" : "rejected",
        action_id: current.receipt.action_id,
        code,
        message: staleRoom
          ? "The Action basis is stale. Reconnect, catch up, and prepare new signed bytes if an A202 head changed."
          : staleA202
            ? "The A202 logical predecessor changed. Rebuild and re-sign the protocol bytes; automatic rebasing is forbidden."
          : "The Pack declared a typed rejection; authoritative state did not change.",
      },
    };
  }
  if (message.type === "error") {
    return closedError(current, "The live WorldStream session reported a bounded error.");
  }
  return current;
}

export function useNegotiateLiveSession(
  initial: NegotiateConsoleBootstrap,
  config?: LiveSessionConfig,
  providedTransport?: WorldStreamTransport | null,
): NegotiateLiveController {
  const initialTransport = useMemo<WorldStreamTransport | null>(() => {
    if (providedTransport !== undefined) return providedTransport;
    if (config?.transportFactory) return config.transportFactory();
    return config ? createLiveSessionTransport(config) : null;
  }, [config, providedTransport]);
  const [transport, setTransport] = useState(initialTransport);
  const [view, setView] = useState<NegotiateLiveView>(() =>
    initialNegotiateLiveView(initial),
  );
  const generation = useRef(0);

  useEffect(() => {
    if (!config || !transport) return;
    const activeGeneration = generation.current + 1;
    generation.current = activeGeneration;
    let disposed = false;
    let sync: SyncTracker | null = null;

    const acknowledgeObservation = (frame: number) => {
      if (
        sync === null
        || !sync.syncAcknowledged
        || !Number.isSafeInteger(frame)
        || frame < 0
        || (sync.observationAckThrough !== null && frame <= sync.observationAckThrough)
      ) return;
      sync.observationAckThrough = frame;
      void transport.send(createRequest("observation.ack", nextId(), {
        room_id: config.roomId,
        member_id: config.memberId,
        through_frame_seq: frame,
      })).catch(() => {
        if (!disposed) {
          setView((current) =>
            closedError(current, "WorldStream observation acknowledgement failed."),
          );
        }
      });
    };

    const acknowledgeWhenReady = () => {
      if (!sync || sync.acknowledgementSent || !syncBatchReady(sync)) return;
      sync.acknowledgementSent = true;
      void transport
        .send(
          createRequest("room.sync_ack", nextId(), {
            room_id: config.roomId,
            member_id: config.memberId,
            through_frame_head: sync.through,
            sync_token: sync.syncToken,
          }),
        )
        .catch(() => {
          if (!disposed) {
            setView((current) =>
              closedError(current, "WorldStream synchronization acknowledgement failed."),
            );
          }
        });
    };

    const unsubscribe = transport.subscribe((message) => {
      if (disposed || generation.current !== activeGeneration) return;
      if (message.type === "room.attached") {
        sync = syncTrackerFromAttach(objectBody(message.body));
      } else if (message.type === "projection.reset" && sync?.kind === "projection_reset") {
        sync.resetInstalled = true;
      } else if (message.type === "observation.deliver" && sync !== null) {
        const frame = objectBody(message.body).frame_seq;
        if (typeof frame === "number" && Number.isSafeInteger(frame)) {
          sync.receivedFrames.push(frame);
          acknowledgeObservation(frame);
        }
      } else if (
        message.type === "room.sync_acked"
        && sync !== null
        && objectBody(message.body).through_frame_head === sync.through
      ) {
        sync.syncAcknowledged = true;
        const latest = latestNegotiateObservationFrameToAck(
          sync.receivedFrames,
          sync.syncAcknowledged,
        );
        if (latest !== null) acknowledgeObservation(latest);
      }
      setView((current) => projectNegotiateMessage(current, message, config));
      acknowledgeWhenReady();
    });

    void transport
      .connect()
      .then(() =>
        transport.send(
          createRequest("room.attach", nextId(), {
            room_id: config.roomId,
            member_id: config.memberId,
            after_frame_seq: null,
          }),
        ),
      )
      .catch(() => {
        if (!disposed) {
          setView((current) =>
            closedError(current, "The Negotiate Room could not be attached."),
          );
        }
      });

    return () => {
      disposed = true;
      unsubscribe();
      transport.close();
    };
  }, [config, transport]);

  return {
    ...view,
    async submitPreparedAction(offer, payload) {
      if (!config || !transport || view.session.connection !== "live") {
        throw new Error("Negotiate Actions require a synchronized live Session");
      }
      const currentOffer = view.session.action_offers.find(
        (candidate) =>
          candidate.action_type === offer.action_type &&
          candidate.payload_schema_digest === offer.payload_schema_digest,
      );
      if (!currentOffer) throw new Error("the exact Action Offer is no longer current");
      const actionId = nextId();
      setView((current) => ({
        ...current,
        receipt: {
          state: "submitting",
          action_id: actionId,
          code: null,
          message: "Submitting the prepared payload at the current exact Room Head.",
        },
      }));
      await transport.send(
        createActionSubmitRequest({
          message_id: nextId(),
          room_id: config.roomId,
          member_id: config.memberId,
          action_id: actionId,
          based_on_room_seq: view.session.room_sequence,
          action_type: offer.action_type,
          payload,
        }),
      );
    },
    reconnect() {
      if (!config?.transportFactory || !transport) {
        setView((current) =>
          closedError(current, "Reconnect authority is unavailable for this Session."),
        );
        return;
      }
      const replacement = replaceLiveTransport(transport, config.transportFactory);
      setView((current) => updateConnection(current, "connecting", null));
      setTransport(replacement);
    },
  };
}

function installAuthorizedProjection(
  current: NegotiateLiveView,
  envelope: JsonObject,
): NegotiateLiveView {
  const source = objectBody("observation" in envelope ? envelope.observation : envelope);
  const projection = objectBody(
    "activity" in source
      ? source.activity
      : "projection" in source
        ? source.projection
        : source,
  );
  const offerSource = Array.isArray(source.action_offers)
    ? source.action_offers
    : Array.isArray(envelope.action_offers)
      ? envelope.action_offers
      : Array.isArray(objectBody(envelope.projection).action_offers)
        ? objectBody(envelope.projection).action_offers
        : current.session.action_offers;
  const candidate = readNegotiateConsoleBootstrap({
    ...current.session,
    projection,
    action_offers: offerSource,
  });
  return candidate === null
    ? closedError(current, "The server returned an invalid Negotiate projection.")
    : { ...current, session: candidate, error: null };
}

function installOffers(
  current: NegotiateLiveView,
  offers: readonly JsonValue[],
): NegotiateLiveView {
  const candidate = readNegotiateConsoleBootstrap({
    ...current.session,
    action_offers: offers,
  });
  return candidate === null
    ? closedError(current, "The server returned invalid Negotiate Action Offers.")
    : { ...current, session: candidate };
}

function updateRoomSequence(
  current: NegotiateLiveView,
  body: JsonObject,
): NegotiateLiveView {
  const head = objectBody(body.room_head);
  const sequence = safeSequence(
    head.room_seq,
    safeSequence(body.cause_room_seq, current.session.room_sequence),
  );
  return { ...current, session: { ...current.session, room_sequence: sequence } };
}

function updateConnection(
  current: NegotiateLiveView,
  connection: NegotiateConsoleBootstrap["connection"],
  error: string | null,
): NegotiateLiveView {
  return { ...current, error, session: { ...current.session, connection } };
}

function closedError(current: NegotiateLiveView, error: string): NegotiateLiveView {
  return updateConnection(current, "disconnected", error);
}

function messageTargetsSession(
  message: ProtocolMessage,
  target: Pick<LiveSessionConfig, "roomId" | "memberId">,
): boolean {
  if (
    ![
      "room.attached",
      "projection.reset",
      "observation.deliver",
      "observation.acked",
      "action.accepted",
      "action.rejected",
    ].includes(message.type)
  ) {
    return true;
  }
  const body = objectBody(message.body);
  return body.room_id === target.roomId && body.member_id === target.memberId;
}

function syncTrackerFromAttach(body: JsonObject): SyncTracker | null {
  const branch = objectBody(body.sync);
  if (typeof body.sync_token !== "string") return null;
  if (
    branch.kind === "projection_reset" &&
    typeof branch.baseline_frame_head === "number" &&
    Number.isSafeInteger(branch.baseline_frame_head)
  ) {
    return {
      kind: "projection_reset",
      through: branch.baseline_frame_head,
      cursorExclusive: null,
      syncToken: body.sync_token,
      receivedFrames: [],
      resetInstalled: false,
      acknowledgementSent: false,
      syncAcknowledged: false,
      observationAckThrough: null,
    };
  }
  if (
    branch.kind === "retained_frames" &&
    typeof branch.cursor_exclusive === "number" &&
    Number.isSafeInteger(branch.cursor_exclusive) &&
    typeof branch.through_frame_head === "number" &&
    Number.isSafeInteger(branch.through_frame_head)
  ) {
    return {
      kind: "retained_frames",
      through: branch.through_frame_head,
      cursorExclusive: branch.cursor_exclusive,
      syncToken: body.sync_token,
      receivedFrames: [],
      resetInstalled: false,
      acknowledgementSent: false,
      syncAcknowledged: false,
      observationAckThrough: null,
    };
  }
  return null;
}

export function latestNegotiateObservationFrameToAck(
  frames: readonly number[],
  synchronized: boolean,
): number | null {
  if (!synchronized) return null;
  let latest: number | null = null;
  for (const frame of frames) {
    if (!Number.isSafeInteger(frame) || frame < 0) continue;
    latest = latest === null ? frame : Math.max(latest, frame);
  }
  return latest;
}

function syncBatchReady(sync: SyncTracker): boolean {
  if (sync.kind === "projection_reset") return sync.resetInstalled;
  const expected: number[] = [];
  for (let frame = (sync.cursorExclusive ?? -1) + 1; frame <= sync.through; frame += 1) {
    expected.push(frame);
  }
  const received = [...new Set(sync.receivedFrames)].sort((left, right) => left - right);
  return JSON.stringify(received) === JSON.stringify(expected);
}

function objectBody(value: unknown): JsonObject {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonObject)
    : {};
}

function safeSequence(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : fallback;
}

function safeCode(value: unknown): string | null {
  return typeof value === "string" && /^[a-z][a-z0-9_]{0,127}$/.test(value)
    ? value
    : null;
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const LIVE_ID_PREFIX = "01J0000000000000";
let sequence = 50_000;

function nextId(): string {
  let value = sequence;
  sequence += 1;
  let suffix = "";
  do {
    suffix = CROCKFORD[value % 32] + suffix;
    value = Math.floor(value / 32);
  } while (value > 0);
  return `${LIVE_ID_PREFIX}${suffix.padStart(10, "0")}`.slice(0, 26);
}
