# Local quickstart

Bring up the authoritative daemon and Studio operator portal from a fresh
checkout. Run every command from the repository root.

## 1. Install the pinned toolchain

| Tool | Required version |
| --- | --- |
| Rust | 1.97.1 |
| Node.js | 24.18.1 |
| pnpm | 11.19.0 |
| Python | 3.14.7 |
| uv | 0.12.5 |

Install a C/C++ build toolchain plus `bash` and `curl` with the operating
system's package manager. On macOS, `xcode-select --install` supplies the native
compiler tools. Then install the language tools:

1. Install `rustup` using the [official Rust installer](https://rust-lang.org/install.html),
   restart the shell, and install the pinned compiler:

   ```sh
   rustup toolchain install 1.97.1 --profile minimal \
     --component rustfmt --component clippy
   ```

2. Install the exact signed Node.js 24.18.1 build from the
   [Node.js v24 archive](https://nodejs.org/en/download/archive/v24), or select
   that exact release with your managed Node installation. Then activate the
   package manager pinned by `package.json`:

   ```sh
   corepack enable
   corepack install --global pnpm@11.19.0
   ```

3. Download and inspect the versioned official uv installer before executing
   it, then let uv install the pinned Python:

   ```sh
   curl -LsSf https://astral.sh/uv/0.12.5/install.sh \
     -o /tmp/worldstream-uv-install.sh
   less /tmp/worldstream-uv-install.sh
   sh /tmp/worldstream-uv-install.sh
   uv python install 3.14.7
   ```

   Astral documents both the [versioned uv installer](https://docs.astral.sh/uv/getting-started/installation/)
   and [managed Python installs](https://docs.astral.sh/uv/guides/install-python/).

Open a new shell if an installer changed `PATH`, then verify every exact pin:

```sh
rustc --version
node --version
pnpm --version
uv --version
uv run --python 3.14.7 python --version
```

Do not continue on a nearby patch release. Release evidence and the local
browser/toolchain gates intentionally compare exact identities.

## 2. Install the locked workspace

```sh
uv sync --project sdk/python --locked --python 3.14.7
pnpm install --frozen-lockfile
cargo build --locked -p worldstream-server --bins
cargo build --locked -p worldstream-studio-supervisor --bins
```

The Supervisor build produces three binaries: the Studio Supervisor, the
assignment-bound MCP helper, and the managed reference Agent Host.

## 3. Create local SQLite authority state

```sh
umask 077
mkdir -p .worldstream/data
if [ ! -e .worldstream/authority.secret ]; then
  head -c 32 /dev/urandom > .worldstream/authority.secret
fi
chmod 600 .worldstream/authority.secret
```

The secret is exactly 32 random bytes—not hexadecimal text and not a line of
text. The setup command reuses an existing secret instead of overwriting it.
Keep it with the same database. WorldStream persists its hash and rejects a
different secret on restart.

Validate before starting:

```sh
target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamctl --config config/development.toml config effective
target/debug/worldstreamctl --config config/development.toml doctor
```

`config effective` redacts secret references. `doctor` is non-mutating, so
storage may still report `not_initialized` before first startup.

## 4. Start Studio and its Supervisor

In terminal 1, start the Studio development stack with the browser origins
bound to their actual local ports:

```sh
scripts/studio-dev.sh \
  --studio-origin http://127.0.0.1:5174 \
  --participant-console-origin http://127.0.0.1:5173
```

Keep this foreground command running. It builds the `worldstreamd` binary, then
starts only these two processes:

- the Studio Supervisor on `127.0.0.1:9420`;
- the Studio portal on `127.0.0.1:5174`.

It does **not** start `worldstreamd`, and it does **not** start the Client Host.
`pnpm studio:dev` is an alias for the same launcher without additional
Supervisor arguments. Until the current CLI origin defaults are aligned with
the Vite ports, use the explicit command above for the complete local flow.

## 5. Start the daemon

Open `http://127.0.0.1:5174`, select **Operations**, and click **Start daemon**.
Wait for the UI to report `worldstreamd connected` and `ready` before running
the health probes in Step 7.

If you prefer to own the daemon in a separate terminal instead of through
Studio, run this from the repository root:

```sh
RUST_LOG=info target/debug/worldstreamd --config config/development.toml
```

Studio discovers that external process through the Supervisor. Do not run both
startup paths for the same `127.0.0.1:9410` listener.

The complete local topology is:

| Process | Address | How it starts |
| --- | --- | --- |
| `worldstreamd` | `127.0.0.1:9410` | **Start daemon** in Studio, or the foreground command above |
| Studio Supervisor | `127.0.0.1:9420` | `scripts/studio-dev.sh ...` in terminal 1 |
| Studio portal | `127.0.0.1:5174` | `scripts/studio-dev.sh ...` in terminal 1 |
| Client Host | `127.0.0.1:5173` | optional `pnpm ui:dev` in terminal 2 |

## 6. Optionally start the Client Host

Skip this step when you only need Studio Operations, daemon health, Activity
Pack inspection, Room administration, backups, or agent setup.

For a browser Activity Client handoff, start the separate first-party Client
Host in terminal 2:

```sh
pnpm ui:dev
```

Keep it running alongside terminal 1. Do not open a client with manually copied
Room credentials. In Studio, provision the human seat and use **Open
participant client**; the Supervisor brokers the one-use handoff to port
`5173`. Exact approved Agent Heist `0.1.0` and `0.2.0` revisions open
`/agent-heist/`; other exact Pack revisions open the generic `/inspector/`
fallback. Studio launches that independent client and never renders the
participant surface itself.

## 7. Verify the live runtime

```sh
curl -fsS http://127.0.0.1:9410/healthz
curl -fsS http://127.0.0.1:9410/readyz
curl -fsS http://127.0.0.1:9410/version
target/debug/worldstreamctl --config config/development.toml health
```

- `/healthz` proves the process serves requests.
- `/readyz` proves storage, authority, writer, and scheduler readiness.
- `/version` reports embedded compatibility and selected engine identity.

## 8. Run a reference story

The offline Agent Heist story needs no daemon or model:

```sh
uv run --project sdk/python --python 3.14.7 \
  python examples/heist/run_story.py --self-test
```

The live Counter acceptance path exercises the daemon boundary:

```sh
uv run --project sdk/python --python 3.14.7 \
  python examples/counter/run_live_acceptance.py
```

Use the [Agent Heist guide](#/activity-packs/agent-heist) before running the
complete Studio-driven acceptance gate.

## Stop and reset

If Studio started the daemon, stop it from **Operations** first. Use `Ctrl-C` in
each foreground terminal to stop Studio/Supervisor and the optional Client
Host. To reset disposable state, stop the daemon and remove `.worldstream/`,
then repeat the secret setup. Never replace only the secret while retaining the
database.

Source: [getting started](https://github.com/imom39a/worldstream/blob/main/docs/getting-started.md),
[runtime configuration](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-runtime/src/config.rs),
and [Studio launcher](https://github.com/imom39a/worldstream/blob/main/scripts/studio-dev.sh).
