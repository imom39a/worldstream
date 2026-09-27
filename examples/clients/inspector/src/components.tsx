import { useState } from "react";
import type { FormEvent, ReactNode } from "react";

import type {
  ActionOffer,
  AttentionSignal,
  DeliveryState,
  HeistFixture,
  HeistActionType,
  MembershipSummary,
  OperatorDiagnostics,
  PublicChallenge,
  PublicClue,
  PublicPlan,
  RolePresence,
  RuntimeStatus,
} from "./fixture";
import type { LiveSessionView } from "./liveSession";

interface SectionProps {
  eyebrow: string;
  title: string;
  description?: string;
  children: ReactNode;
  className?: string;
}

export function Section({ eyebrow, title, description, children, className = "" }: SectionProps) {
  return (
    <section className={`panel ${className}`}>
      <div className="section-heading">
        <div>
          <p className="eyebrow">{eyebrow}</p>
          <h2>{title}</h2>
        </div>
        {description ? <p className="section-description">{description}</p> : null}
      </div>
      {children}
    </section>
  );
}

export function PrivacyLabel({ children }: { children: ReactNode }) {
  return <span className="privacy-label">{children}</span>;
}

export function StatusPill({ children, tone = "neutral" }: { children: ReactNode; tone?: string }) {
  return <span className={`status-pill status-${tone}`}>{children}</span>;
}

export function PhaseHeader({ fixture }: { fixture: HeistFixture }) {
  return (
    <header className="hero panel">
      <div className="hero-copy">
        <div className="hero-kicker">
          <span className="brand-mark" aria-hidden="true">
            ◈
          </span>
          <span>WorldStream / first-party reference</span>
        </div>
        <p className="eyebrow">Agent Heist console</p>
        <h1>{fixture.roomLabel}</h1>
        <p className="hero-description">{fixture.phaseDescription}</p>
        <div className="hero-meta">
          <PrivacyLabel>Public projection</PrivacyLabel>
          <span className="meta-divider" aria-hidden="true" />
          <span>{fixture.fixtureLabel}</span>
        </div>
      </div>
      <div className="phase-card" aria-label="Current phase and deadline">
        <span className="phase-label">Current phase</span>
        <strong>{fixture.phase}</strong>
        <span className="deadline-label">Deadline</span>
        <span className="deadline">{fixture.deadline}</span>
        <span className="sequence">Phase generation {fixture.phaseGeneration}</span>
        <span className="sequence">Room sequence {fixture.roomSequence}</span>
      </div>
    </header>
  );
}

export function DiscoveryPanel({ fixture }: { fixture: HeistFixture }) {
  return (
    <Section
      eyebrow="Discovery"
      title="Room and membership scope"
      description="Discovery is metadata only. It does not grant access or let the client select another membership by changing an ID."
      className="discovery-panel"
    >
      <div className="discovery-metrics">
        <DiscoveryMetric label="Room" value={fixture.discovery.roomRef} />
        <DiscoveryMetric label="Availability" value={fixture.discovery.status} />
        <DiscoveryMetric label="Protocol" value={`WorldStream ${fixture.discovery.protocol}`} />
        <DiscoveryMetric label="Activity Pack" value={fixture.discovery.activityPack} />
      </div>
      <div className="discovery-views" aria-label="Available console views">
        <span className="muted-label">Available views</span>
        {fixture.discovery.availableViews.map((view) => (
          <StatusPill key={view} tone="neutral">
            {view}
          </StatusPill>
        ))}
      </div>
      <MembershipStatusList memberships={fixture.memberships} />
    </Section>
  );
}

function DiscoveryMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="discovery-metric">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function MembershipStatusList({ memberships }: { memberships: MembershipSummary[] }) {
  return (
    <div className="membership-status" aria-label="Room membership status">
      <div className="subsection-heading">
        <div>
          <span className="eyebrow">Membership status</span>
          <h3>Scoped seats</h3>
        </div>
        <PrivacyLabel>Public-safe metadata</PrivacyLabel>
      </div>
      <div className="membership-grid">
        {memberships.map((membership) => (
          <article className="membership-card" key={membership.role}>
            <div className="membership-card-heading">
              <div>
                <h3>{membership.role}</h3>
                <span>{membership.principalKind} · {membership.accessMode}</span>
              </div>
              <StatusPill tone={membership.standing === "Enabled" ? "good" : "attention"}>
                {membership.standing}
              </StatusPill>
            </div>
            <div className="membership-card-meta">
              <span>Session</span>
              <strong>{membership.session}</strong>
              <code>{membership.memberRef}</code>
            </div>
          </article>
        ))}
      </div>
    </div>
  );
}

