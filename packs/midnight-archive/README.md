# Midnight Archive Activity Pack

`worldstream.midnight-archive` is a deterministic escape-room Activity Pack.
One human `lead` explores a small archive, spends a fixed power budget, chooses
among visible ledger candidates, and extracts before the sixteenth turn ends.
The mission remains fully playable alone, with Mira, with Jonah, or with both.
Each optional Agent Participant accepts bounded tasks and authenticated plans.
Every starting crew member begins in Atrium with the same Standard budget.
The source now uses schema version 2; retained version-1 Bundles remain immutable.

The current source adds two authored evidence sources and the archivist's
fixed preservation agreement. A committed Records inspection reveals a
binding clue, and a committed Conservation inspection reveals a marking clue.
Each clue alone leaves two candidates possible; their intersection recommends
one candidate without exposing the hidden truth marker. Players can still
spend power on the catalog verifier instead. The collection can be preserved
whether or not the lead accepts the agreement; the Conservation–Vault gate
opens only after the lead accepts it and the preservation work is complete.

The Pack has no imports, clock, entropy, filesystem, network, model, tool, or
process authority. Its hidden `truth_marker` exists only in Activity State.
Participant, public, operator, historical, and final-reveal projections never
emit that field or an `authentic_candidate_id` alias.

## Start and action interface

Genesis creates the `briefing` phase. The declared
`worldstream/activity-start/v1` contract accepts exactly:

```json
{
  "input_type": "worldstream.midnight-archive/briefing-opened/v1",
  "canonical_payload": { "opened_by": "host" }
}
```

After start, the lead stages one action for free, may replace it for free, and
then sends `commit_turn` to spend one turn. The declared actions are:

| Action | Payload | Committed effect |
| --- | --- | --- |
| `stage_move` | `{"destination":"<location>"}` | Move across an edge whose gate was open at turn start |
| `stage_inspect_records` | `{}` | Spend one turn to disclose the authored Records source |
| `stage_inspect_conservation` | `{}` | Spend one turn to disclose the authored Conservation source |
| `stage_use_verifier` | `{}` | Spend 1 power in Records and identify the instrument's candidate |
| `stage_accept_preservation_agreement` | `{}` | Record the human lead's acceptance of the archivist's fixed agreement |
| `stage_prepare_collection` | `{}` | Spend one turn preparing the threatened collection in Conservation |
| `stage_energize_preservation_equipment` | `{}` | Spend 1 power and one turn completing preservation after preparation |
| `stage_open_service_hatch` | `{}` | Spend 2 power in Plant and open the Plant–Vault edge |
| `stage_recover_candidate` | `{"candidate_id":"<candidate>"}` | Carry or exchange a candidate in the Vault |
| `stage_protect_source_record` | `{}` | Spend 1 power and one turn in Plant after recovering a ledger |
| `stage_extract` | `{}` | Resolve extraction in the Atrium |
| `prepare_extraction` | `{}` | Preview the exact post-resolution extracted crew after staging extraction |
| `acknowledge_extraction` | `{"preview_revision":N,"left_behind_roles":[...]}` | Acknowledge precisely the starting crew left behind, including an empty set |
| `stage_wait` | `{}` | Spend a turn without moving |
| `commit_turn` | `{}` | Commit the currently staged action |

When Mira is present, the lead also receives structured controls that do not
advance the turn by themselves:

| Action | Payload | Effect |
| --- | --- | --- |
| `assign_mira_task` | `{"task_kind":"investigate_records|investigate_conservation|field_assay|open_service_hatch","power_allowance":0|1|2}` | Assign or replace Mira's task; allowance 2 is permitted only for hatch work |
| `cancel_mira_task` | `{}` | Cancel the task and invalidate its plan and preparation |
| `set_mira_follow` | `{}` | Have Mira follow one legal edge when the lead moves |
| `set_mira_hold` | `{}` | Keep Mira at her current location |
| `set_mira_regroup` | `{}` | Have Mira move one legal edge toward Atrium per committed turn |
| `request_mira_plan` | `{}` | Open a short, revision-fenced planning opportunity for Mira |
| `prepare_mira_contribution` | `{}` | Select the next eligible plan step for the lead's next commit |
| `defer_mira_contribution` | `{}` | Explicitly skip Mira's contribution on the lead's next commit |

