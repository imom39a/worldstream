# Midnight Archive browser client

This React example presents a cooperative game through scoped WorldStream
projections and exact Action Offers. It shows the map, turn and power budgets,
known evidence, crew tasks, staged actions, and terminal debrief. Game rules and
outcomes remain in the Pack; the browser never resolves a turn locally.

The live view starts empty and rejects an unexpected Pack identity or Projection
shape. The supported identity is in [src/config.ts](src/config.ts). Scenario
briefings and advice render only the participant's authorized evidence.

From the repository root:

```sh
pnpm --dir examples/clients/midnight-archive-web test
pnpm --dir examples/clients/midnight-archive-web build
pnpm activity-clients:serve
```

The current local build is served at `/midnight-archive-v14/` and supports
verified Replay after a terminal outcome. Live participation requires an
authorized Controller handoff. Historical compiled builds are removed. The
hosted transport adapter remains as source, but its former backend is retired.
See [browser client setup](../README.md).
