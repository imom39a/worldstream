import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { loadDaemonStatus, type DaemonStatus } from "./daemonStatus";
import "./styles.css";

const REFRESH_INTERVAL_MS = 5_000;

function LiveStudio() {
  const [status, setStatus] = useState<DaemonStatus | null>(null);

  useEffect(() => {
    let active = true;
    const refresh = async () => {
      const next = await loadDaemonStatus();
      if (active) setStatus(next);
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), REFRESH_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, []);

  return <App status={status} />;
}

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root mount point");
createRoot(root).render(<StrictMode><LiveStudio /></StrictMode>);
