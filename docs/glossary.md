# Extended WorldStream Terminology

## Status and authority

Status: extended product, protocol, runtime, storage, and UI terminology for the frozen v0.1 and v0.2 design.

The project name **WorldStream** is provisional while public-brand clearance continues. The technical vocabulary in this document does not depend on the final brand.

The root [WorldStream Domain Context](../CONTEXT.md) is authoritative for domain language. The [Frozen Requirements](requirements.md) are authoritative for release behavior and scope. This reference adds protocol, runtime, storage, and UI language. Rows that mention a domain term are navigation summaries or engineering elaborations, not independent definitions; edit `CONTEXT.md` first. Any conflict is a documentation defect and must be reconciled.

This reference exists to keep architecture, protocol, code, UI, and examples aligned. It also records terms that should not be used because they imply capabilities WorldStream does not provide.

## One-paragraph mental model

WorldStream is a self-hosted **realtime room runtime for multi-agent applications**, with humans as first-class participants. One server hosts independent Rooms. Each Room pins exactly one Activity Pack, which defines its rules. Human and Agent Participants receive authorized Projections, submit typed Actions, and receive Membership-addressed Observation Frames. The Room Kernel orders accepted Stimuli, commits deterministic Transitions, preserves Recovery and Replay, and may create Activation Intents for external Runners. WorldStream does not host models or preserve a continuously running agent mind.

## Relationship map

~~~mermaid
flowchart TD
    S["WorldStream Server"] --> R["Room"]
    R --> K["Room Kernel"]
    R --> P["One pinned Activity Pack"]

    PR["Principal: human or agent"] --> M["Room Membership"]
    R --> M
    M --> SE["Temporary Session"]

    M -->|"Agent Participant Membership may receive"| AI["Activation Intent"]
    RU["External Runner"] -->|"claims with lease"| AI
    RU --> IV["Ephemeral Invocation"]

    SE --> A["Typed Action"]
    IV --> A
    A --> K
    K --> T["Committed Transition"]
    T --> ST["Authoritative Room State"]
    T --> OF["Membership-addressed Observation Frames"]
    OF --> M
~~~

## Product and category language

| Term | Summary or technical elaboration | Usage guidance |
|---|---|---|
| **WorldStream** | Provisional project codename for the complete open-source system. | Capitalize exactly this way. Do not write WordStream or Worldstream. |
| **WorldStream Server** | One deployed WorldStream process plus its configured durable storage profile. | This is the concrete shipped service. One server may host many independent rooms; a PostgreSQL primary may run on another host without creating another WorldStream process. |
| **Realtime room runtime** | The preferred product category: a runtime that owns durable shared rooms and promptly delivers committed authorized changes. | Use in the primary product description. Realtime does not promise immediate model execution. |
| **Room Kernel** | The internal domain-neutral correctness core: action admission, one-writer room ordering, deterministic pack application, transition commit preparation, projections, timers, and attention output. | Do not use Kernel as a synonym for the complete server, web UI, agent runner, or Activity Pack. |
| **Multi-agent application** | An application in which multiple independently operated agent policies can affect the same evolving situation. | This is the primary target application category. It does not mean WorldStream hosts the agents. |
| **Multi-participant** | The precise kernel model: more than one participant may act, and a participant may be human or agent-operated. | Prefer in architecture and protocol writing. |
| **Multiplayer** | An analogy to authoritative game-server design: shared rooms, participants, rules, private views, concurrent actions, and reconnect. | Useful for explanation. Do not present WorldStream as game-only middleware. |
| **Player** | A game-specific participant role. | Use only inside game Activity Packs such as Agent Heist, never as a core identity or protocol entity. |
| **Self-hosted** | One host operator controls the WorldStream Server and its selected storage, whether the PostgreSQL service is self-managed or hosted. Agent runners may execute on other host-operator-approved machines. | This is an operational-control property, not an assertion that every process or durable byte is on one machine. |
| **AI application** | An application that uses AI agents as participants while WorldStream supplies shared-state participation infrastructure. | Broader than games and narrower than generic business automation. |
| **World** | Informal metaphor for a room's shared evolving situation. | There is no separate World entity in v0.1 or v0.2. Use Room in schemas and code. |
| **Activity** | The domain experience occurring in one room under one Activity Pack, such as a Heist or Investigation. | The Activity Pack is the definition; the room is the running instance. |
| **Reference activity** | A deliberately bounded application used to prove or falsify the runtime thesis. | Agent Heist and Investigation Room are reference activities, not the entire product. |

