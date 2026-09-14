# Community Hanoi: kernel readiness and next steps

Status: Analysis, 2026-09-13. Source review includes the long-running primitive
implementation `fc3b23b8`, closure evidence `e2a8a7c3`, transfer hardening
`eb43bc1d`, snapshot cadence fix `bd582bb2`, and admission evidence `4ce245af`.
This is a proposal, not a new ADR, a production change, or a passed load
qualification.

## Assessment

WorldStream has the right canonical model for this experiment: one Room,
ordered typed contributions, deterministic Pack reduction, durable receipts,
authorized current views, timers, and replaceable agent execution. Hanoi does
not need a new kernel concept for voting, puzzles, queues, task plans, model
memory, or agent thought turns.

We do not yet have evidence that the complete implementation supports the
promised workload: a Room lasting a day, 100 concurrent participants,
100,000+ accepted Transitions, late arrivals, and restarts. The concrete gaps
are active-Room recovery, PostgreSQL warm execution, qualification at the real
fan-out, and hosted admission. Whole-Head Action freshness is the main
candidate for an additional general kernel semantic. It should be designed
explicitly if direct independent agents must contribute concurrently while
other relevant or irrelevant Room work continues.

Three distinctions matter:

- A kernel **semantic** states what an operation means and what it guarantees.
- A Runtime/storage **implementation** must deliver those guarantees with
  bounded continuation costs.
- Hosted admission and Runner policies use these operations to create the
  community experience.

A slow implementation does not automatically require a different semantic.
A public gateway restriction does not establish a missing Core primitive.

## What already exists

| Requirement | Current foundation | Conclusion |
| --- | --- | --- |
| Accept exactly one ordered contribution | Typed Actions, whole-Head admission, pure reduction, atomic commit and identity-based resolution | Reuse the kernel contract. |
| Agent reports progress or completion | Pack-defined Actions update bounded Activity State; Outcome is Pack-owned | No generic kernel task/status engine needed. |
| Fresh agent understands current work | Projection Reset, current Action Offers, synchronization barrier | No mandatory historical Replay. |
| Returning agent catches up | Membership Cursor, bounded observation delivery, explicit Reset | Reuse; Runner still budgets its model prompt. |
| Execution survives a process replacement | Durable Agent Participant/Membership plus bounded Invocations and generation-fenced Activation leases | Already represented. A lease remains separate from Action authority. |
| Late participant joins canonically | Core Join, Suspend, Resume, Depart, and Role changes in Room order | Already represented. Public admission remains to build. |
| Room ends after one day | Host-recorded Action admission time, scheduled timer generations, fixed-cutoff catch-up | Pack declares the deadline and terminal behavior. |
| No infinite pending refresh queue | ADR 0030 retirement and supersession with retained dispositions | Implemented policy; still budget attention at its source. |
| Export exceeds a legacy in-memory bundle | V2 streaming transfer and streaming retained-backup verification | New implementation exists; qualify actual record counts and public operator path. |

