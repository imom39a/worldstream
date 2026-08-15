# Activity Pack Design

## Status

This is the provisional trusted Rust Activity Pack contract for Agent Heist v0.1 and Investigation Room v0.2.

It is intentionally not a stable public plugin ABI. The interface may be corrected while Heist is built, then is frozen for the Investigation generality test. Only after both activities pass should the project consider a portable or sandboxed ABI.

## Purpose

An Activity Pack defines what one room means:

- configuration and Canonical Activity State;
- participant roles;
- typed actions and legal-action rules;
- deterministic state transitions;
- public, participant, and operator projections;
- timers and attention signals;
- completion conditions and scoring.

WorldStream defines how the room is ordered, persisted, recovered, streamed, reattached, activated, and replayed.

One room pins exactly one Activity Pack revision. Packs cannot call each other or mutate another room in v0.1 or v0.2.

## When an activity fits

An activity is a good fit when:

- two or more independent participants affect shared state;
- roles or visibility differ;
- participants select actions rather than following a fixed server-authored workflow;
- actions can conflict or change legal actions;
- timers, reconnect, recovery, or replay matter.

An activity is a poor fit when it is merely:

- a one-agent task wrapper;
- a connector or ETL step;
- a predetermined process graph;
- a document collection;
- a model or tool execution sandbox.

The pack author still owns the domain model. WorldStream is valuable only if its room semantics remove substantial repeated infrastructure. This Activity Pack tax is an explicit validation risk.

## Package contents in the frozen releases

Each trusted built-in pack contains:

    activity/
    ├── manifest data
    ├── canonical Rust state and input types
    ├── initialization
    ├── action/stimulus application
    ├── projection and observation construction
    ├── timer and attention reason definitions
    ├── deterministic fixture data
    ├── conformance and privacy tests
    └── first-party UI projection schemas

It does not contain:

- an LLM or model provider SDK;
- agent prompts or private memory;
- filesystem, database, network, shell, wallet, or secret access;
- workflow nodes;
- HTML or arbitrary JavaScript;
- another Activity Pack;
- cross-room references.

## Manifest

Logical manifest:

~~~json
{
  "pack_id": "worldstream.agent-heist",
  "name": "Agent Heist",
  "version": "0.1.0",
  "revision_digest": "blake3:...",
  "host_api": "0.1",
  "configuration_schema": "agent-heist.config.v1",
  "state_schema": "agent-heist.state.v1",
  "roles": [
    {
      "id": "navigator",
      "minimum": 1,
      "maximum": 1,
      "allowed_principal_kinds": ["agent", "human"]
    }
  ],
  "maximum_participants": 8,
  "actions": [
    {
      "type": "publish_clue",
      "schema": "agent-heist.publish-clue.v1"
    }
  ],
  "attention_reasons": [
    "offer_received",
    "commitment_opened",
    "required_action_deadline"
  ],
  "projection_schemas": {
    "public": "agent-heist.public.v1",
    "participant": "agent-heist.participant.v1",
    "operator": "agent-heist.operator.v1"
  },
  "limits": {
    "maximum_state_bytes": 2097152,
    "maximum_transition_output_bytes": 262144
  }
}
~~~

Principal kind and Role are separate. A human or agent Principal may hold a Role through a participant Membership if the manifest permits it.

The Activity Pack defines Role names, cardinality, permissions, and Legal Actions. The WorldStream Core reducer exclusively records current assignment in the semantic Membership map. Pack Activity State MUST NOT persist a second role-to-Membership ownership index; reduction derives any needed lookup from the supplied immutable Core view.

The revision digest pins the exact compiled behavior and schemas. A semantic version is explanatory; replay trusts the digest.

## Core boundary seen by packs

`CoreRoomState v1` is host-owned and contains exactly Room Status plus a canonically sorted Membership map. For each Membership it exposes immutable Member ID, Principal ID, and room-local Principal kind; enabled/suspended/departed standing; participant/spectator/operator Access Mode; and a Role exactly for participant access. Room Head, hashes, integrity, Sessions, Cursors/Frames, receipts, policy/Activation, diagnostics, telemetry, and commit time are not Core.

