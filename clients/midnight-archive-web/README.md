# Midnight Archive Activity Client

This private React package is the independent lead participant client for
Midnight Archive solo and human-plus-Mira expeditions. It starts without game
data, installs only the authorized lead Projection delivered by the retained
Activity Client session, and fails closed on Pack identity or Projection drift.

The browser presents the fixed five-location map, visible gate state, sixteen
turns, three power charges, candidate ledgers, the carried ledger's known
confidence, contextual staging controls, and a separate **Commit Turn** step.
The Conservation view presents the Archivist's fixed preservation agreement,
its ordered conditions, and direct controls for accepting and completing it.
Optional collection and source-protection objectives remain visible at every
location and become factual booleans in the terminal debrief.
When Mira is present, a crew card shows her separate Location, standing task,
resource allowance, bounded-plan status, preparation fence, progress and only
the evidence she has disclosed. Structured controls assign, change or cancel
her task; request a plan; prepare or defer one contribution; and select
following, holding or one-edge regrouping. Plan step payloads remain private.
Staging is an authoritative Pack Action. The UI never spends a turn, drains
power, moves the lead, opens a gate, or decides an outcome locally.

The local `/midnight-archive-v4/` surface uses the existing loopback retained
session and exposes exact-head verified Replay after a terminal outcome. The
`/midnight-archive-v4/hosted/` surface uses the authenticated WebSocket session;
its controller currently has no Replay method, so terminal Replay is visibly
unavailable there. Neither surface invents a successful server response.
The immutable v1, v2 and v3 builds remain available at their versioned paths
for Rooms pinned to their exact Pack revisions.

The exact supported Pack identity lives only in [`src/config.ts`](src/config.ts).
The v4 build accepts only the production-proved Mira revision
`blake3:ec4689e090f05f1c1894f21c1dba95e1f56b3afc49fc88c5c8f3530a03a80b61`.

Action types are `stage_move`, `stage_inspect_records`,
`stage_inspect_conservation`, `stage_use_verifier`,
`stage_accept_preservation_agreement`, `stage_prepare_collection`,
`stage_energize_preservation_equipment`, `stage_open_service_hatch`,
`stage_recover_candidate`, `stage_protect_source_record`, `stage_extract`,
`stage_wait`, `commit_turn`, `assign_mira_task`, `cancel_mira_task`,
`set_mira_follow`, `set_mira_hold`, `set_mira_regroup`, `request_mira_plan`,
`prepare_mira_contribution`, and `defer_mira_contribution`. The protocol carries
the Action type outside the Pack payload, so the exact lead-submitted payload is
`{ destination }`, `{ candidate_id }`, `{ task_kind, power_allowance }`, or `{}`
as declared by the Pack schemas.

```sh
pnpm --dir clients/midnight-archive-web test
pnpm --dir clients/midnight-archive-web build
```
