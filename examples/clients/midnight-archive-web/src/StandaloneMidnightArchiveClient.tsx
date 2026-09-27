import { useCallback, useEffect, useRef, useState } from "react";

import {
  ActivityClientRequestQueue,
  ActivityClientSession,
  type ActivityClientActionReceipt,
  type ActivityClientHandoffClient,
  type ActivityClientSessionState,
  type ActivityClientStartup,
} from "@worldstream/client";

import { MidnightArchiveClientView } from "./MidnightArchiveClientView";
import {
  initialMidnightArchiveLiveState,
  prepareMidnightArchiveAction,
  reduceMidnightArchiveObservation,
  type MidnightArchiveLiveState,
} from "./liveAdapter";
import type { MidnightArchiveActionIntent } from "./model";
import {
  replaySummaryFrom,
  type MidnightArchiveReplayState,
} from "./replay";

export function StandaloneMidnightArchiveClient({
  startup,
  client,
  pollIntervalMs = 1_000,
}: {
  readonly startup: ActivityClientStartup;
  readonly client: ActivityClientHandoffClient;
  readonly pollIntervalMs?: number;
}) {
  const sessionRef = useRef<ActivityClientSession | null>(null);
  if (sessionRef.current === null) sessionRef.current = new ActivityClientSession(client);
  const retained = sessionRef.current;
  const queueRef = useRef<ActivityClientRequestQueue | null>(null);
  if (queueRef.current === null) queueRef.current = new ActivityClientRequestQueue();
  const queue = queueRef.current;

  const [session, setSession] = useState<ActivityClientSessionState>(retained.state);
  const [live, setLive] = useState<MidnightArchiveLiveState>(initialMidnightArchiveLiveState);
  const liveRef = useRef(live);
  const [visible, setVisible] = useState(document.visibilityState !== "hidden");
  const [requiresReconnect, setRequiresReconnect] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [replay, setReplay] = useState<MidnightArchiveReplayState>({ kind: "unavailable" });
  const replayHead = useRef<string | null>(null);
  const mounted = useRef(false);

  const install = useCallback((next: ActivityClientSessionState) => {
    if (!mounted.current) return;
    setSession(next);
    if (next.state === "setup_required") {
      const empty = initialMidnightArchiveLiveState();
      liveRef.current = empty;
      setLive(empty);
    } else if (next.state === "live") {
      const reduced = reduceMidnightArchiveObservation(liveRef.current, next.deliveryBatch);
      liveRef.current = reduced;
      setLive(reduced);
      const head = reduced.kind === "ready" && reduced.projection.phase === "complete"
        ? `${reduced.pack.digest}:${reduced.roomSequence}:${reduced.roomHead.authoritativeStateHash}`
        : null;
      if (head !== replayHead.current) {
        replayHead.current = head;
        setReplay(head === null ? { kind: "unavailable" } : { kind: "available" });
      }
    }
  }, []);

  const reconnect = useCallback(async () => {
    setRequiresReconnect(true);
    await queue.run(async () => {
      const next = await retained.reconnect();
      install(next);
      setRequiresReconnect(next.state !== "live");
    });
  }, [install, queue, retained]);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let disposed = false;
    void retained.start(startup).then((next) => {
      if (!disposed) install(next);
    });
    return () => { disposed = true; };
  }, [install, retained, startup]);

  useEffect(() => {
    if (session.state !== "live" || !visible || requiresReconnect) return undefined;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (disposed) return;
      try {
        const next = await queue.run(() => retained.refresh());
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
  }, [install, pollIntervalMs, queue, requiresReconnect, retained, session.state, visible]);

  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        setVisible(false);
        setRequiresReconnect(true);
      } else {
        setVisible(true);
        void reconnect();
      }
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => document.removeEventListener("visibilitychange", onVisibility);
  }, [reconnect]);

  const actionsEnabled = session.state === "live" && visible && !requiresReconnect && !submitting;
  const openReplay = async () => {
    const current = liveRef.current;
    if (current.kind !== "ready" || current.projection.phase !== "complete" || session.state !== "live") {
      setReplay({ kind: "error", message: "Verified Replay is unavailable for this state." });
      return;
    }
    setReplay({ kind: "loading" });
    try {
      const verified = await queue.run(() => retained.replay());
      const summary = replaySummaryFrom(verified, current);
      if (summary === null) throw new Error("replay mismatch");
      setReplay({ kind: "verified", summary });
    } catch {
      setReplay({ kind: "error", message: "Verified Replay was unavailable or did not match this exact Room Head." });
    }
  };
  const submit = async (intent: MidnightArchiveActionIntent) => {
    const current = liveRef.current;
    if (!actionsEnabled || current.kind !== "ready") {
      throw new Error("The authorized Projection is not current. Reconnect before acting.");
    }
    const action = prepareMidnightArchiveAction(current, intent);
    setSubmitting(true);
    try {
      await queue.run(async () => {
        let receipt: ActivityClientActionReceipt;
        try {
          receipt = await retained.act(action);
        } catch (error) {
          install(retained.fail(error));
          throw error;
        }
        install(await retained.refresh());
        if (receipt.state === "rejected") {
          throw new Error("The authoritative Room rejected this Action.");
        }
      });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <MidnightArchiveClientView
      state={live}
      connection={session.state}
      actionsEnabled={actionsEnabled}
      submitting={submitting}
      message={session.message}
      onAction={submit}
      onReconnect={reconnect}
      replay={replay}
      onOpenReplay={openReplay}
    />
  );
}
