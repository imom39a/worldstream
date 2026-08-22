import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import {
  CLIENT_CONTRACT_IDENTITY,
  CLIENT_CONTRACT_IDENTITY_JSON,
} from "./compatibilityIdentity";
import { consumeLiveSessionBootstrap } from "./liveSession";
import { scenarioFromSearch } from "./scenario";
import "./styles.css";

const root = document.getElementById("root");

if (root === null) {
  throw new Error("missing #root mount point");
}

const identityWindow = window as Window & {
  __WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__?: typeof CLIENT_CONTRACT_IDENTITY;
};
identityWindow.__WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__ = CLIENT_CONTRACT_IDENTITY;
document.documentElement.dataset.worldstreamClientContractIdentity =
  CLIENT_CONTRACT_IDENTITY_JSON;

const scenario = scenarioFromSearch(window.location.search);
const liveBootstrap = window as Window & { __WORLDSTREAM_LIVE_SESSION__?: unknown };
const consumedLiveSession = consumeLiveSessionBootstrap(liveBootstrap);

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
