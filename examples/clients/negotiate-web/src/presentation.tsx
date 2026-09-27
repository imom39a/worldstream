import type { ReactNode } from "react";

import {
  hasActionOffer,
  projectionPhase,
  projectionInteger,
  projectionRecord,
  projectionText,
  projectionTextList,
  safeReference,
  type NegotiateActionOffer,
  type NegotiateConsoleBootstrap,
  type NegotiatePersona,
} from "./model";
import type {
  NegotiateEvidenceDownload,
  NegotiateReplaySummary,
} from "./proofs";
import type { NegotiateActionReceipt } from "./liveAdapter";

export interface NegotiateAppProps {
  readonly session: NegotiateDisplaySession;
  readonly onSubmitPreparedAction?: (offer: NegotiateActionOffer) => void;
  readonly onReconnect?: () => void;
  readonly onOpenReplay?: () => void;
  readonly onDownloadEvidence?: () => void;
  readonly receipt?: NegotiateActionReceipt;
  readonly error?: string | null;
  readonly replaySummary?: NegotiateReplaySummary | null;
  readonly evidenceSummary?: NegotiateEvidenceDownload | null;
  readonly proofError?: string | null;
}

export type NegotiateDisplaySession = Pick<
  NegotiateConsoleBootstrap,
  | "room_sequence"
  | "persona"
  | "projection"
  | "action_offers"
  | "connection"
  | "replay"
  | "evidence"
>;

