# Release/evidence closure audit

Date: 2026-08-21

This report covers the release/evidence surface for the fifteen active
WorldStream acceptance rows in Linear:

`IMO-44`, `IMO-48`, `IMO-49`, `IMO-50`, `IMO-51`, `IMO-52`, `IMO-53`,
`IMO-54`, `IMO-55`, `IMO-56`, `IMO-57`, `IMO-58`, `IMO-59`, `IMO-60`, and
`IMO-61`.

The release-specific acceptance rows remain `In Progress`. This lane made a
local automation improvement only; it did not change any issue status,
manifest digest, signature, SBOM, provenance, platform claim, or
`release_ready` value.

## Audit result

The checked-in compatibility pair is still a specification:

- `manifest_kind = "specification"`;
- `release_ready = false`;
- `release_status = "implementation_in_progress_release_evidence_incomplete"`;
- unresolved fields include the SQLite bundle build digest, all pack-executor
  release identities, every release artifact digest, and release-gated
  evidence digests.

The local implementation already provides deterministic source/archive/OCI
packaging mechanics, embedded manifest identity, archive checksums, profile
metadata, OCI context inventories, and structural Sigstore/SPDX/SLSA shape
validation. These are implementation and handoff checks, not release evidence.

## Local change

`scripts/gates.py` now accepts `--report PATH` and writes an atomic JSON gate
report. Release reports include the
`worldstream/release-blocker-matrix/v1` matrix. Each row records the exact
manifest artifact or release-gated evidence row, whether its digest/status is
resolved, the source field, the required environment, and the next evidence
action.

The matrix is fail-closed and never derives a digest from source bytes, test
fixtures, or a local implementation identity. The documentation is in
`docs/release-packaging.md`.

## Verification

Commands run with the repository toolchain and their results:

```text
python3 tests/package_smoke.py                         PASS
python3 tests/manifest_evidence_wave6.py               PASS (4 tests)
uv run --project sdk/python --locked pytest -q tests/gate_smoke.py
                                                         PASS (10 tests)
python3 -m py_compile scripts/gates.py tests/gate_smoke.py
                                                         PASS
git diff --check                                        PASS
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  scripts/gates.sh pre-push --offline \
  --report /tmp/worldstream-pre-push-gate-report.json
                                                         FAIL CLOSED:
                                                         56 checks, 14 failures,
                                                         1 incomplete skip
PATH=/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin:$PATH \
  scripts/gates.sh release --offline \
  --report /tmp/worldstream-release-gate-report.json
                                                         FAIL CLOSED:
                                                         52 checks, 15 failures
```

The pre-push failures include the unrelated, pre-existing incomplete
`examples/heist` wave-9 format/lint/test-collection checks, the unresolved
release/evidence contract rows, and one incomplete optional PostgreSQL URL
cell. The release failures include the specification manifest state, unresolved
artifact and evidence identities, the absent release directory, and contract
rows that cannot be promoted from local fixtures.

The final release report was written to
`/tmp/worldstream-release-gate-report.json`; it contains 24 matrix rows, 23
blocked rows, and no fabricated artifact identity. The final pre-push report
was written to `/tmp/worldstream-pre-push-gate-report.json`.

## External evidence still required

The following cannot be truthfully completed by this checkout alone:

- native Linux and native Windows package outputs, including Windows ACL/path
  and filesystem evidence;
- a pinned Linux/amd64 OCI base image, built image, and exercised runtime;
- macOS source quickstart evidence on the target Darwin environment;
- exact release bundle artifact digests and `SHA256SUMS`;
- a real Sigstore bundle verified by the release signing identity and issuer;
- an SPDX SBOM and SLSA provenance whose subjects match the exact payload
  bytes; and
- the remaining backend, transfer/restore, failure, privacy, lease,
  filesystem, telemetry, performance, soak, and full packaged acceptance
  reports listed by the manifest.

The local gate report is therefore an evidence handoff and precise blocker
matrix, not a release approval.
