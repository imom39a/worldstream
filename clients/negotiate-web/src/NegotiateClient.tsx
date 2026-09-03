import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";

import {
  ActivityClientRequestQueue,
  ActivityClientSession,
  type ActivityClientActionReceipt,
  type ActivityClientHandoffClient,
  type ActivityClientSessionState,
  type ActivityClientStartup,
  type VerifiedRoomReplay,
} from "@worldstream/client";

import {
  initialNegotiateLiveState,
  reduceNegotiateObservation,
  type NegotiateActionReceipt,
  type NegotiateLiveState,
  type NegotiateOfferBinding,
  type NegotiateReadyState,
} from "./liveAdapter";
import type { JsonValue, NegotiateActionOffer } from "./model";
import { NegotiateApp } from "./presentation";
import type { NegotiateReplaySummary } from "./proofs";

export interface NegotiateClientProps {
  readonly startup: ActivityClientStartup;
  readonly client: ActivityClientHandoffClient;
  readonly pollIntervalMs?: number;
}

export interface PendingPreparation {
  readonly requestId: string;
  readonly offer: NegotiateActionOffer;
  readonly binding: NegotiateOfferBinding;
  readonly basedOnRoomSequence: number;
  readonly pack: NegotiateReadyState["pack"];
}

const IDLE_RECEIPT: NegotiateActionReceipt = {
  state: "idle",
  action_id: null,
  code: null,
  message: null,
};

/**
 * Standalone Negotiate Activity Client. Studio and the generic Inspector never
 * import its projection parser, presentation, or action-preparation protocol.
 */
