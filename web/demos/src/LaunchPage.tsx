import { useEffect, useState } from "react";

import {
  enterRun,
  githubSignIn,
  launchMutation,
  readLaunch,
  seatMutation,
  usePlatformSession,
  type HostedLaunch,
  type HostedLaunchSeat,
} from "./hostedApi";
import { friendlyError } from "./CatalogPage";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export const START_RETRY_MAX_ATTEMPTS = 20;
const START_RETRY_MAX_DURATION_MS = 60_000;
const START_RETRY_MIN_DELAY_MS = 2_000;

export function LaunchPage({ launchId, onNavigate }: { launchId: string; onNavigate: Navigate }) {
  const { session } = usePlatformSession();
  const [launch, setLaunch] = useState<HostedLaunch | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "unavailable">("loading");
  const [busy, setBusy] = useState<string | null>(null);
  const [startRequested, setStartRequested] = useState(false);
  const [readinessSyncRequested, setReadinessSyncRequested] = useState(false);
  const [readinessSyncUrl, setReadinessSyncUrl] = useState<string | null>(null);
  const [startRetryCount, setStartRetryCount] = useState(0);
  const [startRetryStartedAt, setStartRetryStartedAt] = useState<number | null>(null);
  const [countdownNow, setCountdownNow] = useState(() => Date.now());
  const [invitation, setInvitation] = useState<{ seat: string; url: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshAttempt, setRefreshAttempt] = useState(0);
  const authenticatedCsrf = session.state === "authenticated" ? session.csrf : null;
  const launchState = launch?.state ?? null;
  const recoveryState = launch?.recovery_state ?? null;
  const retryAfterSeconds = launch?.retry_after_seconds ?? null;
  const houseFillState = launch?.house_fill?.state ?? null;
  const claimWindowClosesAt = houseFillState === "claim_window_open"
    ? launch?.house_fill?.claim_window_closes_at ?? null
    : null;

  useEffect(() => {
    setLaunch(null);
    setState("loading");
    setError(null);
    setInvitation(null);
    setStartRequested(false);
    setReadinessSyncRequested(false);
    setReadinessSyncUrl(null);
    setStartRetryCount(0);
    setStartRetryStartedAt(null);
  }, [launchId, authenticatedCsrf]);

  useEffect(() => {
    if (authenticatedCsrf === null || busy !== null) return undefined;
    let disposed = false;
    let timer: number | undefined;
    const controller = new AbortController();
    const refresh = async () => {
      let delay = 2_000;
      try {
        const value = await readLaunch(launchId, controller.signal);
        if (disposed) return;
        setLaunch(value);
        if (value.state === "run_created" && value.recovery_state === "entry_ready") {
          setStartRequested(false);
          setReadinessSyncRequested(false);
          setReadinessSyncUrl(null);
          setStartRetryCount(0);
          setStartRetryStartedAt(null);
        }
        setState("ready");
        if (isTerminalLaunchState(value.state)) return;
      } catch (cause) {
        if (disposed) return;
        setError(friendlyError(cause));
        setState("unavailable");
        delay = 5_000;
      }
      if (!disposed) timer = window.setTimeout(() => void refresh(), delay);
    };
    void refresh();
    return () => {
      disposed = true;
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [launchId, authenticatedCsrf, busy, refreshAttempt]);

  useEffect(() => {
    if (
      !startRequested ||
      authenticatedCsrf === null ||
      launchState === null ||
      isTerminalLaunchState(launchState) ||
      (recoveryState === "waiting_for_readiness" && !readinessSyncRequested) ||
      busy === "start"
    ) return undefined;
    const boundedStartRetry = recoveryState === "retrying_start" ||
      (recoveryState === "waiting_for_readiness" && readinessSyncRequested) ||
      (retryAfterSeconds !== null && houseFillState !== "claim_window_open");
    const exhaustedMessage = recoveryState === "waiting_for_readiness"
      ? "The Host is still waiting for browser readiness. Start activity again to retry this same room."
      : "The Host is still completing Activity Start. Start activity again to retry this same room.";
    if (boundedStartRetry && startRetryCount >= START_RETRY_MAX_ATTEMPTS) {
      setStartRequested(false);
      if (recoveryState === "waiting_for_readiness") setReadinessSyncRequested(false);
      setError(exhaustedMessage);
      return undefined;
    }
    if (
      boundedStartRetry &&
      startRetryStartedAt !== null &&
      Date.now() - startRetryStartedAt >= START_RETRY_MAX_DURATION_MS
    ) {
      setStartRequested(false);
      if (recoveryState === "waiting_for_readiness") setReadinessSyncRequested(false);
      setError(exhaustedMessage);
      return undefined;
    }
    const claimDeadline = claimWindowClosesAt === null ? Number.NaN : Date.parse(claimWindowClosesAt);
    const untilClaimDeadline = claimDeadline - Date.now();
    const retryAfterMilliseconds = typeof retryAfterSeconds === "number"
      && Number.isFinite(retryAfterSeconds)
      && retryAfterSeconds > 0
      ? retryAfterSeconds * 1_000
      : START_RETRY_MIN_DELAY_MS;
    const delay = recoveryState === "waiting_for_readiness"
      ? Math.max(START_RETRY_MIN_DELAY_MS, retryAfterMilliseconds)
      : houseFillState === "claim_window_open"
      ? (Number.isFinite(untilClaimDeadline) && untilClaimDeadline > 0 ? untilClaimDeadline : 1_000)
      : retryAfterSeconds === null ? 50 : retryAfterMilliseconds;
    const timer = window.setTimeout(() => {
      if (
        boundedStartRetry && (
        startRetryCount >= START_RETRY_MAX_ATTEMPTS ||
        (startRetryStartedAt !== null && Date.now() - startRetryStartedAt >= START_RETRY_MAX_DURATION_MS)
        )
      ) {
        setStartRequested(false);
        if (recoveryState === "waiting_for_readiness") setReadinessSyncRequested(false);
        setError(exhaustedMessage);
        return;
      }
      setBusy("start");
      void launchMutation(authenticatedCsrf, launchId, "start")
        .then((value) => {
          if ("version" in value) setLaunch(value);
          if (
            "version" in value &&
            value.state === "run_created" &&
            value.recovery_state === "entry_ready"
          ) {
            setStartRequested(false);
            setReadinessSyncRequested(false);
            setReadinessSyncUrl(null);
            setStartRetryCount(0);
            setStartRetryStartedAt(null);
          } else if ("version" in value &&
            (isTerminalLaunchState(value.state) || value.recovery_state === "host_attention")) {
            setStartRequested(false);
            setStartRetryCount(0);
            setStartRetryStartedAt(null);
          } else if ("version" in value) {
            // Force a new retry cycle even when the retained DTO's state is
            // unchanged (for example, while waiting for a fresh browser ack).
            const nextBoundedRetry = value.recovery_state === "retrying_start" ||
              (value.recovery_state === "waiting_for_readiness" && readinessSyncRequested) ||
              (value.retry_after_seconds != null && value.house_fill?.state !== "claim_window_open");
            if (nextBoundedRetry) {
              if (startRetryStartedAt === null) setStartRetryStartedAt(Date.now());
              const nextAttempt = startRetryCount + 1;
              setStartRetryCount(nextAttempt);
              if (nextAttempt >= START_RETRY_MAX_ATTEMPTS) {
                setStartRequested(false);
                if (value.recovery_state === "waiting_for_readiness") {
                  setReadinessSyncRequested(false);
                }
                setError(value.recovery_state === "waiting_for_readiness"
                  ? "The Host is still waiting for browser readiness. Start activity again to retry this same room."
                  : "The Host is still completing Activity Start. Start activity again to retry this same room.");
              }
            } else {
              setStartRetryCount(0);
              setStartRetryStartedAt(null);
            }
          }
          setBusy(null);
        })
        .catch((cause) => {
          setError(friendlyError(cause));
          setBusy(null);
          setStartRequested(false);
        });
    }, delay);
    return () => window.clearTimeout(timer);
  }, [authenticatedCsrf, busy, claimWindowClosesAt, houseFillState, launchId, launchState, readinessSyncRequested, recoveryState, retryAfterSeconds, startRequested, startRetryCount, startRetryStartedAt]);

  useEffect(() => {
    if (claimWindowClosesAt === null) return undefined;
    const timer = window.setInterval(() => setCountdownNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [claimWindowClosesAt]);

  const seatAction = async (seat: HostedLaunchSeat, action: "invitation" | "release" | "reset") => {
    if (session.state !== "authenticated") return;
    setBusy(`${action}:${seat.seat_key}`);
    setError(null);
    try {
      const value = await seatMutation(session.csrf, launchId, seat.seat_key, action);
      if ("invitation_token" in value) {
        const url = `${window.location.origin}/join#invite=${value.invitation_token}`;
        setInvitation({ seat: seat.label, url });
        await navigator.clipboard?.writeText(url).catch(() => undefined);
      } else {
        setLaunch(value);
      }
    } catch (cause) {
      setError(friendlyError(cause));
    } finally {
      setBusy(null);
    }
  };

  const start = () => {
    setError(null);
    setStartRetryCount(0);
    setStartRetryStartedAt(
      recoveryState === "retrying_start" ||
      (retryAfterSeconds !== null && houseFillState !== "claim_window_open")
        ? Date.now()
        : null,
    );
    setStartRequested(true);
  };

  if (session.state === "guest") {
    return <MessagePage onNavigate={onNavigate} title="Sign in to open this waiting room" actionLabel="Continue with GitHub" onAction={() => githubSignIn("/")} />;
  }
  if (session.state === "loading" || state === "loading") {
    return <MessagePage onNavigate={onNavigate} title="Opening the waiting room…" />;
  }
  if (session.state === "unavailable" || state === "unavailable" || launch === null) {
    return <MessagePage onNavigate={onNavigate} title="This waiting room is not available"
      detail="The connection could not be confirmed. Retry to check this same room."
      actionLabel={session.state === "authenticated" ? "Retry connection" : undefined}
      onAction={() => { setState("loading"); setRefreshAttempt((attempt) => attempt + 1); }} />;
  }

  const terminal = isTerminalLaunchState(launch.state);
  const setupLocked = startRequested || busy !== null;
  const actions = launch.available_actions ?? [];
  const mayStart = actions.includes("start");
  const waitingForReadiness = launch.recovery_state === "waiting_for_readiness";
  const closeAction = actions.find((action) => action !== "start") ?? null;
  const claimSecondsRemaining = claimWindowClosesAt === null
    ? null
    : secondsUntil(claimWindowClosesAt, countdownNow);
  const houseFailure = houseFillFailureDetail(launch.house_fill?.failure_code ?? null);
  return (
    <div className="site-shell hosted-shell">
      <SiteHeader onNavigate={onNavigate} />
      <main className="waiting-main">
        <section className="waiting-heading">
          <span className="eyebrow">{launch.activity_title} · waiting room</span>
          <div>
            <h1>{launch.state === "collecting" && launch.seats.length === 1 ? "Ready to begin" : stateTitle(launch.state)}</h1>
            <span className={`formation-state state-${launch.state}`}><i />{stateLabel(launch.state)}</span>
          </div>
          <p>{stateDetail(launch)}</p>
          {claimSecondsRemaining === null ? null : (
            <div className="claim-countdown" role="timer" aria-live="off">
              <strong>{claimCountdownLabel(claimSecondsRemaining)}</strong>
              <span>People can still claim open seats.</span>
            </div>
          )}
          {houseFailure === null ? null : <p className="form-error" role="status">{houseFailure}</p>}
        </section>

        <section className="roster-card" aria-labelledby="roster-title">
          <div className="roster-heading">
            <div><span className="section-label">Seats</span><h2 id="roster-title">Room roster</h2></div>
            <span>{launch.fill_mode === "house_agents" ? "House fill enabled" : launch.seats.length === 1 ? "Solo activity" : "Invited participants"}</span>
          </div>
          <div className="roster-list">
            {launch.seats.map((seat) => (
              <article key={seat.seat_key} className={`roster-seat seat-${seat.status}`}>
                <div className="seat-icon" aria-hidden="true">{seat.label.slice(0, 1)}</div>
                <div><strong>{seat.label}</strong><span>{seat.required ? "Required" : "Optional"} · {seatStatus(seat)}</span></div>
                <div className="seat-actions">
                  {seat.status === "open" && launch.can_manage && launch.state === "collecting" ? (
                    <button type="button" disabled={setupLocked} onClick={() => void seatAction(seat, "invitation")}>Copy invite</button>
                  ) : null}
                  {seat.status === "yours" && launch.state === "collecting" && !launch.can_manage ? (
                    <button type="button" disabled={setupLocked} onClick={() => void seatAction(seat, "release")}>Leave seat</button>
                  ) : null}
                  {seat.status === "claimed" && launch.can_manage && launch.state === "collecting" ? (
                    <button type="button" disabled={setupLocked} onClick={() => void seatAction(seat, "reset")}>Release seat</button>
                  ) : null}
                </div>
              </article>
            ))}
          </div>
          {invitation !== null ? (
            <div className="invite-result" role="status">
              <strong>{invitation.seat} invite copied</strong>
              <input readOnly aria-label={`${invitation.seat} invitation URL`} value={invitation.url} onFocus={(event) => event.currentTarget.select()} />
            </div>
          ) : null}
        </section>

        <section className="waiting-actions" aria-busy={startRequested}>
          {launch.run !== null && launch.run.can_enter ? (
            <div className="entry-actions">
              {launch.run.entries.filter((entry) => entry.entry_selector !== null).map((entry) => (
                <button key={entry.entry_selector} type="button" disabled={
                  busy !== null || (waitingForReadiness && readinessSyncRequested)
                } onClick={() => {
                  if (session.state !== "authenticated" || entry.entry_selector === null) return;
                  setBusy("enter");
                  void openRunEntry({
                    csrf: session.csrf,
                    runId: launch.run!.run_id,
                    entrySelector: entry.entry_selector,
                    preserveWaitingRoom: waitingForReadiness,
                  })
                    .then(({ url, openedInNewTab }) => {
                      if (waitingForReadiness) {
                        setReadinessSyncUrl(openedInNewTab ? null : url);
                        setReadinessSyncRequested(openedInNewTab);
                        if (openedInNewTab) {
                          setStartRetryCount(0);
                          setStartRetryStartedAt(Date.now());
                          setStartRequested(true);
                        } else {
                          setError("Open the activity link below once to sync this browser. This room will retry Start automatically.");
                        }
                      } else {
                        window.location.assign(url);
                      }
                    })
                    .catch((cause) => {
                      setError(friendlyError(cause));
                    })
                    .finally(() => setBusy(null));
                }}>{busy === "enter" ? "Opening activity…" : waitingForReadiness ? `Open ${entry.label} to sync` : `Enter ${entry.label}`}</button>
              ))}
            </div>
          ) : null}
          {waitingForReadiness && readinessSyncUrl !== null ? (
            <p className="form-help" role="status">
              <a
                href={readinessSyncUrl}
                target="_blank"
                rel="noopener noreferrer"
                onClick={() => {
                  setError(null);
                  setReadinessSyncUrl(null);
                  setReadinessSyncRequested(true);
                  setStartRetryCount(0);
                  setStartRetryStartedAt(Date.now());
                  setStartRequested(true);
                }}
              >Open the activity to sync this browser</a>. The waiting room will retry the same Start request until Activity Start is committed.
            </p>
          ) : null}
          {mayStart ? (
            <button className="start-room" type="button" disabled={setupLocked} onClick={start}>
              {startRequested ? "Starting safely…" : "Start activity"}
            </button>
          ) : null}
          {closeAction === null ? null : (
            <button className="text-button" type="button" disabled={setupLocked} onClick={() => {
              if (session.state !== "authenticated") return;
              if (closeAction === "end_activity" && !window.confirm(
                "End this activity? Its Room will be archived and cannot be resumed.",
              )) return;
              setBusy("close");
              setError(null);
              void launchMutation(session.csrf, launchId, "close")
                .then(() => onNavigate("/"))
                .catch((cause) => {
                  setError(friendlyError(cause));
                  setBusy(null);
                });
            }}>{busy === "close" ? "Closing safely…" : closeActionLabel(closeAction)}</button>
          )}
          {launch.run?.can_enter !== true && !mayStart && closeAction === null ? (
            <p>{terminal ? "This activity is closed." : "The creator will start the activity."}</p>
          ) : null}
          {error !== null ? <p className="form-error" role="alert">{error}</p> : null}
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}

function MessagePage({ onNavigate, title, detail, actionLabel, onAction }: {
  onNavigate: Navigate;
  title: string;
  detail?: string;
  actionLabel?: string;
  onAction?: () => void;
}) {
  return <div className="site-shell hosted-shell"><SiteHeader onNavigate={onNavigate} /><main className="hosted-message"><span className="eyebrow">Hosted activity</span><h1>{title}</h1>{detail === undefined ? null : <p>{detail}</p>}{actionLabel === undefined ? null : <button type="button" onClick={onAction}>{actionLabel}</button>}</main><SiteFooter /></div>;
}

export function isTerminalLaunchState(state: HostedLaunch["state"]): boolean {
  return ["cancelled", "expired", "failed_pre_genesis", "abandoned_prestart", "closed_by_creator"].includes(state);
}

export function stateTitle(state: HostedLaunch["state"]): string {
  if (state === "run_created") return "Your activity is ready";
  if (state === "closing") return "Closing this activity";
  if (state === "closed_by_creator") return "This activity is closed";
  if (state === "provisioning" || state === "reconciling") return "Building the live room";
  if (isTerminalLaunchState(state)) return "This room did not start";
  return "Gather your crew";
}

function stateLabel(state: HostedLaunch["state"]): string {
  return state.replaceAll("_", " ");
}

export function stateDetail(launch: HostedLaunch): string {
  if (launch.state === "run_created") return "WorldStream recorded Genesis. Enter the activity's standalone client.";
  if (launch.state === "closing") {
    return "WorldStream is fencing this setup and archiving its Room if one exists. Retrying Finish closing is safe.";
  }
  if (launch.state === "closed_by_creator") {
    return "The launch cannot resume. If a Room was created, WorldStream archived it before releasing capacity.";
  }
  if (isTerminalLaunchState(launch.state)) {
    return "This setup ended before Genesis. It is retained in your activity history, but no Room was created.";
  }
  if (launch.recovery_state === "genesis_recorded_repairing") {
    return "Genesis was recorded. WorldStream is restoring entry to this same room.";
  }
  if (launch.recovery_state === "genesis_not_proven") {
    return "WorldStream is checking the original setup. It will not create a replacement room.";
  }
  if (launch.recovery_state === "waiting_for_readiness") {
    return "Open the activity once to sync this browser. The waiting room will retry the same Start request when the Host is ready.";
  }
  if (launch.recovery_state === "retrying_start") {
    return "The Host is completing Activity Start for this same room. Retrying Start is safe and will not create another room.";
  }
  if (launch.recovery_state === "host_attention") {
    return "The Host could not complete Activity Start. End this activity to release its retained room, then start a new one.";
  }
  if (launch.state === "provisioning") return "The exact roster is frozen. The Host is creating one room.";
  if (launch.state === "reconciling") return "The Host is ready. The platform is confirming the exact Run.";
  if (launch.house_fill?.state === "claim_window_open") return "People can claim open seats until the claim window ends. Reviewed House Agents then fill them.";
  if (launch.seats.length === 1) return "Your solo seat is ready. Start when you are ready to begin.";
  return "Share seat invitations. A person can join directly or control an external agent.";
}

function closeActionLabel(action: Exclude<HostedLaunch["available_actions"][number], "start">): string {
  if (action === "cancel_setup") return "Cancel setup";
  if (action === "stop_setup") return "Stop setup";
  if (action === "end_activity") return "End activity";
  return "Finish closing";
}

function secondsUntil(deadline: string, now: number): number {
  const deadlineMilliseconds = Date.parse(deadline);
  if (!Number.isFinite(deadlineMilliseconds)) return 0;
  return Math.max(0, Math.ceil((deadlineMilliseconds - now) / 1_000));
}

function claimCountdownLabel(seconds: number): string {
  if (seconds <= 0) return "House Agents are joining now…";
  return `House Agents join in ${seconds} ${seconds === 1 ? "second" : "seconds"}`;
}

/** Maps only reviewed server codes; unknown text is never rendered to users. */
export function houseFillFailureDetail(failureCode: string | null): string | null {
  switch (failureCode) {
    case null:
      return null;
    case "house_runner_capacity_exhausted":
      return "House Agents are busy. Invite a person or external agent, or try again later.";
    case "house_runner_assignment_conflict":
      return "A House Agent could not take this seat. Invite a person or external agent instead.";
    case "house_credential_unavailable":
    case "house_profile_unavailable":
    case "house_runner_template_unavailable":
      return "House Agents are unavailable right now. Invite a person or external agent instead.";
    case "house_profile_incompatible":
      return "House Agents cannot join this activity. Invite a person or external agent instead.";
    default:
      return "A reviewed House Agent could not join. Invite a person or external agent instead.";
  }
}

function seatStatus(seat: HostedLaunchSeat): string {
  if (seat.status === "open") return "Open";
  if (seat.status === "house") return `House Agent · ${seat.house_display_name ?? "Reviewed agent"}`;
  const kind = seat.participation === "external_agent" ? "External agent" : "Person";
  return seat.status === "yours" ? `Your seat · ${kind}` : `Claimed · ${kind}`;
}

async function openRunEntry(input: {
  readonly csrf: string;
  readonly runId: string;
  readonly entrySelector: string;
  readonly preserveWaitingRoom: boolean;
}): Promise<{ readonly url: string; readonly openedInNewTab: boolean }> {
  // Reserve a tab synchronously while this click still has browser activation. The
  // handoff itself is issued by the authenticated endpoint after the reservation.
  // If a browser blocks the tab, the rendered link below remains usable on mobile.
  const reservedWindow = input.preserveWaitingRoom ? reserveWindow() : null;
  try {
    const url = await enterRun(input.csrf, input.runId, input.entrySelector);
    if (reservedWindow !== null) {
      try {
        reservedWindow.location.replace(url);
        return { url, openedInNewTab: true };
      } catch {
        reservedWindow.close();
      }
    }
    return { url, openedInNewTab: false };
  } catch (cause) {
    reservedWindow?.close();
    throw cause;
  }
}

function reserveWindow(): Window | null {
  try {
    const reserved = window.open("about:blank", "_blank");
    if (reserved === null) return null;
    try {
      reserved.opener = null;
    } catch {
      reserved.close();
      return null;
    }
    return reserved;
  } catch {
    return null;
  }
}
