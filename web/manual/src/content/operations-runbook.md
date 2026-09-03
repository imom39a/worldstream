# Local operations runbook

Start with the smallest truthful signal. Liveness, readiness, integrity,
freshness, storage support, and task readiness are independent facts.

## Startup sequence

1. Validate config and owner-only paths with `worldstreamctl`.
2. While the daemon is stopped, run config-aware Pack restart readiness after
   any install, approval revocation, selectability change, retained-bundle
   restore, or removal.
3. Start the managed Runtime with `worldstreamctl server start`.
4. Wait for `/healthz` before assuming the listener is live.
5. Wait for `/readyz` before creating/serving ordinary Room work.
6. Confirm `/version` matches the expected source/compatibility identity.
7. Start or reconcile approved Runners.
8. Confirm required human session and agent Runner readiness before launch.

For bundled SQLite:

```sh
worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness
```

For PostgreSQL, use the same config plus an owner-only direct-admin DSN file:

```sh
worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness \
  --dsn-file <POSTGRES_ADMIN_DSN_FILE>
```

The command performs production Component admission and executable Replay,
then writes a durable seal over the receipt's exact `storage_profile`,
`inventory_digest`, and pathless `deployment_binding`. The daemon independently
derives the actual target binding at startup and refuses installed portable
bundles when the seal is missing or stale. Do not copy a receipt or seal from a
different config, data directory, profile, or provider deployment.

## First-response table

| Symptom | Inspect first | Safe next action |
| --- | --- | --- |
| process not reachable | `/healthz`, process output, configured bind | correct fixed config/port; start once |
| health works, readiness fails | `/readyz`, startup facts, storage/integrity reason, Pack readiness seal | fix the reported prerequisite; rerun config-aware Pack restart readiness while stopped when the seal is missing/stale; do not bypass readiness |
| Controller unavailable | Controller process and `9420/api/v1/daemon/status` | restart the fixed Controller process |
| Room unavailable | operator Room detail/integrity | keep fault local; follow repair evidence, not direct edits |
| Task cannot launch | Task setup readiness axes | provision missing seat/Runner or establish required session |
| agent is stale | Runner/Activation attention freshness | restart exact approved Runner/helper and reconcile lease |
| Action rejected stale | current Projection/Head and offers | resync; choose a new offer and operation ID |
| backup unsupported | storage capability, not health | use provider-specific offline workflow |
| ambiguous storage operation | retained operation status/receipt | resume/retry identical operation; never start a new identity blindly |

## Daemon lifecycle

The Controller's lifecycle operations are fixed at startup. Client requests
contain no executable path or arguments. Start/restart is idempotent against
retained expected state; graceful stop is bounded and does not silently force a
process after timeout.

When running manually:

```sh
RUST_LOG=info target/debug/worldstreamd --config config/development.toml
```

Stop with `Ctrl-C`. Avoid killing during a diagnostic unless the scenario is
explicitly testing crash recovery.

## Room health and integrity

Room Integrity State is operational, not canonical domain state. A faulted or
quarantined Room is not “archived” and should not be repaired by editing current
state. The verifier uses canonical history, retained pack execution, hashes,
materializations, and incident generation fences.

Global migration/authority corruption blocks readiness. Room-local corruption
should remain isolated so healthy Rooms can continue when the storage contract
allows it.

## Runner and Activation recovery

- identify the exact assignment, Runner instance, last authoritative
  observation time, lease state, and pending backlog;
- distinguish process liveness from MCP freshness;
- use the bounded restart action for the installed instance;
- allow the helper to resume the retained Cursor/lease/operation ledger;
- do not manually complete or ACK work you have not durably processed;
- escalate repeated closed failures with the safe reason code and no private
  payload.

## Reset disposable local state

Stop daemon, Supervisor, Runners, and Vite servers. If the entire local
installation is disposable, remove `.worldstream/` and repeat bootstrap. Do not
delete selected credential, receipt, or operation records to “unstick” a flow;
those cross-record invariants are intentionally fail-closed.

Source: [getting started](https://github.com/imom39a/worldstream/blob/main/docs/getting-started.md),
[runtime architecture](https://github.com/imom39a/worldstream/blob/main/docs/architecture.md),
and [CLI reference](https://github.com/imom39a/worldstream/blob/main/docs/cli-reference.md).
