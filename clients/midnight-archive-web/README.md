# Midnight Archive Activity Client

This private React package is the independent lead participant client for
Midnight Archive solo, one-specialist, and full-crew expeditions. It starts without game
data, installs only the authorized lead Projection delivered by the retained
Activity Client session, and fails closed on Pack identity or Projection drift.

The browser presents the fixed five-location map, visible gate state, sixteen
turns, three power charges, candidate ledgers, the carried ledger's known
confidence, contextual staging controls, and a separate **Commit Turn** step.
The Conservation view presents the Archivist's fixed preservation agreement,
its ordered conditions, and direct controls for accepting and completing it.
Optional collection and source-protection objectives remain visible at every
location and become factual booleans in the terminal debrief.
When Mira or Jonah is present, a role-specific crew card shows their separate
Location, standing task, resource allowance, bounded-plan status, preparation
fence, progress and only the evidence they have disclosed. Structured controls
assign, change or cancel each task; request a plan; prepare or defer one
contribution; and select following, holding or one-edge regrouping. The turn
panel shows typed reservations and conflicts in lead, Mira, Jonah order. Plan
step payloads remain private. Extraction has a post-resolution preview and an
exact left-behind acknowledgement before the lead may commit it; the terminal
debrief attributes completed work only to effects the Pack executed.
Staging is an authoritative Pack Action. The UI never spends a turn, drains
power, moves the lead, opens a gate, or decides an outcome locally.

The local `/midnight-archive-v8/` surface uses the existing loopback retained
session and exposes exact-head verified Replay after a terminal outcome. The
`/midnight-archive-v8/hosted/` surface uses the authenticated WebSocket session;
its controller currently has no Replay method, so terminal Replay is visibly
unavailable there. Neither surface invents a successful server response.
The immutable v1 through v7 builds remain available at their versioned paths
for Rooms pinned to their exact Pack revisions.

The exact supported Pack identity lives only in [`src/config.ts`](src/config.ts).
The v8 build accepts only the production-proved unavailable-companion revision
`blake3:94d59090be49f1abd4dc7413ce52b26d07f12bedd4d427b0c24569f90837845b`.
The retained v7 build targets a superseded Pack candidate and is unqualified;
it is preserved for exact historical Room compatibility only.

Action types are `stage_move`, `stage_inspect_records`,
`stage_inspect_conservation`, `stage_use_verifier`,
`stage_accept_preservation_agreement`, `stage_prepare_collection`,
`stage_energize_preservation_equipment`, `stage_open_service_hatch`,
`stage_recover_candidate`, `stage_protect_source_record`, `stage_extract`,
`stage_wait`, `commit_turn`, `assign_mira_task`, `cancel_mira_task`,
`set_mira_follow`, `set_mira_hold`, `set_mira_regroup`, `request_mira_plan`,
`prepare_mira_contribution`, `defer_mira_contribution`, the corresponding eight
`jonah` actions, `prepare_extraction`, and `acknowledge_extraction`. The protocol carries
the Action type outside the Pack payload, so the exact lead-submitted payload is
`{ destination }`, `{ candidate_id }`, `{ task_kind, power_allowance }`, or `{}`
as declared by the Pack schemas.

```sh
pnpm --dir clients/midnight-archive-web test
pnpm --dir clients/midnight-archive-web build
```
