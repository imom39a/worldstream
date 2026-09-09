# Midnight Archive Activity Client

This private React package is the independent participant client for the first
Midnight Archive solo expedition. It starts without game data, installs only
the authorized lead Projection delivered by the retained Activity Client
session, and fails closed on Pack identity or Projection drift.

The browser presents the fixed five-location map, visible gate state, sixteen
turns, three power charges, candidate ledgers, the carried ledger's known
confidence, contextual staging controls, and a separate **Commit Turn** step.
The Conservation view presents the Archivist's fixed preservation agreement,
its ordered conditions, and direct controls for accepting and completing it.
Optional collection and source-protection objectives remain visible at every
location and become factual booleans in the terminal debrief.
Staging is an authoritative Pack Action. The UI never spends a turn, drains
power, moves the lead, opens a gate, or decides an outcome locally.

The local `/midnight-archive-v3/` surface uses the existing loopback retained
session and exposes exact-head verified Replay after a terminal outcome. The
`/midnight-archive-v3/hosted/` surface uses the authenticated WebSocket session;
its controller currently has no Replay method, so terminal Replay is visibly
unavailable there. Neither surface invents a successful server response.
The immutable v1 and v2 builds remain available at `/midnight-archive-v1/` and
`/midnight-archive-v2/` for Rooms pinned to their exact Pack revisions.

The exact supported Pack identity lives only in [`src/config.ts`](src/config.ts).
The v3 build accepts only the production-proved agreement-route revision
`blake3:6c3ad825a65307b9f5434d4a9140b7db4bd70d1f7830c66d6f6af1d2ba9dc0da`.

Action types are `stage_move`, `stage_inspect_records`,
`stage_inspect_conservation`, `stage_use_verifier`,
`stage_accept_preservation_agreement`, `stage_prepare_collection`,
`stage_energize_preservation_equipment`, `stage_open_service_hatch`,
`stage_recover_candidate`, `stage_protect_source_record`, `stage_extract`,
`stage_wait`, and `commit_turn`. The protocol carries the Action type outside
the Pack payload, so the exact submitted payload is `{ destination }`,
`{ candidate_id }`, or `{}` as declared by the Pack schemas.

```sh
pnpm --dir clients/midnight-archive-web test
pnpm --dir clients/midnight-archive-web build
```
