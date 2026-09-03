import { useEffect, useRef, useState } from "react";

import {
  ParticipantHandoffError,
  type ParticipantBrowserReplay,
  type ParticipantConsoleStartup,
  type ParticipantHandoffClient,
} from "./participantHandoff";
import {
  ParticipantConsoleSession,
  type ParticipantConsoleAction,
  type ParticipantConsoleSessionState,
} from "./participantSession";
import { ParticipantConsoleRequestQueue } from "./participantRequestQueue";

interface ParticipantOfferView {
  offerId: string;
  actionType: string;
  schemaDigest: string;
}

export type InspectorActionReceipt =
  | {
    state: "accepted";
    actionId: string;
    transitionId: string | null;
    roomSeq: number | null;
    duplicate: boolean;
  }
  | {
    state: "rejected";
    code: string;
    message: string;
    retryable: boolean | null;
  };

export function HandedOffParticipant({
  startup,
  client,
}: {
  startup: ParticipantConsoleStartup;
  client: ParticipantHandoffClient;
}) {
  const sessionRef = useRef<ParticipantConsoleSession | null>(null);
  if (sessionRef.current === null) sessionRef.current = new ParticipantConsoleSession(client);
  const session = sessionRef.current;
  const [state, setState] = useState<ParticipantConsoleSessionState>(session.state);
  const [replay, setReplay] = useState<ParticipantBrowserReplay | null>(null);
  const [actionReceipt, setActionReceipt] = useState<InspectorActionReceipt | null>(null);
  const startTask = useRef<Promise<ParticipantConsoleSessionState> | null>(null);
  const requestQueueRef = useRef<ParticipantConsoleRequestQueue | null>(null);
  if (requestQueueRef.current === null) requestQueueRef.current = new ParticipantConsoleRequestQueue();
  const requestQueue = requestQueueRef.current;
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    let disposed = false;
    startTask.current ??= session.start(startup);
    void startTask.current.then((next) => { if (!disposed) setState(next); });
    return () => { disposed = true; };
  }, [session, startup]);

  useEffect(() => {
    if (state.state !== "live") return undefined;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (disposed) return;
      try {
        const next = await requestQueue.run(() => session.refresh());
        if (!disposed) setState(next);
      } finally {
        if (!disposed) timer = setTimeout(poll, 1_000);
      }
    };
    timer = setTimeout(poll, 1_000);
    return () => {
      disposed = true;
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [requestQueue, session, state.state]);

  const submit = async (action: ParticipantConsoleAction) => {
    await requestQueue.run(async () => {
      try {
        const receipt = await session.act(action);
        if (mounted.current) setActionReceipt(readInspectorActionReceipt(receipt, action.actionId));
        const next = await session.refresh();
        if (mounted.current) setState(next);
      } catch (error) {
        if (mounted.current) {
          setActionReceipt(readInspectorActionFailure(error));
          setState(session.fail(error));
        }
      }
    });
  };
  const verifyReplay = async () => {
    await requestQueue.run(async () => {
      try {
        const result = await session.replay();
        if (mounted.current) setReplay(result);
      } catch (error) {
        if (mounted.current) setState(session.fail(error));
      }
    });
  };
  return <ParticipantHandoffView
    state={state}
    actionOfferCandidates={session.actionOffers}
    replay={replay}
    actionReceipt={actionReceipt}
    onReconnect={async () => {
      await requestQueue.run(async () => {
        const next = await session.reconnect();
        if (mounted.current) setState(next);
      });
    }}
    onAct={submit}
    onReplay={verifyReplay}
  />;
}

