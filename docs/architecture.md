# Architecture

WorldStream runs in one process. It stores shared application state in Rooms.
Humans and external agents submit Actions. The kernel checks each Action and
controls changes to the authoritative state.

## Ownership boundaries

| Boundary | Owns |
| --- | --- |
| Room kernel | Membership, ordering, authority, Canonical History, integrity, recovery |
| Activity Pack | Domain state, Roles, Actions, visibility, timers, Outcome |
| Storage adapter | Atomic durable implementation of the Room commit contract |
| Activity Client | Presentation and interaction through a scoped protocol |
| Runner | Models, prompts, tools, credentials, and bounded agent execution |
| Host Operator | Installation, Pack approval, setup, and operational lifecycle |

Each Room pins one immutable Pack revision. Clients submit proposals and render
authorized views; they cannot supply accepted state. A Runner's right to claim
an Activation Intent does not grant a Participant's right to act.

## Action processing

1. Authenticate the scoped client and admit its request into a bounded Room lane.
2. Resolve idempotency, authority, the expected Head, and the exact retained Pack.
3. Run the deterministic rule operation against the current Core and Activity
   State. Normalize the effects. Apply the kernel's invariants.
4. Prepare one backend-neutral Room write, including the transition, semantic
   receipt, observation frames, timers, and activation consequences.
5. Commit the write atomically under the expected Head and storage fences.
6. Acknowledge the durable result and deliver authorized observations. Delivery
   can retry or resume without another transition.

Unknown commit outcomes require receipt-based resolution. A timeout is not proof
that a write failed. The detailed contract and failure rules are in
[ADR 0006](adr/0006-backend-neutral-atomic-room-commit.md).

## State, integrity, and history

Authoritative Room State consists of Core Room State and Activity State. The
Core owns Room Status and the semantic Membership map. The Pack owns domain
facts. Operational health, leases, delivery cursors, and caches are separate.

Genesis and ordered Transitions form Canonical History. State and lineage hashes
bind canonical values and accepted changes. Exact serializers, rule revisions,
and retained executors are part of reproducibility. A Room never upgrades its
rules in place, and a missing or mismatched executor fails closed.

Snapshots reduce recovery time. They are not authoritative. Verification reconstructs state from
retained evidence, and a disagreement can fault or quarantine a Room rather than
silently repair history. Checkpoints and operational receipt proofs bound parts
of recovery without changing the canonical lineage. See
[ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md),
[ADR 0031](adr/0031-verified-checkpoint-recovery.md), and
[ADR 0037](adr/0037-successor-anchored-operational-receipts.md).

## Visibility and agent continuity

A Projection is one Membership's authorized current view. An Observation Frame
contains only that Membership's visible consequences of one accepted transition.
An Observation Stream therefore differs from global Canonical History.

A disconnected client uses its durable Cursor to catch up. When incremental
history is unavailable or visibility changes, an explicit Projection Reset
establishes a new baseline. Replay reconstructs historical state; catch-up
resumes delivery. They are different operations.

Agent Participants retain their Principal, Membership, and observations across
Invocations. An Activation Intent asks an authorized external Runner to consider
fresh work. Claims and leases are fenced, and Invocation Context is bounded to
what the Membership may know. The model's private memory and process lifetime
remain outside the Room. See [Observation and activation](observation-and-activation.md).

## Deterministic Packs

`ActivityPackV1` exposes five operations: `descriptor`, `initialize`, `reduce`,
`view`, and `observe`. A portable `.wspack` binds the exact Component, revision
lock, schemas, codecs, static material, and golden evidence.

The Component Host uses Wasmtime without WASI imports and applies execution and
resource bounds. Pack code has no direct network, filesystem, clock, entropy,
or provider credentials. Recorded stimuli carry time and external facts.
Operator approval is tied to exact bundle bytes; installation does not grant
participant or Runner authority. See [Activity Packs](activity-packs.md).

Counter and historical Heist revisions remain embedded conformance fixtures.
Their retained source artifacts and dependency identities are necessary for
historical Replay. The portable example Packs live under `examples/packs/`.

## Persistence and operation

SQLite and PostgreSQL implement the same logical Room commit surface. The
startup configuration selects one backend. SQLite supplies the self-contained
local path; PostgreSQL is a separate primary, not a second concurrent authority.

Backend-native backup and restore preserve authority and exact Pack material.
SQLite-to-PostgreSQL transfer is an offline, fenced operation. Multiple live
writers, automatic provider failover, clustering, and multi-region operation are
outside this implementation. See [Storage operations](operator-storage.md).

`worldstreamd` serves the runtime. `worldstreamctl` handles operator actions.
The headless Controller supports managed local processes, retained Room setup,
Runner attachment, and scoped client handoffs. Its historical crate name is
`worldstream-studio-supervisor`; there is no Studio web application.

Typed hosted/browser compatibility contracts remain where these operator tools
and their tests depend on them. They do not start the retired public platform.
The hardcoded hosted catalog, gateway application, hosted databases, and cloud
deployment configuration have been removed.

## Implementation map

| Code | Responsibility |
| --- | --- |
| [`worldstream-core`](../crates/worldstream-core/) | Canonical reduction, Pack registry, Replay, authority, storage-neutral contracts |
| [`worldstream-protocol`](../crates/worldstream-protocol/) | Shared wire types and bounded values |
| [`worldstream-server`](../crates/worldstream-server/) | Runtime, HTTP/WebSocket gateway, operator CLI |
| [`worldstream-runtime`](../crates/worldstream-runtime/) | Configuration, filesystem safety, embedded identity |
| [`worldstream-sqlite`](../crates/worldstream-sqlite/), [`worldstream-postgres`](../crates/worldstream-postgres/) | Durable storage adapters |
| [`worldstream-backup`](../crates/worldstream-backup/), [`worldstream-transfer`](../crates/worldstream-transfer/) | Recovery artifacts and offline transfer |
| [`worldstream-pack-bundle`](../crates/worldstream-pack-bundle/), [`worldstream-component-host`](../crates/worldstream-component-host/) | Bundle verification and bounded portable execution |
| [`worldstream-studio-supervisor`](../crates/worldstream-studio-supervisor/) | Headless operator and managed-process support |
| [`sdk/`](../sdk/) | External client and Pack authoring libraries |
| [`examples/`](../examples/README.md) | Applications exercising the boundaries |

## Tradeoffs and limits

The authority checks, exact identities, and recovery rules require substantial
code and tests. One process controls the order of changes. This restriction
limits scale and availability. Retained Pack revisions require storage and
compatibility checks. The examples do not establish product usefulness or agent
performance.

The repository is a reference implementation. Historical compatibility files
are build inputs. They do not certify a production release. Read the
[verification guide](gates.md) for checks and limits. [ADR 0044](adr/0044-preserve-kernel-and-examples.md)
defines the current project scope.
