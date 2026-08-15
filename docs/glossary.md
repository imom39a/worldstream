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
| **Membership** | One Principal's durable room-local seat, including membership identifier, access mode, status, cursor, and any pack-defined role or activation policy. | Membership survives connection loss and invocation termination. |
| **Room Member** | A Principal considered in one Room through a Membership, whether acting, spectating, or operating. | Use this as the umbrella; reserve Participant for participant-access memberships. |
| **Member ID** | The room-scoped identifier of one Membership, used for authorization and observation addressing. | It is not another Member entity and is not interchangeable with the server-level principal ID. |
| **Membership status** | Core membership lifecycle: enabled, suspended, or departed. | It is separate from connection status, runner availability, and activation status. |
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
| **Room status** | Core administrative state: active or archived. | Separate from room health and pack-defined activity phase. |
| **Room health** | Core integrity/runtime state: healthy, faulted, or quarantined. | A fault/quarantine is not an Activity Pack outcome. |
| **Activity phase** | A pack-defined domain phase such as Briefing, Commitment, Analysis, or Closed. | Separate from room status and health. |
| **Terminal phase** | A pack-defined phase after which ordinary domain Actions are no longer permitted. | It is separate from Activity Outcome and core Room archival. |
| **Outcome** | A pack-defined final result and optional deterministic score, possibly established before the Terminal Phase. | Reaching an Outcome does not automatically archive the core Room. |
| **Core room state** | WorldStream-owned current facts about Room lifecycle and Memberships. | It is distinct from pack-owned Activity State and from operational Room health. |
| **Activity state** | Activity Pack-owned current domain facts for one Room. | It contains phases, domain entities, deadlines, and Outcome—not Sessions, Runner state, host diagnostics, or a duplicated cache of derived Legal Actions. |
| **Authoritative room state** | The accepted current shared truth comprising Core Room State and Activity State, changed only by committed Transitions. | Clients never mutate or receive this aggregate directly. |
| **Canonical activity state** | The deterministic, schema-valid representation of Activity State used for hashing and replay. | It is not a second domain state and does not by itself include Core Room State. |
| **Canonical history** | The immutable room genesis followed by the append-only ordered transitions. | Snapshots, indexes, projections, frames, and telemetry are derived or delivery records rather than substitutes for canonical history. |
| **Genesis** | The immutable sequence-zero creation record containing pack digest, configuration, ordered initial memberships/roles, room seed, logical creation time, and initial hashes. | Genesis is sufficient to reconstruct the room when every snapshot is absent. |
| **Room head** | The latest committed room sequence and its transition/state hashes. | It is global to the room, unlike a membership frame cursor. |
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
| **Action rejection** | An expected, deterministic admitted refusal such as stale state, illegal domain action, or expired deadline. | It creates no transition. A durable rejection receipt may consume the action ID. |
| **Transient error** | A failure such as unauthenticated, malformed, rate-limited, room-busy, storage-unavailable, or pre-commit runtime fault. | It creates no action receipt and does not consume a new action ID; an identical request may retry it. |
| **Activity fault** | A pack implementation or integrity failure, such as panic, invalid output, nondeterminism, or projection failure. | It is not expected domain behavior and may fault or quarantine the room. |
| **Action receipt** | The durable idempotency result for one accepted action or deterministic admitted rejection. | It prevents a lost reply from causing duplicate mutation. |
| **Mutation receipt** | The durable idempotency result for a mutating HTTP administration request. | It is distinct from a participant action receipt. |
| **Stimulus** | A fully recorded candidate presented to deterministic pack logic: participant action, timer firing, membership change, external input, or administrative input. | Only an accepted Stimulus produces a Transition. Ambient wall time, network data, and random calls are not Stimuli unless explicitly recorded by the host. |
| **External input** | A host-operator-authenticated, idempotent recorded stimulus from a named source, used narrowly for controlled fixture/input ingestion. | It is not a participant, connector platform, webhook framework, or permission to perform pack I/O. |
| **Host stimulus source** | A server-controlled source of recorded timers or external inputs, such as the Heist facility or Investigation evidence feed. | It is not a human/agent principal, participant, runner, or room actor. |
| **Transition** | One durably ordered accepted Stimulus plus its deterministic next state, domain events, timer changes, attention signals, and hashes. | It is the canonical mutation unit and consumes one room sequence. |
| **Domain event** | A typed semantic fact emitted by the Activity Pack inside a transition, such as plan proposed or claim invalidated. | It explains consequences for audit/UI; it is not an independently ordered broker event or a raw client action. |
| **Logical time** | A host-recorded timestamp or deadline value supplied to deterministic pack logic and preserved for replay. | Wall-clock commit time used for operations/metrics is not automatically logical time. |
| **Timer** | A durable, generation-numbered request for the host to submit a recorded TimerFired stimulus at or after a due time. | It is not an arbitrary cron workflow or an in-memory sleep. |
| **Deterministic context** | Restricted host-supplied pure helpers, including labeled randomness derived from the room seed/sequence and canonical-value utilities. | Recorded time arrives through genesis or the recorded stimulus, not ambient clock access. The context provides no database, network, filesystem, environment, model, wallet, or secret access. |
| **State hash** | A BLAKE3 digest of the pack digest and Canonical Activity State. | It proves deterministic equality of pack-state bytes, not equality of every core/operational record or business correctness. |
| **Transition hash** | A domain-separated digest binding room, sequence, stimulus, ordered domain events, prior hash, and resulting state hash. | It forms the room integrity chain. |
| **Pack digest** | The immutable content/revision identity pinned by a room. | Pack name or semantic version alone is insufficient for recovery and replay. |

