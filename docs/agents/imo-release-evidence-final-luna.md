# IMO-59/60/61 release-evidence final audit

Date: 2026-08-21

## Disposition

This is an evidence audit, not a release approval. No compatibility manifest,
gate policy, release status, Linear status, or artifact was changed. The only
file added by this lane is this report.

IMO-59, IMO-60, and IMO-61 remain incomplete. The implementation has useful
local packaging and fail-closed gate coverage, but the release contract still
requires native platform, live backend, durability, performance, and supply-
chain evidence that is not present in this checkout.

## Host and manifest facts

The audit ran in /Users/vinothshanmugam/code/agent-streamer on:

- macOS 15.6.1, Darwin arm64, with the workspace on APFS;
- Rust 1.97.1 and uv 0.12.5;
- Python 3.13.0, while .python-version requires 3.14.7;
- Node 23.1.0, while .node-version requires 24.18.1;
- no pnpm, pwsh, Syft, or SLSA verifier in PATH;
- PostgreSQL client 14.13, with no configured WORLDSTREAM_POSTGRES_URL;
- Docker 24.0.2, Podman 5.2.5, and cosign 3.1.3 present.

The authored TOML and canonical JSON mirror both report
manifest_kind = "specification" and release_ready = false. There are 14
evidence rows: one existing resolved PostgreSQL row and 13 unresolved rows,
12 of which are release-gated. All eight release artifact rows have
status = "unresolved" and an empty digest. No release directory or
dist/release-manifest.json exists.

The gate semantics are correctly fail-closed:

- compatibility.toml is the authored source and compatibility.json must be
  its deterministic byte-for-byte mirror;
- release mode requires release_ready = true, every release-gated evidence
  row to have a valid digest, and a complete release directory;
- the release handoff explicitly has release_evidence = false and never
  infers an external digest from source or fixture bytes;
- the OCI matrix route checks packaging shape only; image digest and runtime
  evidence remain external.

## Exact bounded checks

All commands below were run from the shared checkout.

    python3 scripts/manifest-evidence-wave6.py --json | <field summary>
    schema=worldstream/manifest-evidence-wave6/v1
    release_ready=False
    manifest_kind=specification
    sqlite_identity_consistent=True
    sqlite_migration_source_count=6
    postgres_migration_source_count=7
    agent_heist_manifest_identity_status=unresolved_by_design
    release_directory_created=False
    writes_manifest=False
    manufactures_external_evidence=False

    python3 tests/manifest_evidence_wave6.py
    Ran 4 tests in 0.794s
    OK

    python3 scripts/verify-manifest.py
    compatibility manifest verified (specification-only, release_ready=false)

    <TOML-to-JSON parity check>
    canonical_mirror_byte_parity=True
    parsed_value_parity=True

    python3 scripts/gates.py minimal-ci --list-cells --format matrix
    {"include":[
      {"cell":"native-linux-x86_64","runner":"ubuntu-24.04","shell":"bash"},
      {"cell":"native-windows-x64","runner":"windows-2025","shell":"pwsh"},
      {"cell":"oci-linux-amd64","runner":"ubuntu-24.04","shell":"bash"}
    ]}

    scripts/macos-source-quickstart.sh
    exit 1
    macOS source quickstart is missing required commands: pnpm
    Install the pinned toolchain before retrying; no build was attempted.

    scripts/package-release.sh --target linux-x86_64 --dry-run
    exit 0; dry-run only; release_ready=False; target/release/worldstreamd and
    target/release/worldstreamctl were blocked inputs.

    scripts/package-oci.sh --dry-run
    exit 0; dry-run only; blocked input: --base-image IMAGE@sha256:DIGEST;
    blocked input: target/release/worldstreamd and target/release/worldstreamctl.

    python3 tests/gate_smoke.py
    exit 0

    python3 tests/package_smoke.py
    exit 0; package/archive/OCI-context smoke and negative-policy checks passed.
    Synthetic smoke fixtures are not release artifacts or release evidence.

    python3 scripts/gates.py release --offline
    exit 1
    Gate summary: 50 checks, 19 failures, 0 incomplete skips

The release run passed local manifest parity, Rust lock/build/test/clippy,
Python lock/lint/tests, gitleaks, cargo-audit, SQLite tests, operator smoke,
and local filesystem/privacy/capability/lease/telemetry fixture probes. It
failed on release_ready, unresolved manifest fields and evidence digests,
Node/pnpm availability, the missing release directory, and the unresolved
contract rows for migration, transfer, restore, snapshot, kill-point, privacy,
capability, lease, filesystem, and telemetry-backpressure.

## Row-by-row truth boundary

