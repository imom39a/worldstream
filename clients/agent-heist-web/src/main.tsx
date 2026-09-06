import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import {
  ActivityClientHandoffClient,
  HostedLiveSessionController,
  selectActivityClientStartup,
} from "@worldstream/client";

import { AgentHeistClient } from "./AgentHeistClient";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");

const startup = selectActivityClientStartup(window);

void platformCsrf().then((csrf) => {
  const origin = window.location.origin;
  const client = new ActivityClientHandoffClient(origin, undefined, {
    browserOrigin: origin,
    csrf,
  });
  const controller = new HostedLiveSessionController({
    authority: client,
    streamUrl:
      import.meta.env.VITE_WORLDSTREAM_STREAM_URL ??
      `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/v1/hosted/browser-stream`,
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
      <h1>Sign-in required</h1>
      <p>Return to the activity catalog, sign in, and enter your Run again.</p>
    </main>,
  );
});

async function platformCsrf(): Promise<string> {
  const response = await fetch("/api/auth/session", {
    credentials: "include",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  const value: unknown = await response.json();
  if (
    !response.ok ||
    typeof value !== "object" ||
    value === null ||
    !("authenticated" in value) ||
    value.authenticated !== true ||
    !("csrf" in value) ||
    typeof value.csrf !== "string" ||
    value.csrf.length < 43 ||
    value.csrf.length > 128 ||
    !/^[A-Za-z0-9_-]+$/u.test(value.csrf)
  ) {
    throw new Error("platform_session_required");
  }
  return value.csrf;
}
