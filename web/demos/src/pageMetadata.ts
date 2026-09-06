import { useEffect } from "react";

export type PageKind = "catalog" | "formation" | "join" | "run" | "agent-heist" | "agent-heist-docs" | "not-found";

const socialPreviewUrl = "https://worldstream-demos.vercel.app/og.png";

const pageMetadata: Record<PageKind, { readonly title: string; readonly description: string }> = {
  catalog: {
    title: "WorldStream live activities",
    description: "Form live rooms where people, external agents, and reviewed House Agents participate together.",
  },
  formation: {
    title: "Live activity waiting room | WorldStream",
    description: "Gather participants and start one reviewed WorldStream activity.",
  },
  join: {
    title: "Join a live activity | WorldStream",
    description: "Claim an invited seat for yourself or an external agent.",
  },
  run: {
    title: "Public activity Run | WorldStream",
    description: "View the public live state or Replay-verified result for one WorldStream activity Run.",
  },
  "agent-heist": {
    title: "Agent Heist recorded demo | WorldStream",
    description: "Inspect selected Agent Heist parity records through Public, Navigator, and Operator examples.",
  },
  "agent-heist-docs": {
    title: "Agent Heist browser fixture documentation | WorldStream",
    description: "Read the data source, controls, boundaries, and exact identity for the Agent Heist browser fixture.",
  },
  "not-found": {
    title: "Page not found | WorldStream",
    description: "The requested WorldStream demo page does not exist.",
  },
};

export function usePageMetadata(page: PageKind) {
  useEffect(() => {
    const metadata = pageMetadata[page];
    const catalogPage = page === "catalog";

    document.title = metadata.title;
    setMetaContent('meta[name="description"]', "name", "description", metadata.description);
    setMetaContent('meta[property="og:title"]', "property", "og:title", metadata.title);
    setMetaContent('meta[property="og:description"]', "property", "og:description", metadata.description);
    setMetaContent('meta[name="twitter:title"]', "name", "twitter:title", metadata.title);
    setMetaContent('meta[name="twitter:description"]', "name", "twitter:description", metadata.description);
    setMetaContent('meta[name="twitter:card"]', "name", "twitter:card", catalogPage ? "summary_large_image" : "summary");

    if (catalogPage) {
      setMetaContent('meta[property="og:image"]', "property", "og:image", socialPreviewUrl);
      setMetaContent('meta[name="twitter:image"]', "name", "twitter:image", socialPreviewUrl);
    } else {
      document.querySelector('meta[property="og:image"]')?.remove();
      document.querySelector('meta[name="twitter:image"]')?.remove();
    }
  }, [page]);
}

function setMetaContent(selector: string, attribute: "name" | "property", key: string, content: string) {
  let element = document.head.querySelector<HTMLMetaElement>(selector);
  if (element === null) {
    element = document.createElement("meta");
    element.setAttribute(attribute, key);
    document.head.append(element);
  }
  element.content = content;
}
