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

V2 pays checkpoint storage and Pack CPU during warm recovery. An eligible,
verified checkpoint bounds continuation recovery to its tail. Full historical
verification still checks the complete lineage. V1 offers a complete resulting value at each
record and the simplest random byte inspection, at the cost of repeated state
copies and storage proportional to state size times history. Physical
compression preserves V1's logical format and corruption semantics, but adds
codec CPU and compressed-block handling without changing the replay model.

## Consequences

The implementation adds typed V2 models, golden canonical bytes, hash vectors,
exact codec dispatch, and backend parity tests. Tests cover equal logical
effects against V1, tampered and missing records or checkpoints, missing exact
executors, archived Rooms, and backup/restore byte preservation. Room creation
defaults to V1. Operators can select V2 for a new Room. Existing Rooms retain
their Genesis-selected format.

## Rejected alternatives

* Rewriting old records to summaries was rejected because it destroys the
  immutable lineage and changes the exact executor contract.
* Storing only a resulting state hash was rejected because it loses the
  recorded input and ordered effects needed for deterministic replay.
* Making compressed V1 the canonical format was rejected because compression
  is a physical storage concern and does not address state-copy cost.
* Dropping checkpoints was rejected because compact records cannot reconstruct
  opaque Activity state without a verified materialized starting point.

## Amendment: additive Genesis and creation identities

The V2 codec boundary is implemented. Both storage profiles, serving, Replay,
recovery, maintenance, backup, restore, and transfer support the format.
Production Room creation defaults to V1 and accepts explicit V2 selection.
This amendment does not convert an existing Room.

Genesis V2 uses this exact tuple:

```text
genesis_version: "worldstream/genesis/v2"
codec_id: "worldstream/genesis-record/v2"
hash_suite: "blake3-canonical-json-v2"
```

Its stored fields are exactly:

```text
genesis_version, codec_id, hash_suite
room_id, core_schema_version, pack_digest
configuration, room_seed, created_at, initial_timers
initial_core_state, initial_activity_state
initial_core_state_hash, initial_activity_state_hash
initial_authoritative_state_hash
transition_version: "worldstream/transition/v2"
transition_codec_id: "worldstream/transition-record/v2"
transition_hash_suite: "blake3-canonical-json-v2"
payload_budget_id: "worldstream/payload-budget/v1"
genesis_hash
```

The separate Genesis hash preimage has exactly these fields:

```text
domain: "worldstream/genesis/v2"
codec_id: "worldstream/genesis-record/v2"
hash_suite: "blake3-canonical-json-v2"
room_id, core_schema, pack_digest
configuration, room_seed, created_at, initial_timers
initial_core_state_hash, initial_activity_state_hash
initial_authoritative_state_hash
transition_version: "worldstream/transition/v2"
transition_codec_id: "worldstream/transition-record/v2"
transition_hash_suite: "blake3-canonical-json-v2"
payload_budget_id: "worldstream/payload-budget/v1"
```

`core_schema` in the preimage and `core_schema_version` in each stored record
both identify `worldstream.core-room-state.v1`. The initial state hashes bind
the complete initial values. Do not include those values or `genesis_hash` in
the preimage. The Transition V2 preimage likewise uses the key `core_schema`.
Its other fields remain as specified above. Existing state hash domains and
the Pack descriptor's canonical JSON identity remain unchanged.

Decode original canonical bytes. Reject duplicate keys, unsafe integers,
floats, unknown fields, missing fields, noncanonical encodings, and unsupported
tuples. Verify initial Core shape, active Room Status, normalized initial Timer
generations, all initial state commitments, and the Genesis hash. The initial
Timers have generation one, strictly increasing Timer IDs, and Scheduled Time
later than creation time. Exact Pack execution separately verifies its initial
Activity and rules.

Select the lineage format once from verified Genesis. A Transition decoder
requires that selected format. It rejects a record from another format. A
compact record verifier checks its exact tuple, aggregate commitment, and
record hash. A successor check also verifies Room, Pack, Core schema, sequence,
and predecessor. These checks do not reconstruct opaque Activity. Structural
preflight must derive Core and Timer facts. Exact semantic Replay must verify
the component state commitments and ordered Pack effects.

### Creation request

The additive creation wire object has these fields:

```text
pack_digest, configuration, ordered_initial_memberships
canonical_history_format: optional "worldstream/transition/v1"
                          or "worldstream/transition/v2"
```

Reject unknown fields and selectors before Pack execution or persistence.
Omission and explicit V1 normalize to the original V1 wire object. Both use
the unchanged `worldstream/create-room-request/v1` preimage and request hash.
Explicit V2 uses this exact separate preimage:

```text
domain: "worldstream/create-room-request/v2"
pack_digest, configuration, ordered_initial_memberships
canonical_history_format: "worldstream/transition/v2"
payload_budget_id: "worldstream/payload-budget/v1"
```

The fixed V2 selection implies the policy identity. Callers cannot select an
arbitrary policy. Generated Room and Member IDs, seed, creation time, and
commit time remain outside the request identity. Keep the existing creation
operation kind and durable receipt conflict behavior. The different format
hash causes a conflict under the same creation identity. A future policy must
have a new supported identity and bind it in both Genesis and the versioned
creation preimage.

