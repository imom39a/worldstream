import { useState } from "react";

import {
  AggregateResultCard,
  ChallengeList,
  CommitmentSummary,
  DiscoveryPanel,
  FinalRevealGate,
  OperatorDiagnostics,
  ParticipantControls,
  PhaseHeader,
  PrivacyLabel,
  PublicClueList,
  PublicPlanList,
  ReplayPanel,
  QuarantinedSurface,
  UnavailableSurface,
  RolePresenceGrid,
  RuntimeStatusPanel,
  Section,
  StatusPill,
} from "./components";
import { assertHeistFixtureParity, resultFixture, type HeistFixture, type ViewId } from "./fixture";

const navigation: Array<{ id: ViewId; label: string; description: string }> = [
  { id: "public", label: "Public board", description: "Public-safe room projection" },
  { id: "participant", label: "Participant", description: "Fixture-gated Action Offers" },
  { id: "operator", label: "Operator diagnostics", description: "Bounded redacted facts" },
  { id: "replay", label: "Replay", description: "Read-only public history" },
];

interface AppProps {
  fixture?: HeistFixture;
  initialView?: ViewId;
}

/** Recorded reference gallery; live Activity Clients use their standalone routes. */
export function App({ fixture = resultFixture, initialView = "public" }: AppProps) {
  assertHeistFixtureParity(fixture);
  const [activeView, setActiveView] = useState<ViewId>(initialView);
  const unavailableRecovery = projectionRecovery(fixture);

  return (
    <main className="app-shell">
      <PhaseHeader fixture={fixture} />
      <DiscoveryPanel fixture={fixture} />
      <nav className="view-nav panel" aria-label="Agent Heist views">
        <div className="view-nav-intro">
          <span className="eyebrow">Reference surfaces</span>
          <span>One fixture, four explicit authorization views</span>
        </div>
        <div className="view-tabs" role="tablist" aria-label="Agent Heist views">
          {navigation.map((item) => (
            <button
              aria-controls={`${item.id}-view`}
              aria-selected={activeView === item.id}
              className={activeView === item.id ? "tab is-active" : "tab"}
              id={`${item.id}-tab`}
              key={item.id}
              onClick={() => setActiveView(item.id)}
              role="tab"
              type="button"
            >
              <span>{item.label}</span>
              <small>{item.description}</small>
            </button>
          ))}
        </div>
      </nav>

      <div className="view-note" role="status">
        <span className="status-dot" aria-hidden="true" />
        <strong>Recorded fixture mode</strong>
        <span>Static reference data only. Live participant and spectator sessions open through standalone Activity Client routes.</span>
      </div>
      <RuntimeStatusPanel runtime={fixture.runtime} />

      {activeView === "public" ? fixture.runtime.roomHealth === "Quarantined" ? <QuarantinedSurface surface="Public Projection" /> : unavailableRecovery ? <UnavailableSurface surface="Public Projection" recovery={unavailableRecovery} /> : <PublicView fixture={fixture} /> : null}
      {activeView === "participant" ? (
        <div id="participant-view" role="tabpanel" aria-labelledby="participant-tab">
          {fixture.runtime.roomHealth === "Quarantined" ? <QuarantinedSurface surface="Participant Projection" /> : <ParticipantControls fixture={fixture} />}
        </div>
      ) : null}
      {activeView === "operator" ? (
        <div id="operator-view" role="tabpanel" aria-labelledby="operator-tab">
          <OperatorDiagnostics diagnostics={fixture.operator} />
        </div>
      ) : null}
      {activeView === "replay" ? (
        <div id="replay-view" role="tabpanel" aria-labelledby="replay-tab">
          {unavailableRecovery ? <UnavailableSurface surface="Historical Replay" recovery={unavailableRecovery} /> : <ReplayPanel fixture={fixture} />}
        </div>
      ) : null}
    </main>
  );
}

function projectionRecovery(fixture: HeistFixture): "Loading" | "CatchingUp" | null {
  return fixture.runtime.recovery === "Loading" || fixture.runtime.recovery === "CatchingUp" ? fixture.runtime.recovery : null;
}

function PublicView({ fixture }: { fixture: HeistFixture }) {
  return (
    <div id="public-view" role="tabpanel" aria-labelledby="public-tab">
      <Section
        eyebrow="Public room projection"
        title="Crew presence"
        description="Role presence and activation markers are public-safe summaries; private participant context is not included."
      >
        <RolePresenceGrid roles={fixture.roles} />
      </Section>

      <div className="two-column">
        <Section eyebrow="Published clues" title="Clue claims" description="Only published public claims are shown.">
          <PublicClueList clues={fixture.publicClues} />
        </Section>
        <Section
          eyebrow="Commitment window"
          title="Commitment count"
          description="The count is public; individual values stay withheld."
        >
          <CommitmentSummary {...fixture.commitmentCount} />
        </Section>
      </div>

      <Section
        eyebrow="Negotiation surface"
        title="Plans and public review"
        description="Endorsements and challenges are aggregate public activity, not private offers or memory."
      >
        <PublicPlanList plans={fixture.publicPlans} />
        <div className="subsection-heading">
          <div>
            <span className="eyebrow">Challenges</span>
            <h3>Open public questions</h3>
          </div>
          <PrivacyLabel>Public event summary</PrivacyLabel>
        </div>
        <ChallengeList challenges={fixture.publicChallenges} />
      </Section>

      <Section
        eyebrow="Outcome"
        title="What the room has established"
        description="Result is distinct from Activity Phase and exposes aggregate checks only."
      >
        <AggregateResultCard fixture={fixture} />
      </Section>

      <div className="public-footer-grid">
        <div className="timeline panel">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Public timeline</p>
              <h2>Observed milestones</h2>
            </div>
            <StatusPill tone="good">Projection only</StatusPill>
          </div>
          <ol className="timeline-list">
            <li><span>Briefing</span><strong>Public claims opened</strong></li>
            <li><span>Negotiation</span><strong>Plans endorsed and challenged</strong></li>
            <li><span>Result</span><strong>Aggregate checks published</strong></li>
          </ol>
        </div>
        <FinalRevealGate fixture={fixture} />
      </div>
    </div>
  );
}