export function RuntimeStatusPanel({ runtime }: { runtime: RuntimeStatus }) {
  const transportTone = runtime.transport === "Live" ? "good" : "attention";
  const healthTone = runtime.roomHealth === "Healthy" ? "good" : "danger";
  return (
    <Section
      eyebrow="Runtime and availability"
      title="Connection truth"
      description="These states come from the transport/runtime boundary. Fixture data never stands in for a live Session, Frame, or Action receipt."
      className="runtime-panel"
    >
      <div className="runtime-banner" role="status" aria-live="polite">
        <div>
          <strong>{runtime.reason}</strong>
          <span>Current transport · {runtime.transport} · Recovery · {runtime.recovery}</span>
        </div>
        <StatusPill tone={transportTone}>{runtime.transport}</StatusPill>
      </div>
      <div className="runtime-summary">
        <RuntimeMetric label="Room health" value={runtime.roomHealth} tone={healthTone} />
        <RuntimeMetric label="Frame delivery" value={runtime.frame.delivery} />
        <RuntimeMetric label="Projection reset" value={runtime.frame.reset} />
        <RuntimeMetric label="Session sync ACK" value={runtime.frame.syncAck} />
        <RuntimeMetric label="Observation ACK" value={runtime.frame.observationAck} />
      </div>
      <RuntimeStateMatrix current={runtime.recovery} />
      <IntegrityStateMatrix current={runtime.roomHealth} />
    </Section>
  );
}

function RuntimeMetric({ label, value, tone = "neutral" }: { label: string; value: string; tone?: string }) {
  return (
    <div className="runtime-metric">
      <span>{label}</span>
      <StatusPill tone={tone}>{value}</StatusPill>
    </div>
  );
}

const runtimeStates: Array<{ state: RuntimeStatus["recovery"]; label: string; description: string }> = [
  { state: "Loading", label: "Loading", description: "No current Projection or mutation controls are trusted." },
  { state: "CatchingUp", label: "Catching up", description: "Retained Frames or a Projection Reset are being installed." },
  { state: "Active", label: "Active", description: "A synchronized Session may render current authorized data." },
  { state: "Passivating", label: "Passivating", description: "Runtime work is winding down; normal mutation is not assumed." },
  { state: "Inactive", label: "Inactive", description: "No runtime Session is serving this fixture." },
];

function RuntimeStateMatrix({ current }: { current: RuntimeStatus["recovery"] }) {
  return (
    <div className="runtime-state-grid" aria-label="Runtime recovery states">
      {runtimeStates.map((item) => (
        <div className={`runtime-state ${item.state === current ? "is-current" : ""}`} key={item.state}>
          <div className="runtime-state-heading">
            <strong>{item.label}</strong>
            {item.state === current ? <StatusPill tone="accent">Current</StatusPill> : null}
          </div>
          <span>{item.description}</span>
        </div>
      ))}
    </div>
  );
}

function IntegrityStateMatrix({ current }: { current: RuntimeStatus["roomHealth"] }) {
  const states: Array<{ state: RuntimeStatus["roomHealth"]; description: string }> = [
    { state: "Healthy", description: "Current authorized projection and exact-head actions may be used." },
    { state: "Faulted", description: "Last verified projection may remain visible; canonical mutation is disabled." },
    { state: "Quarantined", description: "Normal projection and mutation are removed; diagnostics remain bounded." },
  ];
  return (
    <div className="integrity-state-grid" aria-label="Room integrity states">
      {states.map((item) => (
        <div className={`runtime-state ${item.state === current ? "is-current" : ""}`} key={item.state}>
          <div className="runtime-state-heading">
            <strong>{item.state}</strong>
            {item.state === current ? <StatusPill tone="accent">Current</StatusPill> : null}
          </div>
          <span>{item.description}</span>
        </div>
      ))}
    </div>
  );
}