### Fixed initial payload policy

The policy is an additional contract for explicitly selected V2 Rooms. It does
not change retained Pack descriptors or V1 admission. The inclusive canonical
UTF-8 byte ceilings are frozen in
[`payload_budget.rs`](../../crates/worldstream-core/src/payload_budget.rs):

| Value | Bytes |
| --- | ---: |
| Control metadata | 4,096 |
| Action payload | 32,768 |
| External-input payload | 32,768 |
| Creation configuration | 32,768 |
| Domain Event item | 8,192 |
| Ordered Domain Event array | 262,144 |
| Timer-change item | 4,096 |
| Ordered Timer-change array | 65,536 |
| Attention Signal item | 4,096 |
| Ordered Attention Signal array | 65,536 |
| Complete effects accounting object | 393,216 |
| Complete compact Transition | 524,288 |
| Activity State | 262,144 |
| Core Room State | 131,072 |
| Complete Authoritative State accounting object | 524,288 |
| Complete Genesis | 786,432 |
| Complete authorized Observation | 32,768 |
| Complete authorized Projection | 262,144 |
| Application Artifact reference | 2,048 |

Count array brackets and commas. The effects accounting object has exactly
`ordered_attention_signals`, `ordered_domain_events`, and
`ordered_timer_changes`. The Authoritative State accounting object has exactly
`activity_state` and `core_state`. These objects do not change hash preimages.
Count the complete authorized view, including schemas, Action Offers, and
authorized Core. Apply Artifact-reference accounting only when an exact
application schema identifies a reference. Keep complete wire, callback,
Invocation Context, and operational queue bounds separate.

Admission enforcement remains a later integration step. Resolve an exact
accepted durable retry under its original request hash before applying fresh
policy checks. Replay and restore use the original Genesis-bound policy.
Retained V1 vectors remain unchanged. The additive fixed vectors are in
[`core_v2_golden.json`](../../tests/fixtures/core_v2_golden.json). Regenerate only
that new fixture with `cargo run --locked -p worldstream-core --example
generate_core_v2_golden`.

### Additive execution and commit boundary

`CanonicalRoomTrace` selects records from its Genesis format. It uses the same
Core reducer and retained Pack preparer as `CoreTraceV1`. Complete prepared
Core and Activity state remain separate from `TransitionRecord`. Observations,
Timer mutations, and Activation decisions use that verified state.

`PreparedCanonicalRoomCreation` and `PreparedCanonicalRoomCommit` seal exact
record bytes, complete materializations, receipts, and authority fences.
`CanonicalRoomCommitStorage` receives these immutable bundles. The coordinator
installs state only after the exact new durable receipt. Existing and ambiguous
results withhold speculative state. An ambiguous result permits resolution
before retry. A known-absent retry retains the original sealed input and time.

Legacy entrypoints retain their V1 contracts and bytes. Conversion to
`CoreTraceV1` requires genuine V1 records. V2 creation receipts use the additive
`RoomCreationV2` semantic input and preserve the selected request hash.
Production V2 creation remains disabled until storage lifecycle, recovery,
policy admission, and format-aware creation authorization are integrated.

### Structural history and exact Replay

`CanonicalStorageHistoryPreflight` selects every record from immutable Genesis.
It derives Core, Timer schedules and generations, and Membership generations
without a Pack callback. Its result has no opaque Activity value. A supplied
Activity materialization can be checked against the final commitment. This
check does not establish semantic execution.

Exact Replay resolves the retained executor and reproduces initialization,
ordered effects, state commitments, record hashes, and original canonical
bytes. A missing executor reports `RuntimeUnavailable`. Structural corruption
has priority over runtime availability. Historical projection uses the derived
historical Membership and the existing projection checks. The durable adapter
must also revalidate present Capability and captured integrity fences.

The executable page visitor yields complete state verified through each cut.
The adapter must wait for successful full-prefix completion and recheck its
captured Head and integrity fences before it installs generated snapshots.
Pages can be discarded after each fold. Replay creates no serving effects or
durable receipts.

### Current verification and guarded recovery

Current verification requires strict immutable Genesis evidence. It checks the
selected current record, complete canonical Core and Activity materializations,
and all state commitments. A separate predecessor check verifies the exact
Room, Pack, Core schema, sequence, and lineage link. Neither check executes a
Pack or scans historical state.

Structural history exposes full Timer generation facts and fixed Activation
policy decisions. These facts derive from recorded ordered effects. They do
not establish Activity semantics. Storage can check these facts before it
reports an unavailable retained runtime.

Full storage Replay and the explicit observation-enabled paged pass reproduce
operational witnesses. State-only Replay cannot claim this verification.
Recovery retains final complete state and required operational facts. It does
not retain a full-state copy for every historical cut.

Canonical recovery reuses the byte-neutral candidate and checkpoint containers.
An eligible verified checkpoint executes at most 250 tail Transitions. Invalid
checkpoints fall back to full Replay. Missing serving materializations remain
rebuildable. A missing runtime reports `RuntimeUnavailable` and a faulted
integrity disposition. Corruption uses the quarantine disposition. A recovered
executor is released only after storage verifies the captured complete Head
and integrity generation at installation.
