import { useCallback, useEffect, useState } from "react";

import { githubSignIn, readMyGames, usePlatformSession, type MyGamesIndex } from "./hostedApi";
import { SiteFooter, SiteHeader, type Navigate } from "./siteChrome";

export function MyGamesPage({ onNavigate }: { onNavigate: Navigate }) {
  const { session } = usePlatformSession();
  const [index, setIndex] = useState<MyGamesIndex | null>(null);
  const [older, setOlder] = useState<MyGamesIndex["items"]>([]);
  const [state, setState] = useState<"idle" | "loading" | "unavailable">("idle");

  const refresh = useCallback(async (initial: boolean) => {
    if (session.state !== "authenticated") return;
    if (initial) setState("loading");
    try {
      const value = await readMyGames();
      setIndex(value);
      if (initial) setOlder([]);
      setState("idle");
    } catch {
      setState("unavailable");
    }
  }, [session.state]);

  useEffect(() => {
    if (session.state !== "authenticated") return;
    void refresh(true);
    // A bounded private index read is enough to move an open history page from
    // publication pending to a verified result. It does not poll Room state.
    const timer = window.setInterval(() => void refresh(false), 5_000);
    return () => window.clearInterval(timer);
  }, [refresh, session.state]);

  return <div className="site-shell hosted-shell">
    <a className="skip-link" href="#main-content">Skip to content</a>
    <SiteHeader onNavigate={onNavigate} />
    <main id="main-content" className="my-games-page">
      <span className="eyebrow">Your activity history</span>
      <h1>My games</h1>
      {session.state === "loading" ? <p role="status">Checking sign-in…</p> : null}
      {session.state === "guest" ? <section className="catalog-notice"><p>Sign in to find activities you started or joined.</p><button type="button" onClick={() => githubSignIn("/my-games")}>Sign in with GitHub</button></section> : null}
      {session.state === "unavailable" || state === "unavailable" ? <p role="status">My games is temporarily unavailable.</p> : null}
      {state === "loading" ? <p role="status">Finding your activities…</p> : null}
      {index !== null && index.items.length === 0 ? <p>No activities yet. Choose one from Discover when you are ready.</p> : null}
      {index !== null ? <ul className="my-games-list">
        {[...index.items, ...older].map((item) => <li key={item.launch_id}>
          <article
            data-launch-id={item.launch_id}
            data-result-public-id={item.result_public_id}
          >
            <span className="section-label">{historyStatusLabel(item.state)}</span>
            <h2>{item.title}</h2>
            <p>{historyStatusDetail(item.state, item.participation)}</p>
            {item.action === "view_result" && item.result_public_id !== undefined
              ? <button type="button" onClick={() => onNavigate(`/runs/${item.result_public_id}`)}>View result</button>
              : item.action === "continue_setup"
                ? <button type="button" onClick={() => onNavigate(`/launches/${item.launch_id}`)}>Continue setup</button>
                : item.action === "return_to_game"
                  ? <button type="button" onClick={() => onNavigate(`/launches/${item.launch_id}`)}>Return to game</button>
                  : <span>History retained</span>}
          </article>
        </li>)}
      </ul> : null}
      {index?.next !== null && index?.next !== undefined ? <button type="button" onClick={() => {
        void readMyGames(index.next).then((value) => {
          setOlder((items) => [...items, ...value.items]);
          setIndex((current) => current === null ? current : { ...current, next: value.next });
        }).catch(() => setState("unavailable"));
      }}>Show older activities</button> : null}
    </main>
    <SiteFooter />
  </div>;
}

export function historyStatusLabel(state: MyGamesIndex["items"][number]["state"]): string {
  return state.replaceAll("_", " ");
}

export function historyStatusDetail(
  state: MyGamesIndex["items"][number]["state"],
  participation: MyGamesIndex["items"][number]["participation"],
): string {
  if (state === "publication_pending") return "The activity ended. WorldStream is verifying its reviewed result.";
  if (state === "terminal_private") return "The activity ended. Return to the activity to read your private debrief.";
  if (state === "terminal_without_outcome") return "The activity ended without an outcome. No result will be published.";
  if (state === "result_suppressed") return "A result exists but is not available to display after integrity or privacy review.";
  if (state === "dependency_failure") return "WorldStream could not verify the activity status from retained evidence.";
  if (state === "setup_cancelled" || state === "setup_abandoned" || state === "setup_failed") return "This setup ended before a Room was created.";
  if (state === "verified_result") return "This Replay-verified result is ready to view.";
  if (state === "live") return participation === "external_agent" ? "Your external agent is participating." : "You are participating.";
  return participation === "external_agent" ? "Your external agent is waiting to participate." : "You are waiting to participate.";
}