export function NegotiateApp({
  session,
  onSubmitPreparedAction,
  onReconnect,
  onOpenReplay,
  onDownloadEvidence,
  receipt,
  error = null,
  replaySummary = null,
  evidenceSummary = null,
  proofError = null,
}: NegotiateAppProps) {
  const projection = session.projection;
  const phase = projectionPhase(projection);
  const outcome = projectionRecord(projection, "outcome");

  return (
    <main className="app-shell negotiate-shell">
      <header className="panel negotiate-hero">
        <div>
          <p className="eyebrow">WorldStream Negotiate</p>
          <h1>Agreement room</h1>
          <p className="hero-description">
            One authoritative order for independently operated agents, exact Human
            approval, external signing, restart-safe Replay, and portable evidence.
          </p>
        </div>
        <dl className="negotiate-status-grid">
          <StatusFact label="Phase" value={phaseLabel(phase)} />
          <StatusFact label="Your view" value={personaLabel(session.persona)} />
          <StatusFact label="Room sequence" value={String(session.room_sequence)} />
          <StatusFact label="Connection" value={connectionLabel(session.connection)} />
        </dl>
      </header>

      {session.connection !== "live" ? (
        <section className="panel negotiate-recovery" role="status">
          <div>
            <p className="eyebrow">Realtime state</p>
            <h2>{connectionLabel(session.connection)}</h2>
            <p>
              Actions remain disabled until the independent cursor has caught up to
              the authoritative Room Head.
            </p>
          </div>
          {session.connection === "disconnected" ? (
            <button type="button" onClick={onReconnect}>Reconnect and catch up</button>
          ) : null}
        </section>
      ) : null}

      {error === null && (receipt === undefined || receipt.state === "idle") ? null : (
        <section className="panel negotiate-receipt" role={error === null ? "status" : "alert"}>
          <p className="eyebrow">Latest submission</p>
          <h2>{error ?? receipt?.message ?? "No receipt is available."}</h2>
          {receipt?.code === null || receipt?.code === undefined ? null : (
            <code>{receipt.code}</code>
          )}
        </section>
      )}

      <section className="panel">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Authorized projection</p>
            <h2>{personaHeading(session.persona)}</h2>
          </div>
          <span className="privacy-label">Server-scoped · {personaLabel(session.persona)}</span>
        </div>
        <SharedStatus projection={projection} />
        <PersonaPanel session={session} />
      </section>

      <section className="panel">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Exact Action Offers</p>
            <h2>Actions currently admitted for this Membership</h2>
          </div>
          <span className="privacy-label">No hidden client-side permission rules</span>
        </div>
        {session.action_offers.length === 0 ? (
          <p className="status-message">No Action is currently offered to this view.</p>
        ) : (
          <div className="negotiate-actions">
            {session.action_offers.map((offer) => (
              <article className="negotiate-action" key={offer.action_type}>
                <div>
                  <strong>{actionLabel(offer.action_type)}</strong>
                  <span>{actionDescription(offer.action_type)}</span>
                  {offer.payload_schema_digest === null ? null : (
                    <code>{shortIdentity(offer.payload_schema_digest)}</code>
                  )}
                </div>
                <button
                  disabled={session.connection !== "live" || onSubmitPreparedAction === undefined}
                  onClick={() => onSubmitPreparedAction?.(offer)}
                  type="button"
                >
                  Prepare exact action
                </button>
              </article>
            ))}
          </div>
        )}
        <p className="negotiate-boundary-note">
          Signing keys and strategy stay outside WorldStream. This console submits only
          an externally prepared payload against the current Room and A202 heads.
        </p>
      </section>

      <section className="panel negotiate-proof-panel">
        <div>
          <p className="eyebrow">Replay and dual proof</p>
          <h2>Verify what the parties signed and what the venue ordered</h2>
          <p>
            Replay is read-only. Evidence combines exact A202 party objects with
            WorldStream transition, head, timer, and Pack identities.
          </p>
        </div>
        <div className="negotiate-proof-actions">
          <button
            disabled={session.replay !== "available" || onOpenReplay === undefined}
            onClick={onOpenReplay}
            type="button"
          >
            Open verified Replay
          </button>
          <button
            disabled={session.evidence !== "available" || onDownloadEvidence === undefined}
            onClick={onDownloadEvidence}
            type="button"
          >
            Download evidence bundle
          </button>
        </div>
        {proofError === null ? null : <p className="negotiate-proof-error" role="alert">{proofError}</p>}
        {replaySummary === null ? null : (
          <dl className="negotiate-proof-summary" aria-label="Verified Replay summary">
            <StatusFact label="Replay" value={`Verified at sequence ${replaySummary.requested_room_seq}`} />
            <StatusFact label="Pack revision" value={shortIdentity(replaySummary.pack_revision_digest)} />
            <StatusFact label="Lineage" value={shortIdentity(replaySummary.lineage_hash)} />
            <StatusFact label="Projection" value={shortIdentity(replaySummary.projection_hash)} />
          </dl>
        )}
        {evidenceSummary === null ? null : (
          <dl className="negotiate-proof-summary" aria-label="Downloaded dual evidence summary">
            <StatusFact label="Party objects" value={String(evidenceSummary.party_object_count)} />
            <StatusFact label="Venue records" value={String(evidenceSummary.venue_record_count)} />
            <StatusFact label="Cross-index entries" value={String(evidenceSummary.cross_index_count)} />
            <StatusFact label="Offline verifier" value="Required for cryptographic verification" />
          </dl>
        )}
      </section>

      {outcome === null ? null : (
        <section className="panel negotiate-outcome">
          <p className="eyebrow">Outcome</p>
          <h2>{outcomeLabel(projectionText(outcome, "kind"))}</h2>
          <p>The terminal result is part of the authoritative replayable Room state.</p>
        </section>
      )}
    </main>
  );
}

function SharedStatus({ projection }: { projection: NegotiateConsoleBootstrap["projection"] }) {
  return (
    <dl className="negotiate-shared-grid">
      <StatusFact label="Aggregate" value={projectionText(projection, "aggregate_state") ?? "Unavailable"} />
      <StatusFact label="A202 session" value={projectionText(projection, "session_state") ?? "Unavailable"} />
      <StatusFact label="Deadline" value={projectionText(projection, "deadline_progress") ?? "Not exposed"} />
    </dl>
  );
}