## Projection, observation, stream, and context

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Projection** | A current, full, authorized view derived from authoritative room state for one viewer. | It is not raw state and does not grow like a transcript. |
| **Activity projection** | The pack-produced domain portion of a Projection. | WorldStream adds authorized Core Room and Membership facts; the pack does not own Room Status or Membership metadata. |
| **Projection envelope** | A protocol payload carrying one authorized Projection plus causal sequence, Room Health, schema, hash, and delivery metadata. | The envelope is not Authoritative Room State. Metadata beside a Projection does not become part of that Projection or its domain truth. |
| **Public projection** | A pack-defined view safe for every Membership authorized to see public consequences. | In the frozen protocol it is materialized into Membership-addressed streams; it is not a separate anonymous durable stream. |
| **Participant projection** | A current view containing information authorized for one acting membership. | Two participants in the same role may still receive different data. |
| **Operator projection** | A current administrative/diagnostic view explicitly produced for an authorized operator membership. | It is still a projection and must not bypass pack/privacy boundaries casually. |
| **Final-reveal projection** | A pack-defined historical/completed view that may disclose information after an activity terminates. | Replay by itself does not grant reveal access. |
| **Observation** | Information a membership is allowed to learn because the room changed. | Projection is current full view; observation is change-oriented delivery. |
| **Observation frame** | A durable, bounded, Membership-addressed change, notice, or resynchronization frame associated with committed Room state. | A frame may combine public consequences and private changes authorized for that Room Member. |
| **Observation stream** | One membership's monotonically sequenced observation frames. | It is not a global raw event stream, room state dump, or LLM token stream. |
| **Frame sequence (`frame_seq`)** | A monotonic position inside one membership's observation stream. | It is independent from `room_seq` and may skip transitions irrelevant to that member. |
| **Cause room sequence (`cause_room_seq`)** | The committed transition that caused an observation frame or activation intent. | It provides causality but does not replace that object's own frame or activation identity. |
| **Cursor** | The highest frame sequence a membership has durably processed and acknowledged. | A cursor belongs to one membership stream, not the entire room. |
| **Catch-up** | Delivery of retained authorized frames after a membership cursor during attach/reconnect. | Catch-up is transport continuity; it is not deterministic room replay or cognitive resumption. |
| **Projection reset** | An authorized current projection sent when the required historical frame range has been pruned. | The reset is explicit and atomically establishes a new frame baseline; the server never silently skips a gap. |
| **Legal action** | A typed action currently available to a participant, including relevant schema and deadline information. | “Affordance” is an informal UX synonym. The server still revalidates every submitted action. |
| **Realtime** | Commit-first push of relevant observations to connected clients, with bounded latency targets and durable cursor recovery. | It does not mean every database event is broadcast or every agent responds instantly. |
| **Stream** | In product language, one Membership's ordered progression of meaningful Room Observations. | Do not use it to imply token streaming, video streaming, or a generic message broker. |
| **Invocation context** | A temporary authorized input bundle for one agent invocation: activation cause, current projection, retained relevant frames or reset, legal actions, deadline/budget metadata, and artifact references. | It is assembled for a run and is not a new authoritative database or persistent agent mind. |
| **Agent-private memory** | Prompts, model history, private checkpoints, summaries, or other state owned by the external runner/agent implementation. | WorldStream does not own, inspect, or promise this memory. |

