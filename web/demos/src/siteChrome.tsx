import { demoBuildIdentity } from "./buildIdentity";

export type Navigate = (path: string) => void;

export function SiteHeader({ onNavigate }: { onNavigate: Navigate }) {
  return (
    <header className="site-header">
      <button className="brand" type="button" onClick={() => onNavigate("/")}>
        <span className="brand-mark" aria-hidden="true"><i /><b /><em /></span>
        <span><strong>WorldStream</strong><small>Technical demos</small></span>
      </button>
      <nav aria-label="Primary navigation">
        <a href="/#demos">Demos</a>
        <a href="/#capabilities">Capabilities</a>
        <span className="source-status"><LockIcon /> Source private</span>
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

function LockIcon() {
  return <span className="lock-icon" aria-hidden="true"><i /></span>;
}
