import { useEffect, useMemo, useState } from "react";

import {
  RecordedAgentHeistWorkspace,
  type AgentHeistWorkspaceRecord,
} from "@worldstream/agent-heist-client/presentation";

import { demoBuildIdentity } from "./buildIdentity";
import {
  agentHeistEvidence,
  agentHeistRecords,
  visibleFixtureRecords,
  type AgentHeistFixtureRecord,
  type ProjectionLens,
} from "./agentHeistFixture";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export function AgentHeistPage({ onNavigate }: { onNavigate: Navigate }) {
  const [lens, setLens] = useState<ProjectionLens>("navigator");
  const [revealedThroughStep, setRevealedThroughStep] = useState(4);
  const [playing, setPlaying] = useState(false);
  const [selectedRecordId, setSelectedRecordId] = useState<string | null>(null);
  const visibleRecords = useMemo(
    () => visibleFixtureRecords(agentHeistRecords, lens, revealedThroughStep),
    [lens, revealedThroughStep],
  );
  const selectedRecord = visibleRecords.find((record) => record.id === selectedRecordId)
    ?? visibleRecords.at(-1);
  const workspaceRecords = useMemo(
    () => visibleRecords.map(toWorkspaceRecord),
    [visibleRecords],
  );
  const selectedWorkspaceRecord = selectedRecord === undefined
    ? undefined
    : workspaceRecords.find((record) => record.id === selectedRecord.id);
  const phase = agentHeistRecords[revealedThroughStep]?.phase ?? "briefing";

  useEffect(() => {
    if (!playing) return;
    if (revealedThroughStep >= agentHeistRecords.length - 1) {
      setPlaying(false);
      return;
    }

    const timer = window.setInterval(() => {
      setRevealedThroughStep((current) => Math.min(current + 1, agentHeistRecords.length - 1));
    }, 1050);
    return () => window.clearInterval(timer);
  }, [playing, revealedThroughStep]);

  const resetFixture = () => {
    setPlaying(false);
    setRevealedThroughStep(0);
    setSelectedRecordId(null);
  };

  return (
    <div className="site-shell heist-shell">
      <a className="skip-link" href="#heist-inspector">Skip to inspector</a>
      <SiteHeader onNavigate={onNavigate} />
      <main>
        <section className="heist-intro">
          <button className="back-link" type="button" onClick={() => onNavigate("/")}>← Mission select</button>
          <div className="heist-title-row">
            <div>
              <span className="eyebrow">Inside the heist / recorded demo</span>
              <h1>Agent <em>Heist</em></h1>
            </div>
            <span className="fixture-mode"><i /> Recorded fixture · no network</span>
          </div>
          <p>
            Follow the intel. Read the crew. Step through a recorded heist and see how private knowledge shapes a shared plan.
          </p>
        </section>

        <aside className="fixture-notice" aria-label="Demo boundary">
          <strong>Recorded demo · offline</strong>
          <span>Explore local example records. No live Room is connected, and playback does not submit Actions or run Replay.</span>
        </aside>

        <details className="recorded-guide">
          <summary>New to the crew? Open the 60-second guide</summary>
          <section className="guide" aria-labelledby="guide-title">
          <div>
            <span className="section-label">60-second guide</span>
            <h2 id="guide-title">Inspect the fixture</h2>
          </div>
          <ol>
            <GuideStep complete={true} number="01" text="Start with the Navigator view." />
            <GuideStep complete={revealedThroughStep >= 2} number="02" text="Find the private clue at Room sequence 0002." />
            <GuideStep complete={lens === "public" || lens === "operator"} number="03" text="Select Public or Operator. Confirm that the clue is absent." />
            <GuideStep complete={revealedThroughStep === agentHeistRecords.length - 1} number="04" text="Run the fixture to the recorded Replay evidence." />
          </ol>
          </section>
        </details>

        <RecordedAgentHeistWorkspace
          phase={phase}
          metrics={[
            { label: "Data mode", value: "Recorded · offline", tone: "blue" },
            {
              label: "Replay evidence",
              value: agentHeistEvidence.replayVerified ? "recorded · verified" : "recorded · not verified",
              tone: agentHeistEvidence.replayVerified ? "green" : "amber",
            },
            { label: "Recorded phase", value: capitalize(phase), tone: "amber" },
            { label: "Viewpoint", value: capitalize(lens), tone: "blue" },
            { label: "Visible moments", value: String(visibleRecords.length).padStart(2, "0"), tone: "violet" },
          ]}
          lenses={(["public", "navigator", "operator"] as const).map((item) => ({
            id: item,
            label: item === "navigator" ? "Navigator" : capitalize(item),
            description: lensDescription(item),
          }))}
          selectedLens={lens}
          onSelectLens={(next) => {
            setLens(next as ProjectionLens);
            setSelectedRecordId(null);
          }}
          playing={playing}
          canStep={revealedThroughStep < agentHeistRecords.length - 1}
          onTogglePlayback={() => setPlaying((current) => !current)}
          onStep={() => setRevealedThroughStep((current) => Math.min(current + 1, agentHeistRecords.length - 1))}
          onReset={resetFixture}
          progress={((revealedThroughStep + 1) / agentHeistRecords.length) * 100}
          progressLabel={`Step ${revealedThroughStep + 1} of ${agentHeistRecords.length}`}
          boundary={lensBoundary(lens)}
          records={workspaceRecords}
          selectedRecord={selectedWorkspaceRecord}
          onSelectRecord={setSelectedRecordId}
          footer={[
            `${visibleRecords.length} visible fixture records`,
            "Cursor: not simulated",
            "Connection: no network",
            "Replay: not run here",
          ]}
          identity={[
            { label: "Pack", value: agentHeistEvidence.packId },
            { label: "Pack version", value: agentHeistEvidence.packVersion },
            { label: "Revision digest", value: agentHeistEvidence.revisionDigest },
            { label: "Fixture ID", value: agentHeistEvidence.fixtureId },
            { label: "Final Room sequence", value: String(agentHeistEvidence.finalRoomSequence) },
            { label: "Evidence schema", value: agentHeistEvidence.sourceSchema },
            { label: "Transcript digest", value: agentHeistEvidence.transcriptDigest },
            { label: "Browser build", value: demoBuildIdentity.sourceRevision },
            { label: "Wire protocol", value: `${demoBuildIdentity.wireProtocol} · not connected` },
            { label: "WebSocket subprotocol", value: `${demoBuildIdentity.websocketSubprotocol} · not connected` },
          ]}
          identityNote="The browser reads selected metadata from the retained parity fixture. It does not execute this Activity Pack Revision."
        />

        <section className="what-it-shows" id="what-this-shows" aria-labelledby="shows-title">
          <div className="shows-copy">
            <span className="section-label">Technical boundary</span>
            <h2 id="shows-title">What this demo shows</h2>
            <ul>
              <li>The browser illustrates different Projection outputs for three views.</li>
              <li>The Public and Operator examples omit the Navigator clue.</li>
              <li>Recorded Semantic Time values identify phase changes.</li>
              <li>Recorded Attention summaries identify requests for an agent response.</li>
              <li>The parity fixture reports verified, read-only Replay of {agentHeistEvidence.verifiedTransitionCount} Transitions.</li>
            </ul>
            <a className="inline-doc-link" href="/docs/agent-heist/">Read how this fixture works</a>
          </div>
          <div className="data-flow-wrap">
            <div className="data-flow" aria-label="Authorization model represented by the fixture">
              <div><span>01</span><strong>Retained parity fixture</strong><small>Checked-in evidence metadata</small></div>
              <i aria-hidden="true">→</i>
              <div><span>02</span><strong>Authorization boundary</strong><small>WorldStream authority · not executed here</small></div>
              <i aria-hidden="true">→</i>
              <div><span>03</span><strong>Illustrative view</strong><small>Browser visibility filter</small></div>
            </div>
            <p className="data-flow-note">The browser represents this boundary. It does not enforce authorization.</p>
          </div>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}

function GuideStep({ complete, number, text }: { complete: boolean; number: string; text: string }) {
  return <li className={complete ? "is-complete" : ""}><span>{complete ? "✓" : number}</span><p>{text}</p></li>;
}

function toWorkspaceRecord(record: AgentHeistFixtureRecord): AgentHeistWorkspaceRecord {
  return {
    id: record.id,
    sequence: record.roomSequence?.value ?? "—",
    semanticTime: record.semanticTime?.value ?? "—",
    kind: record.kind,
    title: record.title,
    detail: record.detail,
    tone: record.tone,
  };
}

function lensDescription(lens: ProjectionLens) {
  if (lens === "public") return "Public Projection";
  if (lens === "navigator") return "Participant Projection";
  return "Bounded diagnostics";
}

function lensBoundary(lens: ProjectionLens) {
  if (lens === "public") return "This example contains public phase, plan, commitment-count, and Result summaries.";
  if (lens === "navigator") return "This example also contains the Navigator clue. The browser does not enforce authorization.";
  return "This example contains bounded operational summaries. It omits raw Activity State and private clues.";
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}