Jonah has corresponding `assign_jonah_task`, `cancel_jonah_task`,
`set_jonah_follow`, `set_jonah_hold`, `set_jonah_regroup`, `request_jonah_plan`,
`prepare_jonah_contribution`, and `defer_jonah_contribution` Actions. His task
allowance is at most 1; `field_assay` is Mira-only. Jonah opens the Plant hatch
with one prepared work step and one charge. Human and Mira hatch work costs two.
Mira's zero-charge Vault assay requires `collect_assay_sample` followed by
`complete_field_assay` on a later committed turn. Sampling reveals no result;
completion publishes the authored evidence and verified assay result.

Each specialist may submit `submit_companion_plan` with one to three typed steps for
the exact open task and opportunity revisions. A submitted plan never advances
the world automatically. The lead must prepare a still-eligible step, and at
most one prepared step resolves with the next committed human turn. Invalid or
stale plans, preparations, memberships, locations, and power allowances are
rejected or invalidated before they can affect Activity State. There is at most
one open planning opportunity in the Room. Unprepared tasks do no work and do
not block the human's turn. Prepared work reserves shared power and unique
interactions; disclosed conflicts require restaging or explicit deferral.
Independent effects resolve in lead–Mira–Jonah order from beginning-of-turn
prerequisites, so a new gate cannot enable another contribution in that turn.

Each explicit planning request opens one fifteen-second Pack opportunity.
The window uses recorded admission time and a one-shot Timer, independently
of a provider's timeout. Its canonical deadline preserves all admitted
fractional precision, including nanoseconds; a reply at the exact deadline is
late. Any accepted personal edit, task/order/preparation
edit or Core proposal cancels an outstanding window before applying the edit.
A rejected edit leaves it unchanged. A new planning request still rejects
while another window is open, and a proposal must match its exact current
Head, task and opportunity. An accepted standing plan survives ordinary turns;
only its next eligible prepared step can execute.

Expiry advances no Activity Turn, spends no power, executes no staged work,
and emits no new Attention Signal. The lead can defer, wait, follow, regroup
or explicitly extract with crew left behind. The Pack neither accepts a
browser's provider-health assertion nor owns provider attempts or concurrency;
those remain Runner responsibilities.

Extraction requires prepare, acknowledgement and commit. Its preview includes
companions returning to Atrium on that turn. Orders, plans, preparations,
staged work, planning expiry and Core reconciliation invalidate an old preview.
Crew sets use canonical lead–Mira–Jonah order; the acknowledgement must match
the exact preview revision and left-behind set. Starting crew identity is
retained from Genesis, so suspension or departure never erases that liability.

The locations are `atrium`, `records`, `conservation`, `plant`, and `vault`.
Open edges connect Atrium–Records, Atrium–Conservation,
Records–Conservation, and Records–Plant. Conservation–Vault opens after both
the agreement and collection work are committed. Plant–Vault opens after a
committed service-hatch action. A gate opened during a commit can be traversed
starting with the next turn.

Extraction on turn 16 resolves before exhaustion. Its factual outcome is one
of `success`, `partial_extraction`, `wrong_ledger`, or `no_ledger`; full success
requires both the authentic ledger and every starting crew member. Any other sixteenth committed
turn ends with `exhausted_inside`.

## Participant projection

Every participant projection has the same bounded shape, including during
briefing. Each companion is represented by a typed summary of location,
mode, task, planning opportunity, plan progress, preparation, disclosed
knowledge, last contribution and assay progress. Each specialist sees their own
unshared evidence; other participants see only explicitly disclosed findings.
Private plan payloads and Runner inputs are never included in those summaries.

```text
phase, objective, location, turns_used, turns_remaining, power,
gates, map, candidates, staged_action, carried_candidate, debrief,
verifier_result, preservation_agreement, optional_objectives, mira, jonah,
turn_resolution, extraction, crew_debrief, outcome
```

`staged_action` is `null` or an exact staged action object with
`action_type`, `turn_cost`, `power_cost`, and its required destination or
candidate. `verifier_result` is `null` or
`{candidate_id, confidence:"verified"}`. `outcome` is `null` or a one-key
object naming a terminal outcome. Candidate IDs and visible
attributes do not identify the authentic ledger by themselves. Every candidate
also carries a bounded `observed_evidence` list plus an `unknown`, `observed`,
or `recommended` assessment. Terminal `debrief` reports whether zero, one, or
both authored sources were inspected, whether the agreement was accepted or
honored, and the two optional-objective outcomes. Recovery or exchange always
costs a committed turn and does not reveal hidden authenticity. `turn_resolution`
discloses prepared, deferred and unprepared roles, resource reservations and
conflict codes. `crew_debrief` retains starting, extracted and left-behind roles
plus typed, turn-attributed executed companion effects. Plans and advice are
never counted as completed work.

