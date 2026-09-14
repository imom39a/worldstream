# Community Hanoi: a one-day WorldStream scale experiment

Status: Non-normative analysis and prototype brief, 2026-09-12. This document
does not change the accepted Hosted Activity Platform formation contract or
the WorldStream protocol. The accompanying HTML is a throwaway state-model
prototype, not a Runtime implementation.

Follow-up: the [2026-09-13 kernel readiness audit](community-hanoi-kernel-readiness.md)
updates the implementation findings, recommends distinct guest identities for
the final showcase, and corrects the claim that the 15-disk workload necessarily
exceeds 100,000 Transitions. The fixed-slot approach below remains a prototype
option rather than a kernel requirement.

## Decision

Build the first experiment as one 24-hour Room with a 15-disk Towers of Hanoi
Activity Pack Revision, at most 100 connected guest controllers, and an
unbounded-in-principle but operationally capped waiting queue. Anonymous people
watch through the existing Public Projection Relay. Agents receive a bounded
current Projection and one exact Action Offer; they do not ingest the Room's
Replay.

Use 100 fixed `solver` Memberships at Genesis for the first hosted experiment.
Each admitted guest temporarily controls one solver through a short-lived
gateway lease. A queued guest can take a released solver after its previous
controller disconnects. WorldStream attribution remains the stable solver
Membership, while the guest handle is operational presentation data labelled
`External agent -- unverified`.

This is the smallest design that preserves a bounded Core Room State and keeps
the Room's roster fixed before Genesis. It still needs a narrow, reviewed
controller-lease extension to the Hosted Platform: today's House Agent and
browser-agent contracts do not allow arbitrary controllers to rotate over one
Principal. It is not the final identity model. If the product must canonically
attribute every guest, add a separately designed post-Genesis
membership-admission contract; do not hide that feature inside the Pack or
gateway.

## Why 15 disks is the useful test

The minimum solution length for `n` disks is `2^n - 1` moves. Fifteen disks
require 32,767 committed puzzle moves. A first-to-ahead vote margin of three
requires at least three accepted vote Actions for every committed move:

```text
32,767 moves × 3 accepted votes = 98,301 accepted Actions
```

Join, timer, completion, and occasional failed-round Transitions take the
Room beyond 100,000 Transitions in a day. The minimum average rate is about
1.14 accepted vote Actions per second, or 0.38 committed moves per second.
That is an honest stress target for ordering, persistence, projection, and
late attachment without incurring the 3,145,725 minimum accepted votes that
a 20-disk run would require.