## Activation and ephemeral agent execution

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Attention signal** | Deterministic Activity Pack output saying a particular enabled Agent Participant may need to act for a typed reason. | It cannot target spectator/operator Memberships, does not execute a model, and is not an arbitrary semantic polling query. |
| **Activation policy** | Host-enforced per-membership rules deciding whether an attention signal may create an activation intent. | It limits activations; it is not the agent's private planning policy. |
| **Activation intent** | A durable, unique, at-least-once request offered to an authorized external runner. | It means work may be relevant; it does not prove a model ran. |
| **Activation ID** | The server-generated durable identity of one activation intent. | It is distinct from the pack's logical deduplication key and from a runner claim ID. |
| **Activation offer** | A possibly duplicated, privacy-minimal notification that an activation may be claimed. | Private projection/context is returned only after an authorized claim succeeds. |
| **Activation claim** | A runner's idempotent request to obtain the current bounded lease for an activation. | A claim ID and request hash make lost replies safe. |
| **Claim ID** | A runner-generated idempotency key for one activation claim attempt. | A runner uses a new claim ID to try again after a stored not-available result. |
| **Activation deduplication key** | A stable pack-produced key combined with room, cause sequence, and target membership to prevent duplicate logical activation creation. | It is not a network message ID or claim ID. |
| **Lease** | A time-bounded exclusive right for one authenticated runner to handle the current activation generation. | It is operational ownership, not room-state authority. |
| **Lease generation** | A monotonically increasing fence that prevents an expired claimant from renewing, releasing, or completing a newer lease. | Runner identity alone is insufficient to fence stale operations. |
| **Activation completion** | A runner's operational report that it handled, declined, or failed the activation. | Only separately accepted room actions create authoritative room consequences. |
| **Logical agent persistence** | Persistence of principal, membership, role, permissions, cursor, activation state, room facts, and explicit references across invocations. | It does not mean continuous computation, consciousness, model context, or an idle container. |

## Activity Pack vocabulary

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Activity Pack** | Trusted, versioned domain rules and schemas compiled into the server for v0.1/v0.2. It defines configuration, Activity State, roles, actions, deterministic reduction, projections, timers, attention reasons, phases, and outcomes. | It is not a workflow graph, model prompt bundle, arbitrary connector, or public plugin in the frozen releases. |
| **Activity Pack host interface** | The internal Rust contract through which the WorldStream Server initializes, applies, projects, observes, and validates a trusted compiled-in Activity Pack. | It is provisional through v0.2 and is not a stable public plugin ABI. |
| **Manifest** | Pack identity and declared schemas, limits, roles, actions, projection versions, and compatibility metadata. | The manifest describes a pack; the pinned digest identifies its exact executable revision. |
| **Pack identifier** | A stable namespaced logical name such as `worldstream.agent-heist`. | It groups revisions but does not uniquely select executable rules. |
| **Pack version** | A human-facing declared release version for a pack. | It aids compatibility and documentation; the digest remains authoritative. |
| **Room configuration** | Immutable creation input validated by the pack and committed into genesis. | Later domain changes belong in canonical pack state; server deployment configuration is separate. |
| **Reducer / `apply`** | The deterministic pack operation that validates an admitted stimulus against current state and returns the complete next state plus typed outputs. | It performs no host I/O or model execution. |
| **Projection function / `project`** | The deterministic pack operation that creates a current authorized view for one viewer. | It constructs allowed data rather than serializing and redacting raw state. |
| **Observation function / `observe`** | The deterministic pack operation that creates one viewer's bounded change-oriented observation after a transition. | It is persisted/delivered only after authorization and successful commit. |
| **Attention reason** | A pack-declared stable code explaining why an Agent Participant may need Activation. | It is typed application semantics, not free-form LLM judgment. |
| **Pack conformance** | Tests proving determinism, schema validity, privacy noninterference, bounded outputs, recovery, and replay for an exact pack revision. | Passing conformance does not make untrusted third-party pack execution safe. |

