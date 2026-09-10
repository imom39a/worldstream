import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import {
  ActivityClientHandoffClient,
  resumeRetainedActivityClient,
  selectActivityClientStartup,
} from "@worldstream/client";

import { StandaloneMidnightArchiveClient } from "./StandaloneMidnightArchiveClient";
import { createMidnightArchiveRetryingFetch } from "./retryingFetch";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");
const startup = selectActivityClientStartup(window);
const location = new URL(window.location.origin);
const loopback = location.protocol === "http:"
  && ["127.0.0.1", "localhost", "[::1]"].includes(location.hostname);

if (!loopback) {
  createRoot(root).render(
    <main className="archive-boundary">
      <p className="archive-kicker">Midnight Archive</p>
      <h1>Local client only</h1>
      <p>Enter your hosted expedition from the activity library.</p>
    </main>,
  );
} else {
  const client = new ActivityClientHandoffClient(
    import.meta.env.VITE_WORLDSTREAM_SUPERVISOR_URL ?? "http://127.0.0.1:9420",
    createMidnightArchiveRetryingFetch(),
  );
  void resumeRetainedActivityClient(startup, client).then((resolvedStartup) => {
    createRoot(root).render(
      <StrictMode>
        <StandaloneMidnightArchiveClient startup={resolvedStartup} client={client} />
      </StrictMode>,
    );
  });
}
