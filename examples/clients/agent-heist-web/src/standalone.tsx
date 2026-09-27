import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import {
  ActivityClientHandoffClient,
  resumeRetainedActivityClient,
  selectActivityClientStartup,
} from "@worldstream/client";

import { StandaloneAgentHeistClient } from "./StandaloneAgentHeistClient";
import "./styles.css";

if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("variant")) {
  // THROWAWAY UX study: isolated sample state, never a live session or mutation.
  void import("./prototype-player/PlayerPrototype").then(({ mountPrototype }) => mountPrototype());
} else {
const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");
// Strip the one-use handoff before any request, including on a wrong surface.
const startup = selectActivityClientStartup(window);
const location = new URL(window.location.origin);
if (location.protocol !== "http:" || !["127.0.0.1", "localhost", "[::1]"].includes(location.hostname)) {
  createRoot(root).render(<main className="heist-boundary-shell">
    <h1>Local client only</h1>
    <p>Enter your hosted Run from the activity catalog.</p>
  </main>);
} else {
  const client = new ActivityClientHandoffClient(
    import.meta.env.VITE_WORLDSTREAM_SUPERVISOR_URL ?? "http://127.0.0.1:9420",
  );
  void resumeRetainedActivityClient(startup, client).then((resolvedStartup) => {
    createRoot(root).render(<StrictMode>
      <StandaloneAgentHeistClient startup={resolvedStartup} client={client} />
    </StrictMode>);
  });
}
}