export function RolePresenceGrid({ roles }: { roles: RolePresence[] }) {
  return (
    <div className="role-grid">
      {roles.map((role) => (
        <article className="role-card" key={role.role}>
          <div className="role-card-topline">
            <span className={`presence-dot presence-${role.presence.toLowerCase()}`} aria-hidden="true" />
            <StatusPill tone={role.presence === "Present" ? "good" : "attention"}>{role.presence}</StatusPill>
          </div>
          <h3>{role.role}</h3>
          <p>{role.principalKind} participant</p>
          <span className="muted-label">Activation · {role.activation}</span>
        </article>
      ))}
    </div>
  );
}

export function PublicClueList({ clues }: { clues: PublicClue[] }) {
  return (
    <div className="stack-list">
      {clues.map((clue) => (
        <article className="list-row" key={clue.id}>
          <div>
            <h3>{clue.label}</h3>
            <p>{clue.claim}</p>
          </div>
          <StatusPill tone={clue.state === "Published" ? "good" : "attention"}>{clue.state}</StatusPill>
        </article>
      ))}
    </div>
  );
}

export function PublicPlanList({ plans }: { plans: PublicPlan[] }) {
  return (
    <div className="plan-grid">
      {plans.map((plan) => (
        <article className="plan-card" key={plan.id}>
          <div className="plan-card-heading">
            <span className="plan-index">{plan.id.replace("plan-", "").toUpperCase()}</span>
            <StatusPill tone={plan.status === "Leading" ? "accent" : "neutral"}>{plan.status}</StatusPill>
          </div>
          <h3>{plan.label}</h3>
          <code data-plan-id={plan.id}>plan_id · {plan.id}</code>
          <div className="metric-pair">
            <span>
              <strong>{plan.endorsements}</strong> endorsements
            </span>
            <span>
              <strong>{plan.challenges}</strong> challenges
            </span>
          </div>
        </article>
      ))}
    </div>
  );
}

export function ChallengeList({ challenges }: { challenges: PublicChallenge[] }) {
  return (
    <div className="stack-list">
      {challenges.map((challenge) => (
        <article className="list-row" key={challenge.id}>
          <div>
            <h3>{challenge.target}</h3>
            <p>{challenge.reason}</p>
          </div>
          <StatusPill tone={challenge.state === "Open" ? "attention" : "good"}>{challenge.state}</StatusPill>
        </article>
      ))}
    </div>
  );
}

export function CommitmentSummary({ submitted, total }: HeistFixture["commitmentCount"]) {
  return (
    <div className="commitment-summary">
      <div className="commitment-count">
        <strong>{submitted}</strong>
        <span>/ {total} commitments submitted</span>
      </div>
      <div className="commitment-bar" aria-label={`${submitted} of ${total} commitments submitted`}>
        <span style={{ width: `${(submitted / total) * 100}%` }} />
      </div>
      <p>Individual commitment values and contributor identities are withheld from this public result.</p>
    </div>
  );
}

export function AggregateResultCard({ fixture }: { fixture: HeistFixture }) {
  const { result } = fixture;
  return (
    <div className="result-card">
      <div className="result-card-heading">
        <div>
          <span className="phase-label">Aggregate result</span>
          <h3>{result.outcome}</h3>
        </div>
        <StatusPill tone={result.availability === "Available" ? "good" : "neutral"}>{result.availability}</StatusPill>
      </div>
      <div className="result-summary-grid">
        <div>
          <span>Selected plan</span>
          <strong>{result.selectedPlan}</strong>
        </div>
        <div>
          <span>Vote summary</span>
          <strong>{result.voteSummary}</strong>
        </div>
        <div>
          <span>Checks</span>
          <strong>
            {result.checksPassed}/{result.checksTotal}
          </strong>
        </div>
        <div>
          <span>Score band</span>
          <strong>{result.scoreLabel}</strong>
        </div>
      </div>
      <PrivacyLabel>Public aggregate only · individual commitments remain withheld</PrivacyLabel>
    </div>
  );
}

const deliveryStates: Array<{ state: DeliveryState; label: string; description: string }> = [
  { state: "exact-head", label: "Exact head", description: "Current offers may be evaluated." },
  { state: "stale-head", label: "Stale head", description: "The prior Action is permanently tied to its old Head; create no retry until sync completes." },
  { state: "resync-required", label: "Resync required", description: "Do not submit until a reset is installed." },
  { state: "catching-up", label: "Catching up", description: "Retained frames are still being applied." },
  { state: "faulted", label: "Faulted", description: "Canonical mutation remains disabled." },
  { state: "quarantined", label: "Quarantined", description: "Only bounded operator diagnostics remain." },
];