## Persistence and delivery vocabulary

| Term | Summary or technical elaboration | Important distinction |
|---|---|---|
| **Durable** | Persisted with the required filesystem/database ordering before success is acknowledged. | In-memory queues and live WebSocket delivery are not durable. |
| **Ephemeral** | Temporary operational state that may disappear without changing canonical room truth, such as sessions, runner presence, or an invocation process. | Ephemeral does not mean unimportant; it means recovery cannot depend on it. |
| **Commit-before-acknowledgement** | The invariant that accepted mutation, frames, timers, activations, hashes, and receipt commit before success is returned or streamed. | A lost reply after commit is recovered through idempotency and cursors. |
| **Idempotency** | Repeating the same scoped request with the same key and canonical body returns the original result without repeating mutation. | Same key with changed body is a conflict, not a new operation. |
| **At-least-once delivery** | A frame or activation offer may be delivered more than once but must not be silently lost while retained. | Clients/runners deduplicate; WorldStream does not claim exactly-once networking or model execution. |
| **Observation acknowledgement** | A client's statement that it has durably processed frames through a particular membership `frame_seq`. | It advances only that membership cursor; losing it causes redelivery, not data loss. This differs from the server acknowledging an accepted action after commit. |
| **Single logical writer** | Exactly one active room actor serializes accepted stimuli for a room. | Different rooms may progress concurrently. It is not a single writer for the whole server. |
| **Snapshot** | A verified, replaceable checkpoint of Canonical Activity State used to accelerate Room load. | It is a cache; genesis plus Transitions remain sufficient. |
| **Recovery** | Reconstruction of an operational room after startup/failure from genesis or a verified snapshot plus transition tail. | Recovery restores serving state and does not contact runners or repeat external effects. |
| **Replay** | Read-only deterministic reconstruction and verification at a requested room sequence using the exact pinned pack. | Replay sends no live frames, starts no invocations, and creates no fork in v0.1/v0.2. |
| **Passivation** | Safe removal of an idle in-memory room actor after a generation-fenced drain/barrier. | The room and its durable state continue to exist. |
| **Storage profile** | One startup-selected implementation of the backend-neutral durable-storage contract. The frozen profiles are the release-bundled SQLite default and `postgres-primary` for PostgreSQL 17. | A profile changes storage operations, not Room semantics, receipts, hashes, timers, or canonical codecs. It is fixed for a running process. |
| **Storage Epoch** | A deployment-wide, monotonically increasing identity for the currently authoritative storage lineage. | Offline SQLite-to-PostgreSQL finalization advances it; stale or retired storage cannot serve or accept mutation under the new epoch. |
| **Storage Compatibility Manifest** | The reviewed, embedded, machine-readable release contract for engine/build support, settings, schema and migration checksums, canonical and receipt codecs, transfer/recovery formats, connection modes, platforms, and evidence. | `compatibility.toml` is reviewed source and canonical `compatibility.json` is the published/embedded form. |
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
| **Participant view** | A UI rendering of one acting membership's authorized projection, legal actions, and observation state. | It must never contain another membership's private fields. |
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
| **Agent Heist** | The v0.1 reference activity proving private views, concurrent decisions, timers, disconnect/catch-up, activation, restart recovery, and replay. It is a test vehicle, not the product category. |
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