export function ParticipantHandoffView({
  state,
  actionOfferCandidates,
  replay,
  actionReceipt,
  onReconnect,
  onAct,
  onReplay,
}: {
  state: ParticipantConsoleSessionState;
  actionOfferCandidates?: readonly unknown[];
  replay?: ParticipantBrowserReplay | null;
  actionReceipt?: InspectorActionReceipt | null;
  onReconnect: () => Promise<void>;
  onAct: (action: ParticipantConsoleAction) => Promise<void>;
  onReplay?: () => Promise<void>;
}) {
  const [payloads, setPayloads] = useState<Record<string, string>>({});
  const offers = state.state === "live" ? actionOffers(state, actionOfferCandidates) : [];
  return (
    <main className="participant-handoff-shell">
      <p className="eyebrow">WorldStream Inspector</p>
      <h1>Authorized Room session</h1>
      {state.message === null ? <p role="status">Your provisioned participant authority is connected.</p> : <p role="status">{state.message}</p>}
      {actionReceipt === null || actionReceipt === undefined ? null : (
        <InspectorActionReceiptView receipt={actionReceipt} />
      )}
      {state.state === "live" ? (
        <>
          <section aria-labelledby="participant-head">
            <h2 id="participant-head">Authorized Room projection</h2>
            <p>Current sequence: {state.deliveryBatch.room_head.room_seq} · frame {state.deliveryBatch.frame_head}</p>
            {state.deliveryBatch.delivery.map((delivery, index) => (
              <article key={`${delivery.kind}-${index}`}>
                <h3>{delivery.kind === "projection_reset" ? "Projection reset" : "Observation"}</h3>
                <pre>{JSON.stringify(delivery.body, null, 2)}</pre>
              </article>
            ))}
          </section>
          <section aria-labelledby="participant-actions">
            <h2 id="participant-actions">Available Actions</h2>
            {offers.length === 0 ? <p>No Action is offered at this synchronized head.</p> : offers.map((offer) => {
              const payload = payloads[offer.offerId] ?? "{}";
              return (
                <form key={offer.offerId} onSubmit={(event) => {
                  event.preventDefault();
                  let parsed: unknown;
                  try {
                    parsed = JSON.parse(payload);
                  } catch {
                    return;
                  }
                  void onAct({
                    actionId: nextActionUlid(),
                    basedOnRoomSeq: state.deliveryBatch.room_head.room_seq,
                    offerId: offer.offerId,
                    schemaDigest: offer.schemaDigest,
                    actionType: offer.actionType,
                    payload: parsed,
                  });
                }}>
                  <h3>{offer.actionType}</h3>
                  <p>Schema: {offer.schemaDigest}</p>
                  <label>
                    Action payload (JSON)
                    <textarea value={payload} onChange={(event) => setPayloads((current) => ({ ...current, [offer.offerId]: event.target.value }))} />
                  </label>
                  <button type="submit">Submit {offer.actionType}</button>
                </form>
              );
            })}
          </section>
          <section aria-labelledby="participant-replay">
            <h2 id="participant-replay">Authorized Replay</h2>
            <p>Replay reconstructs your authorized historical view from committed Canonical History at this synchronized sequence. It does not start a Runner or model.</p>
            {onReplay ? <button type="button" onClick={() => void onReplay()}>Verify Replay at current sequence</button> : null}
            {replay ? (
              <article>
                <h3>Verified Canonical History at sequence {replay.requested_room_seq}</h3>
                <p>Projection hash: {replay.projection_hash}</p>
                <p>Lineage hash: {replay.room_head.genesis_or_transition_hash}</p>
                <p>Authoritative state hash: {replay.room_head.authoritative_state_hash}</p>
                <pre>{JSON.stringify(replay.projection, null, 2)}</pre>
              </article>
            ) : null}
          </section>
        </>
      ) : null}
      {state.action === "reconnect" ? (
        <button type="button" onClick={() => void onReconnect()}>Reconnect</button>
      ) : null}
      {state.action === "return_to_task_setup" ? (
        <p>Ask the Host Operator to run worldstreamctl client open again for this Room setup operation.</p>
      ) : null}
    </main>
  );
}

function actionOffers(
  state: Extract<ParticipantConsoleSessionState, { state: "live" }>,
  installedCandidates?: readonly unknown[],
): ParticipantOfferView[] {
  let candidates: readonly unknown[] = installedCandidates ?? [];
  if (installedCandidates === undefined) {
    for (const delivery of state.deliveryBatch.delivery) {
      const root = delivery.kind === "projection_reset"
        ? record(delivery.body.projection)
        : record(delivery.body.observation);
      const projection = "projection" in root ? record(root.projection) : root;
      if (Object.hasOwn(projection, "action_offers")) {
        candidates = Array.isArray(projection.action_offers) ? projection.action_offers : [];
      }
    }
  }
  return candidates.flatMap((candidate, index) => {
    const offer = record(candidate);
    if (typeof offer.action_type !== "string" || typeof offer.payload_schema_digest !== "string") return [];
    return [{
      offerId: `${state.deliveryBatch.room_head.room_seq}:${offer.action_type}:${index}`,
      actionType: offer.action_type,
      schemaDigest: offer.payload_schema_digest,
    }];
  });
}

