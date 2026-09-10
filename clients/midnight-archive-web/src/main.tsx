import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import {
  ActivityClientHandoffClient,
  HostedLiveSessionController,
  selectActivityClientStartup,
} from "@worldstream/client";

import { MidnightArchiveClient } from "./MidnightArchiveClient";
import { readPlatformSession } from "./platformSession";
import { createMidnightArchiveRetryingFetch } from "./retryingFetch";
import "./styles.css";

const root = document.getElementById("root") ?? missingRoot();
const startup = selectActivityClientStartup(window);

void platformSession().then(({ csrf, browserStreamUrl }) => {
  const origin = window.location.origin;
  const authority = new ActivityClientHandoffClient(origin, createMidnightArchiveRetryingFetch(), {
    browserOrigin: origin,
    csrf,
  });
  const controller = new HostedLiveSessionController({ authority, streamUrl: browserStreamUrl });
  createRoot(root).render(
    <StrictMode>
      <PlatformNavigation>
        <MidnightArchiveClient startup={startup} controller={controller} />
      </PlatformNavigation>
    </StrictMode>,
  );
}).catch(() => {
  createRoot(root).render(
    <main className="archive-boundary">
      <p className="archive-kicker">Midnight Archive</p>
      <h1>Unable to enter this expedition</h1>
      <p>Return to the activity library, sign in if needed, and enter your Run again.</p>
    </main>,
  );
});

function PlatformNavigation({ children }: { readonly children: React.ReactNode }) {
  return <>
    <nav className="platform-navigation" aria-label="Platform navigation">
      <button type="button" onClick={() => window.location.assign("/")}>Back to activities</button>
    </nav>
    {children}
  </>;
}

async function platformSession() {
  const response = await fetch("/api/auth/session", {
    credentials: "include",
    cache: "no-store",
    headers: { Accept: "application/json" },
  });
  if (!response.ok) throw new Error("platform_session_configuration_unavailable");
  return readPlatformSession(await response.json());
}

function missingRoot(): never {
  throw new Error("missing #root mount point");
}
