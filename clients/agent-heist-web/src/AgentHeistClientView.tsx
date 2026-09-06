import { useState, type FormEvent } from "react";

import type { ActivityClientAction } from "@worldstream/client";

import type {
  AgentHeistActionOffer,
  AgentHeistActionType,
  AgentHeistLiveState,
  AgentHeistReadyState,
} from "./liveAdapter";
import {
  AgentHeistWorkspace,
  PanelHeading,
  type AgentHeistWorkspaceMetric,
} from "./presentation";

export type AgentHeistClientConnection = "connecting" | "live" | "disconnected" | "setup_required";
export type AgentHeistAgentAssist = "checking" | "available" | "unsupported" | "unavailable";

export function AgentHeistClientView({
  state,
  connection,
  message,
  actionsEnabled,
  agentAssist = "checking",
  onAct,
  onReconnect,
}: {
  readonly state: AgentHeistLiveState;
  readonly connection: AgentHeistClientConnection;
  readonly message?: string | null;
  readonly actionsEnabled?: boolean;
  readonly agentAssist?: AgentHeistAgentAssist;
  readonly onAct: (action: ActivityClientAction) => Promise<void>;
  readonly onReconnect?: () => Promise<void>;
}) {
  if (state.kind === "awaiting") {
    return <BoundarySurface title="Waiting for authorized Projection" detail={message ?? "The Activity Client will remain empty until the retained participant session installs a Projection Reset."} />;
  }
  if (state.kind === "incompatible") {
    return <BoundarySurface title="Client incompatible" detail={state.reason} />;
  }
  return <ReadyParticipant
    state={state}
    connection={connection}
    message={message}
    actionsEnabled={actionsEnabled ?? connection === "live"}
    agentAssist={agentAssist}
    onAct={onAct}
    onReconnect={onReconnect}
  />;
}

function ReadyParticipant({
  state,
  connection,
  message,
  actionsEnabled,
  agentAssist,
  onAct,
  onReconnect,
}: {
  readonly state: AgentHeistReadyState;
  readonly connection: AgentHeistClientConnection;
  readonly message?: string | null;
  readonly actionsEnabled: boolean;
  readonly agentAssist: AgentHeistAgentAssist;
  readonly onAct: (action: ActivityClientAction) => Promise<void>;
  readonly onReconnect?: () => Promise<void>;
}) {
  const metrics: AgentHeistWorkspaceMetric[] = [
    { label: "Participant session", value: connectionLabel(connection), tone: connection === "live" ? "green" : "amber" },
    { label: "Activity Phase", value: capitalize(state.projection.phase), tone: "amber" },
    { label: "Room sequence", value: String(state.roomSequence).padStart(4, "0"), tone: "blue" },
    { label: "Pack revision", value: shortIdentity(state.pack.digest), tone: "violet" },
    { label: "Current Action Offers", value: String(state.offers.length).padStart(2, "0"), tone: "green" },
  ];
  return (
    <main className="heist-live-shell">
      <header className="live-client-header">
        <div><span className="eyebrow">WorldStream Activity Client</span><h1>Agent Heist</h1></div>
        <div className="live-client-badges">
          <span className={`live-mode live-mode-${connection}`}><i /> Authorized {state.authorization.accessMode} surface</span>
          <span className={`agent-assist agent-assist-${agentAssist}`}>{agentAssistLabel(agentAssist)}</span>
        </div>
      </header>
      <div className="live-client-notice" role="status">
        <strong>{authorizationLabel(state)}</strong>
        <span>{message ?? (connection === "live" ? "This surface contains only the Projection authorized for this Membership." : "Reconnect before acting; installed information is visibly stale.")}</span>
      </div>
      <AgentHeistWorkspace
        metrics={metrics}
        left={<MembershipPanel state={state} connection={connection} onReconnect={onReconnect} />}
        eyebrow="Authorized shared reality"
        heading="Crew operation board"
        status={<span className="play-state"><i className={connection === "live" ? "is-running" : ""} />{connectionLabel(connection)}</span>}
        center={<PublicBoard state={state} />}
        footer={<>
          <span>Sequence {state.roomSequence}</span>
          <span>Frame {state.frameHead}</span>
          <span>Role {state.authorization.role === null ? "None" : capitalize(state.authorization.role)}</span>
          <span>Projection only</span>
        </>}
        right={state.authorization.accessMode === "participant"
          ? <PrivateParticipantPanel state={state} actionsEnabled={actionsEnabled} onAct={onAct} />
          : <SpectatorPanel />}
      />
    </main>
  );
}

