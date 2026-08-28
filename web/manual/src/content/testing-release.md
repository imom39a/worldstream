# Testing, gates, and release evidence

Every test command proves a different boundary. Do not describe a unit test as
live evidence or a successful build as release readiness.

## Evidence ladder

| Command / workflow | Proves | Does not prove |
| --- | --- | --- |
| offline Heist story | deterministic fixture and Replay corpus | daemon, database, browser, network, provider |
| `scripts/gates.sh fast` | drift, formatting, focused lint/tests, goldens, locks, tracked-secret patterns | full workspace or release |
| `scripts/smoke-operator.sh` | disposable real SQLite daemon reaches health/readiness/version | full Room/client story |
| component suites | package behavior at tested seams | packaged deployment |
| `scripts/verify-local.sh` | broad pinned local checkout verification | signed distribution |
| strict pre-push/CI | declared repository/native matrix | detached release artifacts unless release tier runs |
| live Agent Heist gate | real daemon/Supervisor/MCP/Console story, restart recovery, Replay parity | provider SLA, public network, every model host |
| release gate | detached artifacts, checksums, SBOM, provenance, signatures | universal performance/availability SLA |

## Everyday commands

```sh
scripts/gates.sh fast
cargo test --workspace --locked
uv run --project sdk/python --python 3.14.7 pytest
pnpm ui:test
pnpm studio:test
pnpm docs:test
```

Full local verification:

```sh
scripts/verify-local.sh
```

Strict pre-push:

```sh
scripts/gates.sh pre-push --strict
```

## Activity Pack evidence

Every exact revision needs initialization/reduction/view/observation vectors,
Golden Genesis/Transition/Head hashes, stale Action behavior, timer races,
privacy/noninterference, recovery without snapshots, and executable Replay.
Retained old revisions must continue to decode and execute but need not remain
selectable for new Rooms.

## Release readiness language

`release_ready = true` in compatibility data means the contract's required
fields/evidence graph are complete. It is not proof that a release was built,
signed, uploaded, installed, or operated successfully.

The release gate has a distinct time/evidence boundary and produces detached
artifacts. Keep source build support, native release platforms, OCI support, and
developer-only platforms separate in documentation.

Source: [automated gates](https://github.com/imom39a/worldstream/blob/main/docs/gates.md),
[release packaging](https://github.com/imom39a/worldstream/blob/main/docs/release-packaging.md),
and [compatibility contract](https://github.com/imom39a/worldstream/blob/main/compatibility.toml).
