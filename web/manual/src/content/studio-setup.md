# Studio setup

WorldStream Studio is the local host-operator portal. Its Supervisor is a
separate bounded process that observes and coordinates `worldstreamd`; it is not
a second Room runtime and cannot mutate canonical state directly.

## Processes and ports

```text
Studio portal 127.0.0.1:5174 ─┐
                              ├─ Studio Supervisor 127.0.0.1:9420
Client Host   127.0.0.1:5173 ─┘                │
                                               ▼
                                     worldstreamd 127.0.0.1:9410
```

The Studio Vite server proxies `/api` to the Supervisor. Activity Clients on
the Client Host call the Supervisor's origin-bound handoff/session routes
directly. Studio is the host-operator portal and launcher; it does not render a
Pack's participant UI.

## Fresh-checkout setup

```sh
pnpm install --frozen-lockfile
cargo build --locked -p worldstream-server --bin worldstreamd
cargo build --locked -p worldstream-studio-supervisor --bins
```

Create `.worldstream/data` and `.worldstream/authority.secret` as shown in the
[quickstart](#/quickstart). The Supervisor creates owner-only operational state
under `.worldstream/studio`.

For the operator portal only:

```sh
pnpm studio:dev
```

For Studio plus a browser Activity Client handoff, use two terminals:

```sh
# terminal 1
pnpm ui:dev
```

```sh
# terminal 2
scripts/studio-dev.sh \
  --studio-origin http://127.0.0.1:5174 \
  --participant-console-origin http://127.0.0.1:5173
```

The explicit values are required by the current local ports; the Supervisor
binary's help text currently reports these two defaults in the opposite order.
`--participant-console-origin` retains its compatibility name while pointing
at the first-party Client Host.

**Open participant client** launches one of two closed paths on that origin.
Exact approved Agent Heist `0.1.0` and `0.2.0` identities use
`/agent-heist/`; unsupported exact Pack identities use the Pack-neutral
`/inspector/`. A matching Pack name or version with a different digest cannot
select the Heist client.

## Supervisor startup options

| Option | Default | Meaning |
| --- | --- | --- |
| `--bind` | `127.0.0.1:9420` | Supervisor API listener |
| `--daemon` | `127.0.0.1:9410` | existing daemon operator listener |
| `--probe-timeout-ms` | `750` | bounded daemon request timeout |
| `--daemon-executable` | `target/debug/worldstreamd` | fixed executable controlled by lifecycle operations |
| `--daemon-config` | `config/development.toml` | fixed daemon config |
| `--graceful-stop-timeout-ms` | `10000` | bounded stop wait |
| `--state-dir` | `.worldstream/studio` | protected Supervisor state |
| `--assignment-mcp-executable` | `target/debug/worldstream-assignment-mcp` | fixed MCP helper |
| `--host-authority-reference` | none | retained host authority used by daemon proxies |
| `--runner-templates-dir` | `config/runner-templates` | owner-approved Runner manifests |
| `--storage-profile` | `sqlite-bundled` | expected controlled daemon storage profile |

Inspect the current binary rather than copying stale options:

```sh
target/debug/worldstream-studio-supervisor --help
```

## Protected local state

`.worldstream/studio` contains drafts, exact setup progress, secret references,
Participant handoffs, agent assignments, Runner state, attention history, and
backup operation records. Browser APIs expose closed metadata views, never
secret values or local paths.

Do not edit this directory while the Supervisor is running. For a full
disposable reset, stop Studio and the daemon, then remove all `.worldstream`
state together. Selectively deleting protected records can intentionally make
reconciliation fail closed.

## Verify Studio

```sh
curl -fsS http://127.0.0.1:9420/api/v1/daemon/status
pnpm studio:test
pnpm --dir web/studio lint
pnpm studio:build
```

Source: [Studio documentation](https://github.com/imom39a/worldstream/blob/main/docs/studio.md),
[Supervisor main](https://github.com/imom39a/worldstream/blob/main/crates/worldstream-studio-supervisor/src/main.rs),
and [Studio development script](https://github.com/imom39a/worldstream/blob/main/scripts/studio-dev.sh).
