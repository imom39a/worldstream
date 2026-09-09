import { useEffect, useState } from "react";

import { AgentHeistDocsPage } from "./AgentHeistDocsPage";
import { AgentHeistPage } from "./AgentHeistPage";
import { CatalogPage } from "./CatalogPage";
import { JoinPage } from "./JoinPage";
import { LaunchPage } from "./LaunchPage";
import { MyGamesPage } from "./MyGamesPage";
import { demos, getDemoById } from "./catalog";
import { NotFoundPage } from "./NotFoundPage";
import { usePageMetadata, type PageKind } from "./pageMetadata";
import { RunPage } from "./RunPage";
import type { Navigate } from "./siteChrome";

function usePathname(): [string, Navigate] {
  const [pathname, setPathname] = useState(window.location.pathname);

  useEffect(() => {
    const handlePopState = () => setPathname(window.location.pathname);
    window.addEventListener("popstate", handlePopState);
    return () => window.removeEventListener("popstate", handlePopState);
  }, []);

  const navigate = (path: string) => {
    window.history.pushState(null, "", path);
    setPathname(path);
    window.scrollTo({ top: 0, behavior: "auto" });
  };

  return [pathname, navigate];
}

export function App() {
  const [pathname, navigate] = usePathname();
  const demoId = pathname.match(/^\/demos\/([^/]+)\/?$/)?.[1];
  const documentationId = pathname.match(/^\/docs\/([^/]+)\/?$/)?.[1];
  const launchId = pathname.match(/^\/launches\/([0-9a-f-]+)\/?$/)?.[1];
  const publicRunId = pathname.match(/^\/runs\/([0-9a-f]{32})\/?$/)?.[1];
  const page: PageKind = pathname === "/"
    ? "catalog"
    : pathname === "/join"
      ? "join"
      : pathname === "/my-games"
        ? "catalog"
      : publicRunId !== undefined
        ? "run"
      : launchId !== undefined
        ? "formation"
        : demoId === "agent-heist"
          ? "agent-heist"
          : documentationId === "agent-heist"
            ? "agent-heist-docs"
            : "not-found";

  usePageMetadata(page);

  if (pathname === "/join") return <JoinPage onNavigate={navigate} />;
  if (pathname === "/my-games") return <MyGamesPage onNavigate={navigate} />;
  if (publicRunId !== undefined) return <RunPage publicId={publicRunId} onNavigate={navigate} />;
  if (launchId !== undefined) return <LaunchPage launchId={launchId} onNavigate={navigate} />;

  if (documentationId !== undefined) {
    return documentationId === "agent-heist"
      ? <AgentHeistDocsPage onNavigate={navigate} />
      : <NotFoundPage onNavigate={navigate} />;
  }

  if (demoId !== undefined) {
    const demo = getDemoById(demos, demoId);
    return demo?.id === "agent-heist"
      ? <AgentHeistPage onNavigate={navigate} />
      : <NotFoundPage onNavigate={navigate} />;
  }

  return pathname === "/"
    ? <CatalogPage onNavigate={navigate} />
    : <NotFoundPage onNavigate={navigate} />;
}
