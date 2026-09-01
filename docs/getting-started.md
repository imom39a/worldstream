# Getting started

WorldStream is a multi-language runtime workspace. It does not have one
command that starts every component. Choose the entry point that matches what
you want to do:

| Goal | Entry point |
| --- | --- |
| Run the real service locally | `worldstreamd` with bundled SQLite |
| Operate the service through Studio | Studio portal plus the local Supervisor |
| Learn Room behavior without starting services | Agent Heist offline story |
| Exercise the real human-and-agent boundaries | Agent Heist MVP acceptance |
| Build and prove a product-shaped official Pack | WorldStream Negotiate |
| Inspect configuration or storage | `worldstreamctl` |
| Launch a first-party browser client | Client Host (`pnpm ui:dev`) |
| Exercise the public client | Python SDK |
| Check a change | fast gate or full verification script |

## Prerequisites

The checkout pins these versions:

| Tool | Version | Source of truth |
| --- | --- | --- |
| Rust | 1.97.1 | `rust-toolchain.toml` |
| Node.js | 24.18.1 | `.node-version` and `.nvmrc` |
| pnpm | 11.19.0 | `package.json` |
| Python | 3.14.7 | `.python-version` |
| uv | 0.12.5 | `.uv-version` |

Install `bash` and `curl` as well. Docker and PostgreSQL client tools are only
needed for PostgreSQL, OCI, and release-specific checks.

