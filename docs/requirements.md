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
- typed action routing, Operation Identities, Semantic Receipts, and stable results;
- typed Semantic Time, the bounded Room Admission Lane, and host-owned Timer Generations;
- paired Core+Activity snapshots, recovery, current projection, and deterministic replay;
- Membership-addressed Observation Frames, acknowledgements, and Cursor Catch-up;
- durable activation intents, claim leases, retries, and status;
- bounded network queues, rate limits, and slow-consumer handling;
- a minimal operator-membership/reference UI and protocol/SDK conformance fixtures.

### Activity Packs MUST own

- room configuration and domain state;
- Role definitions, cardinality, permissions, and Legal Actions;
- action validation and deterministic reduction;
- public, participant, and operator projection rules;
- semantic timer mutation requests, attention reasons, completion rules, and result scoring;
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
- Core Room Status MUST be active or archived; operational Room Integrity State MUST be healthy, faulted, or quarantined; Activity Phase and Outcome MUST remain separate pack-defined values and separate from both axes.
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

### FR-3: Prepared Room Commit, identity, and ordering

- Every Action MUST include Room ID, Membership ID, client-generated Action ID, expected Room sequence, Action type, and typed payload. Each active Room MUST have one logical writer and one monotonically increasing committed sequence.
- Deterministic preparation MUST finish before a storage transaction opens and MUST produce one immutable, versioned `PreparedRoomCommit`. It MUST contain its Operation Identity, Canonical Request Hash, complete observed Head, integrity, authority/capability, policy, and operation-specific input witnesses, and exactly one prepared intent: `Advance` or `DurableDisposition { Rejection | NoChange }`.
- Operation Identities MUST be exactly: `(room_id, member_id, action_id)` for an Action; `(authenticated_principal, versioned_operation_kind, idempotency_key)` for Room administration; `(room_id, timer_id, generation, scheduled_for)` for a timer firing; and `(room_id, source_id, input_id)` for host/external input.
- A versioned Canonical Request Hash MUST bind all caller-semantic input, including the target Room, expected basis, operation kind, and complete ordered payload/changeset. It MUST NOT include Action `admitted_at`, a generated Transition ID, commit time, transport-envelope identity, or retry-attempt data.
- The same Operation Identity and Canonical Request Hash MUST resolve the original Semantic Receipt before later Room lifecycle, integrity, or Membership checks, subject to current authentication and permission to read it. The same identity with a different hash MUST resolve `Conflict`; an implementation MUST NOT reinterpret it as a new operation.
- Every new Advance or durable disposition MUST take the transaction-scoped Room write fence and recheck identity absence plus the complete Head: exact `room_seq`, prior Transition hash, Core hash, Activity hash, and aggregate Authoritative hash. Equal sequence with any different hash MUST be an integrity fault, not contention.
- The same guarded transaction MUST revalidate healthy and unchanged integrity generation; exact authority/capability generation and revocation state; current policy revision when noncanonical policy decisions are included; and the exact timer, Action, administration, or external-input witness. Participant Actions MUST never be silently rebased onto another Head.
- Durable database `COMMIT` MUST be the sole linearization point. Lane reservation, row locks/updates, driver return, actor-memory installation, acknowledgement, and publication MUST NOT be treated as public success or Room order.
- The semantic outcome algebra MUST be `Resolved(TransitionCommitted { New | Existing })`, `Resolved(RejectionRecorded { New | Existing })`, `Resolved(NoChangeRecorded { New | Existing })`, `NotApplicable`, `Reprepare`, `Fenced`, `Conflict`, `RetryableKnownAbsent`, `Indeterminate`, or `Fault`. SQLite and PostgreSQL MUST expose the same classifications.
- `Indeterminate` MUST mean COMMIT may or may not have occurred. The server MUST query the authoritative primary with the same Operation Identity and Canonical Request Hash until it finds the stored resolution or proves absence; it MUST NOT blindly retry, invent an identity, re-run the pack, or publish an assumed result.
- Only `RetryableKnownAbsent` MAY cause a bounded retry of the identical sealed plan. `Fault` is known absent, nonretryable, and reserved for malformed sealed plans or verified invariant/hash failure. `Reprepare` MUST discard the plan: Actions/administration resolve a stable stale basis where applicable, a still-scheduled timer reuses its exact identity and recorded fields against the new Head, and a policy-only change recomputes only the affected noncanonical decision. `Fenced`, `Conflict`, and `NotApplicable` MUST NOT create a new Semantic Receipt.
- A durable Rejection MUST be limited to an authenticated, well-formed stable result at an exact fenced Head. An administrative NoChange MUST mean the normalized desired state was already true before pack application. Malformed input, authentication or authority failure, rate/capacity refusal, unhealthy integrity, Activity Fault, storage failure, and obsolete timer candidates MUST NOT become durable dispositions.
- Every accepted Stimulus, including one whose resulting Core/Activity bytes are equal, MUST create one Transition. Only administrative NoChange and a stable rejection consume no `room_seq`.
- One Advance transaction MUST atomically persist the Transition and complete hash chain; new Head and Core/Activity serving materializations; final Membership changeset; timer consumption/schedules/cancellations; addressed Observation Frames and frame heads; activation-policy decision/revision, allowed Activation Intents, and eligibility/archive fences; and the applicable Semantic Receipt.
- Pack execution, deterministic reduction, canonicalization, bound checks, three-hash computation, projection/frame computation, and Attention derivation MUST finish before locks. Network publication, telemetry export, derived indexing, and paired Core+Activity snapshot writes MUST happen after COMMIT and MUST never extend the Room transaction.
- Every Semantic Receipt and equivalent compact tombstone MUST remain resolvable for the retained Room lineage, including after archive. A whole-Room purge MAY remove history and its receipts together; silent receipt expiry or identity reuse is forbidden.

