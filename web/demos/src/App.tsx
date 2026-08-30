import { useEffect, useState } from "react";

import { AgentHeistDocsPage } from "./AgentHeistDocsPage";
import { AgentHeistPage } from "./AgentHeistPage";
import { CatalogPage } from "./CatalogPage";
import { demos, getDemoById } from "./catalog";
import { NotFoundPage } from "./NotFoundPage";
import { usePageMetadata, type PageKind } from "./pageMetadata";
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
  const page: PageKind = pathname === "/"
    ? "catalog"
    : demoId === "agent-heist"
      ? "agent-heist"
      : documentationId === "agent-heist"
        ? "agent-heist-docs"
        : "not-found";

  usePageMetadata(page);

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
