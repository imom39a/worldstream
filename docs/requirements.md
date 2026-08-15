# Frozen Requirements

## Document authority

Status: **FROZEN for Agent Heist v0.1 and Investigation Room v0.2**

Freeze date: 2026-08-13

This is the normative product-behavior and release-scope document. If an architecture, protocol, roadmap, or example conflicts on behavior or scope, this document wins. The root [WorldStream Domain Context](../CONTEXT.md) is authoritative for domain term names and meanings; a conflict between terminology and requirements is a documentation defect that must be reconciled rather than silently redefined.

Change control:

1. A requirement addition needs a short ADR describing the demonstrated need.
2. Before v0.2, new scope must replace scope of comparable cost unless it fixes correctness, security, or the ability to deliver either reference activity.
3. Ideas that do not block a release gate go into the non-normative research backlog.
4. Public protocol and Activity Pack ABI stability are not promised until both reference activities pass.

The words MUST, MUST NOT, SHOULD, and MAY are normative in this document.

## Product definition

WorldStream is a self-hosted realtime room runtime for multi-agent applications. Its precise model is multi-participant: external humans and independently hosted AI agents join a rule-governed room, receive scoped realtime observations, submit typed actions, and retain continuity across connections and ephemeral agent invocations. Multiplayer server design is the architectural analogy, not a game-only product boundary.

Primary adopter: an AI application developer building a multi-participant application.

Primary value:

- the developer defines domain rules once;
- WorldStream supplies durable ordering, shared-state recovery, partial visibility, realtime delivery, reconnect, activation, and replay;
- agent implementations remain framework-neutral and run outside the server.

WorldStream is appropriate when all or most of these are true:

- two or more independent participants affect the same evolving state;
- participants can have different authorized views;
- participant actions may race, conflict, or change what others can do;
- the next action is selected by a human or agent policy rather than a prewired workflow edge;
- disconnect, catch-up, recovery, or replay matters.

WorldStream is not appropriate for a single prompt, a single worker completing one isolated task, a fixed boxes-and-arrows automation, raw pub/sub, or generic document search.

## Frozen hierarchy

Through v0.2 the complete authoritative hierarchy is:

    WorldStream server
    └── zero or more independent rooms
        └── exactly one pinned Activity Pack digest

A room may be described informally as a world. World is not a separate database entity. There is no Project entity, cross-room exchange, shared project memory, or multi-pack room composition in the frozen releases.

## Domain language

The root [WorldStream Domain Context](../CONTEXT.md) is the canonical glossary for the product model. The [Extended WorldStream Terminology](glossary.md) adds protocol, runtime, storage, and UI terms and may repeat domain terms only as non-authoritative navigation summaries. This requirements document adds obligations to those meanings; it does not create competing definitions.

Agent persistence is logical, not computational. WorldStream persists identity, role, permissions, cursor, activation state, room facts, and explicit artifact references. It MUST NOT claim to preserve a continuously thinking model, a hidden chain of thought, or an arbitrary execution stack.

Delivery resume means observation catch-up. The word resume MUST NOT imply cognitive or process continuation. A runner normally starts a fresh invocation and reconstructs its authorized input from the observation and any explicit external checkpoint it owns.

## Ownership boundaries

### WorldStream MUST own

- room identity, lifecycle, and one pinned Activity Pack revision;
- Membership identity, lifecycle, Access Mode, and current pack-defined Role assignment, plus development-grade authentication;
- one total order of committed transitions per room;
- typed action routing, idempotency, and stable action results;
- durable timers and host-recorded nondeterministic inputs;
- state snapshots, recovery, current projection, and deterministic replay;
- Membership-addressed Observation Frames, acknowledgements, and Cursor Catch-up;
- durable activation intents, claim leases, retries, and status;
- bounded network queues, rate limits, and slow-consumer handling;
- a minimal operator-membership/reference UI and protocol/SDK conformance fixtures.

### Activity Packs MUST own

- room configuration and domain state;
- Role definitions, cardinality, permissions, and Legal Actions;
- action validation and deterministic reduction;
- public, participant, and operator projection rules;
- timers, attention reasons, completion rules, and result scoring;
- activity-specific UI projection schemas.

### External clients and runners MUST own