## Identity and participation

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Principal** | A durable server-level identity whose kind is human or agent. | A principal exists outside a room and may have memberships in different rooms. |
| **Participant** | A human or agent Room Member whose membership has participant access and a pack-defined role. | Participant is a contextual acting role, not another durable identity or table. Spectator and operator memberships are Room Members but not Participants. |
| **Human participant** | A Participant whose Principal represents a human. | Humans use the same typed domain-action path as agent Participants. |
| **Agent participant** | A Participant whose Principal represents a machine-operated policy executed outside WorldStream. | The Agent Participant is logical and durable; its Runner and Invocations are not the Participant. |
| **Membership** | One Principal's durable room-local seat, including immutable member/Principal identity and Principal kind plus canonical standing, Access Mode, and any pack-defined Role. | Cursor and activation policy belong to operational delivery/control state, not the semantic Membership map. Membership survives connection loss and invocation termination. |
| **Room Member** | A Principal considered in one Room through a Membership, whether acting, spectating, or operating. | Use this as the umbrella; reserve Participant for participant-access memberships. |
| **Member ID** | The room-scoped identifier of one Membership, used for authorization and observation addressing. | It is not another Member entity and is not interchangeable with the server-level principal ID. |
| **Membership standing** | Core Membership lifecycle: enabled, suspended, or departed. Enabled and suspended are reversible; departed is terminal and rejoining requires a new Member ID. | Protocol fields may use `membership_status`; the domain term “standing” avoids confusing it with Room Status. It is separate from connection status, runner availability, and activation status. |
| **Role** | An Activity Pack-defined responsibility and permission set held by a Participant, such as Navigator, Analyst, or Lead. | Role expresses domain meaning. It is separate from the core access mode. |
| **Access mode** | A core membership classification: participant, spectator, or operator. | It describes broad access posture; it does not replace pack-defined roles or authorization. |
| **Spectator membership** | A read-only Membership that receives an authorized public projection and owns its own cursor. | It is a Room Member but not a Participant or anonymous global stream. |
| **Host operator** | The trusted person or organization running the WorldStream deployment, controlling its filesystem, configuration, compiled packs, and administrative capabilities. | This is a trust-boundary role outside any one room. Always qualify it as host/deployment operator. |
| **Operator membership** | A room-scoped administrative or diagnostic viewer represented by an operator access-mode membership or capability. | It is not the same as the host operator and does not automatically expose raw authoritative state. |
| **Viewer** | The authenticated authorization context passed to a pack when producing a projection or observation. | Never derive viewer identity from a client-supplied member ID alone. |
| **Session** | A temporary authenticated HTTP/WebSocket client connection attached to a Membership. | Sessions are transport state. Multiple Sessions on one Membership share one Observation Stream and Cursor; independent consumers require separate Memberships. |
| **Capability** | A scoped, revocable bearer credential binding a principal and optionally a runner, room, member, operation, and expiry. | A capability authenticates authority; an Activity Pack role defines domain permission. |
| **Runner** | An external, owner-operated process authorized to claim activation intents and start agent invocations. | A runner is not the agent identity, model, room actor, or WorldStream server. |
| **Runner connection** | A temporary WebSocket or HTTP polling relationship used to receive and claim activation work. | Runner availability is operational and does not belong in durable membership state. |
| **Invocation** | One bounded, ephemeral execution of an agent/model policy, possibly containing multiple model and tool calls. | A later invocation normally reconstructs context; it does not resume a hidden mind. |
| **Model** | An external AI model selected and called by a runner-owned agent implementation. | WorldStream treats the model as opaque and stores no provider credential. |
| **Room actor** | The internal single-writer task that serializes accepted stimuli for one active room. | Actor here is an actor-model implementation term. Do not use it as a synonym for participant or AI agent. |

