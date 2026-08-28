import { useEffect, useRef, useState } from "react";

import type { ParticipantBrowserReplay, ParticipantConsoleStartup, ParticipantHandoffClient } from "./participantHandoff";
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
        await session.act(action);
        const next = await session.refresh();
        if (mounted.current) setState(next);
      } catch (error) {
        if (mounted.current) setState(session.fail(error));
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
    replay={replay}
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
  replay,
  onReconnect,
  onAct,
  onReplay,
}: {
  state: ParticipantConsoleSessionState;
  replay?: ParticipantBrowserReplay | null;
  onReconnect: () => Promise<void>;
  onAct: (action: ParticipantConsoleAction) => Promise<void>;
  onReplay?: () => Promise<void>;
}) {
  const [payloads, setPayloads] = useState<Record<string, string>>({});
  const offers = state.state === "live" ? actionOffers(state) : [];
  return (
    <main className="participant-handoff-shell">
      <p className="eyebrow">Participant View</p>
      <h1>{state.state === "live" ? "Participant session" : "Participant session"}</h1>
      {state.message === null ? <p role="status">Your provisioned participant authority is connected.</p> : <p role="status">{state.message}</p>}
      {state.state === "live" ? (
        <>
          <section aria-labelledby="participant-head">
            <h2 id="participant-head">Authorized Room projection</h2>
            <p>Current sequence: {state.observation.room_head.room_seq} · frame {state.observation.frame_head}</p>
            {state.observation.delivery.map((delivery, index) => (
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
                    basedOnRoomSeq: state.observation.room_head.room_seq,
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
      {state.action === "return_to_task_setup" ? <p>Return to Studio Task setup.</p> : null}
    </main>
  );
}

function actionOffers(state: Extract<ParticipantConsoleSessionState, { state: "live" }>): ParticipantOfferView[] {
  const candidates = state.observation.delivery.flatMap((delivery) => {
    const root = delivery.kind === "projection_reset"
      ? record(delivery.body.projection)
      : record(delivery.body.observation);
    const projection = "projection" in root ? record(root.projection) : root;
    return Array.isArray(projection.action_offers) ? projection.action_offers : [];
  });
  return candidates.flatMap((candidate, index) => {
    const offer = record(candidate);
    if (typeof offer.action_type !== "string" || typeof offer.payload_schema_digest !== "string") return [];
    return [{
      offerId: `${state.observation.room_head.room_seq}:${offer.action_type}:${index}`,
      actionType: offer.action_type,
      schemaDigest: offer.payload_schema_digest,
    }];
  });
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
