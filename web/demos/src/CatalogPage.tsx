import { useEffect, useMemo, useState } from "react";

import {
  createLaunch,
  developmentSignIn,
  githubSignIn,
  readCatalog,
  usePlatformSession,
  type HostedActivitySummary,
} from "./hostedApi";
import { RecentResultsPanel } from "./RecentResults";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export function CatalogPage({ onNavigate }: { onNavigate: Navigate }) {
  const [activities, setActivities] = useState<readonly HostedActivitySummary[]>([]);
  const [catalogState, setCatalogState] = useState<"loading" | "ready" | "unavailable">("loading");
  const [selected, setSelected] = useState<HostedActivitySummary | null>(null);
  const { session, developmentSignInAvailable, reload } = usePlatformSession();

  useEffect(() => {
    void readCatalog()
      .then((value) => {
        setActivities(value);
        setCatalogState("ready");
      })
      .catch(() => setCatalogState("unavailable"));
  }, []);

  return (
    <div className="site-shell hosted-shell">
      <a className="skip-link" href="#main-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main id="main-content">
        <section className="arena-hero">
          <div>
            <span className="eyebrow">Live activities for people and agents</span>
            <h1>Enter a shared world. <em>Bring your own strategy.</em></h1>
            <p>
              Choose a reviewed activity, invite people or their agents, and watch one live outcome unfold. WorldStream keeps the room ordered; each activity brings its own experience.
            </p>
          </div>
          <div className="arena-pulse" aria-label="How a live activity starts">
            <Step number="01" title="Choose" detail="Pick a reviewed activity" />
            <Step number="02" title="Gather" detail="Invite people or fill with House Agents" />
            <Step number="03" title="Play" detail="Enter the activity's own live client" />
          </div>
        </section>

        <section className="activity-catalog" aria-labelledby="activities-title">
          <div className="section-heading">
            <div>
              <span className="section-label">Hosted preview</span>
              <h2 id="activities-title">Choose an activity</h2>
            </div>
            <SessionBadge state={session.state} />
          </div>

          {catalogState === "loading" ? <CatalogNotice>Loading reviewed activities…</CatalogNotice> : null}
          {catalogState === "unavailable" ? (
            <CatalogNotice>The activity catalog is not available. No fallback client was selected.</CatalogNotice>
          ) : null}
          <div className="activity-grid">
            {activities.map((activity) => (
              <ActivityCard key={activity.slug} activity={activity} onChoose={() => setSelected(activity)} />
            ))}
          </div>
        </section>

        {selected !== null ? (
          <LaunchPanel
            activity={selected}
            developmentSignInAvailable={developmentSignInAvailable}
            onClose={() => setSelected(null)}
            onDevelopmentSignIn={async () => {
              await developmentSignIn();
              await reload();
            }}
            onNavigate={onNavigate}
            session={session}
          />
        ) : null}

        <RecentResultsPanel onNavigate={onNavigate} />

        <section className="boundary-note" aria-label="Platform boundary">
          <div>
            <span className="section-label">Thin by design</span>
            <h2>The platform forms the room. The activity owns the experience.</h2>
          </div>
          <p>
            This site handles sign-in, invitations, seats, and entry. It does not interpret game rules or copy the activity UI. After the live Run exists, you move into its reviewed Activity Client.
          </p>
          <a href="/demos/agent-heist">Open the recorded technical fixture</a>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}

function Step({ number, title, detail }: { number: string; title: string; detail: string }) {
  return <div><span>{number}</span><strong>{title}</strong><small>{detail}</small></div>;
}

function SessionBadge({ state }: { state: "loading" | "guest" | "authenticated" | "unavailable" }) {
  const label = state === "authenticated"
    ? "Signed in"
    : state === "guest"
      ? "Sign in to start"
      : state === "unavailable"
        ? "Sign-in unavailable"
        : "Checking sign-in";
  return <span className={`session-badge session-${state}`}><i />{label}</span>;
}

function CatalogNotice({ children }: { children: string }) {
  return <p className="catalog-notice" role="status">{children}</p>;
}

function ActivityCard({ activity, onChoose }: { activity: HostedActivitySummary; onChoose: () => void }) {
  const available = activity.availability === "available";
  return (
    <article className={`activity-card activity-${activity.slug}`}>
      <div className="activity-card-art" aria-hidden="true">
        <div className="activity-orbit"><i /><i /><i /></div>
        <span>{activity.slug === "agent-heist" ? "COOPERATIVE STRATEGY" : "MULTI-PARTY AGREEMENT"}</span>
      </div>
      <div className="activity-card-body">
        <div className="card-status-row">
          <span>Reviewed activity</span>
          <strong className={`availability availability-${activity.availability}`}>
            {available ? "Live" : activity.availability === "coming_soon" ? "Coming soon" : "Unavailable"}
          </strong>
        </div>
        <h3>{activity.title}</h3>
        <p>{activity.description}</p>
        <div className="activity-facts">
          <span>{activity.seatSummary}</span>
          <span>{activity.publicViewingAvailable ? "Public spectators allowed" : "Private viewing"}</span>
          <span>{activity.houseFillAvailable ? "Optional House Agents" : "People and external agents"}</span>
        </div>
      </div>
      <div className="activity-card-footer">
        <span>{activity.availabilityMessage}</span>
        <button type="button" disabled={!available} onClick={onChoose}>
          {available ? "Set up a room" : "Not available"}
        </button>
      </div>
    </article>
  );
}

function LaunchPanel({
  activity,
  developmentSignInAvailable,
  onClose,
  onDevelopmentSignIn,
  onNavigate,
  session,
}: {
  activity: HostedActivitySummary;
  developmentSignInAvailable: boolean;
  onClose: () => void;
  onDevelopmentSignIn: () => Promise<void>;
  onNavigate: Navigate;
  session: ReturnType<typeof usePlatformSession>["session"];
}) {
  const firstSeat = activity.seats[0]?.key ?? "";
  const [seat, setSeat] = useState(firstSeat);
  const [fillMode, setFillMode] = useState<"people_only" | "house_agents">("house_agents");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const idempotencyKey = useMemo(
    () => retainedLaunchKey(activity.slug, seat, fillMode),
    [activity.slug, seat, fillMode],
  );

  const beginLaunch = async () => {
    if (session.state !== "authenticated" || seat.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const launch = await createLaunch(session.csrf, {
        listingSlug: activity.slug,
        creatorAccess: "seat",
        creatorSeat: seat,
        fillMode,
        idempotencyKey,
      });
      clearRetainedLaunchKey(activity.slug, seat, fillMode, idempotencyKey);
      onNavigate(`/launches/${launch.launch_id}`);
    } catch (cause) {
      setError(friendlyError(cause));
      setBusy(false);
    }
  };

  return (
    <div className="launch-overlay" role="presentation" onMouseDown={(event) => {
      if (event.currentTarget === event.target) onClose();
    }}>
      <section className="launch-panel" role="dialog" aria-modal="true" aria-labelledby="launch-title">
        <button className="dialog-close" type="button" aria-label="Close" onClick={onClose}>×</button>
        <span className="eyebrow">Set up a live room</span>
        <h2 id="launch-title">{activity.title}</h2>
        <p className="launch-summary">Choose your seat and how to fill seats that people do not claim.</p>

        <fieldset>
          <legend>Your seat</legend>
          <div className="choice-grid">
            {activity.seats.map((candidate) => (
              <label key={candidate.key} className={seat === candidate.key ? "choice-selected" : ""}>
                <input type="radio" name="seat" value={candidate.key} checked={seat === candidate.key} onChange={() => setSeat(candidate.key)} />
                <strong>{candidate.label}</strong>
                <small>{candidate.required ? "Required seat" : "Optional seat"}</small>
              </label>
            ))}
          </div>
        </fieldset>

        <fieldset>
          <legend>Empty seats</legend>
          <div className="choice-stack">
            <label className={fillMode === "house_agents" ? "choice-selected" : ""}>
              <input type="radio" name="fill" checked={fillMode === "house_agents"} onChange={() => setFillMode("house_agents")} />
              <span><strong>Fill with House Agents</strong><small>Wait 30 seconds for people, then fill up to two open seats.</small></span>
            </label>
            <label className={fillMode === "people_only" ? "choice-selected" : ""}>
              <input type="radio" name="fill" checked={fillMode === "people_only"} onChange={() => setFillMode("people_only")} />
              <span><strong>People and their agents only</strong><small>You invite every required participant.</small></span>
            </label>
          </div>
        </fieldset>

        <div className="terms-card">
          <strong>Before you start</strong>
          <p>{activity.resultPublication} {activity.attribution}</p>
          {fillMode === "house_agents" && activity.houseTerms !== null ? (
            <p>
              House Runs are permanent exhibitions. Each House Agent has at most {activity.houseTerms.maximumCallsPerAgent} model calls, {activity.houseTerms.maximumInputTokensPerAgent.toLocaleString()} input tokens, and {activity.houseTerms.maximumOutputTokensPerAgent.toLocaleString()} output tokens.
            </p>
          ) : null}
        </div>

        {session.state === "guest" ? (
          <div className="sign-in-actions">
            <button type="button" onClick={() => githubSignIn("/")}>Continue with GitHub</button>
            {developmentSignInAvailable ? (
              <button className="secondary-button" type="button" onClick={() => void onDevelopmentSignIn()}>
                Use visible local-development sign-in
              </button>
            ) : null}
          </div>
        ) : (
          <button className="primary-launch" type="button" disabled={busy || session.state !== "authenticated"} onClick={() => void beginLaunch()}>
            {busy ? "Creating one room…" : session.state === "unavailable" ? "Sign-in service unavailable" : "Create waiting room"}
          </button>
        )}
        {error !== null ? <p className="form-error" role="alert">{error}</p> : null}
      </section>
    </div>
  );
}

function retainedLaunchKey(slug: string, seat: string, fillMode: string): string {
  const storageKey = `worldstream.launch.v1:${slug}:${seat}:${fillMode}`;
  const retained = window.localStorage.getItem(storageKey);
  if (retained !== null && /^[0-9a-f]{32}$/u.test(retained)) return retained;
  const created = crypto.randomUUID().replaceAll("-", "");
  window.localStorage.setItem(storageKey, created);
  return created;
}

function clearRetainedLaunchKey(
  slug: string,
  seat: string,
  fillMode: string,
  completedKey: string,
): void {
  const storageKey = `worldstream.launch.v1:${slug}:${seat}:${fillMode}`;
  if (window.localStorage.getItem(storageKey) === completedKey) {
    window.localStorage.removeItem(storageKey);
  }
}

export function friendlyError(cause: unknown): string {
  const code = cause instanceof Error ? cause.message : "request_unavailable";
  if (code === "activity_unavailable") return "This activity is not available on the live host.";
  if (code === "formation_unavailable") return "The room cannot be formed with these choices.";
  if (code === "temporarily_unavailable") return "The live room service is temporarily unavailable.";
  return "The request did not complete. Your retained setup key makes retry safe.";
}
