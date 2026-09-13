# Long-running Room primitives and qualification proposal

Status: Proposed, 2026-09-12. This document does not change accepted ADRs,
wire contracts, canonical codecs, or existing Room lineages. It records a
proposed approach and findings from the current implementation, not a passed
scale qualification.

The supporting [foundations research](long-running-agent-foundations-research.md)
compares public agent harnesses and durable execution systems and derives the
freshness and continuity requirements. The [paper review](long-horizon-agent-papers-research.md)
records the actual experimental limits of recent long-horizon agent research.

## Objective and workload

Support a durable Agent Participant working in a Room for days, across many
bounded Invocations and connection replacements, with at least 100,000
accepted Transitions in that same Room. Qualify a million-Transition stress
case separately. Neither count is a promise about model quality.

A model turn, an Invocation, an Action attempt, an accepted Transition, and an
Observation Frame are different units. A model turn can produce no Action;
rejected and duplicate Actions consume no new Room sequence; a Transition can
address several Memberships; several Transitions can matter to one Invocation.
Measure all of these separately. If the requirement is 100,000 model turns
per agent, the Room workload must include the resulting aggregate Actions,
external inputs, timers, and deliveries from every agent.

100,000 Transitions over one day average 1.16 per second; over seven days they
average 0.165 per second. History length, burst rate, payload size, participant
fan-out, active work cardinality, and model latency are separate dimensions.

## The small participation interface

Keep the existing separation between participant, Runner, and Host authority.
These are semantic operations; the wire already separates some into multiple
messages.

| Operation | Contract |
| --- | --- |
| Attach / observe | Authenticate a Membership, receive its authorized current baseline or bounded retained changes, then receive live changes through an exact synchronization barrier. |
| Submit Action | Propose a typed domain change using a stable Action ID and an observed Room basis. The Pack validates it; the agent never overwrites authoritative state directly. |
| Resolve / retry | Resolve the original operation identity and exact request after a lost reply. Preserve the original result. A changed proposal uses a new identity after resynchronization. |
| Acknowledge | Advance the Membership Cursor only after durable processing. A Session synchronization acknowledgement remains distinct. |
| Claim / renew / release / complete Activation | Give an authorized Runner bounded fresh context and temporary handling authority. Completion reports handling disposition; it does not prove a domain task completed or grant Action authority. |
| Admit external input | Let an authorized Host adapter submit a recorded, schema-checked source update with stable source/input identity. General feed ingress remains to be implemented. |

`ActionSubmit` already contains `room_id`, `member_id`, `action_id`,
`based_on_room_seq`, `action_type`, and `payload`
([messages.rs](../crates/worldstream-protocol/src/messages.rs)). For example,
an airline Pack could define the following payload; this Action type is
illustrative and is not implemented:

```json
{
  "action_type": "record_connection_assessment",
  "payload": {
    "connection_id": "connection-123",
    "assessment_id": "assessment-47",
    "expected_assessment_revision": 6,
    "source_versions": { "arrival": 18, "departure": 9 },
    "claim": "connection_at_risk",
    "evidence_refs": ["evidence-42"],
    "status": "proposed"
  }
}
```

The surrounding Action envelope supplies identity and Room basis. The Host
attributes the authenticated Membership and Semantic Time. The Pack defines
the meaning of status, revision, evidence, and validity. A stored agent claim
means the attributed claim was accepted under these rules; it does not turn
an inference into an externally verified fact.

Successful commit atomically persists the Transition, current state, receipt,
timers, addressed observations, and allowed Activation Intents. Publication
follows commit. The reducer and Replay perform no network or model effects.

## What an agent reports

| Information | Owner and persistence |
| --- | --- |
| Connection presence, heartbeat, provider latency, token usage | Operational telemetry or Runner state; it does not advance Room sequence. |
| A useful partial finding, revised claim, blocked work, or completion | A Pack-defined Action producing a durable domain fact when accepted. |
| Prompt transcript, private scratch work, tool stack | External Runner / agent-private memory, subject to its own checkpoint and retention policy. |
| Completion of an Activation | Runner control disposition, distinct from domain completion. |
| Confirmed external-system outcome | A separately authorized input or Pack-defined result Action with exact external operation identity and evidence. |

For shared work, the Pack should carry compact explicit facts such as current
status, responsible Membership, work revision, supporting source versions,
open obligations, and the next permitted Actions. Keep these Pack-owned;
they are not new fields in Core Room State or a generic workflow engine.
Current Projections must expose all authorized unresolved obligations needed
for continuity. A summary alone cannot replace these facts.

## Bound the cost of continuing

Let N be historical Transitions, S current state bytes, M relevant
Memberships, and K the recovery tail. The target is history-independent live
work for fixed S and M, plus indexed storage costs. It is not literal O(1)
for every database operation.

