# WorldStream

WorldStream governs a shared reality in which humans and agents participate. In the canonical model, each independent authoritative shared situation is a Room governed by one Activity Pack; “shared reality” is positioning language, not a separate domain entity. This language applies to the whole product, while each Activity Pack may add its own domain language without changing the WorldStream model.

## Rooms and rules

**Room**:
One independent, authoritative shared situation governed by exactly one Activity Pack.
_Avoid_: World, project, workspace, channel, activity instance

**Activity Pack**:
The reusable rule set that defines a class of Rooms, including Roles, Actions, visibility, phases, and Outcomes.
_Avoid_: Plugin, workflow, prompt pack, room

**Activity Pack Revision**:
One immutable semantic identity for an Activity Pack's rules, schemas, codecs, and executor, pinned by each Room for its whole lineage.
_Avoid_: Mutable pack version, installed plugin, build label

**Activity Pack Bundle**:
The immutable installable artifact carrying one exact portable Activity Pack Revision's executor, schemas, static material, and conformance evidence.
_Avoid_: Plugin package, mutable pack directory, Activity Pack Revision when physical bytes are meant

**Activity Phase**:
A domain stage defined by an Activity Pack for the activity unfolding in a Room.
_Avoid_: Room status, activation status, workflow node

**Terminal Phase**:
An Activity Phase after which ordinary domain Actions are no longer permitted.
_Avoid_: Outcome, archived Room, activation completion

**Outcome**:
The Activity Pack-defined final result of a Room's activity. It may be established before the Terminal Phase and does not itself archive the Room.
_Avoid_: Room closure, room status, activation completion

**Room Status**:
The canonical lifecycle value of a Room: active or archived; archived is terminal and independent of Activity Phase and Outcome.
_Avoid_: Activity phase, outcome, room integrity

**Core Room State**:
The WorldStream-owned canonical value containing exactly Room Status and the semantic Membership map.
_Avoid_: Activity state, Room Integrity State, Room Head, delivery state

**Activity State**:
The Activity Pack-owned current domain facts for the activity unfolding in a Room.
_Avoid_: Core room state, Projection, agent-private memory

**Authoritative Room State**:
The accepted current truth of a Room, comprising exactly its Core Room State and Activity State.
_Avoid_: Projection, observation, context, transcript

**Room Integrity State**:
The durable operational assessment of whether a Room's canonical lineage is healthy, faulted, or quarantined. It is outside Authoritative Room State and canonical hashes.
_Avoid_: Room status, Activity phase, canonical health state

**Canonical History**:
The immutable lineage of a Room's creation facts and ordered Transitions.
_Avoid_: Observation stream, UI timeline, snapshot, prompt history

## Product development

**Pack Author**:
A person or organization responsible for defining and maintaining an Activity Pack and its immutable revisions. Pack authorship grants no authority over a WorldStream installation or Room.
_Avoid_: Plugin developer, participant, host operator

**Application Integrator**:
A person or organization that combines Activity Pack Revisions with Activity Client Releases, Runners, protocol bridges, and deployment configuration into an Activity Distribution. Integration grants no Host Operator authority or Room Membership by itself.
_Avoid_: Agent integrator, Pack Author when rule ownership is meant, host operator

**Activity Distribution**:
An Application Integrator-owned, versioned integration manifest that proposes separately identified Activity Pack Bundles, Activity Client Releases, optional Runner integrations, documentation, and deployment templates without merging their authority or execution boundaries. A template may name required capabilities and secrets but contains no secret value, approval, Client Deployment, or Client Binding Store state, and importing or publishing it grants no operational approval.
_Avoid_: Activity Pack, Activity Pack Bundle, plugin package, executable Pack

**Room Setup Specification**:
A reusable, non-authoritative description of one intended Room's exact Activity Pack Revision, configuration, seats, and execution requirements. It grants no approval and does not define replacement state for an existing Room.
_Avoid_: Activity Distribution, Room, desired Room state, server configuration, setup operation

**Room Setup Operation**:
One Host-local attempt to create and provision a Room from a fixed, resolved Room Setup Specification. Its operational progress can continue across retries but is not an Activity Phase or the Room's Canonical History.
_Avoid_: Room Setup Specification, Room, workflow, canonical Transition

