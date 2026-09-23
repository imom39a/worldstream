# Agent Swarm parallel coding challenge

This experiment tests the application on native macOS using its real local
Controller, Room Runtime, execution daemon, coordinator and guarded worker
processes. The workers produce explicitly supplied deterministic code. This
measures coordination and execution overhead; it does not measure model coding
quality, prove general goal-to-result autonomy, or qualify an external provider.

The challenge found a queued-work launch defect and a macOS control-socket
race, leading to corrections with regression coverage. It also exercised
parallel workers through accepted, reviewed Results. Creating a goal generated
zero ordinary Work Items: the test driver supplied the plans.

## Final measured result

The final managed run passed on 2026-09-22 after both fixes. It took 321.331
seconds including shared startup and cleanup. Both trials accepted candidate
version 3 at Room sequence 30, with identical final artifact bytes.

| Measurement | Serial, cap 1 | Parallel, cap 2 |
| --- | ---: | ---: |
| Two coding Work Attempts, through accepted Contributions | 10.924 s | 7.855 s |
| Whole trial, including setup and fault/recovery steps | 136.434 s | 130.223 s |
| Maximum observed native workers running | 1 | 2 |
| Final behavioral checks passed | 16/16 | 16/16 |
| Supplied worker plans | 14 | 14 |
| Scripted Human Actions | 20 | 20 |
| Explicit scripted reconciliations, included above | 2 | 2 |
| Ordinary Work Items generated from the goal alone | 0 | 0 |

The measured coding-phase ratio is **1.391×**. Each cap has one final trial,
with deliberate fixture sleeps; this is not a live-model performance result
or a statistically established speedup. Most whole-trial time is outside the
two parallel coding steps, so more worker capacity alone cannot remove that
sequential cost.

Both trials rejected premature integration and the defective candidate,
retained a blocking finding despite a later passing review, preserved the
concurrent human edit, rejected conflicted and stale acceptance, and accepted
the revalidated third candidate. The native checker output was independently
read back: candidate 1 passed 15/16 cases and failed `round_trip`; candidates
2 and 3 passed 16/16. Managed-process cleanup was proven.

Evidence: [final report](results.json), [six native checker outputs](behavioral-checks.json),
and [run metadata and binary/source identities](run-metadata.json). The latter
also pins the daemon executable containing the socket fix. An
[earlier passing run](results-run-1.json) is retained as historical evidence;
the table above uses the final binaries and final challenge script.

## Question and workload

Can two independent coding Work Items execute concurrently, publish exact
Contributions, and reach a reviewed Result without bypassing dependencies or
overwriting a newer shared resource?

Use a small CSV transformation task with independent parser and formatter
Contributions, followed by integration, behavioral checks and independent
review. Run the same work with provider capacity one and two. Record the
controlled delay separately from observed wall time. Require observed overlap
for the parallel run and enforce the cap in both; report speedup without
assuming a minimum, since application overhead can dominate short tasks.

Each coding Invocation includes 1,500 ms of artifact-fixture delay plus the
existing controlled adapter's 250 ms delay: 1,750 ms total deliberate sleep.
The report's `synthetic_delay_ms_per_coding_turn` records the 1,500 ms custom
artifact delay. The ratio therefore describes this synthetic workload only.

The test driver supplies plans and expected code. Count that assistance
explicitly. Run a deliberately defective candidate through the checker before
testing its corrected version, and preserve an injected concurrent edit during
write-back. Retain final artifact digests and exact Room Result provenance.

The driver supplies the corrected code, merges the preserved note into the
third candidate, and opens separate protected code-change state for each
candidate version so the new resource baseline is captured. These are scripted
reconciliation steps, not model-generated fixes or automatic rebasing. Ordinary
worker Contributions, checks, review records and acceptance still cross the
actual application and Room boundaries.

## Baseline inspected and run

On 2026-09-22, the focused Rust baseline passed 36 tests across `coordinator`,
`execution_daemon`, `shared_capacity`, `code_change`, `code_change_service` and
`reviewed_code_change`. The Swarm Pack passed all 25 tests and conformance.
The Pack was run with Node 25.9.0, which differs from the declared 24.18.1;
these results do not certify the declared Node environment.

The existing full managed smoke also passed in 245.471 seconds, exercising
review, recovery, shared capacity, budgets, stale output and conflict handling
across two Rooms. Managed-process cleanup was proven. Its
[receipt](managed-baseline.json) predates the queued-work correction; the new
challenge and coordinator regression tests validate that correction.

The local provider preflight found Codex 0.149.0 and Claude Code 2.1.112; Kiro
was absent from PATH. No genuine Swarm provider qualification evidence was
found in the repository. The existing provider admission gate remains intact.
No provider model calls or subscription credential reads were performed.

## Defects found and corrected

The first substantive native run failed after 71.554 seconds with capacity one.
Both independent coding plans were staged at Room sequence 6. The parser's
Contribution advanced the Room to sequence 7; the queued formatter had unchanged
work and attempt revisions, but the coordinator refused to launch it with
`AuthorityChanged`. The pre-launch check required the whole Room Head to remain
unchanged. See [the sanitized failure evidence](queued-work-before-fix.json).