1. **Bound current state.** Keep active work and compact current facts in
   Activity State. Keep completed details in retained history and authorized
   references. Admit new active work only within explicit cardinality/byte
   limits; never evict an unresolved obligation to make room. Large evidence
   belongs in an explicitly supported external immutable artifact store.
2. **Bound each Invocation.** Build context from the current authorized
   Projection, Action Offers, open work, and a limited change window. Enforce a
   total byte limit before allocating/serializing the claim, plus a
   Runner-specific token budget. Retention limits alone are insufficient.
   Additional historical evidence is retrieved in authorized bounded pages.
   Do not append every earlier model message to each new prompt.
3. **Make Reset sufficient.** If the retained range exceeds the context or
   transport budget, return an explicit Projection Reset. Preserve the Cursor;
   require the existing synchronization acknowledgement. Catch-up omissions
   must be explicit. The current state must still carry open work that cannot
   safely be forgotten. A paginated reliable audit consumer is a separate
   requirement from a participant rebuilding its current situation.
4. **Use one current execution basis.** Actions, current reads, timers,
   external inputs, and Activation context preparation should borrow the
   same uniquely owned, storage-fenced Room executor. Retain no historical
   transition vector between operations. Avoid a hidden full recovery on
   every fresh Invocation or after an ordinary timer update.
5. **Make restart use a verified checkpoint and bounded tail.** A usable
   recovery checkpoint needs the complete Core/Activity basis and exact Pack,
   lineage, timer-generation, delivery, and other required recovery witnesses.
   A pair of JSON state blobs is insufficient. Capture immutable upper bounds
   and read short keyset pages. Bound recovery CPU, allocations, reader
   duration, and concurrency. Full forensic Replay remains separate work.
6. **Budget attention.** Do not create a model call for every feed update.
   Pack-defined meaningful changes and operational execution limits decide
   when fresh consideration is useful. Explicitly bound pending work and
   distinguish refreshable state from non-discardable obligations. Existing
   deduplication includes cause sequence and therefore does not coalesce a
   day's worth of distinct updates automatically. Any new cross-Transition
   intent supersession policy requires a versioned contract with retained
   dispositions; never silently drop pending obligations or timer firings.

Fast restart must preserve the accepted integrity model. Define precisely
what previously verified checkpoint evidence permits skipping prefix
execution and what corruption remains detectable at load. Where the current
contract requires complete prefix verification, either perform that work or
explicitly revise the recovery contract before claiming bounded-tail startup.
An unverified cache or background audit cannot silently make a Room healthy.

