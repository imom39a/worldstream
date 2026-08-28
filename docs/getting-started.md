# Getting started

WorldStream is a multi-language runtime workspace. It does not have one
command that starts every component. Choose the entry point that matches what
you want to do:

| Goal | Entry point |
| --- | --- |
| Run the real service locally | `worldstreamd` with bundled SQLite |
| Operate the service through Studio | Studio portal plus the local Supervisor |
| Inspect configuration or storage | `worldstreamctl` |
| Explore the reference interface | Vite web console in fixture mode |
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
head -c 32 /dev/urandom > .worldstream/authority.secret
chmod 600 .worldstream/authority.secret
```

The secret file contains raw bytes, not hexadecimal text and not a trailing
newline. Keep the same secret with the same database. WorldStream persists
only its hash and intentionally rejects a different secret on a later start.
This binds authority identity; it does not encrypt the SQLite file. Keep the
directory owner-only and use host-volume encryption when data-at-rest
encryption is required.

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
the Participant Console and talks to a bounded local Supervisor API. The
Supervisor observes and controls the configured `worldstreamd` process;
`worldstreamd` remains the only authoritative Room runtime.

The default local ports are:

| Component | Address | Purpose |
| --- | --- | --- |
| WorldStream daemon | `http://127.0.0.1:9410` | Authoritative runtime and operator API |
| Studio Supervisor | `http://127.0.0.1:9420` | Local typed control-plane API |
| Participant Console | `http://127.0.0.1:5173` | Human participant view and fixture console |
| Studio portal | `http://127.0.0.1:5174` | Operator UI |

### Studio setup

Complete the SQLite data-directory and authority-secret setup in
[Run the daemon with SQLite](#run-the-daemon-with-sqlite) first. Studio uses
the same `config/development.toml`, `.worldstream/data`, and authority secret.
Then build the daemon and all Supervisor helper binaries:

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

Runner Templates are optional. If you need supervised Runners, place reviewed
manifest files in `config/runner-templates/` before starting Studio. The
manifest format and protected-secret workflow are documented in
`docs/studio.md`. Studio starts normally with an empty Runner Template catalog.

### Start the operator portal

For daemon operations, Room inventory, drafts, backups, and attention without
a live Participant Console, run this from the repository root:

```sh
pnpm studio:dev
```

This command builds `worldstreamd`, starts the Supervisor on port `9420`, and
starts the Studio Vite server on port `5174`. It does not immediately start
`worldstreamd`. Open <http://127.0.0.1:5174>, select **Operations**, and start
the configured daemon there. Alternatively, leave a daemon started by the
previous section running; Studio will discover it through the Supervisor.

The expected healthy path is:

1. Studio loads at `http://127.0.0.1:5174`.
2. Operations shows the Supervisor as reachable.
3. Start the daemon if it is stopped.
4. Daemon health becomes `healthy`, then readiness becomes `ready`.
5. Build and Operations surfaces can now load the live Activity Pack catalog,
   Room inventory, drafts, setup state, backups, and attention items.

Stop the Studio development stack with `Ctrl-C`. The script also stops its
Supervisor child, but it does not delete `.worldstream/` state.

### Start Studio with the Participant Console

A complete human handoff uses both Vite applications. Start the Participant
Console in the first terminal:

```sh
pnpm ui:dev
```

In a second terminal, start Studio and bind the browser origins to their local
development ports explicitly:

```sh
scripts/studio-dev.sh \
  --studio-origin http://127.0.0.1:5174 \
  --participant-console-origin http://127.0.0.1:5173
```

Open Studio at <http://127.0.0.1:5174>. When Task setup has provisioned a human
seat, **Open Participant View** creates a short-lived, one-use handoff and opens
the Participant Console at `http://127.0.0.1:5173`. The Room ID, Membership ID,
and participant bearer are not placed in the URL or copied by the operator.

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

## Run the web console

This is the Participant/reference console, not the Studio operator portal.
Start its Vite development server with:

```sh
pnpm ui:dev
```

Open the local URL printed by Vite, normally `http://localhost:5173`. The
console starts in fixture mode: it demonstrates the Agent Heist public,
participant, operator, and replay surfaces without making a network request.
It does not automatically discover the daemon or create a Room.

A live console session requires an application-supplied Room ID, Membership
ID, scoped bearer, and explicit `window.__WORLDSTREAM_LIVE_SESSION__`
bootstrap. See `web/console/src/liveSession.ts` and the protocol documentation
before integrating that boundary.

Useful UI commands are:

```sh
pnpm ui:test
pnpm ui:lint
pnpm ui:build
```

## Run an offline reference story

For a quick product-level demonstration that needs no daemon, database,
browser, model, or network connection, run:

```sh
uv run --project sdk/python --python 3.14.7 python examples/heist/run_story.py --self-test
uv run --project sdk/python --python 3.14.7 python -m unittest discover -s examples/heist -p 'test_*.py'
```

This validates the deterministic absent-Broker Agent Heist fixture and Replay
corpus. It is intentionally not live service evidence.

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
- **Authority bootstrap fails after a restart:** restore the original secret
  file, or reset both the secret and disposable `.worldstream/data` together.
- **The UI shows fixture data:** this is the default design. Running Vite does
  not provision a Room or inject a capability.
- **Studio cannot start on a fresh checkout:** build all
  `worldstream-studio-supervisor` binaries first; the Supervisor requires the
  fixed assignment MCP helper to exist.
- **Studio shows the daemon as unavailable:** start it from Studio Operations,
  or start `worldstreamd` separately with `config/development.toml`. Then check
  `http://127.0.0.1:9410/healthz` and `/readyz`.
- **Participant handoff is rejected:** confirm the Participant Console is on
  port `5173`, Studio is on `5174`, and the Supervisor was started with those
  exact `--participant-console-origin` and `--studio-origin` values.
- **A Studio or Supervisor port is already in use:** stop the previous
  development process or choose new loopback ports and update the matching
  Supervisor origin/bind arguments.
- **A PostgreSQL or release check is incomplete:** install the optional tools
  and provide the owner-only inputs described in `docs/gates.md` and
  `docs/operator-storage.md`.
