# Getting started

This guide is for a person who is new to WorldStream. Start with the command
line. You do not need Studio to run the server or the automated Agent Heist
test.

Complete Sections 1, 2, and 3 in order. Then use the optional browser sections
if you need them.

> **Current state:** The CLI-first direction is accepted. The full replacement
> for Studio is not built yet. This guide uses commands that exist today.
> Background server management, manual Room setup, and browser-client launch
> through the operator CLI are still pending. Studio remains available during
> the transition. See the [implementation plan](cli-first-implementation-plan.md)
> for the approved replacement and its implementation and verification gates.
> The [new CLI reference](cli-reference.md) describes the planned interface.
> Its commands are not usable while they return `not_implemented`.

## Understand the parts

| Part | Purpose |
| --- | --- |
| **Runtime** | The `worldstreamd` process. It stores Room state and applies Activity Pack rules. |
| **Operator CLI** | The `worldstreamctl` command. It checks configuration and manages installed Pack Bundles. |
| **Room** | One durable shared situation governed by one Activity Pack. Humans and agents can be members. |
| **Activity Pack** | The rules for one type of Room. Agent Heist is an Activity Pack. |
| **Activity Client** | A program that connects to a Room with authorized access. It can use a browser, terminal, SDK, or agent integration. |
| **Runner** | An external program that starts agent Invocations. WorldStream does not host the model. |

An operator manages the installation. A participant uses an Activity Client.
These are different jobs. A browser Activity Client does not require an
administration website.

## 1. Prepare the repository

These commands are for a macOS or Linux shell. Run them from the repository
root: the directory that contains `Cargo.toml` and `package.json`.

### Install the tools for your path

The repository pins these versions. Sections 1–3 need Rust, Python, and uv.
Node.js and pnpm are needed only for the optional browser sections.

| Tool | Version | Used for |
| --- | --- | --- |
| Rust | 1.97.1 | Build the Runtime and CLI |
| Python | 3.14.7 | Run the live Heist test |
| uv | 0.12.5 | Supply Python and its dependencies |
| Node.js | 24.18.1 | Optional browser development |
| pnpm | 11.19.0 | Optional browser dependencies and commands |