### FR-4: Deterministic state, Semantic Time, and timers

- Authoritative Room State at sequence N MUST be a pure function of Room Genesis, the exact Activity Pack revision, and recorded Stimuli through N. A pack MUST NOT read ambient wall time, operating-system randomness, files, network resources, environment variables, provider APIs, or secrets.
- There MUST be no universal Transition timestamp. Participant Actions MUST carry host-recorded `admitted_at`; one Timer Generation MUST carry immutable `scheduled_for`, which is its TimerFired semantic time; and Membership, administration, and external inputs MUST carry their versioned stimulus-specific `recorded_at`. Scan, enqueue, retry, lag, dequeue, transaction, receipt, and commit times MUST remain operational and invisible to pack reduction.
- One application-owned, injectable, normalized-UTC HostClock MUST supply semantic host samples and due eligibility. Client clocks and database clock functions MUST NOT define domain semantics. Issued trusted samples MUST not decrease; an untrustworthy rollback or configured large discontinuity MUST fail time-bearing canonical work closed without reopening deadlines or rewriting committed time.
- For a new Action, the server MUST sample `admitted_at` atomically with successful reservation of a bounded Room Admission Lane position, and only after the complete request is strictly parsed/schema-valid, initially authenticated, rate-admitted, and within size limits. A full/unavailable lane MUST return `room_busy` with no `admitted_at`, receipt, or deadline entitlement.
- Action windows MUST be half-open: `open_at <= admitted_at < deadline`. An Action admitted exactly at or after the deadline MUST receive durable `deadline_passed` even if the closing timer is delayed. A timely Action MAY commit after wall time passes the deadline only if every witness still passes and neither its closing timer nor archive has committed first.
- Participant Actions, canonical administration, and newly due timer candidates MUST reserve positions in one bounded per-Room lane. A due timer MUST NOT overtake earlier reservations; once reserved, later participant work MUST NOT overtake it. Capacity reserved for host stimuli MUST prevent participant saturation from starving due timers. Lane positions are provisional and disappear on crash; database COMMIT remains the only canonical order.
- WorldStream MUST own a monotonic generation for each `(room_id, timer_id)`, starting at one and never reused, wrapped, or chosen by a pack. At most one generation may be scheduled; its `scheduled_for`, payload, and creation cause MUST be immutable.
- Packs MAY request only `ScheduleNext`, `CancelCurrent(expected_generation)`, or `RescheduleCurrent(expected_generation, new_scheduled_for, new_payload)`, normalized to at most one mutation per logical timer ID per Transition. The host MUST deterministically allocate the next generation and replay/retry of the causing Transition MUST resolve that existing generation.
- Every newly scheduled generation MUST be strictly later than the causing Stimulus's typed Semantic Time; an initial Genesis timer MUST be strictly later than the typed recorded creation time. Equal/backward time, implicit replacement, conflicting duplicate mutations, wrong expected generation, cancelling missing/fired/cancelled state, invalid payload, overflow, or unrepresentable time MUST be an Activity Fault with no commit.
- A generation becomes due when `HostClock >= scheduled_for`. TimerFired MUST be reconstructed from the immutable `(room_id, timer_id, generation, scheduled_for, payload)` row and MUST have no pack-visible `fired_at` or separate durable claim. Its exact scheduled-generation witness MUST be consumed by the same Advance that commits its Transition.
- A matching already-fired generation MUST resolve `Existing`; missing/cancelled/obsolete/archive-cancelled MUST resolve `NotApplicable`; a Head change with the generation still scheduled MUST reprepare the same identity and recorded fields. An unknown timer COMMIT MUST be resolved before the candidate is scanned or prepared again.
- Every scheduled due generation MUST remain a durable obligation until it fires, is canonically cancelled/rescheduled, archive cancels it, or integrity/storage temporarily fences progress. Lag, restart, rate limits, or resource budgets MUST NOT expire, merge, coalesce, reorder, skip, or falsely mark it fired.
- Within one Room, due generations MUST be considered one at a time in `(scheduled_for, timer_id, generation)` order and reread after each result. No semantic ordering exists between Rooms.
- Loading a Room with overdue timers MUST enter `CatchingUp`: capture one HostClock cutoff, recursively drain every still-applicable generation with `scheduled_for <= cutoff` in deterministic order, then become Active. Bounded slices MAY yield to other Rooms and runtime/storage duties, but ordinary same-Room canonical commands MUST NOT interleave before the fixed cutoff is drained. Timers becoming due after the cutoff enter the normal Room Admission Lane.
- Capability revocation, integrity fault/quarantine, and diagnostics MUST remain available during CatchingUp. Valid overdue cascades remain obligations; an expensive Room MAY be throttled or operationally faulted without discarding them. Invalid/non-progressing output and time/generation overflow follow the Activity Fault path.
- Canonical Activity State, Core State, and Transition/hash outputs MUST be reproducible without a clock or scheduler during Replay. Core Room State and Activity State MUST avoid floating-point values.

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