## Build and proof

From the repository root with the repository's Node 24 toolchain:

```sh
pnpm --filter @worldstream/midnight-archive pack:check
pnpm --filter @worldstream/midnight-archive pack:test
pnpm --filter @worldstream/midnight-archive pack:build
pnpm --filter @worldstream/midnight-archive pack:inspect
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  pnpm --filter @worldstream/midnight-archive pack:prove
```

The golden corpus executes the documented fifteen-turn powered-verification
and agreement route as 32 Actions, including two free extraction-confirmation
Actions. It preserves the collection,
protects the source record, and extracts with zero power left. Focused tests
also cover the eleven-turn agreement route, independent preservation,
insufficient power, a recoverable committed wait, exact authored terms,
human-only acceptance, briefing and exact start, restaging, closed-gate
traversal, replay, malformed and illegal actions, wrong/no-ledger extraction,
exhaustion, turn-16 extraction, and projection privacy. Focused companion tests
also cover authenticated one-shot planning, standing-task replanning, one
prepared step per committed turn, private inspection and explicit sharing,
allowance and shared-power checks, replacement/suspension invalidation,
terminal timer cancellation, following and one-edge regrouping.
Unavailable-companion tests cover both Roles' cancellation and expiry races,
accepted standing-plan retention, canonical nanosecond deadline boundaries,
no-allowance continuation, restored Membership eligibility, and explicit
partial extraction with attribution only for executed work. Hosted Room
qualification uses an injected HostClock and the real Runtime scheduler to
expire a missing reply, reconnects the original agent capabilities, and
compares the completed mission with authorized Replay.

The current unavailable-companion 0.1.0 bundle is
[`worldstream-midnight-archive-0ddcd385d0b250f9ec5a285df1783fbae45a685a90fcf69703026787959b540f.wspack`](releases/0.1.0/worldstream-midnight-archive-0ddcd385d0b250f9ec5a285df1783fbae45a685a90fcf69703026787959b540f.wspack).
Its physical bundle digest is
`blake3:0ddcd385d0b250f9ec5a285df1783fbae45a685a90fcf69703026787959b540f`,
its semantic revision is
`blake3:d398d13df28f50edcc271aa8f6ffa75f1c26eca1851ac78215aa8e0f6017d8e5`,
its Component digest is
`blake3:326eaf7db7474a9484f8e53a6398f111077e8bf4f41d0b4fc79590a2a0ac6d0d`,
and its production proof is retained in
[`evidence/production-proof-0.1.0-unavailable-companions.json`](evidence/production-proof-0.1.0-unavailable-companions.json).

The earlier consecutive-assay specialist-crew 0.1.0 bundle remains retained
for exact reproducibility:
[`worldstream-midnight-archive-b82e0d5df065af1b31252a06e06a3bbd685a5ba63c3d2bfeb71a0d6dd06ebbab.wspack`](releases/0.1.0/worldstream-midnight-archive-b82e0d5df065af1b31252a06e06a3bbd685a5ba63c3d2bfeb71a0d6dd06ebbab.wspack).
Its physical bundle digest is
`blake3:b82e0d5df065af1b31252a06e06a3bbd685a5ba63c3d2bfeb71a0d6dd06ebbab`,
its semantic revision is
`blake3:59bb814b920b0eb6f8b8886f2d368052189c0f9263d6c36c91894f639c8e19f5`,
its Component digest is
`blake3:1c127958464352e324ddb450e0df398cbcc9867bbcb1834cb972df4595f3a87b`,
and its production proof is retained in
[`evidence/production-proof-0.1.0-specialist-crew-consecutive.json`](evidence/production-proof-0.1.0-specialist-crew-consecutive.json).