export function DeliveryStateMatrix({ current = "exact-head" }: { current?: DeliveryState }) {
  return (
    <div className="delivery-grid" aria-label="Participant delivery state matrix">
      {deliveryStates.map((item) => (
        <div className={`delivery-state ${item.state === current ? "is-current" : ""}`} key={item.state}>
          <div className="delivery-state-heading">
            <span className="state-marker" aria-hidden="true" />
            <strong>{item.label}</strong>
            {item.state === current ? <StatusPill tone="accent">Fixture current</StatusPill> : null}
          </div>
          <span>{item.description}</span>
        </div>
      ))}
    </div>
  );
}

export function AttentionFramePanel({ attention, runtime }: { attention: AttentionSignal[]; runtime: RuntimeStatus }) {
  return (
    <Section
      eyebrow="Attention and Frames"
      title="Delivery lifecycle"
      description="Attention is a bounded signal, not an invocation. Frame and acknowledgement states remain unavailable until this client has a real authorized Session."
    >
      <div className="attention-list">
        {attention.map((signal) => (
          <article className="attention-card" key={signal.id}>
            <div className="attention-card-heading">
              <div>
                <span className="eyebrow">{signal.reason}</span>
                <h3>{signal.target}</h3>
              </div>
              <StatusPill tone={signal.status === "Pending" ? "attention" : "neutral"}>{signal.status}</StatusPill>
            </div>
            <div className="attention-meta">
              <span>Priority {signal.priority}</span>
              <span>Deadline {signal.deadline}</span>
              <span>Offers · {signal.actionTypes.join(", ")}</span>
            </div>
            <code>deduplication · {signal.deduplicationKey}</code>
          </article>
        ))}
      </div>
      <div className="frame-lifecycle" aria-label="Session frame lifecycle">
        <LifecycleRow label="Attach" value={runtime.transport === "Unavailable" ? "Unavailable" : runtime.transport} />
        <LifecycleRow label="Retained catch-up" value={runtime.frame.delivery} />
        <LifecycleRow label="Projection reset" value={runtime.frame.reset} />
        <LifecycleRow label="Session sync ACK" value={runtime.frame.syncAck} />
        <LifecycleRow label="Observation ACK" value={runtime.frame.observationAck} />
      </div>
      <p className="frame-note">
        A stale Action result remains tied to its original Action ID and Head. The client must install retained Frames or a
        complete Projection Reset, recompute the exact offers, and create a new Action ID; it never rebases the old request.
      </p>
    </Section>
  );
}

function LifecycleRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="lifecycle-row">
      <span>{label}</span>
      <StatusPill tone={value === "Live" || value === "Sent" ? "good" : "attention"}>{value}</StatusPill>
    </div>
  );
}

