# Agent Heist browser client

This React example presents hidden-information participant and spectator views.
It starts empty and installs only the authorized Projection Reset from a
WorldStream session. The live adapter has no recorded-fixture fallback.
`RecordedAgentHeistWorkspace` is a separate fixture-driven presentation export.

From the repository root:

```sh
pnpm --dir examples/clients/agent-heist-web test
pnpm --dir examples/clients/agent-heist-web build
pnpm activity-clients:serve
```

The current source build is served at `/agent-heist-v12/` on the loopback client
host. Live participation needs an authorized Controller handoff. The optional
Vite dev server is for component development, not an approved handoff origin.
Hosted adapters are retained source examples; their former services are retired.
See [browser client setup](../README.md).
