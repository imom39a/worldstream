# Getting started

## Choose your path

| What you want to do | Start here |
| --- | --- |
| Try the hosted activity site | [Hosted preview support and limits](hosted-preview-support.md) |
| Develop the full activity site locally, with test sign-in and a fake model | [Hosted local development](hosted-local-development.md) |
| Run the WorldStream kernel and independent Activity Clients locally | Continue with this guide |

The hosted activity site and the kernel are different entry points. A person
who joins a hosted match does not run the commands below or configure an LLM
key. The platform operator supplies its House Agents. These commands are for
a developer who wants to operate a local kernel installation.

## Local kernel and Activity Clients

This guide starts one local WorldStream installation from the command line.
It is for a person who has not used WorldStream before.

At the end of the guide, you will have done these tasks:

1. Initialize a protected local installation.
2. Approve and install the Negotiate Activity Pack.
3. Start the WorldStream Controller and Runtime.
4. Start the independent browser Activity Client Host.
5. Submit a proposal to a live Negotiate Room.
6. Open the Agent Heist browser client with authorized Room access.
7. Complete a different Agent Heist Room with the Python SDK.

Sections 5, 6, and 7 are three separate demonstrations:

- Section 5 submits one Negotiate proposal with Python.
- Section 6 opens an Agent Heist browser client. That Room stays in the Lobby.
- Section 7 creates a different Agent Heist Room and completes it with Python.

The demonstrations do not share a Room. You can stop after the demonstration
that you need.

You do not need the Studio web application. The operator CLI starts a small
local Controller as an implementation service. The Controller manages the
Runtime and issues scoped participant connections. It does not provide a web
administration interface.

This is an MVP development flow. It is not a production deployment or a
release-qualification result. You do not need Docker, PostgreSQL, or an LLM
API key.

## Know which commands start processes

WorldStream uses separate core services and Activity Clients. One command does
not start all local processes.

| Terminal | Command | What it starts | How it runs |
| --- | --- | --- | --- |
| Terminal 1 | `worldstreamctl ... server start` | Controller on port `9420` and Runtime on port `9410` | In the background |
| Terminal 2 | `pnpm activity-clients:serve` | Browser Activity Client Host on port `5173` | In the foreground; keep the terminal open |
| Terminal 3 | Python command in Section 7 | Agent Heist SDK example | In the foreground; keep the terminal open |

The `init` command only prepares local files and authority. It starts no
process. Therefore, `http://127.0.0.1:5173/` will not open after `init` or
`server start` alone. Section 4 starts both the core services and the separate
browser Client Host.

## Know the main parts

| Part | Meaning |
| --- | --- |
| **Runtime** | The `worldstreamd` process. It stores Room state and applies Activity Pack rules. |
| **Operator CLI** | The `worldstreamctl` command. It manages one local installation. |
| **Controller** | A local headless service that starts the Runtime and prepares scoped connections. |
| **Activity Client Host** | A separate local web server that serves browser Activity Clients. It does not manage WorldStream. |
| **Activity Pack** | The rules for one type of Room. Negotiate and Agent Heist are Activity Packs. |
| **Room** | One durable instance of an Activity Pack. Humans and agents can participate in it. |
| **Activity Client** | A browser, terminal program, SDK program, or agent integration that connects to a Room. |
| **Runner** | An external program that receives agent work. WorldStream does not host the model. |

The current Controller binary is named `worldstream-studio-supervisor` for
compatibility. The name does not make the Studio web application part of this
flow.

## 1. Prepare the repository

Run all commands from the repository root. This is the directory that contains
`Cargo.toml` and `package.json`.

### Install the required tools

The repository pins these development versions:

| Tool | Version | Use |
| --- | --- | --- |
| Rust | 1.97.1 | Build the Runtime, CLI, and Controller |
| Python | 3.14.7 | Run the SDK examples |
| uv | 0.12.5 | Install Python and Python dependencies |
| Node.js | 24.18.1 | Build and serve the browser Activity Clients |
| pnpm | 11.19.0 | Install and build browser dependencies |