For every reduction, the host supplies immutable Core-before and proposed-Core-after values. They are equal for a non-Core Stimulus. A versioned Core Stimulus carries authority attribution, idempotency identity, exact expected sequence, reason code, semantic time when applicable, and a canonical before/after changeset. One Stimulus may change several Memberships atomically, sorted by Member ID with at most one pair per ID; the pack observes only the complete proposed final state.

The pack may declare a stable veto for join, resume, Access Mode, or Role proposals. It may not veto archive, suspend, or depart; attempting to do so is a Pack Fault. A veto produces an idempotent administrative rejection with no Transition, not an Activity Fault. Packs may change Activity State or emit deterministic outputs in response to an accepted Core change, but cannot mutate Core itself.

## Logical Rust interface

~~~rust
pub trait ActivityPack: Send + Sync + 'static {
    fn manifest(&self) -> ActivityManifest;

    fn initialize(
        &self,
        input: RoomInitialization,
        context: &DeterministicContext,
    ) -> Result<CanonicalValue, ActivityFault>;

    fn apply(
        &self,
        state: &CanonicalValue,
        core_before: &CoreRoomStateV1,
        proposed_core_after: &CoreRoomStateV1,
        stimulus: &RecordedStimulus,
        context: &DeterministicContext,
    ) -> Result<AppliedTransition, ApplyError>;

    fn project(
        &self,
        state: &CanonicalValue,
        viewer: &Viewer,
    ) -> Result<ActivityProjection, ActivityFault>;

    fn observe(
        &self,
        before: &CanonicalValue,
        after: &CanonicalValue,
        events: &[DomainEvent],
        viewer: &Viewer,
    ) -> Result<Option<ActivityObservation>, ActivityFault>;
}

pub enum ApplyError {
    Rejected(DeclaredRejection),
    ActivityFault(ActivityFault),
}
~~~

Typed helper traits may let a pack implement strongly typed state and action enums. The host boundary canonicalizes them before hashing or persistence.

### RoomInitialization

Contains only recorded input:

- room ID;
- room configuration;
- exact initial `CoreRoomState v1` constructed and validated by the host;
- room seed;
- recorded logical creation time;
- pack revision digest.

### RecordedStimulus

RecordedStimulus is a fully recorded candidate for deterministic application. It becomes the committed Stimulus of a Transition only when apply succeeds and the complete transition commits. The host may present:

~~~rust
pub enum RecordedStimulus {
    ParticipantAction {
        member_id: MemberId,
        principal_kind: PrincipalKind,
        role: String,
        action_id: ActionId,
        based_on_room_seq: u64,
        action_type: String,
        payload: CanonicalValue,
        admitted_at: RecordedTime,
    },
    TimerFired {
        timer_id: TimerId,
        generation: u32,
        scheduled_for: RecordedTime,
        fired_at: RecordedTime,
        payload: CanonicalValue,
    },
    CoreChanged {
        core_stimulus: CoreStimulusV1,
    },
    ExternalInput {
        source_id: String,
        input_id: String,
        input_type: String,
        payload: CanonicalValue,
        recorded_at: RecordedTime,
    },
}
~~~

ExternalInput is host-operator-authenticated and idempotent. In frozen reference demos, fixture evidence is released through recorded timers or deterministic local fixture input. It is not a generic connector mechanism.

### AppliedTransition

~~~rust
pub struct AppliedTransition {
    pub next_state: CanonicalValue,
    pub domain_events: Vec<DomainEvent>,
    pub timer_changes: Vec<TimerChange>,
    pub attention_signals: Vec<AttentionSignal>,
}
~~~

There is no arbitrary effect intent in the frozen interface. WorldStream does not execute emails, trades, deployments, shell commands, or external APIs.