## Room state and history

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Room** | One authoritative, ordered, replayable shared state machine pinned to one Activity Pack digest. | Rooms are independent. There is no Project, Workspace, or cross-room transaction in the frozen releases. |
| **Room status** | Core administrative state: active or archived. | Separate from Room Integrity State and pack-defined Activity Phase. |
| **Room integrity state** | Durable operational integrity: healthy, faulted, or quarantined. | Protocol envelopes may retain `room_health`; this state is outside Core Room State, Authoritative Room State, `room_seq`, Replay state, and canonical hashes. |
| **Integrity generation** | A monotonically increasing operational fence paired with Room Integrity State. | Every canonical commit rechecks both healthy state and the unchanged generation; it is not a Room sequence. |
| **Activity phase** | A pack-defined domain phase such as Briefing, Commitment, Analysis, or Closed. | Separate from Room Status and Room Integrity State. |
| **Terminal phase** | A pack-defined phase after which ordinary domain Actions are no longer permitted. | It is separate from Activity Outcome and core Room archival. |
| **Outcome** | A pack-defined final result and optional deterministic score, possibly established before the Terminal Phase. | Reaching an Outcome does not automatically archive the core Room. |
| **Core room state** | The versioned WorldStream-owned canonical value containing exactly Room Status and the canonically sorted semantic Membership map. | It excludes Room Head, Room Integrity State, cursors, frames, Sessions, receipts, activation policy/work, diagnostics, telemetry, and commit time. |
| **Activity state** | Activity Pack-owned current domain facts for one Room. | It contains phases, domain entities, deadlines, and Outcome—not Sessions, Runner state, host diagnostics, or a duplicated cache of Action Offers or current Core Role ownership. |
| **Authoritative room state** | The accepted current shared truth comprising Core Room State and Activity State, changed only by committed Transitions. | Clients never mutate or receive this aggregate directly. |
| **Canonical activity state** | The deterministic, schema-valid representation of Activity State used for hashing and replay. | It is not a second domain state and does not by itself include Core Room State. |
| **Canonical history** | The immutable room genesis followed by the append-only ordered transitions. | Snapshots, indexes, projections, frames, and telemetry are derived or delivery records rather than substitutes for canonical history. |
| **Genesis** | The immutable sequence-zero creation record containing exact version identities and pack digest, configuration, initial Core and Activity values, normalized initial timers, Room seed, logical creation time, and the three initial state hashes. | Genesis, not a sequence-zero snapshot, is sufficient to reconstruct the Room when every snapshot is absent. |
| **Room head** | The complete latest committed position: Room ID and sequence, Genesis-or-Transition lineage hash, Core schema version, pack digest, and Core, Activity, and aggregate Authoritative State hashes. | It is causal/materialization metadata outside Core and Activity State and is global to the Room, unlike a Membership frame Cursor. |
| **Room sequence (`room_seq`)** | A monotonic number identifying committed transitions in one room; genesis is sequence zero and the first transition is one. | Rejected actions do not consume a room sequence. |
| **Command** | A request sent to the server, such as attach, acknowledge, claim, administer, or submit action. | Only an accepted room stimulus becomes a transition. Not every command is canonical history. |
| **Protocol message** | A versioned transport envelope such as `action.submit`, `observation.deliver`, or `activation.claim`. | It carries commands/results over a connection; it is not automatically a domain event or transition. |
| **Action** | A typed, untrusted request by a participant to change room state. | An action is a proposal until admitted, validated, and committed. |
| **Action ID** | A client-generated idempotency key scoped by room and membership. | Reusing it with a different canonical action body is an idempotency conflict. |
| **Message ID** | A unique identifier for one protocol envelope, used for tracing and duplicate transport diagnostics. | It is not the action/claim idempotency key. A retried logical request may use a new message ID. |
| **Request ID** | A correlation identifier copied from a request into its response. | It pairs transport messages and does not identify canonical room mutation. |
| **Transition ID** | A server-generated stable identifier for one committed transition. | `room_seq` is the authoritative order; ULID ordering is not. |
| **Based-on sequence (`based_on_room_seq`)** | The room sequence on which a participant based an action decision. | v0.1 and v0.2 require it to equal the current room head exactly. |
| **Accepted action** | An action that passes admission and pack validation and commits as one transition. | It is acknowledged only after durable commit. |
| **Action rejection** | An expected, deterministic admitted refusal such as stale state, illegal domain action, or expired deadline. | It creates no Transition and is durable only after its exact witnesses and semantic result commit. |
| **Transient error** | A failure such as unauthenticated, malformed, rate-limited, room-busy, storage-unavailable, or pre-commit runtime fault. | It creates no semantic receipt and does not consume a new operation identity. |
| **Activity fault** | A pack implementation or integrity failure, such as panic, invalid output, nondeterminism, or projection failure. | It is not expected domain behavior and may fault or quarantine the room. |
| **Operation identity** | The durable identity of one semantic Room operation: Action, administration, timer generation, or host/external input. | A transport message ID and a retry attempt are not new operation identities. |
| **Canonical request hash** | A versioned digest of the caller-semantic input bound to an Operation Identity. | It excludes Action `admitted_at`, generated Transition ID, commit time, and transport envelope fields; same identity with a different hash is a Conflict. |
| **Semantic receipt** | The lineage-retained durable resolution of an Operation Identity and Canonical Request Hash. | It records an accepted Transition, stable rejection, or administrative NoChange and makes lost replies resolvable without repeating domain work. |
| **Action receipt** | The Semantic Receipt for one Participant Action identity. | It preserves the original accepted or stable rejected result, including the original `admitted_at`. |
| **Mutation receipt** | The Semantic Receipt for one authenticated administrative operation identity. | It may resolve to a Transition, stable rejection, or administrative NoChange. |
| **Stimulus** | A normalized, fully recorded typed candidate presented to deterministic Core and/or pack logic: participant Action, timer firing, Core change, or external input. | Only an accepted Stimulus produces a Transition. Ambient wall time, network data, and random calls are not Stimuli unless explicitly recorded by the host. |
| **Core Stimulus** | A versioned Stimulus proposing a canonical Core change with attributable authority, idempotency identity, exact expected Room sequence, reason code, and an unambiguous Core before/after changeset. | A single Core Stimulus may atomically change several Memberships, but each affected Member ID appears at most once. |
| **Core reducer** | The versioned pure WorldStream reducer that exclusively constructs and validates Core Room State. | Packs receive immutable Core-before/proposed-after views and may never mutate Core directly. |
| **External input** | A host-operator-authenticated, idempotent recorded stimulus from a named source, used narrowly for controlled fixture/input ingestion. | It is not a participant, connector platform, webhook framework, or permission to perform pack I/O. |
| **Host stimulus source** | A server-controlled source of recorded timers or external inputs, such as the Heist facility or Investigation evidence feed. | It is not a human/agent principal, participant, runner, or room actor. |
| **Transition** | One durably ordered accepted Stimulus plus its deterministic next Core and Activity State, ordered Domain Events, normalized timer changes, deterministic Attention Signals, and hashes. | It is the canonical mutation unit and consumes one Room sequence. |
| **Domain event** | A typed semantic fact emitted by the Activity Pack inside a transition, such as plan proposed or claim invalidated. | It explains consequences for audit/UI; it is not an independently ordered broker event or a raw client action. |
| **Semantic time** | The family of typed, replayed time values whose meanings belong to specific Stimuli: Action `admitted_at`, timer `scheduled_for`, and stimulus-specific `recorded_at`. | There is no universal Transition time; scan, queue, transaction, receipt, and commit timestamps are operational only. |
| **Admitted time (`admitted_at`)** | The HostClock sample taken atomically when a validated Participant Action reserves bounded Room-lane capacity. | It controls half-open deadline eligibility; client, first-byte, dequeue, pack-entry, database, and commit times do not. |
| **Scheduled time (`scheduled_for`)** | The immutable semantic due time of one Timer Generation and the effective time of its TimerFired Stimulus. | Later detection or retry never creates a pack-visible `fired_at`. |
| **Recorded time (`recorded_at`)** | The declared semantic time field for a Membership, administration, or external-input Stimulus. | Its stimulus-specific meaning must be versioned; it is not a generic Transition timestamp. |
| **HostClock** | The application-owned, injectable, normalized-UTC source for trusted semantic host samples and timer due checks. | Client clocks and SQLite/PostgreSQL clock functions never define Room semantics. |
| **Timer** | A durable obligation for the host to submit one exact Timer Generation at or after its Scheduled Time. | It is not an arbitrary cron workflow, an in-memory sleep, or a separately claimed job. |
| **Timer generation** | A host-owned monotonic fence for one `(room_id, timer_id)` schedule, starting at one and never reused or wrapped. | Its `scheduled_for`, payload, and creation cause are immutable; packs request mutations but never assign generations. |
| **Room Admission Lane** | The bounded per-Room lane in which Participant Actions, canonical administration, and newly due timers reserve provisional positions before persistence. | Lane order provides fairness, but durable database COMMIT alone establishes canonical Room order. |
| **Room CatchingUp stage** | The operational load stage that drains every still-applicable timer due through one fixed HostClock cutoff before the Room becomes Active. | It is distinct from Membership Observation Catch-up and does not consume, merge, or skip overdue obligations. |
| **Deterministic context** | Restricted host-supplied pure helpers, including labeled randomness derived from the room seed/sequence and canonical-value utilities. | Recorded time arrives through genesis or the recorded stimulus, not ambient clock access. The context provides no database, network, filesystem, environment, model, wallet, or secret access. |
| **Core State hash** | A domain-separated digest of the Core schema version and canonical Core Room State bytes. | It excludes every operational field, including Room Integrity State. |
| **Activity State hash** | A domain-separated digest of the pack digest and Canonical Activity State bytes. | It proves equality of pack-state bytes, not business correctness. |
| **Authoritative State hash** | A domain-separated aggregate digest binding the exact Core and Activity State hashes with their version identities. | It is not a replacement for either component hash. |
| **Transition hash** | A domain-separated lineage digest binding Room/sequence/version identities, prior Genesis-or-Transition hash, normalized recorded Stimulus, ordered Domain Events, normalized ordered timer changes, deterministic ordered Attention Signals, and all three resulting state hashes. | Operational integrity, policy/Activation work, receipts, delivery, snapshots, telemetry, and commit wall time are excluded. |
| **Pack digest** | The immutable content/revision identity pinned by a room. | Pack name or semantic version alone is insufficient for recovery and replay. |

