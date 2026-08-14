# Product Vision

## Current product thesis

WorldStream is a self-hosted realtime room runtime for multi-agent applications. Its precise model is multi-participant: humans and independently run agents join one authoritative room, receive scoped realtime observations, submit typed actions, and retain continuity across connections and ephemeral agent invocations.

The architectural analogy is a multiplayer game backend generalized for AI applications:

- an application developer supplies the rules;
- WorldStream maintains the shared reality;
- humans and agents are players under those rules;
- each player can see a different authorized view;
- clients may disconnect and later catch up;
- agent models run elsewhere and only when their runner invokes them.

This is a promising infrastructure thesis, not yet a validated product category. The purpose of Agent Heist and Investigation Room is to falsify or validate the thesis with working software.

## The problem being solved

AI application developers repeatedly have to assemble the same coordination layer when several agents or humans affect one situation:

- one authoritative current state;
- deterministic handling of racing or conflicting actions;
- public and private information;
- role-based legal actions;
- live updates without constant polling;
- safe retry when a client or server fails;
- a compact catch-up after an agent invocation has ended;
- evidence of what changed and why;
- reproducible history for debugging.

A WebSocket library transports frames. A message broker orders messages. An agent SDK invokes models and tools. A workflow engine follows a designed process. None of them alone defines a shared, rule-governed environment with participant-specific perception.

WorldStream owns that missing application runtime.

## Why realtime and why Stream

The product does not primarily stream model tokens. Its stream consists of meaningful changes to a shared situation:

- a private clue becomes available;
- a participant publishes a claim;
- two simultaneous actions conflict;
- a decision window opens;
- evidence is corrected;
- a dependent conclusion becomes stale;
- a timer closes a phase;
- an agent runner is asked to start a fresh invocation.

The server always commits before publishing:

    participant action or recorded stimulus
        → pack validation and deterministic reduction
        → durable ordered transition
        → Authoritative Room State update
        → authorized observation frames
        → optional targeted activation intent
        → subsequent participant actions

Realtime matters because the next permitted action can change immediately when another participant acts or a timer fires. Durability matters because a network connection and an LLM invocation are temporary. A participant that was absent receives an exact authorized catch-up from its cursor instead of an unbounded room transcript.

World is the room's current authoritative reality. Stream is how changes to that reality reach participants over time.

## Who uses it

### AI application developer

This is the primary adopter. The developer is building a game, simulation, investigation, negotiation, review room, or another application where independent human and agent participants affect shared state.

Their job is to define domain rules and participant behavior without rebuilding ordering, reconnect, private projections, durable activation, recovery, and replay.

### Activity developer

The activity developer writes a trusted Activity Pack. In the first releases this is Rust code compiled with the server. The developer defines:

- state and configuration;
- participant roles;
- typed actions and rejection rules;
- deterministic transitions and timers;
- public and private projections;
- attention reasons and completion criteria.

The pack is headless. It must remain useful through the protocol even if the reference UI is absent.

### Human participant

Humans can be first-class acting Participants, not merely observers. A human Participant takes a pack-defined Role and submits the same typed Actions as an Agent Participant; a human may instead join as a read-only spectator or operator Room Member.

Investigation Room deliberately requires an acting human lead so this is proven rather than promised.

### Agent developer or owner

The agent owner runs a separate runner using the Python SDK or raw protocol. The runner owns the model, prompts, tools, credentials, private memory, and invocation lifecycle.

WorldStream may offer a durable Activation Intent. An authorized Runner may claim it and start a fresh Invocation with bounded Invocation Context. If that Invocation proposes domain Actions, it submits them through separate participant authority bound to the Agent Participant's Principal and Membership. WorldStream does not claim that a model remained awake, asleep, or continuously alive.

## Product model

The complete frozen hierarchy is intentionally small:

    WorldStream server
    └── rooms
        └── exactly one Activity Pack revision per room

A deployment can host many unrelated rooms. Each room is independently ordered and recovered. There is no Project or Workspace entity, no cross-room memory, and no multi-pack composition in v0.1 or v0.2.

Five concepts carry most of the product:

1. Room — the authoritative shared state machine.
2. Membership — a principal's durable, role-bearing relationship to one room.
3. Projection — the authorized public or participant view.
4. Action — the typed request that may change the room.
5. Cursor and activation — continuity across temporary connections and invocations.

Transitions, snapshots, and replay are the correctness machinery beneath those concepts.

## Agent lifecycle without the metaphor

An AI agent is normally ephemeral:

1. Its durable membership already exists in a room.
2. A pack transition creates an attention signal for that membership.
3. WorldStream persists an activation intent.
4. An external runner claims the intent with a lease.
5. The runner starts a bounded invocation.
6. The invocation receives an authorized current projection, relevant frames after its cursor, legal actions, deadlines, and explicit artifact references.
7. It submits actions and exits.
8. The room and membership remain; the model process does not.

If no runner is connected or polling, nothing executes. The activation stays pending until it expires or is cancelled. This honesty is part of the architecture.

## Context boundary

WorldStream is not a generic context or memory service.

It remembers the shared room:

- current structured state;
- exact accepted transition history;
- membership and cursors;
- authorized observation frames;
- explicit evidence or artifact references;
- activation decisions.

It does not own:

- a model's private memory;
- an ever-growing prompt transcript;
- private reasoning;
- arbitrary documents converted into embeddings;
- automatic semantic summaries in the frozen releases.

Context for an invocation is a temporary, authorized projection of the room. Current truth and legal actions come first; recent relevant frames and explicit evidence references follow. Raw room internals are never injected wholesale.

## When to choose WorldStream

Choose it when:

- several humans or agents share one changing situation;
- views differ by participant;
- participant policies choose among legal actions;
- actions can conflict or change other participants' legal actions;
- live updates plus later catch-up are both needed;
- exact recovery and replay are useful.

Do not choose it when:

- one agent is completing one task;
- the process is a predetermined workflow;
- the only requirement is token streaming or pub/sub;
- the real product is search over documents;
- the application needs WorldStream to host models or tools.

## Boundaries against adjacent products

### Not n8n or Temporal

n8n and Temporal coordinate a process whose control flow is substantially designed in advance. WorldStream coordinates participants inside shared state. The pack defines legal actions and consequences, but a participant policy decides what to do next.

If the desired UI is primarily nodes and connectors, WorldStream is the wrong tool.

### Not NATS, Kafka, or Lightstreamer

Those systems are transport or messaging infrastructure. WorldStream may eventually use or integrate with such systems, but its contribution is above transport: Roles, Authoritative Room State, typed Actions, visibility, Legal Actions, Activation, Recovery, and Replay.

### Not a coding harness

WorldStream does not execute repositories, tools, shells, or model loops. Coding can be one future application, but it is not the identity or initial reference use case.

### Not an agent social network or marketplace

Unstructured agent chatter, listings, bidding, reputation, payments, and crypto do not establish the room semantics. They are excluded from both frozen releases.

## Reference application 1: Agent Heist

Agent Heist proves the difficult multiplayer properties in a visually understandable sandbox:

- three external agents in different roles;
- private clues and structured private offers;
- public plans;
- a simultaneous sealed decision window;
- timers and deterministic conflict resolution;
- one terminated invocation followed by targeted activation and fresh catch-up;
- one forced server restart;
- public and participant-specific replay.

The map and game content stay deliberately small. Its job is to make the runtime behavior obvious.

## Reference application 2: Investigation Room

Investigation Room proves the same Room Kernel supports serious work. In a fictional cold-chain incident:

- a human lead and three agents share one room;
- evidence arrives in recorded waves;
- participants publish source-linked facts and claims;
- a challenger verifies or disputes them;
- a later timestamp correction supersedes evidence;
- dependent claims become stale;
- affected agents receive activation intents;
- the lead submits a structured final brief;
- a deterministic rubric scores the outcome;
- restart and replay reconstruct the entire case.

This is not a general project manager or enterprise workflow. It is one rule-governed shared situation, implemented as one Activity Pack, using the same Room Kernel as Heist.

## What success looks like

The technical thesis is supported if:

- Heist demonstrates ordering, privacy, activation, reconnect, crash recovery, and replay;
- Investigation can be added without a new Room Kernel lifecycle, protocol special case, or persistence model;
- an external developer can connect a runner without writing WebSocket recovery logic;
- an activity author can define a small third activity after v0.2 without changing core;
- the resulting demo is visibly more than a WebSocket plus database plus webhook.

The thesis should be reconsidered if:

- most activity work requires editing the server;
- the second activity needs workflow-engine or generic project-management features;
- participant-specific projections and activation add little beyond ordinary pub/sub;
- another developer cannot understand or use the five core concepts in one day;
- the Activity Pack tax is so large that the runtime removes little reusable work.

## Frozen product boundaries

Until both reference releases pass, the project will not add:

- cross-room projects or exchange;
- generic task boards or business process automation;
- cloud agent execution or coding sandboxes;
- generic context retrieval, embeddings, or semantic memory;
- pack, agent, or labor marketplaces;
- real payments, crypto, wallets, or tokens;
- third-party plugin upload or a WASM host;
- generated UI or third-party renderer code;
- clustering, federation, or multi-region writes.

The detailed normative list is in [Frozen Requirements](requirements.md).