- model provider calls, prompts, private memory, planning, and hidden reasoning;
- tools, credentials, sandboxes, and any real-world side effects;
- deciding whether and how to answer an activation;
- converting an authorized observation into typed actions.

WorldStream v0.1 and v0.2 MUST NOT host an LLM loop, store provider credentials, remote-control consumer subscriptions, or expose raw model access.

## Functional requirements

### FR-1: Rooms and packs

- The server MUST host multiple independent rooms in one process.
- Each room MUST pin exactly one Activity Pack identifier and immutable revision digest at creation.
- A room MUST NOT silently switch pack versions.
- Core Room Status MUST be active or archived; Room Health MUST be healthy, faulted, or quarantined; Activity Phase and Outcome MUST remain separate pack-defined values and separate from both core axes.
- Archiving a Room MUST be recorded as an administrative Stimulus and Transition in that Room's order, as decided in [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md).
- Archived, faulted, and quarantined rooms MUST reject ordinary participant mutation while retaining authorized inspect, export, recovery, and replay operations.
- v0.1 packs MUST be trusted, compiled into the server, and selected from an allowlist.
- The public plugin ABI, untrusted code execution, and pack registry are deferred until after v0.2.

### FR-2: Human and agent participation

- Principal kind MUST be human or agent; Principal kind and pack-defined Role are separate.
- Human and agent participants MUST use the same typed action path.
- Membership MUST survive session disconnects and agent invocation termination.
- Membership status MUST be separate from connection, runner availability, and activation status.
- A domain-relevant Membership, Access Mode, or Role change MUST be recorded as a Membership-change Stimulus and Transition; session presence and Runner availability MUST NOT.
- A human MAY join through a participant, spectator, or operator Membership if the pack and room policy allow it. Only the first is an acting Participant.

### FR-3: Typed actions and ordering

- Every action MUST include room ID, membership ID, client-generated action ID, expected room sequence, action type, and typed payload.
- Each active room MUST have one logical writer and one monotonically increasing committed sequence.
- The kernel MUST reject a participant action when based_on_room_seq does not exactly equal the current room head in v0.1 and v0.2.
- The pack MUST validate an action against the current state and membership before mutation.
- Rejected actions MUST NOT consume a canonical room sequence.
- Only deterministic admitted rejections MAY consume the action ID through a durable action receipt.
- Authentication, authorization, malformed input, rate limit, room busy, storage unavailable, and activity/runtime faults MUST use the error path, MUST NOT consume the action ID, and MAY be retried with the same ID.
- An accepted action MUST be durably committed before the server acknowledges or publishes it.
- The key of room, membership, and action ID MUST map to at most one result.
- Reusing an action ID with different canonical payload bytes MUST be rejected.
- Stale actions MUST receive a typed rejection with the current sequence and current legal-action summary when safe.

### FR-4: Deterministic state and timers

- Authoritative Room State at sequence N MUST be a pure function of Room Genesis, the exact Activity Pack revision, and recorded Stimuli through N.
- A pack MUST NOT read ambient wall time, operating-system randomness, files, network resources, environment variables, provider APIs, or secrets.
- Host time and randomness that affect state MUST enter as recorded stimuli or recorded stimulus fields.
- Timer creation, cancellation, and logical firing MUST be durable.
- A timer retry MUST NOT cause two logical firings.
- Canonical Activity State and Transition hashes MUST be reproducible.
- Core Room State and Activity State in the frozen releases MUST avoid floating-point values.

### FR-5: Scoped projections

- The authoritative room state MUST NOT be sent directly to untrusted clients.
- The pack MUST construct separate public, participant, and operator Activity Projections; WorldStream MUST wrap them with authorized Core Room State and Membership metadata without allowing either layer to overwrite the other.
- Every persisted observation frame MUST have an explicit membership audience.
- Every durably streamed spectator or operator MUST therefore have a read-only room membership and its own cursor.
- Authorization MUST happen before persistence, indexing, ranking, or rendering.
- A participant MUST be able to obtain current legal actions and deadlines without reading the raw transition history.
- Private chain-of-thought MUST NOT be requested or stored.

### FR-6: Realtime delivery and reconnect

