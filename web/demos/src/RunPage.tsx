import { useEffect, useState } from "react";

import {
  readPublicRun,
  type PublicJson,
  type PublicRun,
  type PublicRunParticipant,
} from "./hostedApi";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export function RunPage({ publicId, onNavigate }: { publicId: string; onNavigate: Navigate }) {
  const [run, setRun] = useState<PublicRun | null>(null);

  useEffect(() => {
    let active = true;
    void readPublicRun(publicId)
      .then((value) => {
        if (active) setRun(value);
      })
      .catch(() => {
        if (active) setRun({ version: "public_run.v1", state: "unavailable" });
      });
    return () => { active = false; };
  }, [publicId]);

  return (
    <div className="site-shell hosted-shell">
      <a className="skip-link" href="#main-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main id="main-content" className="public-run-main">
        {run === null ? <RunStatus message="Loading the public Run…" /> : null}
        {run?.state === "unavailable" ? (
          <section className="public-run-unavailable" aria-labelledby="run-unavailable-title">
            <span className="run-state-chip state-unavailable">Unavailable</span>
            <h1 id="run-unavailable-title">This Run is not available.</h1>
            <p>
              The Run may not exist, may not be public, or may have been withheld after an integrity or privacy check.
            </p>
            <button type="button" onClick={() => onNavigate("/")}>Browse activities</button>
          </section>
        ) : null}
        {run !== null && run.state !== "unavailable" ? <AvailableRun run={run} /> : null}
      </main>
      <SiteFooter />
    </div>
  );
}

function AvailableRun({ run }: { run: Exclude<PublicRun, { readonly state: "unavailable" }> }) {
  return (
    <>
      <section className="public-run-hero">
        <div>
          <span className={`run-state-chip state-${run.state}`}>
            {run.state === "live" ? "Live Run" : "Verified result"}
          </span>
          <h1>{run.activity.title}</h1>
          <p>{run.activity.description}</p>
        </div>
        <div className="run-identity-card" aria-label="Public Run identity">
          <span>Public Run</span>
          <code>{run.public_id}</code>
          <strong>{run.evidence.label}</strong>
        </div>
      </section>

      <section className="public-run-grid">
        <article className="run-primary-card">
          {run.state === "live" ? (
            <div className="live-waiting">
              <span aria-hidden="true"><i /><i /><i /></span>
              <h2>The activity is in progress.</h2>
              <p>
                Anonymous live viewing is not connected for this Run. Return later to see a verified public result.
              </p>
            </div>
          ) : (
            <ResultSummary result={run.result} completedAt={run.completed_at} />
          )}
        </article>

        <aside className="run-roster-card" aria-labelledby="public-roster-title">
          <div>
            <span className="section-label">Reviewed attribution</span>
            <h2 id="public-roster-title">Participants</h2>
          </div>
          <div className="public-roster">
            {run.participants.map((participant) => (
              <PublicParticipantRow
                key={`${participant.role}:${participant.seat_label}`}
                participant={participant}
              />
            ))}
          </div>
        </aside>
      </section>

      <section className="run-provenance" aria-label="Public result provenance">
        <div><span>Started</span><strong>{formatDate(run.started_at)}</strong></div>
        <div><span>Pack</span><strong>{run.activity.pack.id} {run.activity.pack.version}</strong></div>
        <div><span>Evidence</span><strong>{run.evidence.label}</strong></div>
      </section>
    </>
  );
}

function ResultSummary({
  result,
  completedAt,
}: {
  result: Readonly<Record<string, PublicJson>>;
  completedAt: string;
}) {
  const entries = summaryEntries(result);
  return (
    <div className="public-result">
      <span className="section-label">Replay-verified public summary</span>
      <h2>Activity complete</h2>
      <p>Completed {formatDate(completedAt)}</p>
      <dl className="result-facts">
        {entries.map(([label, value]) => (
          <div key={label}>
            <dt>{humanize(label)}</dt>
            <dd>{displayValue(value)}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

function PublicParticipantRow({ participant }: { participant: PublicRunParticipant }) {
  const identity = participant.identity;
  const display = participant.kind === "house_agent"
    ? participant.house_agent?.display_name ?? participant.seat_label
    : identity?.kind === "github"
      ? `@${identity.login}`
      : identity?.label ?? participant.seat_label;
  return (
    <div className="public-participant">
      <span className="participant-mark" aria-hidden="true">{participant.seat_label.slice(0, 1)}</span>
      <div>
        <strong>{display}</strong>
        <span>{participant.seat_label} · {humanize(participant.role)}</span>
        {participant.notice !== undefined ? <small>{participant.notice}</small> : null}
      </div>
      <span className={`participant-kind kind-${participant.kind}`}>
        {participant.kind === "external_agent" ? "External agent" : participant.kind.replace("_", " ")}
      </span>
      {participant.house_agent !== undefined ? (
        <details>
          <summary>Exact House Agent details</summary>
          <dl>
            <div><dt>Revision</dt><dd><code>{participant.house_agent.revision_digest}</code></dd></div>
            <div><dt>Route</dt><dd>{participant.house_agent.route.provider_slug} / {participant.house_agent.route.model_slug}</dd></div>
            {Object.entries(participant.house_agent.allowance).map(([key, value]) => (
              <div key={key}><dt>{humanize(key)}</dt><dd>{value.toLocaleString()}</dd></div>
            ))}
          </dl>
        </details>
      ) : null}
    </div>
  );
}

function RunStatus({ message }: { message: string }) {
  return <section className="public-run-unavailable" role="status"><p>{message}</p></section>;
}

function summaryEntries(result: Readonly<Record<string, PublicJson>>): [string, PublicJson][] {
  const nested = result.summary;
  if (nested !== null && typeof nested === "object" && !Array.isArray(nested)) {
    return Object.entries(nested).filter(([key]) => key !== "schema");
  }
  return Object.entries(result).filter(([key]) => key !== "schema");
}

function displayValue(value: PublicJson): string {
  if (value === null) return "None";
  if (typeof value === "string") return humanize(value);
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number") return value.toLocaleString();
  return JSON.stringify(value);
}

function humanize(value: string): string {
  const words = value.replaceAll("_", " ");
  return words.length === 0 ? words : `${words[0]!.toUpperCase()}${words.slice(1)}`;
}

function formatDate(value: string): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}
