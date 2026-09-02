import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import {
  ActivityClientHandoffClient,
  resumeRetainedActivityClient,
  selectActivityClientStartup,
} from "@worldstream/client";

import { NegotiateClient } from "./NegotiateClient";
import "./styles.css";

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");

const client = new ActivityClientHandoffClient(
  import.meta.env.VITE_WORLDSTREAM_SUPERVISOR_URL ?? "http://127.0.0.1:9420",
);
const startup = selectActivityClientStartup(window);

void resumeRetainedActivityClient(startup, client).then((resolvedStartup) => {
  createRoot(root).render(
    <StrictMode>
      <NegotiateClient startup={resolvedStartup} client={client} />
    </StrictMode>,
  );
});
