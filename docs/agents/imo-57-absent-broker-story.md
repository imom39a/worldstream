# IMO-57 absent-Broker story evidence

This note records the deterministic, offline reference story in
[`examples/heist/`](../../examples/heist/). It is evidence for the public
Agent Heist behavior, not a live server or storage integration test.

## Scope and authority

The configured GitHub identity could not fetch the IMO-57 issue: both `gh
issue view 57 --repo imom39/worldstream` and the GitHub connector returned
repository/issue not found. The implementation therefore follows the supplied
IMO-57 request, `docs/requirements.md`, `docs/activity-packs.md`,
`docs/protocol.md`, `docs/ui-architecture.md`, the current public Rust Heist
reducer/registry, and `sdk/python/src/worldstream_sdk/client.py`.

The harness uses only Python's standard library. Its labeled SHA-256 hashes
cover sorted UTF-8 canonical JSON and are evidence hashes for this fixture;
they do not claim to be the Rust pack's BLAKE3 revision digest.

## Story

The explicit fixture is `service_window`:

```json
{"entry_window":"early","extraction":"boat","fixture_id":"service_window","required_tool":"thermal_key","route":"service"}
```

Navigator is cooperative: it inspects/publishes its route clue and proposes
the complete correct plan. Insider is cautious: it inspects its entry-window
clue, endorses the plan, and contributes the required resource. Broker is
withholding/absent as a runner. Its immutable enabled seat remains in the
three-seat denominator and its commitment is missing.

The six public phases are traversed in order:

```text
Briefing → Negotiation → Commitment → Resolution → Result → Complete
```

The current reducer therefore yields the explicit expected outcome:

```json
{
  "checks":{"entry_window":true,"extraction":true,"required_tool":true,"resource_contributed":true,"route":true},
  "missing_roles":["broker"],
  "outcome":"success",
  "reason":"scored_selected_plan",
  "score":5,
  "vote_counts":{"01ARZ3NDEKTSV4RRFFQ69G5FNP":2}
}
```

This is the public Heist rule: two matching commitments are a strict
two-of-three majority; a missing seat does not improve the denominator or
synthesize a commitment.

## Activation and recovery evidence

- A plan proposal emits `endorsement_requested` and creates the first Broker
  Activation. Broker's first Invocation ends before Commitment without a
  participant Action.
- The Commitment-opening timer at `room_seq 7` emits exactly one fresh
  `commitment_opened` Attention for Broker. Its claimed context is hashed and
  contains the exact head at sequence 7, Broker's authorized projection,
  projection hash, `inspect_clue`/`commit_move` Action Offers, membership,
  integrity, policy, authority, delivery, budget, deadline, and artifact
  witnesses.
- The claim has runner-control authority only. The separately authorized
  Insider `commit_move` uses `participant-action-capability`; it is not a side
  effect of claiming the Activation.
- Operational evidence derives and asserts duplicate claim, lost claim reply
  retry, lease expiry/reclaim, stale lease generation, and idempotent
  completion. A duplicate Action and stale timer generation are also recorded
  as no-new-transition outcomes.
- `kill_before_commit` records no new head/receipt. The
  `kill_after_commit_before_publication` marker records recovery by resolving
  the original committed result and publishing the existing transition.
- Result deadline completes the story with Broker still missing. A later
  `round_result_available` Attention may remain pending; no absent runner is
  treated as having acted.

## Replay and limits

The final Replay folds the initial state and all 15 ordered transitions with
the same small reducer, verifies every transition hash and the final head, and
uses zero snapshots. Replay is read-only and creates no Activation. The
transcript separates canonical transitions from operational activation and
recovery evidence.

This harness does not claim live SQLite/PostgreSQL/server/browser execution,
transport concurrency, real power-loss durability, cryptographic equivalence
to the Rust implementation, or paid-model execution. It is deterministic
protocol/story evidence only.

## Reproduction

From a fresh checkout:

```sh
python3 examples/heist/run_story.py --self-test
python3 -m unittest discover -s examples/heist -p 'test_*.py'
```