function MembershipPanel({
  state,
  connection,
  onReconnect,
}: {
  readonly state: AgentHeistReadyState;
  readonly connection: AgentHeistClientConnection;
  readonly onReconnect?: () => Promise<void>;
}) {
  return <>
    <PanelHeading number="01" title="Your Membership" />
    <div className="authorized-membership">
      <span>Access Mode</span><strong>{capitalize(state.authorization.accessMode)}</strong>
      <span>Role</span><strong>{state.authorization.role === null ? "Not assigned" : capitalize(state.authorization.role)}</strong>
      <span>Standing</span><strong>Enabled</strong>
    </div>
    <PanelHeading number="02" title="Crew presence" />
    <ul className="crew-presence">
      {state.projection.seats.map((seat) => (
        <li key={seat.role}><i className={seat.present ? "is-present" : ""} /><span>{capitalize(seat.role)}</span><strong>{seat.present ? "Present" : "Awaiting"}</strong></li>
      ))}
    </ul>
    <div className="control-note">
      <span>Authority boundary</span>
      <p>This live client cannot switch to another participant, spectator, or operator view.</p>
    </div>
    {connection === "disconnected" && onReconnect !== undefined
      ? <button className="reconnect-button" type="button" onClick={() => void onReconnect()}>Reconnect</button>
      : null}
  </>;
}

function PublicBoard({ state }: { readonly state: AgentHeistReadyState }) {
  const result = state.projection.outcome;
  return <div className="live-board">
    <section className="phase-window">
      <div><span className="live-panel-label">Current phase</span><strong>{capitalize(state.projection.phase)}</strong></div>
      <div><span className="live-panel-label">Deadline</span><strong>{deadlineLabel(state.projection.phaseDeadline)}</strong></div>
    </section>
    <div className="live-board-grid">
      <section><span className="live-panel-label">Published claims</span><h3>Clue board</h3>
        {state.projection.publicClaims.length === 0 ? <p className="empty-copy">No clue has been published.</p> : (
          <ul className="live-data-list">{state.projection.publicClaims.map((clue) => <li key={clue.clueId}><span>{humanize(clue.clueId)}</span><strong>{humanize(clue.claimCode)}</strong></li>)}</ul>
        )}
      </section>
      <section><span className="live-panel-label">Commitment window</span><h3>Sealed choices</h3>
        <div className="commitment-meter"><strong>{state.projection.commitmentCount}</strong><span>of 3 commitments recorded</span></div>
        <p>Individual choices remain withheld from this board.</p>
      </section>
    </div>
    <section className="plan-board"><span className="live-panel-label">Negotiation</span><h3>Candidate plans</h3>
      {state.projection.plans.length === 0 ? <p className="empty-copy">No plan has been proposed.</p> : (
        <div className="plan-grid">{state.projection.plans.map((plan) => <article key={plan.planId}>
          <div><span>{plan.proposerRole}</span><code>{shortIdentity(plan.planId)}</code></div>
          <h4>{humanize(plan.route)} · {humanize(plan.entryWindow)}</h4>
          <p>{humanize(plan.requiredTool)} → {humanize(plan.extraction)}</p>
          <footer><span>{plan.endorsements} endorsements</span><span>{plan.challenges} challenges</span></footer>
        </article>)}</div>
      )}
    </section>
    {result === null ? null : <section className="outcome-card"><span className="live-panel-label">Outcome</span><h3>{humanize(result.outcome)}</h3><p>{result.reason}</p><strong>Score {result.score}/5</strong></section>}
  </div>;
}