**Activity Client**:
An independently executing browser, terminal, mobile, service, or agent-owned application that participates through one scoped WorldStream client contract and may present one or more exact Activity Pack Revisions. It may be presentation-rich and Pack-aware, but owns no Room authority, legality, or canonical state.
_Avoid_: Activity Pack, Pack executor, Studio plugin, Client SDK, Participant when the application itself is meant

**Activity Client Release**:
One immutable, content-addressed published build whose release manifest identifies its runnable payloads, Client Surfaces, and supported client-contract versions. Compatibility claims and linked evidence grant no Host approval.
_Avoid_: Activity Client source tree, Client Deployment, Client SDK package, mutable release tag

**Client Deployment**:
One Host Operator-approved operational availability of an Activity Client Release at an exact independently executing launch target, with an explicit Deployment Trust Level.
_Avoid_: Activity Client Release, Client Binding, Studio plugin, Room session

**Client Deployer**:
An optional execution-boundary component that retrieves approved Activity Client Releases, starts or publishes them through a deployment adapter, and reports exact deployment identity and readiness. It is not part of the Room Kernel or Studio.
_Avoid_: Runner, Activity Client, Client Binding Store, Studio plugin, Pack executor

**Deployment Trust Level**:
The Host's operational classification of a Client Deployment as verified from an exact Activity Client Release digest or externally trusted without proof of its running bytes.
_Avoid_: Room authority, client compatibility, publisher reputation, Membership permission

**Client SDK**:
A non-authoritative development library for implementing the generic WorldStream client contract. It is not an Activity Client until an independently executing application uses it.
_Avoid_: Activity Client, Pack executor, Runner

**Client Surface**:
One human-facing entry point exposed by an Activity Client Release, such as a participant browser screen, spectator display, or terminal interface. One Release may expose several Client Surfaces.
_Avoid_: Projection, Pack view, React component imported by Studio

**Client Binding**:
One Host Operator-approved operational association from an exact Activity Pack Revision, client-contract version, Access Mode, and applicable participant Role set to one Host-brokered Client Surface of an approved Client Deployment. Directly connecting clients do not require one. It is Host configuration, not Room state or Pack authority.
_Avoid_: Client implementation, Activity Distribution, Client Deployment, launch token, Pack-owned route

**Client Binding Store**:
The Host-owned durable operational collection of approved Activity Client Releases, Client Deployments, and Client Bindings available to one WorldStream installation. Its records and approvals do not move with Room backup or transfer.
_Avoid_: Artifact registry, client marketplace, Pack catalog, Activity Distribution, renderer registry

**Client Selection**:
The Host's operational resolution of one current Membership and the Client Binding Store to one approved Client Surface for one handoff. It never changes Room or Membership state.
_Avoid_: Client Binding, Role assignment, Room routing, canonical decision

**Artifact Registry**:
An external distribution service for publishing and retrieving immutable, content-addressed artifacts. Availability in an Artifact Registry grants no Host approval.
_Avoid_: Client Binding Store, Pack catalog, client marketplace, Host approval

**Client Catalog**:
An external discovery service that indexes published Activity Distributions and Activity Client Releases without granting Host approval or executing them.
_Avoid_: Client Binding Store, Artifact Registry, Studio catalog, marketplace authority

**Hosted Activity Platform**:
A separately branded application layer built on one operated WorldStream installation. It may own public discovery, Platform Accounts, authoritative platform-only pre-Genesis coordination, bounded launch orchestration, and derived result indexes and may broker Host-issued opaque client handoffs, but it is not the Room Runtime and owns no Room authority.
_Avoid_: WorldStream hosted mode, Room Runtime, game-aware kernel, Client Catalog

**Hosted Gateway**:
A narrow application adapter through which one Hosted Activity Platform and its admitted clients reach an operated WorldStream installation without exposing generic Host or operator authority. It owns no Room state, Membership, legality, or Replay truth.
_Avoid_: Public Runtime, generic proxy, Studio backend, Room authority

**Hosted Deployment Revision**:
One immutable correspondence among the source revision, deployed platform and Runtime artifacts, platform schema head, and exact Listing, Pack, Client, and projector identities of a coordinated hosted release. It is not a WorldStream release claim and does not alter any Room lineage.
_Avoid_: Activity Listing Revision, Activity Distribution, mutable environment, WorldStream release