On a new workstation, install `rustup` from the
[official Rust installer](https://rust-lang.org/install.html), install the exact
Node 24.18.1 signed build from the
[Node.js v24 archive](https://nodejs.org/en/download/archive/v24), then install
the remaining pinned tools:

```sh
rustup toolchain install 1.97.1 --profile minimal \
  --component rustfmt --component clippy
corepack enable
corepack install --global pnpm@11.19.0
curl -LsSf https://astral.sh/uv/0.12.5/install.sh \
  -o /tmp/worldstream-uv-install.sh
less /tmp/worldstream-uv-install.sh
sh /tmp/worldstream-uv-install.sh
uv python install 3.14.7
```

Inspect downloaded installers before execution. Astral documents the
[versioned uv installer](https://docs.astral.sh/uv/getting-started/installation/)
and [managed Python versions](https://docs.astral.sh/uv/guides/install-python/).
Open a new shell if an installer updated `PATH`, and confirm that every command
reports the exact version in the table before continuing.

From the repository root, install the locked Python and JavaScript
dependencies:

```sh
uv sync --project sdk/python --locked --python 3.14.7
pnpm install --frozen-lockfile
```

Rust dependencies are resolved automatically by the locked Cargo commands
below. Shell harnesses invoke `python3` directly. If your version-manager shim
does not expose the pinned interpreter, use the uv-created environment in the
current shell:

```sh
export PATH="$PWD/sdk/python/.venv/bin:$PATH"
```

## Choose the right starter activity

WorldStream is the Room Runtime; Agent Heist and WorldStream Negotiate are
Activity Packs that exercise it for different purposes. Each Room pins exactly
one Pack revision for its complete lineage.

| Activity | Use it for | What it proves |
| --- | --- | --- |
| Agent Heist | Learning and visual/system conformance | Private views, typed Actions, timers, Attention, external-agent Activation, reconnect, Recovery, and Replay |
| WorldStream Negotiate | The first serious public Pack and Pack-authoring reference | Four independent Roles, exact Human approval, signed protocol objects, deadlines, privacy, deterministic restart, and an auditable agreement Outcome |
| Counter | Internal tutorial and focused conformance | The smallest mechanics-only path; it is not the product story |

Start with Agent Heist when you want to understand the runtime visually or
without external commercial data. Start with Negotiate when you want to inspect
the portable TypeScript Pack workflow or the first product-shaped application.
Neither example hosts an LLM: models, prompts, tools, private memory,
credentials, and strategy stay in application-owned Runners.

## Run the daemon with SQLite

The checked-in `config/development.toml` binds the service to loopback and
stores local state in `.worldstream/`. That directory is ignored by Git.

Build the daemon and operator CLI:

```sh
cargo build --locked -p worldstream-server --bins
```

Create the local data directory and one random 32-byte bootstrap secret:

```sh
umask 077
mkdir -p .worldstream/data
if [ ! -e .worldstream/authority.secret ]; then
  head -c 32 /dev/urandom > .worldstream/authority.secret
fi
chmod 600 .worldstream/authority.secret
```

The secret file contains raw bytes, not hexadecimal text and not a trailing
newline. The setup command reuses an existing secret instead of overwriting it.
Keep the same secret with the same database. WorldStream persists only its hash
and intentionally rejects a different secret on a later start. This binds
authority identity; it does not encrypt the SQLite file. Keep the directory
owner-only and use host-volume encryption when data-at-rest encryption is
required.

Validate the configuration and inspect the local environment:

```sh
target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamctl --config config/development.toml config effective
target/debug/worldstreamctl --config config/development.toml doctor
```

Secret paths and values are redacted from effective configuration output.
`doctor` is a non-mutating preflight, so its storage field can remain
`not_initialized`; use the live `/readyz` endpoint after startup to establish
runtime readiness.

Start the service in the foreground:

```sh
RUST_LOG=info target/debug/worldstreamd --config config/development.toml
```

The daemon writes structured JSON logs. Stop it with `Ctrl-C`.

In a second terminal, inspect the live endpoints:

```sh
curl -fsS http://127.0.0.1:9410/healthz
curl -fsS http://127.0.0.1:9410/readyz
curl -fsS http://127.0.0.1:9410/version
target/debug/worldstreamctl --config config/development.toml health
```

- `/healthz` proves that the process is serving requests.
- `/readyz` proves that the selected store, authority, writer, and scheduler
  are ready.
- `/version` reports the embedded compatibility and selected engine identity.

The configuration layer order is defaults, TOML file, environment, then CLI
flags. Use `--bind`, `--data-dir`, or `--storage-profile` for non-secret local
overrides. Use a secret file or inherited handle for authority and PostgreSQL
credentials; do not put credentials in command-line arguments.

## Run WorldStream Studio

WorldStream Studio is the local operator portal. It is served separately from
the first-party Client Host and talks to a bounded local Supervisor API. The
Supervisor observes and controls the configured `worldstreamd` process;
`worldstreamd` remains the only authoritative Room runtime.

The default local ports are:

| Component | Address | Purpose |
| --- | --- | --- |
| WorldStream daemon | `http://127.0.0.1:9410` | Authoritative runtime and operator API |
| Studio Supervisor | `http://127.0.0.1:9420` | Local typed control-plane API |
| Client Host | `http://127.0.0.1:5173` | First-party Agent Heist client and generic Inspector |
| Studio portal | `http://127.0.0.1:5174` | Operator UI |

### Studio setup

Studio uses the same `config/development.toml`, `.worldstream/data`, and
authority secret as `worldstreamd`. On a fresh local Studio start, the launcher
creates the configured owner-only 32-byte bootstrap source before either
daemon or Studio state exists. To start `worldstreamd` by itself, use the
authority-secret setup in [Run the daemon with SQLite](#run-the-daemon-with-sqlite)
first. Then build the daemon and all Supervisor helper binaries:

```sh
cargo build --locked -p worldstream-server --bin worldstreamd
cargo build --locked -p worldstream-studio-supervisor --bins
```

The second command is important on a fresh checkout: managed and external
agent workflows require `worldstream-assignment-mcp`, and the managed reference
host additionally uses `worldstream-managed-agent-host`.

Studio keeps its own owner-only operational state under
`.worldstream/studio/`. This contains drafts, setup progress, opaque credential
references, Runner state, attention history, and backup-operation records. It
is local development state and is ignored by Git. Do not copy it between
machines or commit it.

On its first start, the Supervisor reads the configured owner-only bootstrap
authority and imports it behind one kind-bound opaque Host authority reference
in that state. The raw authority never enters a browser request, URL, process
argument, or Studio diagnostic. Later starts require the configured bootstrap
authority and verify that it still matches the retained reference.

Runner Templates are optional. If you need supervised Runners, place reviewed
manifest files in `config/runner-templates/` before starting Studio. The
manifest format and protected-secret workflow are documented in
`docs/studio.md`. Studio starts normally with an empty Runner Template catalog.

### Start the operator portal and Supervisor

For daemon operations, Room inventory, drafts, backups, and attention without
a live Activity Client, run this from the repository root:

```sh
scripts/studio-dev.sh \
  --studio-origin http://127.0.0.1:5174 \
  --participant-console-origin http://127.0.0.1:5173
```

This command builds `worldstreamd` and all required Supervisor helpers, starts
the Supervisor on port `9420`, and starts the Studio Vite server on port
`5174`. It stops before Vite starts if protected Supervisor startup fails. It
does not immediately start `worldstreamd`, and it does not start the Client
Host. Keep the command running in its terminal.

`pnpm studio:dev` invokes the same `scripts/studio-dev.sh` launcher without
additional Supervisor arguments. The explicit form above is the canonical
complete-development command because it makes the Studio and Client Host
origins explicit. The `--participant-console-origin` option retains its wire
compatibility name.

Open <http://127.0.0.1:5174>, select **Operations**, and click **Start daemon**.
Alternatively, leave a daemon started by the previous section running; Studio
will discover it through the Supervisor. Do not start a second daemon against
the same `127.0.0.1:9410` listener.

The expected healthy path is:

1. Studio loads at `http://127.0.0.1:5174`.
2. Operations shows the Supervisor as reachable.
3. Start the daemon if it is stopped.
4. Daemon health becomes `healthy`, then readiness becomes `ready`.
5. Build and Operations surfaces can now load the live Activity Pack catalog,
   Room inventory, drafts, setup state, backups, and attention items.

Stop the Studio development stack with `Ctrl-C`. The script also stops its
Supervisor child, but it does not delete `.worldstream/` state.

### Add the Client Host when needed

The Client Host is not required for Studio Operations, daemon health, Activity
Pack inspection, Room administration, backups, or agent setup. It is required
for a complete browser Activity Client handoff.

Keep the Studio launcher above running in the first terminal. In a second
terminal, start the first-party Client Host:

```sh
pnpm ui:dev
```

Open Studio at <http://127.0.0.1:5174>. When Task setup has provisioned a human
seat, **Open participant client** creates a short-lived, one-use handoff and
opens the selected Activity Client at `http://127.0.0.1:5173`. The Room ID,
Membership ID, and participant bearer are not placed in the URL or copied by
the operator. Studio is only the operator-side launcher; it neither owns nor
renders the participant UI.

The Supervisor selects a closed path from the Room's exact Pack identity:

- exact first-party Agent Heist `0.1.0` and `0.2.0` revisions open
  `/agent-heist/`;
- every other Pack revision, including Counter and the current Studio-opened
  Negotiate flow, opens the Pack-neutral `/inspector/` fallback.

Selection requires the exact Pack ID, version, and digest. A matching name or
version with different bytes does not select the Agent Heist client.

Keep both origins as exact loopback origins. If you change either Vite port,
pass the corresponding new origin to the Supervisor. Do not expose the
development servers or Supervisor on a public interface.

### Run Studio components separately

For debugging, the Supervisor and portal can be started independently:

```sh
pnpm studio:supervisor
pnpm --dir web/studio dev
```

Run those commands in separate terminals. The Studio Vite server proxies
`/api` requests to `http://127.0.0.1:9420`. Useful Studio-only checks are:

```sh
pnpm studio:test
pnpm --dir web/studio lint
pnpm studio:build
```

For non-default paths or ports, inspect the Supervisor options with:

```sh
target/debug/worldstream-studio-supervisor --help
```

The most commonly changed options are `--bind`, `--daemon`,
`--daemon-executable`, `--daemon-config`, `--state-dir`,
`--runner-templates-dir`, `--studio-origin`, and
`--participant-console-origin`. These are fixed when the Supervisor starts;
browser requests cannot supply executable paths, commands, environment values,
or daemon credentials.

## Run the first-party Client Host

This is the browser process that serves first-party Activity Clients and the
generic WorldStream Inspector. It is not the Studio operator portal. Start its
Vite development server with:

```sh
pnpm ui:dev
```

The server binds to `http://127.0.0.1:5173`. Its bare root retains the old
direct-bootstrap and fixture renderer for compatibility and development only;
do not treat that surface as a Pack catalog, a Room creator, or the current
Studio launch flow. The normal live Human path starts in Studio:
**Open participant client** issues a short-lived one-use handoff, the selected
client redeems it into a retained HttpOnly participant session, and the browser
removes the handoff from the URL.

The current closed dispatch is:

| Path | Surface | Selection |
| --- | --- | --- |
| `/agent-heist/` | Agent Heist Activity Client | Exact approved `worldstream.agent-heist` `0.1.0` or `0.2.0` identity |
| `/inspector/` | Pack-neutral WorldStream Inspector | Every unsupported exact Pack identity |

The live Agent Heist client starts without Activity data and installs only an
authorized Projection Reset or Observation. It never overlays live data onto a
recorded fixture. The Inspector displays the authorized generic Projection and
Action Offers; it is a fallback client, not a Pack-defined experience. During
this migration it also retains the existing Negotiate-specific renderer after
an authorized delivery is recognized. That compatibility renderer is not a
standalone registered Negotiate Activity Client.

A direct application integration may instead supply a Room ID, Membership ID,
scoped bearer, and explicit `window.__WORLDSTREAM_LIVE_SESSION__` bootstrap.
The dedicated direct Negotiate renderer additionally requires the one-shot
`window.__WORLDSTREAM_NEGOTIATE_CONSOLE__` authorized projection. The legacy
root renderer consumes the two bootstraps separately and requires their Room
and Membership identities to match; neither is a substitute for the other. The
bare-root fixture and direct-bootstrap paths remain compatibility/development
surfaces, not Studio-selected Activity Clients and not the model for new
integrations. See
`web/console/src/participantHandoff.ts`, `web/console/src/liveSession.ts`,
`web/console/src/negotiate.ts`, and the protocol documentation before
integrating either boundary.

Useful UI commands are:

```sh
pnpm ui:test
pnpm ui:lint
pnpm ui:build
```

## Explore Agent Heist

Agent Heist is the visual demo and conformance Pack, not WorldStream's primary
product application. It is useful because privacy, absent agents, timers,
reconnect, and Replay are easy to see in one bounded story.

### Open the recorded visual demo

Start the public demo catalog with:

```sh
pnpm demos:dev
```

Open `http://127.0.0.1:5180/demos/agent-heist/`. This is a recorded,
no-authority, no-network story. It reuses the Agent Heist presentation exported
by the Activity Client package, but feeds it through a separate recorded
adapter. It cannot redeem a handoff, attach to a Room, or submit an Action.

### Run the offline story

For a quick offline demo/conformance story that needs no daemon, database,
browser, model, or network connection, run:

```sh
uv run --project sdk/python --python 3.14.7 python examples/heist/run_story.py --self-test
uv run --project sdk/python --python 3.14.7 python -m unittest discover -s examples/heist -p 'test_*.py'
```

This validates the deterministic retained Agent Heist 0.1.0 fixture and Replay
corpus. It is intentionally not live service evidence.

The selected `service_window` story has matching Navigator and Insider
commitments. Its immutable enabled Agent Broker Membership submits no
commitment. The result is therefore a strict two-of-three majority rather than
a fabricated Broker Invocation.

### Run the real-process MVP story

The MVP uses the selectable Agent Heist 0.2.0 Lobby revision, not the retained
0.1.0 offline fixture. Navigator and Insider are required, Broker is optional,
and Studio submits the host-only `host_launch` external input only after the
required seats are ready.

The full acceptance starts disposable SQLite state and crosses the production
daemon, Studio Supervisor, assignment MCP, Agent Heist Activity Client, HTTP,
WebSocket, and stdio boundaries. It creates one reviewed Room with one Human
Navigator and one external Agent Insider, explicitly launches it, restarts the
assignment helper during an Activation lease, reaches a meaningful Outcome,
and verifies the final committed lineage through Replay.

Run the fast acceptance-report contract independently with:

```sh
uv run --project sdk/python --python 3.14.7 python -m unittest \
  examples/heist/mvp_live/test_run_mvp_acceptance.py
```

The live path additionally requires the repository's exact pinned
Chrome-for-Testing identity. Its complete setup and direct command are
documented in
[`mvp-agent-heist-acceptance.md`](mvp-agent-heist-acceptance.md). A missing
browser, binary, loopback port, or proof produces a bounded blocked result; it
is never reported as a completed MVP. Without `--live`, the acceptance runner
only validates an already completed report; it does not create one.

## Build and prove WorldStream Negotiate

`worldstream.negotiate` is the first serious official Pack and the reference
for the public TypeScript Pack-authoring workflow. Its successful golden path
uses exactly four Memberships—buyer agent, seller agent, Human buyer approver,
and venue signer—and commits nine Actions with one mandatory persisted
checkpoint:

`buyer proposal → seller counter → approval request → exact Human approval →
[persisted restart and reconnect] → buyer acceptance → operated selection →
buyer signature → seller signature → agreement commitment`.

Build the public Pack SDK/CLI and production operator CLI first, then check,
test, build, inspect, and prove the Pack through that toolchain:

```sh
pnpm pack:build
cargo build --locked -p worldstream-server --bin worldstreamctl
pnpm --filter @worldstream/official-negotiate pack:check
pnpm --filter @worldstream/official-negotiate pack:test
pnpm --filter @worldstream/official-negotiate pack:build
pnpm --filter @worldstream/official-negotiate pack:inspect
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  pnpm --filter @worldstream/official-negotiate pack:prove
```

This authoring sequence writes and proves the mutable
`worldstream-negotiate-candidate.wspack`. It never overwrites the retained,
digest-named official release. Review the exact immutable release identity in
[`packs/negotiate/README.md`](../packs/negotiate/README.md) before treating any
bundle as an approved release subject.

`pack:test` validates the TypeScript Pack against the independently authored,
machine-readable oracle corpus, including restart behavior, semantic
rejections, the six-persona visibility matrix, and all 30 ordered privacy
mutations. Run the independent Rust oracle itself with:

```sh
cargo test --locked -p worldstream-negotiate-oracle --test conformance
```

`pack:prove` sends the generated WASI-free Component Bundle through the
production Bundle Verifier, Component Host, and unchanged Core admission path.
Run the offline evidence verifier separately:

```sh
cargo test --locked -p worldstream-negotiate-evidence --test offline
```

Together these commands prove deterministic Component execution, the
independent semantic model, and offline proof verification. They do not claim
a persisted live-Room daemon restart or production A202 conformance. The
current immutable bundle identity and detailed scope are recorded in
[`packs/negotiate/README.md`](../packs/negotiate/README.md), while
[`negotiate.md`](negotiate.md) owns the normative behavior and compatibility
boundary.

These author commands also do not install a Pack into a Runtime. To make an
exact reviewed bundle selectable, stop `worldstreamd` and use one configuration
for the complete offline Host Operator lifecycle: inspect the bytes, approve
them, install them retained-only, inspect inventory, set the physical bundle
digest selectable, inspect inventory again, run `pack restart-readiness`, and
only then restart the daemon. The exact commands and fail-closed storage rules
are in [`activity-packs.md`](activity-packs.md#offline-operator-lifecycle).

## Use the Python SDK

The SDK is an asynchronous client for an already provisioned Room Membership:

```python
from worldstream_sdk import Client

client = Client("http://127.0.0.1:9410", bearer)
room = await client.open_room(room_id, member_id)

for frame in await room.sync():
    # Process the authorized frame durably before advancing its cursor.
    await room.ack(frame["frame_seq"])
```

Run SDK commands through its locked environment:

```sh
uv run --project sdk/python --python 3.14.7 pytest
uv run --project sdk/python --python 3.14.7 ruff check
uv run --project sdk/python --python 3.14.7 ruff format --check
```

The bootstrap authority is not a participant session by itself. A live SDK or
browser session also needs a Room, Membership, and appropriately scoped
capability created through the authority API. The exact HTTP and WebSocket
messages are documented in `docs/protocol.md`.

## Test the repository

Choose the smallest useful verification tier:

### Fast local gate

```sh
scripts/gates.sh fast
```

This checks manifest/source drift, formatting, focused Rust lint and tests,
lock resolution, goldens, and tracked-file secret patterns. The tier has a
hard 60-second budget; a cold or slower checkout can time out while compiling,
so use the component commands below when warming or diagnosing the workspace.

### Real daemon smoke test

```sh
cargo build --locked -p worldstream-server --bins
scripts/smoke-operator.sh
```

The smoke test creates temporary SQLite state and authority material, starts
the real daemon, probes health/readiness/version, and removes its temporary
files. On Linux, `scripts/restart-smoke.sh` additionally verifies a bounded
stop/restart cycle.

### Component suites

```sh
cargo test --workspace --locked
uv run --project sdk/python --python 3.14.7 pytest
pnpm ui:test
```

### Full checkout verification

```sh
scripts/verify-local.sh
```

This is the broad developer check. It enforces the exact tool versions, syncs
locked dependencies, checks formatting and lint, builds and tests the Rust
workspace, validates compatibility data, tests the Python SDK and UI, builds
the UI, and runs the operator smoke test. Expect it to take substantially
longer than the fast gate.

The stricter pre-push gate is documented in `docs/gates.md`:

```sh
scripts/gates.sh pre-push --strict
```

## Reset local development state

Stop the daemon first. If the local database is disposable, remove
`.worldstream/` and repeat the secret-generation steps. Do not replace only
the secret while keeping the database; that correctly fails authority
bootstrap.

## Common problems

- **Pinned Python is missing:** let `uv sync --python 3.14.7` install or select
  it instead of relying on an unrelated system `python3`.
- **The port is already in use:** pass the same alternative bind to daemon and
  control commands, for example `--bind 127.0.0.1:9510`.
- **Studio Host authority startup fails:** restore access to the configured
  owner-only bootstrap secret and the matching `.worldstream/studio/` state,
  then restart. The Supervisor will not replace retained authority state.
- **The bare Client Host shows the old fixture:** the root renderer remains a
  compatibility/development path. It is not a Studio-launched client and does
  not provision a Room or inject a capability. Use Studio's **Open participant
  client** action for a live session, or open the recorded demo at port `5180`
  for the no-authority story.
- **Negotiate opens the generic Inspector:** this is the current intended
  Studio handoff. Negotiate has no registered standalone Activity Client in
  this milestone. The Inspector may select the retained Negotiate-specific
  compatibility renderer after it reads an authorized delivery. The separate
  direct-bootstrap path still requires a one-shot
  `window.__WORLDSTREAM_NEGOTIATE_CONSOLE__` projection plus a matching
  ordinary live session.
- **A Negotiate `pack:*` command cannot find `worldstream-pack`:** run
  `pnpm pack:build` from the repository root after the locked install.
- **Negotiate proof cannot find `worldstreamctl`:** build the operator CLI with
  `cargo build --locked -p worldstream-server --bin worldstreamctl`, then rerun
  the proof with
  `WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl"`. Filtered `pnpm`
  scripts run from the Pack directory, so the host path must be absolute.
- **Agent Heist live acceptance is blocked:** use the exact browser identity
  inputs documented in `docs/mvp-agent-heist-acceptance.md`; a fixture browser
  or an unpinned local Chrome does not satisfy the live gate.
- **Studio cannot start on a fresh checkout:** build all
  `worldstream-studio-supervisor` binaries first; the Supervisor requires the
  fixed assignment MCP helper to exist.
- **Studio shows the daemon as unavailable:** start it from Studio Operations,
  or start `worldstreamd` separately with `config/development.toml`. Then check
  `http://127.0.0.1:9410/healthz` and `/readyz`.
- **Participant handoff is rejected:** confirm the Client Host is on port
  `5173`, Studio is on `5174`, and the Supervisor was started with those
  exact `--participant-console-origin` and `--studio-origin` values.
- **A Studio or Supervisor port is already in use:** stop the previous
  development process or choose new loopback ports and update the matching
  Supervisor origin/bind arguments.
- **A PostgreSQL or release check is incomplete:** install the optional tools
  and provide the owner-only inputs described in `docs/gates.md` and
  `docs/operator-storage.md`.
