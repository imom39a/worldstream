# Tower of Hanoi example

This example uses a WorldStream Room for a shared puzzle. Each solver has its
own Membership and Runner credentials. The Pack checks legal moves and records
completion claims and assessments.

The objective is to move the tower from A to C. The Pack does not decide whether
a claim is correct. Completion requires a strict majority of the eligible
solvers recorded when the claim began. A new move invalidates the current claim.

## Build and test the Pack

Run these commands from the repository root:

```sh
pnpm --dir examples/packs/tower-of-hanoi pack:check
pnpm --dir examples/packs/tower-of-hanoi pack:test
pnpm --dir examples/packs/tower-of-hanoi pack:build
pnpm --dir examples/packs/tower-of-hanoi pack:inspect
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  pnpm --dir examples/packs/tower-of-hanoi pack:prove
```

The proof command needs a built `worldstreamctl` binary. It checks the candidate
through the production Component Host. It does not approve the Pack for an
existing installation.

## Run the local demonstration

The live demonstration invokes an installed provider CLI. Configure that provider
before use. The default checks below use fixtures and do not require a provider.

```sh
uv run --project sdk/python python -m examples.tower_of_hanoi.local_harness \
  --community-demo --canvas-demo-hold-seconds 30
```

The command creates five solver participants and a four-disk puzzle. It prints a
loopback URL after the Room, Genesis Replay, and observer are ready. The game
has a 300-second limit. If solvers do not accept completion, the result is
`unresolved` with `demo_time_limit`.

Use `--disks` for 1 through 10 disks. Use `--help` for provider and timing options.
Omit `--canvas-demo-hold-seconds` to keep the completed view open until Ctrl-C.

Each supervisor claims Pack-issued Activations and starts a bounded provider
invocation. The snapshot mode reads a fresh Projection. The stream mode uses
Invocation Context at the claimed Head and an acknowledged observation cursor.
The participant selects a move, completion claim, or assessment.

The supervisor renews its lease during execution. Lease loss or timeout stops
the invocation process group. A stale Action requires a fresh observation and
a new decision. The harness does not choose puzzle Actions for participants.

The canvas has an actionless observer credential. It sends public board state
and selected operational status to the browser through server-sent events.
It does not send bearers, private credential files, or raw protocol frames.

## Optional comparison applications

These applications compare provider policies in two independent Rooms. They are
retained experiments, not controlled measures of model performance.

| Module | Local URL | Behavior |
| --- | --- | --- |
| `examples.tower_of_hanoi.comparison_canvas` | `http://127.0.0.1:5191/` | A selected model and a TypeSafe JEV participant |
| `examples.tower_of_hanoi.comparison_canvas_v2` | `http://127.0.0.1:5291/` | The same model in both Rooms, with a JEV verifier on one side |

Start a module with `uv run --project sdk/python python -m MODULE`.
The applications require prebuilt WorldStream binaries and provider
configuration. They use `JEV_API_KEY` and, for OpenRouter, `OPENROUTER_API_KEY`.
Keys stay in the server process. Provider availability and model behavior can
change. The repository does not maintain those external integrations.

The comparison applications share a seed and start barrier. They retain
separate Room state, participant authority, and outcomes. Pause and end controls
affect application processes. They do not add a generic pause operation to the
WorldStream protocol.

`--fast-start` skips the Rust build and Pack proof and reuses a disposable
Component cache. Use it only after a full build and proof. Original Component
bytes remain authoritative; the cache is not a release artifact.

## Local checks

```sh
uv run --project sdk/python python -m pytest examples/tower_of_hanoi
```

These tests check the harness, participant context, stream decisions, and canvas
controls. They do not measure provider quality or prove mathematical agreement.
The [Pack](../packs/tower-of-hanoi/README.md) defines the exact rules and revisions.
