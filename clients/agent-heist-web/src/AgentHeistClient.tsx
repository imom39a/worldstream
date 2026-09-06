import { useCallback, useEffect, useRef, useState } from "react";

import {
  type ActivityClientAction,
  type ActivityClientStartup,
  type HostedLiveSessionController,
  type HostedLiveSessionSnapshot,
  type JsonValue,
} from "@worldstream/client";

import {
  AgentHeistClientView,
  type AgentHeistClientConnection,
} from "./AgentHeistClientView";
import {
  initialAgentHeistLiveState,
  reduceAgentHeistObservation,
  type AgentHeistLiveState,
} from "./liveAdapter";
import {
  AgentHeistWebMcpBridge,
  registerAgentHeistWebMcp,
} from "./webmcp";

export interface AgentHeistClientProps {
  readonly startup: ActivityClientStartup;
  readonly controller: HostedLiveSessionController;
}

/**
 * Mountable Agent Heist participant client. The React surface interprets Pack
 * data; the shared controller owns only the direct-stream session mechanics.
 */
export function AgentHeistClient({
  startup,
  controller,
}: AgentHeistClientProps) {
  const [sessionState, setSessionState] = useState<HostedLiveSessionSnapshot>(
    controller.state,
  );
  const [liveState, setLiveState] = useState<AgentHeistLiveState>(
    initialAgentHeistLiveState,
  );
  const liveStateRef = useRef(liveState);
  const installedDelivery = useRef<
    HostedLiveSessionSnapshot["deliveryBatch"]
  >(null);
  const [webMcp, setWebMcp] = useState<
    "checking" | "available" | "unsupported" | "unavailable"
  >("checking");
  const [webMcpBridge] = useState(() => new AgentHeistWebMcpBridge({
    controller,
    readLiveState: () => liveStateRef.current,
  }));

  const install = useCallback((next: HostedLiveSessionSnapshot) => {
    setSessionState(next);
    if (next.status === "setup_required" || next.status === "closed") {
      installedDelivery.current = null;
      const empty = initialAgentHeistLiveState();
      liveStateRef.current = empty;
      setLiveState(empty);
      return;
    }
    if (
      next.deliveryBatch !== null &&
      next.deliveryBatch !== installedDelivery.current
    ) {
      installedDelivery.current = next.deliveryBatch;
      const delivery = next.deliveryBatch;
      const reduced = reduceAgentHeistObservation(liveStateRef.current, delivery);
      liveStateRef.current = reduced;
      setLiveState(reduced);
    }
  }, []);

  useEffect(() => {
    const unsubscribe = controller.subscribe(install);
    install(controller.state);
    void controller.start(startup).then(install);
    return unsubscribe;
  }, [controller, install, startup]);

  const accessMode = liveState.kind === "ready"
    ? liveState.authorization.accessMode
    : null;
  useEffect(() => {
    if (accessMode === null) {
      setWebMcp("checking");
      return undefined;
    }
    const registration = registerAgentHeistWebMcp(
      document as unknown as Parameters<typeof registerAgentHeistWebMcp>[0],
      webMcpBridge,
      accessMode,
    );
    if (!registration.supported) {
      setWebMcp("unsupported");
      return registration.dispose;
    }
    let disposed = false;
    void registration.ready.then((ready) => {
      if (!disposed) setWebMcp(ready ? "available" : "unavailable");
    });
    return () => {
      disposed = true;
      registration.dispose();
    };
  }, [accessMode, webMcpBridge]);

  const submit = async (action: ActivityClientAction) => {
    const current = liveState;
    if (current.kind !== "ready") {
      throw new Error("Agent Heist has no authorized Action Offer.");
    }
    const offer = current.offers.find(
      (candidate) => candidate.offerId === action.offerId,
    );
    if (
      offer === undefined ||
      offer.actionType !== action.actionType ||
      offer.schemaDigest !== action.schemaDigest ||
      action.basedOnRoomSeq !== current.roomSequence
    ) {
      throw new Error("Agent Heist Action Offer is no longer current.");
    }
    const receipt = await controller.submitAction({
      actionId: action.actionId,
      basedOnRoomSeq: action.basedOnRoomSeq,
      actionType: action.actionType,
      payload: action.payload as JsonValue,
    });
    if (receipt.state === "rejected") {
      throw new Error(`The authoritative Room rejected this Action (${receipt.code}).`);
    }
  };

  return (
    <AgentHeistClientView
      state={liveState}
      connection={connectionFor(sessionState.status)}
      message={sessionState.message}
      actionsEnabled={sessionState.canAct}
      agentAssist={webMcp}
      onAct={submit}
      onReconnect={() => controller.reconnect().then(() => undefined)}
    />
  );
}

export function connectionFor(
  status: HostedLiveSessionSnapshot["status"],
): AgentHeistClientConnection {
  if (status === "live") return "live";
  if (status === "setup_required") return "setup_required";
  if (status === "disconnected" || status === "closed") return "disconnected";
  return "connecting";
}