### DomainEvent

A domain event is an inspectable consequence for audit, observation construction, and the reference UI:

~~~rust
pub struct DomainEvent {
    pub event_type: String,
    pub payload: CanonicalValue,
    pub visibility: EventVisibility,
}

pub enum EventVisibility {
    Public,
    Members(Vec<MemberId>),
    Operator,
}
~~~

Exact member IDs are preferred for private historical facts. A role-based audience can accidentally expose an earlier event to someone assigned that role later.

Domain events are ordered inside one transition. They do not receive independent room sequences.

### TimerChange

~~~rust
pub enum TimerChange {
    Schedule {
        timer_id: TimerId,
        generation: u32,
        due_at: RecordedTime,
        payload: CanonicalValue,
    },
    Cancel {
        timer_id: TimerId,
        generation: u32,
    },
}
~~~

The pack chooses the logical due time based on recorded input. The host performs scheduling and later records TimerFired.

### AttentionSignal

~~~rust
pub struct AttentionSignal {
    pub member_id: MemberId,
    pub reason_code: String,
    pub priority: u8,
    pub allowed_action_types: Vec<String>,
    pub deadline: Option<RecordedTime>,
    pub deduplication_key: String,
}
~~~

An Attention Signal says an Agent Participant may need to act. It is not a transport instruction and does not start an agent.

In the frozen releases, one AppliedTransition may emit at most one AttentionSignal for a given target Membership. Multiple affected IDs are coalesced into that Membership's authorized Observation and Invocation Context.

The host:

1. validates the reason against the manifest;
2. validates that the target is an enabled participant Membership for an agent Principal with a pack-permitted Role, and checks Room policy;
3. deduplicates by transition and key;
4. persists an ActivationIntent if allowed;
5. offers it to an external runner separately.

A human participant can receive an ordinary observation/notification for the same domain change without an agent activation.

## Projection contract

Viewer variants:

~~~rust
pub enum Viewer {
    Public,
    Participant {
        member_id: MemberId,
        principal_kind: PrincipalKind,
        role: String,
    },
    Operator {
        scopes: Vec<String>,
    },
    ReplayReveal {
        policy: String,
    },
}
~~~

The pack produces an ActivityProjection containing only pack-owned domain information. WorldStream constructs the client-facing Projection by wrapping it with authorized Core Room facts such as Room Status and the authenticated Membership's metadata. A separate protocol Projection Envelope carries causal, operational, and delivery metadata such as complete Room Head, Room Integrity State/generation, schema, and hash. Neither layer may overwrite fields owned by another.

ActivityObservation is the pack-produced authorized delta and legal-action update caused by a Transition. WorldStream wraps it with the recipient Membership and causal delivery metadata.

Every participant ActivityProjection should contain:

- current pack-defined phase;
- current authorized facts;
- current legal actions;
- personal and shared deadlines;
- explicit artifact references rather than blob contents;
- a projection schema identifier.

It should not contain:

- another participant's private fields;
- internal reducer-only values;
- chain-of-thought;
- raw unbounded history;
- provider credentials or runner configuration.

The host enforces maximum bytes but cannot determine semantic privacy. Packs require adversarial noninterference tests.

## Determinism contract

For the same:

- Activity Pack revision digest;
- genesis input and seed;
- immutable Core-before/proposed-after values;
- ordered recorded stimuli;

the pack MUST produce byte-identical Canonical Activity State hashes and logically identical ordered Transition output. The host's Core reducer independently reproduces the Core State hash; the host then binds Core, Activity, and aggregate hashes into Genesis/Transition lineage.

Forbidden inside initialize, apply, project, and observe:

- wall-clock reads;
- OS randomness;
- network calls;
- filesystem or environment access;
- database queries;
- process-global mutable state;
- thread races;
- LLM or tool calls;
- floating-point Activity State.

Allowed:

