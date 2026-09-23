# Local Tower of Hanoi Community demo

This retained local experiment provisions one external `solver` Membership and
one external Runner credential for each Participant, plus an actionless
participant `observer`. The harness never selects or submits a puzzle Action.
Each long-lived local supervisor receives its own paired credentials, claims
Pack-issued Runner Activations, and starts a bounded provider invocation. The
baseline supervisor uses a fresh snapshot; the comparison supervisor uses the
claimed Invocation Context at the exact Room Head plus a persistent, ACKed
observation cursor. In both modes the Participant chooses whether to make a
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

For repeated local iteration, `--fast-start` skips the Rust build and the
offline Pack golden-corpus proof, and reuses one disposable Component
compilation cache under `target/worldstream-component-cache`. A configured
Wasmtime cache makes every process that admits the Pack (proof,
restart-readiness, and `worldstreamd` startup) read the compiled Component
instead of recompiling it; the first run populates the cache and later runs
reuse it. Original Component bytes remain authoritative and the cache is
disposable, so a missing or mismatched cache only recompiles. Pass an explicit
`--component-cache-dir` to choose the directory, or `--skip-pack-proof` alone to
keep the build while skipping proof. The two-room comparison prototype always
launches its children with `--fast-start`.

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

## Two-room comparison prototype

The V2 comparison adapter starts two independent local Runs behind one loopback
UI. The left Room uses the selected Codex or OpenRouter model; the right Room
uses a TypeSafe JEV Participant. Both Rooms receive the same disk count and a
shared epoch start barrier. The current solver roster remains configurable per
side (the default is five seats), and each side keeps its own Participant quorum.
Comparison children default to `--context-mode stream`; standalone harness runs
retain `--context-mode snapshot` as the baseline.

Start the prototype with the local Python environment:

```sh
sdk/python/.venv/bin/python -m examples.tower_of_hanoi.comparison_canvas
```

Open `http://127.0.0.1:5191/`, or `/v1/` on the comparison v2 server. The UI
keeps the provider keys in the server process, offers Codex aliases separately
from verified OpenRouter IDs, and shows the two sanitized public feeds side by
side. Its OpenRouter dropdown uses the same live cheap/fast catalog as the
comparison v2 server. Add `?variant=1`, `?variant=2`,
or `?variant=3` to compare the three interaction layouts; the floating arrows
and keyboard arrows switch layouts as well. A comparison run requires the
prebuilt local WorldStream binaries and uses `JEV_API_KEY` plus either
`OPENROUTER_API_KEY` or the repository's existing `openouterkey` in `.env` for
the corresponding provider.

## JEV-judge comparison

