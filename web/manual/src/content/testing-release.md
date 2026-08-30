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
| qualification gate | authenticated official/custom Starters and six real outside-adopter receipts bound to one signed release | a replacement for primary release evidence or proof manufactured by repository tests |

The Starter Distribution is assembled only from exact subjects in that
detached signed inventory. `scripts/starter-distribution.py verify` checks its
closed offline contents and Sigstore identity; `--structural-only` deliberately
returns a non-release result for artifact-development tests. Verifying or
extracting a Starter never transports Pack approvals or mutates an
installation. See the repository's `docs/starter-distribution.md` runbook.

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

The local Negotiate golden-flow receipt explicitly sets
`release_evidence=false`. The release normalizer rejects it as an oracle or
privacy qualification input. Bundle conformance is read from the exact
detached `.wspack` and proved by `worldstreamctl` from the detached native
Linux archive; policy, A202, and both storage restart rows require closed
released-artifact qualification reports with checkout, debug-target, and
manual-database access disabled.

Portable-bundle startup evidence must also exercise the configured target
contract. Inventory and restart-readiness receipts must agree on
`storage_profile` and canonical `inventory_digest`; restart readiness must use
production Component admission and executable store Replay before recording
the pathless `deployment_binding` in a durable seal. A complete integration
lane then starts the daemon against that same target and proves it rejects a
missing, stale, wrong-profile, wrong-target, or post-mutation seal. Receipt
parsing alone does not prove daemon startup admission. Install, approval
revocation, selectability, retained restore, and removal each need a regression
showing that the prior seal is cleared.

## Release readiness language

The checked-in pair is currently `manifest_kind = "specification"` and
`release_ready = false`. The exact Negotiate Component, semantic revision,
schema/codec/golden, and physical bundle identities are now resolved; packaged
technical gates and genuine outside-adopter trials remain pending. A future
`release_ready = true` means only that the contract's required fields/evidence
graph are complete. It is not proof that a release was built, signed, uploaded,
installed, or operated successfully.

The release gate has a distinct time/evidence boundary and produces detached
artifacts. Keep source build support, native release platforms, OCI support, and
developer-only platforms separate in documentation.

Source: [automated gates](https://github.com/imom39a/worldstream/blob/main/docs/gates.md),
[release packaging](https://github.com/imom39a/worldstream/blob/main/docs/release-packaging.md),
and [compatibility contract](https://github.com/imom39a/worldstream/blob/main/compatibility.toml).

## Outside-adopter qualification

The final adoption gate requires six distinct non-contributors: three Pack
Authors within 60 minutes and three Application Integrators within 30 minutes,
using released artifacts and public documentation only across at least two
installation profiles. Both deterministic and prompt-assisted Pack authoring
must be represented.

`scripts/adopter-trial.py` validates the observer-produced redacted receipts
against one exact detached release-manifest identity. It rejects duplicate
people, missing journeys/checkpoints, mixed artifacts, forbidden maintainer or
source access, failed commands, time overruns, and secret-like diagnostics. It
never creates receipts and repository tests never count as outside people.

The canonical summary is consumed by `scripts/release-qualification.py` only
after the primary release is signed. That tool also authenticates official and
custom Starters against the exact primary manifest, creates three detached
qualification reports, and verifies a second signature over
`release-qualification-manifest.json`. This two-level design avoids signing a
manifest that recursively contains artifacts which embed that same manifest.

See the [outside-adopter trial runbook](https://github.com/imom39a/worldstream/blob/main/docs/outside-adopter-trials.md).
