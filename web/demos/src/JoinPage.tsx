import { useEffect, useState } from "react";

import {
  claimInvitation,
  developmentSignIn,
  githubSignIn,
  usePlatformSession,
} from "./hostedApi";
import { friendlyError } from "./CatalogPage";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

const PENDING_INVITE_KEY = "worldstream.pending-invitation.v1";

export function JoinPage({ onNavigate }: { onNavigate: Navigate }) {
  const [token] = useState(takeInvitationToken);
  const [participation, setParticipation] = useState<"human" | "external_agent">("human");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { session, developmentSignInAvailable, reload } = usePlatformSession();

  useEffect(() => {
    if (token !== null) window.sessionStorage.setItem(PENDING_INVITE_KEY, token);
  }, [token]);

  const retainedToken = token ?? window.sessionStorage.getItem(PENDING_INVITE_KEY);
  const claim = async () => {
    if (session.state !== "authenticated" || retainedToken === null) return;
    setBusy(true);
    setError(null);
    try {
      const launch = await claimInvitation(session.csrf, retainedToken, participation);
      window.sessionStorage.removeItem(PENDING_INVITE_KEY);
      onNavigate(`/launches/${launch.launch_id}`);
    } catch (cause) {
      setError(friendlyError(cause));
      setBusy(false);
    }
  };

  return (
    <div className="site-shell hosted-shell">
      <SiteHeader onNavigate={onNavigate} />
      <main className="join-main">
        <section className="join-card">
          <span className="eyebrow">Seat invitation</span>
          <h1>Join a live activity</h1>
          {retainedToken === null ? (
            <p className="form-error" role="alert">This invitation is missing or malformed. Ask the room creator for a new link.</p>
          ) : (
            <>
              <p>Choose who will use this seat. The choice is fixed when you claim it.</p>
              <div className="choice-stack join-choices">
                <label className={participation === "human" ? "choice-selected" : ""}>
                  <input type="radio" checked={participation === "human"} onChange={() => setParticipation("human")} />
                  <span><strong>I will participate</strong><small>Use the live Activity Client yourself.</small></span>
                </label>
                <label className={participation === "external_agent" ? "choice-selected" : ""}>
                  <input type="radio" checked={participation === "external_agent"} onChange={() => setParticipation("external_agent")} />
                  <span><strong>My external agent will participate</strong><small>The agent is controlled by your account and is shown as unverified.</small></span>
                </label>
              </div>
              <div className="terms-card">
                <strong>Public activity notice</strong>
                <p>The live public view can be shared. A Replay-verified summary can be published with reviewed seat names. Private room authority is never placed in the link.</p>
              </div>
              {session.state === "guest" ? (
                <div className="sign-in-actions">
                  <button type="button" onClick={() => githubSignIn("/join")}>Continue with GitHub</button>
                  {developmentSignInAvailable ? <button className="secondary-button" type="button" onClick={() => void developmentSignIn().then(reload)}>Use visible local-development sign-in</button> : null}
                </div>
              ) : (
                <button className="primary-launch" type="button" disabled={busy || session.state !== "authenticated"} onClick={() => void claim()}>
                  {busy ? "Claiming seat…" : "Claim seat"}
                </button>
              )}
              {error !== null ? <p className="form-error" role="alert">{error}</p> : null}
            </>
          )}
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}

export function takeInvitationToken(): string | null {
  const match = window.location.hash.match(/^#invite=([0-9a-f]{64})$/u);
  window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
  return match?.[1] ?? window.sessionStorage.getItem(PENDING_INVITE_KEY);
}
