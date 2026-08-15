# Frozen Requirements

## Document authority

Status: **FROZEN for Agent Heist v0.1 and Investigation Room v0.2**

Freeze date: 2026-08-15

Reconciliation status: **implementation-ready documentation; repository remains documentation-only**. The [Canonical Decision Index](decision-index.md) locates each invariant's normative source, accepted decision, conformance evidence, and implementation owner without redefining this document.

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
- typed action routing, Operation Identities, Canonical Request Hashes, Semantic Receipts, and stable results;
- typed Semantic Time, the bounded Room Admission Lane, host-owned Timer Generations, and recorded nondeterministic inputs;
- immutable Genesis/Transition lineage, paired Core-and-Activity snapshots, verified current materializations, recovery, current projection, and deterministic Replay;
- durable Room Integrity State, its monotonic generation, and append-only incident/repair audit outside Authoritative Room State;
- Membership-addressed Observation Frames, acknowledgements, and Cursor Catch-up;
- durable activation intents, claim leases, retries, and status;
- bounded network queues, rate limits, and slow-consumer handling;
- a minimal operator-membership/reference UI and protocol/SDK conformance fixtures.

### Activity Packs MUST own

- room configuration and domain state;
- Role definitions, cardinality, permissions, and exact Action Offers;
- deterministic initialization, reduction, views, and observations through `ActivityPackV1`;
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
- A Room MUST NOT change its pinned digest or rewrite canonical Activity State in place. A semantic pack revision creates a new Room.
- Every revision digest MUST be the build-computed digest of a canonical `PackRevisionLockV1` covering the host contract and codec versions, manifest, exact schema-content digests, deterministic static data, pack rule source, and deterministic dependency lock.
- The embedded registry MUST map each digest to its exact executor, descriptor/schema bundle, state/stimulus/output codecs, golden-corpus digest, and separate selectable-for-new-Rooms and runnable-for-retained-Rooms status. Selectable MUST imply runnable.
- Every digest referenced by retained lineage MUST remain runnable for load, advance, view, observe, Recovery, and Replay even after it becomes non-selectable.
- `CoreRoomState v1` MUST contain exactly Room Status plus the canonically sorted semantic Membership map. Room Head, hashes, Room Integrity State, Sessions, delivery, receipts, Activation, policy, diagnostics, telemetry, and commit time MUST NOT be Core fields.
- Room Status MUST be active or archived. Archive MUST be an irreversible administrative Stimulus and Core Transition in the Room order, as decided in [ADR 0002](adr/0002-sequence-domain-relevant-room-changes.md); reaching a Terminal Phase or Outcome MUST NOT archive automatically.
- In the winning Room order, archive MUST atomically cancel scheduled timers and generation-fence pending and leased Activation work. A healthy archived Room MAY serve authorized reads, export, and Replay and accept ordered suspend/depart changes, but MUST reject joins, resumes, participant work, and Access/Role elevation.
- Room Integrity State MUST be `healthy`, `faulted`, or `quarantined` and MUST remain durable operational state outside Core, Authoritative Room State, `room_seq`, Replay state, and every canonical hash. Activity Phase and Outcome MUST remain separate pack-defined values.
- v0.1 packs MUST be trusted, compiled into the server, and selected from an allowlist.
- Dynamic pack download, public plugin upload/registry service, untrusted code execution, and a portable plugin ABI are deferred until after v0.2. The required embedded exact-revision registry is not a plugin marketplace.
- These pack-seam and executable-retention boundaries are decided in [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

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
- One Core Stimulus MAY carry a canonically sorted atomic multi-Membership final-state changeset, with at most one typed component per Member ID. Component kinds MUST be Join, Resume, AccessModeChange, RoleChange, Suspend, or Depart. Archive MUST remain its own CoreProposed kind and MUST NOT occur inside MembershipChangeSet. The host MUST validate the complete final state and pack Role cardinality without persisting, hashing, or exposing an invalid intermediate state.
- Join, Resume, AccessModeChange, and RoleChange components are vetoable; Suspend and Depart components are mandatory. The host MUST reject a MembershipChangeSet mixing those classes before pack entry, with no pack call, Transition, or receipt. A homogeneous changeset inherits its components' class: an all-vetoable set MAY receive one stable pack rejection for the whole atomic proposal, while an all-mandatory set MUST Apply and a Reject is PackFault. Packs MUST receive immutable Core-before/proposed-after views and MUST NOT mutate Core. Archive is independently mandatory. A vetoable administrative rejection MUST create an idempotent durable no-Transition result and MUST NOT be classified as an Activity Fault.
- A pre-existing desired final state MAY return a durable NoChange disposition. Every other accepted state-affecting Core Stimulus MUST produce one Transition; Session presence and Runner availability MUST NOT.
- A human MAY join through a participant, spectator, or operator Membership if the pack and room policy allow it. Only the first is an acting Participant.

### FR-3: Prepared Room Write, identity, and ordering

- Every Action MUST include Room ID, Membership ID, client-generated Action ID, expected Room sequence, Action type, and typed payload. Each active Room MUST have one logical writer and one monotonically increasing committed sequence.
- The host MUST pre-admit an Action type only when it appears in the exact current viewer's canonical Action Offer bytes. Those same bytes MUST appear in the current Projection, Projection Reset, Observation Frame updates, Invocation Context, and host pre-admission; no second legality list MAY exist. The pack MAY still declare a payload-specific domain rejection during reduction.
- The storage port MUST expose exactly `commit(PreparedRoomWriteV1)` and `resolve(OperationIdentity, CanonicalRequestHash)`. `PreparedRoomWriteV1` MUST be exactly `Create(PreparedRoomCreationV1) | Existing(PreparedRoomCommitV1)`, and deterministic preparation of either branch MUST finish before its storage transaction opens.
- `PreparedRoomCommitV1` MUST contain its Operation Identity, Canonical Request Hash, complete observed Head, integrity, authority/capability, policy, and operation-specific input witnesses, and exactly one prepared intent: `Advance` or `DurableDisposition { Rejection | NoChange }`.
- `PreparedRoomCreationV1` MUST seal the administration Operation Identity and Canonical Request Hash; exact creation-authority witness; selected exact PackRevisionLock/digest; generated Room and initial Member IDs; Room seed and logical creation time; Genesis; initial Core and Activity State; normalized initial timers; the three initial state hashes, complete Head zero, and current materializations; and the `genesis_created` Semantic Receipt returning those generated IDs and Head zero. It MUST contain no basis Complete Head or Room Integrity witness.
- Operation Identities MUST be exactly: `(room_id, member_id, action_id)` for an Action; `(authenticated_principal, versioned_operation_kind, idempotency_key)` for Room administration; `(room_id, timer_id, generation)` for a timer firing; and `(room_id, source_id, input_id)` for host/external input. The Timer Canonical Request Hash and exact input witness MUST bind immutable `scheduled_for` and payload, so changed semantic bytes under the same identity are a `Conflict`.
- A versioned Canonical Request Hash MUST bind all caller-semantic input. For an existing-Room operation this includes target Room, expected basis, operation kind, and complete ordered payload/changeset. Room creation instead binds the exact selected pack digest, configuration, and ordered initial Membership proposal; it has no basis Head and MUST exclude generated Room/Member IDs, Room seed, and recorded creation time. A request hash MUST NOT include Action `admitted_at`, a generated Transition ID, commit time, transport-envelope identity, or retry-attempt data.
- The same Operation Identity and Canonical Request Hash MUST resolve the original Semantic Receipt before later Room lifecycle, integrity, or Membership checks, subject to current authentication and permission to read it. The same identity with a different hash MUST resolve `Conflict`; an implementation MUST NOT reinterpret it as a new operation.
- Every new existing-Room Advance or durable disposition MUST take the transaction-scoped Room write fence and recheck identity absence plus the indivisible CompleteHeadV1 tuple `(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash)`. Equal sequence with any different lineage, version, or state-hash field MUST be an integrity fault, not contention.
- The same existing-Room guarded transaction MUST revalidate healthy and unchanged integrity generation; exact authority/capability generation and revocation state; current policy revision when noncanonical policy decisions are included; and the exact timer, Action, administration, or external-input witness. Participant Actions MUST never be silently rebased onto another Head.
- A creation transaction MUST first resolve and fence the administration Operation Identity, then revalidate the sealed creation authority and prove the generated Room ID absent. It MUST atomically install the Room root, Genesis, initial Core/Activity/Membership/timer materializations, all three hashes and complete Head zero, and the `genesis_created` Semantic Receipt before COMMIT. Creation MUST NOT acquire or synthesize an existing-Room lane, basis Head fence, or Room Integrity fence.
- Durable database `COMMIT` MUST be the sole linearization point. Lane reservation, row locks/updates, driver return, actor-memory installation, acknowledgement, and publication MUST NOT be treated as public success or Room order.
- The Room Commit resolution algebra MUST be `Resolved(GenesisCreated { New | Existing })`, `Resolved(TransitionCommitted { New | Existing })`, `Resolved(RejectionRecorded { New | Existing })`, `Resolved(NoChangeRecorded { New | Existing })`, `NotApplicable`, `Reprepare`, `Fenced`, `Conflict`, `RetryableKnownAbsent`, `Indeterminate`, or `Fault`. SQLite and PostgreSQL MUST expose the same classifications. `Outcome` is reserved for an Activity Pack's domain result.
- `Indeterminate` MUST mean COMMIT may or may not have occurred. The server MUST query the authoritative primary with the same Operation Identity and Canonical Request Hash until it finds the stored resolution or proves absence; it MUST NOT blindly retry, invent an identity, re-run the pack, or publish an assumed result.
- Only `RetryableKnownAbsent` MAY cause a bounded retry of the identical sealed plan, including an identical `PreparedRoomCreationV1`. `Fault` is known absent, nonretryable, and reserved for malformed sealed plans or verified invariant/hash failure. `Reprepare` MUST discard the plan: a generated creation Room-ID collision reseals only generated creation values under the same identity/hash and caller-semantic input; Actions/administration resolve a stable stale basis where applicable; a still-scheduled timer reuses its exact identity and recorded fields against the new Head; and a policy-only change recomputes only the affected noncanonical decision. `Fenced`, `Conflict`, and `NotApplicable` MUST NOT create a new Semantic Receipt. A same-identity/hash creation replay MUST return the originally generated Room and Member IDs and exact Head zero from `Resolved(GenesisCreated { Existing })`.
- A durable Rejection MUST be limited to an authenticated, well-formed stable result at an exact fenced Head. An administrative NoChange MUST mean the normalized desired state was already true before pack application. Malformed input, authentication or authority failure, rate/capacity refusal, unhealthy integrity, Activity Fault, storage failure, and obsolete timer candidates MUST NOT become durable dispositions.
- Every accepted Stimulus, including one whose resulting Core/Activity bytes are equal, MUST create one Transition. Only administrative NoChange and a stable rejection consume no `room_seq`.
- One Advance transaction MUST atomically persist the Transition and complete hash chain; new Head and Core/Activity serving materializations; final Membership changeset; timer consumption/schedules/cancellations; addressed Observation Frames and frame heads; activation-policy decision/revision, allowed Activation Intents, and eligibility/archive fences; and the applicable Semantic Receipt, including one for every committed TimerFired or external-input Operation Identity. `NotApplicable` binds no identity, hash, or receipt.
- Archive or any Membership after-state that is no longer an enabled Agent Participant with participant Access Mode and a current Role MUST cancel and generation-fence that target's pending/leased Activation work in the same Advance. This includes suspension, departure, and participant-to-spectator/operator changes.
- Pack execution, deterministic reduction, canonicalization, bound checks, three-hash computation, projection/frame computation, and Attention derivation MUST finish before locks. Network publication, telemetry export, derived indexing, and paired Core+Activity snapshot writes MUST happen after COMMIT and MUST never extend the Room transaction.
- Every Semantic Receipt and equivalent compact tombstone MUST remain resolvable for the retained Room lineage, including after archive. A whole-Room purge MAY remove history and its receipts together; silent receipt expiry or identity reuse is forbidden.

### FR-4: Deterministic state, Semantic Time, and timers

- Authoritative Room State at sequence N MUST be a pure function of Room Genesis, the exact Core and Activity Pack revisions, and recorded Stimuli through N.
- A pack MUST NOT read ambient wall time, operating-system randomness, files, network resources, environment variables, provider APIs, or secrets.
- The trusted pack seam MUST expose exactly five synchronous operations: `descriptor`, `initialize`, `reduce`, `view`, and `observe`. Each MUST finish before persistence handoff and MUST be pure and bounded; the pack MUST receive no storage, scheduler, Activation, Session/delivery state, telemetry, or artifact-byte capability.
- Initialization MUST receive exact creation inputs and return only canonical initial Activity State plus ordered timer requests. Genesis MUST create no Domain Event, Attention Signal, Activation, or Observation Frame.
- Reduction MUST receive prior Activity State, exact Core before/proposed after, the canonically sorted current timer view, next Room sequence, and one normalized typed Stimulus.
- Reduction MUST return exactly Apply with complete next state, ordered Domain Events, timer requests, and Attention Signals, or a declared Reject. `PackFault` MUST remain a distinct contract-failure channel.
- Clean Reject MAY apply only to participant Actions and join, resume, Access Mode, or Role proposals. Archive, suspend, depart, required TimerFired, and accepted External Input MUST NOT be vetoed; attempting to do so is `PackFault`.
- Host-normalized administrative NoChange MUST resolve before pack entry. Every Apply MUST create one Transition even when Activity State bytes do not change.
- Panic where catchable, malformed or undeclared output, bound violation, privacy/view failure, or deterministic disagreement MUST fail closed before commit. Required-input repeat failure faults the Room; canonical hash disagreement quarantines it.
- Host time and randomness that affect state MUST enter as recorded stimuli or recorded stimulus fields.
- The canonical hash contract MUST use separate domain-separated Core State, Activity State, and aggregate Authoritative State hashes. Core hashing MUST bind the Core schema version and canonical Core bytes; Activity hashing MUST bind the exact pack digest and canonical Activity bytes; the aggregate MUST bind both component hashes and their version identities.
- Genesis, every accepted Transition, every paired snapshot, and the complete Room Head MUST bind all three applicable state hashes. The complete Head MUST also identify the Room sequence, Genesis-or-Transition lineage hash, Core schema version, and exact pack digest.
- The Genesis hash MUST bind Room/version identities, exact pack digest, configuration, initial Core and Activity hashes, normalized initial timers, Room seed, and logical creation time.
- Each Transition hash MUST bind the Room/sequence/version identities, prior Genesis-or-Transition hash, normalized recorded Stimulus, ordered Domain Events, normalized ordered timer changes, deterministic ordered Attention Signals, and all three resulting state hashes.
- Room Integrity State/generation/incidents, operational authorization and commit witnesses, receipts, snapshots and materializations, projections/frames/cursors/resets/Sessions, Activation policy/decisions/intents/leases/delivery, diagnostics, telemetry, and commit wall time MUST NOT enter canonical state or Transition hashes. Canonical attribution and idempotency fields inside the normalized Stimulus remain included.
- These state, lineage, integrity, repair, and Replay boundaries are decided in [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).
- There MUST be no universal Transition timestamp. Participant Actions MUST carry host-recorded `admitted_at`; one Timer Generation MUST carry immutable `scheduled_for`, which is its TimerFired semantic time; and Membership, administration, and external inputs MUST carry their versioned stimulus-specific `recorded_at`. Scan, enqueue, retry, lag, dequeue, transaction, receipt, and commit times MUST remain operational and invisible to pack reduction.
- One application-owned, injectable, normalized-UTC HostClock MUST supply semantic host samples and due eligibility. Client clocks and database clock functions MUST NOT define domain semantics. Issued trusted samples MUST not decrease; an untrustworthy rollback or configured large discontinuity MUST fail time-bearing canonical work closed without reopening deadlines or rewriting committed time.
- For a new Action, the server MUST sample `admitted_at` atomically with successful reservation of a bounded Room Admission Lane position, and only after the complete request is strictly parsed/schema-valid, initially authenticated, rate-admitted, and within size limits. A full/unavailable lane MUST return `room_busy` with no `admitted_at`, receipt, or deadline entitlement.
- Action windows MUST be half-open: `open_at <= admitted_at < deadline`. An Action admitted exactly at or after the deadline MUST receive durable `deadline_passed` even if the closing timer is delayed. A timely Action MAY commit after wall time passes the deadline only if every witness still passes and neither its closing timer nor archive has committed first.
- Participant Actions, canonical administration, and newly due timer candidates MUST reserve positions in one bounded per-Room lane. A due timer MUST NOT overtake earlier reservations; once reserved, later participant work MUST NOT overtake it. Capacity reserved for host stimuli MUST prevent participant saturation from starving due timers. Lane positions are provisional and disappear on crash; database COMMIT remains the only canonical order.
- WorldStream MUST own a monotonic generation for each `(room_id, timer_id)`, starting at one and never reused, wrapped, or chosen by a pack. At most one generation may be scheduled; its `scheduled_for`, payload, and creation cause MUST be immutable.
- Packs MAY request only `ScheduleNext(timer_id, due, payload)`, `CancelCurrent(timer_id, expected_generation)`, or `RescheduleCurrent(timer_id, expected_generation, new_due, new_payload)`, normalized to at most one mutation per logical timer ID per Transition. The host MUST validate `due`/`new_due`, deterministically allocate the next generation, and record that generation's immutable host-facing `scheduled_for`; replay/retry of the causing Transition MUST resolve that existing generation.
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
- A participant MUST obtain exact current Action Offers and deadlines from its authorized Projection without reading raw transition history.
- `view` MUST return one authorized Activity Projection plus ordered Action Offers. `observe` MUST receive before/after Core and Activity, normalized Stimulus, ordered events, exact Membership viewer, and the exact after-view bytes and MUST return zero or one bounded authorized observation.
- If an authorized before/after view changes, `observe = None` MUST be `PackFault`; a hidden Transition MAY produce None. Operator Membership MUST receive only an explicit bounded projection, never raw Activity State.
- Private chain-of-thought MUST NOT be requested or stored.

### FR-6: Realtime delivery and reconnect

- WebSocket is the native bidirectional live transport.
- HTTP MAY be used for room administration, current projection reads, runner polling, and artifact transfer.
- Observation delivery MUST be at-least-once.
- Genesis MUST emit no Observation Frame. Each later accepted Transition MUST emit zero or one coalesced frame per authorized Membership.
- Each Membership MUST have an independent, monotonic frame head, retained floor, and Cursor; pruning MUST NOT advance the Cursor or permit frame-sequence reuse.
- An observation acknowledgement MUST only monotonically advance that Membership's Cursor and MUST NOT exceed its current frame head.
- Attach MUST capture a complete Room Head and frame barrier. It MUST return every retained authorized frame after the Cursor through the captured head or a full authorized Projection Reset at that baseline. Reset is required on first attach, a below-floor/pruned or otherwise unavailable range, visibility loss, or any other condition that makes incremental delivery inappropriate.
- Only `room.sync_ack` carrying that Session's captured synchronization token MAY make that Session Live; it MUST NOT advance the durable Cursor. A distinct `observation.ack` MAY advance the shared Membership Cursor but MUST NOT satisfy any Session synchronization barrier.
- It MUST NOT silently skip an unavailable range.
- A stored stale Action result MUST remain tied to its original Action ID and basis Head. Retrying after synchronization MUST use a new Action ID; the server MUST NOT rebase the old request.
- Loading and Room-CatchingUp MUST deny normal attachment and current Projection service. Faulted MAY serve last-verified authorized data with integrity metadata; Quarantined MUST expose only host-operator diagnostics/export/restore/verification surfaces.
- Each connection MUST have bounded input size, output bytes, frame count, and send time.
- A slow consumer MUST be disconnected without blocking the room actor or growing memory without bound.
- Acknowledged frames MUST retain a seven-day safety window while the stream is under its hard ceiling. A ceiling of 10,000 frames or 64 MiB per Membership MAY force an earlier explicit Reset but MUST NOT change canonical history or Cursor.

### FR-7: Explicit agent activation

- An Activity Pack MAY emit an Attention Signal for an Agent Participant's Membership as deterministic Transition output.
- The host MUST bind the Attention Signal into the Transition. It MUST atomically persist the policy revision and allow/deny/intent decision beside that Transition while excluding the decision from canonical hashes.
- The host MUST reject an Attention Signal unless its target Membership is enabled, has participant Access Mode, belongs to an agent Principal, and has a pack-permitted Role.
- An Activation Intent MUST be pending, leased, completed, expired, or cancelled. Claim/control receipts MUST NOT be represented as additional states.
- An activation MUST identify cause sequence, reason code, target Membership, deduplication key, priority, and optional semantic deadline.
- A runner MUST claim an activation with a bounded lease before reporting work on it.
- An Activation claim MUST NOT grant participant Action authority or advance the Membership Cursor. Actions and Observation acknowledgements require separate participant authority, as decided in [ADR 0003](adr/0003-separate-activation-and-action-authority.md).
- Activation delivery MUST be at-least-once and creation MUST be unique by room, cause sequence, target membership, and pack deduplication key.
- A pack MUST emit at most one Attention Signal per target Membership per Transition, and the host MUST allow at most one live Activation lease per Membership in the frozen releases.
- Claim, renew, release, and complete MUST each use an operation ID, canonical request hash, durable result receipt, and exact generation witnesses. An expired prior claimant MUST NOT alter a newer lease.
- The original Activation operation result code and result hash MUST remain immutable. A granted claim MUST explicitly distinguish retained exact context bytes from a versioned context tombstone while preserving its context hash; after retirement, an identical retry deterministically returns wire `result_retired` without changing the original stored disposition or regenerating context.
- If no runner is available, the intent MUST remain pending until expiry or host-operator cancellation; WorldStream MUST NOT pretend that an agent ran.
- A granted claim MUST persist and return the exact complete Head, authorized current Projection and hash, Action Offers, Membership/integrity/policy/authority/delivery witnesses, budget/deadline metadata, explicit Artifact references, and exactly one of retained frames after the Cursor or a Projection Reset.
- Archive and affected Membership/Access/Role changes MUST cancel and generation-fence pending/leased intents. Capability revocation MUST take effect immediately. Recovery and unhealthy states MUST make pending intents unclaimable without deleting them.
- v0.1 MUST support a connected runner control channel or HTTP long poll. Arbitrary outbound webhooks are not required.
- Replay MUST reproduce and verify deterministic Attention Signals and recorded policy-decision evidence but MUST NOT evaluate current policy, reconstruct a new operational allow/deny decision, create or offer an intent, grant a lease, contact a Runner, or start an Invocation.

### FR-8: Recovery, Replay, integrity, and repair

- Immutable Genesis plus ordered accepted Transitions MUST be the sole canonical lineage. Neither may be edited, skipped, reordered, or silently replaced.
- Every Room MUST store an immutable canonical Genesis record containing the exact reconstructible initial inputs and outputs required by FR-4, including initial Core and Activity values, normalized initial timers, and all three state hashes.
- A snapshot MUST pair Core and Activity State at one sequence and bind Core schema version, exact pack digest, both canonical state values and component hashes, aggregate hash, and the applicable lineage hash. Snapshots MUST be disposable, idempotent postcommit caches and MUST NOT be written inside or determine success of the causing canonical commit.
- Current Room/Core/Membership/Activity rows and actor memory MUST be verified serving materializations, not an independent source of truth.
- Startup MUST reconstruct each accessed Room from Genesis when no valid paired snapshot exists, or from its newest compatible verified pair plus Transition tail. Deleting every snapshot and current materialization MUST still permit reconstruction from Genesis and Transitions.
- A committed action that was acknowledged before process termination MUST not be lost.
- A commit that occurred before a lost acknowledgement MUST return the stored original result when retried.
- Replay MUST be read-only, run the same versioned Core and exact pack reducers, reconstruct Core and Activity State, and verify the Genesis/Transition chain plus all three state hashes.
- Replay MUST use the exact retained executor and codecs. Retaining decoders alone or silently dispatching an old digest to newer rules is forbidden. A restore or storage transfer MUST NOT become ready until every retained digest loads and fully replays.
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
7. Restart and Replay reproduce the final case board, deterministic Attention Signals, brief, and score without reevaluating Activation policy.
8. There are no live web requests, business-system writes, or LLM judging inside the activity.

The generality gate fails if Investigation needs a new core room lifecycle, an Investigation-specific protocol message, special-case persistence tables beyond generic artifact metadata, or server-side model logic.

## Non-functional requirements

### Storage profiles and backend-neutral durability

The frozen storage and portability decision is recorded in [ADR 0004](adr/0004-supported-storage-profiles-and-offline-portability.md); release, recovery, and evidence consequences are recorded in [ADR 0011](adr/0011-release-compatibility-recovery-and-supply-chain-gate.md).

- SQLite durability settings and benchmark settings MUST be published.
- Database writes that form one transition MUST be atomic.
- Startup MUST verify the selected engine/schema/manifest, supported capabilities, applicable filesystem/permission constraints, complete Room Head, canonical lineage, paired-snapshot, and current-materialization consistency. Only exclusively locked, backed-up SQLite startup MAY run pending migrations; production PostgreSQL migrations require the offline direct-admin workflow below.
- Storage-full, corrupt snapshot, pack failure, hash disagreement, and slow-client paths MUST fail closed and produce actionable host-operator diagnostics without rewriting canonical history.

- v0.1 and v0.2 MUST support exactly two startup-selected durable profiles: the default release-bundled SQLite profile and `postgres-primary` against one writable hosted or self-managed PostgreSQL 17 primary.
- Exactly one WorldStream process serves a deployment under either profile. A remote PostgreSQL primary imposes no same-host restriction, but a second live WorldStream process, authoritative replica reads, automatic failover, and provider-specific correctness dependencies are unsupported.
- Both profiles MUST expose the same logical transaction boundary, canonical bytes and hashes, operation identities and receipts, timers, frames/cursors, Activation evidence and fencing, failure classes, recovery, and Replay. Selecting a backend MUST NOT change Room semantics.
- SQLite MUST use the exact bundled build and required WAL, `synchronous=FULL`, foreign-key, bounded-busy, query-only-reader, local-filesystem, and controlled-writer policy recorded in the release manifest. Host SQLite, network/UNC filesystems, and shared writers MUST fail closed.
- PostgreSQL MUST accept major 17 only, use `synchronous_commit=on`, Read Committed transactions with a transaction-scoped Room-root lock or durable compare-and-set/fence predicates, a least-privilege runtime role, and TLS for remote connections. Different Rooms MAY commit concurrently; PostgreSQL MUST NOT impose global commit serialization. Direct, session-pooled, and bounded transaction-pooled runtime connections are supported; correctness MUST NOT depend on extensions, session state, named prepared statements, provider APIs, or a connection surviving between transactions.
- Database writes that form one authoritative operation MUST be atomic. Storage-full/unavailable, corrupt snapshot, pack failure, and slow-client paths MUST fail closed with stable actionable diagnostics.

### Migration and compatibility contract

- One ordered logical migration history and schema-contract fingerprint MUST govern both profiles; backend-specific DDL/execution MAY differ but every migration is checksummed, atomic or restart-safe, and covers an empty database and every earlier v0.1 schema.
- Production migrations are forward-only. Down migrations, rolling mixed binary/schema versions, and starting an old binary after migration are unsupported; rollback restores a pre-upgrade backend backup together with the previous binary.
- SQLite MAY migrate automatically only during exclusive locked startup after creating and verifying a recoverable backup. Production PostgreSQL migration MUST use an explicit offline maintenance command, a direct admin connection, and no serving process; the daemon uses only its runtime role and verifies the resulting schema. Development-only auto-migration does not count as production evidence.
- Every release MUST publish, embed, and enforce reviewed [`compatibility.toml`](../compatibility.toml) plus semantically identical canonical [`compatibility.json`](../compatibility.json). The Storage Compatibility Manifest MUST pin product/wire/config/storage/Core/hash versions, engine builds and settings, schema and migration checksums, connection modes, canonical and receipt codec writers plus retained readers, exact retained Activity Pack executors, transfer/recovery formats, platform support, artifact digests, and verification evidence.
- The authored root pair is the v0.1.0 specification, not evidence that a release exists. It MUST remain `manifest_kind = "specification"` and `release_ready = false` while any required checksum, fingerprint, executor digest, artifact digest, or evidence item is unresolved. Release tooling MUST fail closed until it produces a reviewed, semantically identical pair with every required field populated, every gate passed, and `release_ready = true`.
- The v0.1.0 specification pins wire `0.1`, config `1`, storage schema `1`, Core schema version `worldstream.core-room-state.v1`, `blake3-canonical-json-v1`, Rust 1.97.1 edition 2024, Node 24.18.1 LTS as build-only, Python SDK 3.11–3.14, and Python 3.14.7 for the quickstart. It selects SQLite 3.53.4, treats 3.51.3 as the frozen corrective floor, denies 3.52.0, and supports PostgreSQL 17 from 17.11; a release-valid manifest additionally distinguishes release-verified 17.x patches from newer supported-but-unverified 17.x patches, and other majors fail closed.

### Transfer, backup, restore, and semantic verification

- WorldStream MUST provide one versioned, resumable, whole-deployment, offline transfer from SQLite to an empty PostgreSQL target. It MUST NOT provide live switching, dual writes, reverse transfer, or a backend-fallback path.
- Transfer MUST quiesce serving, create and verify a recoverable source backup, mark the source `transfer_pending`, emit a deterministic checksummed manifest, import while neither backend serves, run full target verification, and require explicit finalization to retire SQLite and activate PostgreSQL under the next monotonically increasing Storage Epoch.
- The transfer bundle MUST copy canonical serialized bytes verbatim rather than decode/re-encode them through PostgreSQL types. It MUST preserve lineage/export identity, Genesis, Transitions, every Head and hash, Core/Activity materializations, Memberships and authority, exact timer identity/generation/`scheduled_for` values, Frames/Cursors, Operation Identities/Canonical Request Hashes/dispositions/Semantic Receipts, Activation Intents and every Activation operation receipt, each context-retention discriminator with its matching retained Invocation Context bytes or versioned tombstone, request/result/context hashes, lease and witness generations, claims/audit/fences, principals/capabilities/revocations, integrity incidents, and artifact metadata and bytes.
- Snapshots, indexes, caches, telemetry, Sessions, Runner presence, mailboxes/delivery attempts, and temporary state MAY be rebuilt or invalidated. Scheduled timers retain their recorded `scheduled_for` values and enter normal CatchingUp after transfer; no fire or generation is invented. Nonterminal Activation leases MUST be fenced before target readiness while eligible intents remain reclaimable.
- Before source retirement, abort MUST discard the target and leave the verified SQLite source authoritative. After PostgreSQL accepts its first write in the new epoch, rollback to SQLite is unsupported; retired SQLite remains a read-only recovery artifact unless an explicit destructive override abandons continuity.
- Backup mechanisms MUST be backend-native: WorldStream owns SQLite online backup/restore orchestration; PostgreSQL uses operator/provider-native snapshot, PITR, dump, and restore facilities over a direct admin path. Every restore and transfer target MUST then pass the read-only, full WorldStream semantic verifier.
- The verifier MUST check backup ID, lineage, schema, manifest, artifact digests/bytes, Operation Identities/Canonical Request Hashes/Semantic Receipts, timers, Frames/Cursors, Activation operation receipts, every context-retention discriminator and matching retained Invocation Context bytes or versioned tombstone, request/result/context hashes, lease and witness generations/fencing, exact retained pack executors, and Genesis-to-Head Replay with every hash for every healthy Room. Global mismatch blocks readiness. A byte-preserved Room already marked faulted or quarantined MAY remain isolated and unhealthy without blocking verified healthy Rooms; a newly introduced mismatch aborts verification.

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
- Structured JSON logs, Prometheus metrics, W3C trace correlation, and an optional OpenTelemetry/OTLP export seam MUST remain vendor-neutral and outside admission, reduction, commit, replay, Room Integrity State, and readiness. Post-commit telemetry is bounded and nonblocking, holds no Room/database lock, emits a drop metric and rate-limited warning on overflow, and receives at most a bounded three-second shutdown flush.
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
- Activity Pack marketplace, dynamic/public pack registry or upload, public agent marketplace, reputation, payments, token, wallet, escrow, or blockchain integration;
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
- [x] ActivityPackV1, exact Action Offer parity, host-owned timers, PackRevisionLock, retained executability, and no in-place upgrade are frozen.
- [x] Agent Heist has exact three-seat/six-phase, majority, scoring, privacy, Attention, and golden-corpus contracts.
- [x] Agent Heist is the only v0.1 activity.
- [x] Investigation Room is the only v0.2 application goal.
- [x] Investigation adds no Investigation-specific Room Kernel concept beyond the preplanned generic artifact subsystem.
- [x] Bundled SQLite and `postgres-primary` are the only storage profiles, with one WorldStream process and backend-neutral semantics.
- [x] Forward-only migrations, retained codecs, offline one-way transfer, Storage Epoch fencing, backend-native recovery, and full semantic verification are testable invariants.
- [x] Native Linux/Windows, Linux/amd64 OCI, macOS source-only, config/secrets/probes/telemetry, supply-chain evidence, and all negative release clauses are explicit.
- [x] All excluded marketplace, crypto, workflow, cross-room, coding, memory, plugin, and generated-UI ideas are non-normative.
- [x] Every performance statement is labeled target or accompanied by a reproducible report.
