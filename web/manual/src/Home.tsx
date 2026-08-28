function Feature({ index, title, text, route }: { index: string; title: string; text: string; route: string }) {
  return <a className="feature" href={`#${route}`}><span>{index}</span><h2>{title}</h2><p>{text}</p><b>Read guide →</b></a>;
}

export function Home() {
  return <>
    <section className="hero">
      <p className="eyebrow">Authoritative shared state for humans + agents</p>
      <h1>Build worlds that can explain <em>exactly</em> what happened.</h1>
      <p className="lede">WorldStream is a deterministic Room Kernel for ordered shared change, scoped observations, durable agent activation, crash recovery, and replay—without hosting the model loop.</p>
      <div className="actions"><a className="primary" href="#/quickstart">Start locally</a><a href="#/reference/capabilities">Explore capabilities</a></div>
    </section>
    <section className="feature-grid">
      <Feature index="01" title="Run the whole stack" text="Install pinned tools, launch Studio and the daemon, verify readiness, and understand each process." route="/quickstart" />
      <Feature index="02" title="Build Activity Packs" text="Define typed Actions, deterministic state, scoped views, timers, attention, and Outcomes." route="/activity-packs/overview" />
      <Feature index="03" title="Connect any agent" text="Use one assignment-bound MCP contract from any model provider or external runner." route="/agents/overview" />
      <Feature index="04" title="Operate with confidence" text="Use bounded lifecycle, backup, recovery, observability, privacy, and release runbooks." route="/operations/runbook" />
    </section>
    <section className="boundary"><span>WorldStream owns</span><strong>shared truth · authorization · ordering · views · continuity</strong><span>Your agent owns</span><strong>models · prompts · policy · tools · private memory</strong></section>
    <section className="manual-scope"><p className="eyebrow">Local v0.1 scope</p><h2>Complete enough to build. Honest enough to trust.</h2><p>This manual covers the executable repository today. Proposed features are labeled, provider recipes are not called certifications, and the eventual public server release remains a separate milestone.</p></section>
  </>;
}