function PersonaPanel({ session }: { session: NegotiateDisplaySession }) {
  const projection = session.projection;
  if (session.persona === "spectator") {
    return <BoundaryCopy>Public status only. Commercial objects, approvals, signatures, and evidence are withheld.</BoundaryCopy>;
  }
  if (session.persona === "operator") {
    const proposal = safeReference(projectionRecord(projection, "current_proposal_reference"));
    return (
      <div className="negotiate-persona-panel">
        <ReferenceCard title="Current proposal reference" reference={proposal} />
        <StatusFact label="Collected signatures" value={numberText(projection.signature_count)} />
        <StatusFact label="Evidence links" value={numberText(projection.evidence_count)} />
        <BoundaryCopy>Operator diagnostics expose counts and identities, never another participant's private projection.</BoundaryCopy>
      </div>
    );
  }
  if (session.persona === "buyer_approver") {
    const proposal = safeReference(projectionRecord(projection, "current_proposal"));
    const pendingRecord = projectionRecord(projection, "pending_approval");
    const pending = safeReference(pendingRecord);
    const candidate = pendingRecord === null ? null : projectionText(pendingRecord, "candidate_canonical_json");
    const expiry = pendingRecord === null ? null : projectionInteger(pendingRecord, "expires_at");
    return (
      <div className="negotiate-persona-panel">
        <ReferenceCard title="Proposal requiring exact review" reference={proposal} />
        <ReferenceCard title="Approval binding" reference={pending} />
        <HeadCard title="Room Head at request" head={pendingRecord === null ? null : projectionRecord(pendingRecord, "room_head_at_request")} />
        <HeadCard title="A202 transaction Head at request" head={pendingRecord === null ? null : projectionRecord(pendingRecord, "transaction_head_at_request")} />
        <HeadCard title="A202 session Head at request" head={pendingRecord === null ? null : projectionRecord(pendingRecord, "session_head_at_request")} />
        <StatusFact label="Approval expires at" value={expiry === null ? "Unavailable" : String(expiry)} />
        <ExactBytesCard title="Exact acceptance candidate bytes" value={candidate} />
        <BoundaryCopy>
          Approval is valid only for the exact candidate bytes, proposal identity,
          Room/A202 heads, approver, and expiry shown by the server projection.
        </BoundaryCopy>
      </div>
    );
  }
  if (session.persona === "venue_signer") {
    const proposal = safeReference(projectionRecord(projection, "current_proposal_reference"));
    const committed = safeReference(projectionRecord(projection, "committed_agreement_reference"));
    const eventCandidates = projectionTextList(projection, "event_candidates_required");
    return (
      <div className="negotiate-persona-panel">
        <ReferenceCard title="Proposal reference" reference={proposal} />
        <ReferenceCard title="Committed agreement reference" reference={committed} />
        <HeadCard title="A202 transaction Head" head={projectionRecord(projection, "transaction_head")} />
        <HeadCard title="A202 session Head" head={projectionRecord(projection, "session_head")} />
        <StatusFact label="Deadline progress" value={projectionText(projection, "deadline_progress") ?? "Unavailable"} />
        <StatusFact label="Required signed event candidates" value={eventCandidates.length === 0 ? "None offered" : eventCandidates.join(", ")} />
        <BoundaryCopy>
          The venue records independently signed protocol evidence; WorldStream never
          obtains or uses the venue signing key.
        </BoundaryCopy>
      </div>
    );
  }

  const proposal = safeReference(projectionRecord(projection, "current_proposal"));
  const agreement = safeReference(projectionRecord(projection, "agreement"));
  return (
    <div className="negotiate-persona-panel">
      <ReferenceCard title="Current proposal" reference={proposal} />
      <ReferenceCard title="Agreement" reference={agreement} />
      {session.persona === "buyer_agent" && hasActionOffer(session, "accept_current_proposal") ? (
        <BoundaryCopy>The exact approved candidate can now be accepted at the current heads.</BoundaryCopy>
      ) : null}
      {session.persona === "seller_agent" ? (
        <BoundaryCopy>The approval view excludes the buyer's private acceptance candidate bytes.</BoundaryCopy>
      ) : null}
    </div>
  );
}

