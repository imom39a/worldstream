import { useEffect, useRef, useState, type FormEvent } from "react";

import type { ActivityClientAction } from "@worldstream/client";
import { HeistArtwork } from "./HeistArtwork";
import { PhaseCountdown } from "./PhaseCountdown";
import { AGENT_HEIST_REVISION_0_5, type AgentHeistActionOffer, type AgentHeistActionType, type AgentHeistLiveState, type AgentHeistReadyState } from "./liveAdapter";

export type AgentHeistClientConnection = "connecting" | "live" | "disconnected" | "setup_required";
export type AgentHeistAgentAssist = "checking" | "available" | "unsupported" | "unavailable";

/** Exact 0.5 public role/object semantics, reviewed with the Pack source.
 * It contains object labels only, never a fixture answer or dynamic legality. */
const HEIST_0_5_DOSSIERS: Readonly<Record<string, Readonly<Record<string, readonly string[]>>>> = {
  [AGENT_HEIST_REVISION_0_5]: { navigator: ["route"], insider: ["entry_window"], broker: ["required_tool", "extraction"] },
};

export function AgentHeistClientView({ state, connection, message, actionsEnabled, agentAssist = "checking", onAct, onReconnect }: {
  readonly state: AgentHeistLiveState; readonly connection: AgentHeistClientConnection; readonly message?: string | null;
  readonly actionsEnabled?: boolean; readonly agentAssist?: AgentHeistAgentAssist;
  readonly onAct: (action: ActivityClientAction) => Promise<void>; readonly onReconnect?: () => Promise<void>;
}) {
  if (state.kind === "awaiting") return <MissionConnection connection={connection} message={message} onReconnect={onReconnect} />;
  if (state.kind === "incompatible") return <Boundary title="Mission display unavailable" detail={state.reason} />;
  return <MissionFocus state={state} connection={connection} message={message} agentAssist={agentAssist}
    actionsEnabled={actionsEnabled ?? connection === "live"} onAct={onAct} onReconnect={onReconnect} />;
}

function MissionConnection({ connection, message, onReconnect }: {
  readonly connection: AgentHeistClientConnection; readonly message?: string | null;
  readonly onReconnect?: () => Promise<void>;
}) {
  const pending = useRef(false);
  const [retrying, setRetrying] = useState(false);
  const [retryFailed, setRetryFailed] = useState(false);
  const retry = async () => {
    if (pending.current || onReconnect === undefined) return;
    pending.current = true;
    setRetrying(true);
    setRetryFailed(false);
    try { await onReconnect(); } catch { setRetryFailed(true); }
    finally { pending.current = false; setRetrying(false); }
  };
  const requiresEntry = connection === "setup_required";
  return <main className="heist-boundary-shell" aria-busy={retrying || connection === "connecting"}>
    <span className="eyebrow">Agent Heist Activity Client</span>
    <h1>{requiresEntry ? "Re-enter your mission" : "Waiting for the mission"}</h1>
    <p role="status">{retrying ? "Reconnecting to this mission…" : message ?? "Your mission will appear when this connection is ready."}</p>
    {requiresEntry
      ? <p>Return to My games and choose Return to game. This restores access to the same Room; it does not restart the game.</p>
      : <p>Reconnect does not resend a move. Your mission will appear only after the Room confirms this connection.</p>}
    {connection === "disconnected" && onReconnect !== undefined
      ? <button type="button" disabled={retrying} onClick={() => void retry()}>Reconnect</button> : null}
    {retryFailed ? <p role="alert">The connection is still unavailable. Try again, or return to My games to re-enter.</p> : null}
  </main>;
}