- WebSocket is the native bidirectional live transport.
- HTTP MAY be used for room administration, current projection reads, runner polling, and artifact transfer.
- Observation delivery MUST be at-least-once.
- Each membership MUST have a monotonic observation cursor independent of the room sequence.
- An observation acknowledgement MUST only advance that membership's cursor.
- Reconnect MUST return every retained authorized frame after the cursor or explicitly return resync-required with an authorized current projection.
- It MUST NOT silently skip an unavailable range.
- Each connection MUST have bounded input size, output bytes, frame count, and send time.
- A slow consumer MUST be disconnected without blocking the room actor or growing memory without bound.

### FR-7: Explicit agent activation

- An Activity Pack MAY emit an Attention Signal for an Agent Participant's Membership as deterministic Transition output.
- The host MUST convert an allowed attention signal into a durable activation intent in the same transaction as the transition.
- The host MUST reject an Attention Signal unless its target Membership is enabled, has participant Access Mode, belongs to an agent Principal, and has a pack-permitted Role.
- An activation MUST identify cause sequence, reason code, target membership, relevant cursor range, allowed action types, priority, and optional deadline.
- A runner MUST claim an activation with a bounded lease before reporting work on it.
- An Activation claim MUST NOT grant participant Action authority or advance the Membership Cursor. Actions and Observation acknowledgements require separate participant authority, as decided in [ADR 0003](adr/0003-separate-activation-and-action-authority.md).
- Activation delivery MUST be at-least-once and creation MUST be unique by room, cause sequence, target membership, and pack deduplication key.
- A pack MUST emit at most one Attention Signal per target Membership per Transition, and the host MUST allow at most one live Activation lease per Membership in the frozen releases.
- Claim retries MUST be idempotent by activation ID and claim ID. Every lease MUST have a generation or opaque token so an expired prior claimant cannot renew, release, or complete a newer lease.
- If no runner is available, the intent MUST remain pending until expiry or host-operator cancellation; WorldStream MUST NOT pretend that an agent ran.
- A fresh invocation MUST receive the cause, authorized current projection, retained observation frames after its cursor, budget/deadline metadata, and explicit artifact references.
- v0.1 MUST support a connected runner control channel or HTTP long poll. Arbitrary outbound webhooks are not required.
- Replay MUST reconstruct activation decisions but MUST NOT contact runners or start invocations.

### FR-8: Recovery and replay

- Canonical transitions MUST be append-only.
- Every room MUST store an immutable canonical genesis record containing pack digest, configuration, ordered initial memberships/roles, seed, and recorded creation time.
- Snapshots MUST be replaceable performance caches, never the only source of truth.
- Startup MUST reconstruct each accessed room from genesis when no valid snapshot exists, or from its latest compatible verified snapshot and transition tail.
- A committed action that was acknowledged before process termination MUST not be lost.
- A commit that occurred before a lost acknowledgement MUST return the stored original result when retried.
- Replay MUST be read-only, use the exact pack revision, and verify recorded Activity State hashes plus the ordered Core Room State changes.
- A hash mismatch or missing pack revision MUST fault replay instead of continuing with uncertain state.
- Timeline branching, promotion, or merge is not part of v0.1 or v0.2.

### FR-9: Reference user interface

- The server MUST be usable without the web UI.
- The first-party UI MUST consume only authorized public, participant, or operator projections.
- v0.1 MUST include a Heist public board/map, participants, current phase/deadline, activation state, timeline, and replay controls.
- v0.2 MUST add Investigation evidence, claim, challenge, correction, timeline, final brief, and deterministic score views.
- The UI MUST submit the same typed commands as another client and MUST NOT enforce server authorization by itself.
- Runtime LLM-generated UI, arbitrary pack JavaScript, third-party renderers, and a general View Pack ABI are deferred.

### FR-10: Developer experience

- v0.1 MUST provide an async Python SDK and raw protocol examples.
- The SDK MUST handle reconnect, observation acknowledgement, duplicate delivery, activation claim lease, and safe action retry.
- The SDK MUST expose Runner activation-control authority separately from Room Member observation and participant Action authority.
- Deterministic bot runners MUST exercise the complete Heist without a paid model API.
- v0.2 MUST provide deterministic Investigation agents and one human action path.
- A fresh checkout MUST start the server, reference clients, and UI in under ten minutes on a documented supported platform.

## Reference release A: Agent Heist v0.1

