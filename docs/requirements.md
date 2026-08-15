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
- immutable Genesis/Transition lineage, paired Core-and-Activity snapshots, verified current materializations, recovery, current projection, and deterministic Replay;
- durable Room Integrity State, its monotonic generation, and append-only incident/repair audit outside Authoritative Room State;
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
- `CoreRoomState v1` MUST contain exactly Room Status plus the canonically sorted semantic Membership map. Room Head, hashes, Room Integrity State, Sessions, delivery, receipts, Activation, policy, diagnostics, telemetry, and commit time MUST NOT be Core fields.
- Room Status MUST be active or archived. Archive MUST be an irreversible administrative Stimulus and Core Transition in the Room order, as decided in [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md); reaching a Terminal Phase or Outcome MUST NOT archive automatically.
- In the winning Room order, archive MUST atomically cancel scheduled timers and generation-fence pending and leased Activation work. A healthy archived Room MAY serve authorized reads, export, and Replay and accept ordered suspend/depart changes, but MUST reject joins, resumes, participant work, and Access/Role elevation.
- `RoomIntegrityState` MUST be `healthy`, `faulted`, or `quarantined` and MUST remain durable operational state outside Core, Authoritative Room State, `room_seq`, Replay state, and every canonical hash. Activity Phase and Outcome MUST remain separate pack-defined values.
- v0.1 packs MUST be trusted, compiled into the server, and selected from an allowlist.
- The public plugin ABI, untrusted code execution, and pack registry are deferred until after v0.2.

### FR-2: Human and agent participation

- Principal kind MUST be human or agent; Principal kind and pack-defined Role are separate.
- Human and agent participants MUST use the same typed action path.
- Membership MUST survive session disconnects and agent invocation termination.
- The semantic Membership map MUST record immutable Member ID, Principal ID, and room-local Principal kind plus Membership Standing, Access Mode, and current Role. Member/Principal binding and Principal kind MUST never change.
- Membership Standing MUST be `enabled`, `suspended`, or `departed`. Enabled and suspended MAY transition in either direction; departed MUST be terminal. Rejoining MUST create a new Member ID that inherits neither Cursor nor private Observation Stream.
- A Room MUST NOT contain more than one non-departed Membership for one Principal. Suspended or departed Memberships MUST NOT attach, act, receive new frames, or be activated.
- Participant Access Mode MUST carry exactly one pack-valid Role; spectator and operator Access Modes MUST carry no Role. An Access Mode/Role change MUST be atomic.
- Membership Standing MUST be separate from Session connection, Runner availability, and Activation status.
- The versioned pure Core reducer MUST exclusively construct Room Status and the Membership map from a typed Core Stimulus containing attributable authority, an idempotency identity, exact expected Room sequence, reason code, and an unambiguous canonical Core before/after result.
- One Core Stimulus MAY carry a canonically sorted atomic multi-Membership final-state changeset, with at most one before/after pair per Member ID. The host MUST validate the complete final state and pack Role cardinality without persisting, hashing, or exposing an invalid intermediate state.
- Packs MUST receive immutable Core-before/proposed-after views and MUST NOT mutate Core. They MAY return an expected stable veto for join, resume, Access Mode, or Role proposals; they MUST NOT veto archive, suspend, or depart. A vetoable administrative rejection MUST create an idempotent durable no-Transition result and MUST NOT be classified as an Activity Fault; a mandatory-change veto is a Pack Fault.
- A pre-existing desired final state MAY return a durable NoChange disposition. Every other accepted state-affecting Core Stimulus MUST produce one Transition; Session presence and Runner availability MUST NOT.
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

- Authoritative Room State at sequence N MUST be a pure function of Room Genesis, the exact Core and Activity Pack revisions, and recorded Stimuli through N.
- A pack MUST NOT read ambient wall time, operating-system randomness, files, network resources, environment variables, provider APIs, or secrets.
- Host time and randomness that affect state MUST enter as recorded stimuli or recorded stimulus fields.
- Timer creation, cancellation, and logical firing MUST be durable.
- A timer retry MUST NOT cause two logical firings.
- The canonical hash contract MUST use separate domain-separated Core State, Activity State, and aggregate Authoritative State hashes. Core hashing MUST bind the Core schema version and canonical Core bytes; Activity hashing MUST bind the exact pack digest and canonical Activity bytes; the aggregate MUST bind both component hashes and their version identities.
- Genesis, every accepted Transition, every paired snapshot, and the complete Room Head MUST bind all three applicable state hashes. The complete Head MUST also identify the Room sequence, Genesis-or-Transition lineage hash, Core schema version, and exact pack digest.
- The Genesis hash MUST bind Room/version identities, exact pack digest, configuration, initial Core and Activity hashes, normalized initial timers, Room seed, and logical creation time.
- Each Transition hash MUST bind the Room/sequence/version identities, prior Genesis-or-Transition hash, normalized recorded Stimulus, ordered Domain Events, normalized ordered timer changes, deterministic ordered Attention Signals, and all three resulting state hashes.
- Room Integrity State/generation/incidents, operational authorization and commit witnesses, receipts, snapshots and materializations, projections/frames/cursors/resets/Sessions, Activation policy/decisions/intents/leases/delivery, diagnostics, telemetry, and commit wall time MUST NOT enter canonical state or Transition hashes. Canonical attribution and idempotency fields inside the normalized Stimulus remain included.
- These state, lineage, integrity, repair, and Replay boundaries are decided in [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).
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
- Replay MUST reproduce deterministic Attention Signals but MUST NOT reevaluate Activation policy, reconstruct an operational allow/deny decision, create an intent, contact a Runner, or start an Invocation.