The coordinator now permits an exact owned Work Attempt to launch after an
unrelated Room update. It still checks ownership, work and attempt revisions,
execution epoch, open phase, current provider/model/effort selection, and the
offered Action schema. It prepares the Invocation from a fresh authorized
observation. Relevant unresolved blockers, resource conflicts and escalated
problems prevent launch even if they did not change the Work Item revision;
missing or malformed decision data also prevents launch. Resource-basis
freshness remains enforced by the ordinary Contribution and acceptance rules.

A focused regression failed before this change and passed after it. This is a
development feedback loop driven by a repeatable test and code review; it is
not evidence that the Swarm autonomously diagnosed or rewrote its own software.

Review of that fix caught an additional boundary case: a Direction targeting
several Work Items has a legitimate `null` blocker `work_id`. The launch check
now follows the exact Work Item's `blocker_ids` for that case, so a Direction
affecting A and B does not stop independent C. Regression tests cover both
unresolved and resolved Directions, plus rejection of an affected Work Item.

A later managed repeat failed while launching the independent reviewer. The
retained request worked when replayed, including 100 consecutive native
launches, so simply repeating immediate requests did not reproduce the fault.
A targeted probe connected a valid local client, waited 150 ms, and then sent
its authenticated frame. That reliably produced `invalid_request`: on this
Mac, the accepted socket inherited the long-lived listener's nonblocking mode.
The daemon tried to read before the client wrote and treated `WouldBlock` as
an invalid request. Existing tests used `serve_once`, which switched the
listener to blocking mode and missed this production-path race.

The daemon now explicitly puts each accepted control socket in blocking mode
before applying its existing five-second IO timeout. The listener remains
nonblocking. A regression exercises delayed frame delivery through the actual
long-lived daemon loop. This was an execution-control fault outside accepted
Room transitions; Canonical History alone could not explain it. The original
wire bytes were not retained, so the delayed-frame probe establishes the
concrete transport defect rather than reconstructing the original packet.
See [the failure and reproduction evidence](reviewer-launch-before-fix.json).

## Other improvements

- A repeatable managed challenge with machine-readable measurements.
- A bounded controlled-worker instruction for exact code artifacts, distinct
  from real model output, with negative tests for malformed or excessive input.
- Stronger Work Item tests: a current-revision competing claim cannot replace
  an owner, two independent items can be claimed simultaneously, integration
  waits for both, and unrelated work can finish while one branch is blocked.
- README and TUI disclosure that creating a goal alone does not start ordinary
  agent work; explicit worker plans are still required.
- Bounded failure snapshots capture daemon status and selected coordinator
  state before cleanup, without retaining prompts or credentials in the
  diagnostic snapshot. Capture failure preserves the original test error.

## Validation

The final coordinator suite passes all 9 tests, including an 18-case changed
or malformed authority matrix. All 10 daemon tests, the 6 TUI tests and 2
controlled-artifact tests also pass. The managed-harness/challenge Python suite
passes 16 tests; Ruff and Rustfmt checks pass. Scoped Clippy for the application
library, controlled
worker and changed Rust tests passes with `--no-deps -- -D warnings`.

The earlier Clippy invocation that included dependencies stopped on existing
`worldstream-core` warnings. This task does not claim a clean repository-wide
Clippy gate or compatibility certification for another platform.

## Reproduce on native macOS

From the repository root, with the existing 0.2.0 Pack bundle available:

```sh
cargo build --locked -p worldstream-agent-swarm --features managed-local-runtime --bins -p worldstream-server -p worldstream-studio-supervisor
challenge_root="$(mktemp -d "${TMPDIR:-/tmp}/swarm-challenge.XXXXXX")"
uv run --project sdk/python --python 3.14.7 python scripts/verify-agent-swarm-managed.py \
  --workspace "$PWD" --root "$challenge_root" \
  --pack packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack \
  --challenge-report .scratch/swarm-challenge-report.json
```

Successful publication requires both trials and managed-process cleanup to
succeed. The report retains executable/script identities and exact Result
provenance. The isolated runtime directory contains private local authority
state; the JSON report is the shareable evidence, not a copy of that directory.

## Next improvements to measure

1. Qualify one real subscription CLI provider through the existing admission
   contract, including exact effective model/effort evidence and process cleanup.
   Controlled receipts cannot satisfy that qualification.
2. Add an ordinary planning driver: observe the accepted goal, propose bounded
   Work Items and dependencies, stage eligible worker plans, then integrate,
   check and independently review the produced artifacts. Count supplied plans
   and human interventions so a successful scripted run is not called autonomy.
3. Add bounded replanning for genuinely invalidated work. Preserve the stale
   attempt and its evidence, create a fresh authorized plan, and escalate after
   a configured recovery limit. This complements the unrelated-update fix.
4. Evaluate JEV in shadow mode against labeled candidate/evidence pairs, as
   allowed by [ADR 0043](../../adr/0043-use-jev-as-an-application-layer-swarm-advisor.md).
   Measure missed defects and false alarms before using its
   advice to prioritize ordinary review. Deterministic checks and independent
   review remain acceptance requirements.

After provider qualification and the planning driver exist, reuse this task
with worker-written code and hidden behavioral cases, then expand to several
unseen coding and analysis tasks. Run repeated matched serial/parallel trials
with the same goal, provider selections and budgets. Measure accepted-result
correctness, erroneous acceptance, elapsed time, model usage, intervention
counts and recovery outcomes. Keep a held-out set so changing the planner to
pass one visible task does not masquerade as general improvement.
