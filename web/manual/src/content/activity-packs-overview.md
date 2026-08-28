# Activity Pack development

An Activity Pack is the deterministic rule set for a class of Rooms. In v0.1 it
is trusted Rust code embedded in the server and selected by exact immutable
revision. The public portable plugin ABI is deferred.

## What a revision owns

Every retained revision binds:

- descriptor, pack ID, semantic revision, and content digest;
- configuration and canonical state schemas;
- Action payload schemas and Action Offer vocabulary;
- deterministic initialization and stimulus reduction;
- viewer-scoped projection and observation consequences;
- Activity Phases, timers, attention, and Outcomes;
- canonical codecs and retained Replay behavior;
- conformance/golden corpus evidence.

Changing any semantic element requires a new revision. Existing Rooms remain
pinned to their original executable revision for their complete lineage.

## The exact ActivityPackV1 seam

The Rust trait lives in `crates/worldstream-core/src/activity_pack.rs`. These
are the current five signatures:

```rust
pub trait ActivityPackV1: Send + Sync + 'static {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1;
    fn initialize(
        &self,
        input: &ActivityGenesisInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1>;
    fn reduce(
        &self,
        input: &ActivityReduceInputV1<'_>,
        cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1>;
    fn view(&self, input: &ViewInputV1<'_>)
        -> Result<PackViewV1, PackFaultV1>;
    fn observe(&self, input: &ObserveInputV1<'_>)
        -> Result<Option<PackObservationV1>, PackFaultV1>;
}
```

There is no sixth callback, dynamic loader, model hook, storage handle, or
ambient clock. Re-check the source trait when implementing because it—not this
website—is the compile-time authority.

## Build an in-tree pack revision

Local v0.1 does not have a public package generator or plug-in ABI. Adding an
Activity is a reviewed `worldstream-core` change. Counter is the smallest
compiling template; Agent Heist shows the full privacy/timer path.

### 1. Create the executor module

Add `crates/worldstream-core/src/<pack>.rs`. A first compile checkpoint can use
this exact method shape while the descriptor, reducer, and view helpers are
built:

```rust
use crate::{
    ActivityDispositionV1, ActivityGenesisInputV1, ActivityPackV1,
    ActivityReduceInputV1, DeterministicContextV1, InitialOutputV1,
    ObserveInputV1, PackFaultV1, PackObservationV1,
    PackRevisionDescriptorV1, PackViewV1, ViewInputV1,
};

#[derive(Clone, Copy)]
pub(crate) struct ExampleV1;

impl ActivityPackV1 for ExampleV1 {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1 { todo!() }

    fn initialize(
        &self,
        _input: &ActivityGenesisInputV1<'_>,
        _cx: &DeterministicContextV1<'_>,
    ) -> Result<InitialOutputV1, PackFaultV1> { todo!() }

    fn reduce(
        &self,
        _input: &ActivityReduceInputV1<'_>,
        _cx: &DeterministicContextV1<'_>,
    ) -> Result<ActivityDispositionV1, PackFaultV1> { todo!() }

    fn view(&self, _input: &ViewInputV1<'_>)
        -> Result<PackViewV1, PackFaultV1> { todo!() }

    fn observe(&self, _input: &ObserveInputV1<'_>)
        -> Result<Option<PackObservationV1>, PackFaultV1> { todo!() }
}
```

Run `cargo check -p worldstream-core` after adding the module in `lib.rs`.
Replace every `todo!()` before registry construction; a panic is contained but
is still a Pack Fault, never valid behavior.

### 2. Define canonical schemas and behavior

Follow `counter.rs` in this order:

1. add strict `serde` configuration, state, and projection types with unknown
   fields denied;
2. declare stable pack, Role, Action, event, attention, and schema IDs;
3. construct the descriptor, schema bundle, codec bundle, revision lock, and
   source-derived executor artifact digest;
4. implement deterministic initialization and reduction over canonical JSON;
5. implement viewer-class-specific `view`, including exact Action Offers;
6. implement `observe` from predecessor/successor authorized views;
7. add unit tests for every Stimulus, rejection, viewer, and privacy boundary.

Do not persist current Role ownership in Activity State. Read it from the
immutable Core view. Do not request a Core mutation from the pack.

### 3. Add the retained registry row

Add `crates/worldstream-core/src/<pack>_registry.rs`, following
`counter_registry.rs`. Construct one `PackRegistryEntryV1` with the exact
revision lock, descriptors, schema/codecs, executor artifact digest, golden
corpus and digest, concrete executor, and independent selection/retention
flags.

