import { useState, type ReactNode } from "react";

import type { MidnightArchiveActionType } from "./actionContract";
import type {
  MidnightArchiveLiveState,
  MidnightArchiveReadyState,
} from "./liveAdapter";
import {
  actionCost,
  candidateById,
  candidateConfidence,
  type MidnightArchiveProjection,
  type ArchiveStagedAction,
  type MidnightArchiveActionIntent,
} from "./model";
import {
  ArchiveMap,
  ArchiveOutcomePanel,
  CarriedLedgerCard,
  CostPills,
  ResourceStrip,
  ScenarioBriefing,
  SectionHeading,
} from "./presentation";
import type { MidnightArchiveReplayState } from "./replay";
import { CrewPanel } from "./CrewPanel";
import { ExtractionPreviewPanel } from "./ExtractionPreviewPanel";
import { TurnResolutionPanel } from "./TurnResolutionPanel";

export type MidnightArchiveConnection =
  | "connecting"
  | "live"
  | "disconnected"
  | "setup_required";

export interface MidnightArchiveClientViewProps {
  readonly state: MidnightArchiveLiveState;
  readonly connection: MidnightArchiveConnection;
  readonly actionsEnabled: boolean;
  readonly submitting?: boolean;
  readonly message?: string | null;
  readonly onAction: (intent: MidnightArchiveActionIntent) => Promise<void>;
  readonly onReconnect?: () => Promise<void>;
  readonly replay?: MidnightArchiveReplayState;
  readonly onOpenReplay?: () => Promise<void>;
}

export function MidnightArchiveClientView({
  state,
  connection,
  actionsEnabled,
  submitting = false,
  message = null,
  onAction,
  onReconnect,
  replay = { kind: "unavailable" },
  onOpenReplay,
}: MidnightArchiveClientViewProps) {
  if (state.kind === "awaiting") {
    return <BoundarySurface
      eyebrow="Midnight Archive"
      title="Waiting for authorized Projection"
      detail={message ?? "The archive remains hidden until the retained participant session installs a current Projection."}
      action={connection === "disconnected" && onReconnect !== undefined ? (
        <button type="button" onClick={() => void onReconnect()}>Reconnect securely</button>
      ) : null}
    />;
  }
  if (state.kind === "incompatible") {
    return <BoundarySurface
      eyebrow="Client boundary"
      title="This archive cannot be opened"
      detail={state.reason}
    />;
  }
  return (
    <ReadyArchive
      state={state}
      connection={connection}
      actionsEnabled={actionsEnabled}
      submitting={submitting}
      message={message}
      onAction={onAction}
      onReconnect={onReconnect}
      replay={replay}
      onOpenReplay={onOpenReplay}
    />
  );
}

