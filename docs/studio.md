# WorldStream Studio

WorldStream Studio is the local host-operator portal. It is a companion control
plane, not the Participant Console, and `worldstreamd` remains the only
authoritative Room runtime.

## Run locally

Start the Studio Supervisor and separately served portal:

```bash
pnpm studio:dev
```

Open <http://127.0.0.1:5174>. The portal asks the Supervisor at
`127.0.0.1:9420` for a live, typed daemon snapshot. The Supervisor probes the
daemon at `127.0.0.1:9410`. The Operations surface can start, gracefully stop,
or restart only the configured `target/debug/worldstreamd` process with
`config/development.toml`; `studio:dev` builds that daemon before serving the
portal.

To run only the Supervisor, use:

```bash
pnpm studio:supervisor
```

The Supervisor exposes typed status and configured-lifecycle routes under
`/api/v1/daemon`. It accepts `--bind`, `--daemon`, `--probe-timeout-ms`,
`--daemon-executable`, `--daemon-config`, `--graceful-stop-timeout-ms`, and
`--state-dir` options. The executable and configuration are fixed when the
Supervisor starts; requests cannot supply commands, arguments, environment
values, or paths. The Supervisor does not expose arbitrary command execution or
direct Room mutation.

## Verified live backups

The Backups Operations surface reports the configured storage profile's health,
backup support, verification result, and freshness as separate facts. For the
bundled SQLite profile, the Supervisor persists an idempotent operation beneath
its fixed `<state-dir>/backups/<operation-id>` root and asks the live daemon to
use SQLite's online backup mechanism. The daemon accepts only the stable
operation ID: it derives the exact `backup.sqlite3` child beneath its
startup-fixed owner-only backup root. At startup, both processes derive that
root through the same runtime policy from the controlled daemon configuration;
the Supervisor fails closed unless its canonical `<state-dir>/backups` is the
identical directory. The daemon reopens the published bytes and verifies the
native artifact, and compares deterministic source and destination exports as
an artifact-consistency check. It returns only a bounded pathless
name/size/checksum summary. That consistency digest is not the repository's
full semantic restore verifier, so the API reports semantic restore verification
as unavailable with the reason `full_semantic_restore_verification_not_run`.
An intent without a terminal record remains retryable after restart, including
the crash window where the artifact was published before the result record;
retrying uses the original operation identity.

PostgreSQL reports provider-managed backup as unsupported by this local live
workflow; ephemeral storage also reports unsupported. Capability is not storage
health. Studio does not browse or accept destination paths, schedule provider
snapshots, restore data, or turn this artifact into an automatic recovery
workflow. A failed or uncertain operation leaves the running store untouched;
restore and full post-restore readiness verification remain separate operator
workflows.

## Protected credential references

Credentials retained by the Supervisor live under the owner-only state
directory and are addressed by random opaque references. Host, Membership,
Runner, and future model-provider credentials use distinct kind-bound files, so
a reference cannot silently substitute authority from another boundary. Raw
values are resolved only inside the Supervisor and the resolved buffer is
zeroized when dropped.

Studio calls the read-only `/api/v1/secrets` endpoint. That response contains
exactly the configured, missing, or unavailable state for each supported kind;
it contains no credential reference, filesystem path, or secret value. The
exact-reference diagnostic endpoint is likewise metadata-only. Vault failures
use a closed, pathless error vocabulary suitable for logs and diagnostics.

## Owner-installed Runner Templates

Runner Templates are installed from the owner-controlled directory selected by
`--runner-templates-dir` (default `config/runner-templates`). Each JSON manifest
binds a stable template identity and immutable exact revision to one local
executable BLAKE3 digest. It also declares exact compatible Activity Pack
revisions, bounded capacity, a loopback HTTP health contract, explicit
non-secret environment settings, Supervisor secret references, and stable local
instance identities.

On first load the Supervisor verifies the executable bytes and copies the
normalized manifest into owner-only state under
`<state-dir>/runner-templates/installed`. Reusing the same template identity and
revision with different content fails closed. Installed records are not a
download catalog: Studio has no register, upload, marketplace, executable-path,
shell, command, argument, or environment-value endpoint.

The Build surface reads browser-safe immutable metadata from
`GET /api/v1/runner-templates`. The Operations surface reads health, freshness,
compatibility, capacity, and actionable failures from
`GET /api/v1/runner-instances`, then requests only these closed workflows:

```text
POST /api/v1/runner-instances/{installed-instance-id}/start
POST /api/v1/runner-instances/{installed-instance-id}/stop
POST /api/v1/runner-instances/{installed-instance-id}/restart
```

Those requests have no launch body. The Supervisor always starts the exact
owner-installed executable with its installed settings, resolves any secret
references locally, and requests a graceful stop. Persistent expected state is
reconciled after a Supervisor restart; a live instance is not launched again,
and an unreconciled or unhealthy instance is shown with a safe next action.

An owner manifest has this shape (replace the digest, path, and optional secret
reference with locally provisioned values):

```json
{
  "schema": "worldstream/runner-template/v1",
  "template_id": "local-mcp-helper",
  "revision": "r2",
  "display_name": "Local MCP Helper",
  "executable": {
    "path": "/owner/approved/bin/local-mcp-helper",
    "blake3": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "compatibility": [
    { "activity_pack_id": "agent-heist", "exact_revisions": ["1.0", "1.1"] }
  ],
  "capacity": { "maximum_concurrent_invocations": 4 },
  "health": { "path": "/healthz", "timeout_ms": 500, "stale_after_ms": 5000 },
  "non_secret_environment": { "LOG_LEVEL": "info" },
  "secret_environment": [],
  "instances": [
    { "instance_id": "local-mcp-helper-1", "health_address": "127.0.0.1:9501" }
  ]
}
```

Runner instance lifecycle is operational only. Runner authority remains
separate from participant Action authority, and the Supervisor never starts a
model or mutates Authoritative Room State.
