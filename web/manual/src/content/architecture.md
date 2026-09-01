# Architecture

WorldStream is deliberately one process with explicit internal boundaries. The
kernel orders Room changes; adapters make them durable; transports expose only
typed, authorized operations.

## Component map

```text
Activity Client ─┐
External Agent ──┼─ HTTP/WebSocket/MCP ─ Gateway ─ Room supervisor
Studio ──────────┘                              │
                                              ▼
                                      single Room actor
                                              │
                         prepare ─ ActivityPackV1 ─ project
                                              │
                                              ▼
                                   atomic Room Commit
                                   ├─ state + Transition
                                   ├─ semantic receipt
                                   ├─ observation frames
                                   ├─ timers
                                   └─ activation intents
                                              │
                                      SQLite / PostgreSQL
```

## One accepted Action

1. The Gateway authenticates a canonical bearer and resolves its scoped
   capability.
2. The Room supervisor routes work to the current Room actor generation.
3. Admission checks Membership standing, access, purpose, expiry, revocation,
   rate limits, Action Offer identity, and exact Head precondition.
4. The Activity Pack reduces the normalized Stimulus against the exact current
   Core and Activity State.
5. Core constructs and validates its proposed Membership/status changes; the
   pack can veto only the narrow proposal classes allowed by the contract.
6. The storage adapter atomically persists the new authoritative state,
   Transition, semantic receipt, frames, timers, and Activation intents.
7. Only after commit does the actor install the new in-memory Head and publish
   delivery/attention work.
8. A lost response is resolved from the durable semantic receipt; the Action is
   not blindly executed again.

## Canonical state and history

Authoritative Room State is exactly Core Room State plus Activity State.
Operational health, snapshots, Cursors, delivery queues, and agent-private
memory are not canonical state. Canonical History begins with Genesis and then
contains ordered Transitions. Every Complete Head binds sequence and hashes.

Semantic Time belongs to a Stimulus: admitted time for Actions, scheduled time
for timers, and declared recorded time for other sources. There is no invented
universal Transition timestamp.

## Concurrency model

- One logical writer advances a Room at a time.
- A bounded Room admission lane provides ordering and backpressure.
- The storage commit uses an exact expected-Head guard.
- Stale or losing candidates receive stable dispositions and do not install
  speculative state.
- A stale actor generation cannot publish after passivation/reload.

## Recovery model

On load, the runtime verifies canonical history and materializations. Due timer
generations up to a fixed startup cutoff are drained before the Room becomes
Active. Corruption faults or quarantines only the affected Room when possible;
global schema or authority corruption blocks readiness.

Replay reconstructs from Genesis and retained Transitions using the pinned
Activity Pack revision. It never consumes the live Observation Cursor and never
reruns a model.

## Crate ownership

| Crate | Responsibility |
| --- | --- |
| `worldstream-core` | canonical types, Activity Packs, authority, prepare/commit contracts, Replay |
| `worldstream-protocol` | wire envelopes, IDs, messages, bounds |
| `worldstream-server` | Gateway, runtime shell, HTTP/WebSocket routes, CLI |
| `worldstream-sqlite` | bundled SQLite durable adapter |
| `worldstream-postgres` | PostgreSQL 17 durable adapter and native operations |
| `worldstream-runtime` | configuration, filesystem admission, embedded compatibility |
| `worldstream-backup` | backup evidence and semantic verification |
| `worldstream-transfer` | provider-neutral logical transfer |
| `worldstream-studio-supervisor` | bounded local Studio control plane and agent bridge |
| `worldstream-conformance` | backend-neutral black-box scenarios |

Primary source: [architecture](https://github.com/imom39a/worldstream/blob/main/docs/architecture.md),
[atomic Room Commit ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0006-backend-neutral-atomic-room-commit.md),
and [canonical integrity ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0005-canonical-core-state-integrity-and-hash-lineage.md).