- deterministic helpers supplied in DeterministicContext;
- integer and fixed-point arithmetic;
- labeled randomness derived from room seed and next sequence;
- recorded timestamps and payloads from the stimulus;
- pure schema and projection code.

Maps are canonicalized by key. Unknown input fields are rejected. Golden replay fixtures run on every supported platform in CI.

## Rejections and faults

ApplyError has two variants: Rejected and ActivityFault. Rejected is expected for a ParticipantAction:

- domain constraint violated after strict schema admission;
- wrong role;
- action not legal in current phase;
- missing resource or evidence;
- deadline passed;
- duplicate domain commitment;
- activity in a pack-defined terminal phase.

It produces no canonical Transition.

Rejected is also valid for a vetoable Core proposal—join, resume, Access Mode, or Role change—when the proposed complete final Core state violates pack domain/cardinality rules. It produces a stable idempotent administrative rejection with no Transition and is not an ActivityFault. Returning Rejected for archive, suspend, or depart is an ActivityFault because those Core operations are mandatory.

A duplicate or stale timer candidate is discarded by host admission before pack application. Invalid timer/external input or an internally inconsistent mandatory Core Stimulus reaching deterministic pack logic is an ActivityFault; it is never disguised as a participant Action rejection.

The Room Kernel rejects based_on_room_seq mismatch before pack application in the frozen releases.

ActivityFault is an implementation or integrity failure:

- panic;
- invalid next state;
- oversized state/output;
- unknown attention reason;
- invalid timer operation;
- projection failure;
- noncanonical value;
- replay hash mismatch.

For a client action, the server returns activity_fault and reloads or quarantines the room. For a required timer/system stimulus that deterministically faults again, the room becomes faulted. The host never silently skips it.

## Pack lifecycle

### Installation

v0.1 and v0.2 packs are compiled into worldstreamd and registered in a static allowlist.

### Room creation

Room creation:

1. resolves an exact compiled revision digest;
2. validates configuration and constructs initial `CoreRoomState v1` with the Core reducer;
3. calls initialize with immutable initial Core and receives canonical Activity State plus normalized initial timer requests;
4. computes separate Core, Activity, and aggregate hashes and a Genesis hash binding the exact creation inputs and normalized timers;
5. records immutable Genesis and complete Head zero;
6. constructs initial authorized Projections without creating an Observation Frame, Attention Signal, or Activation;
7. commits Genesis, Head, verified current materializations, timers, resource, and creation receipt atomically.

### Upgrade

There is no in-place pack upgrade in the frozen releases. A new pack revision creates a new room. Export/import state migration is future research.

### Public plugins

Not supported. Rust code in the process is trusted. Wasmtime, Component Model ABI, signing, registry, sandbox limits, and third-party renderer isolation may be designed only after both built-ins reveal the real contract.

## Presentation boundary

A pack publishes typed projection schemas and semantic labels. It does not ship executable frontend code.

v0.1 has a first-party Heist renderer plus generic state/timeline panels. v0.2 adds a first-party Investigation renderer. Both use the same authorized protocol.

The project deliberately does not freeze:

- ViewSpec;
- a dashboard builder;
- custom widget ABI;
- pack JavaScript;
- runtime LLM-generated layout;
- a renderer marketplace.

## Reference Activity A: Agent Heist

### Purpose

Heist is a compact deterministic multiplayer game used to prove:

- private participant views;
- structured negotiation;
- one shared sealed-decision window with concurrent submissions serialized by Room order;
- timer-driven state changes;
- conflicting actions;
- targeted agent activation;
- disconnect and catch-up;
- crash recovery;
- deterministic replay.

It is not intended to be a rich game platform.

### Participants, access, and input

| Acting role | Default kind | Private knowledge |
|---|---|---|
| Navigator | Agent | Route hazards and access geometry |
| Insider | Agent | Guard schedule and identity clue |
| Broker | Agent | Tool cost, availability, and extraction constraint |

Additional room access and input sources are not Heist roles:

