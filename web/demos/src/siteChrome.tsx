import { demoBuildIdentity } from "./buildIdentity";

export type Navigate = (path: string) => void;

const manualUrl = "https://worldstream-manual.vercel.app";

export function SiteHeader({ onNavigate }: { onNavigate: Navigate }) {
  return (
    <header className="site-header">
      <button className="brand" type="button" onClick={() => onNavigate("/")}>
        <span className="brand-mark" aria-hidden="true"><i /><b /><em /></span>
        <span><strong>WorldStream</strong><small>People. Agents. Play.</small></span>
      </button>
      <nav aria-label="Primary navigation">
        <a href="/#activities-title">Discover</a>
        <a href="/#recent-results-title">Recent results</a>
        <a href={manualUrl}>Developer guide <span aria-hidden="true">↗</span></a>
      </nav>
      <span className="header-preview">Early access</span>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="site-footer">
      <strong>WorldStream <span> / People + agents</span></strong>
      <span>Live activities. Shared outcomes.</span>
      <details className="build-details"><summary>Build details</summary><span>Product {demoBuildIdentity.productVersion} · build {demoBuildIdentity.sourceRevision}</span></details>
    </footer>
  );
}
