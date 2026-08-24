# Agent Heist MVP completion gate

The WorldStream MVP is complete only when the live IMO-78 gate finishes with a
`completed` report. Unit, fixture, or in-process adapter tests are supporting
evidence; they do not replace this gate.

## Required story

One run starts from an empty disposable SQLite profile and uses the production
boundaries to create exactly one reviewed Task with exactly two required seats:

- one human `navigator`, opened through a Studio-issued one-use Participant
  Console handoff;
- one external-agent `insider`, opened through a Supervisor-issued opaque
  assignment-MCP launch reference.

The Task remains in the Lobby until both live readiness signals are observed
and the operator explicitly launches it. The human and external agent then use
only currently offered Agent Heist Actions. The helper is stopped while an
Activation lease is outstanding and restarted with the same opaque launch
reference. It must resume the exact lease, complete it once, and return the
same retained receipt for the identical retry. The story must reach the
committed `complete` phase with a non-empty Outcome.

The gate obtains authoritative Room inventory before and after the story. The
delta must be exactly one Room. It obtains Replay at the final committed Room
sequence and requires equality of the live and replayed Activity projection,
Outcome, final Transition lineage hash, Activity-state hash, and
authoritative-state hash. The Transition hash proves Replay reached the
identical committed lineage rather than only an equivalent-looking projection.
Stable Action and Activation completion retries must not add a second
transition.

## Boundaries exercised

The release run uses separate real processes and production protocols:

- `worldstreamd`: HTTP and WebSocket APIs over a disposable durable SQLite
  data directory;
- `worldstream-studio-supervisor`: reviewed draft, Room creation, Task setup,
  Participant handoff, explicit launch, and assignment launch over HTTP;
- `worldstream-assignment-mcp`: MCP JSON-RPC over stdio, with only the opaque
  launch reference and owner-only state directory on its command line;
- Participant Console: the production build served over loopback and opened by
  the repository's pinned CDP browser adapter using the real fragment-only
  handoff URL. The gate waits for the live authorized projection and proves the
  fragment was scrubbed;
- Studio: the same strict browser-facing HTTP DTOs used by the production
  client. No daemon capability is supplied to a browser-facing request.

## Running the gate

Build the three Rust binaries and the production Console first:

```sh
cargo build -p worldstream-server --bin worldstreamd
cargo build -p worldstream-studio-supervisor \
  --bin worldstream-studio-supervisor --bin worldstream-assignment-mcp
npm --prefix web/console run build
```

Install the locked Python environment used by the pinned CDP adapter, then
configure its exact browser identity variables. The source quickstart accepts
the complete pin as explicit arguments and verifies the frozen version, binary
and archive identities before running the same adapter:

```sh
uv sync --project sdk/python --locked --python 3.13.0
scripts/macos-source-quickstart.sh \
  --browser /absolute/path/to/chrome-headless-shell \
  --browser-version 152.0.7977.54 \
  --browser-sha256 4e0c165ef2f0d7265fb1e6b3df2d03d1d6581fb72cdfcebeac19c09760571df6 \
  --browser-size-bytes 167333040 \
  --browser-archive-url https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-arm64/chrome-headless-shell-mac-arm64.zip \
  --browser-archive-sha256 ef5d61434f13d9d2d9bdc7c9ab4bff92225979e196458cf846640862b25f127d \
  --browser-archive-size-bytes 98034515
```

The x86_64 pin is closed separately in that script. With the seven
`WORLDSTREAM_BROWSER_*` variables exported, run:

```sh
PYENV_VERSION=3.13.0 python3 \
  examples/heist/mvp_live/run_mvp_acceptance.py \
  --live --report target/agent-heist-mvp-acceptance.json
```

CI can run the fast report-contract gate independently:

```sh
PYENV_VERSION=3.13.0 python3 -m unittest \
  examples/heist/mvp_live/test_run_mvp_acceptance.py
PYENV_VERSION=3.13.0 python3 \
  examples/heist/mvp_live/run_mvp_acceptance.py \
  --report target/agent-heist-mvp-acceptance.json
```

The live command exits `2` with one bounded `blocked:<reason>` when a build,
pinned browser, loopback port, production component, or authoritative proof is
unavailable. A blocked report is never treated as MVP completion.

## Credential and private-data rule

The disposable Host, Membership, Runner, replay, handoff, session, and launch
material exists only in owner-readable temporary storage or in its authorized
transport. The retained report contains none of it. Before publishing a
`completed` report, the gate scans Studio HTTP evidence, Participant Console
browser evidence, MCP traffic, and process logs for raw capability/token
patterns and exact private values. MCP may carry the assigned agent's private
Activation context, but never a Host, Membership, or Runner bearer. Browser and
operator evidence may carry neither.

## Explicit non-goals

This is a single-machine MVP completion gate, not a throughput, multi-Room,
PostgreSQL failover, managed-model-provider, public-network, or multi-browser
test. Those profiles retain their own conformance and operational gates. The
test does not inspect or modify SQLite directly, fabricate an Action Offer,
Activation, timer payload, Room transition, Replay digest, or credential.