The production registry is intentionally closed. Extend all of these reviewed
seams explicitly:

| Seam | Required change |
| --- | --- |
| `activity_pack.rs` | add a `ReviewedExecutorProvenanceV1` variant, exact concrete type/constructor mapping, artifact digest mapping, and typed `PackRegistryEntryV1` constructor |
| `lib.rs` | declare the executor/registry modules and export only the required catalog helpers |
| `<pack>_registry.rs` | build and self-verify the revision row and frozen golden corpus |
| `registry.rs` | combine the new validated registry into `builtin_worldstream_registry()` |
| `compatibility.toml` | declare the exact digest and selection/retention status |
| generated compatibility JSON | regenerate and verify the checked mirror with `xtask` |

`PackRegistryV1::try_new` recomputes and cross-checks every identity. A missing
schema, codec, implementation, provenance mapping, digest, or golden transcript
must fail registry construction.

### 4. Freeze evidence and run the focused gates

Author a multi-view golden corpus in the registry module. Include Genesis,
accepted and rejected Actions, timers/Core changes where applicable, every
viewer class, Action Offers, observations, and the terminal Outcome. Compute
the corpus digest with `PackGoldenCorpusV1::digest()`, review the complete
transcript, then freeze the literal. Never silently refresh a digest after a
semantic change.

```sh
cargo fmt --all -- --check
cargo test -p worldstream-core <pack_name>
cargo test -p worldstream-conformance
cargo run --locked -p xtask -- compat verify
scripts/gates.sh fast
```

Finally add a real boundary-level example under `examples/<pack>/`; unit tests
or an offline story alone are not evidence that the daemon/client path works.

## Determinism rules

Pack code must be a pure function of its exact inputs. It may not read the
clock, generate randomness, access the filesystem/network, call a model, depend
on map iteration order, or inspect unrecorded host state.

The host supplies deterministic context:

- normalized Stimulus and Semantic Time;
- exact prior Core and Activity State;
- immutable pack configuration and revision lock;
- recorded deterministic bytes/seed when rules require them;
- exact viewer Membership when projecting.

Panics are contained and invalid output fails closed. A repeatedly failing
required input may fault the Room; canonical mismatch may quarantine it.

## Reduction workflow

1. Match the normalized Stimulus kind.
2. Validate pack-specific Role, phase, deadline, and payload rules.
3. Return a typed rejection without mutation, or a proposed next Activity
   State and consequences.
4. For a Core Stimulus, inspect the immutable Core-before and host-proposed
   Core-after views; veto only join, resume, Access Mode, or Role proposals
   when the pack's stable rules require it. A pack never mutates Core.
5. Emit deterministic Domain Events, timers, observation consequences,
   attention, and Outcome changes.
6. Let Core construct, authorize, and validate every Membership and Room
   lifecycle change before persistence.

Never duplicate Action legality in the UI or Runner. `view` emits exact
current Action Offers for the viewer; clients submit one of those offers.

## Projection checklist

For every viewer class—participant Role, spectator, operator, historical
viewer—test:

- allowed fields are present;
- forbidden fields are structurally absent, not null/redacted placeholders;
- Action Offers match only current view/state;
- visibility loss cannot expose prior private knowledge through incremental
  delivery;
- terminal reveals occur only at the declared phase;
- debug/errors contain no private state.

## Revision workflow

1. Add a new exact revision; never mutate the semantic behavior of a retained
   revision.
2. Add schemas/codecs and the descriptor lock.
3. Register the executor in `worldstream-core`'s production registry.
4. Update `compatibility.toml` and its generated JSON mirror.
5. Add golden transcripts and retained-corpus tests.
6. Run pack tests, backend-neutral conformance, storage adapters, Replay, and
   privacy matrices.
7. Keep the old revision executable if compatibility says it is retained.

## Current examples

- [Counter tutorial](#/activity-packs/counter) — smallest state/action/replay
  path.
- [Agent Heist](#/activity-packs/agent-heist) — phases, timers, privacy,
  attention, multiple agents, and Outcome.
- Investigation Room in `docs/activity-packs.md` — a design/generality reference,
  not a shipped executable pack.

Primary source: [Activity Pack contract](https://github.com/imom39a/worldstream/blob/main/docs/activity-packs.md),
[exact-revision ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0010-activity-pack-v1-and-executable-replay-retention.md),
and [trait source](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-core/src/activity_pack.rs).