**Hosted Recovery Checkpoint**:
One verified recovery set preserving mutually coherent WorldStream and Hosted Activity Platform state for one Hosted Deployment Revision. It is the only unit from which the hosted preview may claim continuity after a restore.
_Avoid_: Provider snapshot, independent database backup, WorldStream backup, Hosted Deployment Revision

**Platform Operator**:
The person or organization responsible for a Hosted Activity Platform's catalog, admission policy, House Agent definitions, and provider spending. This role grants no WorldStream installation authority; the initial project host separately holds both Platform Operator and Host Operator roles.
_Avoid_: Host Operator, Operator Membership, creator, generic operator

**Platform Account**:
A stable pseudonymous identity recognized by a Hosted Activity Platform and linked separately to a mutable external authentication identity and optional public profile. It grants no WorldStream authority and is not a Principal. Removing the authentication link removes login and profile data but leaves retained shared history linkable through the pseudonymous account tombstone.
_Avoid_: Principal, Membership, Host Operator, shared agent identity

**Platform Coordination State**:
Hosted Activity Platform-owned facts governing pre-Genesis formation, including Launch Requests, Seat Claims, Seat Invitations, quota reservations, and immutable Host-operation correspondence. It may be authoritative for platform coordination but never for Room, Membership, Activity Phase, Outcome, or Replay truth.
_Avoid_: Core Room State, Activity State, Room Setup Operation, derived Room truth

**Platform Capacity Reservation**:
An authoritative Hosted Activity Platform coordination record that reserves either one account's pre-Genesis launch slot or one account and the deployment's active-Run capacity. It never describes Room Status or Activity Phase, and ambiguous Host state retains rather than reallocates it.
_Avoid_: Room capacity, Activity Run status, billing quota, Host lease

**Activity Listing**:
A stable Hosted Activity Platform catalog identity describing one reviewed, launchable experience through immutable Activity Listing Revisions. Catalog review and listing never grant Host approval or Room authority.
_Avoid_: Activity Pack, Activity Distribution, Client Catalog entry, mutable launch configuration, Host approval

**Activity Listing Revision**:
One immutable, content-identified revision of an Activity Listing. It pins the exact Activity Pack Revision, required Activity Client Release and Client Surface, reviewed launch-input contract, catalog visibility, fixed seat and creator-access policy, public-viewing policy, Result Publication Policy, and Result Projector Revision; it may reference an exact Activity Distribution as provenance without granting approval.
_Avoid_: Mutable catalog row, Room Setup Specification, Activity Distribution, Host allowlist

**Result Publication Policy**:
The immutable part of an Activity Listing Revision that determines whether a projected result is public and fixes its accepted public schemas, pseudonymous attribution, and suppression behavior. The policy never defines or replaces an Outcome.
_Avoid_: Outcome, mutable site setting, leaderboard rule, client result

**Result Projector Revision**:
One immutable, content-identified deterministic platform-code revision that interprets an exact authorized Public Projection for a compatible Activity Pack Revision, determines its result disposition, and may produce one schema-valid bounded summary. It owns no Outcome and adds no Activity Pack operation.
_Avoid_: Activity Pack operation, Activity Client result, generic score extractor, LLM judge

**Launch Request**:
A Hosted Activity Platform's idempotent pre-Genesis request to start one exact Activity Listing Revision with schema-allowed inputs. It gathers permitted Seat Claims and House Agent Assignments for fixed seats, freezes one roster and Room Setup Specification, and maps to at most one Host-local Room Setup Operation; only a terminal failure proven to precede Genesis has no Activity Run, while ambiguous state reconciles the same operation and any observed Genesis creates exactly one Run.
_Avoid_: Room Setup Operation, Activity Run, Room, generic remote administration

**Seat Claim**:
A pre-Genesis correspondence between one authenticated Platform Account and one exact seat plus a server-derived reference for a new run-scoped Principal. It cannot change the seat's Role or policy and grants no Membership or Room authority.
_Avoid_: Membership, Role selection, participant session, seat reservation after Genesis

**Seat Invitation**:
An opaque, revocable pre-Genesis capability to claim one exact unclaimed seat under its Activity Listing Revision's policy. It grants no Membership, Role choice, or Host authority.
_Avoid_: Membership credential, client handoff, open join code, Role selector

