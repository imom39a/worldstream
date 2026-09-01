import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { AgentHeistClient } from "@worldstream/agent-heist-client";
import "@worldstream/agent-heist-client/styles.css";
import {
  ActivityClientHandoffClient,
  resumeRetainedActivityClient,
  selectActivityClientStartup,
} from "@worldstream/client";

import { App } from "./App";
import { clientSurfaceForPath } from "./clientSurface";
import {
  CLIENT_CONTRACT_IDENTITY,
  CLIENT_CONTRACT_IDENTITY_JSON,
} from "./compatibilityIdentity";
import { consumeLiveSessionBootstrap } from "./liveSession";
import { scenarioFromSearch } from "./scenario";
import { StudioPrototype } from "./StudioPrototype";
import { HandedOffParticipant } from "./HandedOffParticipant";
import { NegotiateApp, NegotiateLiveApp } from "./NegotiateApp";
import {
  consumeNegotiateConsoleBootstrap,
  negotiateBootstrapMatchesLiveSession,
} from "./negotiate";
import "./styles.css";

const root = document.getElementById("root");

if (root === null) {
  throw new Error("missing #root mount point");
}

const isStudioPrototype = import.meta.env.DEV && window.location.pathname === "/prototype/studio";
const clientSurface = clientSurfaceForPath(window.location.pathname);

if (isStudioPrototype) {
  createRoot(root).render(
    <StrictMode>
      <StudioPrototype />
    </StrictMode>,
  );
} else {
  const identityWindow = window as Window & {
    __WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__?: typeof CLIENT_CONTRACT_IDENTITY;
  };
  identityWindow.__WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__ = CLIENT_CONTRACT_IDENTITY;
  document.documentElement.dataset.worldstreamClientContractIdentity =
    CLIENT_CONTRACT_IDENTITY_JSON;

  const client = new ActivityClientHandoffClient(
    import.meta.env.VITE_WORLDSTREAM_SUPERVISOR_URL ?? "http://127.0.0.1:9420",
  );
  const startup = selectActivityClientStartup(window);
  void resumeRetainedActivityClient(startup, client).then((resolvedStartup) => {
    if (clientSurface === "agent-heist") {
      createRoot(root).render(
        <StrictMode>
          <AgentHeistClient startup={resolvedStartup} client={client} />
        </StrictMode>,
      );
      return;
    }
    if (clientSurface === "inspector") {
      createRoot(root).render(
        <StrictMode>
          <HandedOffParticipant startup={resolvedStartup} client={client} />
        </StrictMode>,
      );
      return;
    }
    if (resolvedStartup.kind !== "direct") {
      createRoot(root).render(<StrictMode><HandedOffParticipant startup={resolvedStartup} client={client} /></StrictMode>);
      return;
    }
    const liveBootstrap = window as Window & { __WORLDSTREAM_LIVE_SESSION__?: unknown };
    const consumedLiveSession = consumeLiveSessionBootstrap(liveBootstrap);
    const negotiateBootstrap = consumeNegotiateConsoleBootstrap(
      window as Window & { __WORLDSTREAM_NEGOTIATE_CONSOLE__?: unknown },
    );
    if (negotiateBootstrap !== null) {
      const mismatchedLiveSession = consumedLiveSession.config !== null
        && !negotiateBootstrapMatchesLiveSession(
          negotiateBootstrap,
          consumedLiveSession.config,
        );
      createRoot(root).render(
        <StrictMode>
          {mismatchedLiveSession ? (
            <NegotiateApp
              session={{ ...negotiateBootstrap, connection: "disconnected", action_offers: [] }}
              error="The Negotiate view and live Membership authority do not identify the same Room Session."
            />
          ) : consumedLiveSession.config === null ? (
            <NegotiateApp session={negotiateBootstrap} />
          ) : (
            <NegotiateLiveApp
              initial={negotiateBootstrap}
              config={consumedLiveSession.config}
              transport={consumedLiveSession.transport}
              replayClient={consumedLiveSession.replayClient}
            />
          )}
        </StrictMode>,
      );
      return;
    }
    const scenario = scenarioFromSearch(window.location.search);

    createRoot(root).render(
      <StrictMode>
        <App
          fixture={scenario.fixture}
          initialView={scenario.initialView}
          liveSession={consumedLiveSession.config ?? undefined}
          liveTransport={consumedLiveSession.transport}
          liveReplayClient={consumedLiveSession.replayClient}
        />
      </StrictMode>,
    );
  });
}