function ReadyArchive({
  state,
  connection,
  actionsEnabled,
  submitting,
  message,
  onAction,
  onReconnect,
  replay,
  onOpenReplay,
}: {
  readonly state: MidnightArchiveReadyState;
  readonly connection: MidnightArchiveConnection;
  readonly actionsEnabled: boolean;
  readonly submitting: boolean;
  readonly message: string | null;
  readonly onAction: (intent: MidnightArchiveActionIntent) => Promise<void>;
  readonly onReconnect?: () => Promise<void>;
  readonly replay: MidnightArchiveReplayState;
  readonly onOpenReplay?: () => Promise<void>;
}) {
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const projection = state.projection;
  const active = projection.phase === "active";
  const canAct = active && actionsEnabled && !submitting;
  const offerTypes = new Set(state.offers.map((offer) => offer.actionType));
  const act = (intent: MidnightArchiveActionIntent) => {
    if (!canAct || !offerTypes.has(intent.action)) return;
    setActionNotice(intent.action === "commit_turn"
      ? "Committing the staged turn to the Room…"
      : "Recording your staged Action with the Room…");
    void onAction(intent).then(
      () => setActionNotice(intent.action === "commit_turn"
        ? "Turn submitted. Waiting for the authoritative result…"
        : "Stage submitted. Waiting for the authoritative board…"),
      (error: unknown) => setActionNotice(error instanceof Error ? error.message : "The Action could not be submitted."),
    );
  };

  return (
    <main className="archive-shell">
      <a className="skip-link" href="#archive-actions">Skip to turn controls</a>
      {projection.companionDialogue.length > 0 ? <section className="archive-notice" aria-label="Companion advice">
        <div><h2>Companion advice</h2><p>Recommendations from your companions. You choose and commit every turn.</p>
          {projection.companionDialogue.map((item, index) => <div key={index}>
            <strong>{item.speaker === "mira" ? "Mira" : "Jonah"} · Turn {item.turn} · Recommendation</strong>
            <p>{item.text}</p>
          </div>)}
        </div>
      </section> : null}
      <header className="archive-hero">
        <div className="hero-copy">
          <p className="archive-kicker">WorldStream expedition 01</p>
          <h1>Midnight <em>Archive</em></h1>
          <p className="objective"><span>Your objective</span>{projection.objective}</p>
        </div>
        <div className="hero-seal" aria-hidden="true"><span>MA</span><i /></div>
        <div className="session-pill">
          <i className={connection === "live" && actionsEnabled ? "is-live" : ""} />
          {sessionLabel(connection, actionsEnabled, projection.phase)}
        </div>
      </header>

      {connection !== "live" ? (
        <section className="archive-notice connection-notice" role="status">
          <span aria-hidden="true">↻</span>
          <div><strong>Actions are locked</strong><p>{message ?? "Reconnect and wait for the current authorized Projection to be acknowledged."}</p></div>
          {connection === "disconnected" && onReconnect !== undefined ? (
            <button type="button" onClick={() => void onReconnect()}>Reconnect securely</button>
          ) : null}
        </section>
      ) : projection.phase === "briefing" ? (
        <section className="archive-notice briefing-notice" role="status">
          <span aria-hidden="true">⌛</span>
          <div><strong>Preparing the archive</strong><p>The Room is synchronized for readiness. Gameplay begins only after the Host records Activity Start.</p></div>
        </section>
      ) : !actionsEnabled && projection.phase !== "complete" ? (
        <section className="archive-notice connection-notice" role="status">
          <span aria-hidden="true">↻</span>
          <div><strong>Actions are locked</strong><p>{message ?? "Wait for the current authorized Projection to be acknowledged."}</p></div>
        </section>
      ) : null}

      <ResourceStrip projection={projection} />
      <ScenarioBriefing projection={projection} />
      <ArchiveOutcomePanel projection={projection} />
      {projection.phase === "complete" ? (
        <ReplayPanel replay={replay} onOpenReplay={onOpenReplay} />
      ) : null}

      <div className="archive-workspace">
        <div className="archive-main-column">
          <ArchiveMap
            projection={projection}
            enabled={canAct}
            moveOffered={offerTypes.has("stage_move")}
            onAction={act}
          />
          <CandidateBoard
            state={state}
            enabled={canAct}
            recoveryOffered={offerTypes.has("stage_recover_candidate")}
            onAction={act}
          />
        </div>

        <aside className="archive-turn-column" id="archive-actions" aria-label="Turn controls">
          <CarriedLedgerCard projection={projection} />
          <CrewPanel
            projection={projection}
            enabled={canAct}
            offerTypes={offerTypes}
            onAction={act}
          />
          <TurnResolutionPanel projection={projection} />
          <OptionalObjectivesPanel projection={projection} />
          {projection.location === "conservation" ? (
            <ArchivistAgreementCard projection={projection} />
          ) : null}
          {projection.verifierResult === null ? null : (
            <section className="verifier-result" role="status">
              <span aria-hidden="true">✓</span>
              <div>
                <small>Catalog verifier</small>
                <strong>{candidateById(projection, projection.verifierResult.candidateId)?.label ?? "Known candidate"}</strong>
                <p>Instrument confidence: verified</p>
              </div>
            </section>
          )}
          <ContextActions
            state={state}
            enabled={canAct}
            offerTypes={offerTypes}
            onAction={act}
          />
          <ExtractionPreviewPanel
            projection={projection}
            enabled={canAct}
            offerTypes={offerTypes}
            onAction={act}
          />
          <TurnCommitPanel
            state={state}
            enabled={canAct && offerTypes.has("commit_turn")}
            submitting={submitting}
            onCommit={() => act({ action: "commit_turn" })}
          />
          {actionNotice === null ? null : <p className="action-notice" role="status">{actionNotice}</p>}
        </aside>
      </div>

      <footer className="archive-footer">
        <span>Authorized lead Projection</span>
        <span>Room sequence {state.roomSequence}</span>
        <details>
          <summary>Session integrity</summary>
          <p>This client cannot switch Memberships or reveal the archive's hidden ledger truth.</p>
          <code>{state.pack.digest}</code>
        </details>
      </footer>
    </main>
  );
}

