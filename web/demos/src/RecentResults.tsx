import { useEffect, useState } from "react";

import { readRecentResults, type RecentResults } from "./hostedApi";
import type { Navigate } from "./siteChrome";

export function RecentResultsPanel({ onNavigate }: { onNavigate: Navigate }) {
  const [feed, setFeed] = useState<RecentResults | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    let active = true;
    void readRecentResults()
      .then((value) => {
        if (active) setFeed(value);
      })
      .catch(() => {
        if (active) setUnavailable(true);
      });
    return () => { active = false; };
  }, []);

  return (
    <section className="recent-results" aria-labelledby="recent-results-title">
      <div className="section-heading">
        <div>
          <span className="section-label">From the community / newest first</span>
          <h2 id="recent-results-title">Recent results<span className="heading-dot">.</span></h2>
        </div>
        <span className="recent-limit">Agent Heist · recent results</span>
      </div>
      {feed === null && !unavailable ? <p className="catalog-notice">Loading recent results…</p> : null}
      {unavailable ? <p className="catalog-notice">Recent results are temporarily unavailable.</p> : null}
      {feed?.results.length === 0 ? (
        <div className="recent-empty">
          <strong>Every activity starts a new story.</strong>
          <span>No public results yet. Completed, Replay-verified Agent Heist Runs will appear here.</span>
        </div>
      ) : null}
      {feed !== null && feed.results.length > 0 ? (
        <div className="recent-result-list">
          {feed.results.map((run) => (
            <a
              key={run.public_id}
              href={`/runs/${run.public_id}`}
              onClick={(event) => {
                event.preventDefault();
                onNavigate(`/runs/${run.public_id}`);
              }}
            >
              <span className="result-date">{formatDate(run.completed_at)}</span>
              <strong>{run.activity.title}</strong>
              <span>{run.participants.map((participant) => participant.seat_label).join(" · ")}</span>
              <em>{run.evidence.label}</em>
              <b aria-hidden="true">→</b>
            </a>
          ))}
        </div>
      ) : null}
    </section>
  );
}

function formatDate(value: string): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}
