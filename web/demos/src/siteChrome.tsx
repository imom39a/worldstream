import { demoBuildIdentity } from "./buildIdentity";

export type Navigate = (path: string) => void;

const manualUrl = "https://worldstream-manual.vercel.app";

export function SiteHeader({ onNavigate }: { onNavigate: Navigate }) {
  return (
    <header className="site-header">
      <button className="brand" type="button" onClick={() => onNavigate("/")}>
        <span className="brand-mark" aria-hidden="true"><i /><b /><em /></span>
        <span><strong>WorldStream</strong><small>Live activities</small></span>
      </button>
      <nav aria-label="Primary navigation">
        <a href="/#activities-title">Activities</a>
        <a href="/demos/agent-heist">Recorded demo</a>
        <a href={manualUrl}>Manual</a>
      </nav>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="site-footer">
      <strong>WorldStream</strong>
      <span>Product {demoBuildIdentity.productVersion} · build {demoBuildIdentity.sourceRevision}</span>
      <span>STE-based draft</span>
    </footer>
  );
}
