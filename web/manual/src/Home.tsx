function Feature({ index, title, text, route }: { index: string; title: string; text: string; route: string }) {
  return <a className="feature" href={`#${route}`}><span>{index}</span><h2>{title}</h2><p>{text}</p><b>Open guide →</b></a>;
}

export function Home() {
  return <>
    <section className="hero">
      <p className="eyebrow">WorldStream · developer manual</p>
      <h1>Build and operate <em>WorldStream.</em></h1>
      <p className="lede">Use this manual to run the local stack, build Activity Packs, connect agents, and operate the system.</p>
      <div className="actions"><a className="primary" href="#/quickstart">Start locally</a><a href="#/reference/capabilities">View capability reference</a></div>
    </section>
    <section className="feature-grid">
      <Feature index="01" title="Run the whole stack" text="Install pinned tools, launch Studio and the daemon, verify readiness, and understand each process." route="/quickstart" />
      <Feature index="02" title="Build Activity Packs" text="Define typed Actions, deterministic state, scoped views, timers, attention, and Outcomes." route="/activity-packs/overview" />
      <Feature index="03" title="Connect any agent" text="Use one assignment-bound MCP contract from any model provider or external runner." route="/agents/overview" />
      <Feature index="04" title="Operate the stack" text="Use lifecycle, backup, recovery, observability, privacy, and release runbooks." route="/operations/runbook" />
    </section>
    <section className="boundary"><span>WorldStream owns</span><strong>shared truth · authorization · ordering · views · continuity</strong><span>Your agent owns</span><strong>models · prompts · policy · tools · private memory</strong></section>
    <section className="manual-scope"><p className="eyebrow">Local v0.1 scope</p><h2>Manual coverage</h2><p>This manual describes the executable repository. It labels proposed features and reference integrations. The public server release is a separate milestone.</p></section>
  </>;
}