### Correctness and durability

- SQLite durability settings and benchmark settings MUST be published.
- Database writes that form one transition MUST be atomic.
- Startup MUST run migrations, verify supported SQLite capabilities, check data-directory permissions, and validate room head/snapshot consistency.
- Storage-full, corrupt snapshot, pack failure, and slow-client paths MUST fail closed and produce actionable host-operator diagnostics.

### Moderate single-node performance envelope

These are release targets, not claims until measured on documented hardware and payloads:

- 1,000 concurrently connected mostly idle WebSocket clients;
- 100 simultaneously loaded small rooms;
- 100 accepted actions per second in aggregate for the reference payload profile;
- p95 local action acknowledgement below 100 milliseconds;
- recovery of a 100,000-transition room from a recent snapshot and tail within 5 seconds;
- a one-hour soak with bounded process memory, queues, WAL size, and artifact temp space.

The report MUST separate connection count, active rooms, transition rate, observation fan-out, p50/p95/p99 latency, memory, database growth, and recovery time. WorldStream MUST NOT describe these targets as internet scale.

### Supported deployment

- v0.1 and v0.2 MUST run as one process on one host with local SSD-backed storage.
- SQLite WAL files MUST remain on the same local filesystem as the main database.
- Network filesystems, multi-writer shared volumes, stateless replicas, and multi-region writes are unsupported.
- Docker packaging MAY be provided, but the data directory MUST be bind-mounted or placed on a persistent local volume.

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
- [x] All excluded marketplace, crypto, workflow, cross-room, coding, memory, plugin, and generated-UI ideas are non-normative.
- [x] Every performance statement is labeled target or accompanied by a reproducible report.