The superseded pre-live specialist-crew 0.1.0 bundle remains retained for
exact reproducibility and must not be selected for new Rooms:
[`worldstream-midnight-archive-309721db5290e9f2daf8c092ed97528d8cf7dc2bdcf577edef737f26e14157ea.wspack`](releases/0.1.0/worldstream-midnight-archive-309721db5290e9f2daf8c092ed97528d8cf7dc2bdcf577edef737f26e14157ea.wspack).
Its physical bundle digest is
`blake3:309721db5290e9f2daf8c092ed97528d8cf7dc2bdcf577edef737f26e14157ea`,
its semantic revision is
`blake3:894f7a58c01b0ca99ac29b1b84a9083bab7f0f858f0b33ce495413255cf91339`,
its Component digest is
`blake3:6c645f1eefc5bf9620fb670fd6b2908af2775fabe3ececfe7a19d314d40e36f0`,
and its production proof is retained in
[`evidence/production-proof-0.1.0-specialist-crew.json`](evidence/production-proof-0.1.0-specialist-crew.json).

The retained IMO-202 Mira 0.1.0 bundle (before schema version 2 specialists) is
[`worldstream-midnight-archive-ea79ce7ff3e90ab5d073409486af1227286b82512daec98db3e921097dd8ac99.wspack`](releases/0.1.0/worldstream-midnight-archive-ea79ce7ff3e90ab5d073409486af1227286b82512daec98db3e921097dd8ac99.wspack).
Its physical bundle digest is
`blake3:ea79ce7ff3e90ab5d073409486af1227286b82512daec98db3e921097dd8ac99`,
its semantic revision is
`blake3:ec4689e090f05f1c1894f21c1dba95e1f56b3afc49fc88c5c8f3530a03a80b61`,
its Component digest is
`blake3:adf00ecdfa0df9285b813d77277c3b695ff33faacf954093835e15865e14c862`,
and its production proof is retained in
[`evidence/production-proof-0.1.0-mira.json`](evidence/production-proof-0.1.0-mira.json).

The agreement-route 0.1.0 bundle remains retained at
[`worldstream-midnight-archive-877702b321352288553cc0e5ea6510f1f8dea3e18687759658714ebc09a3c269.wspack`](releases/0.1.0/worldstream-midnight-archive-877702b321352288553cc0e5ea6510f1f8dea3e18687759658714ebc09a3c269.wspack).
Its physical bundle digest is
`blake3:877702b321352288553cc0e5ea6510f1f8dea3e18687759658714ebc09a3c269`,
its semantic revision is
`blake3:6c3ad825a65307b9f5434d4a9140b7db4bd70d1f7830c66d6f6af1d2ba9dc0da`,
its Component digest is
`blake3:e41912be5a4fd3117fddf07a583cf7f42f559c708b0eeef234e88d9043ae8ba5`,
and its production proof is retained in
[`evidence/production-proof-0.1.0-agreement-route.json`](evidence/production-proof-0.1.0-agreement-route.json).

The connected-evidence 0.1.0 revision remains retained at
[`worldstream-midnight-archive-e0626769fa745fafd0e41238473902988f446b453f283c7a7a7155a9122cf03f.wspack`](releases/0.1.0/worldstream-midnight-archive-e0626769fa745fafd0e41238473902988f446b453f283c7a7a7155a9122cf03f.wspack).
Its physical bundle digest is
`blake3:e0626769fa745fafd0e41238473902988f446b453f283c7a7a7155a9122cf03f`,
its semantic revision is
`blake3:ee85f264b9c3dfb185ebedc9646bea655740f351c287793cce997336f0f419f2`,
and its Component digest is
`blake3:b5464e19490b558e798c0e3b58f65cb8cc0fe17053e7d2a8fca8938e4fdcdd8e`.
Its production proof is retained in
[`evidence/production-proof-0.1.0-evidence-route.json`](evidence/production-proof-0.1.0-evidence-route.json).

The first-playable 0.1.0 revision remains retained at
[`worldstream-midnight-archive-d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b.wspack`](releases/0.1.0/worldstream-midnight-archive-d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b.wspack).
Its physical bundle digest is
`blake3:d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b`,
its semantic revision is
`blake3:679022bf13c15ea014e18a7129b679c9bfd27873c570fd9cd0c02818f0880a7a`,
and its Component digest is
`blake3:9b86dcd4173aad23167e01cfd936a91d95396fc6faf02220cdac8046fdb22783`.
Its production proof is retained in
[`evidence/production-proof-0.1.0.json`](evidence/production-proof-0.1.0.json).