This choice adapts the result in Meyerson et al.'s **MAKER: Solving a
Million-Step LLM Task with Zero Errors**. The reported 20-disk run completed
1,048,575 dependent moves by reducing the problem to single-move proposals,
sampling multiple outputs, discarding malformed responses, and accepting a
candidate only when it was ahead by a configured vote margin. The large run
used GPT-4.1-mini with margin three. The paper supplies the decomposition and
known puzzle algorithm, so it is evidence for reliable execution of tiny,
locally checkable steps rather than evidence that agents can discover an
open-world million-step plan. See the [paper](https://arxiv.org/html/2511.09030v1)
and the repository's [research note](long-horizon-agent-papers-research.md#5-maker-solving-a-million-step-llm-task-with-zero-errors).

The Pack should exploit the stronger property available here: it can verify a
Hanoi move deterministically. Voting measures how reliably independent agent
controllers propose the correct next move and filters model errors. It is not
distributed-system consensus and it does not create puzzle truth. WorldStream
still provides the one canonical Transition order.

## Boundary of each component

| Component | Owns | Must not own |
| --- | --- | --- |
| Hanoi Activity Pack | Puzzle board, round, vote counts, legal moves, deadline, completion, Outcome | Queue, sockets, model calls, credentials, persistence |
| WorldStream Runtime | Memberships, Action admission and receipts, ordered Transitions, authorized views, observation delivery, Replay | Puzzle-specific scheduling, public discovery, guest identity claims |
| Community gateway | Queue, connection cap, short leases, rate limits, serialized action grants, unverified guest handles | Move legality, vote result, Room state, Outcome |
| External Runner/controller | Model call, prompt budget, proposal selection, private memory | Canonical state, acceptance decision |
| Public Projection Relay | Republishing the authorized Public Projection | Participant credentials, private views, Actions, Replay |
| Canvas client | Rendering the Public Projection and gateway health | Reconstructing authoritative state locally |

The public canvas must render the Public Projection received from the relay.
Queue length, connected-controller count, and lease health are gateway metrics
and should be clearly presented as operational data rather than Activity State.

## The primitive loop

One contribution should have this shape:

```text
1. A controller holds a short gateway lease for solver-037.
2. It reads one current Projection Reset or replacement Projection.
3. The Projection exposes board, round, move number, goal, and its vote status.
4. The Action Offer exposes the exact `propose_move` payload schema.
5. The controller submits one proposal against the current Room Head.
6. WorldStream returns an accepted, duplicate, stale, rejected, or fault receipt.
7. The Pack counts an accepted vote. At margin three it deterministically
   validates and applies the winning move in that same Transition.
8. The next current Projection replaces the prior one in model context.
```

The gateway issues at most one current-head action grant at a time. Otherwise,
100 controllers can all read the same Head, race to submit, and cause one
accepted Action followed by 99 stale receipts. Sequential grants preserve the
existing `based_on_room_seq` semantics while still allowing 100 agents to
remain connected, think, observe, and wait. A later protocol could add a
different dependency-scoped admission primitive, but this experiment should
measure the system we have.

The action is intentionally small:

```json
{
  "action_type": "propose_move",
  "payload": {
    "round_id": 23785,
    "from": "A",
    "to": "C"
  }
}
```

WorldStream supplies the Room, Membership, Action ID, and whole-Head basis in
the surrounding Action submission. The Pack checks phase, Role, round ID,
one-vote-per-Membership, directed peg pair, and current legality. Malformed or
out-of-round proposals are rejected and never counted.

## Bounded Pack data

A proposed Activity State is:

```json
{
  "phase": "running",
  "disk_count": 15,
  "pegs": {
    "A": [15, 14, 13],
    "B": [],
    "C": [12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
  },
  "move_number": 23784,
  "optimal_moves": 32767,
  "round": {
    "round_id": 23785,
    "basis_board_hash": "blake3:<digest>",
    "proposal_counts": {
      "A>B": 0,
      "A>C": 2,
      "B>A": 0,
      "B>C": 0,
      "C>A": 0,
      "C>B": 0
    },
    "voted_member_ids": ["solver-006", "solver-071"],
    "leader": "A>C",
    "runner_up_count": 0,
    "margin_required": 3,
    "status": "open"
  },
  "recent_commits": [
    { "move_number": 23784, "move": "B>C", "votes": 3 }
  ],
  "successful_rounds": 23784,
  "failed_rounds": 18,
  "deadline_at": "2026-09-13T16:00:00Z",
  "outcome": null
}
```

The real initial peg contains all 15 disks; the abbreviated example only
illustrates shape. `voted_member_ids` is bounded by 100 and is cleared at the
next round. `recent_commits` is a fixed-size window, such as 20 entries. The
Pack does not retain the queue, presence, full move list, cumulative raw
votes, transcripts, prompts, or per-guest profiles in Activity State. Those
facts either belong to canonical history, gateway operations, or the Runner.

The descriptor declares one participant Role, `solver`, with minimum and
maximum 100 for the fixed-slot experiment; one `propose_move` Action; timer
input for the 24-hour deadline and round timeout; bounded state, Projection,
observation, events, attention, collections, strings, and nesting. Existing
Pack role cardinality is expressed as `u32`, so the Pack model can declare
100. Current public product limits are lower: the Hosted contract caps seats
at 32, Studio room drafts cap seats at 64, and architecture currently lists a
default of 32 durable members per Room. A 100-seat qualification therefore
requires explicit reviewed limit changes and load evidence; it is not merely
a Pack configuration edit.

## Reducer logic

Conceptual logic for an accepted `propose_move` Action:

```text
require phase == running
require caller has enabled solver Membership
require payload.round_id == state.round.round_id
require caller has not voted in this round
require from != to and both pegs are declared

record caller and increment the candidate count
derive leader, runner-up, and their difference

if every eligible voter has voted and no candidate leads by three:
    close failed round; clear votes; open a retry for the same move

if leader is ahead by at least three:
    if leader is not the deterministic expected optimal move:
        close failed round; clear votes; open a retry for the same move
    else:
        apply the move
        increment move_number and successful_rounds
        append one bounded recent-commit record
        if all disks are on target peg:
            establish Outcome and enter terminal phase
        else:
            open the next round
```

The vote that reaches margin three also commits the puzzle move. A separate
Pack-internal `commit_move` Transition would add no information and inflate
the count. The deterministic check keeps the Room correct even when a wrong
candidate wins the sample. Failed rounds remain visible in canonical history
and a bounded current counter.

For the showcase, the controller's compact instruction can include the Hanoi
rule and ask for the next legal optimal move. A stronger experiment gives
controllers only the current board and goal; that tests planning as well as
execution and should be reported separately from reproduction of MAKER's
decomposed algorithm.

## Joining, leaving, and identity

### Recommended first experiment: fixed solver Memberships

At Genesis, create `solver-001` through `solver-100`. The community gateway
holds no generic operator authority and never gives a guest a reusable
WorldStream credential. It acts as the narrow Activity Client/Runner adapter
for a solver slot: an admitted guest receives a short gateway lease and can
only read that slot's authorized current view and submit the offered bounded
proposal through the adapter. The adapter submits with the slot's Action
authority. Disconnect fences the controller session and lease; the Membership
remains. The next queued controller receives a new lease. Its first read is a
complete current Projection Reset.

This yields constant Membership cardinality and stable per-Membership
observation streams. The cost is attribution: canonical actions prove that
`solver-037` acted, while the mapping from that slot to `guest-kestrel` is a
separate operational record. Runner/Invocation execution authority remains
separate from the Membership's Action authority. Public presentation must not
imply a verified model or durable guest identity. If controller-level
attribution matters for research, preserve signed gateway lease receipts as
non-authoritative study evidence with explicit retention and privacy policy.

### Later option: one Membership per guest

Give every admitted guest a new run-scoped Principal and Membership, mark the
old Membership Departed on exit, and admit a queued guest. This provides exact
canonical attribution. It conflicts with today's hosted fixed roster: ADR
0021 freezes the exact roster before Genesis and permits only the same
Principal to reconnect after Genesis. It also grows the retained Membership
map even though Departed memberships stop counting toward Pack role maximums.
Because current V1 Transitions carry resulting state, churn increases write
amplification as well as current-state size.

This option needs a new ADR that defines a narrow post-Genesis admission
operation, Principal creation, idempotency, ambiguous-result reconciliation,
credential issuance, privacy, total retained admission limit, and recovery.
The Hosted Platform may coordinate admission, but the Host must create the
canonical Membership. Neither a Pack Action nor a gateway-only row can do so.

## Late attachment and context budget

A fresh controller does not read 98,000 prior votes. It receives:

- a versioned static rule brief;
- a Projection Reset containing the complete authorized current board and
  open round;
- exact current Action Offers;
- a short activation reason or gateway grant; and
- no more than a small bounded window of recent commits for orientation.

A returning controller receives Catch-up after its Cursor when the retained
range is appropriate, otherwise a Reset. Replay is reserved for server
recovery, verification, and explicit historical inspection. These are the
accepted semantics in [ADR 0008](adr/0008-membership-observation-streams-and-reset-barriers.md).

The Runner replaces its prior current-state block after every observation. It
does not append complete projections to an ever-growing prompt. A useful
initial target is a Projection below 16 KiB, with a separate total model input
budget and reserved output budget. The Room can have 100,000 Transitions while
the model sees a few hundred tokens of current state.

This does not by itself make recovery O(1). The current SQLite activation path
can still reach full-history recovery. Add verified materializations and
bounded recovery checkpoints as separate Runtime work while preserving
canonical hash lineage and Replay correctness.

## Attention and scheduling

Do not emit 100 Attention Signals for every move. The gateway already knows
which guest connections are live and can offer sequential vote grants in
round-robin order. Registered WorldStream Runners can receive a small bounded
cohort of attention, such as five to nine solvers per round. Attention is a
wake-up hint; it does not grant Action authority or guarantee that a proposal
will remain applicable.

Controller diversity matters to the MAKER-style error model. One model call
copied 100 times does not create independent evidence. Record the configured
provider/model/prompt policy for an experiment and report correlation risks.
Do not claim a formal zero-error probability from empirical success.

## Public product surface

The one-screen client should resemble a simple itch.io game page:

- canvas board with 15 disks and visible progress;
- remaining day clock and current Room sequence;
- current vote leader, runner-up, and required margin;
- connected controller count, 100-slot capacity, and queue size;
- recent committed moves and unverified guest-controller labels; and
- a compact `Connect an agent` panel documenting the gateway protocol.

Anonymous viewers connect through the existing Public Projection Relay design
and receive no participant credential. The gateway can expose five narrow
agent operations backed by one service module:

| Operation | Result |
| --- | --- |
| `join` | Opaque queue ticket or active lease |
| `status` | Queue position, lease expiry, or terminal state |
| `read_state` | Current bounded Projection, Action Offer, and one-use grant |
| `submit_vote` | WorldStream-derived receipt with accepted/stale/rejected state |
| `leave` | Idempotent lease release |

The same semantics can later be presented over HTTP, MCP, or WebMCP. Transport
adapters must not independently implement queue order, leases, or receipts.

## Qualification evidence

The public run is credible only if its result page reports protocol and system
facts, not just a solved canvas:

- exact Pack Revision and client release;
- disk count, deadline, and configured vote margin;
- final move count and whether every move matched the optimal sequence;
- total accepted, stale, rejected, duplicate, and faulted Actions;
- successful and failed voting rounds;
- maximum connected controllers and queue depth;
- Projection Reset and Catch-up counts and byte distributions;
- p50/p95/p99 Action-to-receipt and public Projection latency;
- restart/recovery incidents and verified resumed Head;
- final canonical Head and Replay verification result; and
- clear disclosure that external agent identities are unverified.

A 24-hour soak should include controlled disconnect, slow-consumer, Runtime
restart, gateway restart, duplicate Action, stale basis, malformed proposal,
and queue-turnover drills. The puzzle is solved only when the Pack establishes
the Outcome. The platform publishes the result only after the existing
Replay-verification path confirms its correspondence with the final Public
Projection.

## What the prototype tests

The standalone prototype at
[`web/demos/community-hanoi-state-prototype.html`](../web/demos/community-hanoi-state-prototype.html)
contains no backend and makes no protocol claim. It is designed to answer four
questions before implementation:

1. Can a spectator understand progress, vote state, capacity, and queue from
   one screen?
2. Can a late agent act from a bounded current Projection without Replay?
3. Does serializing grants make the stale-head contention problem visible?
4. How does fixed-slot controller turnover differ from one-Membership-per-guest
   growth?

The prototype's state inspector deliberately exposes the split between Pack,
WorldStream, gateway, and Runner data. If that split is confusing in the
prototype, the implementation boundary is not ready.

## Implementation sequence after analysis approval

1. Run the prototype scenarios with developers and revise the projection and
   gateway vocabulary.
2. Write the Activity Pack contract and trace fixtures for a small disk count;
   then qualify 15 disks without changing semantics.
3. Decide and record the 100-seat limit changes and fixed-slot credential
   broker in an ADR extending the accepted hosted formation decisions.
4. Add the gateway queue and lease state machine with idempotent operations.
5. Build the canvas client against recorded Public Projections, then the live
   Public Projection Relay.
6. Fix or bound long-history activation recovery before calling a 100,000+
   Transition soak successful.
7. Execute local fault and soak qualification, then publish only after the
   exact artifacts and public-result policy are reviewed.
