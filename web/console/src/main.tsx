import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

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
import { scenarioFromSearch } from "./scenario";
import { StudioPrototype } from "./StudioPrototype";
import { HandedOffParticipant } from "./HandedOffParticipant";
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
    const scenario = scenarioFromSearch(window.location.search);

    createRoot(root).render(
      <StrictMode>
        <App
          fixture={scenario.fixture}
          initialView={scenario.initialView}
        />
      </StrictMode>,
    );
  });
}