| Item | Kind | Purpose |
|---|---|---|
| Spectator or operator membership | Human access mode | Public state or authorized local administration |
| Facility | Host stimulus source | Timers and deterministic resolution |

Role cardinality is exactly one for the three active roles in v0.1. A human is technically allowed to occupy a player role for manual testing.

### Hidden fixture

Room seed selects one small immutable facility configuration:

- correct route: canal, service, or roof;
- correct entry window: early, middle, or late;
- required tool: jammer, disguise, or thermal key;
- one extraction constraint.

No participant initially sees all four values. The three role clues are sufficient together.

### Phases

| Phase | Default duration | Meaning |
|---|---:|---|
| Briefing | 30 seconds | Private clues become inspectable |
| Negotiation | 90 seconds | Publish clues, make exchanges, propose plans |
| Commitment | 30 seconds | Each role submits one sealed commitment |
| Resolution | Immediate | Pack resolves recorded commitments |
| Result | 20 seconds | Participants acknowledge or a timer advances the Activity to Complete |
| Complete | Terminal | Final public/reveal projections available |

Durations are fixture configuration, not a general workflow engine.

### Canonical Activity State

State contains:

- phase and phase generation;
- recorded deadlines;
- facility configuration;
- private clue ownership and disclosure state;
- structured offers and accepted exchanges;
- public plan proposals and endorsements;
- each member's sealed commitment;
- resources and contribution decisions;
- outcome and deterministic explanation.

### Typed actions

#### inspect_clue

Marks a role-owned clue as inspected and returns it only in that member's observation.

#### publish_clue

Publishes a selected pre-authored claim code derived from a clue. It does not accept arbitrary hidden reasoning.

#### offer_exchange

Offers one owned clue reference to a named member in exchange for an endorsement or another clue reference.

#### accept_exchange

Accepts a still-valid offer. The reducer updates disclosure audiences atomically.

#### propose_plan

Creates a public structured plan:

    route
    entry_window
    required_tool
    extraction_choice

#### endorse_plan

Publicly endorses one current plan during Negotiation.

#### challenge_plan

Adds a bounded public reason code such as route_conflict, timing_conflict, tool_conflict, or extraction_conflict.

#### commit_move

During Commitment, submits:

    selected_plan_id
    contribute_required_resource
    private_fallback_choice

The payload is visible only to the submitting member and an authorized operator membership until resolution.

#### acknowledge_result

Records that the Participant handled the result. A timer can advance the Activity to Complete if an Invocation is absent.

### Resolution

Resolution is deterministic:

1. Choose the plan with at least two commitments; ties use the earliest valid plan ID.
2. Check route, entry window, tool, and extraction constraint against the hidden fixture.
3. Check required resource contribution.
4. Emit success, partial_failure, or failure with named rule outcomes.
5. Reveal the configured post-game fields.

There is no LLM judge.

### Privacy matrix

| Data | Owner | Addressed member | Other players | Public spectator | Terminal reveal |
|---|---|---|---|---|---|
| Undisclosed role clue | Yes | No | No | No | Yes |
| Accepted clue exchange | Yes | Yes | No | No | Yes |
| Public clue claim | Yes | Yes | Yes | Yes | Yes |
| Structured offer | Yes | Yes | No | No | Yes |
| Proposed plan | Yes | Yes | Yes | Yes | Yes |
| Sealed commitment | Yes | No | No | No | Yes |
| Outcome explanation | Yes | Yes | Yes | Yes | Yes |

Terminal reveal is a pack-defined Projection after the Terminal Phase, not authorization bypass through Replay.

### Attention reasons

- offer_received;
- endorsement_requested;
- commitment_opened;
- required_action_deadline;
- round_result_available.

Only commitment_opened and required_action_deadline are release-critical. Public chatter does not create an activation.

### Heist release gates