function ReplayPanel({
  replay,
  onOpenReplay,
}: {
  readonly replay: MidnightArchiveReplayState;
  readonly onOpenReplay?: () => Promise<void>;
}) {
  const available = replay.kind === "available" || replay.kind === "error";
  return (
    <section className="replay-panel" aria-labelledby="archive-replay-title">
      <div>
        <p className="archive-kicker">Verified history</p>
        <h2 id="archive-replay-title">Replay this expedition</h2>
        <p>Replay is read-only and must match this exact terminal Room Head.</p>
      </div>
      <button
        disabled={!available || onOpenReplay === undefined}
        onClick={() => { void onOpenReplay?.(); }}
        type="button"
      >
        {replay.kind === "loading" ? "Verifying…" : replay.kind === "verified" ? "Replay verified" : "Verify Replay"}
      </button>
      {replay.kind === "verified" ? (
        <div className="replay-status" role="status">
          <strong>✓ Verified at Room sequence {replay.summary.roomSequence}</strong>
          <span>Projection {shortDigest(replay.summary.projectionHash)}</span>
          <span>Pack {shortDigest(replay.summary.packDigest)}</span>
        </div>
      ) : replay.kind === "error" ? (
        <p className="replay-error" role="alert">{replay.message}</p>
      ) : replay.kind === "unavailable" ? (
        <p className="replay-unavailable">Replay is available on the local retained-session surface.</p>
      ) : null}
    </section>
  );
}

