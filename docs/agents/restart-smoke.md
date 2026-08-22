# WorldStream process/restart smoke evidence

This lane is intentionally bounded and fail-closed. It starts the real
`worldstreamd` binary on Linux with a private temporary `--data-dir`, probes
the operator HTTP surface, sends `TERM`, starts the same binary again against
the same directory, and finally checks that an invalid storage path cannot
serve readiness. Temporary logs, database paths, and synthetic bearer values
are created at runtime only; none are committed here.

## Exact commands

From the repository root:

```sh
cargo build --locked -p worldstream-server --bin worldstreamd
bash -n scripts/restart-smoke.sh
python3 tests/restart_smoke.py
bash scripts/restart-smoke.sh
```

The live command requires Linux, `curl`, Python 3, and an executable
`target/debug/worldstreamd`. A different locally built binary may be selected
without putting its path in evidence:

```sh
WORLDSTREAMD_BIN=/path/to/worldstreamd bash scripts/restart-smoke.sh
```

The harness uses a loopback listener, a bounded startup/shutdown window, an
owner-only temporary data directory, and an optional `WORLDSTREAM_SMOKE_PORT`.
It removes its temporary directory on exit.

## Observations required for a pass

- `/healthz` returns HTTP 200.
- `/readyz` returns HTTP 200 with `status: ready`.
- `/version` returns HTTP 200 and reports `manifest.release_ready: true`,
  `engine.status: verified`, and a non-empty `engine.exact_identity`.
- A SQLite file appears under the temporary data directory, survives the
  first graceful `TERM`, and is reopened at the same file identity after the
  restart. This is bounded file-persistence evidence only; it is not Room
  persistence evidence.
- The daemon exits within five seconds after `TERM`.
- A regular file supplied as `--data-dir` causes startup failure and never
  exposes a successful `/readyz` response.
- The Room probe never accepts a 2xx response. It uses a synthetic unknown
  bearer and therefore expects `403 forbidden`; that is explicitly recorded
  as an authority/bootstrap blocker, not as a created Room.

## Scope of the evidence

The daemon exposes a successful verified ready state, but this bounded smoke
lane intentionally has no authority-bootstrap fixture or secret capability.
Consequently it does not claim Room creation, Room replay, or durable Room
state across restart. Those behaviors are covered by the packaged reference
and recovery lanes; this harness proves the process, signal, restart, bounded
SQLite-file, verified runtime identity, and invalid-storage observations.

The Python test file uses a temporary fixture daemon to exercise those process
boundaries when a live Rust binary is absent. It also injects an unready
fixture and verifies that the shell harness rejects that result rather than
weakening the evidence gate.
