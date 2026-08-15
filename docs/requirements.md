# Frozen Requirements

## Document authority

Status: **FROZEN for Agent Heist v0.1 and Investigation Room v0.2**

Freeze date: 2026-08-15

This is the normative product-behavior and release-scope document. If an architecture, protocol, roadmap, or example conflicts on behavior or scope, this document wins. The root [WorldStream Domain Context](../CONTEXT.md) is authoritative for domain term names and meanings; a conflict between terminology and requirements is a documentation defect that must be reconciled rather than silently redefined.

Change control:

1. A requirement addition needs a short ADR describing the demonstrated need.
2. Before v0.2, new scope must replace scope of comparable cost unless it fixes correctness, security, or the ability to deliver either reference activity.
3. Ideas that do not block a release gate go into the non-normative research backlog.
4. The trusted semantic `ActivityPackV1` seam is frozen for v0.1 retained Rooms. Public protocol stability and any portable, dynamically loaded, or untrusted pack ABI remain unpromised until a separate post-v0.2 decision.

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
- durable timers, monotonically nonreused host-owned timer generations, and host-recorded nondeterministic inputs;
- state snapshots, recovery, current projection, and deterministic replay;
- Membership-addressed Observation Frames, acknowledgements, and Cursor Catch-up;
- durable activation intents, claim leases, retries, and status;
- bounded network queues, rate limits, and slow-consumer handling;
- a minimal operator-membership/reference UI and protocol/SDK conformance fixtures.

### Activity Packs MUST own

