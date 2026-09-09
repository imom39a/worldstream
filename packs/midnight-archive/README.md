# Midnight Archive Activity Pack

`worldstream.midnight-archive` is a deterministic solo escape-room Activity
Pack. One human `lead` explores a small archive, spends a fixed power budget,
chooses among visible ledger candidates, and extracts before the sixteenth
turn ends. The optional `mira` and `jonah` Roles are declared for later
companion work; they can observe their own private projection but cannot act
in version 0.1.0.

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
| `stage_use_verifier` | `{}` | Spend 1 power in Records and identify the instrument's candidate |
| `stage_open_service_hatch` | `{}` | Spend 2 power in Plant and open the Plant–Vault edge |
| `stage_recover_candidate` | `{"candidate_id":"<candidate>"}` | Carry or exchange a candidate in the Vault |
| `stage_extract` | `{}` | Resolve extraction in the Atrium |
| `stage_wait` | `{}` | Spend a turn without moving |
| `commit_turn` | `{}` | Commit the currently staged action |

The locations are `atrium`, `records`, `conservation`, `plant`, and `vault`.
Open edges connect Atrium–Records, Atrium–Conservation,
Records–Conservation, and Records–Plant. Conservation–Vault is closed in this
scenario. Plant–Vault opens after a committed service-hatch action.

Extraction on turn 16 resolves before exhaustion. Its factual outcome is one
of `success`, `wrong_ledger`, or `no_ledger`; any other sixteenth committed
turn ends with `exhausted_inside`.

## Participant projection

Every participant projection has the same bounded shape, including during
briefing:

```text
phase, objective, location, turns_used, turns_remaining, power,
gates, map, candidates, staged_action, carried_candidate,
verifier_result, outcome
```

`staged_action` is `null` or an exact staged action object with
`action_type`, `turn_cost`, `power_cost`, and its required destination or
candidate. `verifier_result` is `null` or
`{candidate_id, confidence:"verified"}`. `outcome` is `null` or a one-key
object naming one of the four terminal outcomes. Candidate IDs and visible
attributes do not identify the authentic ledger by themselves.

## Build and proof

From the repository root with the repository's Node 24 toolchain:

```sh
pnpm --filter @worldstream/midnight-archive pack:check
pnpm --filter @worldstream/midnight-archive pack:test
pnpm --filter @worldstream/midnight-archive pack:build
pnpm --filter @worldstream/midnight-archive pack:inspect
WORLDSTREAM_PACK_HOST=target/debug/worldstreamctl \
  pnpm --filter @worldstream/midnight-archive pack:prove
```

The golden corpus executes the documented ten-turn powered route as 20
stage/commit Actions. Focused tests also cover briefing and exact start,
restaging, closed-gate traversal, replay, malformed and illegal actions,
companion rejection, wrong/no-ledger extraction, exhaustion, turn-16
extraction, and projection privacy.

The retained 0.1.0 bundle is
[`worldstream-midnight-archive-d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b.wspack`](releases/0.1.0/worldstream-midnight-archive-d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b.wspack).
Its physical bundle digest is
`blake3:d14e21273d58d1c0d1cc1b5bd0c002975a531b118bfe0c65bfa33a3efadcf85b`,
its semantic revision is
`blake3:679022bf13c15ea014e18a7129b679c9bfd27873c570fd9cd0c02818f0880a7a`,
and its Component digest is
`blake3:9b86dcd4173aad23167e01cfd936a91d95396fc6faf02220cdac8046fdb22783`.
The production proof is retained in
[`evidence/production-proof-0.1.0.json`](evidence/production-proof-0.1.0.json).
