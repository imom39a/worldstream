# Local operations runbook

Start with the smallest truthful signal. Liveness, readiness, integrity,
freshness, storage support, and task readiness are independent facts.

## Startup sequence

1. Validate config and owner-only paths with `worldstreamctl`.
2. Start `worldstreamd` or use Studio Operations.
3. Wait for `/healthz` before assuming the listener is live.
4. Wait for `/readyz` before creating/serving ordinary Room work.
5. Confirm `/version` matches the expected source/compatibility identity.
6. Start or reconcile approved Runners.
7. Confirm required human session and agent Runner readiness before launch.

## First-response table

| Symptom | Inspect first | Safe next action |
| --- | --- | --- |
| process not reachable | `/healthz`, process output, configured bind | correct fixed config/port; start once |
| health works, readiness fails | `/readyz`, startup facts, storage/integrity reason | fix the reported prerequisite; do not bypass readiness |
| Studio disconnected | Supervisor process and `9420/api/v1/daemon/status` | restart the fixed Supervisor process |
| Room unavailable | operator Room detail/integrity | keep fault local; follow repair evidence, not direct edits |
| Task cannot launch | Task setup readiness axes | provision missing seat/Runner or establish required session |
| agent is stale | Runner/Activation attention freshness | restart exact approved Runner/helper and reconcile lease |
| Action rejected stale | current Projection/Head and offers | resync; choose a new offer and operation ID |
| backup unsupported | storage capability, not health | use provider-specific offline workflow |
| ambiguous storage operation | retained operation status/receipt | resume/retry identical operation; never start a new identity blindly |

## Daemon lifecycle

Studio's lifecycle operations are fixed at Supervisor startup. Browser requests
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
and [Studio operations](https://github.com/imom39a/worldstream/blob/main/docs/studio.md).