## Projection, observation, stream, and context

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Projection** | A current, full, authorized view derived from authoritative room state for one viewer. | It is not raw state and does not grow like a transcript. |
| **Activity projection** | The pack-produced domain portion of a Projection. | WorldStream adds authorized Core Room and Membership facts; the pack does not own Room Status or Membership metadata. |
| **Projection envelope** | A protocol payload carrying one authorized Projection plus causal sequence, Room Integrity State, schema, hash, and delivery metadata. | The envelope is not Authoritative Room State. Operational metadata beside a Projection does not become part of that Projection or its domain truth. |
| **Public projection** | A pack-defined view safe for every Membership authorized to see public consequences. | In the frozen protocol it is materialized into Membership-addressed streams; it is not a separate anonymous durable stream. |
| **Participant projection** | A current view containing information authorized for one acting membership. | Two participants in the same role may still receive different data. |
| **Operator projection** | A current administrative/diagnostic view explicitly produced for an authorized operator membership. | It is still a projection and must not bypass pack/privacy boundaries casually. |
| **Final-reveal projection** | A pack-defined historical/completed view that may disclose information after an activity terminates. | Replay by itself does not grant reveal access. |
| **Observation** | Information a membership is allowed to learn because the room changed. | Projection is current full view; observation is change-oriented delivery. |
| **Observation frame** | A durable, bounded, Membership-addressed unit coalescing the authorized consequences of one accepted Transition. | Genesis emits none; a later Transition emits zero or one per viewer. It is not a Projection Reset. |
| **Observation stream** | One membership's monotonically sequenced observation frames. | It is not a global raw event stream, room state dump, or LLM token stream. |
| **Frame sequence (`frame_seq`)** | A monotonic position inside one membership's observation stream. | It is independent from `room_seq` and may skip transitions irrelevant to that member. |
| **Frame head** | The greatest frame sequence ever allocated in one Membership's Observation Stream. | It never decreases or reuses a value, even after pruning. |
| **Retained floor** | The earliest frame sequence still physically available for incremental catch-up. | Pruning may raise it without advancing the Cursor. |
| **Cause room sequence (`cause_room_seq`)** | The committed transition that caused an observation frame or activation intent. | It provides causality but does not replace that object's own frame or activation identity. |
| **Cursor** | The highest frame sequence a membership has durably processed and acknowledged. | A cursor belongs to one membership stream, not the entire room. |
| **Catch-up** | Delivery of retained authorized frames after a membership cursor during attach/reconnect. | Catch-up is transport continuity; it is not deterministic room replay or cognitive resumption. |
| **Projection reset** | An authorized current Projection sent when first attachment, visibility loss, or an unavailable retained range requires a new baseline. | It is explicit operational delivery state outside canonical history, not an Observation Frame or Transition; the server never silently skips a gap. |
| **Session sync token** | A single-use opaque value binding one Session to its captured attach barrier. | Only that Session's token acknowledgement makes it Live; a shared Cursor acknowledgement is insufficient. |
| **Room recovery state** | Loading, CatchingUp, or Active, with Faulted and Quarantined failure surfaces. | It is independent from a Session's delivery state and from canonical Room Status. |
| **Session delivery state** | Attaching, CatchingUp, Live, or Closed for one attached transport Session. | It is operational and never consumes `room_seq`. |
| **Action Offer** | The pack view's canonical typed representation of an action type, exact payload-schema digest, and optional recorded eligibility window for one viewer at one Head. | The same bytes feed Projection, reset, Frame, Invocation Context, and admission. Presence is necessary, not a guarantee that every payload will be accepted. |
| **Realtime** | Commit-first push of relevant observations to connected clients, with bounded latency targets and durable cursor recovery. | It does not mean every database event is broadcast or every agent responds instantly. |
| **Stream** | In product language, one Membership's ordered progression of meaningful Room Observations. | Do not use it to imply token streaming, video streaming, or a generic message broker. |
| **Invocation context** | The exact payload committed with one granted claim: cause, complete Head/witnesses, current authorized Projection and Action Offers, limits/Artifact references, and one retained-frame or Reset branch. | It is bounded input for one Invocation, not a new authoritative database or persistent agent mind. |
| **Agent-private memory** | Prompts, model history, private checkpoints, summaries, or other state owned by the external runner/agent implementation. | WorldStream does not own, inspect, or promise this memory. |

