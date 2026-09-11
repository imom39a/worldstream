# Midnight Archive Activity Client

This private React package is the independent lead participant client for
Midnight Archive solo, one-specialist, and full-crew expeditions. It starts without game
data, installs only the authorized lead Projection delivered by the retained
Activity Client session, and fails closed on Pack identity or Projection drift.

The browser presents the fixed five-location map, visible gate state, sixteen
turns, the current and initial power reserve, candidate ledgers, the carried ledger's known
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
The Companion advice panel renders the newest four authorized accepted utterances as literal text, labeled recommendations. It adds no human free-text controls or permissions.
Staging is an authoritative Pack Action. The UI never spends a turn, drains
power, moves the lead, opens a gate, or decides an outcome locally.

Standard and Low Reserve are fixed authored scenarios. Their candidate markings,
source evidence, authentic ledger, and initial reserve differ; the browser reveals
only the participant's authorized evidence. Standard starts with three shared
power charges; Low Reserve starts with two. The scenario briefing explains the
projected verifier (one power) and ordinary service hatch (two power) costs, whose
combined cost exceeds Low Reserve's initial budget. Action Offers still determine
which controls are available; the client neither selects nor rerolls a scenario.

The current local `/midnight-archive-v14/` surface uses the existing loopback
retained session and exposes exact-head verified Replay after a terminal outcome.
The current `/midnight-archive-v14/hosted/` surface uses the authenticated
WebSocket session. Its controller currently has no Replay method, so terminal
Replay is visibly unavailable there. The retained `/midnight-archive-v12/`
paths remain available
for Rooms pinned to their exact revision. Neither surface invents a successful
server response. The retained v1 through v12 builds remain available at their
versioned paths for Rooms pinned to their exact Pack revisions.

The exact supported Pack identity lives only in [`src/config.ts`](src/config.ts).
The current v13 build requires the v5 session-expiry Projection contract,
including the retained bounded-dialogue fields, and its exact Pack revision
`blake3:aea45a1c056c4a7da744be33d38df672499062fd2548302fae43adc49b833b07`.
The retained v12 build preserves that exact revision for its pinned Rooms.
The retained v9 build supports the earlier unavailable-companion
revision `blake3:d398d13df28f50edcc271aa8f6ffa75f1c26eca1851ac78215aa8e0f6017d8e5`.
The retained v7 and v8 builds target superseded Pack candidates and are
unqualified; they are preserved for exact historical Room compatibility only.

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