Agent Heist is a deliberately small game used to prove the Room Kernel, not an MMO or social network.

Required Room Members and input:

- three distinct Agent Principals with participant Memberships in the Navigator, Insider, and Broker Roles;
- three deterministic external Runners serving those Agent Participants in the reference demo;
- one human with an operator or spectator membership;
- one deterministic facility host-stimulus source represented by recorded timers.

Required behavior:

- role-specific private clues;
- public clue publication and structured private offers;
- plan proposal and endorsement;
- one shared sealed-decision window in which concurrent submissions are serialized and stale submissions retry before the deadline;
- deterministic conflict and outcome resolution;
- at least one timed phase change;
- at least one targeted activation while an agent invocation is absent;
- one runner claiming the activation and starting a fresh invocation;
- cursor catch-up without receiving another role's private data;
- server termination after a committed action and successful restart recovery;
- deterministic read-only Replay to the same final Activity State hash and Core Room State.

Required Heist acceptance gates:

1. Three deterministic agents drive the Heist Activity to its Terminal Phase and Outcome solely through public protocol and SDK calls; the Room remains active until explicitly archived.
2. One invocation terminates before the commitment phase.
3. Commitment opening creates a durable targeted activation.
4. A fresh invocation claims it, catches up, and submits a valid commitment.
5. Private clues, offers, and sealed choices never reach unauthorized participants or the public UI.
6. Retrying an accepted action after a lost acknowledgement does not apply it twice.
7. Killing the server during the scripted failure point loses no acknowledged state.
8. Replay produces the same checkpoint hashes, outcome, and public history.
9. The deterministic demo requires no network service, model key, wallet, or paid API.

## Reference release B: Investigation Room v0.2

Investigation Room proves the Room Kernel supports serious non-game collaboration without Investigation-specific room, protocol, storage, or activation semantics. It uses the generic artifact subsystem already planned for v0.2.

The bundled fictional fixture is Cold Chain Incident: a shipment appears to have exceeded its permitted temperature range. Evidence arrives in recorded waves, including sensor readings, manifest data, maintenance records, a witness report, and a later timestamp correction.

Required Room Members and input:

- one human investigation lead who assigns or reviews work and submits the final brief;
- one timeline analyst agent;
- one evidence analyst agent;
- one challenger/verifier agent;
- one deterministic evidence-feed host-stimulus source.

Required typed domain behavior:

- register immutable evidence and versions;
- assign or claim evidence;
- publish a source-linked fact;
- propose, support, challenge, revise, or withdraw a claim;
- request and resolve verification;
- mark claims stale when a cited evidence version is superseded;
- notify affected human Room Members and create Activation Intents for affected Agent Participants;
- submit a structured final brief with conclusion, timeline, evidence references, contradictions, uncertainties, and confidence;
- score the brief against a deterministic fixture rubric rather than an LLM judge.

Required Investigation acceptance gates:

1. It uses the Activity Pack host interface frozen after Heist without adding domain fields to core protocol or storage; only the preplanned generic artifact subsystem may be added.
2. The human lead and agents act through the same action submission path.
3. Active participants receive different authorized evidence and draft-work projections.
4. A recorded correction supersedes evidence, marks dependent claims stale, notifies the human Lead, and creates Activation Intents for affected Agent Participants.
5. A fresh invocation reconstructs only authorized current context and source references.
6. The lead submits a source-linked structured brief and receives a deterministic score.
7. Restart and replay reproduce the final case board, activation decisions, brief, and score.
8. There are no live web requests, business-system writes, or LLM judging inside the activity.

The generality gate fails if Investigation needs a new core room lifecycle, an Investigation-specific protocol message, special-case persistence tables beyond generic artifact metadata, or server-side model logic.

## Non-functional requirements

### Storage profiles and backend-neutral durability

The frozen storage and portability decision is recorded in [ADR 0004](adr/0004-supported-storage-profiles-and-offline-portability.md); release, recovery, and evidence consequences are recorded in [ADR 0011](adr/0011-release-compatibility-recovery-and-supply-chain-gate.md).

