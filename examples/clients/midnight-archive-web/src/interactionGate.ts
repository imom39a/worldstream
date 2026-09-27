import type { HostedLiveSessionSnapshot } from "@worldstream/client";

import type { MidnightArchiveLiveState } from "./liveAdapter";

export interface ArchiveInteractionGate {
  readonly visible: boolean;
  readonly recoveryRequired: boolean;
  readonly projectionCurrent: boolean;
  readonly submitting: boolean;
}

export type ArchiveInteractionEvent =
  | { readonly type: "snapshot"; readonly current: boolean }
  | { readonly type: "visibility_hidden" }
  | { readonly type: "visibility_visible" }
  | { readonly type: "reconnect_started" }
  | { readonly type: "reconnect_completed"; readonly current: boolean }
  | { readonly type: "submission_started" }
  | { readonly type: "submission_finished"; readonly current: boolean };

export function initialArchiveInteractionGate(visible = true): ArchiveInteractionGate {
  return {
    visible,
    recoveryRequired: !visible,
    projectionCurrent: false,
    submitting: false,
  };
}

export function reduceArchiveInteractionGate(
  state: ArchiveInteractionGate,
  event: ArchiveInteractionEvent,
): ArchiveInteractionGate {
  switch (event.type) {
    case "snapshot":
      return { ...state, projectionCurrent: state.visible && event.current };
    case "visibility_hidden":
      return { ...state, visible: false, recoveryRequired: true, projectionCurrent: false };
    case "visibility_visible":
      return { ...state, visible: true, projectionCurrent: false };
    case "reconnect_started":
      return { ...state, recoveryRequired: true, projectionCurrent: false };
    case "reconnect_completed":
      return {
        ...state,
        recoveryRequired: !(state.visible && event.current),
        projectionCurrent: state.visible && event.current,
      };
    case "submission_started":
      return { ...state, submitting: true };
    case "submission_finished":
      return { ...state, submitting: false, projectionCurrent: state.visible && event.current };
  }
}

export function archiveActionsEnabled(state: ArchiveInteractionGate): boolean {
  return state.visible
    && !state.recoveryRequired
    && state.projectionCurrent
    && !state.submitting;
}

export function isAuthorizedProjectionCurrent(
  session: HostedLiveSessionSnapshot,
  live: MidnightArchiveLiveState,
): boolean {
  const installed = session.deliveryBatch;
  return live.kind === "ready"
    && session.status === "live"
    && session.synchronized
    && session.canAct
    && installed !== null
    && installed.frame_head === live.frameHead
    && installed.pack.id === live.pack.id
    && installed.pack.version === live.pack.version
    && installed.pack.digest === live.pack.digest
    && installed.room_head.room_seq === live.roomSequence
    && installed.room_head.pack_digest === live.pack.digest
    && installed.room_head.genesis_or_transition_hash === live.roomHead.genesisOrTransitionHash
    && installed.room_head.authoritative_state_hash === live.roomHead.authoritativeStateHash;
}
