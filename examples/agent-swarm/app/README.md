# Agent Swarm application

This Rust example separates durable coordination from provider process execution.
The default build exercises deterministic fixtures without creating a live Room
or calling an AI provider. From the repository root:

```sh
cargo test --locked -p worldstream-agent-swarm
cargo run --locked -p worldstream-agent-swarm -- --help
mkdir -p .scratch/swarm-example
cargo run --locked -p worldstream-agent-swarm -- smoke-test \
  --state-dir .scratch/swarm-example
```

Use a disposable state directory for the smoke example.

## Local Room integration

The optional `managed-local-runtime` feature adds `create`, `list`, `open`,
`observe`, `act`, and `tui` commands backed by a running local Controller and
Runtime:

```sh
cargo test --locked -p worldstream-agent-swarm --features managed-local-runtime
cargo run --locked -p worldstream-agent-swarm \
  --features managed-local-runtime -- --help
```

Follow [Getting started](../../../docs/getting-started.md) to initialize the
local installation. Build the [Agent Swarm Pack](../../packs/agent-swarm/README.md)
with `pnpm --dir examples/packs/agent-swarm pack:build`, approve/install that
exact bundle, and use `worldstreamctl pack list` to obtain its running identity.
The managed commands take explicit Controller/Runtime addresses, state directory,
and Pack ID/version/digest. `create --help` describes the request file options;
`src/managed_local.rs` contains the executable integration examples.

## Execution boundary

The Room is authoritative for the goal, roster, work, and accepted results.
`worldstream-agent-swarmd` owns process trees, scheduling, budgets, priorities,
and pause/stop/resume state. Artifact references are resolved within the approved
working area and verified against their digests. The execution journal does not
replace the Room transcript.

Provider discovery is read-only. Native adapters require exact executable and
qualification evidence; missing or unsupported selections fail closed. The
retained Codex, Claude, Kiro, and optional advisor integrations are experimental
source, not a current compatibility promise. Default tests use controlled workers
and do not prove autonomous delivery or provider behavior.

The optional adaptive planning policy chooses among authorized work actions;
result acceptance still requires the Pack's checks and review. Read the
[design](../docs/design.md) and [Pack contract](../../packs/agent-swarm/README.md)
for the coordination model. Historical product plans and qualification campaigns
are available in Git history.