function CandidateBoard({
  state,
  enabled,
  recoveryOffered,
  onAction,
}: {
  readonly state: MidnightArchiveReadyState;
  readonly enabled: boolean;
  readonly recoveryOffered: boolean;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const projection = state.projection;
  const atVault = projection.location === "vault";
  const recoveryCost = actionCost(projection, "stage_recover_candidate");
  return (
    <section className="candidate-board" aria-labelledby="candidate-board-title">
      <SectionHeading
        eyebrow="Vault catalog"
        title="Candidate ledgers"
        aside={<span className="uncertainty-note">Visible marks only · hidden truth withheld</span>}
      />
      <div className="candidate-grid">
        {projection.candidates.map((candidate) => {
          const confidence = candidateConfidence(projection, candidate.candidateId);
          const carried = projection.carriedCandidate === candidate.candidateId;
          return (
            <article className={`candidate-card${carried ? " is-carried" : ""}`} key={candidate.candidateId}>
              <header>
                <span className="ledger-icon" aria-hidden="true">▥</span>
                <div><small>Archive candidate</small><h3>{candidate.label}</h3></div>
                {carried ? <b>Carried</b> : null}
              </header>
              <dl>
                {candidate.visibleAttributes.map((attribute) => (
                  <div key={`${attribute.label}:${attribute.value}`}><dt>{attribute.label}</dt><dd>{attribute.value}</dd></div>
                ))}
              </dl>
              <div className={`candidate-evidence evidence-${candidate.evidenceAssessment}`}>
                <small>{candidate.evidenceAssessment === "recommended"
                  ? "Evidence recommendation"
                  : candidate.evidenceAssessment === "observed"
                    ? "Observed source evidence"
                    : "Source evidence"}</small>
                {candidate.observedEvidence.length === 0 ? (
                  <p>Unknown until Records or Conservation is inspected.</p>
                ) : candidate.observedEvidence.map((evidence) => (
                  <p key={evidence.sourceId}>
                    <strong>{evidence.sourceLabel}:</strong> observed {evidence.attributeLabel.toLowerCase()} {evidence.observedValue}; this ledger {evidence.relation === "matches" ? "matches" : "does not match"} ({evidence.candidateValue}).
                  </p>
                ))}
              </div>
              <div className={`candidate-confidence confidence-${confidence}`}>
                {confidence === "verified" ? "✓ Verified by the catalog instrument" : "? Authenticity remains uncertain"}
              </div>
              <button
                disabled={!enabled || !recoveryOffered || !atVault || carried}
                onClick={() => onAction({ action: "stage_recover_candidate", candidate_id: candidate.candidateId })}
                type="button"
                aria-label={`Stage recovery of ${candidate.label}; costs ${recoveryCost.turns} turn and ${recoveryCost.power} power`}
              >
                {carried ? "Currently carried" : atVault ? "Stage this ledger" : "Recover at the Vault"}
              </button>
              <CostPills turns={recoveryCost.turns} power={recoveryCost.power} />
            </article>
          );
        })}
      </div>
    </section>
  );
}

function ContextActions({
  state,
  enabled,
  offerTypes,
  onAction,
}: {
  readonly state: MidnightArchiveReadyState;
  readonly enabled: boolean;
  readonly offerTypes: ReadonlySet<MidnightArchiveActionType>;
  readonly onAction: (intent: MidnightArchiveActionIntent) => void;
}) {
  const projection = state.projection;
  const actions: Array<{
    intent: MidnightArchiveActionIntent;
    eyebrow: string;
    title: string;
    description: string;
    turns: number;
    power: number;
    available: boolean;
    unavailableReason?: string;
  }> = [];
  if (projection.location === "records") actions.push({
    intent: { action: "stage_inspect_records" },
    eyebrow: "Records source",
    title: "Inspect the intake evidence",
    description: "Disclose the authored Records source. The projected operation cost is shown below.",
    ...actionCost(projection, "stage_inspect_records"),
    available: !projection.candidates.some((candidate) => candidate.observedEvidence.some((evidence) => evidence.sourceId === "records")),
  });
  if (projection.location === "records") actions.push({
    intent: { action: "stage_use_verifier" },
    eyebrow: "Records instrument",
    title: projection.verifierResult === null ? "Run the catalog verifier" : "Verifier result recorded",
    description: `Spend ${powerChargeLabel(actionCost(projection, "stage_use_verifier").power)} to identify one candidate with instrument confidence.`,
    ...actionCost(projection, "stage_use_verifier"),
    available: projection.verifierResult === null
      && projection.power >= actionCost(projection, "stage_use_verifier").power,
    unavailableReason: projection.verifierResult === null
      ? powerRequirement(actionCost(projection, "stage_use_verifier").power)
      : "The verifier result is already recorded.",
  });
  if (projection.location === "conservation") actions.push({
    intent: { action: "stage_inspect_conservation" },
    eyebrow: "Conservation source",
    title: "Inspect the restoration evidence",
    description: "Disclose the authored Conservation source. The projected operation cost is shown below.",
    ...actionCost(projection, "stage_inspect_conservation"),
    available: !projection.candidates.some((candidate) => candidate.observedEvidence.some((evidence) => evidence.sourceId === "conservation")),
  });
  if (projection.location === "conservation") actions.push({
    intent: { action: "stage_accept_preservation_agreement" },
    eyebrow: "Archivist agreement",
    title: projection.preservationAgreement.commitment === "not_accepted"
      ? "Accept the preservation agreement"
      : "Preservation agreement accepted",
    description: "Accept the Archivist's fixed terms. No free-text terms or hidden conditions are added.",
    ...actionCost(projection, "stage_accept_preservation_agreement"),
    available: projection.preservationAgreement.commitment === "not_accepted",
    unavailableReason: "The lead has already accepted this agreement.",
  });
  if (projection.location === "conservation") actions.push({
    intent: { action: "stage_prepare_collection" },
    eyebrow: "Optional objective",
    title: projection.optionalObjectives.collectionPreserved.status === "not_started"
      ? "Prepare the threatened collection"
      : "Collection preparation recorded",
    description: "Stabilize the collection before the preservation equipment can be energized.",
    ...actionCost(projection, "stage_prepare_collection"),
    available: projection.optionalObjectives.collectionPreserved.status === "not_started",
    unavailableReason: "The collection has already been prepared.",
  });
  if (projection.location === "conservation") {
    const collectionStatus = projection.optionalObjectives.collectionPreserved.status;
    actions.push({
      intent: { action: "stage_energize_preservation_equipment" },
      eyebrow: "Optional objective",
      title: collectionStatus === "complete"
        ? "Preservation equipment energized"
        : "Energize preservation equipment",
      description: `Spend ${powerChargeLabel(actionCost(projection, "stage_energize_preservation_equipment").power)} to preserve the prepared collection and honor the agreement when accepted.`,
      ...actionCost(projection, "stage_energize_preservation_equipment"),
      available: collectionStatus === "prepared"
        && projection.power >= actionCost(projection, "stage_energize_preservation_equipment").power,
      unavailableReason: collectionStatus === "not_started"
        ? "Prepare the collection first."
        : collectionStatus === "complete"
          ? "The collection is already preserved."
          : powerRequirement(actionCost(projection, "stage_energize_preservation_equipment").power),
    });
  }
  if (projection.location === "plant") actions.push({
    intent: { action: "stage_open_service_hatch" },
    eyebrow: "Service controls",
    title: projection.gates.service_hatch === "open" ? "Service hatch is open" : "Open the service hatch",
    description: "Route emergency power to the Plant–Vault passage. Traversal starts next turn.",
    ...actionCost(projection, "stage_open_service_hatch"),
    available: projection.gates.service_hatch === "closed"
      && projection.power >= actionCost(projection, "stage_open_service_hatch").power,
    unavailableReason: projection.gates.service_hatch === "open"
      ? "The service hatch is already open."
      : powerRequirement(actionCost(projection, "stage_open_service_hatch").power),
  });
  if (projection.location === "plant") {
    const sourceStatus = projection.optionalObjectives.sourceRecordProtected.status;
    actions.push({
      intent: { action: "stage_protect_source_record" },
      eyebrow: "Optional objective",
      title: sourceStatus === "complete"
        ? "Source record protected"
        : "Protect the source's identifying record",
      description: `Spend ${powerChargeLabel(actionCost(projection, "stage_protect_source_record").power)} after recovering a ledger to protect the source record.`,
      ...actionCost(projection, "stage_protect_source_record"),
      available: sourceStatus === "available"
        && projection.power >= actionCost(projection, "stage_protect_source_record").power,
      unavailableReason: sourceStatus === "locked"
        ? "Recover a ledger before protecting the source record."
        : sourceStatus === "complete"
          ? "The source record is already protected."
          : powerRequirement(actionCost(projection, "stage_protect_source_record").power),
    });
  }
  if (projection.location === "atrium") actions.push({
    intent: { action: "stage_extract" },
    eyebrow: "Atrium exit",
    title: "Extract from the archive",
    description: projection.carriedCandidate === null
      ? "Leave now without a ledger. The debrief will record an empty-handed extraction."
      : "Leave now with the carried ledger. Hidden authenticity resolves only in the outcome.",
    ...actionCost(projection, "stage_extract"),
    available: projection.mira.presence !== "active" || projection.mira.location === "atrium",
    unavailableReason: `Regroup Mira from ${locationLabel(state, projection.mira.location)} before extracting.`,
  });
  actions.push({
    intent: { action: "stage_wait" },
    eyebrow: "Hold position",
    title: "Wait for one turn",
    description: "Advance time without moving or spending power. No companion contribution is created by waiting; only a prepared Room-authorized contribution can resolve beside this turn.",
    ...actionCost(projection, "stage_wait"),
    available: true,
  });

  return (
    <section className="context-actions">
      <SectionHeading eyebrow="At this location" title="Available moves" />
      <div className="action-card-list">
        {actions.map((item) => {
          const offered = offerTypes.has(item.intent.action);
          return (
            <article className="action-card" key={item.intent.action}>
              <div>
                <small>{item.eyebrow}</small>
                <h3>{item.title}</h3>
                <p>{item.description}</p>
              </div>
              <CostPills turns={item.turns} power={item.power} />
              {item.available ? null : (
                <p className="unavailable-reason" role="note">
                  {item.unavailableReason ?? "This action is not currently available."}
                </p>
              )}
              <button
                disabled={!enabled || !offered || !item.available}
                onClick={() => onAction(item.intent)}
                type="button"
                aria-label={`Stage ${item.title}; costs ${item.turns} turn and ${item.power} power`}
              >
                {item.available ? "Stage action" : "Unavailable"}
              </button>
            </article>
          );
        })}
      </div>
    </section>
  );
}

function powerChargeLabel(power: number): string {
  return power === 1 ? "one charge" : `${power} charges`;
}

function powerRequirement(power: number): string {
  return power === 1 ? "One power charge is required." : `${power} power charges are required.`;
}

function TurnCommitPanel({
  state,
  enabled,
  submitting,
  onCommit,
}: {
  readonly state: MidnightArchiveReadyState;
  readonly enabled: boolean;
  readonly submitting: boolean;
  readonly onCommit: () => void;
}) {
  const staged = state.projection.stagedAction;
  return (
    <section className={`commit-panel${staged === null ? " is-empty" : ""}`}>
      <p className="archive-kicker">Staged turn</p>
      {staged === null ? (
        <div className="empty-stage">
          <span aria-hidden="true">＋</span>
          <strong>No Action staged</strong>
          <p>Choose a location or Action above. Staging does not spend turns or power.</p>
        </div>
      ) : (
        <div className="staged-summary">
          <span aria-hidden="true">◎</span>
          <div><strong>{stagedActionTitle(state, staged)}</strong><p>{stagedActionDetail(staged)}</p></div>
          <CostPills turns={staged.turnCost} power={staged.powerCost} />
        </div>
      )}
      <button
        className="commit-button"
        disabled={!enabled || staged === null || submitting}
        onClick={onCommit}
        type="button"
      >
        <span>{submitting ? "Submitting…" : "Commit Turn"}</span>
        <small>{staged === null ? "Stage an Action first" : `Apply ${staged.turnCost} turn · ${staged.powerCost} power`}</small>
      </button>
      <p className="commit-explainer">Only Commit Turn advances danger. The Room validates the action and returns the next authoritative state.</p>
    </section>
  );
}

function stagedActionTitle(state: MidnightArchiveReadyState, staged: ArchiveStagedAction): string {
  switch (staged.actionType) {
    case "stage_move": return `Move to ${locationLabel(state, staged.destination)}`;
    case "stage_inspect_records": return "Inspect the Records intake evidence";
    case "stage_inspect_conservation": return "Inspect the Conservation restoration evidence";
    case "stage_use_verifier": return "Run the catalog verifier";
    case "stage_accept_preservation_agreement": return "Accept the preservation agreement";
    case "stage_prepare_collection": return "Prepare the threatened collection";
    case "stage_energize_preservation_equipment": return "Energize preservation equipment";
    case "stage_open_service_hatch": return "Open the service hatch";
    case "stage_recover_candidate": return `Recover ${candidateById(state.projection, staged.candidateId)?.label ?? "candidate ledger"}`;
    case "stage_protect_source_record": return "Protect the source's identifying record";
    case "stage_extract": return "Extract through the Atrium";
    case "stage_wait": return "Wait in place";
  }
}

function stagedActionDetail(staged: ArchiveStagedAction): string {
  return staged.actionType === "stage_inspect_records" || staged.actionType === "stage_inspect_conservation"
    ? "The source is disclosed only when this committed turn completes."
    : staged.actionType === "stage_accept_preservation_agreement"
      ? "The lead accepts only the fixed terms shown on the Archivist card."
      : staged.actionType === "stage_prepare_collection"
        ? "Preparation completes before the equipment can be energized."
        : staged.actionType === "stage_energize_preservation_equipment"
          ? "Preservation completes this turn and opens the archive gate only if the agreement was accepted."
    : staged.actionType === "stage_open_service_hatch"
    ? "The gate opens this turn; movement through it begins on a later turn."
    : staged.actionType === "stage_recover_candidate"
      ? "Recovery changes what you carry without revealing hidden authenticity."
      : "Review the known cost, then commit when ready.";
}

function ArchivistAgreementCard({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const agreement = projection.preservationAgreement;
  return (
    <section className="agreement-card" aria-labelledby="archivist-agreement-title">
      <p className="archive-kicker">Authored offer · {agreement.speaker}</p>
      <h2 id="archivist-agreement-title">Preservation agreement</h2>
      <blockquote>“{agreement.statement}”</blockquote>
      <p className={`commitment-state commitment-${agreement.commitment}`}>
        Commitment: {agreement.commitment.replace("_", " ")}
      </p>
      <ol>
        {agreement.conditions.map((condition) => (
          <li className={`condition-${condition.status}`} key={condition.conditionId}>
            <span aria-hidden="true">{condition.status === "complete" ? "✓" : condition.status === "blocked" ? "×" : "○"}</span>
            <div><strong>{condition.label}</strong><small>{condition.status}</small></div>
            <CostPills turns={condition.turnCost} power={condition.powerCost} />
          </li>
        ))}
      </ol>
      <p className="fixed-terms-note">These terms are fixed by the Activity Pack and cannot be rewritten in free text.</p>
    </section>
  );
}

function OptionalObjectivesPanel({ projection }: { readonly projection: MidnightArchiveProjection }) {
  const objectives = [
    projection.optionalObjectives.collectionPreserved,
    projection.optionalObjectives.sourceRecordProtected,
  ];
  return (
    <section className="optional-objectives" aria-labelledby="optional-objectives-title">
      <p className="archive-kicker">Persistent mission status</p>
      <h2 id="optional-objectives-title">Optional objectives</h2>
      <div>
        {objectives.map((objective) => (
          <article key={objective.label}>
            <span className={`objective-status status-${objective.status}`}>{objective.status.replace("_", " ")}</span>
            <strong>{objective.label}</strong>
            <CostPills turns={objective.turnCost} power={objective.powerCost} />
          </article>
        ))}
      </div>
    </section>
  );
}

function locationLabel(state: MidnightArchiveReadyState, location: string): string {
  return state.projection.map.locations.find((candidate) => candidate.id === location)?.name ?? location;
}

function sessionLabel(
  connection: MidnightArchiveConnection,
  current: boolean,
  phase: MidnightArchiveReadyState["projection"]["phase"],
): string {
  if (connection !== "live") return "Reconnect required";
  if (phase === "briefing") return "Preparing archive";
  if (phase === "complete") return "Expedition complete";
  return current ? "Projection current" : "Catching up";
}

function shortDigest(value: string): string {
  return `${value.slice(0, 15)}…${value.slice(-8)}`;
}

function BoundarySurface({ eyebrow, title, detail, action = null }: {
  readonly eyebrow: string;
  readonly title: string;
  readonly detail: string;
  readonly action?: ReactNode;
}) {
  return (
    <main className="archive-boundary">
      <div className="boundary-seal" aria-hidden="true">MA</div>
      <p className="archive-kicker">{eyebrow}</p>
      <h1>{title}</h1>
      <p>{detail}</p>
      {action}
    </main>
  );
}
