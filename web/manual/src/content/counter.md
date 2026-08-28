# Counter: the smallest working pack

Counter is the best first end-to-end WorldStream exercise. It proves the real
Room runtime with one tiny rule set: create a Room, attach two Memberships,
receive exact Action Offers, submit Actions against an exact Room Head, observe
authorized Transitions, restart the daemon, and Replay the same history.

Counter is deliberately a test-oriented conformance Activity, not the release
Activity. It exists to make the kernel understandable before adding agent
Runners, models, multi-phase rules, timers, or the Studio control plane.

## Start with the right boundary

The Counter acceptance path is independent of the normal Studio development
stack:

```text
Python acceptance harness
        │
        ├── HTTP: create Room, read Projection, request Replay
        ├── WebSocket: attach, submit Action, receive Frame, ACK
        ▼
   worldstreamd
        │
        ├── WorldStream Core
        ├── exact Counter v2 executor
        └── temporary bundled SQLite database
```

The harness creates its own authority secret and data directory, selects an
unused loopback port, starts a real `worldstreamd`, and removes the temporary
state when it finishes. It does not read, reset, or modify the repository's
`.worldstream/` directory.

An already-running Studio daemon on `127.0.0.1:9410` does not conflict with this
exercise because the harness uses a separate random port.

### What this exercise covers

- the production `worldstreamd` process boundary;
- bundled SQLite startup, durable commits, shutdown, and recovery;
- operator Room creation and scoped Membership capabilities;
- HTTP Projections and historical Replay;
- WebSocket attach, synchronization, Action submission, Frames, and ACKs;
- participant-versus-spectator visibility;
- idempotent retries and conflicting Action IDs;
- stale Room Head rejection and resynchronization;
- exact post-restart state and lineage hash parity.

### What it intentionally skips

- Studio and the Studio Supervisor;
- Task setup and Task Templates;
- Runner processes, Agent Profiles, and model providers;
- the Participant Console;
- Agent Heist's phases, timers, attention, and Outcome.

Counter proves the runtime foundation. The skipped components build on that
foundation and are easier to understand afterward.

## Run it locally