export function NegotiateClient({
  startup,
  client,
  pollIntervalMs = 1_000,
}: NegotiateClientProps) {
  const sessionRef = useRef<ActivityClientSession | null>(null);
  if (sessionRef.current === null) sessionRef.current = new ActivityClientSession(client);
  const session = sessionRef.current;
  const queueRef = useRef<ActivityClientRequestQueue | null>(null);
  if (queueRef.current === null) queueRef.current = new ActivityClientRequestQueue();
  const queue = queueRef.current;

  const [sessionState, setSessionState] = useState<ActivityClientSessionState>(session.state);
  const [liveState, setLiveState] = useState<NegotiateLiveState>(initialNegotiateLiveState);
  const [receipt, setReceipt] = useState<NegotiateActionReceipt>(IDLE_RECEIPT);
  const [replaySummary, setReplaySummary] = useState<NegotiateReplaySummary | null>(null);
  const [proofError, setProofError] = useState<string | null>(null);
  const liveStateRef = useRef<NegotiateLiveState>(initialNegotiateLiveState());
  const pending = useRef<PendingPreparation | null>(null);
  const mounted = useRef(false);

  const install = useCallback((next: ActivityClientSessionState) => {
    if (!mounted.current) return;
    setSessionState(next);
    if (next.state === "live") {
      const reduced = reduceNegotiateObservation(liveStateRef.current, next.deliveryBatch);
      liveStateRef.current = reduced;
      setLiveState(reduced);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let disposed = false;
    void session.start(startup).then((next) => { if (!disposed) install(next); });
    return () => { disposed = true; };
  }, [install, session, startup]);

  useEffect(() => {
    if (sessionState.state !== "live") return undefined;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (disposed) return;
      try {
        const next = await queue.run(() => session.refresh());
        if (!disposed) install(next);
      } finally {
        if (!disposed) timer = setTimeout(poll, pollIntervalMs);
      }
    };
    timer = setTimeout(poll, pollIntervalMs);
    return () => {
      disposed = true;
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [install, pollIntervalMs, queue, session, sessionState.state]);

  useEffect(() => {
    const submitPrepared = (event: Event) => {
      if (!(event instanceof CustomEvent)) return;
      const retained = pending.current;
      if (retained === null || !preparedActionMatches(retained, event.detail)) return;
      const current = liveStateRef.current;
      if (current.kind !== "ready" || !preparationMatchesCurrentState(retained, current)) {
        pending.current = null;
        setReceipt({
          state: "stale",
          action_id: null,
          code: "prepared_action_stale",
          message: "The Room Head or offered Action changed while the payload was being prepared.",
        });
        return;
      }
      pending.current = null;
      void submitAction(retained, event.detail.payload);
    };
    window.addEventListener("worldstream:negotiate-prepared-action", submitPrepared);
    return () => window.removeEventListener("worldstream:negotiate-prepared-action", submitPrepared);
  });

  const submitAction = async (preparation: PendingPreparation, payload: JsonValue) => {
    setReceipt({ state: "submitting", action_id: null, code: null, message: "Submitting exact prepared payload…" });
    await queue.run(async () => {
      try {
        const submission = await submitPreparedAgainstLatest(
          preparation,
          payload,
          () => liveStateRef.current,
          (exactPayload) => session.act({
            actionId: nextUlid(),
            basedOnRoomSeq: preparation.basedOnRoomSequence,
            offerId: preparation.binding.offerId,
            schemaDigest: preparation.binding.schemaDigest,
            actionType: preparation.binding.actionType,
            payload: exactPayload,
          }),
        );
        if (submission.state === "stale") {
          setReceipt({
            state: "stale",
            action_id: null,
            code: "prepared_action_stale",
            message: "The Room Head or offered Action changed before submission.",
          });
          return;
        }
        setReceipt(receiptFrom(submission.receipt));
        install(await session.refresh());
      } catch (error) {
        setReceipt({
          state: "rejected",
          action_id: null,
          code: "activity_client_submission_failed",
          message: "The authoritative Room rejected this Action or the client disconnected.",
        });
        install(session.fail(error));
      }
    });
  };

  if (liveState.kind === "awaiting") {
    return <Boundary title="Waiting for authorized Projection" detail={sessionState.message ?? "This client remains empty until its retained session installs a Projection Reset."} />;
  }
  if (liveState.kind === "incompatible") {
    return <Boundary title="Client incompatible" detail={liveState.reason} />;
  }

  const connection = sessionState.state === "live"
    ? "live"
    : sessionState.state === "connecting"
      ? "connecting"
      : "disconnected";
  const display = {
    ...liveState.session,
    connection,
  } as const;
  return (
    <NegotiateApp
      session={display}
      onSubmitPreparedAction={liveState.authorization.accessMode === "participant" && connection === "live"
        ? (offer) => requestPreparation(liveState, offer, pending)
        : undefined}
      onReconnect={sessionState.state === "disconnected"
        ? () => { void queue.run(async () => install(await session.reconnect())); }
        : undefined}
      onOpenReplay={() => { void openReplay(session, liveState, setReplaySummary, setProofError); }}
      receipt={receipt}
      replaySummary={replaySummary}
      proofError={proofError}
    />
  );
}

function requestPreparation(
  state: NegotiateReadyState,
  offer: NegotiateActionOffer,
  pending: MutableRefObject<PendingPreparation | null>,
): void {
  const index = state.session.action_offers.indexOf(offer);
  const binding = index < 0 ? undefined : state.offerBindings[index];
  if (binding === undefined) return;
  const requestId = crypto.randomUUID();
  pending.current = {
    requestId,
    offer,
    binding,
    basedOnRoomSequence: state.roomSequence,
    pack: { ...state.pack },
  };
  window.dispatchEvent(new CustomEvent("worldstream:negotiate-action-requested", {
    detail: {
      request_id: requestId,
      action_type: binding.actionType,
      payload_schema_digest: binding.schemaDigest,
      based_on_room_seq: state.roomSequence,
      pack: state.pack,
    },
  }));
}

export function preparedActionMatches(
  pending: PendingPreparation,
  value: unknown,
): value is {
  readonly request_id: string;
  readonly action_type: string;
  readonly payload_schema_digest: string;
  readonly based_on_room_seq: number;
  readonly pack: NegotiateReadyState["pack"];
  readonly payload: JsonValue;
} {
  const candidate = record(value);
  const pack = record(candidate.pack);
  return Object.keys(candidate).length === 6
    && Object.keys(pack).length === 3
    && candidate.request_id === pending.requestId
    && candidate.action_type === pending.binding.actionType
    && candidate.payload_schema_digest === pending.binding.schemaDigest
    && candidate.based_on_room_seq === pending.basedOnRoomSequence
    && pack.id === pending.pack.id
    && pack.version === pending.pack.version
    && pack.digest === pending.pack.digest
    && Object.hasOwn(candidate, "payload");
}

export function preparationMatchesCurrentState(
  pending: PendingPreparation,
  current: NegotiateReadyState,
): boolean {
  return current.roomSequence === pending.basedOnRoomSequence
    && current.pack.id === pending.pack.id
    && current.pack.version === pending.pack.version
    && current.pack.digest === pending.pack.digest
    && current.offerBindings.some((binding) => (
      binding.offerId === pending.binding.offerId
      && binding.actionType === pending.binding.actionType
      && binding.schemaDigest === pending.binding.schemaDigest
    ));
}

export async function submitPreparedAgainstLatest(
  pending: PendingPreparation,
  payload: JsonValue,
  current: () => NegotiateLiveState,
  submit: (payload: JsonValue) => Promise<ActivityClientActionReceipt>,
): Promise<
  | { readonly state: "stale" }
  | { readonly state: "submitted"; readonly receipt: ActivityClientActionReceipt }
> {
  const latest = current();
  if (latest.kind !== "ready" || !preparationMatchesCurrentState(pending, latest)) {
    return { state: "stale" };
  }
  return { state: "submitted", receipt: await submit(payload) };
}

export function replaySummaryFrom(
  replay: VerifiedRoomReplay,
  state: NegotiateReadyState,
): NegotiateReplaySummary | null {
  if (
    replay.requested_room_seq !== state.roomSequence
    || replay.room_head.room_seq !== state.roomSequence
    || replay.room_head.pack_digest !== state.pack.digest
    || replay.room_head.genesis_or_transition_hash
      !== state.roomHead.genesis_or_transition_hash
    || replay.room_head.authoritative_state_hash
      !== state.roomHead.authoritative_state_hash
    || replay.verification !== "verified"
  ) return null;
  return {
    room_id: null,
    requested_room_seq: replay.requested_room_seq,
    pack_id: "worldstream.negotiate",
    pack_revision_digest: state.pack.digest,
    lineage_hash: replay.room_head.genesis_or_transition_hash,
    authoritative_state_hash: replay.room_head.authoritative_state_hash,
    projection_hash: replay.projection_hash,
    verification: "verified",
  };
}

async function openReplay(
  session: ActivityClientSession,
  state: NegotiateReadyState,
  setSummary: (summary: NegotiateReplaySummary) => void,
  setError: (error: string | null) => void,
): Promise<void> {
  try {
    const replay = await session.replay();
    const summary = replaySummaryFrom(replay, state);
    if (summary === null) throw new Error("replay mismatch");
    setSummary(summary);
    setError(null);
  } catch {
    setError("Verified Replay was unavailable or did not match this exact Room Head.");
  }
}

export function receiptFrom(value: ActivityClientActionReceipt): NegotiateActionReceipt {
  const receipt = record(record(value).receipt);
  return value.state === "accepted"
    ? {
        state: "accepted",
        action_id: text(receipt.action_id),
        code: null,
        message: "The Action was accepted. Waiting for committed Room output.",
      }
    : {
        state: "rejected",
        action_id: text(receipt.action_id),
        code: text(receipt.code) ?? "action_rejected",
        message: text(receipt.message) ?? "The authoritative Room rejected this Action.",
      };
}

function Boundary({ title, detail }: { readonly title: string; readonly detail: string }) {
  return <main className="negotiate-boundary"><p className="eyebrow">Negotiate Activity Client</p><h1>{title}</h1><p>{detail}</p></main>;
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 && value.length <= 2_048 ? value : null;
}

function nextUlid(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[0] = (bytes[0] ?? 0) & 0x3f;
  let value = bytes.reduce((result, byte) => (result << 8n) | BigInt(byte), 0n);
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let encoded = "";
  for (let index = 0; index < 26; index += 1) {
    encoded = alphabet[Number(value & 31n)] + encoded;
    value >>= 5n;
  }
  return encoded;
}