- room configuration and domain state;
- Role definitions, cardinality, permissions, and exact Action Offers;
- deterministic initialization, reduction, views, and observations through `ActivityPackV1`;
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
- A Room MUST NOT change its pinned digest or rewrite canonical Activity State in place. A semantic pack revision creates a new Room.
- Every revision digest MUST be the build-computed digest of a canonical `PackRevisionLockV1` covering the host contract and codec versions, manifest, exact schema-content digests, deterministic static data, pack rule source, and deterministic dependency lock.
- The embedded registry MUST map each digest to its exact executor, descriptor/schema bundle, state/stimulus/output codecs, golden-corpus digest, and separate selectable-for-new-Rooms and runnable-for-retained-Rooms status. Selectable MUST imply runnable.
- Every digest referenced by retained lineage MUST remain runnable for load, advance, view, observe, Recovery, and Replay even after it becomes non-selectable.
- Core Room Status MUST be active or archived; Room Health MUST be healthy, faulted, or quarantined; Activity Phase and Outcome MUST remain separate pack-defined values and separate from both core axes.
- Archiving a Room MUST be recorded as an administrative Stimulus and Transition in that Room's order, as decided in [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md).
- Archived, faulted, and quarantined rooms MUST reject ordinary participant mutation while retaining authorized inspect, export, recovery, and replay operations.
- v0.1 packs MUST be trusted, compiled into the server, and selected from an allowlist.
- Dynamic pack download, public plugin upload/registry service, untrusted code execution, and a portable plugin ABI are deferred until after v0.2. The required embedded exact-revision registry is not a plugin marketplace.
- These pack-seam and executable-retention boundaries are decided in [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

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
- The host MUST pre-admit an action type only when it appears in the exact current viewer's canonical Action Offer bytes. The pack MAY still declare a payload-specific domain rejection during reduction.
- The same Action Offer bytes MUST appear in current Projection, Projection Reset, Observation Frame updates, Invocation Context, and host pre-admission; no second legality list MAY exist.
- Rejected actions MUST NOT consume a canonical room sequence.
- Only deterministic admitted rejections MAY consume the action ID through a durable action receipt.
- Authentication, authorization, malformed input, rate limit, room busy, storage unavailable, and activity/runtime faults MUST use the error path, MUST NOT consume the action ID, and MAY be retried with the same ID.
- An accepted action MUST be durably committed before the server acknowledges or publishes it.
- The key of room, membership, and action ID MUST map to at most one result.
- Reusing an action ID with different canonical payload bytes MUST be rejected.
- Stale actions MUST receive a typed rejection with the current sequence and exact current Action Offers when safe.

### FR-4: Deterministic state and timers

- Authoritative Room State at sequence N MUST be a pure function of Room Genesis, the exact Activity Pack revision, and recorded Stimuli through N.
- The trusted pack seam MUST expose exactly five synchronous operations: descriptor, initialize, reduce, view, and observe. Each MUST finish before persistence handoff and MUST be pure and bounded.
- A pack MUST NOT receive or access ambient wall time, operating-system randomness, files, network resources, environment variables, storage, scheduler, Activation, Session/delivery state, telemetry, artifact bytes, provider APIs, or secrets.
- Initialization MUST receive exact creation inputs and return only canonical initial Activity State plus ordered timer requests. Genesis MUST create no Domain Event, Attention Signal, Activation, or Observation Frame.
- Reduction MUST receive prior Activity State, exact Core before/proposed after, the canonically sorted current timer view, next Room sequence, and one normalized typed Stimulus.
- Normalized participant Actions MUST record exact basis Head and `admitted_at`; TimerFired MUST contain only exact logical timer ID, host generation, immutable `scheduled_for`, and payload; Core proposals and narrow External Inputs MUST carry canonical `recorded_at`.
- Reduction MUST return exactly Apply with complete next state, ordered Domain Events, timer requests, and Attention Signals, or a declared Reject. `PackFault` MUST remain a distinct contract-failure channel.
- Clean Reject MAY apply only to participant Actions and join, resume, Access Mode, or Role proposals. Archive, suspend, depart, required TimerFired, and accepted External Input MUST NOT be vetoed; attempting to do so is `PackFault`.
- Host-normalized administrative NoChange MUST resolve before pack entry. Every Apply MUST create one Transition even when Activity State bytes do not change.
- Host time and randomness that affect state MUST enter as recorded stimuli or recorded stimulus fields.
- Packs MAY request only ScheduleNext, CancelCurrent with expected generation, or RescheduleCurrent with expected generation/new due/new payload. The host MUST assign and verify monotonically nonreused generations.
- A Transition MUST request at most one mutation per logical timer ID. Every new due MUST be strictly later than the causing Stimulus's semantic time; wrong generation, equal/backward time, implicit replacement, conflicting request, invalid payload/time, or overflow MUST be `PackFault` with no commit.
- Timer creation, cancellation, rescheduling, and logical firing MUST be durable.
- A timer retry MUST NOT cause two logical firings.
- Canonical Activity State and Transition hashes MUST be reproducible.
- Core Room State and Activity State in the frozen releases MUST avoid floating-point values.
- Panic where catchable, malformed/undeclared output, bound violation, privacy/view failure, or deterministic disagreement MUST fail closed before commit. Required-input repeat failure faults the Room; Replay/hash disagreement quarantines it.

### FR-5: Scoped projections

- The authoritative room state MUST NOT be sent directly to untrusted clients.
- The pack MUST construct separate public, participant, and operator Activity Projections; WorldStream MUST wrap them with authorized Core Room State and Membership metadata without allowing either layer to overwrite the other.
- Every persisted observation frame MUST have an explicit membership audience.
- Every durably streamed spectator or operator MUST therefore have a read-only room membership and its own cursor.
- Authorization MUST happen before persistence, indexing, ranking, or rendering.
- A participant MUST obtain exact current Action Offers and deadlines from its authorized Projection without reading raw transition history.
- `view` MUST return one authorized Activity Projection plus ordered Action Offers. `observe` MUST receive before/after Core and Activity, normalized Stimulus, ordered events, exact Membership viewer, and the exact after-view bytes and MUST return zero or one bounded authorized observation.
- If an authorized before/after view changes, `observe = None` MUST be `PackFault`; a hidden Transition MAY produce None. Operator Membership MUST receive only an explicit bounded projection, never raw Activity State.
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
- Replay MUST reproduce deterministic Attention Signals but MUST NOT evaluate Activation policy or create an intent, offer, lease, Invocation, or participant Action authority.

### FR-8: Recovery and replay

- Canonical transitions MUST be append-only.
- Every room MUST store an immutable canonical genesis record containing pack digest, configuration, ordered initial memberships/roles, seed, and recorded creation time.
- Snapshots MUST be replaceable performance caches, never the only source of truth.
- Startup MUST reconstruct each accessed room from genesis when no valid snapshot exists, or from its latest compatible verified snapshot and transition tail.
- A committed action that was acknowledged before process termination MUST not be lost.
- A commit that occurred before a lost acknowledgement MUST return the stored original result when retried.
- Replay MUST be read-only, use the exact pack revision, and verify recorded Activity State hashes plus the ordered Core Room State changes.
- Replay MUST use the exact retained executor and codecs. Retaining decoders alone or silently dispatching an old digest to newer rules is forbidden.
- A hash mismatch or missing exact executor/codec MUST fault Replay instead of continuing with uncertain state. A restore or storage transfer MUST NOT become ready until every retained digest loads and fully replays.
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

Agent Heist is a deliberately small deterministic game and one ordinary ActivityPackV1 implementation. Its exact schema and golden corpus are normative in [Activity Pack Design](activity-packs.md); no Heist-specific Kernel primitive is permitted.

The frozen configuration contains exactly three immutable Genesis seats (Navigator, Insider, Broker); Briefing 30 seconds; Negotiation 90 seconds; Commitment 30 seconds; a reminder 10 seconds before its deadline; Result 20 seconds; at most twelve plans; and at most four open offers per seat. The Room seed selects canal_shift, service_window, or roof_signal with deterministic label agent-heist/fixture/v1.

Navigator initially owns the route clue, Insider the entry-window clue, and Broker the required-tool and extraction clues. Suspension/departure makes a seat missing but MUST NOT change the denominator, transfer private knowledge, or permit replacement/reassignment.

Canonical Actions MUST cover clue inspection/publication, bounded exchange offer/acceptance, structured plan proposal/endorsement/challenge, one immutable sealed {selected_plan_id, contribute_required_resource} commitment per seat, and Result acknowledgement. No fallback commitment field, arbitrary clue/chat text, binary voting, or Activity-State mirror of current Role ownership may exist.

The only phase path is:

    Briefing → Negotiation → Commitment → Resolution → Result → Complete

Every phase change MUST be ordered and generation-fenced. The third commitment enters Resolution early, cancels both Commitment timer witnesses, and schedules resolution at the minimum semantic tick strictly after admitted_at. Deadline closure preserves missing seats and schedules resolution strictly after its scheduled_for. Resolution computes Result and its deadline; the third acknowledgement completes early by cancelling that deadline, otherwise the deadline enters Complete. Complete is terminal Activity State while Core Room Status remains active until archive.

Plan selection requires at least two of the fixed three seats. Zero, one, two split, or three all-different commitments produce failure, score zero, and reason no_strict_majority. Two matching, 3-0, and 2-1 select the majority plan. No arrival-order, earliest-plan, plan-ID, or other tie-break MAY exist.

A selected plan scores exactly five Boolean checks: route, entry window, required tool, extraction, and at least one supporting resource contribution. Five yields success, three or four partial failure, and zero through two failure.

Public, participant, operator, historical-Replay, Result, and post-Complete final-reveal views MUST remain distinct. Participants additionally see only their authorized clues, addressed offers, own commitment, and exact Action Offers. Operator Membership sees bounded diagnostics/aggregates, never raw state, fixture truth, clues, offers, or commitments. Result reveals aggregates/checks/Outcome while individual commitments remain sealed. Final reveal is separately labeled, available only after Complete, and currently authorized.

The only Attention reasons are offer_received, endorsement_requested, commitment_opened, required_action_deadline, and round_result_available. Per-target precedence MUST be required_action_deadline, commitment_opened, offer_received, endorsement_requested, then round_result_available. Replay reproduces Attention only.

Required Heist acceptance gates:

1. Canonical goldens cover Genesis, all six phases, exact timer effects, events, Attention, Action Offers, state, explanations, and hashes.
2. The full commitment matrix covers zero, one, 2-0, 1-1, 3-0, 2-1, and 1-1-1; scoring covers 5/5, 4/5, 3/5, 0-2/5, and no majority.
3. Deadline equality, concurrent exact-Head commitments, stale retry/new identity, duplicate Actions/timers, lost replies, and early/deadline closure pass.
4. Crash after commit/before publication, crash after early close, fixed-cutoff restart, and Replay from Genesis with every snapshot deleted reproduce the exact Head.
5. Paired privacy fixtures cover live view, Frame, catch-up, reset, UI/log, operator, historical Replay, Result, and final reveal.
6. The absent-Broker Activation path remains separate from participant Action and Cursor authority.
7. Cooperative Navigator, cautious Insider, and withholding Broker use deterministic ranking by known-field matches, endorsements, creation sequence, then plan ID—never as a majority tie-break.
8. Byte-identical corpus results pass through the exact retained executor/codecs on every supported platform and storage profile.
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
- Activity Pack marketplace, dynamic/public pack registry or upload, public agent marketplace, reputation, payments, token, wallet, escrow, or blockchain integration;
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
- [x] ActivityPackV1, exact Action Offer parity, host-owned timers, PackRevisionLock, retained executability, and no in-place upgrade are frozen.
- [x] Agent Heist has exact three-seat/six-phase, majority, scoring, privacy, Attention, and golden-corpus contracts.
- [x] Agent Heist is the only v0.1 activity.
- [x] Investigation Room is the only v0.2 application goal.
- [x] Investigation adds no Investigation-specific Room Kernel concept beyond the preplanned generic artifact subsystem.
- [x] All excluded marketplace, crypto, workflow, cross-room, coding, memory, plugin, and generated-UI ideas are non-normative.
- [x] Every performance statement is labeled target or accompanied by a reproducible report.
