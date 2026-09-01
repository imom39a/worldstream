# Troubleshooting

## Installation and builds

### Pinned tool version mismatch

Use `.node-version`, `.python-version`, `.uv-version`, `rust-toolchain.toml`, and
the root `packageManager` value. Hosted or local verification may reject a newer
version even when it appears compatible because reproducibility is part of the
evidence contract.

### Supervisor says assignment MCP is unavailable

Build all Supervisor binaries, not only the main binary:

```sh
cargo build --locked -p worldstream-studio-supervisor --bins
```

### Address already in use

Stop the previous process or choose a new loopback address. Keep daemon,
Supervisor, Vite proxy, and origin flags consistent. Do not bind development
services publicly.

## Daemon and storage

### SQLite authority bootstrap conflicts after restart

This failure commonly appears when Studio starts the daemon after
`.worldstream/authority.secret` was regenerated but `.worldstream/data` still
contains the database bound to the previous secret:

```text
Error: SQLite authority bootstrap failed closed

Caused by:
    0: authority bootstrap transaction was rejected
    1: authority change conflicts with another request
```

This is an intentional fail-closed authority check, not a migration failure.
The database stores only a hash of the original 32-byte secret, so the original
secret cannot be recovered from the database.

Stop the daemon before recovery, then choose one path:

- **Preserve existing Rooms:** restore the exact original
  `.worldstream/authority.secret`, keep it owner-only with `chmod 600`, and
  retry the start. Do not edit the SQLite authority tables or receipts.
- **Reset disposable local Rooms:** keep the current secret and archive the old
  data directory before creating an empty owner-only replacement:

  ```sh
  mv .worldstream/data \
    ".worldstream/data.authority-mismatch.$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -m 700 .worldstream/data
  ```

  Start the daemon again from Studio. The archived directory remains available
  if it is needed for later investigation.

The [quickstart](#/quickstart) reuses an existing secret instead of overwriting
it. Keep each SQLite data directory paired with the secret that first
bootstrapped it.

### `/healthz` works but `/readyz` fails

The listener is live but a required storage/authority/writer/scheduler/recovery
fact is unavailable. Read the closed readiness reason and fix that prerequisite;
do not use health as a substitute.

### SQLite path or permission failure

Use an owner-only local filesystem path. Symlinks, broad secret permissions,
path substitution, existing backup destinations, and unsupported filesystems
may fail closed by design.

## Studio

### Studio loads but requests fail

Verify the Supervisor at `127.0.0.1:9420` and the Vite `/api` proxy. Check the
browser origin values. Current local application ports require:

```text
--studio-origin http://127.0.0.1:5174
--participant-console-origin http://127.0.0.1:5173
```

### Participant handoff rejected

Confirm the human seat is provisioned, the handoff is fresh and unused, the
Client Host is on its exact configured origin, and cookies are accepted. The
`--participant-console-origin` flag retains its compatibility name. Do not
extract the fragment and manually construct a participant session.

### Empty Runner Template catalog

This is valid. Install reviewed manifests under the startup-fixed
`config/runner-templates` directory only when supervised Runners are needed.

## Agents

### MCP host cannot discover tools

Verify it launched the helper as stdio MCP, completed the MCP initialization
sequence, used the correct owner-only state directory, and received a current
assignment reference. Do not feed line-delimited JSON-RPC through an ordinary
shell chat interface.

### Agent sees no Task or no Activation

The helper is assignment-scoped. Check exact Agent Profile assignment, separate
participant/Runner provisioning, Lobby/setup state, Runner readiness, and
whether there is legitimately no pending work.

### Action is stale

Observe again, list current offers, choose an exact new offer, and use a new
operation identity. Never alter a retained request under the old identity.

### ChatGPT web cannot connect

Expected: the current helper is local stdio. ChatGPT web requires a remote MCP
boundary or approved tunnel; that integration is not shipped here.

## Documentation site

```sh
pnpm docs:lint
pnpm docs:test
pnpm docs:build
pnpm docs:preview
```

If Pages assets return 404, rebuild with a `DOCS_BASE` matching the project
Pages subpath. A successful local build is not deployment proof; inspect the
actual GitHub Actions or GitLab pipeline, the deploy job, and the live Pages
URL.

For deeper diagnosis, use the owning source linked from the relevant page and
the [security model](#/concepts/security) before collecting logs.
