# Local Tower of Hanoi Community demo

This retained local experiment provisions one external `solver` Membership and
one external Runner credential for each Participant, plus an actionless
participant `observer`. The harness never selects or submits a puzzle Action.
Each long-lived local supervisor receives its own paired credentials, claims
Pack-issued Runner Activations, and starts a bounded Luna invocation. That
Participant snapshots the current Projection and chooses whether to make a
legal move, post a completion claim, or assess the open claim.

The public objective is to move the tower from A to C. It guides Participants
only. The Pack does not compare the board with that target and cannot decide
whether a claim is correct. Completion means only that a strict majority of the
claim's snapshotted eligible solver electorate accepted it.

Build and prove the candidate before starting a Room:

```sh
pnpm --dir packs/tower-of-hanoi pack:check
pnpm --dir packs/tower-of-hanoi pack:test
pnpm --dir packs/tower-of-hanoi pack:build
pnpm --dir packs/tower-of-hanoi pack:inspect
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  pnpm --dir packs/tower-of-hanoi pack:prove
```

Run the five-Participant, four-disk local canvas demo:

```sh
sdk/python/.venv/bin/python -m examples.tower_of_hanoi.local_harness \
  --community-demo --canvas-demo-hold-seconds 30
```

The command prints a stable `http://127.0.0.1:.../` URL after the Room,
Genesis replay, and loopback observer are ready. It keeps serving after the
outcome; omit `--canvas-demo-hold-seconds` to inspect it until `Ctrl-C`.
The 300-second gameplay window begins only at that point. Build and proof time
are reported separately from gameplay elapsed time.

`--community-demo` provisions `solver-a` through `solver-e`, defaults to four
disks, and uses medium-effort Luna. `--solver-effort low` is available for a
lower-cost experiment. `--disks` accepts 1 through 10 and overrides the
community default, including `--community-demo --disks 10`. The neutral
`--move-limit` safety cap defaults to 10,000 and is not a predicted path
length or a completion condition. The wall clock returns `unresolved` with
`demo_time_limit` if Participants do not reach their own accepted completion.

The loopback canvas holds one actionless observer credential in its own
process. It streams bounded public board state, Room sequence, delivery cursor,
work revision, current claim, labeled assessments, pending review attention,
outcome, countdown, solver labels, and sanitized supervisor receipts through
reconnectable SSE. It sends no Room/member ID, bearer, Runtime URL, raw frame,
credential file, or next-action guidance to the browser.

Every Runner Activation has `board_changed` or `claim_review_requested` from
the Pack. A disk move advances `work_revision` and supersedes the current
claim. A claim snapshots its eligible solver electorate and majority threshold;
assessments bind both work revision and claim round. The supervisor renews its
30-second lease while Codex runs and kills the invocation process group on
lease loss or timeout. Stale heads are normal: the Participant refreshes and
decides again within its bounded invocation.

Run focused checks without a Runtime:

```sh
sdk/python/.venv/bin/python -m unittest \
  examples.tower_of_hanoi.test_local_harness \
  examples.tower_of_hanoi.test_live_canvas
sdk/python/.venv/bin/python -m ruff check examples/tower_of_hanoi
```

The demo demonstrates independent Participant actions, legal move enforcement,
claim review, stale-head fencing, public observation, and replay equivalence.
It does not prove that Participants agreed on a mathematical solution or cover
agreement/defer policy or branch-merge semantics beyond the Pack's direct
claim mechanics.
