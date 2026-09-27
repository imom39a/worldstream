# Getting started

The kernel runs locally with bundled SQLite. It needs no hosted account or model
provider. Run commands from the repository root.

## Build and run

Install Rust using the checked-in `rust-toolchain.toml` (Rust 1.97.1). A native C
build toolchain is required for bundled dependencies. Linux and macOS use the
following shell commands; Windows users can build with Cargo and use PowerShell
for process and filesystem setup.

```sh
cargo build --locked -p worldstream-server --bins
umask 077
mkdir -p .worldstream/data
if [ ! -e .worldstream/authority.secret ]; then
  head -c 32 /dev/urandom > .worldstream/authority.secret
fi
chmod 600 .worldstream/authority.secret
target/debug/worldstreamctl --config config/development.toml config validate
target/debug/worldstreamd --config config/development.toml
```

The supplied [configuration](../config/development.toml) binds to loopback and
keeps state under the ignored `.worldstream/` directory. Retain the bootstrap
secret alongside the database; generating another secret is not a recovery path.

In another terminal:

```sh
curl -fsS http://127.0.0.1:9410/healthz
curl -fsS http://127.0.0.1:9410/readyz
curl -fsS http://127.0.0.1:9410/version
```

`healthz` reports liveness. `readyz` succeeds only after the selected storage and
runtime are ready. `version` reports the embedded identity and selected engine.
Stop the foreground daemon with Ctrl-C. Its durable state remains on disk.

## Exercise a Room

The [Counter example](../examples/counter/README.md) provisions participant and
spectator Memberships and drives real typed Actions through the Python SDK. It
starts its own disposable runtime, so it can run independently of the process
above.

Python is optional for the kernel. For this example, use uv with the pinned
Python version in `.python-version`:

```sh
uv sync --project sdk/python --locked
uv run --project sdk/python python examples/counter/run_live_acceptance.py
```

The command returns a structured pass/failure report. No model API is involved.
The other [examples](../examples/README.md) exercise portable packs, timed games,
external evidence, and application-level agent coordination.

## Operator CLI

```sh
target/debug/worldstreamctl --help
target/debug/worldstreamctl room --help
target/debug/worldstreamctl pack --help
```

The CLI also supports a managed installation with a headless Controller. Build
that executable when using managed lifecycle or setup commands:

```sh
cargo build --locked -p worldstream-studio-supervisor --bins
```

See [CLI reference](cli-reference.md), [initialization inputs](cli-initialization-inputs.md),
and [Room setup examples](../examples/room-setup/README.md). Initialization,
approval, setup, and service startup are separate operations; listing or
validating a setup never grants approval or launches a provider.

## Optional TypeScript tools

Install the Node and pnpm versions recorded in `.node-version` and `package.json`
(Node 24.18.1 and pnpm 11.19.0), then:

```sh
pnpm install --frozen-lockfile
pnpm pack:build
pnpm --dir examples/packs/tower-of-hanoi pack:check
pnpm --dir examples/packs/tower-of-hanoi pack:test
```

See the [Pack SDK](../sdk/typescript-pack/README.md) and
[browser clients](../examples/clients/README.md). Client builds and model-provider
configuration are not required to start the kernel.

## Verify and inspect

```sh
scripts/verify-local.sh
```

This runs formatting, compiler checks, build, tests, and compatibility identity checks for
the default Rust members. [Verification](gates.md) lists additional example,
SDK, and live protocol checks. [Architecture](architecture.md) maps the code and
explains the tradeoffs.

If startup fails, check file permissions, whether another process holds the
same data directory, and the selected storage identity. Use a separate empty
directory for a fresh experiment. Do not edit existing Room history, change
retained Pack bytes, or delete an authority secret to bypass a validation error.