| Unresolved row | What this host can truthfully produce | Full row disposition |
|---|---|---|
| manifest-syntax-parity | The authored/mirror parity check and the read-only manifest identity report pass. | Locally verifiable, but still unresolved because no release evidence artifact digest was promoted. This lane did not write one. |
| sqlite-conformance-migration-backup-restore-crash | Bundled SQLite migration, backup/restore, snapshot, and recovery fixtures pass locally. | Not complete. Crash/power-loss durability and native operational restore semantics need a dedicated durability run; the local fixture result is insufficient for the release row. |
| all-prior-forward-migrations-both-backends | Source inventory reports 6 SQLite and 7 PostgreSQL migrations; local fixture checks pass. | Requires full forward-history execution against the supported PostgreSQL release environment as well as the bundled SQLite path. No live PostgreSQL evidence was produced here. |
| sqlite-postgresql-transfer-byte-parity-and-epoch-fencing | Deterministic transfer and in-memory destination fixtures pass. | Requires a real SQLite-to-PostgreSQL adapter transfer, canonical-byte comparison, durable checkpoints/finalization, and epoch fencing on a live supported PostgreSQL environment. External backend infrastructure is required. |
| backend-native-isolated-restore-and-bounded-semantic-verifier | Bundled SQLite backup and read-only verifier fixtures pass. | Requires PostgreSQL snapshot/PITR/dump/isolated restore and the explicit single-Pack/no-resource/no-fired-Timer semantic fixture, plus native durability evidence. General deployment support is not claimed; external PostgreSQL/provider or CI infrastructure is required. |
| native-linux-release-profile | Linux package shape and archive policy can be inspected by dry-run. | Requires a native Linux x86-64 runner to build, start, probe, and hash the supported archive. Cross-building on this arm64 macOS host is not equivalent. |
| native-windows-release-profile | No Windows execution or ACL evidence is available. | Requires the declared native Windows x64 / PowerShell runner for archive, DACL, reparse/path, runtime, and digest evidence. |
| oci-linux-amd64-release-profile | OCI context policy and non-root/read-only-root/volume shape are covered by dry-run and smoke tests. | Requires a built and exercised Linux/amd64 image with a pinned base-image digest and runtime evidence. The local OCI context is explicitly not an image result. |
| macos-source-quickstart | The host is Darwin/APFS and is platform-compatible. | Not producible with the currently installed toolchain: pnpm is absent, Node is 23.1.0 instead of 24.18.1, and Python is 3.13.0 instead of 3.14.7. Install the pins locally or use the declared macOS CI runner; no signed/notarized binary is implied. |
| config-secrets-probes-observability-security | Local redaction, bounded telemetry, capability, lease, POSIX mode, and symlink-policy probes pass. | The combined row also covers cross-process behavior, native Windows ACL/reparse policy, and supported remote PostgreSQL TLS/runtime behavior. Local partial probes cannot resolve the combined release row. |
| checksums-signature-sbom-provenance | Checksums are mechanically computable once the exact four payload artifacts exist; package smoke validates document shape and negative cases. | Requires the exact release bundle, verified Sigstore identity/issuer and bundle, SPDX generation, and SLSA provenance whose subjects match the final bytes. Syft/SLSA tooling and release signing identity are absent; the full row requires release CI/signing infrastructure. |
| failure-fuzz-resource-and-one-hour-sqlite-soak | Cargo and the repository’s bounded fault/soak tooling are present; local execution is technically possible. | Not run in this bounded audit. It needs a dedicated long-duration campaign with retained logs/digest and the frozen reference conditions. External infrastructure is not logically mandatory, but a CI/soak runner is the appropriate evidence owner. |
| reference-performance-per-backend (non-release-gated) | Local microbenchmarks could be run, but would not match the frozen reference profile. | Official evidence requires the Linux reference profile and both supported backends, including PostgreSQL. This host cannot produce the required comparable per-backend reference without the Linux/PostgreSQL environment. |

The eight unresolved release artifacts have the same boundary: a source
archive and checksum file are mechanically assembleable in a release workspace,
but the native Linux archive, native Windows archive, OCI image, verified
Sigstore bundle, SPDX SBOM, and SLSA provenance require their declared
platform/build/signing environments. No exact artifact digest is claimed here.

## External-blocker list

1. Native Linux x86-64 CI for archive/runtime/readiness/version and filesystem
   evidence.
2. Native Windows x64 CI with PowerShell, DACL/reparse/path, and runtime
   checks.
3. Linux/amd64 OCI build/runtime with a pinned base image and image digest.
4. Supported PostgreSQL infrastructure for full migrations, live transfer,
   native snapshot/PITR/dump restore, semantic verification, and the required
   backend performance profile. The installed PostgreSQL client 14.13 is not
   the supported PostgreSQL 17.11 evidence environment.
5. A pinned macOS toolchain (Node 24.18.1, pnpm 11.19.0, Python 3.14.7) for
   the source quickstart; this can be supplied by local installation or the
   declared macOS CI runner.
6. A release assembly workspace containing the exact payloads and
   release-manifest.json.
7. Release signing/OIDC identity for a verified Sigstore bundle, pinned SBOM
   generation, and CI-attested SLSA provenance.
8. A retained long-duration failure/fuzz/resource/soak run and the Linux
   reference performance run.

No Linear status was changed. No compatibility field, digest, platform pass,
signature, SBOM, provenance, or release_ready=true claim was fabricated.