### FR-8: Recovery, Replay, integrity, and repair

- Immutable Genesis plus ordered accepted Transitions MUST be the sole canonical lineage. Neither may be edited, skipped, reordered, or silently replaced.
- Every Room MUST store an immutable canonical Genesis record containing the exact reconstructible initial inputs and outputs required by FR-4, including initial Core and Activity values, normalized initial timers, and all three state hashes.
- A snapshot MUST pair Core and Activity State at one sequence and bind Core schema version, exact pack digest, both canonical state values and component hashes, aggregate hash, and the applicable lineage hash. Snapshots MUST be disposable, idempotent postcommit caches and MUST NOT be written inside or determine success of the causing canonical commit.
- Current Room/Core/Membership/Activity rows and actor memory MUST be verified serving materializations, not an independent source of truth.
- Startup MUST reconstruct each accessed Room from Genesis when no valid paired snapshot exists, or from its newest compatible verified pair plus Transition tail. Deleting every snapshot and current materialization MUST still permit reconstruction from Genesis and Transitions.
- A committed action that was acknowledged before process termination MUST not be lost.
- A commit that occurred before a lost acknowledgement MUST return the stored original result when retried.
- Replay MUST be read-only, run the same versioned Core and exact pack reducers, reconstruct Core and Activity State, and verify the Genesis/Transition chain plus all three state hashes.
- Present authentication and authorization MUST first admit a Replay request. At sequence N, reconstructed historical Membership Standing, Access Mode, and Role MUST determine the participant/private view; a Membership absent at N receives no participant/private view at N, and a later Role or replacement Membership MUST NOT inherit earlier private data. Spectator/operator history and a final-reveal view require explicit current projection policy and MUST NOT bypass pack privacy.
- A hash mismatch, missing exact Core/pack revision, or unverifiable canonical byte MUST fail closed instead of continuing with uncertain state.
- Room Integrity State MUST carry a monotonic integrity generation and a separate append-only incident/repair audit. Every canonical commit MUST atomically recheck `healthy` plus the unchanged generation; a failed fence MUST commit no Transition, sequence, or receipt.
- `faulted` means the last canonical Head verifies but the runtime cannot safely advance it. A faulted Room MUST reject every canonical mutation but MAY serve only its last verified authorized Projection, retained Frame Catch-up, and verified Replay with an explicit integrity envelope.
- `quarantined` means canonical integrity cannot be established. A quarantined Room MUST serve no normal Projection, Catch-up, or claimed-current Replay; only authenticated host-operator diagnostics, raw export, restore, and verification remain available.
- Capability revocation, diagnostics, raw export, restore, and verifier repair MUST remain operational while canonical mutation is fenced. An operator MAY request repair, but only a successful generation-fenced verifier MAY restore `healthy`.
- Repair MAY rebuild caches/materializations, reinstall the exact pack, or restore exact canonical bytes from a verified backup. It MUST NOT edit, omit, reorder, synthesize, or silently replace Genesis/Transitions. Restore MUST verify healthy before archive or Membership mutation.
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
- deterministic read-only Replay to the same final Core, Activity, and aggregate Authoritative State hashes.

Required Heist acceptance gates:

1. Three deterministic agents drive the Heist Activity to its Terminal Phase and Outcome solely through public protocol and SDK calls; the Room remains active until explicitly archived.
2. One invocation terminates before the commitment phase.
3. Commitment opening creates a durable targeted activation.
4. A fresh invocation claims it, catches up, and submits a valid commitment.
5. Private clues, offers, and sealed choices never reach unauthorized participants or the public UI.
6. Retrying an accepted action after a lost acknowledgement does not apply it twice.
7. Killing the server during the scripted failure point loses no acknowledged state.
8. Replay produces the same three checkpoint hashes, lineage hash, outcome, and public history.
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
7. Restart and Replay reproduce the final case board, deterministic Attention Signals, brief, and score without reevaluating Activation policy.
8. There are no live web requests, business-system writes, or LLM judging inside the activity.

The generality gate fails if Investigation needs a new core room lifecycle, an Investigation-specific protocol message, special-case persistence tables beyond generic artifact metadata, or server-side model logic.

## Non-functional requirements

### Correctness and durability

- SQLite durability settings and benchmark settings MUST be published.
- Database writes that form one transition MUST be atomic.
- Startup MUST run migrations, verify supported SQLite capabilities, check data-directory permissions, and validate complete Room Head, canonical lineage, paired-snapshot, and current-materialization consistency.
- Storage-full, corrupt snapshot, pack failure, hash disagreement, and slow-client paths MUST fail closed and produce actionable host-operator diagnostics without rewriting canonical history.

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
