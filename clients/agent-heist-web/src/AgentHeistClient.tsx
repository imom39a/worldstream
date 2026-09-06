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
  const installedDelivery = useRef<
    HostedLiveSessionSnapshot["deliveryBatch"]
  >(null);

  const install = useCallback((next: HostedLiveSessionSnapshot) => {
    setSessionState(next);
    if (
      next.deliveryBatch !== null &&
      next.deliveryBatch !== installedDelivery.current
    ) {
      installedDelivery.current = next.deliveryBatch;
      const delivery = next.deliveryBatch;
      setLiveState((current) => reduceAgentHeistObservation(current, delivery));
    }
  }, []);

  useEffect(() => {
    const unsubscribe = controller.subscribe(install);
    install(controller.state);
    void controller.start(startup).then(install);
    return unsubscribe;
  }, [controller, install, startup]);

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
