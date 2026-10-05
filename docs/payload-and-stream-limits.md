# Payload and stream limits

A new Room can select `worldstream/transition/v2` through
`canonical_history_format` in the creation request. Omission selects V1.
Explicit V1 has the same request identity as omission. Existing Rooms keep
their original format. A creation retry must keep its original selection.

V2 Genesis binds `worldstream/payload-budget/v1`. This policy adds typed byte
limits. V1 retains its original admission contract. The exact retained Activity
Pack descriptor also applies. Do not change a policy's values under its existing
identity.

## Typed admission

Each limit is inclusive. Count canonical JSON UTF-8 bytes, including escaping,
object keys, array brackets, and commas. A small item does not remove the array
or complete record limit.

| Canonical value | Maximum bytes |
|---|---:|
| Classified control metadata | 4,096 |
| Action payload | 32,768 |
| External input payload | 32,768 |
| Creation configuration | 32,768 |
| Domain Event item | 8,192 |
| Complete ordered Domain Event array | 262,144 |
| Timer-change item | 4,096 |
| Complete ordered Timer-change array | 65,536 |
| Attention Signal item | 4,096 |
| Complete ordered Attention Signal array | 65,536 |
| Complete effects accounting object | 393,216 |
| Complete compact Transition | 524,288 |
| Activity State | 262,144 |
| Core Room State | 131,072 |
| Complete Authoritative State accounting object | 524,288 |
| Complete Genesis | 786,432 |
| Complete authorized Observation | 32,768 |
| Complete authorized Projection | 262,144 |
| Explicitly declared Artifact reference | 2,048 |

The effects accounting object contains `ordered_attention_signals`,
`ordered_domain_events`, and `ordered_timer_changes`. The Authoritative State
accounting object contains `activity_state` and `core_state`. These objects count
bytes. They do not change state hash preimages.

Artifact-reference checks require an application-declared reference schema.
The SDK helpers do not infer references from arbitrary JSON. Control accounting
uses the fields classified by its contract. It excludes content with a separate
Projection or Observation limit.

After bounded parsing and current authorization, the host resolves the durable
Semantic Receipt by exact identity and canonical request hash. A matching
accepted retry returns its original result. It does not apply a fresh limit or
run the Pack again. Changed request content conflicts.

For fresh work, the host rejects an oversized caller input before Pack execution.
An oversized Pack output causes a Pack Fault before canonical acceptance. No
successful receipt or partial Transition survives that fault. Initialization,
state, ordered effects, complete records, authorized views, and observations
have separate checks.

The Python and TypeScript SDKs expose explicit fresh-payload check helpers. They
do not automatically apply those helpers when sending or retrying a request.
This preserves carriage of an accepted legacy request above a new fresh limit.

## Compact history

A V2 Transition retains its normalized stimulus, ordered effects, and resulting
state commitments. The complete resulting Core and Activity values remain in
the prepared commit and current materializations. They are not copied into each
immutable V2 Transition.

Genesis selects one exact record tuple. Every reader uses that selection.
Unknown tuples and mixed-format records reject. Readers do not guess by trying
multiple decoders. Backup, restore, and transfer retain the original canonical
bytes and exact Pack lock.

Recovery uses a verified paired checkpoint when its tail is at most 250
Transitions. An absent, invalid, or older checkpoint selects full Genesis Replay.
The exact retained executor verifies all effects and commitments. A checkpoint
does not prove the semantics of its skipped prefix. Use full verification for
that proof. See [ADR 0034](adr/0034-compact-canonical-transition-format.md).

The 768 KiB Genesis and 512 KiB Transition limits fit below the default 1 MiB
stream-transfer record allowance, with framing space. Historical readers retain
their 2 MiB record allowance. The 1 MiB portable callback allowance is independent.
Several valid values can exceed that allowance when combined in one callback.

## Authorized live delivery

Publication coalesces committed Room work. Four workers use a bounded pending
Room set. A dirty mark or periodic scan recovers work when a notification is
missing or the pending set is full. The gateway indexes Sessions by Room.

The process limits blocking reads to eight. Observation preparation and queued
delivery each have a separate 32 MiB reservation limit. The global queue holds
at most 2,048 frames. A Session queue holds at most 256 frames and 4 MiB. A send
retains its reservations until completion or its 10-second deadline. These
reservations do not bound every other process allocation.

Each publication read has a 750 ms absolute storage budget within its one-second
caller wait. Authentication, shared reads, and fallback reads use the same
budget. Checks before enqueue and send receive their own bounded read budget.
PostgreSQL publication reads bound connection and socket waits. They also set
transaction-local SQL and lock deadlines. SQLite uses a separate read connection
with a bounded lock wait and a query progress check. These deadlines do not
change Room time or canonical records.

A timeout returns storage unavailability. It retains the read and preparation
reservations until the read actually ends. Publication can retry retained frames.
It does not advance Cursor or mark a Room corrupt because a read timed out.

A payload page contains at most 32 frames and 1 MiB. The adapter checks metadata
before allocating payloads. Compatible Sessions can share an immutable page and
encoded Observation bodies. Each Session receives a fresh envelope and a
separate authority check.

Checks bind the exact Room, Member, complete Head, integrity generation,
Membership generation, reset, binding, and Session grant. The gateway repeats
the relevant checks before enqueue and before send. A current Head may advance
only when its captured prefix still has exact lineage evidence. Revocation,
reattach, reset, or a different Member cannot inherit another Session's access.

Queue failure closes the affected connection. Reconnect uses its durable Cursor
or an explicit reset. Only `observation.ack` advances Cursor. Enqueue and socket
send do not acknowledge client processing.

## Measurements

Runtime message compression remains disabled. The payload script tests lossless
zstd round trips and bounded decoding as a separate experiment. Compression does
not replace typed admission or authorization checks.

Build and run the paired Core workload:

```sh
cargo build --locked --release -p worldstream-core --features conformance-tracer --example measure_room_stream_scaling
python3 scripts/measure-room-stream-scaling.py --mode core-stream --formats v1,v2 --transitions 100,10000,100000 --state-bytes 1024,16384,65536,262144 --output-dir .scratch/core-stream-new-run
```

The output directory must be new. This measures checked executor preparation,
canonical records, and in-memory sealed installation. It does not measure
database durability or socket delivery. The backend and gateway scripts provide
separate workload modes and record their conditions. Use their `--help` output
for provider inputs and output paths. Never place provider credentials in a
measurement report.

Compare exact stored bytes, callback counts, recovery tails, and completed
delivery counts. Keep failed attempts and unsupported workload cells explicit.
Observed timings on a shared machine do not establish a capacity guarantee.