1. Three distinct Agent Participants, served by deterministic external Runners, drive the Heist Activity to its Terminal Phase and Outcome through public SDK/protocol calls.
2. Each role receives different private projections.
3. Concurrent sealed submissions are serialized in Room order; stale submissions catch up and retry within the shared deadline.
4. One model invocation is absent when Commitment opens.
5. The pack emits attention for that member; one durable activation is claimed.
6. A fresh invocation catches up and commits before deadline.
7. Kill/restart loses no acknowledged move.
8. Same action retry never mutates twice.
9. Replay reproduces every selected Core, Activity, aggregate, and lineage hash plus the final Outcome.
10. The public UI never receives private clues, offers, or commitments.

## Reference Activity B: Investigation Room

### Purpose

Investigation Room validates that the same runtime supports serious human-agent work without becoming a workflow engine or adding core concepts.

The built-in fictional fixture is Cold Chain Incident. A shipment appears to have exceeded its safe temperature range. Evidence arrives over time, and a later timestamp correction changes how earlier sensor data should be interpreted.

There is no live web ingestion. Fixture inputs are immutable and replayable.

### Participants, access, and input

| Role | Kind in reference demo | Responsibility |
|---|---|---|
| Lead | Human | Assigns/reviews work and submits final brief |
| Timeline analyst | Agent | Establishes ordering and clock consistency |
| Evidence analyst | Agent | Extracts source-linked facts |
| Challenger | Agent | Tests claims and resolves verification requests |
| Evidence feed | Host stimulus | Releases immutable fixture evidence |

The human Lead uses the same action.submit protocol as the agents.

### Evidence fixture

The fixture includes content-addressed:

- shipment manifest and custody timestamps;
- primary temperature-sensor readings;
- refrigeration maintenance record;
- loading-dock witness statement;
- ambient weather record;
- corrected sensor clock-offset notice.

The deterministic answer key concludes that the apparent spike is best explained by a sensor removed for maintenance plus clock offset, not proven product-temperature excursion. The rubric still requires acknowledging the lack of a redundant product probe as uncertainty.

### Phases

- Intake: initial evidence is released and assigned.
- Analysis: participants publish facts and claims.
- Review: required claim challenges and verification are resolved.
- Brief: the Lead can submit once minimum evidence and review criteria are satisfied or the deadline opens.
- Closed: deterministic score and reveal projection are available.

These phases are domain rules inside one state machine. They are not user-authored workflow nodes.

### Canonical Activity State

State contains:

- case objective and phase;
- immutable evidence metadata and version graph;
- visibility and assignment per evidence item;
- bounded private draft records;
- published facts with exact evidence digests;
- claims and revisions;
- support, contradiction, and dependency edges;
- verification requests and dispositions;
- stale reasons caused by superseded evidence;
- participant deadlines and action-relevant domain facts;
- structured final brief;
- deterministic rubric result.

Artifact bytes remain in the generic content-addressed store. Canonical Activity State references digests and declared metadata.

### Typed actions

#### assign_evidence

Lead assigns an available evidence item to a role/member.

#### claim_evidence

An allowed analyst claims an unassigned item through a bounded evidence work reservation.

#### publish_fact

Publishes:

    bounded statement text
    exact evidence digests
    fact code/category
    confidence integer from 0 to 100
    optional time interval

The pack verifies structure and reference authorization, not natural-language truth.

#### flag_source

Records a typed source concern: timestamp, completeness, provenance, internal_conflict, or unsupported_format.

#### propose_claim

Publishes a conclusion candidate referencing current nonwithdrawn facts.

#### support_claim

Adds exact fact IDs and a bounded reason code.

#### challenge_claim

Adds contradictory fact IDs, reason code, and bounded explanatory text.

#### revise_claim

Creates a new immutable claim revision and marks the old revision superseded.

#### request_verification

Lead or analyst sends a named fact or claim to the Challenger with a deadline.

#### resolve_verification

Challenger records verified, rejected, insufficient_evidence, or stale with exact references.

#### submit_brief

