import type { VerifiedRoomReplay } from "@worldstream/client";

import type { MidnightArchiveReadyState } from "./liveAdapter";

export interface MidnightArchiveReplaySummary {
  readonly roomSequence: number;
  readonly packDigest: string;
  readonly lineageHash: string;
  readonly authoritativeStateHash: string;
  readonly projectionHash: string;
  readonly verification: "verified";
}

export type MidnightArchiveReplayState =
  | { readonly kind: "unavailable" }
  | { readonly kind: "available" }
  | { readonly kind: "loading" }
  | { readonly kind: "verified"; readonly summary: MidnightArchiveReplaySummary }
  | { readonly kind: "error"; readonly message: string };

export function replaySummaryFrom(
  replay: VerifiedRoomReplay,
  state: MidnightArchiveReadyState,
): MidnightArchiveReplaySummary | null {
  if (
    state.projection.phase !== "complete"
    || replay.requested_room_seq !== state.roomSequence
    || replay.room_head.room_seq !== state.roomSequence
    || replay.room_head.pack_digest !== state.pack.digest
    || replay.room_head.genesis_or_transition_hash !== state.roomHead.genesisOrTransitionHash
    || replay.room_head.authoritative_state_hash !== state.roomHead.authoritativeStateHash
    || replay.verification !== "verified"
    || !isDigest(replay.projection_hash)
  ) return null;
  return {
    roomSequence: state.roomSequence,
    packDigest: state.pack.digest,
    lineageHash: replay.room_head.genesis_or_transition_hash,
    authoritativeStateHash: replay.room_head.authoritative_state_hash,
    projectionHash: replay.projection_hash,
    verification: "verified",
  };
}

function isDigest(value: unknown): value is string {
  return typeof value === "string" && /^(?:blake3|sha256):[0-9a-f]{64}$/u.test(value);
}