- v0.1 and v0.2 MUST support exactly two startup-selected durable profiles: the default release-bundled SQLite profile and `postgres-primary` against one writable hosted or self-managed PostgreSQL 17 primary.
- Exactly one WorldStream process serves a deployment under either profile. A remote PostgreSQL primary imposes no same-host restriction, but a second live WorldStream process, authoritative replica reads, automatic failover, and provider-specific correctness dependencies are unsupported.
- Both profiles MUST expose the same logical transaction boundary, canonical bytes and hashes, operation identities and receipts, timers, frames/cursors, Activation evidence and fencing, failure classes, recovery, and Replay. Selecting a backend MUST NOT change Room semantics.
- SQLite MUST use the exact bundled build and required WAL, `synchronous=FULL`, foreign-key, bounded-busy, query-only-reader, local-filesystem, and controlled-writer policy recorded in the release manifest. Host SQLite, network/UNC filesystems, and shared writers MUST fail closed.
- PostgreSQL MUST accept major 17 only, use `synchronous_commit=on`, Read Committed transactions with durable compare-and-set/fence predicates, a least-privilege runtime role, and TLS for remote connections. Direct, session-pooled, and bounded transaction-pooled runtime connections are supported; correctness MUST NOT depend on extensions, session state, named prepared statements, provider APIs, or a connection surviving between transactions.
- Database writes that form one authoritative operation MUST be atomic. Storage-full/unavailable, corrupt snapshot, pack failure, and slow-client paths MUST fail closed with stable actionable diagnostics.

### Migration and compatibility contract

- One ordered logical migration history and schema-contract fingerprint MUST govern both profiles; backend-specific DDL/execution MAY differ but every migration is checksummed, atomic or restart-safe, and covers an empty database and every earlier v0.1 schema.
- Production migrations are forward-only. Down migrations, rolling mixed binary/schema versions, and starting an old binary after migration are unsupported; rollback restores a pre-upgrade backend backup together with the previous binary.
- SQLite MAY migrate automatically only during exclusive locked startup after creating and verifying a recoverable backup. Production PostgreSQL migration MUST use an explicit offline maintenance command, a direct admin connection, and no serving process; the daemon uses only its runtime role and verifies the resulting schema. Development-only auto-migration does not count as production evidence.
- Every release MUST publish, embed, and enforce reviewed `compatibility.toml` plus canonical `compatibility.json`. The Storage Compatibility Manifest MUST pin product/wire/config/storage/Core/hash versions, engine builds and settings, schema and migration checksums, connection modes, canonical and receipt codec writers plus retained readers, exact retained Activity Pack executors, transfer/recovery formats, platform support, and verification evidence.
- v0.1.0 pins wire `0.1`, config `1`, storage schema `1`, Core semantics `1`, `blake3-canonical-json-v1`, Rust 1.97.1 edition 2024, Node 24.19.0 LTS as build-only, Python SDK 3.11–3.14, and Python 3.14.7 for the quickstart. It bundles SQLite 3.53.4, treats 3.51.3 as the frozen corrective floor, denies 3.52.0, and supports PostgreSQL 17 from 17.11; the manifest distinguishes release-verified 17.x patches from newer supported-but-unverified 17.x patches, and other majors fail closed.

### Transfer, backup, restore, and semantic verification

- WorldStream MUST provide one versioned, resumable, whole-deployment, offline transfer from SQLite to an empty PostgreSQL target. It MUST NOT provide live switching, dual writes, reverse transfer, or a backend-fallback path.
- Transfer MUST quiesce serving, create and verify a recoverable source backup, mark the source `transfer_pending`, emit a deterministic checksummed manifest, import while neither backend serves, run full target verification, and require explicit finalization to retire SQLite and activate PostgreSQL under the next monotonically increasing Storage Epoch.
- The transfer bundle MUST copy canonical serialized bytes verbatim rather than decode/re-encode them through PostgreSQL types. It MUST preserve lineage/export identity, Genesis, Transitions, every Head and hash, Core/Activity materializations, Memberships and authority, exact timer identity/generation/due values, Frames/Cursors, operation identities/payload hashes/dispositions/receipts, Activation intents/claims/audit/fences, principals/capabilities/revocations, integrity incidents, and artifact metadata and bytes.
- Snapshots, indexes, caches, telemetry, sessions, runner presence, mailboxes/delivery attempts, and temporary state MAY be rebuilt or invalidated. Scheduled timers retain their recorded due values and enter normal CatchingUp after transfer; no fire or generation is invented. Nonterminal Activation leases MUST be fenced before target readiness while eligible intents remain reclaimable.
- Before source retirement, abort MUST discard the target and leave the verified SQLite source authoritative. After PostgreSQL accepts its first write in the new epoch, rollback to SQLite is unsupported; retired SQLite remains a read-only recovery artifact unless an explicit destructive override abandons continuity.
- Backup mechanisms MUST be backend-native: WorldStream owns SQLite online backup/restore orchestration; PostgreSQL uses operator/provider-native snapshot, PITR, dump, and restore facilities over a direct admin path. Every restore and transfer target MUST then pass the read-only, full WorldStream semantic verifier.
- The verifier MUST check backup ID, lineage, schema, manifest, artifact digests/bytes, receipts, timers, Frames/Cursors, Activation fencing, exact retained pack executors, and Genesis-to-Head replay with every hash for every healthy Room. Global mismatch blocks readiness. A byte-preserved Room already marked faulted or quarantined MAY remain isolated and unhealthy without blocking verified healthy Rooms; a newly introduced mismatch aborts verification.

