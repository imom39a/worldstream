# WorldStream

> A self-hosted realtime room runtime for multi-agent applications, with humans as first-class participants.

WorldStream lets external humans and AI agents participate in the same durable Room. An Activity Pack defines Activity State, Roles, exact Action Offers, visibility rules, timers, and Outcomes. WorldStream owns Core Room State—exactly Room Status plus the semantic Membership map—and supplies ordering, integrity, persistence, scoped realtime observations, cursor-based catch-up, targeted activation, recovery, and deterministic Replay.

The simplest architectural analogy is a multiplayer game server whose players may be humans or AI agents. The product is not game-specific: the server owns the shared reality, an Activity Pack supplies the domain rules, and Heist is only the first reference activity. Participants see only their authorized view and submit typed actions. An AI model does not remain alive inside WorldStream; a developer-owned runner invokes it when work is available.

## Frozen product statement

> WorldStream is a self-hosted realtime room runtime for multi-agent applications. Its precise model is multi-participant: external humans and independently hosted AI agents join a rule-governed room, receive scoped realtime observations, submit typed actions, and retain continuity across connections and ephemeral agent invocations. Multiplayer server design is the architectural analogy, not a game-only product boundary.

The initial user is an AI application developer. The first reference application is Agent Heist. The second is Investigation Room, a serious evidence-analysis activity that must run on the same unchanged room semantics.

## How it works

~~~mermaid
flowchart LR
    H["Human client"] --> G["HTTP and WebSocket gateway"]
    R["External agent runner"] --> G
    G --> W["WorldStream room runtime"]
    W --> P["One trusted Activity Pack"]
    W --> D["Bundled SQLite or PostgreSQL 17 primary"]
    P --> O["Authorized Membership projections"]
    O --> G
    W --> A["Durable activation intents"]
    A --> R
~~~

One accepted action follows this path:

    typed action
      → validate against current Authoritative Room State
      → commit one ordered transition
      → update Authoritative Room State
      → persist authorized observation frames and activation intents
      → acknowledge and stream the committed result

This is why the name includes Stream. WorldStream does not primarily stream LLM tokens. It streams meaningful, ordered changes in a shared situation and lets a disconnected participant catch up from a durable cursor.

The frozen releases run exactly one WorldStream process and support exactly two startup-selected durable profiles under the same Room semantics: release-bundled SQLite by default, or one hosted/self-managed PostgreSQL 17 primary. Linux x86-64 and Windows x64 are native release profiles, Linux/amd64 is the only OCI profile, and macOS is a source-build quickstart only. See [ADR 0004](docs/adr/0004-supported-storage-profiles-and-offline-portability.md) and [ADR 0011](docs/adr/0011-release-compatibility-recovery-and-supply-chain-gate.md).

## The frozen boundary

WorldStream owns:

- rooms, Membership lifecycle, Access Mode, current Role assignment, and scoped authorization;
- one total order of accepted transitions per room;
- idempotent action submission and commit-before-acknowledgement;
- three-hash canonical lineage, paired disposable snapshots, verified state reconstruction, recovery, and Replay;
- generation-fenced operational Room Integrity State and verifier-only repair without history rewrite;
- Membership-addressed observations and exact participant Action Offers;
- durable activation intents for external agent runners;
- bounded WebSocket delivery and cursor-based reconnect;
- a small first-party inspector and reference-activity UI.

Activity Packs own:

- domain state and configuration;
- Role definitions and constraints, typed actions, validation, and deterministic reduction;
- public and private projection rules;
- timers, activation reasons, and terminal outcomes.

The trusted v0.1 seam is frozen as `ActivityPackV1`: descriptor, initialize, reduce, view, and observe. Each Room pins a build-computed semantic revision lock; retained exact executors/codecs remain runnable, and Rooms never upgrade in place. This is not a dynamic or sandboxed public plugin ABI.

Agent runners own:

- models, prompts, private memory, tools, credentials, and execution;
- turning an activation intent into a bounded agent invocation;
- submitting typed actions back to the room.

## Two reference activities

1. Agent Heist freezes three immutable seats and six phases, two-of-three sealed plan selection, five-check scoring, scoped reveal, deadlines, activation separation, restart recovery, and replay.
2. Investigation Room proves the same Room Kernel works for serious non-game work: evidence arrives over time, agents publish source-linked claims, a correction invalidates dependent claims, affected agents are activated, and a human lead submits a deterministic structured brief.

There is exactly one Activity Pack per room in both reference releases.

## What this project is not

WorldStream is not n8n, Temporal, a general project manager, a message broker, a context database, a model host, a coding harness, or an agent marketplace. Version 0.1 and 0.2 deliberately exclude workflow canvases, connector catalogs, cross-room projects, payments, crypto, cloud agent execution, vector memory, arbitrary plugins, generated UI, clustering, multiple live WorldStream processes, live/dual-write/reverse storage transfer, provider HA services, cloud resources, and multi-region operation.

## Documentation

- [WorldStream domain context](CONTEXT.md) — canonical whole-product language and concept boundaries
- [Frozen requirements](docs/requirements.md) — normative release scope and change control
- [Canonical decision index](docs/decision-index.md) — frozen invariant, normative source, ADR, conformance evidence, and implementation ownership map
- [Compatibility manifest specification](compatibility.toml) and [canonical JSON mirror](compatibility.json) — fail-closed authored profile; not release evidence while `release_ready = false`
- [Extended terminology](docs/glossary.md) — protocol, runtime, storage, UI, and lifecycle reference
- [Product vision](docs/vision.md) — audience, value, and boundaries
- [System architecture](docs/architecture.md) — stack, storage, filesystem, failure semantics, and scaling
- [Wire protocol](docs/protocol.md) — sessions, actions, observations, cursors, and activations
- [Observation and Activation](docs/observation-and-activation.md) — frozen attach/reset, delivery, intent, lease, and Invocation Context contracts
- [Activity Packs](docs/activity-packs.md) — host contract and both reference activities
- [Context model](docs/context-and-memory.md) — Authoritative Room State versus Invocation Context
- [UI architecture](docs/ui-architecture.md) — deliberately small first-party presentation layer
- [Security model](docs/security.md) — trust boundary and required tests
- [Delivery roadmap](docs/roadmap.md) — Agent Heist MVP followed by Investigation Room
- [Architecture decisions](docs/adr/) — accepted product, Room sequencing, Core/integrity lineage, observation, Activation, retained pack, storage, portability, recovery, and release decisions
- [Idea archive](docs/ideas-and-research.md) — non-normative research only

## Status

The reconciled canonical documentation set is implementation-ready as of 2026-08-15. This repository is still documentation-only: it contains no production runtime, storage adapter, Activity Pack, SDK, UI, release artifact, or conformance evidence, and the compatibility manifest therefore remains `release_ready = false`. Performance figures are reference targets until a reproducible report exists and are never universal SLAs.
