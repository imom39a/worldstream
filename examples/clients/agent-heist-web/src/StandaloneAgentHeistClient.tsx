import { useCallback, useEffect, useRef, useState } from "react";

import {
  ActivityClientRequestQueue,
  ActivityClientSession,
  type ActivityClientAction,
  type ActivityClientActionReceipt,
  type ActivityClientHandoffClient,
  type ActivityClientSessionState,
  type ActivityClientStartup,
} from "@worldstream/client";

import { AgentHeistClientView } from "./AgentHeistClientView";
import {
  initialAgentHeistLiveState,
  reduceAgentHeistObservation,
  type AgentHeistLiveState,
} from "./liveAdapter";

export interface StandaloneAgentHeistClientProps {
  readonly startup: ActivityClientStartup;
  readonly client: ActivityClientHandoffClient;
  readonly pollIntervalMs?: number;
}

/**
 * Standalone loopback client using the existing cookie-backed HTTP session.
 * Hosted participation uses the separate direct-stream entrypoint, never this
 * compatibility polling path as an authentication or connection fallback.
 */
export function StandaloneAgentHeistClient({
  startup,
  client,
  pollIntervalMs = 1_000,
}: StandaloneAgentHeistClientProps) {
  const sessionRef = useRef<ActivityClientSession | null>(null);
  if (sessionRef.current === null) sessionRef.current = new ActivityClientSession(client);
  const session = sessionRef.current;

  const queueRef = useRef<ActivityClientRequestQueue | null>(null);
  if (queueRef.current === null) queueRef.current = new ActivityClientRequestQueue();
  const queue = queueRef.current;

  const [sessionState, setSessionState] = useState<ActivityClientSessionState>(session.state);
  const [liveState, setLiveState] = useState<AgentHeistLiveState>(initialAgentHeistLiveState);
  const mounted = useRef(false);

  const install = useCallback((next: ActivityClientSessionState) => {
    if (!mounted.current) return;
    setSessionState(next);
    if (next.state === "setup_required") setLiveState(initialAgentHeistLiveState());
    if (next.state === "live") {
      setLiveState((current) => reduceAgentHeistObservation(current, next.deliveryBatch));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let disposed = false;
    void session.start(startup).then((next) => {
      if (!disposed) install(next);
    });
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

  const submit = async (action: ActivityClientAction) => {
    await queue.run(async () => {
      let receipt: ActivityClientActionReceipt;
      try {
        receipt = await session.act(action);
      } catch (error) {
        install(session.fail(error));
        throw error;
      }
      install(await session.refresh());
      if (receipt.state === "rejected") {
        throw new Error("The authoritative Room rejected this Action.");
      }
    });
  };

  const reconnect = async () => {
    await queue.run(async () => install(await session.reconnect()));
  };

  return (
    <AgentHeistClientView
      state={liveState}
      connection={sessionState.state}
      message={sessionState.message}
      agentAssist="unavailable"
      onAct={submit}
      onReconnect={reconnect}
    />
  );
}
