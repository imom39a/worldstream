// Deliberately separate from both authenticated hosted and local kernel sessions.
// Practice never consumes handoffs, opens streams, or runs models.
import { createRoot } from "react-dom/client";
import { PlayerPrototype } from "./prototype-player/PlayerPrototype";

if (location.hash) history.replaceState(null, "", `${location.pathname}${location.search}`);
createRoot(document.getElementById("root")!).render(
  <PlayerPrototype practiceMode onExitPractice={() => location.assign("/")} />,
);
