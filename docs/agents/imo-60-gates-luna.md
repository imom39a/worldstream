# IMO-60 compatibility, security, and supply-chain gates

Date: 2026-08-21
Lane: Luna gate automation audit and implementation

## Implemented in this lane

- Fresh CI runners now install and invoke the exact gate Python `3.14.7`
  through `uv`; the fast job still runs `cargo fetch --locked` before its
  offline gate. Native cells retain the parent’s exact PostgreSQL 17.11
  Linux image digest and Windows PostgreSQL 17 service provisioning.
- Source drift now runs locked `xtask compat verify` and the Wave 6
  manifest/pack/executor identity audit. A manifest, artifact, schema, codec,
  or pack-executor identity mismatch is a blocking gate result.
- Fast, pre-push, and native tiers run named SQLite property/fuzz, PostgreSQL
  adapter, and Core golden-corpus contract slices in addition to the existing
  workspace checks.
- Linux native/OCI CI and strict pre-push invoke the real pinned
  `scripts/postgres-live-evidence.sh` boundary. The gate validates the
  machine-readable result, including direct/runtime conformance, verified
  PgBouncer transaction pooling, the seven-scenario shared comparison,
  cleanup, and `release_evidence=false`; a successful process exit alone is
  not accepted.
- Pre-push/native tiers execute the process kill-point, telemetry-failure, and
  bounded SQLite soak probes when the native launcher is available. Windows
  cannot substitute fixture tests for POSIX process evidence; strict/CI mode
  reports that gap as incomplete/failing.
- PowerShell gate bootstrap now rejects Python older than 3.11 and falls back
  to the Windows `py -3` launcher when `python` is absent.
- Added regressions for exact CI Python bootstrapping, locked dependency
  priming, pinned native PostgreSQL setup, and manifest-driven matrix shape.
- Release identity is explicitly detached from the embedded compatibility
  contract. The gate accepts `status = "detached"` rows only when their empty
  digest fields point to `release-manifest.json`, requires the detached v2
  inventory to cover every release artifact and release-gated evidence row,
  recomputes every detached hash, and rejects self-referential or extra paths.
- Checksums, SPDX subject checksums, and SLSA subjects use the same exact
  payload/evidence subject set. The detached manifest records the hashes of
  those subjects plus `SHA256SUMS`, SPDX, and SLSA; only the Sigstore bundle is
  excluded from signed subjects and handled as verification material. Cosign
  verifies the detached `release-manifest.json` itself; missing cosign or
  certificate identity/issuer still fails the release tier. Compatibility
  `release_ready = true` is never used as a substitute for detached byte
  evidence.
- The manual release workflow now provisions the pinned Rust/Node/pnpm/uv
  gate toolchain, builds the supported native/OCI/source surfaces, and
  keyless-signs the detached manifest with release-job-only OIDC permission
  once the reviewed assembler and SPDX/SLSA generators exist; it does not add
  provider credentials or turn baseline/provider-neutral lanes into provider
  tests.

## Verification

Passed:

```text
uv run --project sdk/python --offline pytest -q \
  tests/gate_smoke.py tests/supply_chain_evidence.py
31 passed

uv run --project sdk/python --offline ruff format --check \
  scripts/gates.py tests/gate_smoke.py
2 files already formatted

uv run --project sdk/python --offline ruff check \
  scripts/gates.py tests/gate_smoke.py tests/supply_chain_evidence.py
All checks passed

bash -n scripts/gates.sh scripts/supply-chain-evidence.sh
passed
```

The release workflow also parses as YAML. Its detached assembler, pinned
`cosign`, and reviewed SPDX/SLSA generation interfaces are intentionally
checked before signing; their absence is an explicit release blocker.

The real local PostgreSQL/PgBouncer run used the checked-in pinned images and
returned:

```text
status=incomplete
release_evidence=false
pooler=pass
adapter=pass
cleanup=pass
imo50_shared_direct=failed
imo50_shared_pooler=failed
transfer=blocked_missing_global_pack_resource_evidence
```

This is an honest IMO-50/IMO-51 dependency result, not a gate implementation
failure or a release claim.

## Remaining external evidence

IMO-60 remains blocked on detached distribution evidence, not on the embedded
contract. The parent-promoted compatibility manifest is `manifest_kind =
"release"`, `release_ready = true`, with detached artifact/evidence rows and
empty unresolved fields. Non-release tiers validate that complete embedded
contract but do not claim distribution proof. The remaining evidence includes
the complete shared PostgreSQL conformance comparison, whole-deployment
transfer pack and resource identities, native Windows ACL/reparse and runtime
evidence, native Linux/Windows/OCI/macOS release artifacts, backend-native
restore and power-loss/soak evidence, and a genuine detached release
inventory plus checksum/Sigstore/SPDX/SLSA artifacts. No archive/evidence
digest, support label, signature, SBOM, provenance, or release readiness was
fabricated by this lane.
