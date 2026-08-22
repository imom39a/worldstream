# IMO-59/60/61 release automation handoff

Date: 2026-08-21

This lane audited the compatibility manifest shape, packaging entrypoints,
gate runner, GitHub workflow, and release-packaging documentation. The
worktree was already dirty; no Rust, core, server, SQLite, transfer, UI, SDK,
manifest, or Linear files were changed by this lane.

## Changes

- `scripts/gates.py` now routes the declared `oci-linux-amd64` release
  platform as a Linux CI cell when the manifest does not duplicate it under
  `gate_cells`. Unknown or malformed platform routes still fail matrix
  generation.
- The OCI cell runs the packaging dry-run boundary only. It verifies the
  declared `linux/amd64` route and keeps image, pinned-base, runtime, and
  digest evidence external; it never treats a dry-run as an image result.
- Release gate reports include a deterministic
  `worldstream/release-evidence-handoff/v1` payload. The optional `--handoff`
  output lists the manifest-declared archive, OCI, checksum, Sigstore, SPDX,
  and SLSA deliverables, deterministic expected paths, owners, and the
  manifest's current status/digest values. It is explicitly marked
  `release_evidence=false` and does not infer or populate any digest.
- `.github/workflows/compatibility-gates.yml` uploads per-platform gate
  reports, includes the Linux/amd64 OCI cell in the manifest-driven matrix,
  and uploads the manual release gate plus handoff report even when the gate
  fails. The manual release job waits for both platform cells and the macOS
  source gate.
- `tests/gate_smoke.py` covers OCI matrix materialization and preservation of
  unresolved release evidence.

## Verification

Passed:

```text
python3 tests/package_smoke.py
python3 tests/gate_smoke.py
python3 -m py_compile scripts/gates.py tests/gate_smoke.py tests/package_smoke.py
uv run --project sdk/python --offline ruff format --check scripts/gates.py tests/gate_smoke.py tests/package_smoke.py
uv run --project sdk/python --offline ruff check scripts/gates.py tests/gate_smoke.py tests/package_smoke.py
bash -n scripts/package-release.sh scripts/package-oci.sh scripts/verify-release.sh scripts/gates.sh scripts/macos-source-quickstart.sh
python3 scripts/gates.py minimal-ci --list-cells --format matrix
```

The matrix output contains exactly:

```text
native-linux-x86_64 -> ubuntu-24.04/bash
native-windows-x64 -> windows-2025/pwsh
oci-linux-amd64 -> ubuntu-24.04/bash
```

The pinned offline pre-push gate was run with Node 24.18.1/pnpm 11.19.0. It
remained fail-closed. Its final summary was 56 checks, 16 failures, and one
incomplete PostgreSQL skip. The release gate was also run with atomic report
and handoff outputs; it remained fail-closed at 52 checks, 18 failures, and
zero incomplete skips. The handoff was emitted with eight declared artifacts,
all eight still carrying unresolved digests, and `release_evidence=false`.

`pwsh` is not installed on this macOS worker, so PowerShell execution/syntax
verification could not be performed locally. The Windows route remains a
hosted native CI obligation.

## Exact remaining blockers

The checked-in manifest remains `manifest_kind = "specification"` and
`release_ready = false`. The release gate still requires, without local
substitution:

- the SQLite bundled build digest and unresolved Agent Heist pack identity
  fields;
- native Linux x86-64 archive/runtime evidence;
- native Windows x64 archive, ACL/reparse, and runtime evidence;
- a built and exercised Linux/amd64 OCI image with a pinned base-image
  digest;
- macOS source/APFS execution evidence;
- migration, SQLite/PostgreSQL transfer, backend-native restore, crash/kill,
  privacy/capability/lease/filesystem, telemetry, fuzz/resource, and soak
  evidence;
- a release directory with exact `release-manifest.json` inventory;
- genuine `SHA256SUMS`, verified Sigstore bundle, SPDX SBOM, and SLSA
  provenance, with their exact artifact digests.

Observed unrelated gate failures in the dirty shared checkout included Rust
format/clippy/test issues in backup/server/telemetry code and Python test
collection failure from `examples/heist/wave10_live/test_absent_broker_live.py`
importing `run_absent_broker_live` without a package-qualified path. This lane
did not edit those out-of-scope files or weaken the gates.

No artifact digest, platform pass, signature, SBOM, provenance, or
`release_ready=true` claim was fabricated.