function InspectorActionReceiptView({ receipt }: { receipt: InspectorActionReceipt }) {
  if (receipt.state === "rejected") {
    return (
      <section className="inspector-action-receipt inspector-action-receipt-rejected" role="alert" aria-labelledby="inspector-action-receipt">
        <p className="eyebrow">Action receipt</p>
        <h2 id="inspector-action-receipt">Action rejected</h2>
        <p>{receipt.message}</p>
        <dl>
          <div><dt>Code</dt><dd>{receipt.code}</dd></div>
          <div><dt>Retryable</dt><dd>{receipt.retryable === null ? "Unknown" : receipt.retryable ? "Yes" : "No"}</dd></div>
        </dl>
      </section>
    );
  }
  return (
    <section className="inspector-action-receipt inspector-action-receipt-accepted" role="status" aria-labelledby="inspector-action-receipt">
      <p className="eyebrow">Action receipt</p>
      <h2 id="inspector-action-receipt">Action accepted</h2>
      <p>{receipt.duplicate ? "The duplicate submission resolved to its original receipt." : "The Action was accepted at the authoritative Room Head."}</p>
      <dl>
        <div><dt>Action</dt><dd>{receipt.actionId}</dd></div>
        {receipt.transitionId === null ? null : <div><dt>Transition</dt><dd>{receipt.transitionId}</dd></div>}
        {receipt.roomSeq === null ? null : <div><dt>Room sequence</dt><dd>{receipt.roomSeq}</dd></div>}
        <div><dt>Duplicate</dt><dd>{receipt.duplicate ? "Yes" : "No"}</dd></div>
      </dl>
    </section>
  );
}

export function readInspectorActionReceipt(value: unknown, fallbackActionId: string): InspectorActionReceipt {
  const envelope = record(value);
  const receipt = Object.hasOwn(envelope, "receipt") ? record(envelope.receipt) : envelope;
  if (envelope.state === "rejected") {
    return {
      state: "rejected",
      code: boundedText(receipt.code, 96, "action_rejected"),
      message: boundedText(receipt.message, 320, "The Action was rejected."),
      retryable: typeof receipt.retryable === "boolean"
        ? receipt.retryable
        : typeof receipt.retryable_with_same_action_id === "boolean"
          ? receipt.retryable_with_same_action_id
          : null,
    };
  }
  if (envelope.state !== "accepted") {
    return {
      state: "rejected",
      code: "action_receipt_invalid",
      message: "The Activity Client did not receive a valid Action receipt.",
      retryable: null,
    };
  }
  const head = record(receipt.room_head);
  return {
    state: "accepted",
    actionId: boundedIdentifier(receipt.action_id) ?? boundedIdentifier(fallbackActionId) ?? "submitted-action",
    transitionId: boundedIdentifier(receipt.transition_id),
    roomSeq: safeSequence(head.room_seq) ?? safeSequence(receipt.room_seq),
    duplicate: receipt.duplicate === true,
  };
}

export function readInspectorActionFailure(error: unknown): InspectorActionReceipt {
  if (error instanceof ParticipantHandoffError) {
    return {
      state: "rejected",
      code: boundedText(error.code, 96, "action_rejected"),
      message: boundedText(error.message, 320, "The Action was rejected."),
      retryable: error.retryable,
    };
  }
  return {
    state: "rejected",
    code: "action_submission_failed",
    message: "The Action could not be submitted. Reconnect before trying again.",
    retryable: null,
  };
}

function boundedIdentifier(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.length > 128) return null;
  if (/^(?:wsb1|wst1):/i.test(value)) return null;
  return /^[A-Za-z0-9:._-]+$/.test(value) ? value : null;
}

function boundedText(value: unknown, limit: number, fallback: string): string {
  if (typeof value !== "string") return fallback;
  const normalized = value.replace(/[\u0000-\u001f\u007f]/g, " ").trim();
  if (/(?:wsb1:|wst1:|https?:\/\/|\/api\/|(?:room|member|membership)_id)/i.test(normalized)) return fallback;
  return normalized.length === 0 ? fallback : normalized.slice(0, limit);
}

function safeSequence(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function nextActionUlid(): string {
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
