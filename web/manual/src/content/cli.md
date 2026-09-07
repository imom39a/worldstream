# CLI reference

The binaries are self-describing. Use `--help` from the exact built revision;
this page is the navigation map.

## `worldstreamd`

```text
worldstreamd [--config FILE] [--bind ADDRESS]
             [--storage-profile PROFILE] [--data-dir DIRECTORY]
```

Runs the authoritative process shell with one startup-selected profile. Use
`RUST_LOG` for logging selection; do not pass secrets on the command line.

## `worldstreamctl`

| Command | Purpose |
| --- | --- |
| `config validate` | parse, layer, and strictly validate configuration |
| `config effective` | print redacted effective configuration |
| `doctor` | bounded non-mutating config/manifest/path checks |
| `health` | probe configured daemon liveness |
| `version` | print embedded compatibility summary |
| `pack inspect|approve|revoke|install|inventory|set-selectable|restart-readiness|export|remove` | operate exact local Pack Bundles while the daemon is stopped |
| `sqlite backup` | publish native backup plus sealed verifier envelope |
| `sqlite restore` | restore exact backup/envelope to a new destination |
| `sqlite verify` | read-only native SQLite verification |
| `postgres migrate` | forward-only migration via direct admin connection |
| `postgres verify` | verify engine and reviewed migration/schema contract |
| `postgres transfer begin|resume|finalize|abort` | one-way offline deployment transfer |
| `postgres snapshots` | rebuild reviewed disposable native-backup caches |
| `postgres native directory-identity|restore|recover` | crash-recoverable provider-native workflows |

Always run the leaf help before a storage-changing command:

```sh
target/debug/worldstreamctl postgres transfer begin --help
```

Pack operator commands are config-aware. Use one exact prefix throughout the
offline lifecycle so receipts name the same `storage_profile` and inventory:

```sh
target/debug/worldstreamctl --config <WORLDSTREAM_CONFIG> pack <COMMAND>
```

`pack restart-readiness` admits installed original Components through the
production host, executes configured-store Replay, and writes the durable seal
that an installed-bundle daemon startup requires. Its readiness receipt binds
the canonical `inventory_digest` to a pathless `deployment_binding` for the
actual data directory, profile, and provider metadata. With
`postgres-primary`, also pass
`--dsn-file <POSTGRES_ADMIN_DSN_FILE>`; never place DSN contents on the command
line. Install, revoke, set-selectable, retained restore, and remove clear the
prior seal.

## Controller and agent binaries

```sh
target/debug/worldstream-studio-supervisor --help
target/debug/worldstream-assignment-mcp --help
target/debug/worldstream-managed-agent-host --help
```

- The headless Controller fixes daemon executable/config, state, Runner manifest
  directory, browser origins, storage profile, and timeouts at startup.
- The MCP helper accepts only `--launch-reference` and `--state-dir`.
- The managed host accepts stdio transport, `openai-compatible` provider,
  loopback provider address, and exact model; its credential arrives privately.

## Workspace scripts

| Command | Purpose |
| --- | --- |
| `pnpm ui:dev` | First-party Client Host (`/agent-heist-v2/`, `/negotiate/`, `/negotiate-v2/`, and `/inspector/`) |
| `pnpm docs:dev` | this developer manual locally |
| `pnpm docs:build` | deterministic static manual build |
| `scripts/gates.sh fast` | bounded fast developer gate |
| `scripts/verify-local.sh` | broad local checkout verification |
| `scripts/smoke-operator.sh` | disposable real daemon smoke test |

## `xtask`

Use the workspace maintenance binary for compatibility contract work:

```sh
cargo run --locked -p xtask -- --help
cargo run --locked -p xtask -- compat verify
```

Do not hand-edit a generated compatibility mirror without running the matching
generator/verifier and reviewing the semantic source.

Source: [`worldstreamctl` command source](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-server/src/bin/worldstreamctl.rs),
[Controller compatibility binary](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-studio-supervisor/src/main.rs),
and [`xtask`](https://github.com/imom39a/worldstream/blob/main/xtask/src/main.rs).