function MissionFocus({ state, connection, message, actionsEnabled, agentAssist, onAct, onReconnect }: {
  readonly state: AgentHeistReadyState; readonly connection: AgentHeistClientConnection; readonly message?: string | null;
  readonly actionsEnabled: boolean; readonly agentAssist: AgentHeistAgentAssist; readonly onAct: (action: ActivityClientAction) => Promise<void>;
  readonly onReconnect?: () => Promise<void>;
}) {
  const [requested, setRequested] = useState<AgentHeistActionType | null>(null);
  const active = state.offers.find((offer) => offer.actionType === requested) ?? preferredOffer(state);
  const stale = requested !== null && !state.offers.some((offer) => offer.actionType === requested);
  const copy = phaseCopy(state, active?.actionType ?? null);
  return <main className="heist-live-shell mission-focus-shell"
    data-room-sequence={state.roomSequence}
    data-phase={state.projection.phase}
    data-phase-generation={state.projection.phaseGeneration}
    data-phase-deadline={state.projection.phaseDeadline ?? undefined}>
    <a className="heist-skip-link" href="#mission-action">Skip to current mission action</a>
    <header className="mission-focus-header"><div><span className="eyebrow">Cooperative planning operation</span><h1>Agent <em>Heist</em></h1></div>
      <div className="mission-hud"><span className={`connection-chip connection-${connection}`}>{connectionLabel(connection)}</span><span className="mission-clock"><small>Phase time</small><PhaseCountdown key={`${state.projection.phaseGeneration}:${state.projection.phaseDeadline}:${connection}`} deadline={state.projection.phaseDeadline} connected={connection === "live"} /></span></div>
    </header>
    <div className="mission-connection" role="status"><strong>{connection === "live" ? "Mission link live" : "Mission link needs attention"}</strong><span>{message ?? connectionMessage(state, connection)}</span>
      {connection === "disconnected" && onReconnect !== undefined ? <button type="button" onClick={() => void onReconnect()}>Reconnect</button> : null}</div>
    <section className="mission-focus-layout" aria-labelledby="mission-title"><section className="mission-main">
      <div className="mission-hero"><span className="mission-scene-art" aria-hidden="true">✦</span><div><span>Current phase · {humanize(state.projection.phase)}</span><h2 id="mission-title">{copy.title}</h2><p>{copy.detail}</p></div></div>
      <ol className="mission-progress" aria-label="Mission progress">{(["briefing", "negotiation", "commitment", "result"] as const).map((phase, index) => <li key={phase} className={phaseState(state.projection.phase, phase)} aria-current={state.projection.phase === phase ? "step" : undefined}><span>{String(index + 1).padStart(2, "0")}</span>{humanize(phase)}</li>)}</ol>
      {state.projection.outcome === null ? null : <OutcomeDebrief state={state} />}
      <section className="mission-action" id="mission-action" aria-live="polite"><div className="mission-section-heading"><span>Your move</span><h3>{active === undefined ? "Wait for the crew" : actionTitle(active.actionType)}</h3></div>
        {state.authorization.accessMode === "participant" && state.projection.ownCommitment !== null ? <p className="own-commitment" role="status">Your choice is sealed.</p> : null}
        {stale ? <p className="action-stale">That move is no longer offered at the current mission moment. Your unsubmitted draft was not sent.</p> : null}
        {active === undefined ? <NoAction phase={state.projection.phase} /> : <MissionAction key={`${state.pack.digest}:${state.authorization.role}:${state.projection.phase}:${active.actionType}`} offer={active} state={state} enabled={actionsEnabled} onAct={onAct} />}
      </section>
      {state.authorization.accessMode === "participant" && state.offers.length > 1 ? <nav className="mission-moves" aria-label="Other currently offered moves"><span>Other moves</span>{state.offers.map((offer) => <button key={offer.offerId} type="button" aria-pressed={offer.actionType === active?.actionType} onClick={() => setRequested(offer.actionType)}>{actionTitle(offer.actionType)}</button>)}</nav> : null}
    </section><aside className="mission-rail"><RoleCard state={state} /><PrivateIntel state={state} />
      <details className="mission-board"><summary>Open crew board <span>{state.projection.plans.length} plans · {state.projection.publicClaims.length} shared clues</span></summary><CrewBoard state={state} /></details>
      <details className="mission-help"><summary>How this operation works</summary><Rules /></details>
      <details className="mission-help technical-details"><summary>Connection and session</summary><p>This client only acts through your current authorized Membership. It cannot switch roles or reveal another player's intel.</p><p>Agent assistance: {agentAssistLabel(agentAssist)}.</p></details>
    </aside></section>
  </main>;
}

