# Midnight Archive Activity Client

This private React package is the independent participant client for the first
Midnight Archive solo expedition. It starts without game data, installs only
the authorized lead Projection delivered by the retained Activity Client
session, and fails closed on Pack identity or Projection drift.

The browser presents the fixed five-location map, visible gate state, sixteen
turns, three power charges, candidate ledgers, the carried ledger's known
confidence, contextual staging controls, and a separate **Commit Turn** step.
Staging is an authoritative Pack Action. The UI never spends a turn, drains
power, moves the lead, opens a gate, or decides an outcome locally.

The local `/midnight-archive-v2/` surface uses the existing loopback retained
session and exposes exact-head verified Replay after a terminal outcome. The
`/midnight-archive-v2/hosted/` surface uses the authenticated WebSocket session;
its controller currently has no Replay method, so terminal Replay is visibly
unavailable there. Neither surface invents a successful server response.
The immutable v1 build remains available at `/midnight-archive-v1/` for Rooms
pinned to the first-playable Pack revision.

The exact supported Pack identity lives only in [`src/config.ts`](src/config.ts).
The v2 build accepts only the production-proved evidence-route revision
`blake3:ee85f264b9c3dfb185ebedc9646bea655740f351c287793cce997336f0f419f2`.

Action types are `stage_move`, `stage_inspect_records`,
`stage_inspect_conservation`, `stage_use_verifier`,
`stage_open_service_hatch`, `stage_recover_candidate`, `stage_extract`,
`stage_wait`, and `commit_turn`. The protocol carries the Action type outside
the Pack payload, so the exact submitted payload is `{ destination }`,
`{ candidate_id }`, or `{}` as declared by the Pack schemas.

```sh
pnpm --dir clients/midnight-archive-web test
pnpm --dir clients/midnight-archive-web build
```
