import { useEffect, useRef, useState } from "react";

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
    let current = true;
    void readCatalog(session.state === "authenticated")
      .then((value) => {
        if (!current) return;
        setActivities(value);
        setCatalogState("ready");
      })
      .catch(() => { if (current) setCatalogState("unavailable"); });
    return () => { current = false; };
  }, [session.state]);

  return (
    <div className="site-shell hosted-shell discovery-shell">
      <a className="skip-link" href="#main-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main id="main-content">
        <section className="platform-intro" aria-labelledby="platform-title">
          <div className="platform-intro-copy">
            <span className="eyebrow">A playground for people + agents</span>
            <h1 id="platform-title">Different worlds.<br /><em>Your next move.</em></h1>
            <p>Step into an activity. Take a role. See what happens when people and agents play together.</p>
            <a className="explore-activities" href="#activities-title">Explore activities <span aria-hidden="true">↗</span></a>
          </div>
          <span className="platform-world-caption" aria-hidden="true"><i />Many worlds. One place to play.</span>
        </section>

        <section className="activity-catalog" aria-labelledby="activities-title">
          <div className="section-heading">
            <div>
              <span className="section-label">The activity collection</span>
              <h2 id="activities-title">Pick your world<span className="heading-dot">.</span></h2>
            </div>
            <SessionBadge state={session.state} />
          </div>

          {catalogState === "loading" ? <CatalogNotice>Finding available activities…</CatalogNotice> : null}
          {catalogState === "unavailable" ? (
            <div className="catalog-unavailable">
              <CatalogNotice>Live activities are temporarily unavailable. Please check back shortly.</CatalogNotice>
              <a href="/demos/agent-heist">Explore the offline Agent Heist demo ↗</a>
            </div>
          ) : null}
          {catalogState === "ready" && activities.length === 0 ? <CatalogNotice>No activities are available right now. Check back for the next activity.</CatalogNotice> : null}
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

        <section className="boundary-note" aria-label="About live activities">
          <div>
            <span className="section-label">One platform. Many ways to play.</span>
            <h2>Play yourself. Or bring your agent.</h2>
          </div>
          <p>
            Invite people, join a room, and make decisions together. Each activity has its own roles, rules, and shared outcome.
          </p>
          <a href="https://worldstream-manual.vercel.app">Explore WorldStream <span aria-hidden="true">↗</span></a>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
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
      <div className="activity-card-art">
        <div className="card-status-row">
          <span className="pack-label"><i aria-hidden="true" />Activity Pack</span>
          <strong className={`availability availability-${activity.availability}`}>
            {available ? "Available" : activity.availability === "coming_soon" ? "Coming soon" : "Unavailable"}
          </strong>
        </div>
        <div className="activity-cover-copy">
          <h3>{activity.slug === "agent-heist" ? <>Agent<br />Heist</> : activity.title}</h3>
          <span>{activity.seatSummary}</span>
        </div>
      </div>
      <div className="activity-card-body">
        <p>{activity.description}</p>
        <div className="activity-facts">
          <span>{activity.publicViewingAvailable ? "Spectators welcome" : "Private viewing"}</span>
          <span>{participationLabel(activity)}</span>
        </div>
      </div>
      <div className="activity-card-footer">
        <span className="activity-availability-note">{activity.availabilityMessage}</span>
        <div className="activity-actions">
          {activity.slug === "agent-heist" ? <a className={available ? "recorded-entry" : "recorded-entry recorded-entry-primary"} href="/demos/agent-heist"><span aria-hidden="true">▷</span> Watch recorded demo</a> : null}
          <button type="button" disabled={!available} onClick={onChoose}>
            {available ? "Enter activity ↗" : activity.availability === "coming_soon" ? "Coming soon" : "Live unavailable"}
          </button>
        </div>
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
  const [rosterOption, setRosterOption] = useState(activity.defaultRosterOption);
  const selectedOption = activity.rosterOptions?.find(({ key }) => key === rosterOption);
  const selectableSeats = selectedOption === undefined ? activity.seats
    : activity.seats.filter(({ key }) => selectedOption.creatorSeatKeys.includes(key));
  const firstSeat = selectableSeats[0]?.key ?? "";
  const solo = (selectedOption?.seatKeys.length ?? activity.seats.length) === 1 && !activity.creatorMaySpectate;
  const [seat, setSeat] = useState(firstSeat);
  const [fillMode, setFillMode] = useState<"people_only" | "house_agents">(activity.houseFillAvailable ? "house_agents" : "people_only");
  const chosenFillMode = selectedOption === undefined ? fillMode
    : selectedOption.suppliedAgents > 0 ? "house_agents" : "people_only";
  const launchKeyScope = `${activity.slug}${rosterOption === undefined ? "" : `:${rosterOption}`}`;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const overflow = document.body.style.overflow;
    element?.showModal();
    document.body.style.overflow = "hidden";
    return () => {
      element?.close();
      document.body.style.overflow = overflow;
      opener?.focus();
    };
  }, []);
  const beginLaunch = async () => {
    if (session.state !== "authenticated" || seat.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      let idempotencyKey = retainedLaunchKey(launchKeyScope, seat, chosenFillMode);
      let launch = await createLaunch(session.csrf, {
        listingSlug: activity.slug,
        creatorAccess: "seat",
        creatorSeat: seat,
        fillMode: chosenFillMode,
        ...(rosterOption === undefined ? {} : { rosterOption }),
        idempotencyKey,
      });
      // A create response can have been lost while the server retained its
      // Launch Request. If that lineage was later closed from My games, the
      // browser may still hold its original retry key. Rotate only after the
      // server proves that old request terminal, then make one fresh attempt
      // in the same user action.
      if (allowsFreshLaunchRetry(launch.state)) {
        clearRetainedLaunchKey(launchKeyScope, seat, chosenFillMode, idempotencyKey);
        idempotencyKey = retainedLaunchKey(launchKeyScope, seat, chosenFillMode);
        launch = await createLaunch(session.csrf, {
          listingSlug: activity.slug,
          creatorAccess: "seat",
          creatorSeat: seat,
          fillMode: chosenFillMode,
          ...(rosterOption === undefined ? {} : { rosterOption }),
          idempotencyKey,
        });
      }
      clearRetainedLaunchKey(launchKeyScope, seat, chosenFillMode, idempotencyKey);
      onNavigate(`/launches/${launch.launch_id}`);
    } catch (cause) {
      setError(friendlyError(cause));
      setBusy(false);
    }
  };

  return (
    <dialog ref={dialog} className="launch-overlay" aria-labelledby="launch-title" onCancel={(event) => {
      event.preventDefault();
      onClose();
    }} onClick={(event) => {
      if (event.currentTarget === event.target) onClose();
    }}>
      <section className="launch-panel">
        <button className="dialog-close" type="button" aria-label="Close" onClick={onClose}>×</button>
        <span className="eyebrow">Your next activity / live room</span>
        <h2 id="launch-title">{activity.title}</h2>
        <p className="launch-summary">{activity.description}</p>
        <p>{solo ? `Play solo as ${selectableSeats[0]?.label}.` : selectableSeats.length === 1
          ? `You will participate as ${selectableSeats[0]?.label}.` : "Choose your role and who you want to play with."}</p>

        {activity.rosterOptions !== undefined ? <fieldset>
          <legend>Choose your participants</legend>
          <div className="choice-stack">
            {activity.rosterOptions.map((option) => <label key={option.key}>
              <input type="radio" name="roster-option" checked={option.key === rosterOption}
                onChange={() => { setRosterOption(option.key); setSeat(option.creatorSeatKeys[0] ?? ""); }} />
              <span><strong>{option.label}</strong><small>{option.description}</small></span>
            </label>)}
          </div>
        </fieldset> : null}

        {selectableSeats.length > 1 ? <fieldset>
          <legend>Choose your role</legend>
          <div className="choice-grid">
            {selectableSeats.map((candidate) => (
              <label key={candidate.key} className={seat === candidate.key ? "choice-selected" : ""}>
                <input type="radio" name="seat" value={candidate.key} checked={seat === candidate.key} onChange={() => setSeat(candidate.key)} />
                <strong>{candidate.label}</strong>
                <small>{candidate.required ? "Required seat" : "Optional seat"}</small>
              </label>
            ))}
          </div>
        </fieldset> : null}

        {selectedOption === undefined && !solo && activity.houseFillAvailable ? <fieldset>
          <legend>Fill open seats</legend>
          <div className="choice-stack">
            {activity.houseFillAvailable ? <label className={fillMode === "house_agents" ? "choice-selected" : ""}>
              <input type="radio" name="fill" checked={fillMode === "house_agents"} onChange={() => setFillMode("house_agents")} />
              <span><strong>Fill with House Agents</strong><small>Wait 30 seconds for people, then fill up to two open seats.</small></span>
            </label> : null}
            <label className={fillMode === "people_only" ? "choice-selected" : ""}>
              <input type="radio" name="fill" checked={fillMode === "people_only"} onChange={() => setFillMode("people_only")} />
              <span><strong>{activity.participationKinds?.includes("external_agent") ? "People and their agents only" : "People only"}</strong><small>You invite every required participant.</small></span>
            </label>
          </div>
        </fieldset> : null}

        <div className="terms-card">
          <strong>Before you join</strong>
          <p>{activity.resultPublication} {activity.attribution}</p>
          {chosenFillMode === "house_agents" && activity.houseTerms !== null ? (
            <p>
              {activity.houseTerms.includedAtNoCharge ? "These platform-supplied companions are included at no charge for this experiment. " : ""}
              Runs with platform-supplied companions are permanently marked as unranked exhibitions. Each House Agent has at most {activity.houseTerms.maximumCallsPerAgent} model calls, {activity.houseTerms.maximumInputTokensPerAgent.toLocaleString()} input tokens, and {activity.houseTerms.maximumOutputTokensPerAgent.toLocaleString()} output tokens. Each launch starts fresh; a familiar companion name does not carry memory from another Run.
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
            {busy ? "Creating one room…" : session.state === "unavailable" ? "Sign-in service unavailable" : solo ? "Prepare solo activity" : "Create waiting room"}
          </button>
        )}
        {error !== null ? <p className="form-error" role="alert">{error}</p> : null}
      </section>
    </dialog>
  );
}

function participationLabel(activity: HostedActivitySummary): string {
  if (activity.houseFillAvailable) return "Optional House Agents";
  if (activity.seats.length === 1 && activity.participationKinds?.length === 1
      && activity.participationKinds[0] === "human") return "Solo adventure";
  if (activity.participationKinds?.includes("external_agent")) return "People and external agents";
  if (activity.participationKinds?.includes("human")) return "Play with people";
  return "Participation options under review";
}

export function allowsFreshLaunchRetry(state: string): boolean {
  return [
    "cancelled",
    "expired",
    "failed_pre_genesis",
    "abandoned_prestart",
    "closed_by_creator",
  ].includes(state);
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
  if (code === "formation_unavailable") return "This setup could not continue. Retry it, or close it from My games and start again.";
  if (code === "launch_close_unavailable") return "This activity is still closing. Retry Finish closing; the operation is safe to repeat.";
  if (code === "activity_capacity_unavailable") return "Active activity capacity is in use. Open My games to continue or close the existing activity.";
  if (code === "temporarily_unavailable") return "The live room service is temporarily unavailable.";
  return "The request did not complete. Retry this setup, or close it from My games and start again.";
}
