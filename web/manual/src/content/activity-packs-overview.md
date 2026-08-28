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

## The ActivityPackV1 shape

The Rust trait lives in `crates/worldstream-core/src/activity_pack.rs`. Its
responsibilities are conceptually:

```rust
pub trait ActivityPackV1: Send + Sync + 'static {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1;
    fn initialize(/* exact request and deterministic host context */);
    fn reduce(/* prior activity state plus one normalized stimulus */);
    fn view(/* exact state and one PackViewerV1 */);
    fn observe(/* predecessor and successor views */);
}
```

Use the source trait—not this abbreviated illustration—as the compile-time
contract.

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
4. Request any WorldStream-owned Core changes explicitly.
5. Emit deterministic Domain Events, timers, observation consequences,
   attention, and Outcome changes.
6. Let Core validate/veto protected Membership and Room lifecycle changes.

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