`examples.tower_of_hanoi.comparison_canvas_v2` runs the same comparison layout
with one shared model for both Rooms: the left Room runs the model alone and the
right Room runs the exact same model plus a JEV verifier. The verifier scores
each proposal with one atomic Noul ("does this best advance the objective given
the board, recent moves, and cycle history?") and, when the score is below the
threshold, the proposer gets exactly one repair turn that carries the verifier's
note. The judge only scores the proposer's candidates; it never selects from the
open set. Both Rooms share one seed and one epoch barrier, and the default
roster is a single solver per Room.

```sh
sdk/python/.venv/bin/python -m examples.tower_of_hanoi.comparison_canvas_v2
```

Open `http://127.0.0.1:5291/`. The retained v1 comparison prototype (`LLM Room`
vs `JEV Room`) is hosted in the same process at
`http://127.0.0.1:5291/v1/`, so both experiments can be recorded from one
origin. The model dropdown is populated live from the
OpenRouter catalog, filtered to inexpensive text-only models (prompt at or below
`$1/M` and completion at or below `$3/M`). Known fast, instruction-following
models at that ceiling are surfaced first with a `★` prefix; the rest follow by
price. Free (`:free`) endpoints are excluded because account guardrails and
zero-data-retention settings frequently reject them with an HTTP 404 at request
time. OpenRouter requests ask for the lowest-latency provider
(`provider.sort = "latency"`), because a cheap model routed to a slow upstream
can take tens of seconds per reply. Provider calls are bounded (30s OpenRouter,
20s JEV) by a hard wall-clock deadline, and a failed activation turn is retried
until the run deadline with capped exponential backoff and jitter, so an
upstream outage cannot stall a Room. After three failed attempts the solver
falls through the shared fallback model chain (`mistral-small`,
`ling-3.0-flash`, `llama-3.1-8b`). Both Rooms read one shared ladder file
(`active-model.json` in the comparison directory), so a promotion on either
side moves both and the comparison keeps model parity; the index only ever
advances and is applied once per step even if both sides request it. A provider
failure is recorded as `provider_request_failed_<status>`
or `provider_request_timeout`, and an unusable HTTP 200 reply is recorded as
`openrouter_choice_missing:<reason>`. The right Room's
decision trace shows the judge engine, probability, and whether the proposal was
approved or repaired, and the event feed renders a `judge` row per verdict. A
JEV judge necessarily adds one JEV call per turn and up to one extra proposer
call when it repairs.

The retained comparison prototype is unchanged and still runs on port `5191`.

## Shared solver contract

Both Rooms receive one external contract. WorldStream supplies the authoritative
board at the claimed Room Head, the objective, the currently offered actions,
bounded recent history, previous own decisions, cycle hints, and any open
completion claim with quorum status. Each solver returns at most one decision:

```json
{
  "based_on_room_seq": 18,
  "action": "move_disk | post_completion_claim | assess_claim | wait",
  "payload": {},
  "rationale": "optional telemetry",
  "reason": "progress | break_cycle | uncertain | abandon"
}
```

The OpenRouter/Codex LLM answers with that decision object. JEV receives the same
state as one closed Choice problem (`move:A>B:1`, `post_completion_claim`,
`assess:endorse`, `wait`, ...) and returns one choice with confidence and
probabilities. JEV never answers a separate completion question: completion is
the `post_completion_claim` choice and uncertainty is `wait`. WorldStream still
owns authorization, legal-move validation, exact Room Head binding, stale-action
rejection, claim recording, quorum, and durable cursors; each solver owns
strategy and completion judgement.

The harness does not pick or filter moves. Every legal move stays offered, and
each one carries history facts in `choice_history`: `visits` (how many times the
resulting board was already seen), `repeats_recent` (played in the bounded recent
window), and `undoes_last` (reverses the most recent move). `recent_moves`,
`cycle_hint`, and the resulting-state visit counts are also in the shared state
and in the JEV choice descriptions, so the agent can notice a cycle and choose to
break it, try a different move, `wait`, or claim. `target_reached` is a computed
fact that the public objective board is assembled; when it is true the claim
choice is labelled accordingly, but the agent still decides. `reason` is optional
telemetry and never changes protocol behaviour. The Pack still never decides
whether the board is solved; the claim and its quorum do. There is no
"unsolvable" state for 3-peg Hanoi; a looping room is a cycle, not an unreachable
goal.

## Serialized local action grants

The local harness has no gateway that serializes Action grants, so with several
solvers every board change wakes all of them and only the first submission per
Room Head is accepted; the rest are stale. In a measured five-solver run,
roughly three quarters of submissions were stale. The harness now elects one
acting member per `room_seq` with a shared, leased grant file
(`evidence/action-grant.json`) under an `flock`. A solver that sees a superseded
offer, or loses the grant, releases its claimed Activation without a provider
call; a solver that advances the Room marks the grant acted so no peer retries
that head. Offers are chosen newest-first by `cause_room_seq`. The grant is a
local scheduling convenience only: elected Actions still pass ordinary Pack
admission and may be rejected. On platforms without `fcntl` the grant is a no-op.


## Randomized starting board

To solve from a mid-state instead of the full A tower, pass `--randomize-board`
(optionally `--board-seed N`) or `--initial-board '{"A":[3],"B":[2,1],"C":[]}'`
to the harness. The Pack validates the reviewed `initial_board` configuration,
so the board still originates from WorldStream. The comparison UI exposes the
same option; both Rooms share one seed so the experiment stays fair.

## Per-Room controls

Each Room header in the comparison UI has `Pause`/`Resume` and `End room`.
`POST /pause`, `POST /resume`, and `POST /end` take `{"side":"left"|"right"}`
and act on that Room's child process group only. A paused Room is frozen with
`SIGSTOP` so the peer Room keeps running; ending a Room terminates only its
process group and marks that side `ended`. Pause is best-effort: a frozen Room's
Activation leases can expire, so use `End room` when it must not recover.

The event feed exposes the last 100 relayed events per Room with a `Copy feed`
button and a `Download full` button (`GET /feed?side=left|right`) that returns the
complete retained run history as JSON; the count shows `shown / total`.
Participant seats use unique display names instead of opaque `left-solver-1` ids.
When a Room's completion is accepted, the relay stamps `completed_at_ms` and
publishes `elapsed_ms` from the shared start barrier, shown next to the Outcome
as `accepted · mm:ss`. When **both** Rooms have accepted completion the run ends:
remaining child processes are reaped and the shared clock freezes at
`timer.status = "complete"` with a final `elapsed_ms`, so the comparison stops at
the finish instead of ticking to the time limit.
The relay is an open race: a move wakes every solver in the Room and only the
first submission per Room Head is accepted, so the event mix can favour the
seat that keeps winning the race. The relay does not schedule turn order.
