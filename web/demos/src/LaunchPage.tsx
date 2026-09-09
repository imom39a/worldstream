import { useCallback, useEffect, useState } from "react";

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

export function LaunchPage({ launchId, onNavigate }: { launchId: string; onNavigate: Navigate }) {
  const { session } = usePlatformSession();
  const [launch, setLaunch] = useState<HostedLaunch | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "unavailable">("loading");
  const [busy, setBusy] = useState<string | null>(null);
  const [startRequested, setStartRequested] = useState(false);
  const [invitation, setInvitation] = useState<{ seat: string; url: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (session.state !== "authenticated") return;
    try {
      const value = await readLaunch(launchId);
      setLaunch(value);
      setState("ready");
    } catch (cause) {
      setError(friendlyError(cause));
      setState("unavailable");
    }
  }, [launchId, session.state]);

  useEffect(() => {
    if (session.state !== "authenticated" || (launch !== null && isTerminalLaunchState(launch.state))) {
      return undefined;
    }
    if (launch === null) void refresh();
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => window.clearInterval(timer);
  }, [launch?.state, refresh, session.state]);

  useEffect(() => {
    if (
      !startRequested ||
      session.state !== "authenticated" ||
      launch === null ||
      isTerminalLaunchState(launch.state) ||
      busy === "start"
    ) return undefined;
    const delay = Math.max(1, launch.retry_after_seconds ?? 1) * 1_000;
    const timer = window.setTimeout(() => {
      setBusy("start");
      void launchMutation(session.csrf, launchId, "start")
        .then((value) => {
          if ("version" in value) setLaunch(value);
          if ("version" in value && value.state === "run_created") setStartRequested(false);
          setBusy(null);
        })
        .catch((cause) => {
          setError(friendlyError(cause));
          setBusy(null);
          setStartRequested(false);
        });
    }, launch.house_fill?.state === "claim_window_open" ? delay : 50);
    return () => window.clearTimeout(timer);
  }, [busy, launch, launchId, session, startRequested]);

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
    setStartRequested(true);
  };

  if (session.state === "guest") {
    return <MessagePage onNavigate={onNavigate} title="Sign in to open this waiting room" actionLabel="Continue with GitHub" onAction={() => githubSignIn("/")} />;
  }
  if (session.state === "loading" || state === "loading") {
    return <MessagePage onNavigate={onNavigate} title="Opening the waiting room…" />;
  }
  if (session.state === "unavailable" || state === "unavailable" || launch === null) {
    return <MessagePage onNavigate={onNavigate} title="This waiting room is not available" detail="The room service did not supply a safe fallback." />;
  }

  const terminal = isTerminalLaunchState(launch.state);
  const houseFailure = houseFillFailureDetail(launch.house_fill?.failure_code ?? null);
  return (
    <div className="site-shell hosted-shell">
      <SiteHeader onNavigate={onNavigate} />
      <main className="waiting-main">
        <section className="waiting-heading">
          <span className="eyebrow">{launch.activity_title} · waiting room</span>
          <div>
            <h1>{stateTitle(launch.state)}</h1>
            <span className={`formation-state state-${launch.state}`}><i />{stateLabel(launch.state)}</span>
          </div>
          <p>{stateDetail(launch)}</p>
          {houseFailure === null ? null : <p className="form-error" role="status">{houseFailure}</p>}
        </section>

        <section className="roster-card" aria-labelledby="roster-title">
          <div className="roster-heading">
            <div><span className="section-label">Seats</span><h2 id="roster-title">Room roster</h2></div>
            <span>{launch.fill_mode === "house_agents" ? "House fill enabled" : "People and external agents"}</span>
          </div>
          <div className="roster-list">
            {launch.seats.map((seat) => (
              <article key={seat.seat_key} className={`roster-seat seat-${seat.status}`}>
                <div className="seat-icon" aria-hidden="true">{seat.label.slice(0, 1)}</div>
                <div><strong>{seat.label}</strong><span>{seat.required ? "Required" : "Optional"} · {seatStatus(seat)}</span></div>
                <div className="seat-actions">
                  {seat.status === "open" && launch.can_manage && launch.state === "collecting" ? (
                    <button type="button" disabled={busy !== null} onClick={() => void seatAction(seat, "invitation")}>Copy invite</button>
                  ) : null}
                  {seat.status === "yours" && launch.state === "collecting" && !launch.can_manage ? (
                    <button type="button" disabled={busy !== null} onClick={() => void seatAction(seat, "release")}>Leave seat</button>
                  ) : null}
                  {seat.status === "claimed" && launch.can_manage && launch.state === "collecting" ? (
                    <button type="button" disabled={busy !== null} onClick={() => void seatAction(seat, "reset")}>Release seat</button>
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

        <section className="waiting-actions">
          {launch.run !== null && launch.run.can_enter ? (
            <div className="entry-actions">
              {launch.run.entries.filter((entry) => entry.entry_selector !== null).map((entry) => (
                <button key={entry.entry_selector} type="button" onClick={() => {
                  if (session.state !== "authenticated" || entry.entry_selector === null) return;
                  setBusy("enter");
                  void enterRun(session.csrf, launch.run!.run_id, entry.entry_selector)
                    .then((url) => window.location.assign(url))
                    .catch((cause) => {
                      setError(friendlyError(cause));
                      setBusy(null);
                    });
                }}>{busy === "enter" ? "Opening activity…" : `Enter ${entry.label}`}</button>
              ))}
            </div>
          ) : launch.can_manage && !terminal ? (
            <>
              <button className="start-room" type="button" disabled={startRequested || busy !== null} onClick={start}>
                {startRequested ? "Starting safely…" : "Start activity"}
              </button>
              <button className="text-button" type="button" disabled={busy !== null} onClick={() => {
                if (session.state !== "authenticated") return;
                setBusy("cancel");
                void launchMutation(session.csrf, launchId, "cancel")
                  .then(() => onNavigate("/"))
                  .catch((cause) => {
                    setError(friendlyError(cause));
                    setBusy(null);
                  });
              }}>Abandon room</button>
            </>
          ) : (
            <p>{terminal ? "This room ended before the activity started." : "The creator will start the activity."}</p>
          )}
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
  return ["cancelled", "expired", "failed_pre_genesis", "abandoned_prestart"].includes(state);
}

export function stateTitle(state: HostedLaunch["state"]): string {
  if (state === "run_created") return "Your activity is ready";
  if (state === "provisioning" || state === "reconciling") return "Building the live room";
  if (isTerminalLaunchState(state)) return "This room did not start";
  return "Gather your crew";
}

function stateLabel(state: HostedLaunch["state"]): string {
  return state.replaceAll("_", " ");
}

export function stateDetail(launch: HostedLaunch): string {
  if (launch.state === "run_created") return "WorldStream recorded Genesis. Enter the activity's standalone client.";
  if (isTerminalLaunchState(launch.state)) {
    return "This setup ended before Genesis. It is retained in your activity history, but no Room was created.";
  }
  if (launch.recovery_state === "genesis_recorded_repairing") {
    return "Genesis was recorded. WorldStream is restoring entry to this same room.";
  }
  if (launch.recovery_state === "genesis_not_proven") {
    return "WorldStream is checking the original setup. It will not create a replacement room.";
  }
  if (launch.state === "provisioning") return "The exact roster is frozen. The Host is creating one room.";
  if (launch.state === "reconciling") return "The Host is ready. The platform is confirming the exact Run.";
  if (launch.house_fill?.state === "claim_window_open") return "People have 30 seconds to claim open seats before reviewed House Agents fill them.";
  return "Share seat invitations. A person can join directly or control an external agent.";
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