function RoleCard({ state }: { readonly state: AgentHeistReadyState }) {
  const role = state.authorization.role;
  if (role === null) return <section className="role-card spectator-card"><span>Public spectator view</span><h3>Follow the crew</h3><p>You can see only the shared mission board. Private intel and actions are unavailable.</p></section>;
  return <section className={`role-card role-${role}`}><span>Your role</span><h3>{humanize(role)}</h3><p>{role === "navigator" ? "You hold the route intel." : role === "insider" ? "You hold the entry-time intel." : "You hold the equipment and extraction intel."}</p><ul>{state.projection.seats.map((seat) => <li key={seat.role}><i className={seat.present ? "present" : ""} />{humanize(seat.role)} <small>{seat.present ? "present" : "away"}</small></li>)}</ul></section>;
}

function PrivateIntel({ state }: { readonly state: AgentHeistReadyState }) {
  if (state.authorization.accessMode !== "participant") return null;
  const targets = dossiersFor(state).filter((id) => !state.projection.privateClues.some((clue) => clue.clueId === id));
  const inspect = state.offers.some((offer) => offer.actionType === "inspect_clue");
  return <details className="private-intel"><summary>Your private dossiers</summary><div className="private-intel-content">
    {targets.map((id) => <article className="dossier-card" key={id}><span className="dossier-glyph" aria-hidden="true">✦</span><div><strong>{clueLabel(id)} dossier</strong><span>Sealed — only you can open it</span><small>{inspect ? "Choose “Open a dossier” in your moves." : "Not available in this phase."}</small></div></article>)}
    {inspect && targets.length === 0 ? <p className="intel-note">There is no sealed dossier available to open right now.</p> : null}
    {state.projection.privateClues.length === 0 && targets.length === 0 && !inspect ? <p className="intel-note">No private dossier is currently open.</p> : null}
    {state.projection.privateClues.map((clue) => <article className="known-clue" key={clue.clueId}><span>{clueLabel(clue.clueId)}</span><strong>{humanize(clue.claimCode)}</strong><small>Only you can see this until you share it.</small></article>)}
  </div></details>;
}

function CrewBoard({ state }: { readonly state: AgentHeistReadyState }) {
  const p = state.projection;
  return <div className="crew-board-content"><section><span>Shared clues</span>{p.publicClaims.length === 0 ? <p>No clues are shared yet.</p> : <ul>{p.publicClaims.map((clue) => <li key={clue.clueId}><strong>{clueLabel(clue.clueId)}</strong>{humanize(clue.claimCode)}</li>)}</ul>}</section>
    <section><span>Candidate plans</span>{p.plans.length === 0 ? <p>No plan is on the board yet.</p> : <div className="crew-plan-list">{p.plans.map((plan, index) => <article key={plan.planId}><b>Plan {index + 1} · {humanize(plan.proposerRole)}</b><p>{humanize(plan.route)} · {humanize(plan.entryWindow)}<br />{humanize(plan.requiredTool)} → {humanize(plan.extraction)}</p><small>{plan.endorsements} backed · {plan.challenges} flagged</small></article>)}</div>}</section>
    <section className="crew-commitments"><span>Commitments</span><strong>{p.commitmentCount} of {p.seats.length}</strong><p>Choices stay private until the result.</p></section>
  </div>;
}