export function ParticipantControls({ fixture, live }: { fixture: HeistFixture; live?: LiveSessionView | null }) {
  const action = live?.action;
  const [resyncNotice, setResyncNotice] = useState<string | null>(null);
  const canSubmit = live?.status === "live" && fixture.runtime.recovery === "Active" && fixture.runtime.roomHealth === "Healthy" && fixture.runtime.frame.delivery === "Live" && action?.state !== "submitting" && action?.state !== "stale" && action?.state !== "resync-required";
  const offersCurrent = fixture.runtime.recovery !== "Loading" && fixture.runtime.recovery !== "CatchingUp" && fixture.runtime.roomHealth !== "Quarantined";
  const deliveryState: DeliveryState = action?.state === "stale"
    ? "stale-head"
    : action?.state === "resync-required" || live?.status === "attached"
      ? "resync-required"
      : fixture.runtime.roomHealth === "Quarantined"
        ? "quarantined"
        : fixture.runtime.roomHealth === "Faulted"
          ? "faulted"
          : fixture.runtime.recovery === "CatchingUp"
            ? "catching-up"
            : "exact-head";
  return (
    <>
      <Section
        eyebrow="Participant membership"
        title="Current Action Offers"
        description={canSubmit ? "Exact typed offers are submitted only through the attached WorldStream Session." : "Exact typed offers are shown as a disabled fixture surface. No action is sent from this console."}
      >
        <div className="participant-banner">
          <div>
          <strong>{action?.message ?? fixture.participant.fixtureGate}</strong>
          <span>Participant-only projection · not an operator or public permission grant</span>
        </div>
          <StatusPill tone={canSubmit ? "good" : action?.state === "stale" ? "danger" : "attention"}>{canSubmit ? "Exact head" : action?.state ?? "Disabled"}</StatusPill>
        </div>
        {live && (action?.state === "stale" || action?.state === "resync-required") ? (
          <div className="resync-banner" role="status">
            <span>{action.message ?? "A synchronized Room Head is required before a new Action ID can be created."}</span>
            <button type="button" data-testid="resync-session" onClick={() => { setResyncNotice("Installing a fresh Projection Reset…"); void live.resync().catch(() => setResyncNotice("Resync is unavailable; no Action was submitted.")); }}>
              Resync and create new Action ID
            </button>
            {resyncNotice ? <small>{resyncNotice}</small> : null}
          </div>
        ) : null}
        {fixture.participant.privateClues.length > 0 ? (
          <div className="participant-private-clues" data-testid="participant-private-clues">
            <div className="subsection-heading">
              <div>
                <span className="eyebrow">Participant-only knowledge</span>
                <h3>Known clues available for disclosure</h3>
              </div>
              <PrivacyLabel>Authorized to this Participant</PrivacyLabel>
            </div>
            {fixture.participant.privateClues.map((clue) => <div className="private-clue-row" key={clue.clueId}><span>{clue.clueId}</span><code data-private-claim={clue.clueId}>{clue.claimCode}</code></div>)}
          </div>
        ) : null}
        {offersCurrent ? (
          <div className="offer-list">
            {fixture.participant.offers.map((offer) => (
              <ActionOfferRow key={offer.id} offer={offer} fixture={fixture} live={live} canSubmit={canSubmit} />
            ))}
          </div>
        ) : (
          <div className="deferred-surface" role="status">
            <strong>Current Action Offers withheld</strong>
            <span>Install the authorized Projection Reset or retained Catch-up before rendering participant offers.</span>
          </div>
        )}
        <div className="exact-head-note">
          <span className="eyebrow">Exact-head basis</span>
          <code>seq {fixture.roomSequence} · {fixture.participant.exactHead}</code>
          <span>Any stale response requires a new synchronized Action ID; the UI never rebases it.</span>
        </div>
      </Section>
      <Section
        eyebrow="Transport truth"
        title="Synchronization and integrity states"
        description="The matrix makes stale, recovery, and fail-closed states visible without claiming a live connection."
      >
        <DeliveryStateMatrix current={deliveryState} />
      </Section>
      <AttentionFramePanel attention={fixture.attention} runtime={fixture.runtime} />
    </>
  );
}

interface ActionField {
  name: string;
  label: string;
  kind: "text" | "select" | "checkbox";
  placeholder?: string;
  options?: string[];
}

const actionFields: Record<HeistActionType, ActionField[]> = {
  inspect_clue: [{ name: "clue_id", label: "Clue", kind: "text", placeholder: "clue id from the installed offer" }],
  publish_clue: [
    { name: "clue_id", label: "Clue", kind: "text", placeholder: "clue id" },
    { name: "claim_code", label: "Claim code", kind: "text", placeholder: "published claim code" },
  ],
  offer_exchange: [
    { name: "recipient_role", label: "Recipient role", kind: "select", options: ["Navigator", "Insider", "Broker"] },
    { name: "offered_clue_id", label: "Offered clue", kind: "text", placeholder: "clue id" },
    { name: "consideration", label: "Consideration", kind: "text", placeholder: "structured consideration" },
  ],
  accept_exchange: [{ name: "offer_id", label: "Offer", kind: "text", placeholder: "offer id from the installed offer" }],
  propose_plan: [
    { name: "route", label: "Route", kind: "text", placeholder: "route" },
    { name: "entry_window", label: "Entry window", kind: "text", placeholder: "entry window" },
    { name: "required_tool", label: "Required tool", kind: "text", placeholder: "required tool" },
    { name: "extraction", label: "Extraction", kind: "text", placeholder: "extraction" },
  ],
  endorse_plan: [{ name: "plan_id", label: "Plan", kind: "text", placeholder: "plan id" }],
  challenge_plan: [
    { name: "plan_id", label: "Plan", kind: "text", placeholder: "plan id" },
    {
      name: "reason",
      label: "Reason",
      kind: "select",
      options: ["route_conflict", "timing_conflict", "tool_conflict", "extraction_conflict"],
    },
  ],
  commit_move: [
    { name: "selected_plan_id", label: "Selected plan", kind: "text", placeholder: "plan id" },
    { name: "contribute_required_resource", label: "Contribute required resource", kind: "checkbox" },
  ],
  acknowledge_result: [],
};