Run every command from the repository root. The commands below assume the
repository-pinned Rust, Python, and `uv` versions from the
[local quickstart](#/quickstart). Node and pnpm are not required for this
Counter path.

### 1. Enter the repository

```sh
cd /path/to/agent-streamer
```

### 2. Verify the required tools

```sh
rustc --version
uv --version
uv run --python 3.14.7 python --version
```

The required identities are:

| Tool | Required version |
| --- | --- |
| Rust | 1.97.1 |
| Python | 3.14.7 |
| uv | 0.12.5 |

### 3. Install the locked Python SDK environment

```sh
uv sync --project sdk/python --locked --python 3.14.7
```

The acceptance scenario imports the public `worldstream_sdk`; it does not call
private Rust storage APIs.

### 4. Build the real daemon

```sh
cargo build --locked -p worldstream-server --bins
```

The acceptance harness uses `target/debug/worldstreamd` by default. Check that
it exists before continuing:

```sh
test -x target/debug/worldstreamd
```

No output and exit code `0` mean the binary exists and is executable.

### 5. Run the live Counter story

```sh
uv run --project sdk/python --python 3.14.7 \
  python examples/counter/run_live_acceptance.py
```

The command emits one bounded JSON report. A successful run exits with code
`0` and contains:

```json
{"status":"completed"}
```

That snippet illustrates the success field; the real report also contains the
storage identity, passed criteria, hashes, frame shapes, and reference
measurements. It does not emit capability or authority secrets.

For a report that is easier to inspect, save and format it:

```sh
uv run --project sdk/python --python 3.14.7 \
  python examples/counter/run_live_acceptance.py \
  --report target/counter-live-report.json

uv run --python 3.14.7 \
  python -m json.tool target/counter-live-report.json
```

A failed required criterion exits with code `2` and reports `"status":"blocked"`
plus a safe `reason_code` or failing `stage`. A blocked report is not a pass.

### 6. Run the focused harness tests

```sh
uv run --project sdk/python --python 3.14.7 \
  pytest -q examples/counter/test_acceptance.py
```

These focused tests validate the report's canonical hashing, safe error
redaction, and owner-only PostgreSQL DSN-file boundary. They complement the
live scenario; they do not replace it.

## What the live story does

The scenario is intentionally small, but it exercises the complete client and
commit loop.

### 1. Start a disposable authoritative runtime

The harness creates a temporary owner-only authority secret and SQLite data
directory. It starts `worldstreamd` on an unused loopback port and waits for
`/readyz` before making any Room request. It also verifies `/version` reports a
verified bundled-SQLite engine.

### 2. Create one Counter v2 Room

The operator client creates a Room with this Activity configuration:

```json
{
  "initial_value": 0,
  "maximum_value": 8
}
```

The Room has two Memberships:

| Membership | Access mode | What it may do and see |
| --- | --- | --- |
| participant | participant | attach, act, Replay, see `value` and `private_ack_count` |
| spectator | spectator | attach, Replay, see only public `value` |

The harness issues each Membership a different scoped capability. It proves
that the spectator capability cannot attach as the participant.

### 3. Read initial Projections and Replay

Before opening the live streams, both clients request the current Projection
and Replay at Room sequence `0`.

- The participant Projection contains `value` and `private_ack_count`.
- The spectator Projection contains only `value`.
- The same visibility boundary applies to historical Replay.

Private data is structurally absent from the spectator response; it is not
sent as a null or redacted field.

### 4. Attach and synchronize both Memberships

Each client opens a Room WebSocket, installs its initial Projection Reset, and
completes the Session's synchronization barrier. It separately ACKs its starting
frame position. Synchronization makes the Session live; the observation ACK
records the Membership's Cursor. The server now has two independently scoped
delivery streams for one authoritative Room.

### 5. Submit a private Action

The participant submits `private_ack` against Room sequence `0`. The accepted
Transition advances the Room to sequence `1` and increments only
`private_ack_count`.

- The participant receives a Frame containing `value` and
  `private_ack_count`.
- The spectator receives no Frame because its authorized Projection did not
  change.

### 6. Submit a public increment

The participant submits `increment` against Room sequence `1`. Counter v2 adds
`2`, so the public value changes from `0` to `2` and the Room advances to
sequence `2`.

- The participant Frame contains `value` and `private_ack_count`.
- The spectator Frame contains only `value`.
- Both clients ACK their own contiguous Frame sequence.

### 7. Prove Action identity and stale-state safety

The harness then exercises three important failure boundaries:

1. Retrying the same Action ID with the same payload returns the original
   Transition as a duplicate; it does not increment twice.
2. Reusing that Action ID with a different payload fails with
   `idempotency_conflict`.
3. Using a new Action ID against the old Room sequence `1` returns
   `stale_room_state`.

After the stale response, the participant resynchronizes and submits a fresh
`increment` against sequence `2`. Counter v2 adds another `2`, producing value
`4` at Room sequence `3`.

The resulting authoritative progression is:

| Room sequence | Accepted stimulus | Public value | Private ACK count |
| ---: | --- | ---: | ---: |
| 0 | Genesis | 0 | 0 |
| 1 | `private_ack` | 0 | 1 |
| 2 | `increment` | 2 | 1 |
| 3 | fresh `increment` after resync | 4 | 1 |

Duplicate, conflicting, and stale submissions do not create additional
Transitions.

### 8. Replay, restart, and reconnect

Both Memberships request current and historical Replay under their own present
authority. The harness records their Projection and Room Head hashes, closes
the WebSockets, stops the daemon, and starts it again against the same temporary
SQLite database.

After reconnecting from each Membership's last acknowledged Cursor, it requests
the current Projections and Replay at sequence `3` again. The run passes only if
the before-and-after Projection and Room Head hash snapshots match exactly.
This proves that the committed history, viewer-scoped projection, and exact pack
executor survive restart.

## Counter's domain model

Counter keeps its authoritative Activity State deliberately compact:

```text
configuration
  initial_value
  maximum_value

authoritative Activity State
  value
  maximum_value
  private_ack_count
```

It exposes two Actions, both with an empty `{}` payload:

| Action | Rule |
| --- | --- |
| `increment` | Add the exact revision's delta if the result does not exceed `maximum_value` |
| `private_ack` | Increment the private count while it is below its fixed bound |

Only participant viewers receive Action Offers. The pack removes an offer when
its corresponding bound is reached. The host admits only an exact current
offer with the declared payload schema.

Counter has separate authorized projections:

```text
participant Projection       spectator Projection
  value                        value
  private_ack_count
```

`maximum_value` remains authoritative state but is not included in either
Activity Projection.

## The minimal client loop

The live scenario follows this protocol loop:

```text
attach Membership
  → install Projection Reset
  → read current Action Offers and Room Head
  → submit exact offer + payload + Head precondition
  → retain the accepted or rejected receipt
  → process the authorized Observation Frame durably
  → ACK the contiguous Frame sequence
```

Do not simplify this to “send an increment command.” The Action Offer says the
Action is currently available to this viewer. The Room Head precondition says
which exact state the decision was made against. Together they prevent clients
from applying stale decisions to newer authoritative state.

This example uses the repository's
[canonical domain model](https://github.com/imom39a/worldstream/blob/main/CONTEXT.md).
The [protocol contract](https://github.com/imom39a/worldstream/blob/main/docs/protocol.md)
defines Room Heads, capabilities, synchronization, and observation ACKs.

## Why Counter has v1 and v2

Counter v1 increments by `1`; Counter v2 increments by `2`. Both revisions
remain compiled and runnable, while only v2 is selectable for new conformance
Rooms.

The version string alone is not the historical identity. A Room is pinned to
an exact revision digest that binds its schemas, codecs, rule source, and
deterministic dependencies. The registry retains the executor and frozen golden
transcript for that revision. Replay must execute that original revision rather
than substituting the newest Counter behavior.

This is the smallest demonstration of the rule that an Activity Pack revision
is executable history, not a mutable label.

## Read the implementation in this order

| Step | Question | Repository source |
| ---: | --- | --- |
| 1 | What happens across the real public boundary? | `examples/counter/run_live_acceptance.py`, starting at `run_acceptance` |
| 2 | What does the public client expose? | `sdk/python/src/worldstream_sdk/client.py`, especially `Client` and `Room` |
| 3 | What are Counter's configuration, state, and projections? | `crates/worldstream-core/src/counter.rs` |
| 4 | How do `initialize`, `reduce`, `view`, and `observe` work? | `crates/worldstream-core/src/counter.rs` |
| 5 | How are v1, v2, and golden transcripts retained? | `crates/worldstream-core/src/counter_registry.rs` |
| 6 | What must work for every storage backend? | `crates/worldstream-conformance` |

Read the live scenario before the lower-level server implementation. It gives
each protocol operation a concrete purpose, making the deeper commit and
storage code easier to follow.

## Troubleshooting

### `worldstreamd_binary_missing`

Build the daemon and confirm the default path is executable:

```sh
cargo build --locked -p worldstream-server --bins
test -x target/debug/worldstreamd
```

Use `--binary /absolute/path/to/worldstreamd` only when deliberately testing a
different binary.

### Cargo waits for the build-directory lock

Another Cargo command is compiling or testing in the same checkout. Let it
finish, then rerun the build. Do not delete Cargo locks or the `target/`
directory to force concurrent writers.

### Node reports an unsupported engine

Node is not used by this Counter harness. Fix the Node version to the pinned
`24.18.1` before running Studio or the web applications, but it does not block
the Rust daemon plus Python SDK exercise.

### The normal `.worldstream/` authority state is broken

The Counter harness does not use it. Its authority secret and database live in
a temporary directory and are deleted together. Diagnose persistent Studio
state separately; do not clear it merely to run this exercise.

### The report says `blocked`

Use `reason_code` and `stage` to locate the failed boundary. The harness emits
safe typed errors rather than raw exception or credential material. Fix the
reported boundary and rerun the whole scenario; never reinterpret a blocked
criterion as acceptance.

## Use Counter as a template

When creating a new small pack, copy the shape rather than the Counter domain
names:

1. define one compact canonical Activity State;
2. define one or two closed Action types and strict payload schemas;
3. initialize deterministically from recorded configuration;
4. reduce only exact offers against exact Room Heads;
5. project state and offers separately for every viewer class;
6. derive authorized observations from before-and-after views;
7. freeze a reviewed golden transcript;
8. prove both current execution and retained Replay through a real boundary.

Add complexity one axis at a time: Roles, private views, timers, attention,
Membership changes, and Outcome. Continue with
[Agent Heist](#/activity-packs/agent-heist) when the Counter path is familiar.

Primary sources: [Counter executor and schemas](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-core/src/counter.rs),
[Counter registry](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-core/src/counter_registry.rs),
[live acceptance](https://github.com/imom39a/worldstream/blob/main/examples/counter/run_live_acceptance.py),
and [protocol conformance requirements](https://github.com/imom39a/worldstream/blob/main/docs/protocol.md#required-conformance-scenarios).
