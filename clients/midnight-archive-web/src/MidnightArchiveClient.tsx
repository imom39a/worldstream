import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import type {
  ActivityClientStartup,
  HostedLiveSessionController,
  HostedLiveSessionSnapshot,
  JsonValue,
} from "@worldstream/client";

import {
  MidnightArchiveClientView,
  type MidnightArchiveConnection,
} from "./MidnightArchiveClientView";
import {
  archiveActionsEnabled,
  initialArchiveInteractionGate,
  isAuthorizedProjectionCurrent,
  reduceArchiveInteractionGate,
} from "./interactionGate";
import {
  initialMidnightArchiveLiveState,
  prepareMidnightArchiveAction,
  reduceMidnightArchiveObservation,
  type MidnightArchiveLiveState,
} from "./liveAdapter";
import type { MidnightArchiveActionIntent } from "./model";

export interface MidnightArchiveClientProps {
  readonly startup: ActivityClientStartup;
  readonly controller: MidnightArchiveSessionController;
}

export type MidnightArchiveSessionController = Pick<
  HostedLiveSessionController,
  "state" | "subscribe" | "start" | "reconnect" | "submitAction"
>;

export function MidnightArchiveClient({
  startup,
  controller,
}: MidnightArchiveClientProps) {
  const [session, setSession] = useState<HostedLiveSessionSnapshot>(controller.state);
  const [live, setLive] = useState<MidnightArchiveLiveState>(initialMidnightArchiveLiveState);
  const liveRef = useRef(live);
  const installedDelivery = useRef<HostedLiveSessionSnapshot["deliveryBatch"]>(null);
  const [gate, dispatchGate] = useReducer(
    reduceArchiveInteractionGate,
    undefined,
    () => initialArchiveInteractionGate(document.visibilityState !== "hidden"),
  );
  const [clientMessage, setClientMessage] = useState<string | null>(null);

  const install = useCallback((next: HostedLiveSessionSnapshot) => {
    setSession(next);
    let nextLive = liveRef.current;
    if (next.status === "setup_required" || next.status === "closed") {
      installedDelivery.current = null;
      nextLive = initialMidnightArchiveLiveState();
      liveRef.current = nextLive;
      setLive(nextLive);
    } else if (next.deliveryBatch !== null && next.deliveryBatch !== installedDelivery.current) {
      installedDelivery.current = next.deliveryBatch;
      nextLive = reduceMidnightArchiveObservation(nextLive, next.deliveryBatch);
      liveRef.current = nextLive;
      setLive(nextLive);
    }
    dispatchGate({ type: "snapshot", current: isAuthorizedProjectionCurrent(next, nextLive) });
  }, []);

  const reconnect = useCallback(async () => {
    dispatchGate({ type: "reconnect_started" });
    setClientMessage("Revalidating your retained participant session…");
    try {
      const next = await controller.reconnect();
      install(next);
      const current = isAuthorizedProjectionCurrent(next, liveRef.current);
      dispatchGate({ type: "reconnect_completed", current });
      setClientMessage(current ? null : "Waiting for the current authorized Projection acknowledgement…");
    } catch (error) {
      dispatchGate({ type: "reconnect_completed", current: false });
      setClientMessage(error instanceof Error ? error.message : "The archive could not reconnect.");
    }
  }, [controller, install]);

  useEffect(() => {
    const unsubscribe = controller.subscribe(install);
    install(controller.state);
    void controller.start(startup).then(install).catch((error: unknown) => {
      setClientMessage(error instanceof Error ? error.message : "The archive session could not start.");
    });
    return unsubscribe;
  }, [controller, install, startup]);

  useEffect(() => {
    const onVisibilityChange = () => {
      if (document.visibilityState === "hidden") {
        dispatchGate({ type: "visibility_hidden" });
        setClientMessage("This view was hidden. Re-entry must synchronize before Actions unlock.");
        return;
      }
      dispatchGate({ type: "visibility_visible" });
      void reconnect();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => document.removeEventListener("visibilitychange", onVisibilityChange);
  }, [reconnect]);

  const submit = async (intent: MidnightArchiveActionIntent) => {
    const current = liveRef.current;
    if (current.kind !== "ready" || !archiveActionsEnabled(gate)) {
      throw new Error("The authorized Projection is not current. Reconnect before acting.");
    }
    const action = prepareMidnightArchiveAction(current, intent);
    dispatchGate({ type: "submission_started" });
    try {
      const receipt = await controller.submitAction({
        actionId: action.actionId,
        basedOnRoomSeq: action.basedOnRoomSeq,
        actionType: action.actionType,
        payload: action.payload as JsonValue,
      });
      if (receipt.state === "rejected") {
        throw new Error(rejectedActionMessage(intent.action, receipt.code));
      }
    } finally {
      const next = controller.state;
      install(next);
      dispatchGate({
        type: "submission_finished",
        current: isAuthorizedProjectionCurrent(next, liveRef.current),
      });
    }
  };

  return (
    <MidnightArchiveClientView
      state={live}
      connection={connectionFor(session.status)}
      actionsEnabled={archiveActionsEnabled(gate)}
      submitting={gate.submitting}
      message={clientMessage ?? session.message}
      onAction={submit}
      onReconnect={reconnect}
    />
  );
}

export function connectionFor(
  status: HostedLiveSessionSnapshot["status"],
): MidnightArchiveConnection {
  if (status === "live") return "live";
  if (status === "setup_required") return "setup_required";
  if (status === "disconnected" || status === "closed") return "disconnected";
  return "connecting";
}

export function rejectedActionMessage(
  action: MidnightArchiveActionIntent["action"],
  code: string,
): string {
  return `The authoritative Room rejected ${action.replaceAll("_", " ")} (${code}). The board was refreshed; continue only with its current offered controls.`;
}