function OutcomeDebrief({ state }: { readonly state: AgentHeistReadyState }) {
  const outcome = state.projection.outcome!;
  const checks = outcome.checks === null ? [] : [["Route", outcome.checks.route], ["Entry time", outcome.checks.entryWindow], ["Equipment", outcome.checks.requiredTool], ["Extraction", outcome.checks.extraction], ["Crew resource", outcome.checks.resourceContributed]] as const;
  return <section className="outcome-debrief"><span className="result-glyph" aria-hidden="true">✦</span><div><span>Mission debrief</span><h3>{humanize(outcome.outcome)} · {outcome.score}/5</h3><p>{outcome.reason === "no_strict_majority" ? "The crew did not settle on one plan with at least two sealed choices." : "The selected plan was scored from the authorized crew result."}</p>{Object.keys(outcome.voteCounts).length > 0 ? <p>Sealed support: {Object.values(outcome.voteCounts).join(", ")}.</p> : null}{outcome.missingRoles.length > 0 ? <p>Missing sealed choices: {outcome.missingRoles.map(humanize).join(", ")}.</p> : null}{checks.length > 0 ? <ul>{checks.map(([label, passed]) => <li key={label} className={passed ? "passed" : "missed"}>{passed ? "✓" : "×"} {label}</li>)}</ul> : null}</div></section>;
}

