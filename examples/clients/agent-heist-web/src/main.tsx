import { StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

import {
  ActivityClientHandoffClient,
  HostedLiveSessionController,
  PublicProjectionSessionController,
  selectActivityClientStartup,
} from "@worldstream/client";

import { AgentHeistClient } from "./AgentHeistClient";
import { readPlatformSession, type PlatformSessionConfiguration } from "./platformSession";
import {
  isSelectedPublicViewerLaunch,
  readPlatformReturnTarget,
  readPublicViewerContext,
} from "./publicViewer";
import "./styles.css";

const rootElement = document.getElementById("root") ?? missingRoot();

const startup = selectActivityClientStartup(window);
const publicViewer = readPublicViewerContext(window);
const platformReturnTarget = readPlatformReturnTarget(window);

void (publicViewer === null ? participantClient() : publicViewerClient(publicViewer)).catch(() => {
  createRoot(rootElement).render(
    <main className="heist-boundary-shell">
      <span className="eyebrow">Agent Heist Activity Client</span>
      <h1>Unable to enter this Run</h1>
      <p>Return to the activity catalog, sign in if needed, and enter your Run again.</p>
    </main>,
  );
});

async function participantClient(): Promise<void> {
  const { csrf, browserStreamUrl } = await platformSession();
  const origin = window.location.origin;
  const client = new ActivityClientHandoffClient(origin, undefined, {
    browserOrigin: origin,
    csrf,
  });
  const controller = new HostedLiveSessionController({
    authority: client,
    streamUrl: browserStreamUrl,
  });
  createRoot(rootElement).render(
    <StrictMode>
      <ActivityClientNavigation returnTarget={platformReturnTarget}>
        <AgentHeistClient startup={startup} controller={controller} />
      </ActivityClientNavigation>
    </StrictMode>,
  );
}

async function publicViewerClient(context: NonNullable<typeof publicViewer>): Promise<void> {
  const response = await fetch(`/api/runs/${context.publicId}`, {
    credentials: "omit",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  const value = await response.json() as Record<string, unknown>;
  const live = value.live;
  if (
    !response.ok || value.state !== "live" ||
    live === null || typeof live !== "object" ||
    typeof (live as Record<string, unknown>).stream_url !== "string" ||
    !isSelectedPublicViewerLaunch(window, (value.client as Record<string, unknown> | undefined)?.launch_url)
  ) throw new Error("public_viewer_unavailable");
  const controller = new PublicProjectionSessionController({
    streamUrl: (live as Record<string, string>).stream_url,
  });
  createRoot(rootElement).render(
    <StrictMode>
      <ActivityClientNavigation returnTarget={context.backToGames} resultPath={context.resultPath}>
        <AgentHeistClient startup={{ kind: "direct" }} controller={controller} />
      </ActivityClientNavigation>
    </StrictMode>,
  );
}

function ActivityClientNavigation({
  children,
  returnTarget,
  resultPath,
}: {
  readonly children: ReactNode;
  readonly returnTarget: "/";
  readonly resultPath?: string;
}) {
  return <>
    <nav className="activity-client-navigation" aria-label="Platform navigation">
      <button type="button" onClick={() => window.location.assign(returnTarget)}>Back to games</button>
      {resultPath === undefined ? null : <a href={resultPath}>View public result</a>}
    </nav>
    {children}
  </>;
}

function missingRoot(): never {
  throw new Error("missing #root mount point");
}

async function platformSession(): Promise<PlatformSessionConfiguration> {
  const response = await fetch("/api/auth/session", {
    credentials: "include",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  if (!response.ok) throw new Error("platform_session_configuration_unavailable");
  return readPlatformSession(await response.json());
}
