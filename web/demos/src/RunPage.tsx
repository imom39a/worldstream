import { useEffect, useState } from "react";

import {
  readPublicRun,
  type PublicJson,
  type PublicRun,
  type PublicRunParticipant,
} from "./hostedApi";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

const PUBLIC_RUN_RECONCILIATION_INTERVAL_MS = 5_000;
const PUBLIC_RUN_RECONCILIATION_SLOW_INTERVAL_MS = 30_000;
const PUBLIC_RUN_RECONCILIATION_FAST_POLLS = 12;

export function RunPage({ publicId, onNavigate }: { publicId: string; onNavigate: Navigate }) {
  const [run, setRun] = useState<PublicRun | null>(null);

  useEffect(() => {
    let active = true;
    let livePolls = 0;
    let timer: number | null = null;
    const controller = new AbortController();

    const read = async (): Promise<void> => {
      try {
        const value = await readPublicRun(publicId, controller.signal);
        if (!active) return;
        setRun(value);
        if (value.state === "live") {
          livePolls += 1;
          const interval = livePolls <= PUBLIC_RUN_RECONCILIATION_FAST_POLLS
            ? PUBLIC_RUN_RECONCILIATION_INTERVAL_MS
            : PUBLIC_RUN_RECONCILIATION_SLOW_INTERVAL_MS;
          timer = window.setTimeout(() => {
            timer = null;
            void read();
          }, interval);
        }
      } catch {
        if (active && !controller.signal.aborted) {
          setRun({ version: "public_run.v1", state: "unavailable" });
        }
      }
    };

    void read();
    return () => {
      active = false;
      controller.abort();
      if (timer !== null) window.clearTimeout(timer);
    };
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
  if (run.state === "live") return <LiveRun key={run.public_id} run={run} />;
  return (
    <>
      <section className="public-run-hero">
        <div>
          <span className={`run-state-chip state-${run.state}`}>
            Verified result
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
          <ResultSummary result={run.result} completedAt={run.completed_at} />
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

function LiveRun({ run }: { run: Extract<PublicRun, { readonly state: "live" }> }) {
  const client = run.client;
  return (
    <>
      <section className="public-live-context" aria-label="Public Run context">
        <div>
          <span className="run-state-chip state-live">Live · read only</span>
          <strong>{run.activity.title}</strong>
          <span>{run.evidence.label}</span>
        </div>
        <div>
          <span>Public Run</span>
          <code>{run.public_id}</code>
        </div>
      </section>
      <section className="public-run-unavailable" aria-labelledby="public-live-client-title">
        <span className="run-state-chip state-live">Live · read only</span>
        <h1 id="public-live-client-title">Open the reviewed Activity Client</h1>
        <p>
          This public page does not interpret Pack state. The selected client opens a
          credential-free spectator projection for this exact reviewed release.
        </p>
        {client === undefined ? (
          <p>The retained client release for this Run does not support public viewing.</p>
        ) : (
          <a className="button-link" href={client.launch_url}>Watch live Run</a>
        )}
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