## Activation and ephemeral agent execution

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Attention signal** | Deterministic Activity Pack output saying a particular enabled Agent Participant may need to act for a typed reason. | It cannot target spectator/operator Memberships, does not execute a model, and is not an arbitrary semantic polling query. |
| **Activation policy** | Host-enforced per-membership rules deciding whether an attention signal may create an activation intent. | It limits activations; it is not the agent's private planning policy. |
| **Activation intent** | A durable, unique, at-least-once request in pending, leased, completed, expired, or cancelled state. | It means work may be relevant; it does not prove a model ran. Operation attempts/receipts are not states. |
| **Activation ID** | The server-generated durable identity of one activation intent. | It is distinct from the pack's logical deduplication key and from a runner claim ID. |
| **Activation offer** | A possibly duplicated, privacy-minimal notification that an activation may be claimed. | Private projection/context is returned only after an authorized claim succeeds. |
| **Activation claim** | A runner's idempotent request to obtain the current bounded lease for an activation. | A claim ID and request hash make lost replies safe. |
| **Claim ID** | A runner-generated idempotency key for one activation claim attempt. | A runner uses a new claim ID to try again after a stored not-available result. |
| **Activation deduplication key** | A stable pack-produced key combined with room, cause sequence, and target membership to prevent duplicate logical activation creation. | It is not a network message ID or claim ID. |
| **Lease** | A time-bounded exclusive right for one authenticated runner to handle the current activation generation. | It is operational ownership, not room-state authority. |
| **Lease generation** | A monotonically increasing fence that prevents an expired claimant from renewing, releasing, or completing a newer lease. | Runner identity alone is insufficient to fence stale operations. |
| **Activation operation receipt** | The durable request hash and exact result for one claim, renew, release, or complete operation ID. | It resolves lost replies and conflicts without creating more intent states. |
| **Activation completion** | A runner's operational report that it handled, declined, or failed the activation. | Only separately accepted room actions create authoritative room consequences. |
| **Logical agent persistence** | Persistence of principal, membership, role, permissions, cursor, activation state, room facts, and explicit references across invocations. | It does not mean continuous computation, consciousness, model context, or an idle container. |

