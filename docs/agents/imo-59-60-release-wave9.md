# IMO-59/60 release packaging wave 9B

Date: 2026-08-21

This worker audited the release tooling against `docs/release-packaging.md`,
ADR 0011, `scripts/package.py`, the shell wrappers, `tests/package_smoke.py`,
and `scripts/gates.py`. The worktree was intentionally dirty. No commit was
created, and no compatibility manifest, server/core/storage, or scheduler file
was changed.

## Changes

- `scripts/package.py` now owns OCI context verification and the canonical
  content inventory. `oci-context` verifies its generated output before
  returning, and `verify` recognizes a generated OCI context as distinct from
  a release evidence directory.
- OCI reports use
  `worldstream/oci-context-report/v1` with a sorted
  `worldstream/artifact-inventory/v1` list containing every regular file,
  path, byte size, and SHA-256. Reports are marked `release_evidence=false`.
  The verifier checks the inventory again after validation and fails if the
  context changed during verification.
- Fixed the OCI canonical metadata boundary: `metadata/release.json` is
  reconstructed from package payload and profile files, excluding both
  `metadata/release.json` and `checksums.sha256`. The checksum file is added
  afterward and covers the packaged payload exactly; it is not part of the
  metadata input and cannot create a self-reference.
- `scripts/package-oci.sh` is now a thin interpreter wrapper, eliminating a
  second shell-local verifier. `scripts/verify-release.sh` can write the same
  verified OCI report when given an OCI context and `--report`.
- `scripts/macos-source-quickstart.sh` reports missing tool commands before
  building, validates `.python-version`, uses locked SDK/UI installs, and runs
  SDK and UI tests before the UI build and manifest check.
- Gate reports now include deterministic `summary`, `blocking_failures`, and
  `incomplete_skips` sections and print the same blockers to stderr.
- Updated release/gate documentation and added focused regression coverage.

## Exact verification

All of the following passed after the OCI fix:

```text
python3 tests/package_smoke.py                         pass
uv run --project sdk/python --locked pytest -q tests/gate_smoke.py   9 passed
uv run --project sdk/python --locked ruff check scripts/package.py scripts/gates.py tests/package_smoke.py tests/gate_smoke.py   pass
uv run --project sdk/python --locked ruff format --check scripts/package.py scripts/gates.py tests/package_smoke.py tests/gate_smoke.py   pass
python3 -m py_compile scripts/package.py scripts/gates.py tests/package_smoke.py tests/gate_smoke.py   pass
bash -n scripts/package-oci.sh scripts/package-release.sh \
  scripts/verify-release.sh scripts/macos-source-quickstart.sh         pass
python3 scripts/verify-manifest.py                    pass
scripts/package-release.sh --target linux-x86_64 --dry-run             pass
scripts/package-oci.sh --dry-run                                      pass
```

`tests/package_smoke.py` now proves deterministic fixture archives for Linux,
Windows, and source profiles; generated OCI contexts verify successfully with
18 files and the stable fixture inventory digest
`sha256:cc1e7f38a2953339414fa3644da84f0d55cd5d638ba96390dcdf8ff52680edda`;
the full OCI report is written and re-verified through both wrappers. It also
regresses the checksum/metadata cycle that previously failed with
`OCI metadata/release.json is not canonical`.

The real repository release gate was run as:

```text
WORLDSTREAM_GATE_REPORT=/tmp/worldstream-wave9-release-gate.json \
  scripts/gates.sh release --offline
```

It correctly failed closed: 52 checks, 19 failures, 0 incomplete skips. The
new report summary and blocker list are deterministic. Packaging changes did
not create release evidence or change the manifest.

## Remaining requirements

The repository still has `manifest_kind = "specification"` and
`release_ready = false`. The nine unresolved manifest classes remain
intentionally unresolved, including the SQLite bundle digest, pack executor
revision/artifact/descriptor/schema/codec/golden-corpus digests, release
artifact digests, and evidence artifact digests.

The release tier still requires independently produced evidence for native
Linux and Windows runtime/ACL cells, a built and digested Linux/amd64 OCI
image, an actually executed macOS/APFS source quickstart, migration/transfer/
restore and failure/soak drills, privacy/capability/lease/filesystem/
telemetry evidence, a release directory, and genuine checksums, Sigstore
signature, SPDX SBOM, and SLSA provenance. This wave produces only a verified
OCI build context and non-release inventory; it does not claim any of those
external results.

The same release-gate run also observed pre-existing dirty-worktree Rust
failures outside this worker's scope: formatting differences in
`crates/worldstream-sqlite/src/lib.rs`, `clippy::cast_possible_wrap` in that
file, and missing `scheduler_runtime` initializers in
`crates/worldstream-server/src/lib.rs`. They were not modified because this
worker is prohibited from changing server/core/storage or the scheduler lane.
