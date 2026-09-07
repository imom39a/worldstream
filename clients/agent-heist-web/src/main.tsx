import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import {
  ActivityClientHandoffClient,
  HostedLiveSessionController,
  selectActivityClientStartup,
} from "@worldstream/client";

import { AgentHeistClient } from "./AgentHeistClient";
import { readPlatformSession, type PlatformSessionConfiguration } from "./platformSession";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");

const startup = selectActivityClientStartup(window);

void platformSession().then(({ csrf, browserStreamUrl }) => {
  const origin = window.location.origin;
  const client = new ActivityClientHandoffClient(origin, undefined, {
    browserOrigin: origin,
    csrf,
  });
  const controller = new HostedLiveSessionController({
    authority: client,
    streamUrl: browserStreamUrl,
  });
  createRoot(root).render(
    <StrictMode>
      <AgentHeistClient startup={startup} controller={controller} />
    </StrictMode>,
  );
}).catch(() => {
  createRoot(root).render(
    <main className="heist-boundary-shell">
      <span className="eyebrow">Agent Heist Activity Client</span>
      <h1>Unable to enter this Run</h1>
      <p>Return to the activity catalog, sign in if needed, and enter your Run again.</p>
    </main>,
  );
});

async function platformSession(): Promise<PlatformSessionConfiguration> {
  const response = await fetch("/api/auth/session", {
    credentials: "include",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  if (!response.ok) throw new Error("platform_session_configuration_unavailable");
  return readPlatformSession(await response.json());
}