function PrivateParticipantPanel({
  state,
  actionsEnabled,
  onAct,
}: {
  readonly state: AgentHeistReadyState;
  readonly actionsEnabled: boolean;
  readonly onAct: (action: ActivityClientAction) => Promise<void>;
}) {
  return <>
    <PanelHeading number="03" title="Private knowledge" />
    {state.projection.privateClues.length === 0 ? <p className="private-empty">No private clue has been inspected.</p> : (
      <div className="private-clue-list">{state.projection.privateClues.map((clue) => <article key={clue.clueId}>
        <span>Authorized private clue</span><strong>{humanize(clue.clueId)}</strong><code>{clue.claimCode}</code>
      </article>)}</div>
    )}
    {state.projection.ownCommitment === null ? null : <div className="own-commitment">
      <span>Your sealed commitment</span>
      <strong>{humanize(state.projection.ownCommitment.selectedPlanId)}</strong>
      <small>{state.projection.ownCommitment.contributeRequiredResource ? "Required resource committed" : "No resource committed"}</small>
    </div>}
    <PanelHeading number="04" title="Incoming exchanges" />
    {state.projection.addressedOffers.length === 0 ? <p className="private-empty">No exchange is addressed to this role.</p> : (
      <div className="incoming-exchanges">{state.projection.addressedOffers.map((offer) => <article key={offer.offerId}>
        <span>{capitalize(offer.senderRole)} offers {humanize(offer.offeredClueId)}</span>
        <strong>For {humanize(offer.considerationKind)}: {humanize(offer.considerationId)}</strong>
        <small>{humanize(offer.status)}</small>
      </article>)}</div>
    )}
    <PanelHeading number="05" title="Current Actions" />
    {state.offers.length === 0 ? <p className="private-empty">No Action is offered at this synchronized Head.</p> : (
      <div className="live-offer-list">{state.offers.map((offer) => <ActionOfferForm
        key={offer.offerId}
        offer={offer}
        roomSequence={state.roomSequence}
        enabled={actionsEnabled}
        onAct={onAct}
      />)}</div>
    )}
  </>;
}

function SpectatorPanel() {
  return <>
    <PanelHeading number="03" title="Public spectator view" />
    <div className="control-note">
      <span>Read-only authorization</span>
      <p>This surface contains only the public Projection authorized for the spectator Membership. Private clues, commitments, addressed offers, and Actions are unavailable.</p>
    </div>
  </>;
}

interface ActionField {
  readonly name: string;
  readonly label: string;
  readonly kind: "checkbox" | "select" | "text";
  readonly options?: readonly string[];
}

const actionFields: Record<AgentHeistActionType, readonly ActionField[]> = {
  inspect_clue: [{ name: "clue_id", label: "Clue ID", kind: "text" }],
  publish_clue: [{ name: "clue_id", label: "Clue ID", kind: "text" }, { name: "claim_code", label: "Claim code", kind: "text" }],
  offer_exchange: [
    { name: "recipient_role", label: "Recipient", kind: "select", options: ["navigator", "insider", "broker"] },
    { name: "offered_clue_id", label: "Offered clue", kind: "text" },
    { name: "consideration_kind", label: "Consideration", kind: "select", options: ["clue_disclosure", "plan_endorsement"] },
    { name: "consideration_id", label: "Clue or plan ID", kind: "text" },
  ],
  accept_exchange: [{ name: "offer_id", label: "Offer ID", kind: "text" }],
  propose_plan: [
    { name: "route", label: "Route", kind: "text" },
    { name: "entry_window", label: "Entry window", kind: "text" },
    { name: "required_tool", label: "Required tool", kind: "text" },
    { name: "extraction", label: "Extraction", kind: "text" },
  ],
  endorse_plan: [{ name: "plan_id", label: "Plan ID", kind: "text" }],
  challenge_plan: [
    { name: "plan_id", label: "Plan ID", kind: "text" },
    { name: "reason", label: "Reason", kind: "select", options: ["route_conflict", "timing_conflict", "tool_conflict", "extraction_conflict"] },
  ],
  commit_move: [{ name: "selected_plan_id", label: "Plan ID", kind: "text" }, { name: "contribute_required_resource", label: "Contribute resource", kind: "checkbox" }],
  acknowledge_result: [],
};

