---
status: accepted
date: 2026-09-12
---

# ADR 0034: Compact canonical Transition records for new Room lineages

## Context

`TransitionV1` stores the complete resulting Core and Activity values in every
immutable record. This makes a long Room history grow with the Activity state,
even when one transition changes only a small input or emits a small
observation. Physical compression reduces disk bytes but does not remove the
state copy during commit, or make a compressed record independently
reconstructible. The repeatable comparison in
[`docs/transition-format-comparison.md`](https://github.com/imom39a/worldstream/blob/ac7f443562bce33088229351d5f9eba40bb67bdd/docs/transition-format-comparison.md)
measures the tradeoff for 1, 16, 256, and 1024 KiB states at 1,000, 10,000,
and 100,000 transitions.

## Decision

New lineages may opt into `worldstream/transition/v2`, a compact canonical
record. This ADR specifies the format boundary; it does not change existing
storage or migrate a live Room. A V2 record contains these fields:

```text
transition_version: "worldstream/transition/v2"
codec_id: "worldstream/transition-record/v2"
hash_suite: "blake3-canonical-json-v2"
room_id, room_seq, core_schema_version, pack_digest
previous_transition_or_genesis_hash
recorded_stimulus
ordered_domain_events
ordered_timer_changes
ordered_attention_signals
resulting_core_state_hash
resulting_activity_state_hash
resulting_authoritative_state_hash
transition_hash
```

The record is encoded with the existing checked canonical JSON rules: UTF-8,
sorted object keys, no insignificant whitespace, safe integers, no duplicate
keys, and strict `deny_unknown_fields` decoding. Binary values use the existing
canonical digest representation. The record contains no resulting Core or
Activity state and no model summary. It retains the exact normalized stimulus
and every ordered reducer output required to replay the exact Pack.

`transition_hash` is BLAKE3 over the canonical encoding of a separate
hash-preimage object; the stored `transition_hash` field is explicitly
excluded. That preimage has fields `domain = "worldstream/transition/v2"`, `codec_id`,
`hash_suite`, Room identity and sequence, Core schema, Pack digest, previous
lineage hash, the recorded stimulus, three ordered output arrays, and the
three resulting state hashes. The state hashes retain the ADR 0005 domains and bind the exact Core,
Pack-scoped Activity, and aggregate Authoritative State. No receipt, delivery
frame, snapshot, operational integrity row, or commit timestamp enters the
canonical hash.

V2 requires a verified paired checkpoint at Genesis or within 250 transitions.
The checkpoint is the complete witness described by ADR 0031: canonical Core
and Activity bytes and hashes, lineage Head, exact Pack lock, timer ledger,
membership generations, observation frames/consequences, and any other
operational materialization needed for continuation. Snapshot cadence remains
the postcommit `250 transitions or five active minutes` policy; a V2 warm
reader uses the bounded path only when that policy leaves a tail of at most
250, and otherwise uses full Genesis replay. Checkpoints are replaceable
materializations, never canonical history.

On replay, the exact retained Pack receives the checkpoint state and each V2
record's immutable stimulus. The host requires the Pack's ordered events,
timer changes, attention signals, resulting Core/Activity values, all three
hashes, previous lineage hash, and transition hash to match before advancing
the trace. A mismatch, missing record, missing Pack, malformed checkpoint, or
unknown codec fails closed. Full Genesis replay remains available for backup
and forensic verification. As with ADR 0031, a trusted checkpoint does not
prove the semantic validity of its skipped prefix; deployments requiring that
proof run the full verifier before serving.

Codec dispatch is by the immutable Genesis/lineage codec identity. V1 Rooms
continue to decode and execute `TransitionV1` with the V1 hash framing and
exact Pack contract. V2 Rooms never accept a V1 record, and an unknown version
is an explicit restore/recovery error. SQLite and PostgreSQL backup, restore,
transfer, and export paths must preserve the original opaque record bytes and
dispatch through this same version table. There is no in-place rewriting,
deletion, summary compaction, or conversion of an existing lineage. A future
migration, if approved, creates a new lineage with an explicit provenance
record and independently verifies both histories.

## Evidence and tradeoffs

The benchmark uses deterministic incompressible state bytes and a 1 KiB
observation payload. On the reference run, a 256 KiB state over 100,000
transitions is about 24.62 GiB with V1, 295.33 MiB with V2 records plus
250-step checkpoints, 185.78 MiB with V1 compressed by zlib-9, and 69.94 MiB
with V2 compressed by zlib-9. Encoding 100 256 KiB records took about 4.10 s
for V1 and 31.62 ms for the compact record. Values are workload measurements, not
capacity guarantees; rerun the script on the target engine and state shape.

V2 pays checkpoint storage and Pack CPU during warm recovery, but bounds a
historical view to the nearest checkpoint tail and localizes corruption to a
checkpoint or bounded interval. V1 offers a complete resulting value at each
record and the simplest random byte inspection, at the cost of repeated state
copies and storage proportional to state size times history. Physical
compression preserves V1's logical format and corruption semantics, but adds
codec CPU and compressed-block handling without changing the replay model.

## Consequences

The production implementation remains a follow-up: add typed V2 models,
golden canonical bytes, hash vectors, exact codec dispatch, and backend parity
tests before allowing a new Room creation path to select V2. Those tests must
cover equal logical effects against V1, tampered and missing records or
checkpoints, missing exact executors, archived Rooms, and backup/restore byte
preservation. Until that boundary is implemented and enabled, all Rooms retain
the current V1 format.

## Rejected alternatives

* Rewriting old records to summaries was rejected because it destroys the
  immutable lineage and changes the exact executor contract.
* Storing only a resulting state hash was rejected because it loses the
  recorded input and ordered effects needed for deterministic replay.
* Making compressed V1 the canonical format was rejected because compression
  is a physical storage concern and does not address state-copy cost.
* Dropping checkpoints was rejected because compact records cannot reconstruct
  opaque Activity state without a verified materialized starting point.