**Activity Run**:
A Hosted Activity Platform record created only after one Room's Genesis and permanently referencing exactly that one authoritative Room and its originating Activity Listing Revision. It may carry platform discovery, admission, public-viewing, and indexing references, but owns no Room state, Membership, Activity Phase, Action legality, or Outcome.
_Avoid_: Room, activity instance, Launch Request, parallel activity state machine

**Run Membership Correspondence**:
A private, immutable post-Genesis platform record that relates one Activity Run admission purpose to the exact Principal and Membership created by the Host. A participant correspondence also carries its seat, Role, and controlling Platform Account or House Agent Assignment; a spectator correspondence may instead represent the creator or a platform service. It supports scoped re-entry or service recovery but grants no Room authority and cannot replace the Membership.
_Avoid_: Seat Claim, Membership, public participant profile, replaceable seat, Room authority

**Run Entry Selector**:
A random, opaque, non-secret reference that distinguishes one account-controlled Run Membership Correspondence within one Activity Run. It authorizes nothing without the matching Platform Account and never exposes or replaces the Principal, Membership, or Host-issued client handoff.
_Avoid_: Membership credential, Seat Claim, Principal ID, Membership ID, reusable client handoff

**Run Terminal Evidence**:
An immutable, non-authoritative Hosted Activity Platform record that one Activity Run's pinned Result Projector Revision classified its authorized Public Projection at one Complete Head as terminal. It justifies platform capacity release but does not copy or replace Activity Phase, Outcome, or Room Status.
_Avoid_: Terminal Phase, Outcome, Indexed Activity Result, archived Room

**Indexed Activity Result**:
A rebuildable but immutable, non-authoritative internal platform record produced under one Activity Listing Revision's Result Publication Policy after its Result Projector Revision yields a final summary and healthy integrity and Replay verification agree through one Complete Head. It retains exact Room, Activity Pack Revision, projector, Public Projection, integrity, Replay, and Head provenance. A later Head yielding the same summary confirms it; a divergent summary conflicts; public suppression never rewrites it. It never replaces the Outcome or Replay evidence.
_Avoid_: Outcome, mutable result, universal score, client-submitted result, leaderboard authority

**Published Activity Result**:
The privacy-reviewed public representation of an Indexed Activity Result. It uses reviewed pseudonymous seat labels by default, may join a current opted-in public profile, exposes no Host or platform-store identifiers, and may be suppressed without deleting immutable internal result evidence.
_Avoid_: Outcome, raw Indexed Activity Result row, Public Projection, leaderboard entry

**Result Indexer**:
A platform service that uses a dedicated scoped Spectator Membership to obtain one Activity Run's authorized Public Projection and apply its pinned Result Projector Revision, including Replay-verification evidence when publication is enabled. It is distinct from the Public Projection Relay and holds no participant authority.
_Avoid_: Public Projection Relay, Host Operator, anonymous viewer, Pack operation

**Public Projection Relay**:
A platform-controlled Activity Client authorized through a scoped Spectator Membership to republish only one Room's Public Projection to anonymous viewers when its Activity Listing Revision and Host policy explicitly permit that access. It creates no anonymous WorldStream access and holds no participant authority.
_Avoid_: Public Room, shared participant credential, Projection authority, generic Pack interpreter, Result Indexer

**Browser Activity Session**:
An opaque Host-brokered continuation that binds one browser Activity Client to one current Membership and Client Selection without giving the browser the Membership's reusable authority credential. It is operational session state, not a Platform Account session, Membership, or part of Room state.
_Avoid_: Platform session, Membership credential, Room session, browser bearer

**Stream Admission Ticket**:
A short-lived, one-use grant derived from a Browser Activity Session that admits one direct browser connection to the session's exact WorldStream stream and approved Client Deployment origin. It selects no Room, Membership, Role, or client and is not reusable Room authority.
_Avoid_: Membership bearer, Browser Activity Session, WebSocket URL, routing token

**Client Conformance Evidence**:
Immutable, content-addressed evidence produced by a client conformance kit for one exact Activity Client Release and its claimed contract and Pack compatibility. Passing conformance never grants Host approval.
_Avoid_: Client Binding, security audit, Host approval, Pack conformance evidence

## Participation

**Principal**:
A durable identity recognized by WorldStream, representing either a human or an agent.
_Avoid_: Participant, membership, session, runner

