# WorldStream

WorldStream models independent Rooms in which human and agent Participants act under one Activity Pack and receive only authorized views. This language applies to the whole product; each Activity Pack may add its own domain language without changing the WorldStream model.

## Rooms and rules

**Room**:
One independent, authoritative shared situation governed by exactly one Activity Pack.
_Avoid_: World, project, workspace, channel, activity instance

**Activity Pack**:
The reusable rule set that defines a class of Rooms, including Roles, Actions, visibility, phases, and Outcomes.
_Avoid_: Plugin, workflow, prompt pack, room

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

**Legal Action**:
An Action currently permitted for a Participant by its Role and the Room's current state.
_Avoid_: Affordance as a formal term, guaranteed action, command

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
A Projection for one acting Participant, including that Participant's current knowledge and Legal Actions.
_Avoid_: Member projection when access mode matters, authoritative state

**Observation Frame**:
One durable Membership-addressed unit coalescing the authorized consequences of one accepted Transition. Genesis and a Transition hidden from that Membership produce no frame.
_Avoid_: Event, message, projection, state dump

**Projection Reset**:
A full authorized Projection that establishes a new delivery baseline when retained Observation Frames cannot cover a Membership's Cursor.
_Avoid_: Observation frame, replay, silent cursor advance

**Observation Stream**:
The ordered progression of Observation Frames belonging to one Membership.
_Avoid_: Public stream, room event stream, model-token stream

**Cursor**:
One Membership's acknowledged position in its Observation Stream.
_Avoid_: Room version, model progress, replay position

**Projection Reset**:
A complete authorized Projection that atomically establishes a new Observation Stream baseline when incremental catch-up is unavailable or inappropriate.
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
A durable five-state WorldStream request for an authorized Runner to consider starting a fresh Invocation for an Agent Participant.
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