function ReferenceCard({
  title,
  reference,
}: {
  title: string;
  reference: ReturnType<typeof safeReference>;
}) {
  return (
    <article className="negotiate-reference">
      <strong>{title}</strong>
      {reference.id === null && reference.hash === null && reference.digest === null ? (
        <span>Not available in this phase or view.</span>
      ) : (
        <dl>
          {reference.id === null ? null : <StatusFact label="Object" value={reference.id} />}
          {reference.hash === null ? null : <StatusFact label="Content hash" value={shortIdentity(reference.hash)} />}
          {reference.digest === null ? null : <StatusFact label="Wire digest" value={shortIdentity(reference.digest)} />}
        </dl>
      )}
    </article>
  );
}

function HeadCard({ title, head }: { title: string; head: ReturnType<typeof projectionRecord> }) {
  const sequence = head === null ? null : projectionInteger(head, "sequence");
  const digest = head === null
    ? null
    : projectionText(head, "digest") ?? projectionText(head, "event_hash");
  return (
    <article className="negotiate-reference">
      <strong>{title}</strong>
      {sequence === null || digest === null ? (
        <span>Not available in this phase or view.</span>
      ) : (
        <dl>
          <StatusFact label="Sequence" value={String(sequence)} />
          <StatusFact label="Digest" value={shortIdentity(digest)} />
        </dl>
      )}
    </article>
  );
}

function ExactBytesCard({ title, value }: { title: string; value: string | null }) {
  return (
    <article className="negotiate-exact-bytes">
      <strong>{title}</strong>
      {value === null ? <span>Not available in this phase or view.</span> : <pre>{value}</pre>}
    </article>
  );
}

function StatusFact({ label, value }: { label: string; value: string }) {
  return <div><dt>{label}</dt><dd>{value}</dd></div>;
}

function BoundaryCopy({ children }: { children: ReactNode }) {
  return <p className="negotiate-boundary-note">{children}</p>;
}

function personaLabel(persona: NegotiatePersona): string {
  const labels: Record<NegotiatePersona, string> = {
    buyer_agent: "Buyer agent",
    seller_agent: "Seller agent",
    buyer_approver: "Human buyer approver",
    venue_signer: "Venue signer",
    operator: "Operator member",
    spectator: "Spectator",
  };
  return labels[persona];
}

function personaHeading(persona: NegotiatePersona): string {
  return `${personaLabel(persona)} workspace`;
}

function phaseLabel(phase: string): string {
  return phase.split("_").map(capitalize).join(" ");
}

function connectionLabel(connection: NegotiateDisplaySession["connection"]): string {
  if (connection === "catching_up") return "Catching up";
  return capitalize(connection);
}

function actionLabel(action: string): string {
  return action.split("_").map(capitalize).join(" ");
}

function actionDescription(action: string): string {
  if (action === "record_exact_approval") return "Review and bind a Human decision to the exact candidate.";
  if (action === "record_agreement_signature") return "Submit externally signed agreement bytes without exposing a key.";
  if (action === "commit_agreement") return "Commit only after every required independent signature is present.";
  if (action.includes("deadline")) return "Record signed venue evidence for deterministic deadline resolution.";
  return "Prepare the typed payload outside the runtime, then submit it at the current exact heads.";
}

function outcomeLabel(kind: string | null): string {
  if (kind === "agreement_committed") return "Agreement committed";
  if (kind === "formation_expired") return "Formation expired";
  return "Terminal outcome";
}

function numberText(value: unknown): string {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? String(value)
    : "Unavailable";
}

function shortIdentity(value: string): string {
  return value.length > 30 ? `${value.slice(0, 18)}…${value.slice(-8)}` : value;
}

function capitalize(value: string): string {
  return value.length === 0 ? value : `${value[0]!.toUpperCase()}${value.slice(1)}`;
}