interface ActionFacts { readonly packDigest: string; readonly clues: readonly AgentHeistReadyState["projection"]["privateClues"][number][]; readonly plans: readonly AgentHeistReadyState["projection"]["plans"][number][]; readonly exchanges: readonly AgentHeistReadyState["projection"]["addressedOffers"][number][]; readonly dossiers: readonly string[]; readonly recipients: readonly AgentHeistReadyState["projection"]["seats"][number][]; readonly unavailable: string | null; }
function MissionAction({ offer, state, enabled, onAct }: { readonly offer: AgentHeistActionOffer; readonly state: AgentHeistReadyState; readonly enabled: boolean; readonly onAct: (action: ActivityClientAction) => Promise<void> }) {
  const [notice, setNotice] = useState<string | null>(null); const [pending, setPending] = useState(false); const [planReady, setPlanReady] = useState(offer.actionType !== "propose_plan"); const inFlight = useRef(false); const generation = useRef(0); const facts = actionFacts(offer.actionType, state);
  useEffect(() => { generation.current += 1; inFlight.current = false; setPending(false); setNotice(null); setPlanReady(offer.actionType !== "propose_plan"); }, [offer.offerId, offer.actionType]);
  const submit = (event: FormEvent<HTMLFormElement>) => { event.preventDefault(); if (!enabled || facts.unavailable !== null || inFlight.current) return; const payload = payloadFor(offer.actionType, new FormData(event.currentTarget), state); if (payload === null) { setNotice("That choice is no longer available on the current board."); return; } inFlight.current = true; setPending(true); setNotice("Sending your move to the mission…"); const submissionGeneration = generation.current; const action: ActivityClientAction = { actionId: nextUlid(), basedOnRoomSeq: state.roomSequence, offerId: offer.offerId, schemaDigest: offer.schemaDigest, actionType: offer.actionType, payload }; void Promise.resolve().then(() => onAct(action)).then(() => { if (generation.current === submissionGeneration) setNotice("Sent. Waiting for the Room to confirm the result."); }, () => { if (generation.current === submissionGeneration) setNotice("This move could not be submitted. Reconnect or wait for the next mission update."); }).finally(() => { if (generation.current === submissionGeneration) { inFlight.current = false; setPending(false); } }); };
  return <form className="mission-action-surface" onSubmit={submit}><p>{actionInstruction(offer.actionType)}</p><ActionChoices action={offer.actionType} facts={facts} onPlanReadyChange={setPlanReady} />{offer.actionType === "commit_move" ? <p className="seal-warning">Sealing is separate from backing a plan and cannot be revised under the current rules.</p> : null}{facts.unavailable === null ? null : <p className="action-stale">{facts.unavailable}</p>}<button disabled={!enabled || facts.unavailable !== null || pending || !planReady} type="submit">{!enabled ? "Reconnect before acting" : facts.unavailable !== null ? "No safe choice available" : pending ? "Sending move…" : !planReady ? "Finish all four parts" : actionButton(offer.actionType)}</button>{notice === null ? null : <output role="status">{notice}</output>}</form>;
}
function actionFacts(action: AgentHeistActionType, state: AgentHeistReadyState): ActionFacts { const clues = state.projection.privateClues, plans = state.projection.plans, exchanges = state.projection.addressedOffers, dossiers = dossiersFor(state).filter((id) => !clues.some((clue) => clue.clueId === id)), recipients = state.projection.seats.filter((seat) => seat.present && seat.role !== state.authorization.role); let unavailable: string | null = null; if (action === "inspect_clue" && dossiers.length === 0) unavailable = "There is no sealed dossier available to open right now."; if (action === "publish_clue" && clues.length === 0) unavailable = "Open a dossier before sharing its intel."; if (action === "offer_exchange" && (clues.length === 0 || recipients.length === 0)) unavailable = "A private clue and a teammate are needed to make this exchange."; if (action === "accept_exchange" && exchanges.length === 0) unavailable = "No exchange is addressed to you."; if (["endorse_plan", "challenge_plan", "commit_move"].includes(action) && plans.length === 0) unavailable = "No current crew plan is available to choose."; if (action === "challenge_plan" && challengeReasons({ packDigest: state.pack.digest, clues, plans, exchanges, dossiers, recipients, unavailable }).length === 0) unavailable = "Open the relevant dossier before flagging a conflict."; return { packDigest: state.pack.digest, clues, plans, exchanges, dossiers, recipients, unavailable }; }
function ActionChoices({ action, facts, onPlanReadyChange }: { readonly action: AgentHeistActionType; readonly facts: ActionFacts; readonly onPlanReadyChange: (ready: boolean) => void }) { if (action === "inspect_clue") return <Choice name="clue_id" label="Choose your sealed dossier" options={facts.dossiers.map((id) => [id, clueLabel(id)])} />; if (action === "publish_clue") return <Choice name="clue_id" label="Share a private clue" options={facts.clues.map((clue) => [clue.clueId, `${clueLabel(clue.clueId)} — ${humanize(clue.claimCode)}`])} />; if (action === "offer_exchange") return <ExchangeChoices facts={facts} />; if (action === "accept_exchange") return <Choice name="offer_id" label="Accept an incoming exchange" options={facts.exchanges.map((offer) => [offer.offerId, `${humanize(offer.senderRole)} offers ${clueLabel(offer.offeredClueId)}`])} />; if (action === "propose_plan") return <PlanBuilder onReadyChange={onPlanReadyChange} />; if (action === "endorse_plan") return <PlanChoice name="plan_id" label="Back a crew plan" plans={facts.plans} />; if (action === "challenge_plan") return <><PlanChoice name="plan_id" label="Flag a crew plan" plans={facts.plans} /><Choice name="reason" label="What conflicts with your intel?" options={challengeReasons(facts)} /></>; if (action === "commit_move") return <><PlanChoice name="selected_plan_id" label="Seal one plan" plans={facts.plans} /><label className="resource-choice"><input name="contribute_required_resource" type="checkbox" /> I will contribute the crew resource.</label></>; return null; }
function ExchangeChoices({ facts }: { readonly facts: ActionFacts }) {
  const [recipient, setRecipient] = useState("");
  const clueTargets = recipient === "" ? [] : dossiersForRole(recipient, facts.packDigest);
  return <><label className="mission-choice"><span>Send to</span><select name="recipient_role" value={recipient} required onChange={(event) => setRecipient(event.target.value)}><option value="" disabled>Choose a teammate…</option>{facts.recipients.map((seat) => <option key={seat.role} value={seat.role}>{humanize(seat.role)}</option>)}</select></label><Choice name="offered_clue_id" label="Offer this private clue" options={facts.clues.map((clue) => [clue.clueId, clueLabel(clue.clueId)])} /><label className="mission-choice"><span>Ask for</span><select name="consideration" defaultValue="" required disabled={recipient === ""}><option disabled value="">Choose…</option>{clueTargets.map((id) => <option key={`clue:${id}`} value={`clue_disclosure:${id}`}>A clue disclosure about {clueLabel(id)}</option>)}{facts.plans.map((plan, index) => <option key={`plan:${plan.planId}`} value={`plan_endorsement:${plan.planId}`}>Backing plan {index + 1}</option>)}</select></label></>;
}
function challengeReasons(facts: ActionFacts) { const options: Array<[string, string]> = []; if (facts.clues.some((clue) => clue.clueId === "route")) options.push(["route_conflict", "Route"]); if (facts.clues.some((clue) => clue.clueId === "entry_window")) options.push(["timing_conflict", "Entry time"]); if (facts.clues.some((clue) => clue.clueId === "required_tool")) options.push(["tool_conflict", "Equipment"]); if (facts.clues.some((clue) => clue.clueId === "extraction")) options.push(["extraction_conflict", "Extraction"]); return options; }
const PLAN_PARTS = [
  { key: "route", label: "Route", options: ["canal", "service", "roof"] },
  { key: "entry_window", label: "When to enter", options: ["late", "early", "middle"] },
  { key: "required_tool", label: "Equipment", options: ["disguise", "thermal_key", "jammer"] },
  { key: "extraction", label: "Extraction", options: ["van", "boat", "motorbike"] },
] as const;
type PlanPart = typeof PLAN_PARTS[number];