Install Rust with the [official Rust installer](https://rust-lang.org/tools/install/).
Install uv with the [official uv instructions](https://docs.astral.sh/uv/getting-started/installation/).
Install Node.js from the [Node.js download archive](https://nodejs.org/en/download/archive/v24).

Enable the pinned pnpm version:

```sh
corepack enable
corepack install --global pnpm@11.19.0
```

Check the versions:

```sh
rustc --version
uv --version
uv run --python 3.14.7 python --version
node --version
pnpm --version
```

Use the exact versions in the table. Some evidence checks reject other
versions.

### Build the local programs

Run these commands:

```sh
cargo build --locked -p worldstream-server --bins
cargo build --locked -p worldstream-studio-supervisor --bins
uv sync --project sdk/python --locked --python 3.14.7
pnpm install --frozen-lockfile
```

Do not build the browser clients separately here. The Client Host command in
Section 4 builds them before it starts.

The build creates these local programs:

- `target/debug/worldstreamctl`
- `target/debug/worldstreamd`
- `target/debug/worldstream-studio-supervisor`
- `target/debug/worldstream-assignment-mcp`

The CLI-first operator commands are part of the default build.

### Set paths for this guide

Use Terminal 1 for operator commands. Run this block in Terminal 1:

```sh
export CTL="$PWD/target/debug/worldstreamctl"
export CONFIG="$PWD/config/development.toml"
export BUNDLE="$PWD/packs/negotiate/releases/0.2.0/worldstream-negotiate-83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8.wspack"
export BUNDLE_DIGEST="blake3:83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8"

mkdir -p target
export RUN_DIR="$(mktemp -d "$PWD/target/worldstream-getting-started.XXXXXX")"
chmod 700 "$RUN_DIR"
printf 'Work files: %s\n' "$RUN_DIR"
```

`RUN_DIR` is a new owner-only directory. Room setup files and credentials go
in this directory. Copy the printed absolute path when a later step tells you
to use another terminal.

## 2. Initialize the installation

The development configuration uses these local values:

- Runtime address: `127.0.0.1:9410`
- Controller address: `127.0.0.1:9420`
- Runtime data: `.worldstream/data`
- Controller data: `.worldstream/studio`
- Configured browser Client Host origin: `http://127.0.0.1:5173` (not started
  by `init`)

The `.worldstream/studio` path is a retained compatibility name. It contains
Controller state, not the Studio web application.

You will run `init` three times. These calls are not retries:

1. The first call prepares the local installation.
2. The second call previews the client declarations without applying them.
3. The third call approves and applies the exact previewed declarations.

Initialize the protected state:

```sh
"$CTL" --config "$CONFIG" init --json
```

This command creates the installation authority when it is absent. It reuses
valid existing authority. It does not start a process. A successful response
contains `"services_started":false`. This value is expected and is not an
error.

Now preview the checked-in Activity Client declarations:

```sh
"$CTL" --config "$CONFIG" init \
  --client-declaration "$PWD/config/activity-clients/cli-import.json" \
  --preview \
  --json
```

Find `import_review.digest` in the JSON output. Copy the complete value. It
starts with `blake3:`. Set it in Terminal 1. Do not use the placeholder below:

```sh
export IMPORT_DIGEST='blake3:paste-the-complete-preview-digest-here'
```

Apply the exact reviewed import:

```sh
"$CTL" --config "$CONFIG" init \
  --client-declaration "$PWD/config/activity-clients/cli-import.json" \
  --approve-imports "$IMPORT_DIGEST" \
  --json
```

The digest binds the exact local declarations to this installation. A changed
declaration needs a new preview and a new approval. The import does not start
the Client Host, Controller, or Runtime.

## 3. Install the Negotiate Activity Pack

The Agent Heist Pack is embedded in the Runtime. The Negotiate Pack is a
portable `.wspack` file. You must install it before you start the Runtime.

All Pack inventory changes are offline operations. If this is a new
installation, skip the next block.

If this installation is already running, stop the Runtime and confirm its
state:

```sh
"$CTL" --config "$CONFIG" server stop --json
"$CTL" --config "$CONFIG" server status --json
```

Continue only if `server status` reports that the Runtime is stopped. If stop
is partial or fails, leave the Controller running and inspect `server logs`.
Do not continue with Pack commands.

After you confirm that the Runtime is stopped, stop the Controller:

```sh
"$CTL" --config "$CONFIG" server controller-stop --json
```

Inspect the untrusted Bundle bytes:

```sh
"$CTL" --config "$CONFIG" pack inspect --bundle "$BUNDLE"
```

Verify that the Runtime can load this Pack safely:

```sh
"$CTL" --config "$CONFIG" pack prove "$BUNDLE" --json
```

The proof can take approximately 30 seconds on the first run. Continue only
after it succeeds.

Record a local approval, and then install the same exact bytes:

```sh
export APPROVED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
"$CTL" --config "$CONFIG" pack approve \
  --bundle "$BUNDLE" \
  --operator-id local-developer \
  --decided-at "$APPROVED_AT"

export INSTALLED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
"$CTL" --config "$CONFIG" pack install \
  --bundle "$BUNDLE" \
  --installed-at "$INSTALLED_AT"
```

Installation first retains the Pack without selecting it for new Rooms. Make
the exact Bundle selectable at the next Runtime start:

```sh
"$CTL" --config "$CONFIG" pack inventory
"$CTL" --config "$CONFIG" pack set-selectable \
  --bundle-digest "$BUNDLE_DIGEST" \
  --selectable true
"$CTL" --config "$CONFIG" pack inventory
```

Run the restart-readiness check while the Runtime is still stopped:

```sh
"$CTL" --config "$CONFIG" pack restart-readiness
```

This command admits the installed Pack through the startup path. It also
replays retained healthy Rooms with their exact Pack revision. It writes a
readiness seal for this exact inventory and local data target.

Do not change Pack inventory while the Runtime is running.

## 4. Start the core services and browser clients

This section uses two terminals. Terminal 1 starts and checks the WorldStream
core services. Terminal 2 serves the independent browser Activity Clients.

### Terminal 1: start the Controller and Runtime

Start the Controller and Runtime:

```sh
"$CTL" --config "$CONFIG" server start \
  --participant-console-origin http://127.0.0.1:5173 \
  --json
```

The command starts managed background processes. You do not have to keep this
terminal open for the processes. It does not start the browser Client Host.

The `--participant-console-origin` option allows authorized client handoffs to
use `http://127.0.0.1:5173`. The option does not start a process on port
`5173`.

Check the installation:

```sh
"$CTL" --config "$CONFIG" server status --json
"$CTL" --config "$CONFIG" health
"$CTL" --config "$CONFIG" pack list
"$CTL" --config "$CONFIG" server logs --tail 50
```

`server status` must report a live and ready Runtime. `pack list` must show
both of these exact selectors:

- `worldstream.agent-heist@0.2.0`
- `worldstream.negotiate@0.2.0`

Read-only commands do not start a stopped Controller or Runtime.

### Terminal 2: start the Activity Client Host

Open a second terminal at the repository root. Run:

```sh
pnpm activity-clients:serve
```

The command first builds the browser clients. Wait until it prints this line:

```text
WorldStream Activity Client Host listening on http://127.0.0.1:5173
```

Keep Terminal 2 open. You can now open the generic Inspector at
`http://127.0.0.1:5173/`.

Do not open a Room-specific client path directly. A direct URL has no
Membership authority. After you create a Room, use `client open` as shown in
Section 6. The CLI selects the approved Activity Client and gives it a one-use
authorized handoff.

## Before you create a Room

The next sections use four values. Learn them once here:

| Value | Meaning |
| --- | --- |
| `operation_id` | The durable identity of one Room setup attempt. Use it to resume setup or select a seat. |
| `room_id` | The durable identity of the created Room. Use it to inspect or launch that Room. |
| `seat` | A stable label in the Room setup file, such as `navigator` or `buyer-agent`. A seat label is not a Room ID or a Pack Role name. |
| credential file | An owner-only file with authority for one Membership or Runner. Treat it as a secret. |

`room create --json` prints `operation_id` and `room_id`. Copy those values to
the shell variables shown in each section. Do not copy example placeholders.

If creation reports a partial or uncertain result, do not run `room create`
again. Inspect and resume the same operation:

```sh
"$CTL" --config "$CONFIG" room setup status OPERATION_ID
"$CTL" --config "$CONFIG" room setup resume OPERATION_ID
```

Each credential export needs a new filename in the owner-only `RUN_DIR`.
WorldStream never prints the credential and never overwrites an existing file.
Do not commit a credential file or paste its contents into logs or chat.

## 5. Submit a proposal to a Negotiate Room with Python

This demonstration does not use the browser Client Host. It connects directly
to the WorldStream HTTP and WebSocket interfaces through the Python SDK.

Generate a setup file from the exact installed Pack revision:

```sh
"$CTL" --config "$CONFIG" room example \
  --pack worldstream.negotiate@0.2.0 \
  --output "$RUN_DIR/negotiate-room.json"

"$CTL" --config "$CONFIG" room validate \
  --file "$RUN_DIR/negotiate-room.json"
```

Open and review the generated JSON file before creation. The supplied example
uses a `formation_deadline` of `4102444800`, which is 2100-01-01 00:00:00 UTC.
This far-future value is only for the local example.

Negotiate starts at Room creation. The setup does not use a lobby. Create it
with the required start acknowledgement:

```sh
"$CTL" --config "$CONFIG" room create \
  --file "$RUN_DIR/negotiate-room.json" \
  --acknowledge-start \
  --json
```

Copy `operation_id` and `room_id` from the result:

```sh
export NEGOTIATE_OPERATION='paste-the-negotiate-operation-id'
export NEGOTIATE_ROOM='paste-the-negotiate-room-id'
```

Export the Buyer Agent Membership credential:

```sh
"$CTL" --config "$CONFIG" client export-credentials \
  --operation "$NEGOTIATE_OPERATION" \
  --seat buyer-agent \
  --output "$RUN_DIR/negotiate-buyer.json" \
  --json
```

Use the Python SDK example to connect, synchronize, and submit the reviewed
first proposal:

```sh
"$PWD/sdk/python/.venv/bin/python" -m examples.cli_activity.negotiate \
  --membership-file "$RUN_DIR/negotiate-buyer.json"
```

The result must have `"status":"accepted"`. Inspect the Room after the
proposal:

```sh
"$CTL" --config "$CONFIG" room inspect "$NEGOTIATE_ROOM" --json
```

Do not run `room launch` for Negotiate. Its Activity already started at Room
creation.

## 6. Verify an authorized Agent Heist browser client

This is a browser connection test. It creates a new Heist Room, opens an
authorized participant view, and leaves the Room in the Lobby. It does not
complete the Heist. Do not reuse this Room for the full SDK run in Section 7.

Before you continue, confirm that Terminal 2 still shows the Activity Client
Host from Section 4. Do not open `/agent-heist-v2/` directly. The CLI gives the
browser a one-use authorized handoff later in this section.

### Create the browser Room

Return to Terminal 1. Generate, validate, and create a Heist Room:

```sh
"$CTL" --config "$CONFIG" room example \
  --pack worldstream.agent-heist@0.2.0 \
  --output "$RUN_DIR/heist-browser-room.json"

"$CTL" --config "$CONFIG" room validate \
  --file "$RUN_DIR/heist-browser-room.json"

"$CTL" --config "$CONFIG" room create \
  --file "$RUN_DIR/heist-browser-room.json" \
  --json
```

Copy the two result values:

```sh
export HEIST_BROWSER_OPERATION='paste-the-browser-heist-operation-id'
export HEIST_BROWSER_ROOM='paste-the-browser-heist-room-id'
```

Open the approved Agent Heist Activity Client for the human `navigator` seat:

```sh
"$CTL" --config "$CONFIG" client open \
  --operation "$HEIST_BROWSER_OPERATION" \
  --seat navigator \
  --json
```

The CLI asks your system browser to open a protected local handoff file. The
file redirects to the approved client. The one-use handoff gives the browser
only the selected Membership authority.

Keep the browser page open. Inspect the Room:

```sh
"$CTL" --config "$CONFIG" room inspect "$HEIST_BROWSER_ROOM" --json
```

This browser-only example stays in the Lobby. The required external `insider`
agent is not connected. An open browser window alone proves no participant
readiness. The live evidence in `room inspect` can confirm the Navigator
connection. Section 7 uses a separate Room and connects both required seats.

## 7. Complete a separate Agent Heist Room with the Python SDK

This is a separate end-to-end demonstration. It does not use the browser
Client Host. You can run it after Section 6 or run it instead of Section 6.

Generate, validate, and create another Heist Room in Terminal 1:

```sh
"$CTL" --config "$CONFIG" room example \
  --pack worldstream.agent-heist@0.2.0 \
  --output "$RUN_DIR/heist-sdk-room.json"

"$CTL" --config "$CONFIG" room validate \
  --file "$RUN_DIR/heist-sdk-room.json"

"$CTL" --config "$CONFIG" room create \
  --file "$RUN_DIR/heist-sdk-room.json" \
  --json
```

Copy the two new result values:

```sh
export HEIST_SDK_OPERATION='paste-the-sdk-heist-operation-id'
export HEIST_SDK_ROOM='paste-the-sdk-heist-room-id'
```

Export one Membership credential for each required seat. Also export the
separate Runner credential for the external Insider:

```sh
"$CTL" --config "$CONFIG" client export-credentials \
  --operation "$HEIST_SDK_OPERATION" \
  --seat navigator \
  --output "$RUN_DIR/heist-navigator.json" \
  --json

"$CTL" --config "$CONFIG" client export-credentials \
  --operation "$HEIST_SDK_OPERATION" \
  --seat insider \
  --output "$RUN_DIR/heist-insider.json" \
  --json

"$CTL" --config "$CONFIG" runner export-credentials \
  --operation "$HEIST_SDK_OPERATION" \
  --seat insider \
  --output "$RUN_DIR/heist-insider-runner.json" \
  --json
```

The Insider needs both files. Its Membership credential submits Room Actions.
Its Runner credential receives, claims, and completes agent Activations. The
credentials are not interchangeable.

Open Terminal 3 at the repository root. Set `RUN_DIR` to the absolute work
directory that Terminal 1 printed in Section 1:

```sh
export RUN_DIR='/paste-the-absolute-work-directory'
```

Start the deterministic Heist integration in Terminal 3:

```sh
"$PWD/sdk/python/.venv/bin/python" -m examples.cli_activity.heist \
  --navigator-membership-file "$RUN_DIR/heist-navigator.json" \
  --membership-file "$RUN_DIR/heist-insider.json" \
  --runner-file "$RUN_DIR/heist-insider-runner.json" \
  --timeout-seconds 600
```

Keep this command running. It connects the Navigator Membership, Insider
Membership, and Insider Runner. The Room stays in the Lobby until the operator
launches it.

Return to Terminal 1. Inspect the Room:

```sh
"$CTL" --config "$CONFIG" room inspect "$HEIST_SDK_ROOM" --json
```

If a required seat is not ready, wait a few seconds and run the inspect command
again. Keep the Python command running. When both required seats are ready,
launch the Room:

```sh
"$CTL" --config "$CONFIG" room launch "$HEIST_SDK_ROOM" --json
```

The Python example now performs the deterministic Navigator and Insider work.
It handles agent Activations and the timed Heist phases. It can be quiet while
it waits for timers. The final JSON in Terminal 3 must have
`"status":"complete"`.

Inspect the completed Room:

```sh
"$CTL" --config "$CONFIG" room inspect "$HEIST_SDK_ROOM" --json
"$CTL" --config "$CONFIG" server logs --tail 100
```

The example uses public HTTP and WebSocket interfaces. It does not call a paid
model API. Replace this deterministic program with your own agent integration
when you develop an Activity Client or Runner.

## 8. Stop the local installation

First, stop the Client Host with `Ctrl-C` in Terminal 2. Close the Heist
browser tab.

Then stop managed Runners and the Runtime from Terminal 1:

```sh
"$CTL" --config "$CONFIG" server stop --json
"$CTL" --config "$CONFIG" server status --json
```

Continue only if `server status` reports that the Runtime is stopped. If stop
is partial or fails, leave the Controller running and inspect `server logs`.

The Controller stays available after a successful Runtime stop. Stop it
explicitly when you are finished:

```sh
"$CTL" --config "$CONFIG" server controller-stop --json
```

Keep `.worldstream/` if you want to use the same installation again. It
contains the database, authority, Pack inventory, and Controller state. Do not
delete this directory as a normal troubleshooting step.

## Common problems

### A command says that the Controller is unavailable

Run:

```sh
"$CTL" --config "$CONFIG" server status --json
```

If you have not started the installation, run Section 4. If a managed process
failed, use `server logs` before you retry. Do not delete `.worldstream/`.

### A client declaration import is rejected on an old local installation

An Activity Client Deployment ID is immutable. A retained `.worldstream/`
directory can contain the same Deployment ID for an older client Release.
WorldStream rejects an attempt to change that identity.

Do not repeatedly approve new digests. First decide whether the retained Rooms
and state are valuable:

- For disposable MVP state, stop the Runtime and Controller. Rename
  `.worldstream/` to a backup name. Then repeat Section 2 with a new local
  installation.
- For valuable state, keep `.worldstream/`. Do not delete it. Use a migration
  or a new Deployment ID as described in
  [Activity Clients](activity-clients.md).

### A Pack command says that the Runtime is active

Pack approval, installation, selection, and restart-readiness are offline
operations. Stop the Runtime and Controller as shown in Section 3. Complete
the full Pack lifecycle before you start the Runtime again.

### `room create` returns a partial result

Use its `operation_id`. Run `room setup status` and `room setup resume` for
that same operation. Do not create a replacement Room. A lost reply is not a
rollback.

### An output file already exists

The CLI does not overwrite setup files or credential files. Use a new filename
or create a new `RUN_DIR`. Do not modify an exported credential file.

### The browser client waits for an authorized Projection

Confirm these conditions:

- Terminal 2 still runs `pnpm activity-clients:serve`.
- You used `client open` for this exact operation and seat.
- You did not open the client URL directly.
- `server status` reports a ready Runtime.

Close a stale tab and run `client open` again to get a new one-use handoff.

### `http://127.0.0.1:5173/` does not open

The Controller and Runtime do not serve browser files. Start the independent
Activity Client Host in Terminal 2:

```sh
pnpm activity-clients:serve
```

Keep the command running. Wait for the `Activity Client Host listening`
message before you open the URL. If the command exits, read its terminal
output; the Client Host is no longer running.

### Agent Heist is not ready to launch

Keep the direct Python example running. Run `room inspect` again after a few
seconds. The required Navigator Membership, Insider Membership, and Insider
Runner must all be connected and fresh. The optional Broker does not block
launch.

### The direct Heist example has no output

The example can be quiet while it waits for the operator launch or for phase
timers. Inspect the same Room from Terminal 1. The command in this guide uses a
ten-minute timeout.

### An existing installation has authority or database errors

Stop. Do not generate a replacement authority beside an existing database.
Do not delete the database. Restore the original protected state or follow the
[operator storage and transfer guide](operator-storage.md).

## Continue with one subject

| Task | Guide |
| --- | --- |
| Learn all operator commands | [Operator CLI reference](cli-reference.md) |
| Understand Pack approval and retained execution | [Activity Packs](activity-packs.md) |
| Build or host an independent browser client | [Activity Clients](activity-clients.md) |
| Understand Negotiate rules and contracts | [Negotiate](negotiate.md) |
| Use the Python SDK | [Python SDK README](../sdk/python/README.md) |
| Understand HTTP and WebSocket messages | [Protocol](protocol.md) |
| Select an automated or release gate | [Automated gates](gates.md) |
