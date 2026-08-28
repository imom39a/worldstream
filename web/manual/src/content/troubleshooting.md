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

### Authority bootstrap fails after restart

Restore the exact original 32-byte secret. If local state is disposable, reset
both the database and secret together. Replacing only the secret is correctly
rejected.

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

Confirm the human seat is provisioned, the handoff is fresh and unused, Console
is on its exact configured origin, and cookies are accepted. Do not extract the
fragment and manually construct a participant session.

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

If GitLab Pages assets 404, rebuild with a `DOCS_BASE` matching the project Pages
subpath. A successful local build is not deployment proof; inspect the actual
GitLab pipeline and Pages URL.

For deeper diagnosis, use the owning source linked from the relevant page and
the [security model](#/concepts/security) before collecting logs.