function ActionOfferRow({ offer, fixture, live, canSubmit }: { offer: ActionOffer; fixture: HeistFixture; live?: LiveSessionView | null; canSubmit: boolean }) {
  const [notice, setNotice] = useState<string | null>(null);
  const fields = actionFields[offer.actionType];
  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!canSubmit || !live) {
      setNotice("Not submitted: the fixture has no live participant transport.");
      return;
    }
    const form = new FormData(event.currentTarget);
    const payload: Record<string, string | boolean> = {};
    for (const field of fields) {
      const value = form.get(field.name);
      payload[field.name] = field.kind === "checkbox" ? value === "on" : String(value ?? "");
    }
    setNotice("Submitting through the attached Session…");
    void live.submitAction({ offerId: offer.id, schemaDigest: offer.schemaDigest, actionType: offer.actionType, payload, basedOnRoomSeq: fixture.roomSequence }).then(
      () => setNotice("Action submitted; awaiting the server outcome."),
      () => setNotice("Action was not submitted: the live Session is no longer attached."),
    );
  };

  return (
    <div className="offer-row">
      <div>
        <div className="offer-title-line">
          <h3>{offer.label}</h3>
          <code>{offer.actionType}</code>
        </div>
        <p>
          {offer.eligibility} · {offer.schemaDigest}
        </p>
        <form className="action-form" data-action-type={offer.actionType} data-offer-id={offer.id} onSubmit={handleSubmit}>
          <fieldset>
            <legend>Draft payload</legend>
            {fields.length === 0 ? <span className="form-empty">No payload fields; this offer is an explicit acknowledgement.</span> : null}
            {fields.map((field) => (
              <label className="action-field" htmlFor={`${offer.id}-${field.name}`} key={field.name}>
                <span>{field.label}</span>
                {field.kind === "select" ? (
                  <select defaultValue="" id={`${offer.id}-${field.name}`} name={field.name}>
                    <option disabled value="">
                      Select…
                    </option>
                    {field.options?.map((option) => (
                      <option key={option} value={option}>
                        {option}
                      </option>
                    ))}
                  </select>
                ) : field.kind === "checkbox" ? (
                  <input id={`${offer.id}-${field.name}`} name={field.name} type="checkbox" />
                ) : (
                  <input id={`${offer.id}-${field.name}`} name={field.name} placeholder={field.placeholder} type="text" />
                )}
              </label>
            ))}
            <div className="action-form-footer">
              <span>
                based_on_room_seq <code>{fixture.roomSequence}</code> · Action ID generated only by a live client
              </span>
              <button
                disabled={!canSubmit}
                aria-disabled={!canSubmit}
                title="Unavailable until an authorized live Session is attached"
                type="submit"
              >
                {canSubmit ? "Submit action" : "Submit unavailable"}
              </button>
            </div>
            {notice ? <output className="action-form-notice" role="status">{notice}</output> : null}
          </fieldset>
        </form>
      </div>
    </div>
  );
}

function DiagnosticRow({ label, value, detail }: { label: string; value: ReactNode; detail?: ReactNode }) {
  return (
    <div className="diagnostic-row">
      <span>{label}</span>
      <strong>{value}</strong>
      {detail ? <small>{detail}</small> : null}
    </div>
  );
}

