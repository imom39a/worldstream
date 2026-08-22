# Daemon readiness

`worldstreamd` reports readiness from runtime-owned facts rather than a
bootstrap-milestone stub.

For the bundled SQLite profile, successful `SqliteRoomStore::open` establishes
the schema, durable storage, and writer-startup facts. The daemon then consumes
one exact 32-byte bootstrap secret from an owner-readable file or inherited
readable handle and calls Core's public `AuthorityV1::bootstrap` seam through
the SQLite adapter. SQLite durably installs the generation-one host Principal
and `operator:room_admin` Capability, or returns the exact durable receipt on a
replay. A different secret, an occupied authority store, missing material, or
invalid material fails startup closed. Only the token hash is persisted; the
bearer is never logged or placed in the effective configuration.

The versioned configuration shape is:

```toml
config_version = 1

[authority.bootstrap]
secret_file = "/owner-only/worldstream-authority.secret"
```

The equivalent inherited-handle environment key is
`WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_HANDLE`. The file/handle contains
exactly 32 raw bytes (no newline). `worldstreamctl config effective` reports
only `owner_readable_secret_file` or `inherited_handle` and `[REDACTED]`; it
never emits the path, handle, or bytes. There is deliberately no plaintext
secret CLI flag.

`/healthz` remains a liveness probe and returns `200`. `/version` reports the
selected engine as `verified` with its exact bundled SQLite version/source
identity after successful startup. PostgreSQL startup remains fail-closed
because no daemon adapter is available.

Authority bootstrap does not make `/readyz` return `200`: the daemon still has
no real scheduler/Room admission loop, timer dispatch loop, or activation
worker. It reports `503` with `storage_not_initialized` and check details that
show `authority_bootstrap=true` and `scheduler=false`. The SQLite gateway's
real authenticated Room create path is nevertheless wired and can be used
with the bootstrap bearer; readiness remains false until a scheduler-owned
runtime is integrated.

The parent-runnable server tests named `daemon_readiness_*` cover:

- the `200 {"status":"ready"}` branch only when every readiness fact is true;
- the SQLite-startup-shaped `503` response, including scheduler and authority
  blockers and machine-readable check details;
- the authority-bootstrapped-but-no-scheduler `503` response, proving that
  bootstrap does not fake readiness; and
- rejection of an existing regular file used as a data-directory path.

The focused daemon tests also cover missing/short bootstrap material and exact
bootstrap replay. The startup SQLite open and its schema, integrity, WAL, and
writer checks are real and fail closed. Live readiness remains intentionally
unproven until the scheduler boundary above is implemented.