function ActionOfferForm({
  offer,
  roomSequence,
  enabled,
  onAct,
}: {
  readonly offer: AgentHeistActionOffer;
  readonly roomSequence: number;
  readonly enabled: boolean;
  readonly onAct: (action: ActivityClientAction) => Promise<void>;
}) {
  const [notice, setNotice] = useState<string | null>(null);
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!enabled) return;
    const values = new FormData(event.currentTarget);
    const payload: Record<string, unknown> = {};
    for (const field of actionFields[offer.actionType]) {
      const value = values.get(field.name);
      payload[field.name] = field.kind === "checkbox" ? value === "on" : String(value ?? "");
    }
    if (offer.actionType === "offer_exchange") {
      const kind = String(payload.consideration_kind ?? "");
      const id = String(payload.consideration_id ?? "");
      delete payload.consideration_kind;
      delete payload.consideration_id;
      payload.consideration = kind === "clue_disclosure" ? { kind, clue_id: id } : { kind, plan_id: id };
    }
    setNotice("Submitting through the retained participant session…");
    void onAct({
      actionId: nextUlid(),
      basedOnRoomSeq: roomSequence,
      offerId: offer.offerId,
      schemaDigest: offer.schemaDigest,
      actionType: offer.actionType,
      payload,
    }).then(
      () => setNotice("Submitted. Waiting for committed Room output."),
      () => setNotice("The Action was rejected or could not be submitted."),
    );
  };
  return <form className="live-action-form" onSubmit={submit}>
    <div><strong>{humanize(offer.actionType)}</strong><code>{offer.actionType}</code></div>
    <small>{offer.eligibility}</small>
    {actionFields[offer.actionType].map((field) => <label key={field.name}><span>{field.label}</span>{field.kind === "select" ? (
      <select name={field.name} defaultValue="" required><option disabled value="">Select…</option>{field.options?.map((option) => <option key={option} value={option}>{humanize(option)}</option>)}</select>
    ) : <input name={field.name} type={field.kind === "checkbox" ? "checkbox" : "text"} required={field.kind !== "checkbox"} />}</label>)}
    <p>based_on_room_seq <code>{roomSequence}</code></p>
    <button disabled={!enabled} type="submit">{enabled ? "Submit action" : "Reconnect before acting"}</button>
    {notice === null ? null : <output role="status">{notice}</output>}
  </form>;
}

function BoundarySurface({ title, detail }: { readonly title: string; readonly detail: string }) {
  return <main className="heist-boundary-shell"><span className="eyebrow">Agent Heist Activity Client</span><h1>{title}</h1><p>{detail}</p></main>;
}

function connectionLabel(connection: AgentHeistClientConnection): string {
  if (connection === "live") return "Live";
  if (connection === "connecting") return "Connecting";
  if (connection === "setup_required") return "Setup required";
  return "Disconnected";
}

function agentAssistLabel(status: AgentHeistAgentAssist): string {
  if (status === "available") return "Agent tools ready";
  if (status === "unsupported") return "Human controls";
  if (status === "unavailable") return "Agent tools unavailable";
  return "Checking agent tools";
}

function deadlineLabel(value: string | null): string {
  if (value === null) return "No active deadline";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Authoritative deadline set";
  return date.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    timeZoneName: "short",
  });
}

function capitalize(value: string): string { return value.charAt(0).toUpperCase() + value.slice(1); }
function authorizationLabel(state: AgentHeistReadyState): string {
  return state.authorization.role === null
    ? "Spectator view"
    : `${capitalize(state.authorization.role)} participant`;
}
function humanize(value: string): string { return value.replaceAll("_", " "); }
function shortIdentity(value: string): string { return value.length <= 20 ? value : `${value.slice(0, 11)}…${value.slice(-6)}`; }

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