Install Rust with the [official Rust installer](https://rust-lang.org/tools/install/).
Install uv with the [official uv instructions](https://docs.astral.sh/uv/getting-started/installation/).

Check the tool versions:

```sh
rustc --version
uv --version
uv run --python 3.14.7 python --version
```

Use the exact versions in the table. Some release checks reject other
versions. You do not need Docker, PostgreSQL, or an LLM API key for this guide.

### Install dependencies and build the tools

Run these commands one time after you clone the repository:

```sh
uv sync --project sdk/python --locked --python 3.14.7
cargo build --locked -p worldstream-server --bins
```

The build creates `target/debug/worldstreamd` and `target/debug/worldstreamctl`.
The following steps use these local binaries. You do not need to install them
globally.

## 2. Run and inspect your local server

This section starts or reuses a local installation. It does not create a Room.

### Prepare local state

The supplied `config/development.toml` uses:

- SQLite data in `.worldstream/data`;
- an authority secret in `.worldstream/authority.secret`;
- the local address `127.0.0.1:9410`.

If Studio already manages this installation, stop its daemon through Studio
Operations first. Do not run two daemons against the same local data.

Run this block to prepare a new installation or check that an existing one
still has its secret:

```sh
(
  set -eu
  umask 077
  if [ ! -e .worldstream ]; then
    mkdir -p .worldstream/data
    (set -C; head -c 32 /dev/urandom > .worldstream/authority.secret)
  fi
  if [ ! -s .worldstream/authority.secret ]; then
    echo "Stop: existing local state has no authority secret. Restore the original secret." >&2
    exit 1
  fi
)
```

Continue only if the block succeeds. It creates a secret only for a new
installation. It does not replace an existing secret. Never create a new
secret beside an existing database.

### Check configuration

```sh
target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamctl --config config/development.toml config effective
```

The first command must succeed. The second shows the effective configuration
with secret references redacted. Server configuration is separate from the
Pack-specific configuration used to create a Room.

Environment variables named `WORLDSTREAM__SECTION__KEY` can override the
configuration file. Check the effective values if the paths or address differ
from those listed above.

### Start the server

```sh
target/debug/worldstreamd --config config/development.toml
```

Keep this terminal open. The server runs in the foreground and writes its logs
here.

In a second terminal, from the repository root, check the running server:

```sh
target/debug/worldstreamctl --config config/development.toml health
curl -fsS http://127.0.0.1:9410/readyz
curl -fsS http://127.0.0.1:9410/version
```

The health command checks that the process responds. The readiness check must
also succeed before you use the Runtime.

### Stop the server and inspect installed Bundles

Press `Ctrl-C` in the first terminal. Wait for the server to exit.

Then inspect the local portable Pack inventory:

```sh
target/debug/worldstreamctl --config config/development.toml pack inventory
```

An empty inventory is normal on a fresh checkout. This command lists installed
portable Bundles, not embedded Packs. Agent Heist is embedded in the local
Runtime build, so it does not need a separate Bundle installation.

To start this installation again, run the same `worldstreamd` command. Its
database and authority remain in `.worldstream/`.

The `worldstreamctl server start`, `stop`, and `logs` backends are not
implemented yet. Use the foreground process and its terminal output. For exact Bundle
approval and installation, see [Activity Packs](activity-packs.md).

## 3. Run a real Agent Heist Room

Leave the server from Section 2 stopped. The following test starts and stops
its own server. It uses private temporary SQLite state and an available port,
not your `.worldstream/` installation.

Run the live Agent Heist test:

```sh
uv run --project sdk/python --python 3.14.7 python \
  examples/heist/wave10_live/run_absent_broker_live.py \
  --spawn-daemon \
  --report target/getting-started-heist.json
```

The command can be quiet while the phase timers run. A normal run takes about
three minutes.

The test uses the public HTTP and WebSocket interfaces and the Python SDK.
It creates a Room, submits participant Actions, processes agent Activation
work, and restarts the Runtime. It then completes all six Heist phases, checks
the result with Replay, and stops its server.

The command prints a JSON result. It must contain:

```json
{
  "live_evidence": true,
  "status": "completed"
}
```

The complete result is in `target/getting-started-heist.json`.

This is an automated live test, not a manual activity session. It does not
open a browser, use Studio, or call a paid model API. A complete operator-CLI
flow for creating your own Room and attaching participants is not available
yet. That flow is required before Studio is removed.

The [Agent Heist MVP completion gate](mvp-agent-heist-acceptance.md) covers
additional release boundaries. Its current checks still include Studio. This
guide does not replace those checks or claim release qualification.

## 4. Optional: open the recorded Agent Heist demo

Use the recorded demo to see the Agent Heist browser interface.

### Prepare the browser tools once

Sections 4–6 share these tools. Skip this preparation if you already have the
pinned versions and installed dependencies.

Install Node.js from the [Node.js download archive](https://nodejs.org/en/download/archive/v24).
Then run:

```sh
corepack enable
corepack install --global pnpm@11.19.0
node --version
pnpm --version
pnpm install --frozen-lockfile
```

Node.js must report `24.18.1`. pnpm must report `11.19.0`.

### Open the demo

```sh
pnpm demos:dev
```

Keep the command running. Open:

<http://127.0.0.1:5180/demos/agent-heist/>

You must see the Agent Heist board. Use its controls to inspect the recorded
story.

This is not the Room from Section 3. It uses recorded safe data. It does not
start the Runtime, create credentials, or send Actions to a server.

Stop the demo server with `Ctrl-C`.

## 5. Optional: develop a browser Activity Client

First complete [browser tool preparation](#prepare-the-browser-tools-once).
You do not need to run the recorded demo.

The Client Host is a local web server for independent Activity Clients. Start
it with:

```sh
pnpm ui:dev
```

| Address | Client |
| --- | --- |
| `http://127.0.0.1:5173/agent-heist/` | Agent Heist |
| `http://127.0.0.1:5173/negotiate/` | Negotiate |
| `http://127.0.0.1:5173/inspector/` | Pack-neutral Inspector |

Opening a URL does not create a Room or grant access. Without a participant
session, the client waits for an authorized Projection. This is expected.

The current live browser clients use the Supervisor's session broker. A
normal launch requires provisioned participant access and a one-use handoff.
The new CLI launch command is not implemented yet. The current Studio form
also cannot enter the complete Agent Heist configuration, so this is not a
complete beginner path to a manual Heist session.

These browser clients remain part of WorldStream's ecosystem. Retiring
Studio does not retire them. Stop the Client Host with `Ctrl-C`.

## 6. Optional: use Studio during the transition

Skip this section for the CLI path. Studio remains available while its
replacement is built and verified. It is not required for Sections 2 or 3.
It uses the same [browser tools](#prepare-the-browser-tools-once).

Studio is the current administration website. Its Supervisor is a separate
headless service with process, credential, setup, and browser-session
functions. Removing the website will not mean deleting those functions.

Make sure the foreground server from Section 2 is stopped. Then run:

```sh
pnpm studio:dev
```

This starts the Supervisor at `http://127.0.0.1:9420` and Studio at
`http://127.0.0.1:5174`. It builds the Runtime but does not start it.

Open <http://127.0.0.1:5174/>. Select **Operations**, then **Start daemon**.
Wait until it reports healthy and ready. Use Studio to inspect existing
operator surfaces. Do not expect the current Room form to create a working
Heist browser session.

To stop this stack, select **Stop daemon** first. Then press `Ctrl-C` in the
Studio terminal. See [Studio](studio.md) for the existing interface.

## Common problems

### Existing state has no authority secret

Stop. Do not generate a replacement secret or delete the database. Restore
the original secret from your protected backup. The database is bound to that
authority. If you cannot restore it, use the recovery documentation before
you change local state.

### The server cannot start

Read the server terminal. Check that:

- no other process uses port `9410`;
- commands run from the repository root;
- `config/development.toml` exists;
- the authority secret and database belong to the same installation.

If installed Bundles require a new restart-readiness check, follow
[Activity Packs](activity-packs.md). Do not bypass that check or remove
retained Bundles to make the server start.

### The live Heist test shows no output

Wait for the phase timers. A normal run takes about three minutes. Inspect
`target/getting-started-heist.json` after the command ends.

### The browser client waits for a Projection

A direct client URL has no participant access. Use Section 4 for the recorded
interface or Section 3 for a real automated Room.

### Studio cannot create an Agent Heist Room

The current form does not support all fixed and list-valued configuration
fields. This is a known product limit, not a local setup error. The CLI-first
replacement is pending; repeatedly entering values will not fix the form.

### A tool has the wrong version

Use the versions in Section 1. Open a new terminal after a version manager
changes your `PATH`. Then run the version checks again.

## Keep local state safe

Stop the process with the tool that started it: `Ctrl-C` for a foreground
daemon, or **Stop daemon** for a Studio-managed daemon. Stop each development
web server separately.

The `.worldstream/` directory contains the local database, authority data,
and any Supervisor state. Keep it to continue the same installation.
Removing Studio's web app must not remove this directory.

Do not delete local state as a routine troubleshooting step. See
[Operator storage and transfer](operator-storage.md) for backup and recovery.

## Continue with one subject

| Task | Guide |
| --- | --- |
| Understand the CLI-first replacement and pending work | [Implementation plan](cli-first-implementation-plan.md) |
| Approve and install an exact Pack Bundle | [Activity Packs](activity-packs.md) |
| Build the Negotiate Activity Pack | [Negotiate Pack README](../packs/negotiate/README.md) |
| Build a client or understand handoffs | [Activity Clients](activity-clients.md) |
| Use the Python SDK | [Python SDK README](../sdk/python/README.md) |
| Understand HTTP and WebSocket messages | [Protocol](protocol.md) |
| Select a test or release gate | [Automated gates](gates.md) |