## Activity Pack vocabulary

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Activity Pack** | Trusted, versioned domain rules and schemas compiled into the server for v0.1/v0.2. It defines configuration, Activity State, roles, actions, deterministic reduction, projections, timers, attention reasons, phases, and outcomes. | It is not a workflow graph, model prompt bundle, arbitrary connector, or public plugin in the frozen releases. |
| **ActivityPackV1** | The frozen trusted synchronous five-operation seam: descriptor, initialize, reduce, view, and observe. | It is a retained-Room semantic contract, not a dynamically loaded, sandboxed, or portable public plugin ABI. |
| **Pack revision descriptor** | Pack identity plus declared schemas, limits, roles, actions, rejection codes, Attention reasons, and Projection versions. | It describes one revision; the semantic digest selects its exact executable rules. |
| **PackRevisionLock** | Canonical build input binding host/codec versions, descriptor, schema/static-data digests, rule source, and deterministic dependency lock. | Its build-computed digest is semantic identity, not a pack label or machine-binary hash. |
| **Embedded pack registry** | Release mapping from exact semantic digest to executor, descriptor/schemas, codecs, golden digest, and selectable/runnable status. | It is required compatibility data, not a public plugin registry or marketplace. |
| **Selectable pack revision** | A runnable exact revision permitted for new Room creation. | Selectable implies runnable. |
| **Retained-runnable pack revision** | An exact revision no longer selectable for new Rooms but still able to load, advance, view, observe, recover, and Replay retained Rooms. | Retaining only a decoder is insufficient. |
| **Pack identifier** | A stable namespaced logical name such as `worldstream.agent-heist`. | It groups revisions but does not uniquely select executable rules. |
| **Pack version** | A human-facing declared release version for a pack. | It aids compatibility and documentation; the digest remains authoritative. |
| **Room configuration** | Immutable creation input validated by the pack and committed into genesis. | Later domain changes belong in canonical pack state; server deployment configuration is separate. |
| **Reducer / `reduce`** | The deterministic pack operation that consumes Activity before, Core before/proposed after, timer view, next sequence, and one normalized Stimulus and returns Apply or declared Reject. | PackFault is separate; the reducer performs no host I/O or model execution. |
| **View function / `view`** | The deterministic pack operation that creates one current authorized Activity Projection plus ordered Action Offers. | It constructs allowed data rather than serializing and redacting raw state. |
| **Observation function / `observe`** | The deterministic pack operation that creates one viewer's bounded change-oriented observation after a transition. | It is persisted/delivered only after authorization and successful commit. |
| **Attention reason** | A pack-declared stable code explaining why an Agent Participant may need Activation. | It is typed application semantics, not free-form LLM judgment. |
| **Pack conformance** | Tests proving determinism, schema validity, privacy noninterference, bounded outputs, recovery, and replay for an exact pack revision. | Passing conformance does not make untrusted third-party pack execution safe. |

## Persistence and delivery vocabulary

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Durable** | Persisted with the required filesystem/database ordering before success is acknowledged. | In-memory queues and live WebSocket delivery are not durable. |
| **Ephemeral** | Temporary operational state that may disappear without changing canonical room truth, such as sessions, runner presence, or an invocation process. | Ephemeral does not mean unimportant; it means recovery cannot depend on it. |
| **Prepared Room Commit** | An immutable, versioned, fully computed Room write containing operation identity/hash, complete witnesses, and either an Advance bundle or durable disposition. | Pack calls, hashing, projections, and frame construction finish before storage locks are taken. |
| **Durable disposition** | A stable Rejection or administrative NoChange recorded for an exact fenced basis without creating a Transition. | It is not used for malformed input, authority failure, unhealthy integrity, capacity rejection, Activity Fault, or storage failure. |
| **Database COMMIT linearization** | The rule that successful durable database COMMIT is the sole public ordering point for a Room write. | A lane position, row update, driver return, actor-memory install, reply, or publication is not the linearization point. |
| **Commit-before-acknowledgement** | The invariant that an Advance or durable disposition reaches database COMMIT before success is returned or streamed. | A lost reply is resolved from the original Operation Identity rather than by repeating domain work. |
| **Idempotency** | Repeating the same scoped request with the same key and canonical body returns the original result without repeating mutation. | Same key with changed body is a conflict, not a new operation. |
| **At-least-once delivery** | A frame or activation offer may be delivered more than once but must not be silently lost while retained. | Clients/runners deduplicate; WorldStream does not claim exactly-once networking or model execution. |
| **Observation acknowledgement** | A client's statement that it has durably processed frames through a particular membership `frame_seq`. | It advances only that membership cursor; losing it causes redelivery, not data loss. This differs from the server acknowledging an accepted action after commit. |
| **Single logical writer** | Exactly one active room actor serializes accepted stimuli for a room. | Different rooms may progress concurrently. It is not a single writer for the whole server. |
| **Snapshot** | A paired, verified, replaceable postcommit checkpoint of Core and Activity State at one Room sequence, binding the complete applicable Head and all three state hashes. | The pair is one disposable cache; Genesis plus Transitions remain sufficient after every snapshot is deleted. |
| **Current materialization** | A verified current row or in-memory value derived from canonical lineage for serving, including current Core/Membership and Activity rows. | It is not an independent source of truth and may be rebuilt by Replay. |
| **Recovery** | Reconstruction of an operational Room after startup/failure from Genesis or a verified paired snapshot plus Transition tail. | Recovery restores verified serving materializations and does not contact Runners or repeat external effects. |
| **Replay** | Read-only deterministic reconstruction and verification at a requested room sequence using the exact pinned pack. | Replay sends no live frames, starts no invocations, and creates no fork in v0.1/v0.2. |
| **Verifier repair** | A generation-fenced operator-requested process that verifies canonical lineage and may rebuild materializations/caches, reinstall the exact pack, or restore exact canonical bytes from a verified backup. | Only its successful verification may restore healthy integrity; it never edits, skips, or replaces Genesis or Transitions. |
| **Passivation** | Safe removal of an idle in-memory room actor after a generation-fenced drain/barrier. | The room and its durable state continue to exist. |
| **Storage profile** | One startup-selected implementation of the backend-neutral durable-storage contract. The frozen profiles are the release-bundled SQLite default and `postgres-primary` for PostgreSQL 17. | A profile changes storage operations, not Room semantics, receipts, hashes, timers, or canonical codecs. It is fixed for a running process. |
| **Storage Epoch** | A deployment-wide, monotonically increasing identity for the currently authoritative storage lineage. | Offline SQLite-to-PostgreSQL finalization advances it; stale or retired storage cannot serve or accept mutation under the new epoch. |
| **Storage Compatibility Manifest** | The reviewed machine-readable contract for engine/build support, settings, schema and migration checksums, canonical and receipt codecs, transfer/recovery formats, connection modes, platforms, artifacts, and evidence. | Root [`compatibility.toml`](../compatibility.toml) and [`compatibility.json`](../compatibility.json) are the authored fail-closed specification while `release_ready` is false; only a fully populated, verified pair marked ready is embedded/published as a release-valid manifest. |
| **Transfer bundle** | A deterministic, resumable, checksummed export used only for whole-deployment offline SQLite-to-PostgreSQL transfer. | It preserves canonical serialized bytes and authority evidence; it is not a live replication stream or room-by-room move. |
| **WorldStream semantic verifier** | A read-only, full-deployment verifier that validates storage lineage and every healthy Room from Genesis through Head using exact retained pack executors. | It complements backend-native backup/restore; it does not repair or sample authoritative state. Exactly preserved pre-existing unhealthy Rooms may remain isolated and unhealthy. |
| **WAL** | SQLite write-ahead log mode used by the bundled SQLite storage profile for local durability and concurrent bounded reads. | It is a backend mechanism, not WorldStream's canonical transition history. |
| **Content-addressed storage (CAS)** | v0.2 local artifact storage whose immutable path/identity derives from a verified BLAKE3 digest. | Artifact bytes are outside the selected database; authoritative room linkage is committed through a typed action. |
| **Artifact** | Immutable external bytes plus generic verified metadata, introduced in v0.2. | Facts, claims, evidence versions, and supersession remain Activity Pack concepts. |
| **Artifact reference** | An authorized digest, size, safe media type, label, and pack-level relationship exposed through projection/context. | Observation frames carry references, not entire artifact bodies. |
| **Staged upload** | An owner-scoped, expiring upload record created after artifact bytes are durable but before a typed room action links them. | It is not canonical room content until the linking transition commits. |
| **Derived data / cache** | Rebuildable data such as a snapshot, index, cached projection, or structural lookup table. | It may accelerate reads but cannot authorize access or replace canonical history. |
| **Ephemeral operational data** | Non-authoritative runtime state such as connection presence, in-memory queues, metrics, and runner availability. | Losing it may affect service quality but must not change canonical room truth. |
| **Membership inbox** | The durable database representation of one membership's retained observation frames. | It is an implementation/storage term for the observation stream, not a general chat mailbox. |