### Reference one-process performance envelope

These are release targets, not claims until measured on documented hardware and payloads:

- 1,000 concurrently connected mostly idle WebSocket clients;
- 100 simultaneously loaded small rooms;
- 100 accepted actions per second in aggregate for the reference payload profile;
- p95 local action acknowledgement below 100 milliseconds;
- recovery of a 100,000-transition room from a recent snapshot and tail within 5 seconds;
- a one-hour soak with bounded process memory, queues, WAL size, and artifact temp space.

The report MUST separate connection count, active rooms, transition rate, observation fan-out, p50/p95/p99 latency, memory, database growth, and recovery time. WorldStream MUST NOT describe these targets as internet scale.

### Supported release and deployment profiles

| Profile | Required support and release evidence |
|---|---|
| Native Linux | `x86_64-unknown-linux-musl`, kernel 5.15+, local ext4/XFS for SQLite; Ubuntu 24.04 x86-64/ext4 is the release reference. |
| Native Windows | `x86_64-pc-windows-msvc`; Windows 11 25H2+ or Server 2022/2025 on fixed NTFS/ReFS; SQLite and PostgreSQL profiles are both product-supported and build/package, ACL/filesystem, SQLite, PostgreSQL-connect, and recovery evidence MUST execute natively. |
| OCI | `linux/amd64` only; static/minimal, user 65532, read-only-root compatible, persistent `/var/lib/worldstream`; SQLite on a writable container overlay MUST be rejected. |
| macOS quickstart | Source-build development path on macOS 15+ APFS for Intel and Apple Silicon; no macOS release binary/archive. |

- Release artifacts MUST include native server/CLI, embedded UI, examples and Heist clients, licenses, the compatibility manifest, checksums, Sigstore signatures, SPDX SBOM, and SLSA provenance as applicable to that profile. Cross-compilation alone is never platform evidence.
- ARM64 release artifacts, macOS binary distribution, Windows containers, MSI/MSIX, Windows Service integration, package repositories, Kubernetes/Helm, cloud resources, and release-pipeline implementation are outside the frozen releases.
- Configuration precedence MUST be compiled defaults, one explicitly selected versioned TOML file, `WORLDSTREAM__SECTION__KEY` environment values, then documented CLI flags; `--config` beats `WORLDSTREAM_CONFIG`, and no current-directory or home-directory search is allowed. Unknown, duplicate, wrong-type, out-of-range, inactive-backend, or unsupported values MUST fail startup.
- DSNs, capabilities, and exporter credentials MUST arrive only through owner-readable secret files or inherited handles, never plaintext TOML, CLI arguments, or logs. Admin/migration credentials exist only in offline maintenance commands, not the daemon. `config validate`, redacted `config effective`, and `doctor` MUST expose configuration, permission, filesystem, backend, migration, capacity, and manifest diagnostics.
- `/healthz` reports only process/event-loop liveness. `/readyz` requires current schema, globally healthy authoritative storage with writable capacity, and running writer/scheduler; telemetry failure or an already isolated unhealthy Room does not fail readiness. `/version` reports product/build, protocol/config/storage/Core/hash/manifest, and exact engine identity. `/metrics` is low-cardinality Prometheus exposition.
- Structured JSON logs, Prometheus metrics, W3C trace correlation, and an optional OpenTelemetry/OTLP export seam MUST remain vendor-neutral and outside admission, reduction, commit, replay, Room Health, and readiness. Post-commit telemetry is bounded and nonblocking, holds no Room/database lock, emits a drop metric and rate-limited warning on overflow, and receives at most a bounded three-second shutdown flush.
- Correctness, durability, resource bounds, crash/replay/hash parity, migrations, transfer, restore, semantic verification, filesystem/permission/disk-full behavior, and both storage profiles are hard release gates. Performance is measured separately per backend on the Linux reference profile and published as reference evidence, never a universal blocker, SLA, or Windows performance claim.

