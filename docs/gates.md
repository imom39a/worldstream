# Verification

Checks exercise the source implementation. They do not certify a supported
release, a hosted deployment, or autonomous-agent performance.

## Kernel and operator tools

```sh
scripts/verify-local.sh
```

On Windows with PowerShell 7.4+, use `scripts/verify-local.ps1`. Both run the
following from the repository root:

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo build --locked
cargo test --locked -- --test-threads=1
cargo run --locked -p xtask -- compat verify
```

Default Cargo members exclude the application examples. To include all Rust
examples and the opt-in Core tracer checks:

```sh
cargo test --workspace --locked -- --test-threads=1
cargo test --locked -p worldstream-core --features conformance-tracer
cargo test --locked -p worldstream-agent-swarm --features managed-local-runtime
```

The Component Host limits concurrent compilation. Run the full Rust suite with
one test thread to prevent unrelated fixtures from competing for that limit.

Some PostgreSQL tests require an explicitly configured local test database and
skip when it is absent. Their absence does not establish live PostgreSQL restore
or transfer coverage. Provider credentials are never required by default CI.

## Client SDKs and Activity Packs

```sh
uv sync --project sdk/python --locked
uv run --project sdk/python pytest sdk/python/tests
pnpm install --frozen-lockfile
pnpm sdk:check
pnpm pack:build
pnpm pack:lint
pnpm pack:test
pnpm --dir examples/packs/tower-of-hanoi pack:check
pnpm --dir examples/packs/tower-of-hanoi pack:test
```

Each example Pack README specifies its own checks. Component builds/proofs may
require the production host binary; a TypeScript typecheck alone does not prove
runtime conformance.

## Live local smoke

After building the binaries, run the isolated operator smoke with Python 3.11+
and curl available, or the Counter protocol example:

```sh
scripts/smoke-operator.sh
uv run --project sdk/python python examples/counter/run_live_acceptance.py
```

Both use temporary state. Neither touches an existing local installation or calls
a model provider.

## Optional browser clients and static site

```sh
pnpm activity-clients:check
pnpm activity-clients:build
node --test tests/activity_client_identities.test.mjs
node scripts/check-site.mjs
python3 scripts/check-docs.py
```

The static GitHub Pages site is uploaded directly from `site/`. It has no package
install or frontend build. The CI workflow checks its links and repository docs.

The old platform/release campaign, credential-dependent qualification gates, and
their evidence dumps have been retired. Runtime manifest parity, exact Pack
identity, retained executors, and canonical golden fixtures remain checked.

## Lint status

`cargo clippy --locked --workspace --all-targets -- -D warnings` currently reports
an existing lint backlog, including documentation and style warnings in the
unchanged kernel. CI shows this as an advisory step. Compiler checks, formatting, tests, and identity verification remain required. The Clippy warnings remain visible. This cleanup does not change kernel behavior to remove those warnings.