Lead submits:

    conclusion_code
    ordered timeline entries with evidence references
    selected claim revisions
    contradictions addressed
    uncertainties
    overall confidence from 0 to 100

### Evidence correction and invalidation

When the corrected clock-offset notice arrives:

1. the host records the evidence-release stimulus;
2. the pack registers a new immutable evidence version and supersedes the old timestamp interpretation;
3. facts citing the superseded item are marked stale;
4. claims depending on those facts are marked stale;
5. verification results depending on them are marked needs_review;
6. domain events list exact invalidated IDs;
7. attention signals target affected Agent Participants and the active agent verifier; the human Lead receives an Observation Frame, while an agent-operated Lead could receive an Attention Signal.

No semantic vector search or LLM decides dependency. Explicit evidence and fact IDs make invalidation deterministic.

### Attention reasons

- evidence_assigned;
- verification_requested;
- cited_evidence_superseded;
- dependent_claim_stale;
- review_deadline;
- final_brief_opened.

The reference demo MUST use cited_evidence_superseded or dependent_claim_stale to start a fresh agent invocation.

### Deterministic scoring

The fixture rubric scores integers for:

- correct conclusion code;
- use of manifest, maintenance, sensor, and correction evidence;
- correct event ordering after applying clock offset;
- explicit treatment of the witness contradiction;
- withdrawal or revision of stale claims;
- acknowledgement of missing redundant measurement;
- required structured brief fields.

Text style is not scored. There is no LLM judge or hidden chain-of-thought requirement.

### Projections

Lead projection:

- published evidence metadata, facts, claims, challenges, reviews, deadlines, and brief requirements;
- no analyst private draft text unless explicitly published.

Analyst projection:

- assigned evidence and authorized artifact references;
- own drafts;
- published case board;
- own stale dependencies and legal actions.

Challenger projection:

- authorized evidence;
- published board;
- assigned verification queue and deadlines.

Public/spectator projection:

- published case board and phase;
- no private evidence assignment or draft;
- final brief and score after closure.

Operator-membership projection:

- operational state and all fixture data for local debugging;
- never model chain-of-thought or provider credentials.

### Investigation generality gate

The activity passes only if:

1. it uses the Activity Pack host interface frozen after Heist;
2. it adds no Investigation-specific wire message;
3. it adds no core domain table beyond generic artifact metadata;
4. it uses existing room ordering, timer, projection, cursor, activation, recovery, and replay semantics;
5. a human and agents submit actions through the same path;
6. a correction deterministically invalidates dependent work;
7. targeted activation supplies only authorized context;
8. the final brief and score replay identically;
9. Heist conformance remains green.

If these fail, the team must revise and retest the abstraction rather than hiding a special case in core.

## Activity conformance suite

Every built-in pack MUST pass:

- manifest/schema consistency;
- initialization determinism;
- golden Genesis/Transition lineage plus Core, Activity, and aggregate hashes;
- repeated apply output equality;
- invalid domain Action rejection plus kernel stale-Action conformance;
- join/resume/Access/Role declared vetoes and mandatory archive/suspend/depart handling;
- atomic multi-Membership final-state Role/cardinality changes with no reflected pack ownership;
- timer retry idempotency;
- maximum-state and output bounds;
- projection schema validation;
- randomized cross-participant privacy/noninterference;
- completed-reveal authorization;
- activation-reason declaration and deduplication;
- crash recovery from an older paired Core-and-Activity snapshot and from Genesis alone;
- replay without external I/O;
- present-plus-historical Replay authorization across suspend/depart/rejoin and Role changes;
- absence of floating-point authoritative values;
- no core changes specific to the pack.

## Future ABI decision

After v0.2, use the two real implementations to decide:

- whether the interface should remain a Rust crate API;
- whether a WebAssembly Component Model boundary is justified;
- which schema/version compatibility rules are real;
- how resource limits and deterministic execution are enforced;
- whether third-party packs are worth the security and support cost.

No marketplace or public upload flow should be built merely because an Activity Pack host interface exists.