### Security posture

- The frozen releases are a self-hosted developer preview, not a hardened public multi-tenant service.
- The host operator and compiled-in packs are trusted.
- Network clients, human input, agent output, evidence text, and file metadata are untrusted.
- Authentication, authorization, payload limits, rate limits, path safety, projection isolation, HTML escaping, and secret-safe logging are required.
- Investigation artifacts MUST be immutable, content-addressed, size-limited, MIME-checked, and never executed by WorldStream.

## Explicit non-goals through v0.2

The following are frozen out:

- workflow canvas, node graph, ETL, connector catalog, cron replacement, or job orchestrator;
- generic project management, enterprise finance reporting, or running an entire company;
- Project or Workspace entities, cross-room exchange, cross-room memory, or multi-pack composition;
- coding harnesses, repository worktrees, tool sandboxes, branch promotion, or cloud agent execution;
- model hosting, prompt management, provider routing, consumer-subscription pooling, or raw model resale;
- generic RAG, vector database, embedding pipeline, semantic wake classifier, or automatic summarization;
- Activity Pack marketplace, public pack upload, public agent marketplace, reputation, payments, token, wallet, escrow, or blockchain integration;
- arbitrary process snapshots, hidden-model-state capture, or claims of continuous agent life;
- timeline forks, branch merge, or counterfactual promotion;
- runtime-generated UI, general dashboard builder, arbitrary third-party JavaScript, or renderer marketplace;
- A2A, MCP, AG-UI, OpenClaw, Hermes, or other full protocol integrations beyond small examples;
- Wasmtime or another untrusted plugin sandbox;
- Redis, NATS, Kafka, Temporal, a service mesh, Kubernetes requirement, Raft, CRDTs, federation, active-active mutation, or multi-region operation;
- live backend switching, dual writes, PostgreSQL-to-SQLite or room-at-a-time transfer, reverse transfer, multi-process serving, authoritative replica reads, automatic failover, HA orchestration, provider services or correctness dependencies, and cloud-resource provisioning;
- production SaaS tenancy, billing, moderation, compliance certification, or uptime SLA.

These ideas are not rejected forever. They require evidence after both reference releases and a separate ADR.

## Requirements freeze checklist

- [x] The server-to-room-to-one-pack hierarchy appears consistently.
- [x] Human and agent Principals can both become first-class Participants.
- [x] Membership, session, runner, invocation, cursor, and activation are distinct.
- [x] No normative document calls an agent process alive, asleep, awakened, or mentally resumed.
- [x] Action ordering, idempotency, commit-before-ack, projection privacy, cursor catch-up, activation, recovery, and replay have testable invariants.
- [x] Agent Heist is the only v0.1 activity.
- [x] Investigation Room is the only v0.2 application goal.
- [x] Investigation adds no Investigation-specific Room Kernel concept beyond the preplanned generic artifact subsystem.
- [x] Bundled SQLite and `postgres-primary` are the only storage profiles, with one WorldStream process and backend-neutral semantics.
- [x] Forward-only migrations, retained codecs, offline one-way transfer, Storage Epoch fencing, backend-native recovery, and full semantic verification are testable invariants.
- [x] Native Linux/Windows, Linux/amd64 OCI, macOS source-only, config/secrets/probes/telemetry, supply-chain evidence, and all negative release clauses are explicit.
- [x] All excluded marketplace, crypto, workflow, cross-room, coding, memory, plugin, and generated-UI ideas are non-normative.
- [x] Every performance statement is labeled target or accompanied by a reproducible report.
