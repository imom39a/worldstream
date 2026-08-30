import { useEffect, useMemo, useRef } from "react";

import {
  NegotiateApp,
  preparedActionMatches,
  type NegotiateDisplaySession,
  type PendingNegotiatePreparation,
} from "./NegotiateApp";
import {
  readNegotiateAuthorizedView,
  type NegotiateActionOffer,
} from "./negotiate";
import type { NegotiateReplaySummary } from "./negotiateProofs";
import type {
  ParticipantBrowserObservation,
  ParticipantBrowserReplay,
} from "./participantHandoff";
import type { ParticipantConsoleAction } from "./participantSession";

const DIGEST = /^(?:blake3|sha256):[0-9a-f]{64}$/;

export interface NegotiateRetainedSessionView {
  readonly session: NegotiateDisplaySession;
  readonly observation: ParticipantBrowserObservation;
}

/** Select the Pack renderer from the pinned identity, never from UI guesswork. */
export function readNegotiateRetainedSessionView(
  observation: ParticipantBrowserObservation,
): NegotiateRetainedSessionView | null {
  if (
    observation.pack.id !== "worldstream.negotiate"
    || observation.pack.digest !== observation.room_head.pack_digest
  ) return null;
  for (const delivery of [...observation.delivery].reverse()) {
    const body = record(delivery.body);
    const root = delivery.kind === "projection_reset"
      ? record(body.projection)
      : record(body.observation);
    const envelope = "projection" in root ? record(root.projection) : root;
    const projection = "activity" in envelope
      ? record(envelope.activity)
      : envelope;
    const offers = Array.isArray(envelope.action_offers)
      ? envelope.action_offers
      : Array.isArray(root.action_offers)
        ? root.action_offers
        : Array.isArray(projection.action_offers)
          ? projection.action_offers
          : [];
    const authorized = readNegotiateAuthorizedView(
      projection,
      projection.persona,
      offers,
    );
    if (authorized === null) continue;
    return {
      observation,
      session: {
        room_sequence: observation.room_head.room_seq,
        persona: authorized.persona,
        projection: authorized.projection,
        // The retained supervisor submit contract binds an exact schema digest.
        // Legacy string-only offers remain displayable on the direct protocol
        // path, but must never become actionable through this generic proxy.
        action_offers: authorized.action_offers.filter(
          (offer) => offer.payload_schema_digest !== null,
        ),
        connection: "live",
        replay: "available",
        evidence: "unavailable",
      },
    };
  }
  return null;
}

export function readRetainedNegotiateReplaySummary(
  replay: ParticipantBrowserReplay | null | undefined,
  observation: ParticipantBrowserObservation,
): NegotiateReplaySummary | null {
  if (replay === null || replay === undefined) return null;
  if (
    replay.requested_room_seq !== observation.room_head.room_seq
    || replay.room_head.room_seq !== observation.room_head.room_seq
    || replay.room_head.pack_digest !== observation.pack.digest
    || !isDigest(replay.room_head.genesis_or_transition_hash)
    || !isDigest(replay.room_head.authoritative_state_hash)
    || !isDigest(replay.projection_hash)
  ) return null;
  return {
    room_id: null,
    requested_room_seq: replay.requested_room_seq,
    pack_id: "worldstream.negotiate",
    pack_revision_digest: observation.pack.digest,
    lineage_hash: replay.room_head.genesis_or_transition_hash,
    authoritative_state_hash: replay.room_head.authoritative_state_hash,
    projection_hash: replay.projection_hash,
    verification: "verified",
  };
}

export function NegotiateRetainedParticipant({
  view,
  replay,
  onAct,
  onReplay,
}: {
  readonly view: NegotiateRetainedSessionView;
  readonly replay?: ParticipantBrowserReplay | null;
  readonly onAct: (action: ParticipantConsoleAction) => Promise<void>;
  readonly onReplay?: () => Promise<void>;
}) {
  const pending = useRef<PendingNegotiatePreparation | null>(null);
  const latest = useRef(view);
  latest.current = view;
  const replaySummary = useMemo(
    () => readRetainedNegotiateReplaySummary(replay, view.observation),
    [replay, view.observation],
  );

  useEffect(() => {
    const submit = (event: Event) => {
      if (!(event instanceof CustomEvent)) return;
      const current = pending.current;
      if (
        current === null
        || !preparedActionMatches(
          current,
          event.detail,
          latest.current.session.room_sequence,
        )
      ) return;
      pending.current = null;
      const offerIndex = latest.current.session.action_offers.findIndex(
        (offer) => sameOffer(offer, current.offer),
      );
      if (offerIndex < 0) return;
      const schemaDigest = current.offer.payload_schema_digest;
      if (schemaDigest === null) return;
      const detail = event.detail as { payload: unknown };
      void onAct({
        actionId: nextUlid(),
        basedOnRoomSeq: current.based_on_room_seq,
        offerId: `${current.based_on_room_seq}:${current.offer.action_type}:${offerIndex}`,
        schemaDigest,
        actionType: current.offer.action_type,
        payload: detail.payload,
      });
    };
    window.addEventListener("worldstream:negotiate-prepared-action", submit);
    return () => window.removeEventListener("worldstream:negotiate-prepared-action", submit);
  }, [onAct]);

  const requestPreparation = (offer: NegotiateActionOffer) => {
    const requestId = crypto.randomUUID();
    pending.current = {
      request_id: requestId,
      offer,
      based_on_room_seq: view.session.room_sequence,
    };
    window.dispatchEvent(new CustomEvent("worldstream:negotiate-retained-action-requested", {
      detail: {
        request_id: requestId,
        action_type: offer.action_type,
        payload_schema_digest: offer.payload_schema_digest,
        based_on_room_seq: view.session.room_sequence,
        authority_binding: "retained_http_only_participant_session",
      },
    }));
  };

  return (
    <NegotiateApp
      session={view.session}
      onSubmitPreparedAction={requestPreparation}
      onOpenReplay={onReplay === undefined ? undefined : () => void onReplay()}
      replaySummary={replaySummary}
      proofError={replay !== null && replay !== undefined && replaySummary === null
        ? "Verified Replay did not match this exact retained Room Head."
        : null}
    />
  );
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function sameOffer(left: NegotiateActionOffer, right: NegotiateActionOffer): boolean {
  return left.action_type === right.action_type
    && left.payload_schema_digest === right.payload_schema_digest;
}

function isDigest(value: unknown): value is string {
  return typeof value === "string" && DIGEST.test(value);
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
