import { useEffect, useMemo, useRef, useState } from "react";

import { CapabilityExplorer } from "./CapabilityExplorer";
import { Home } from "./Home";
import { MarkdownPage } from "./MarkdownPage";
import { normalizeRoute, searchPages } from "./manual";
import { manualPages, navigationGroups } from "./pages";

function currentRoute(): string {
  return normalizeRoute(window.location.hash, manualPages);
}

export function App() {
  const [route, setRoute] = useState(currentRoute);
  const [query, setQuery] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  const page = manualPages.find((item) => item.route === route);

  useEffect(() => {
    const onHashChange = () => {
      setRoute(currentRoute());
      setMenuOpen(false);
      window.scrollTo({ top: 0 });
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "/" && !["INPUT", "SELECT", "TEXTAREA"].includes(document.activeElement?.tagName ?? "")) {
        event.preventDefault();
        search.current?.focus();
      }
      if (event.key === "Escape") {
        setQuery("");
        search.current?.blur();
      }
    };
    window.addEventListener("hashchange", onHashChange);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("hashchange", onHashChange);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  useEffect(() => {
    document.title = `${page?.title ?? "Developer Manual"} · WorldStream`;
  }, [page]);

  const matches = useMemo(() => searchPages(query, manualPages).slice(0, 9), [query]);
  const demosUrl = import.meta.env.VITE_DEMOS_URL || "https://worldstream-demos.vercel.app";

  return (
    <div className="manual-shell">
      <a className="skip-link" href="#main-content">Skip to manual content</a>
      <header className="topbar">
        <a className="brand" href="#/" aria-label="WorldStream manual home">
          <span className="brand-mark" aria-hidden="true"><i /><b /><em /></span>
          <span><strong>WorldStream</strong><small>Developer manual</small></span>
        </a>
        <div className="search-wrap">
          <input ref={search} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search concepts, commands, APIs…" aria-label="Search the manual" />
          <kbd>/</kbd>
          {query.trim() !== "" && (
            <div className="search-results" role="listbox">
              {matches.map((item) => <a key={item.route} href={`#${item.route}`} onClick={() => setQuery("")}><strong>{item.title}</strong><small>{item.summary}</small></a>)}
              {matches.length === 0 && <p>No matching page. Try “Studio”, “backup”, or “MCP”.</p>}
            </div>
          )}
        </div>
        <nav className="topbar-links" aria-label="Related sites">
          <a className="source-link" href={demosUrl}>Demos</a>
        </nav>
        <button className="menu-button" type="button" onClick={() => setMenuOpen((value) => !value)} aria-expanded={menuOpen}>Menu</button>
      </header>
      <aside className={menuOpen ? "sidebar open" : "sidebar"}>
        <nav aria-label="Home"><a className={route === "/" ? "active" : ""} href="#/">Manual home</a></nav>
        {navigationGroups.map((group) => <nav key={group.label} aria-label={group.label}><p>{group.label}</p>{group.items.map((item) => <a className={route === item.route ? "active" : ""} key={item.route} href={`#${item.route}`}>{item.title}</a>)}</nav>)}
      </aside>
      <main className="content" id="main-content">
        {route === "/" ? <Home /> : route === "/reference/capabilities" ? <CapabilityExplorer /> : page ? <MarkdownPage page={page} /> : <Home />}
      </main>
    </div>
  );
}