Sources: [Core integrity and Membership semantics](adr/0005-canonical-core-state-integrity-and-hash-lineage.md),
[atomic commit](adr/0006-backend-neutral-atomic-room-commit.md),
[time and timers](adr/0007-semantic-time-timer-ordering-and-catch-up.md),
[observation barriers](adr/0008-membership-observation-streams-and-reset-barriers.md),
[Activation authority](adr/0009-activation-intents-context-and-lease-fencing.md),
and [five Pack operations](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

The implementation changed after the first prototype analysis. SQLite's
production `activation_claim` now borrows the verified current executor through
`with_cached_current_trace` and `prepare_activation_claim_from_serving_trace`.
Both stores page retained frames and select a Reset when frame-count or
payload budgets are exceeded. Core additionally rejects an Invocation Context
over 512 KiB. That is a transport ceiling, not an acceptable default model
prompt size. The Pack and Runner should use substantially smaller budgets.
See [SQLite claim](../crates/worldstream-server/src/sqlite_backend.rs),
[Core context limits](../crates/worldstream-core/src/activation.rs), and
[attention policy](adr/0030-bounded-refresh-activation-attention.md).

## Required Runtime work: recovery for an ordinary active Room

The new checkpoint path does not yet cover Hanoi.

`inspect_checkpoint_candidate_at_path` in the SQLite store rejects a checkpoint
candidate if the Room has any timer rows. It also rejects candidates when
observation frames, observation consequences, or Membership generations above
one exist. These checks apply even to a checkpoint at the current Head. The
safe fallback is full Genesis replay. A Hanoi Room with a deadline, public
observations, and participant turnover meets these fallback conditions.

This is intentional safety, not a reason to remove the checks. The snapshot
does not carry the operational witnesses needed to authenticate those facts
at its own cut. [ADR 0031](adr/0031-verified-checkpoint-recovery.md) explicitly
records this open schema work. PostgreSQL currently retains full recovery too.

Required implementation:

1. Define one recovery checkpoint manifest binding the exact Room Head,
   Core/Activity state, Pack revision, timer state/generations, Membership
   generations, and the delivery/consequence witnesses needed to continue.
2. Capture those witnesses at one consistent cut. A stale state snapshot plus
   unrelated current operational rows is not a checkpoint.
3. Restore from a verified checkpoint and at most the configured Transition
   tail, then recheck the current writer, integrity, authority, and lifecycle
   fences before installing the serving executor.
4. Preserve full forensic Replay and the existing safe fallback. State
   explicitly that checkpoint-based serving trusts the verified cache for
   the skipped prefix; it does not re-execute or prove that prefix on restart.
5. Implement equivalent witness semantics for each supported backend.

The target is `O(current bounded state + bounded recovery witnesses + tail)`,
plus indexed lookup costs. Reading a complete historical timer ledger or
observation-consequence log defeats that target even if reducer calls are
bounded. Historical witnesses need a verified summarized basis under the
accepted trust model; complete historical records remain available for audit.

Completion evidence must keep real deadline timers, observation records,
Cursors, and changed Memberships present. The existing bounded-checkpoint
regression removes observation frames and checks a zero-Transition tail;
it does not establish active-Hanoi restart performance. Source:
[SQLite checkpoint selection and regression](../crates/worldstream-sqlite/src/lib.rs).

Full replay may still be fast enough for an initial small Room. A live
measurement can establish that narrower operating limit. It cannot establish
history-independent continuation for days or a million-Transition successor.

## Required backend parity: PostgreSQL fresh claims

PostgreSQL `prepare_activation_claim` still calls `verify_room` and then
`recover_room` before constructing the current view. The SQLite warm-executor
improvement has not removed this cost from the PostgreSQL path. Apply the
same storage-fenced current-executor approach there, preserving claim-time
rechecks rather than weakening them.

The closure report's 1,000-sample SQLite latency measurement repeatedly uses
the same `warm-claim` identity with four history rows. After the initial claim,
the production route can return its existing receipt. This is useful
idempotency evidence; it is not a measurement of 1,000 fresh Invocations or
fresh claims after 100,000 Transitions. Source:
[closure evidence](imo-220-222-closure-evidence.md),
[SQLite claim regression](../crates/worldstream-server/src/sqlite_backend.rs),
[PostgreSQL claim preparation](../crates/worldstream-postgres/src/lib.rs).

Next qualification must create new pending work and claim identities, complete
or release leases, advance the Room, and repeat. Measure fresh claim
preparation separately from receipt retries. A SQLite-only showcase can defer
PostgreSQL optimization, but should state that deployment scope explicitly.

## Semantic question: what makes a delayed proposal stale?

Today `based_on_room_seq` must equal the current Room sequence before Pack
reduction. A Pack-level `round_id` cannot bypass that check. Two votes for the
same unchanged board still conflict if they carry the same original Head.
One succeeds; the next is stale because the first vote advanced the Room.
Source: [stable Action admission](../crates/worldstream-core/src/trace.rs).

This is correct serialization, but more conservative than the Hanoi domain
requires. There are two different issues:

- **Synchronization:** a hidden Transition emits no frame, so a live client's
  known Room Head can remain old even before it starts reasoning. The existing
  IMO-217 investigation identifies this issue. A generic correction must
  respect private activity visibility.
- **Concurrent reasoning:** the client was current when it started, but
  another Action committed while its policy was deciding. Refreshing the
  transport does not eliminate that race.

Both need qualification. They should not be conflated, and neither justifies
removing the commit-time current-state and authority checks. See
[existing freshness investigation](heist-mission-focus-release.md#remaining-playability-blocker-hidden-room-head-advances).

### Near-term Hanoi policy using the current kernel

Allow a small cohort of controllers to reason concurrently about one board
and round. Before submission, the Runner/Activity Client adapter obtains the
latest authorized view and explicitly re-evaluates the proposal: same board,
same round, still enabled and authorized, not already voted, current Action
Offer, and unexpired domain deadline. If valid, it forms a new Action from
that current basis. If any necessary fact changed, it discards the proposal.
It must resolve any earlier indeterminate submission before forming a new one.

This re-evaluation can be deterministic for the narrowly specified Hanoi
policy; it need not call the model again. It belongs in the declared Runner
policy, not in an invisible gateway rewrite of an agent's already-submitted
Action. The gateway may serialize short submissions. It must not hold the
Room's admission lane or a database lock while waiting for model inference.
An intervening timer or administrative change can still make a submission
stale, and the existing receipt contract remains authoritative.

This approach can demonstrate concurrency without a protocol successor. It
does require every participating client to understand the re-evaluation rule.
The initial prototype's serial-grant description does not by itself specify
this behavior and should be revised before implementation.

### General kernel direction worth designing

For direct independent agents, design an opt-in **Pack-defined Action validity
scope** while retaining exact-Head Actions as the default. This is proposed
semantics, not a current wire feature.

For Hanoi, a scope could bind the board revision and voting round. Vote
counts can change within that scope; a committed move, closed round, or rule
change invalidates it. Other Packs would define their own bounded scope.

The contract would need to establish all of the following:

- The exact Pack revision declares which Action types allow scoped validity
  and includes an authorized scope token in their versioned Action Offers.
- An Action records its actual observed basis and scope. The host never
  claims the agent observed a later state.
- Admission compares against the current offer, checks current authority,
  Membership/Role eligibility, deadline, and Pack reduction, and commits
  against the exact **current** Complete Head.
- The recorded Stimulus preserves both the observed evidence and the actual
  evaluation basis. Retries preserve the original identity and request;
  changed requests conflict.
- Old tokens cannot be reused across Memberships, Rooms, Pack revisions, or
  invalidated scopes. Tokens expose no unauthorized facts.
- Replay validates the same deterministic conditions. Existing V1 lineages
  and their exact-Head semantics remain unchanged.

This separates the state needed for a decision from the serialization fence
needed for a commit. It permits concurrent independent reasoning while still
committing one ordered Transition at a time. It does not introduce multiwriter
Room authority or waive domain conflicts.

The new scope representation affects Action Offers, admission, recorded
Stimuli, hashes/codecs, SDKs, Replay, and compatibility evidence. It merits an
ADR and conformance design before implementation. A `round_id` field alone
is not that design. Prefer this small domain-neutral extension over a generic
workflow engine or a Hanoi-specific kernel exception.

## Guest admission needs a Hosted contract, not a new Join primitive

Core already permits a new Principal's Membership to Join an existing active
Room. [ADR 0021](adr/0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md)
restricts the current public Hosted Platform to a frozen pre-Genesis roster.
Extending the public path should broker the existing Host-authorized Core
operations through a narrow, idempotent admission interface.

For the final community showcase, prefer a distinct Principal/Membership for
each independently participating agent. A restart of the same logical agent
reuses its original identity; a different guest does not inherit that identity
or private history. The fixed-slot prototype remains useful for exploring
replaceable controllers, but demonstrates slot attribution rather than exact
guest attribution. It is not required by the kernel.

Operational requirements:

- Define the 100-person limit precisely. Pack Role maxima count all
  non-Departed participants, including Suspended ones; connection count is
  separate. Spectator/operator Memberships need additional capacity.
- The Hosted contract currently caps seats at 32, Studio drafts at 64, and
  architecture documents a 32-member public profile. Raise the reviewed
  limits consistently and qualify 100 participants plus service spectators.
- Use a Role range such as `0..100`, with the minimum needed to start or
  finish voting enforced by Pack phase rules. The previous proposed
  `minimum = maximum = 100` would obstruct ordinary departures under final
  Role cardinality validation.
- Depart is mandatory and Join is vetoable. Core rejects a changeset mixing
  those classes. Release then admit through two separately resolved ordered
  operations; retain the gateway reservation across uncertain results.
- A disconnected agent can have a bounded reconnect grace period. Once
  departed, any later admission needs a new Membership and fresh baseline.
- Bound the queue and total distinct admissions per Room/day. A cap of 100
  concurrent participants does not bound the retained Membership map.

No change should make Membership identities mutable or silently prune
Departed entries. Supporting unbounded lifetime churn at fixed current-state
cost would require a separately versioned Core representation; even compact
Transition records alone would not solve that problem.

Sources: [Core Membership reducer](../crates/worldstream-core/src/reducer.rs),
[Role cardinality](../crates/worldstream-core/src/activity_pack.rs),
[Hosted limits](../crates/worldstream-hosted-contract/src/lib.rs), and
[Studio limits](../crates/worldstream-studio-supervisor/src/room_drafts.rs).

## Storage, attention, and public observation costs

Current `TransitionV1` still embeds the complete resulting Core and Activity
values. Compact Transition V2 has an accepted design in
[ADR 0034](adr/0034-compact-canonical-transition-format.md), but the canonical
implementation remains V1. Transfer stream V2 is a different format and must
not be confused with compact canonical records.

Small bounded Hanoi state can use V1 for a measured initial experiment. Include
the entire Membership map in the byte estimate, not only the 15 disk values.
If state size or admission churn exceeds the operating budget, implement the
already-described new-lineage codec rather than rewriting retained V1 history.
Source: [canonical record](../crates/worldstream-core/src/lineage.rs).

Current consequence preparation evaluates eligible viewers from the
Membership set on each Transition. If 100 participants each receive a 1-KiB
frame on 100,000 Transitions, that alone is about 9.54 GiB of generated frame
payload and ten million frame rows before pruning, indexes, receipts, and
service spectators. Streaming transfer defaults to ten million total records;
that budget therefore also needs workload-specific review. Those are sizing
calculations, not measured storage usage.

Use bounded participant observations and attention cohorts; the public relay
can fan one Spectator stream out to many human viewers. Only hide a change
when the authorized current view really permits it. Do not arbitrarily skip
required frames to reduce fan-out, nor expose other voters' proposals to an
agent and then claim its vote was independent. A public live tally also means
untrusted guests can deliberately coordinate; the community exhibition cannot
claim MAKER's error assumptions have been verified.

ADRs 0030 and 0007 keep refresh attention and timer obligations distinct.
For Hanoi, represent an open round in Activity State and signal a bounded
cohort. Avoid generating a new deadline-bearing Activation for all 100
participants on every vote. Provider latency and heartbeats remain operational
telemetry, not canonical Transitions.

Sources: [consequence preparation](../crates/worldstream-core/src/room_commit.rs),
[stream limits](../crates/worldstream-transfer/src/lib.rs), and
[streaming backup verification](../crates/worldstream-backup/src/native_sqlite.rs).

## Evidence obtained in this analysis

I ran the existing deterministic starvation probe at two unrelated Room
updates per second, with participant occupancy set to 100:

| Decision delay | Useful trials / 8 | Stale attempts | Interpretation |
| --- | ---: | ---: | --- |
| 250 ms | 8 | 7 | Retries eventually find a current basis. |
| 1 second | 0 | 256 | All eight trials exhaust 32 attempts. |
| 5 seconds | 0 | 256 | Refresh alone cannot produce progress under this schedule. |

The probe mirrors the production comparison and uses periodic virtual time.
It does not execute 100 real agents or measure database throughput/fairness.
It demonstrates a possible liveness failure, not the probability of failure
under a model provider. Source: [probe](../scripts/action-starvation-probe.py).

The focused production Core test
`room_commit_tests::production_action_admission_matrix_preserves_exact_basis_fence`
also passed during this analysis. It calls actual stable admission and confirms
that a synchronized Action is allowed and an Action carrying the prior basis
is rejected after an intervening Transition. Its delay/rate and visibility
values label cases; it does not simulate provider timing or 100 participants.
This corroborates the safety rule, separately from the virtual-time liveness
results above.

I also checked the workload arithmetic. Fifteen disks require 32,767 moves
and a minimum of 98,301 accepted votes at margin three. It is incorrect to
promise that this automatically exceeds 100,000 Transitions: queue operations,
leases, observations, and rejected Actions do not advance Room sequence.
Use an explicit >=100,000-Transition durability fixture; choose game size
separately. Sixteen disks guarantee 196,605 accepted votes for a successful
margin-three solution, with a correspondingly harder one-day latency target.

For 15 disks, a completed solution within 24 hours needs an average move every
2.637 seconds. Fully serial model calls would need one accepted vote every
0.879 seconds before overhead. Even perfectly parallel voters do not remove
the dependency between successive board states. Measure actual policy latency
before promising completion; a 24-hour Room can validly end with an expired
Outcome if the board is unsolved.

The repository's [history qualification](room-history-qualification.md)
explicitly distinguishes deterministic smoke evidence from real backend
evidence and an actual 72-hour soak. No new production benchmark or soak was
run for this analysis.

## Follow-up: Git-like parallel futures are a separate missing capability

The user's remembered design is preserved in
[BranchLab — Git for live agent futures](ideas-and-research.md#2-branchlab--git-for-live-agent-futures).
It proposes a shared typed checkpoint, isolated concurrent strategies,
validation, and winner promotion. The document labels it a parked exploration.
[ADR 0001](adr/0001-product-boundary.md) excludes timeline forks and branch
promotion from the frozen scope; the [protocol](protocol.md#replay) explicitly
has no fork or branch-creation interface in v0.1 or v0.2.

The current implementation provides many independent Rooms, each with one
canonical lineage. Membership Observation Streams are authorized views of
that lineage; they are not independently mutable alternative futures. Pack
Actions such as Heist's `propose_plan` and `endorse_plan` demonstrate proposals
and voting inside one Room, not branch execution or promotion.

The assessment above assumes a single authoritative Hanoi board. If the
showcase should instead allow agents to select, explore, and vote on competing
futures, add this explicit design decision before implementing it:

| Option | Semantics | Placement |
| --- | --- | --- |
| Bounded candidate-path tree | The Pack stores a bounded set of alternative move sequences, assigned work, evidence, and votes. One Room orders every proposal and the selected result. | Pack implementation over current kernel; no native timeline fork. |
| Native isolated futures | Each branch has its own state and ordered continuation from a retained parent checkpoint; agents attach to authorized branches, and a verified result can be promoted. | New versioned lineage/branch contract plus Host and Runner integration. |

For a native-future design, specify the parent Room and exact fork Head, branch
identity and storage lineage, current and historical authority, independent
timers and Activation lifecycles, retention/GC, and external-effect isolation.
Voting remains Pack policy. Promotion must validate the candidate and the
parent's current basis, produce one idempotent accepted parent change, and
retain provenance without rewriting existing history. If the parent advanced,
revalidate or explicitly reject; do not assume arbitrary Activity states can
be merged like text. Cross-Room atomic promotion, if required, is another
contract beyond today's single-Room commit.

For Hanoi, bounded candidate paths can answer the product question cheaply.
They do not prove native branch recovery, shared-prefix storage, or safe effect
promotion. Native futures would therefore be an additional kernel initiative,
separate from the scoped Action validity proposal and active-Room recovery.

## Recommended implementation order

1. **Freeze the workload and identity contract.** Specify >=100,000 accepted
   Room Transitions, 100 simultaneous participant identities plus service
   spectators, bounded lifetime admissions, vote observations, and policy
   latency assumptions. Keep model turns, Invocations, Action attempts,
   Transitions, and Frames as separate counters.
2. **Complete active-Room checkpoint witnesses.** Start with the showcase
   backend. Pass recovery with timers, retained observations, acknowledgements,
   Membership churn, and pending/leased Activations at a real 100k Head.
3. **Prove fresh warm execution and parity.** Exercise repeated new
   claim/action/complete cycles, timer and administration interleaving, and
   cache fencing at 1k/10k/100k history. Bring PostgreSQL onto the same current
   executor approach before claiming backend parity.
4. **Specify freshness and concurrent proposal handling.** Close the known
   hidden-Head synchronization gap. Compare explicit Hanoi re-evaluation
   against an opt-in Action-validity scope using delayed clients. Write the
   successor ADR if direct generic concurrent participation is the product
   requirement; retain V1 semantics for existing clients and lineages.
5. **Add hosted open admission using existing Core operations.** Implement
   queue reservations, current-identity re-entry, Join/Depart reconciliation,
   scoped credentials, and consistent 100-participant limits. Keep this
   outside the Pack and the kernel's authoritative state model.
6. **Qualify the entire Room.** Use deterministic external Runners through the
   real daemon/client paths. Measure latency, retained state, generated frames,
   WAL/storage growth, and fresh join cost. Inject lost replies, Runtime and
   Runner replacement, due timers, slow spectators, stale work, and corrupt
   checkpoints. Complete backup/restore and transfer at the actual row count.
7. **Run the public model-backed exhibition after the system gate.** Report
   puzzle quality separately from runtime durability. A 24-hour showcase is
   not the repository's broader 72-hour qualification gate.

The first required engineering work is bounded recovery for active Rooms.
The first new kernel semantic to design is opt-in Action validity under
concurrent updates. Queueing, vote aggregation, Hanoi strategy, model memory,
and a mutable long-lived conversation remain outside the kernel.
