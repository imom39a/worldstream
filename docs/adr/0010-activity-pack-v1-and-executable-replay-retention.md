---
status: accepted
date: 2026-08-15
---

# Activity Pack v1 and executable Replay retention

> **Partial supersession:** [ADR 0014](0014-installable-wasi-free-activity-pack-bundles.md)
> replaces only the embedded-registry-only distribution and “no portable ABI”
> conclusions below. The five-operation `ActivityPackV1` semantics,
> `PackRevisionLockV1`, immutable Room pin, exact retained execution, privacy,
> Recovery, and Replay requirements remain authoritative.

WorldStream freezes one trusted synchronous `ActivityPackV1` seam with exactly five operations: `descriptor`, `initialize`, `reduce`, `view`, and `observe`. The seam is deliberately smaller than a plugin system: packs receive canonical inputs and deterministic helpers, return bounded canonical values, and receive no clock, storage, network, filesystem, scheduler, Activation, Session, delivery, telemetry, or artifact-byte capability.

The `view` result owns both the authorized Activity Projection and its ordered Action Offers. Those exact canonical Action Offer bytes are reused by Projection Reset, Observation Frames, Invocation Context, and host Action pre-admission; WorldStream does not maintain another legality representation.

Every semantic pack revision has a build-computed `PackRevisionLockV1` digest covering its host-contract and codec versions, schemas, deterministic static data, rule source, and deterministic dependency lock. The embedded registry maps that exact digest to its executor, descriptor and schemas, codecs, golden-corpus digest, and two statuses: selectable for new Rooms and runnable for retained Rooms. Selectable implies runnable, and every digest referenced by retained lineage must remain runnable even after it is no longer selectable.

A Room never changes its pinned digest or rewrites canonical Activity State in place. A new semantic revision creates a new Room; recovery, projection, advancement, and Replay of an old Room continue through the exact retained executor and codecs. Missing exact executable support is an explicit compatibility failure, not an invitation to dispatch old bytes through newer rules.

Agent Heist v0.1 is the proving pack for this contract, not a Kernel exception. Its three immutable Genesis seats, six phases, sealed two-of-three plan selection, five-check scoring, viewer-scoped disclosure, deterministic Attention, and phase timers are ordinary Activity State, reducer, view, observation, and timer-request behavior specified in [Activity Pack Design](../activity-packs.md).

## Consequences

- Pack callbacks finish before persistence handoff and may fail only as declared domain rejection or fail-closed `PackFault`.
- Retaining decoders without the historical reducer is insufficient for a healthy retained Room.
- Every behavior, legality, event, timer, Attention, or visibility-rule change creates a new semantic digest.
- Release compatibility evidence must exercise exact executors and codecs through full Replay, not merely decode old bytes.
- There is no dynamic code download, third-party pack upload, sandbox promise, general effect interface, or stable portable plugin ABI in v0.1.

## Considered options

- A mutable trait that could drift during Heist was rejected because its persisted state and lineage become a long-lived compatibility contract at the first Room creation.
- Decoder-only retention or in-place migration was rejected because Replay must execute the original rules and verify their outputs rather than reinterpret or rewrite history.
- Separate legality lists were rejected because projection, frames, Invocation Context, and admission would eventually disagree.
- A generic plugin/effect ABI was rejected because Counter and Heist need only the five deterministic operations, while trust isolation and arbitrary effects require a separate future design.