## UI and presentation vocabulary

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Reference UI** | The small first-party web client used to demonstrate, inspect, and test Agent Heist and Investigation Room. | It is not a general dashboard builder or required to operate the server. |
| **Public view** | A UI rendering of a public/spectator projection for an authorized read-only membership. | Public does not imply anonymous access or a raw global feed. |
| **Participant view** | A UI rendering of one acting membership's authorized projection, Action Offers, and observation state. | It must never contain another membership's private fields. |
| **Operator-membership view** | A UI rendering of room-scoped diagnostics, controls, and operator projection authorized for an operator membership. | It is distinct from the host operator trust role. UI visibility is not authorization; the server rechecks every operation. |
| **Inspector** | A diagnostic view of authorized room metadata, connections, cursors, activation status, timers, and hashes. | It is not direct database access or unrestricted canonical-state exposure. |
| **Timeline** | An authorized presentation of Domain Events and Transition/Observation metadata in Room order. | It is not necessarily the complete Canonical Activity State, Core Room State, or another Participant's private history. |
| **Replay view** | A read-only UI over deterministic historical reconstruction under replay authorization. | It cannot mutate, activate agents, or bypass pack-defined reveal rules. |
| **Live mode** | The UI follows newly committed authorized observations from the current cursor. | It is distinct from connection status; a disconnected client may still show the last live projection. |
| **Catching-up mode** | The UI is applying retained frames or an explicit projection reset before joining live delivery. | It must not silently present stale data as current. |
| **Replay mode** | The UI shows an authorized historical room sequence and is read-only. | It never submits live actions or creates activations. |

## Reference-activity vocabulary

| Term | Meaning |
|---|---|
| **Agent Heist** | The v0.1 ActivityPackV1 reference: three immutable Genesis seats, six phases, sealed two-of-three plan selection, five-check scoring, scoped reveal, timers, Activation separation, recovery, and Replay. It is a test vehicle, not the product category. |
| **Investigation Room** | The v0.2 serious-work reference activity proving that the same room semantics support a human Lead, multiple agents, evidence correction, dependency invalidation, activation, verification, and a deterministic structured outcome. |
| **Cold Chain Incident** | The fictional deterministic Investigation fixture. It uses local immutable evidence and no live enterprise systems. |
| **Evidence version** | An Investigation Pack identity for one immutable source revision. It is not a generic artifact-store column. |
| **Fact** | A source-linked Investigation assertion derived from exact evidence versions. |
| **Claim** | A higher-level Investigation assertion supported or challenged by facts/other claims. |
| **Supersedes relation** | An explicit Investigation relationship saying a newer immutable evidence version replaces an earlier version for current analysis. It never overwrites history. |
| **Invalidation / stale dependency** | A deterministic pack consequence marking derived work out of date when a referenced source/revision changes. It does not delete the prior work. |