export function OperatorDiagnostics({ diagnostics }: { diagnostics: OperatorDiagnostics }) {
  return (
    <Section
      eyebrow="Operator membership"
      title="Bounded diagnostics"
      description="Redacted operational facts only. This view has no raw Core/Activity state, private clues, offers, commitments, credentials, or model trace."
    >
      <div className="operator-banner">
        <div>
          <strong>Diagnostics are read-only</strong>
          <span>Membership, delivery, runner, timer, activation, and integrity metadata are bounded for this fixture.</span>
        </div>
        <StatusPill tone="good">{diagnostics.integrity.state}</StatusPill>
      </div>
      <div className="diagnostic-grid">
        <DiagnosticCard title="Membership" privacy="Redacted membership facts">
          <DiagnosticRow label="Access" value={diagnostics.membership.access} />
          <DiagnosticRow label="Standing" value={diagnostics.membership.standing} />
          <DiagnosticRow label="Member ref" value={diagnostics.membership.memberRef} />
          <DiagnosticRow label="Role" value={diagnostics.membership.role} />
        </DiagnosticCard>
        <DiagnosticCard title="Session & frame" privacy="Cursor metadata only">
          <DiagnosticRow label="Session" value={diagnostics.session.status} />
          <DiagnosticRow label="Session ref" value={diagnostics.session.sessionRef} />
          <DiagnosticRow label="Cursor" value={diagnostics.session.cursor} />
          <DiagnosticRow label="Frame delivery" value={diagnostics.frame.delivery} />
          <DiagnosticRow label="Head / cursor" value={`${diagnostics.frame.headSequence} / ${diagnostics.frame.cursorSequence}`} detail={diagnostics.frame.retainedRange} />
        </DiagnosticCard>
        <DiagnosticCard title="Runner & Activation" privacy="No invocation payloads">
          <DiagnosticRow label="Availability" value={diagnostics.runner.availability} />
          <DiagnosticRow label="Runner ref" value={diagnostics.runner.runnerRef} />
          <DiagnosticRow label="Activation" value={diagnostics.runner.activation} />
          <DiagnosticRow label="Invocation" value={diagnostics.runner.invocation} />
        </DiagnosticCard>
        <DiagnosticCard title="Timer" privacy="Schedule metadata only">
          <DiagnosticRow label="Phase deadline" value={diagnostics.timer.phaseDeadline} />
          <DiagnosticRow label="Generation" value={diagnostics.timer.timerGeneration} />
          <DiagnosticRow label="Next transition" value={diagnostics.timer.nextTransition} />
          <DiagnosticRow label="Heartbeat" value={diagnostics.session.lastHeartbeat} />
        </DiagnosticCard>
        <DiagnosticCard title="Room integrity" privacy="Redacted hash metadata">
          <DiagnosticRow label="State" value={diagnostics.integrity.state} />
          <DiagnosticRow label="Generation" value={diagnostics.integrity.generation} />
          <DiagnosticRow label="Complete head" value={diagnostics.integrity.completeHead} />
          <DiagnosticRow label="Core hash" value={diagnostics.integrity.coreHash} />
          <DiagnosticRow label="Activity hash" value={diagnostics.integrity.activityHash} />
          <DiagnosticRow label="Aggregate hash" value={diagnostics.integrity.aggregateHash} />
          <DiagnosticRow label="Safe reason" value={diagnostics.integrity.safeReason} />
        </DiagnosticCard>
      </div>
    </Section>
  );
}

function DiagnosticCard({ title, privacy, children }: { title: string; privacy: string; children: ReactNode }) {
  return (
    <article className="diagnostic-card">
      <div className="diagnostic-card-heading">
        <h3>{title}</h3>
        <span>{privacy}</span>
      </div>
      <div className="diagnostic-rows">{children}</div>
    </article>
  );
}