function PlanBuilder({ onReadyChange }: { readonly onReadyChange: (ready: boolean) => void }) {
  const [part, setPart] = useState(0);
  const [selection, setSelection] = useState<Partial<Record<PlanPart["key"], string>>>({});
  const current = PLAN_PARTS[part]!;
  const ready = PLAN_PARTS.every((item) => selection[item.key] !== undefined);
  useEffect(() => { onReadyChange(ready); });
  const choose = (value: string) => setSelection((currentSelection) => ({ ...currentSelection, [current.key]: value }));
  return <fieldset className="plan-builder"><legend>Build the four-part plan</legend>
    <div className="plan-step-tabs" role="tablist" aria-label="Plan parts">{PLAN_PARTS.map((item, index) => <button key={item.key} type="button" role="tab" aria-selected={part === index} onClick={() => setPart(index)}><span>{index + 1}</span>{item.label}{selection[item.key] === undefined ? "" : " ✓"}</button>)}</div>
    <span className="plan-step-label">Part {part + 1} of 4</span><h4>{current.label}</h4>
    <div className="plan-option-cards" role="radiogroup" aria-label={current.label}>{current.options.map((option) => <button key={option} type="button" role="radio" aria-checked={selection[current.key] === option} className={selection[current.key] === option ? "selected" : ""} onClick={() => choose(option)}><HeistArtwork kind={current.key} value={option} /><strong>{humanize(option)}</strong><small>{selection[current.key] === option ? "In your plan" : "Choose"}</small></button>)}</div>
    <div className="plan-summary" aria-label="Plan summary">{PLAN_PARTS.map((item) => <span key={item.key}><small>{item.label}</small><b>{selection[item.key] === undefined ? "Not chosen" : humanize(selection[item.key]!)}</b><input name={item.key} type="hidden" value={selection[item.key] ?? ""} /></span>)}</div>
    <div className="plan-step-controls"><button type="button" onClick={() => setPart((index) => Math.max(0, index - 1))} disabled={part === 0}>Previous</button><button type="button" onClick={() => setPart((index) => Math.min(PLAN_PARTS.length - 1, index + 1))} disabled={part === PLAN_PARTS.length - 1 || selection[current.key] === undefined}>Next: {part === PLAN_PARTS.length - 1 ? "review" : PLAN_PARTS[part + 1]!.label}</button></div>
  </fieldset>;
}
function PlanChoice({ name, label, plans }: { readonly name: string; readonly label: string; readonly plans: readonly AgentHeistReadyState["projection"]["plans"][number][] }) { return <Choice name={name} label={label} options={plans.map((plan, index) => [plan.planId, `Plan ${index + 1}: ${humanize(plan.route)} · ${humanize(plan.entryWindow)} · ${humanize(plan.requiredTool)} → ${humanize(plan.extraction)}`])} />; }
function Choice({ name, label, options }: { readonly name: string; readonly label: string; readonly options: readonly (readonly string[])[] }) { return <label className="mission-choice"><span>{label}</span><select name={name} defaultValue="" required><option disabled value="">Choose…</option>{options.map(([value, text]) => <option key={value} value={value}>{text}</option>)}</select></label>; }
function payloadFor(action: AgentHeistActionType, values: FormData, state: AgentHeistReadyState): Record<string, unknown> | null {
  const get = (name: string) => String(values.get(name) ?? "");
  const clues = state.projection.privateClues;
  const plans = state.projection.plans;
  if (action === "inspect_clue") { const id = get("clue_id"); return dossiersFor(state).filter((target) => !clues.some((clue) => clue.clueId === target)).includes(id) ? { clue_id: id } : null; }
  if (action === "publish_clue") { const clue = clues.find((item) => item.clueId === get("clue_id")); return clue === undefined ? null : { clue_id: clue.clueId, claim_code: clue.claimCode }; }
  if (action === "offer_exchange") {
    const recipient = get("recipient_role"); const [kind, id] = get("consideration").split(":", 2); const clue = clues.find((item) => item.clueId === get("offered_clue_id"));
    const recipientIsPresent = state.projection.seats.some((seat) => seat.present && seat.role === recipient && recipient !== state.authorization.role);
    const validConsideration = kind === "clue_disclosure" ? dossiersForRole(recipient, state.pack.digest).includes(id ?? "") : kind === "plan_endorsement" && plans.some((plan) => plan.planId === id);
    if (clue === undefined || !recipientIsPresent || !validConsideration || id === undefined) return null;
    return { recipient_role: recipient, offered_clue_id: clue.clueId, consideration: kind === "clue_disclosure" ? { kind, clue_id: id } : { kind, plan_id: id } };
  }
  if (action === "accept_exchange") { const id = get("offer_id"); return state.projection.addressedOffers.some((offer) => offer.offerId === id) ? { offer_id: id } : null; }
  if (action === "propose_plan") { const payload = { route: get("route"), entry_window: get("entry_window"), required_tool: get("required_tool"), extraction: get("extraction") }; return PLAN_PARTS.every((part) => part.options.includes(payload[part.key] as never)) ? payload : null; }
  if (action === "endorse_plan" || action === "challenge_plan") { const id = get("plan_id"); if (!plans.some((plan) => plan.planId === id)) return null; return action === "endorse_plan" ? { plan_id: id } : { plan_id: id, reason: get("reason") }; }
  if (action === "commit_move") { const id = get("selected_plan_id"); return plans.some((plan) => plan.planId === id) ? { selected_plan_id: id, contribute_required_resource: values.get("contribute_required_resource") === "on" } : null; }
  return {};
}
function NoAction({ phase }: { readonly phase: string }) { return <p className="no-action">{phase === "resolution" ? "The Room is resolving sealed choices. It will advance the mission when ready." : phase === "complete" ? "This operation is complete." : "No move is offered at this synchronized mission moment."}</p>; }
function Rules() { return <div className="rules-copy"><p>Open only your private intel, share true clues, then build or back a four-part crew plan.</p><p>Each player seals one plan and a resource choice. At least two sealed choices must name the same plan. The selected plan is checked for route, entry time, equipment, extraction, and whether any supporter contributed the resource.</p><p>Private intel stays private unless you share or trade it. This help never pauses the clock.</p></div>; }
function dossiersFor(state: AgentHeistReadyState) { const role = state.authorization.role; return role === null ? [] : dossiersForRole(role, state.pack.digest); }
function dossiersForRole(role: string, digest: string) { return HEIST_0_5_DOSSIERS[digest]?.[role] ?? []; }
function preferredOffer(state: AgentHeistReadyState) {
  const order: AgentHeistActionType[] = ["inspect_clue", "accept_exchange", "publish_clue", "propose_plan", "endorse_plan", "challenge_plan", "offer_exchange", "commit_move", "acknowledge_result"];
  return order.map((type) => state.offers.find((offer) => offer.actionType === type)).find((offer) => offer !== undefined && actionFacts(offer.actionType, state).unavailable === null);
}
function phaseCopy(state: AgentHeistReadyState, action: AgentHeistActionType | null) { if (state.projection.outcome !== null) return { title: "Read the crew debrief", detail: "The result below explains the crew's five mission checks." }; if (action === "inspect_clue") return { title: "Start with your dossier", detail: "Choose your sealed dossier. Opening it reveals intel only to you." }; if (state.projection.phase === "negotiation") return { title: "Find a plan the crew can trust", detail: "Share what you know, compare the choices, and make one deliberate move." }; if (state.projection.phase === "commitment") return { title: "Seal your decision", detail: "Choose a current plan. Sealed choices are not public endorsements." }; return { title: state.projection.phase === "lobby" ? "Crew assembling" : "Stay with the operation", detail: "The mission controls timing and will offer the next move." }; }
function phaseState(current: string, phase: string) { const order = ["lobby", "briefing", "negotiation", "commitment", "resolution", "result", "complete"]; const now = order.indexOf(current), target = order.indexOf(phase); return now === target ? "current" : now > target ? "past" : "future"; }
function actionTitle(action: AgentHeistActionType) { return ({ inspect_clue: "Open a dossier", publish_clue: "Share intel", offer_exchange: "Offer an exchange", accept_exchange: "Accept an exchange", propose_plan: "Build a plan", endorse_plan: "Back a plan", challenge_plan: "Flag a conflict", commit_move: "Seal your choice", acknowledge_result: "Acknowledge the result" } as const)[action]; }
function actionInstruction(action: AgentHeistActionType) { return ({ inspect_clue: "Choose your sealed dossier. Opening it reveals intel only to you.", publish_clue: "Share the exact clue you already know with the crew.", offer_exchange: "Offer one private clue in return for another clue or backing a current plan.", accept_exchange: "Choose an exchange addressed to you.", propose_plan: "Choose all four parts. These are proposal options, not claimed facts.", endorse_plan: "Back one current plan. This does not seal your final choice.", challenge_plan: "Flag the detail that conflicts with intel you personally know.", commit_move: "Review the plan and your resource contribution before sealing.", acknowledge_result: "Acknowledge the debrief when you are ready." } as const)[action]; }
function actionButton(action: AgentHeistActionType) { return ({ inspect_clue: "Open dossier", publish_clue: "Share with crew", offer_exchange: "Send exchange", accept_exchange: "Accept exchange", propose_plan: "Put plan on board", endorse_plan: "Back this plan", challenge_plan: "Flag conflict", commit_move: "Seal my choice", acknowledge_result: "Acknowledge result" } as const)[action]; }
function clueLabel(value: string) { return ({ route: "Route", entry_window: "Entry time", required_tool: "Equipment", extraction: "Extraction" } as Record<string, string>)[value] ?? humanize(value); }
function connectionLabel(connection: AgentHeistClientConnection) { return connection === "live" ? "Live" : connection === "connecting" ? "Connecting" : connection === "setup_required" ? "Setup needed" : "Disconnected"; }
function connectionMessage(state: AgentHeistReadyState, connection: AgentHeistClientConnection) { return connection === "live" ? state.authorization.accessMode === "participant" ? "Your private intel stays private until you choose to share it." : "You are following the crew's authorized public mission board." : "The board may be out of date. Reconnect before making a move."; }
function agentAssistLabel(status: AgentHeistAgentAssist) { return status === "available" ? "available" : status === "unsupported" ? "not supported" : status === "unavailable" ? "unavailable" : "checking"; }
function humanize(value: string) { return ({ service: "Service entrance", roof: "Rooftop" } as Record<string, string>)[value] ?? value.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase()); }
function Boundary({ title, detail }: { readonly title: string; readonly detail: string }) { return <main className="heist-boundary-shell"><span className="eyebrow">Agent Heist Activity Client</span><h1>{title}</h1><p>{detail}</p></main>; }
function nextUlid() { const bytes = crypto.getRandomValues(new Uint8Array(16)); bytes[0] = (bytes[0] ?? 0) & 0x3f; let value = bytes.reduce((result, byte) => (result << 8n) | BigInt(byte), 0n); const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"; let encoded = ""; for (let index = 0; index < 26; index += 1) { encoded = alphabet[Number(value & 31n)] + encoded; value >>= 5n; } return encoded; }
