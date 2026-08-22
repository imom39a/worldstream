# Release gate diagnostics lane

Date: 2026-08-21

## Scope

This lane audited only `scripts/gates.py` and `tests/gate_smoke.py`, with this
handoff document as the lane record. Existing worktree changes were preserved.
`scripts/package.py` and the OCI runtime smoke script were not changed.

The checked-in compatibility manifest remains a specification with
`release_ready = false`. No placeholder artifact digest, evidence digest,
base-image identity, or signing claim was resolved or promoted.

## Changes

- Release artifact diagnostics now identify the compared artifact ID, profile,
  path, manifest digest, metadata digest, and digest observed from bytes.
  Manifest identity errors report expected and observed source, mirror, and
  compatibility-mirror digest values.
- Gate outcomes retain a classification of `pass`, `failure`, or `incomplete`.
  Missing external artifacts, unresolved evidence, unavailable signing
  material, and wrong-runner platform blockers are incomplete. Malformed
  metadata, unsafe paths, digest mismatches, and command failures remain
  failures. Strict/release mode still converts incomplete blockers to an
  effective `FAIL`, so the gate remains fail-closed while the report preserves
  the reason.
- The release blocker matrix requires coherent artifact profile/status/digest
  identity, a regular `release-manifest.json`, and both the exact platform
  target and its owning evidence/artifact rows. A gate failure can never leave
  the matrix in `ready` state.
- CI matrix output includes the declared platform and operating-system route.
  A native-platform mismatch reports the required runner, shell, system, and
  observed system as an explicit platform blocker.
- JSON reports include `status`, `fail_closed`, and `incomplete_blockers`,
  while retaining the existing summary and outcome sections. The release
  handoff remains `release_evidence = false`.

## Focused verification

```text
uv run --project sdk/python --offline pytest -q tests/gate_smoke.py
15 passed

uv run --project sdk/python --offline ruff check scripts/gates.py tests/gate_smoke.py
All checks passed

uv run --project sdk/python --offline ruff format --check scripts/gates.py tests/gate_smoke.py
2 files already formatted

/opt/homebrew/bin/python3 -m py_compile scripts/gates.py tests/gate_smoke.py
pass
```

The focused tests cover zero-test-count rejection, native Windows filesystem
incompleteness, exact CI platform blockers, artifact identity diagnostics,
placeholder preservation, and fail-closed report state.

## Remaining blockers

Release approval still requires the owning native Linux, native Windows, and
Linux/amd64 OCI jobs; the exact release bundle and checksums; real Sigstore
verification inputs; SPDX/SLSA documents tied to the final bytes; and every
release-gated evidence digest. This lane reports those blockers but does not
manufacture any of them.