export function ReplayPanel({ fixture }: { fixture: HeistFixture }) {
  const maxSequence = fixture.replay.checkpoints.at(-1)?.sequence ?? fixture.roomSequence;
  const [selectedSequence, setSelectedSequence] = useState(Math.min(fixture.roomSequence, maxSequence));
  const selected = fixture.replay.checkpoints.find((checkpoint) => checkpoint.sequence === selectedSequence) ?? fixture.replay.checkpoints.at(-1);
  if (fixture.runtime.roomHealth === "Quarantined") {
    return <QuarantinedSurface surface="Historical Replay" />;
  }
  return (
    <>
      <Section
        eyebrow="Historical view"
        title="Replay · read-only"
        description="Replay uses the authorized public projection in this reference. It never enables mutation or bypasses visibility."
      >
        <div className="replay-control">
          <label htmlFor="replay-sequence">Public sequence</label>
          <input
            aria-describedby="replay-selection"
            id="replay-sequence"
            max={maxSequence}
            min="0"
            onChange={(event) => setSelectedSequence(Number(event.target.value))}
            type="range"
            value={selectedSequence}
          />
          <span id="replay-selection">Sequence {selected?.sequence ?? selectedSequence} · {selected?.phase ?? "Unavailable"} · {fixture.replay.availableThrough}</span>
        </div>
        <div className="replay-status-grid">
          <StatusPill tone="good">{fixture.replay.verification}</StatusPill>
          <span>{fixture.replay.currentView}</span>
          <StatusPill tone={fixture.replay.presentAuthorization === "Authorized" ? "good" : "attention"}>Present authorization · {fixture.replay.presentAuthorization}</StatusPill>
          <StatusPill tone={fixture.replay.historicalAuthorization === "Public projection authorized" ? "good" : "attention"}>Historical authorization · {fixture.replay.historicalAuthorization}</StatusPill>
          <span>Mutation controls unavailable</span>
        </div>
        <div className="replay-checkpoint" aria-live="polite">
          <span className="eyebrow">Authorized historical projection</span>
          <strong>{selected?.summary ?? "No verified historical projection is available."}</strong>
        </div>
        <p className="replay-digest">
          Canonical story replay · {fixture.parity.replayTransitionCount} transitions · {fixture.parity.transcriptDigest}
        </p>
        <div className="replay-hashes" aria-label="Replay Room Head hashes">
          <ReplayHash label="Core" value={fixture.replay.hashes.coreStateHash} />
          <ReplayHash label="Activity" value={fixture.replay.hashes.activityStateHash} />
          <ReplayHash label="Aggregate" value={fixture.replay.hashes.aggregateStateHash} />
          <ReplayHash label="Transition" value={fixture.replay.hashes.lineageHash} />
        </div>
      </Section>
      <FinalRevealGate fixture={fixture} />
    </>
  );
}

function ReplayHash({ label, value }: { label: string; value: string }) {
  return <div className="replay-hash"><span>{label} hash</span><code data-replay-hash={label.toLowerCase()}>{value}</code></div>;
}

export function FinalRevealGate({ fixture }: { fixture: HeistFixture }) {
  const available = fixture.phase === "Complete" && fixture.finalRevealAuthorized && fixture.runtime.roomHealth === "Healthy";
  return (
    <section className={`panel reveal-gate ${available ? "reveal-available" : "reveal-locked"}`} aria-labelledby="final-reveal-title">
      <div>
        <p className="eyebrow">Separate authorization boundary</p>
        <h2 id="final-reveal-title">Final reveal</h2>
        <p>
          {available
            ? "Available to an authorized current membership after Complete. This fixture still keeps private payloads out of the reference console."
            : "Locked until the Activity Phase is Complete and current final-reveal authorization is present."}
        </p>
      </div>
      <StatusPill tone={available ? "good" : "attention"}>{available ? "Available" : "Locked"}</StatusPill>
    </section>
  );
}

export function QuarantinedSurface({ surface }: { surface: string }) {
  return (
    <section className="panel quarantined-surface" role="alert" aria-label={`${surface} unavailable while quarantined`}>
      <p className="eyebrow">Room integrity boundary</p>
      <h2>{surface} unavailable while Quarantined</h2>
      <p>Normal Projection, Catch-up, Action Offers, and claimed-current Replay are withheld. Only bounded operator diagnostics may remain visible.</p>
      <StatusPill tone="danger">Quarantined · fail closed</StatusPill>
    </section>
  );
}

export function UnavailableSurface({ surface, recovery }: { surface: string; recovery: "Loading" | "CatchingUp" }) {
  const label = recovery === "Loading" ? "Loading" : "Catching up";
  return (
    <section className="panel quarantined-surface" role="status" aria-label={surface + " unavailable while " + label}>
      <p className="eyebrow">Projection availability boundary</p>
      <h2>{surface} unavailable while {label}</h2>
      <p>No current authorized bytes are rendered until the Session installs a Projection Reset or completes retained catch-up.</p>
      <StatusPill tone="attention">{label} · fail closed</StatusPill>
    </section>
  );
}