**Membership**:
One Principal's durable room-local seat, carrying an immutable Principal binding and kind, its Membership Standing, Access Mode, and any pack-defined Role.
_Avoid_: Account, session, connection, invocation

**Membership Standing**:
The canonical lifecycle value of a Membership: enabled, suspended, or departed; enabled and suspended are reversible, while departed is terminal.
_Avoid_: Session status, connection status, runner availability

**Room Member**:
A Principal considered in one Room through a Membership, whether acting, spectating, or operating.
_Avoid_: Participant when the Membership may be read-only, member record

**Access Mode**:
The broad posture of a Membership: participant, spectator, or operator. It is separate from Principal kind and pack-defined Role.
_Avoid_: Role, Principal kind, permission scope

**Participant**:
A human or agent Room Member whose Membership has participant access and a pack-defined Role. An enabled Participant may submit domain Actions.
_Avoid_: Every room member, spectator, operator, player outside a game pack

**Human Participant**:
A Participant whose Principal represents a human.
_Avoid_: Human spectator, host operator

**Agent Participant**:
A Participant whose Principal represents a machine-operated policy. It is a durable logical participant, not an executing process.
_Avoid_: Runner, invocation, model, resident agent

**House Agent**:
An Agent Participant supplied by a Hosted Activity Platform to fill an eligible seat when the Activity Listing Revision and creator's launch choice permit it. It remains distinct from its external Runner, provider model, and the creator's Principal and receives no broader platform or Host authority.
_Avoid_: Default bot, model, Runner, creator's agent identity, fallback human

**House Agent Revision**:
An immutable Platform Operator-published definition that pins one House Agent's model route, behavior policy, exact Host-approved Agent Profile and Runner Template revisions, permitted tools, accounting tokenizer, and execution allowance. An Activity Listing Revision may allowlist it, but selection creates a new House Agent for one launch lineage rather than a reusable Principal.
_Avoid_: User Agent profile, model alias, mutable provider configuration, Agent Principal

**House Agent Assignment**:
An immutable, Launch Request-scoped Hosted Activity Platform record created only after its retained Runner-capacity reservation succeeds. It binds one eligible seat and server-derived setup-local Principal reference to one exact House Agent Revision, provider slug, model, Runner identity, reservation receipt, and execution allowance before Genesis. Observed Genesis carries it into the corresponding Activity Run; later terminal pre-Genesis failure retires it with the Launch Request. It is platform coordination evidence, not Activity State or reusable agent identity.
_Avoid_: House Agent Revision, Seat Claim, Membership, mutable bot slot, model fallback

**House Fill Operation**:
A durable, Launch Request-scoped Hosted Activity Platform operation that retains one final claim window, one eligible candidate and exclusion set, one random draw, and the exact House Agent and Runner-capacity reservation intents derived from it. Retry resumes the same draw and never selects replacements.
_Avoid_: House Agent Assignment, matchmaking, reroll, reusable agent pool

**Role**:
An Activity Pack-defined responsibility held by a Participant through its Membership.
_Avoid_: Access mode, Principal kind, permission scope

**Spectator Membership**:
A read-only Membership that receives an authorized public view of a Room.
_Avoid_: Participant, anonymous public stream

**Operator Membership**:
A read-only Membership with scoped room-level administrative or diagnostic visibility.
_Avoid_: Host operator, participant role

**Host Operator**:
The person or organization with authority over a WorldStream installation. That authority does not itself create Membership in any Room.
_Avoid_: Operator membership, participant, activity role

## State change and causality

**Action**:
A typed proposal from a Participant to affect a Room.
_Avoid_: Command, event, direct state update

**Action Offer**:
The Activity Pack's canonical typed representation of an Action a Participant may submit from one exact authorized Room view.
_Avoid_: Duplicate legality list, affordance as a formal term, guaranteed action, command

**Stimulus**:
A fully recorded candidate for advancing a Room, derived from an Action, timer firing, membership change, or host input. Only an accepted Stimulus produces a Transition.
_Avoid_: Event, protocol message, accepted transition

**Genesis**:
The immutable sequence-zero creation record from which a Room's initial Core and Activity State are reconstructed.
_Avoid_: Initial snapshot, first transition