Snapshots plus subsequent events are an established event-sourcing technique;
the WorldStream-specific work is preserving its stronger authority, privacy,
timer, and integrity witnesses. [Microsoft event-sourcing guidance](https://learn.microsoft.com/en-us/azure/architecture/patterns/event-sourcing).
Short reads also matter operationally: a long SQLite WAL reader can prevent
checkpoint progress. [SQLite WAL documentation](https://www.sqlite.org/wal.html#concurrency).

## Concurrency and external effects

Preserve strict `based_on_room_seq` admission initially. When an agent returns
with an old basis, refresh, re-evaluate its proposal against current facts,
and submit a new Action identity. Pack-level source/work revisions express
causal validity but do not bypass the current Room-head check. Measure stale
rejection rate and successful-contribution latency under realistic model
delays and external-update bursts.

If unrelated updates demonstrably prevent progress, evaluate a versioned
protocol successor with declared dependency preconditions or explicit
commutative Action classes. That would change accepted Action semantics and
requires an ADR; it must not be introduced as silent rebasing. Required
membership, visibility, policy, and domain dependencies must still be checked.

For external effects, use a durable Pack-defined request identity. An external
executor calls the target using the same idempotency key where supported and
records the confirmed result under that identity. If it crashes after the
external effect but before reporting, reconcile the external operation before
retrying. An ambiguous result remains pending/unknown; it is not failure or
success by inference. Room receipts deduplicate Room changes, not arbitrary
external effects. Runner leases fence control operations only; they do not
revoke separately held Action authority. Late domain results must also obey
Pack-level current work revision, ownership, and result-identity rules.

## Implementation findings

| Area | Current evidence | Consequence |
| --- | --- | --- |
| Live executor | `room_trace_cache.rs` discards retained history; the server's cached participant Action path checks the durable current fence. | Useful foundation for bounded live work. |
| Activation context | SQLite `prepare_activation_claim` calls `gateway_room_snapshot`, which calls recovery. | Claim preparation can scale with full history despite the participant Action cache. |
| Recovery | SQLite inspection collects Transitions from sequence 1 through Head; Core recovery invokes `replay_for_recovery` from Genesis. | The snapshot-tail target is not established by the presence of snapshot rows. |
| Claim delivery | SQLite's retained-frame branch collects the entire Cursor-to-frame-head range into a vector. | Add a total context budget and explicit oversized-range Reset before constructing a large claim. |
| Snapshots | SQLite prepares a postcommit snapshot for each Advance and retains three. The architecture specifies 250 Transitions or five active minutes. | Reconcile policy and measured write cost; reducing cadence alone will not fix recovery. |
| Canonical records | `TransitionV1` embeds full resulting Core and Activity State. | History size scales with N times state size, not just changed fields. |
| Portability | Transfer v4 has a 100,000-record and 64-MiB bundle ceiling; default backup verification has a 100,000-record-per-Room ceiling. | A Room that accepts 100,000 Transitions can exceed portability limits once other records are included. Qualification must include export, restore, and transfer. |
| Existing workload | The reference 100k fixture alternates authorized Membership suspend/resume. The soak uses small Counter workloads. | Useful checks, but insufficient evidence for 100k agent observation/claim/Action cycles in one Room. |

Source anchors:

- [Protocol Actions](../crates/worldstream-protocol/src/messages.rs)
- [Current executor cache](../crates/worldstream-core/src/room_trace_cache.rs)
- [SQLite server cache and activation paths](../crates/worldstream-server/src/sqlite_backend.rs)
- [SQLite recovery, activation, and snapshots](../crates/worldstream-sqlite/src/lib.rs)
- [Core recovery coordinator](../crates/worldstream-core/src/room_commit.rs)
- [Canonical Transition format](../crates/worldstream-core/src/lineage.rs)
- [Transfer limits](../crates/worldstream-transfer/src/lib.rs)
- [Backup verifier limits](../crates/worldstream-backup/src/lib.rs)
- [Reference workload](../scripts/reference-target-workload.py)

At 100,000 Transitions, a constant 1-MiB Activity State alone contributes about
97.7 GiB of repeated state bytes before compression, indexes, receipts,
observations, or snapshot writes. A state that appends every past contribution
would grow until its bound and incur quadratic cumulative copying before
that point. V1 therefore needs small bounded state, measured storage budgets,
and explicit overload behavior. A later canonical format could retain compact
recorded inputs, outputs, and resulting hashes with periodic full checkpoints;
that requires new codecs and replay evidence while retaining exact old
lineages. Never rewrite V1 history or treat lossy summaries as canonical
compaction.

## Qualification and implementation order

First close the existing-contract gaps: bounded Activation preparation,
consistent current-executor use, incremental recovery with an explicit
integrity contract, and bounded storage readers. Then address snapshot
write amplification, pending-intent/context retention, and streaming
portability limits. Only measured contention or storage costs should drive
new Action semantics or canonical record formats.

Use one small nonterminal Activity Pack and replaceable deterministic Runners
through the production client and daemon interfaces. Run both embedded and
portable Pack cases, then SQLite and PostgreSQL separately. Deterministic
Runners qualify runtime scale without 100,000 paid model calls; a separate
bounded model-backed test checks whether reconstructed context supports useful
continuation.

| Test | Required evidence |
| --- | --- |
| Same Room at 1k, 10k, 100k, and 1m Transitions | Read, claim, and Action p50/p95/p99; allocations; resident memory; bytes per Transition; active-state size; portable callback cost. |
| Agents offline beyond retention/context limits | Explicit Reset, no unauthorized disclosure, open obligations retained, successful fresh contribution. |
| Repeated Runner replacement | Original Principal/Membership continuity, private-state separation, no authority gained from a lease, stale work rejected. |
| Lost reply and crash around commit | Original identity resolves to the same receipt; no acknowledged loss or duplicate domain change. |
| Realistic concurrent updates | Stale rejection rate, successful-work latency, bounded backlog, fairness, timer progress, no starvation hidden by retries. |
| Restart at 100k with recent checkpoint | Exact final state/Head and timer obligations; count prefix and tail records actually read/reduced; report time and memory. |
| Missing/corrupt checkpoint | Explicit fallback or integrity failure with unchanged canonical guarantees; no false healthy result. |
| 24–72 hour accelerated-rate soak | Stable live memory and queue bounds; measured expected history growth; controlled WAL; bounded retained contexts; credential renewal and timer catch-up. |
| Backup/restore/transfer at scale | Complete history, operational receipts, current state, authority, and retained executors survive beyond 100k-record limits. |

Use the existing Linux reference profile for comparable measurements: release
build, 4 vCPU, 8 GiB RAM, local SSD, explicit state sizes and participant counts.
The existing p95 commit-to-ack target below 100 ms and 100k recovery target
below five seconds remain targets. Require no full-history scans in ordinary
warm reads, claims, or Actions, with fixed active state and participants;
record any logarithmic index effects separately. Publish backend-specific
results and test failures. Neither existing unit-test totals nor a passing
20-Transition reduced fixture qualify this workload.

The single-process, single-authority, one-Pack-per-Room scope remains in force.
Days of history alone do not require distributed mutation. Sharding across
processes, cross-Room transactions, in-place Pack upgrades, and semantic
history deletion remain separate proposals governed by their existing ADRs.