## Preferred language

| Avoid | Say instead | Why |
|---|---|---|
| “WorldStream is a multiplayer game server.” | “WorldStream is a realtime room runtime for multi-agent applications; multiplayer servers are the architectural analogy.” | The product supports non-game activities. |
| “WorldStream is a multi-agent orchestrator.” | “WorldStream coordinates multi-participant shared state; external runners operate the agents.” | It does not own agent plans, models, or tool loops. |
| “The runtime” when ownership is unclear. | Use “WorldStream Server,” “Room Kernel,” or “runner-owned execution runtime.” | These have different trust, durability, and execution responsibilities. |
| “The operator” when scope is unclear. | Use “host operator” or “operator membership.” | One controls the deployment; the other is a room-scoped access mode. |
| “Context” without qualification. | Use “deterministic context,” “invocation context,” or “agent-private memory.” | These name pack helpers, one run's authorized input, and runner-owned state respectively. |
| “The agent lives in the room.” | “The agent has a durable room membership.” | Membership persists; computation normally does not. |
| “The agent is sleeping.” | “The membership is enabled with no active invocation,” or “an activation is pending.” | There is no resident sleeping model or process. |
| “WorldStream wakes the agent.” | “WorldStream creates/offers an activation intent; a runner may start a fresh invocation.” | The server requests execution but does not execute the model itself. |
| “The agent resumes where it stopped.” | “A fresh invocation catches up from its cursor and authorized context.” | Hidden process/model state is not preserved. Use resume only for a runner-owned explicit checkpoint. |
| “The agent is online.” | “A session is connected,” “a runner is available,” or “an invocation is running.” | These are three different operational facts. |
| “Persistent agent memory.” | “Durable room state,” “invocation context,” or “runner-owned private memory.” | The system does not provide generic agent memory. |
| “WorldStream is a context layer/database.” | “WorldStream owns authoritative room state and derives scoped invocation context.” | It is not generic RAG, vector memory, or document search. |
| “Agents update shared state.” | “Agent Participants submit typed Actions; the Room Kernel commits validated Transitions.” | Clients cannot directly write Authoritative Room State. |
| “The stream contains room state.” | “The Membership stream contains authorized Observation Frames; current state is obtained through a Projection.” | Raw Core Room State and Canonical Activity State never cross the client boundary. |
| “Public stream.” | “Public projection materialized into each authorized membership stream.” | Every durable viewer has a membership and cursor in the frozen protocol. |
| “Event” when the kind is unclear. | Use “action,” “stimulus,” “transition,” “domain event,” or “observation frame.” | These have different authority, ordering, and delivery semantics. |
| “Exactly-once delivery/execution.” | “At-least-once delivery with idempotent requests, deduplication, and fenced leases.” | Networks and model processes cannot honestly provide exactly-once execution. |
| “The room completed.” | “The Activity reached a Terminal Phase and established an Outcome; the Room remains active until archived.” | Activity Phase, Outcome, and core Room Status are separate. |
| “The snapshot is the room history.” | “Genesis and transitions are canonical; snapshots accelerate recovery.” | Snapshots are replaceable caches. |
| “Activity Pack plugin.” | “Trusted compiled-in Activity Pack.” | There is no stable public plugin ABI or untrusted pack loading through v0.2. |
| “Project,” “Workspace,” or “world containing packs.” | “Independent room pinned to one Activity Pack.” | Cross-room projects and multi-pack rooms are explicitly deferred. |
| “Worker” without qualification. | Use “runner,” “invocation,” “room actor,” or “participant.” | Worker is overloaded and hides ownership/lifecycle differences. |

## Naming and spelling conventions

- **WorldStream** is the provisional project spelling. Never write **WordStream**.
- The current `agent-streamer` repository directory is a legacy local slug, not a product name. Rename it only after public-brand clearance.
- Use **WorldStream Server** for the deployable service and **Room Kernel** for its internal correctness core.
- Use **Activity Pack** for the named abstraction and **pack** after the context is established.
- Use **Agent Heist**, **Investigation Room**, and **Cold Chain Incident** as proper names.
- Use **WebSocket**, **SQLite**, **BLAKE3**, **JSON**, **HTTP**, and **ULID** with their standard capitalization.
- Use `room_id`, `member_id`, `principal_id`, `action_id`, `activation_id`, and `claim_id` in protocol/schema contexts.
- Use `room_seq` for canonical transition order and `frame_seq` for one membership's observation order.
- Use **realtime** in the product-category phrase. Do not imply a hard real-time system.
- Use **self-hosted**, **multi-agent**, and **multi-participant** with hyphens.

## Five-question terminology check

Before introducing a new term, ask:

1. Does it describe durable identity, temporary execution, transport, or domain state?
2. Is it a core WorldStream concept or an Activity Pack concept?
3. Is it authoritative input/history, derived projection, or operational telemetry?
4. Does it accidentally imply model hosting, continuous cognition, exactly-once execution, or game-only scope?
5. Can an existing canonical term express the same idea more precisely?