**Semantic Time**:
A recorded, stimulus-specific time value whose meaning is part of Room rules and Replay. Actions use Admitted Time, timer firings use Scheduled Time, and other stimuli use their declared Recorded Time; there is no universal Transition timestamp.
_Avoid_: Logical time when it implies one clock for every Transition, database time, commit time

**Admitted Time**:
The host-recorded Semantic Time at which a validated Action acquires a place in the bounded Room Admission Lane. It determines deadline eligibility but does not guarantee that the Action will commit.
_Avoid_: Client time, request arrival time, dequeue time, commit time

**Scheduled Time**:
The immutable Semantic Time assigned to one Timer Generation and reused as that timer firing's effective time.
_Avoid_: Fired time, scan time, dequeue time

**Timer Generation**:
A host-owned monotonic fence identifying one immutable schedule of a Room timer.
_Avoid_: Pack-assigned generation, retry count, scheduler claim

**Host Stimulus Source**:
A non-participant source of recorded Room input, such as a timer facility or controlled evidence feed.
_Avoid_: System actor, participant, agent, runner

**Transition**:
One authoritative ordered step resulting from exactly one accepted Stimulus.
_Avoid_: Action, event, observation, message

**Domain Event**:
A semantic fact describing a consequence within a Transition.
_Avoid_: Transition, protocol event, observation frame

## Visibility and continuity

**Projection**:
A full current view of Authoritative Room State containing only what one Membership is allowed to know.
_Avoid_: Room state, observation, stream, redacted state dump

**Public Projection**:
A Projection safe for Spectator Memberships and other authorized public viewers. Public does not imply anonymous.
_Avoid_: Public stream, raw room state

**Participant Projection**:
A Projection for one acting Participant, including that Participant's current knowledge and Action Offers.
_Avoid_: Member projection when access mode matters, authoritative state

**Observation Frame**:
One durable Membership-addressed unit coalescing the authorized consequences of one accepted Transition. Genesis and a Transition hidden from that Membership produce no frame.
_Avoid_: Event, message, projection, state dump

**Observation Stream**:
The ordered progression of Observation Frames belonging to one Membership.
_Avoid_: Public stream, room event stream, model-token stream

**Cursor**:
One Membership's acknowledged position in its Observation Stream.
_Avoid_: Room version, model progress, replay position

**Projection Reset**:
A complete authorized Projection that atomically establishes a new Observation Stream baseline on first attach, when the retained range cannot cover the Cursor, or when visibility loss or another condition makes incremental delivery inappropriate.
_Avoid_: Observation Frame, silent skip, Replay

**Catch-up**:
Delivery of retained authorized Observation Frames after a Membership's Cursor.
_Avoid_: Replay, cognitive resume, process resume

**Replay**:
Read-only reconstruction of a Room's Canonical History under the original Activity Pack rules.
_Avoid_: Catch-up, resume, fork, new Room

## Agent attention and execution

**Runner**:
An external execution host authorized to start Invocations for Agent Participants. Runner authority alone never authorizes a domain Action.
_Avoid_: Agent, participant, worker, model

**Invocation**:
One bounded occurrence of an agent policy executing for an Agent Participant. The Agent Participant and its Membership outlive every Invocation.
_Avoid_: Agent, session, persistent process, resumed mind

**Attention Signal**:
An Activity Pack determination that an Agent Participant may need to act for a stated reason.
_Avoid_: Activation intent, model call, wake-up

**Activation Intent**:
A durable WorldStream request for an authorized Runner to consider starting a fresh Invocation for an Agent Participant.
_Avoid_: Invocation, task, proof of execution, wake-up job

**Invocation Context**:
The bounded authorized information made available to one Invocation from its Room and Membership.
_Avoid_: Context layer, room memory, agent-private memory, raw history

**Agent-Private Memory**:
Information retained by an external agent implementation outside WorldStream.
_Avoid_: Authoritative room state, invocation context, persistent WorldStream memory

**Logical Agent Persistence**:
Continuity of an Agent Participant's Principal, Membership, observations, and Room facts across Invocations.
_Avoid_: Continuous computation, sleeping agent, persistent mind, restored hidden state

## Referenced material

**Artifact**:
Immutable external material that a Room may reference under its Activity Pack's rules.
_Avoid_: Evidence version, mutable document, observation, authoritative state
