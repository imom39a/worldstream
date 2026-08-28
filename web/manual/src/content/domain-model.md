# Domain model

Precise language is part of the design. Use these terms in code, tests, docs,
and Activity Pack APIs.

## Room and rules

- **Room:** one independent authoritative shared situation governed by exactly
  one Activity Pack.
- **Activity Pack:** reusable deterministic rules defining Roles, Actions,
  visibility, phases, attention, timers, and Outcomes.
- **Activity Pack Revision:** immutable semantic identity pinned for a Room's
  complete lineage.
- **Activity Phase:** pack-defined domain stage. It is not Room lifecycle.
- **Outcome:** pack-defined final result. It does not archive the Room.
- **Room Status:** WorldStream-owned `active` or `archived` lifecycle.

## People, agents, and seats

- **Principal:** durable identity representing a human or agent.
- **Membership:** one Principal's durable Room-local seat.
- **Participant:** enabled human or agent Membership with participant access and
  a pack-defined Role.
- **Spectator Membership:** authorized read-only public view.
- **Operator Membership:** scoped read-only diagnostic/administrative view.
- **Role:** pack-defined responsibility; never use it as a synonym for access
  mode or authority scope.

## Change and causality

- **Action:** typed proposal from a Participant.
- **Action Offer:** exact current typed Action the pack permits from one
  authorized view.
- **Stimulus:** fully recorded candidate derived from Action, timer,
  Membership change, or controlled host input.
- **Transition:** one ordered authoritative step produced by an accepted
  Stimulus.
- **Domain Event:** semantic consequence inside a Transition.
- **Semantic Receipt:** durable stable disposition for one operation identity.

## Visibility and continuity

- **Projection:** complete current Membership-authorized view.
- **Observation Frame:** durable Membership-addressed consequence of one
  Transition.
- **Observation Stream:** ordered frames for one Membership.
- **Cursor:** durable acknowledged position in that stream.
- **Catch-up:** delivery after a Cursor. It is not Replay.
- **Replay:** read-only reconstruction of Canonical History.

## Agent execution

- **Agent Participant:** durable logical machine-operated Participant.
- **Runner:** external execution host authorized to start Invocations.
- **Invocation:** one bounded occurrence of agent policy execution.
- **Attention Signal:** pack determination that an Agent Participant may need to
  act.
- **Activation Intent:** durable request for a Runner to consider a fresh
  Invocation.
- **Invocation Context:** bounded authorized information for one Invocation.
- **Agent-Private Memory:** information held outside WorldStream by the agent
  implementation.

## Common naming mistakes

| Avoid | Use instead |
| --- | --- |
| “agent” for a running process | Runner or Invocation |
| “event” for an accepted state step | Transition |
| “room state” for a viewer response | Projection |
| “replay” for reconnect delivery | Catch-up |
| “public” meaning unauthenticated | Public Projection for an authorized viewer |
| “plugin” for v0.1 rules | trusted Activity Pack revision |
| “room status” for a game phase | Activity Phase |

Primary source: the complete [WorldStream domain context](https://github.com/imom39a/worldstream/blob/main/CONTEXT.md)
and [glossary](https://github.com/imom39a/worldstream/blob/main/docs/glossary.md).
