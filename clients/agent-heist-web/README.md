# Agent Heist Activity Client

This private package owns the Agent Heist participant UI and the pure adapter
from authorized WorldStream delivery into that UI. It exports
`AgentHeistClient` for the first-party Client Host and
`RecordedAgentHeistWorkspace` for the recorded demos site.

The Vite entry point on `127.0.0.1:5175` is an isolated component-development
surface. It is **not** a Controller handoff origin in the frozen single-origin
milestone, so it cannot redeem a production participant handoff. The current
Agent Heist 0.3 binding mounts `AgentHeistClient` at
`http://127.0.0.1:5173/agent-heist-v6/` inside the first-party Client Host;
that origin also owns `/inspector/`. Retained Runs can select their original
v2–v5 client paths, including v3; new handoffs must use the reviewed binding
rather than a manually chosen retained route.

The live graph has no recorded-fixture fallback. Until a retained participant
session installs a valid Agent Heist Projection Reset, it renders only its
empty waiting boundary.

```sh
pnpm --dir clients/agent-heist-web test
pnpm --dir clients/agent-heist-web build
```
