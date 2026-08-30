import { SiteHeader, type Navigate } from "./siteChrome";

export function NotFoundPage({ onNavigate }: { onNavigate: Navigate }) {
  return (
    <div className="site-shell not-found-shell">
      <a className="skip-link" href="#not-found-content">Skip to content</a>
      <SiteHeader onNavigate={onNavigate} />
      <main className="not-found" id="not-found-content">
        <span>404</span>
        <h1>This page does not exist.</h1>
        <button type="button" onClick={() => onNavigate("/")}>Open demo catalog</button>
      </main>
    </div>
  );
}
