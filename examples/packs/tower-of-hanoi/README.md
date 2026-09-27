# WorldStream Tower of Hanoi

This deterministic WASI-free Activity Pack governs a shared three-rod board,
legal disk moves, bounded move safety cap, solver attribution, and a
Participant-owned completion-claim protocol. The public objective is to move
the tower from A to C. It is guidance for Participants, never a reducer rule:
the Pack contains no path, next-move policy, target-board comparison, or
mathematical completion predicate.

The configuration accepts 1 through 10 disks and a neutral `move_limit` up to
10,000. Eligible completion voters are enabled agent Participants in the
`solver` role. The Pack supports 1 through 16 such Participants, matching the
current host's maximum Attention fanout. A community run fixes five seats.

A solver may submit:

- `move_disk`, a normal legal move. It increments `work_revision` and clears
  an open claim and all assessments.
- `post_completion_claim`, anchored to the observed `work_revision`. It cannot
  overwrite another open claim at the same revision. The claim snapshots the
  eligible electorate and its strict-majority quorum.
- `assess_claim`, anchored to both `work_revision` and `claim_round`, with
  `endorse`, `challenge`, or `defer`. Each reviewer has one latest assessment;
  the claimant is already counted and cannot assess its own claim.

The claimant plus endorsements meeting the snapshotted quorum transitions the
Pack to `complete` with `participant_accepted_completion`. This only records
Participant acceptance. A target-looking board remains `solving` unless the
claim mechanics accept it.

Legal moves emit coalescible `board_changed` Attention to every current
eligible solver, including the actor, so an N=1 room can receive later work.
A posted claim emits `claim_review_requested` Attention to snapshotted eligible
reviewers that have not assessed it. Claims leave `move_disk` offered.

## Check and build

```sh
pnpm install
pnpm --dir examples/packs/tower-of-hanoi pack:check
pnpm --dir examples/packs/tower-of-hanoi pack:test
pnpm --dir examples/packs/tower-of-hanoi pack:build
pnpm --dir examples/packs/tower-of-hanoi pack:inspect
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  pnpm --dir examples/packs/tower-of-hanoi pack:prove
```

`pack:build` writes the candidate Bundle to
`releases/0.1.0/worldstream-tower-of-hanoi-candidate.wspack` and records the
semantic revision and component digests in `.worldstream/build-receipt.json`.
Use `pack:inspect` to capture the physical Bundle digest before operator
approval and selection. `pack:prove` needs the production-host CLI and writes
the successful evidence to `.worldstream/first-success-receipt.json`.

## Evidence and visibility

The conformance fixture is one arbitrary legal move followed by a claim and
assessments on a non-goal board. It proves legal state transitions and
Participant quorum mechanics without encoding a Hanoi solution. Public
projections expose board state, objective, cap, work revision, claim summary,
assessments, quorum, outcome, and contribution attribution. The private claim
electorate remains Pack state; a browser adapter replaces member IDs with
configured labels before emitting its bounded loopback DTO.

The local supervisor integration lives in
[`examples/tower_of_hanoi/`](../../tower_of_hanoi/). It pairs each
solver Membership with its own external Runner and never introduces a central
turn controller.
