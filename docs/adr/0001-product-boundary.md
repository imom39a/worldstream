# ADR 0001: Freeze WorldStream as a Realtime Room Runtime

Date: 2026-08-13

Status: Accepted

Partial supersession: [ADR 0014](0014-installable-wasi-free-activity-pack-bundles.md)
replaces the compiled-in-only Pack distribution boundary and the Heist-first /
Investigation-committed release ordering. The Room authority boundary, one
process, external model execution, and excluded workflow/marketplace scope in
this decision remain accepted.

## Context

Exploration began with a Rust alternative to realtime streaming products and expanded into agent worlds, games, marketplaces, crypto rewards, coding harnesses, enterprise project rooms, shared context, dynamic UI, long-running agents, and cross-room work.

Several of those ideas are interesting, but together they describe incompatible products:

- infrastructure for developers;
- a vertical business application;
- a workflow and project-management platform;
- a model execution service;
- a game/social destination;
- a labor and payment marketplace.

The broad scope also made the Activity Pack responsible for almost all application value while asking the core to anticipate every domain. That would be unrealistic for a solo open-source project and difficult to explain.

## Decision

WorldStream will be a self-hosted realtime room runtime for applications where humans and independently hosted AI agents:

- share one authoritative room state;
- receive scoped participant-specific projections;
- submit typed actions under pack-defined rules;
- disconnect and catch up through cursors;
- use durable activation intents across ephemeral agent invocations;
- recover and replay deterministic history.

One server may host many independent rooms. Each room pins exactly one trusted Activity Pack revision.

The initial adopter is an AI application developer. Agent Heist is the v0.1 proof. Investigation Room is the v0.2 generality test.

## Consequences

WorldStream owns participation semantics, not model execution or business workflow design.

The frozen releases:

- include human and agent participants;
- use trusted compiled-in Rust packs;
- run exactly one WorldStream process and remain local-first by default; the optional PostgreSQL 17 primary may be hosted or self-managed on another machine without authorizing a second WorldStream process;
- provide only a small first-party UI;
- do not stabilize a public plugin ABI before both reference packs exist.

The frozen releases exclude:

- Project and Workspace entities;
- cross-room data exchange and multi-pack rooms;
- coding harnesses, shells, sandboxes, and model hosting;
- workflow canvases, connectors, and generic business task management;
- marketplaces, payments, crypto, identity chains, and tokens;
- generic RAG, vector memory, and LLM-generated summaries;
- runtime-generated UI and arbitrary renderer code;
- timeline forks and branch promotion;
- clustering, federation, and multi-region mutation.

## Why Agent Heist and Investigation Room

Agent Heist exposes partial information, racing decisions, timers, targeted activation, and replay in a format that is easy to see.

Investigation Room stresses different semantics: a human acts alongside agents, evidence changes over time, claims depend on exact source versions, a correction invalidates earlier work, and the final output is verified without an LLM judge.

If Investigation needs new Room Kernel concepts instead of merely new domain state, actions, projections, and views, the Activity Pack boundary has failed its first generality test.

## Revisit conditions

No excluded product family should return to the committed roadmap before:

1. both reference releases pass their acceptance gates;
2. at least two outside developers identify the same missing capability;
3. a small ADR shows why an external system or adapter cannot solve it;
4. the added scope does not turn WorldStream into a workflow engine, model host, or marketplace.
