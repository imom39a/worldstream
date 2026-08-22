# ADR 0011: Gate Releases on Compatibility, Recovery, and Supply-Chain Evidence

Date: 2026-08-15

Status: Accepted

## Context

Engine support, migrations, backup, transfer, native platforms, containers, and build provenance affect whether acknowledged WorldStream state can be recovered. Treating them as packaging details would leave the durability promise untestable.

## Decision

Every release publishes and embeds its canonical Storage Compatibility Manifest. Release evidence covers the exact bundled SQLite build, supported PostgreSQL 17 patch policy, schema and migration checksums, retained canonical/receipt codec readers, exact Activity Pack executors, transfer and recovery formats, supported connection modes, and the platform matrix.

Artifact and evidence byte digests are detached from the embedded manifest as
specified by [ADR 0012](0012-detach-release-evidence-from-embedded-compatibility.md).
The embedded manifest declares the closed contract and subject inventory; the
signed release manifest, SBOM, and provenance bind final artifact and evidence
bytes without a cryptographic self-reference.
Every compiled release binary embeds the exact source commit exposed by
`/version`; SPDX and SLSA are accepted only when their locked component,
material, toolchain, target, runner, compiler, build-argument, and OCI-base
relationships reproduce the payload build identities exactly.

The repository may author a specification pair before implementation, but it remains fail-closed with `release_ready = false` while portable implementation identities are unresolved. A fully populated, semantically identical TOML/JSON pair may set `release_ready = true` to identify a complete buildable contract. As clarified by ADR 0012, that embedded flag is not artifact or acceptance evidence; only the detached signed inventory can verify a distribution.

Backup remains backend-native: WorldStream owns SQLite backup/restore orchestration, while PostgreSQL uses operator/provider-native snapshot, PITR, dump, or restore facilities. Every restored deployment must then pass the read-only WorldStream semantic verifier. Global lineage or manifest failures block readiness; a byte-preserved Room already recorded as faulted or quarantined may remain isolated without blocking verified healthy Rooms.

The release matrix is native Linux x86-64, native Windows x64, Linux/amd64 OCI, and a macOS source-build quickstart. Releases include checksums, Sigstore signatures, SPDX SBOM, SLSA provenance, configuration and secret-handling checks, health/readiness/version contracts, vendor-neutral telemetry evidence, migrations, transfer, restore, failure injection, and backend conformance. Performance results are reference measurements, not universal SLAs.

## Consequences

Native Windows is a supported product path, not a cross-compilation claim. No cloud account is required to certify a release, and provider verification is scoped to a dated migration plus backup/restore/replay drill rather than provider plans, HA, SLA, regions, or durability promises.

The frozen releases do not ship ARM64 release artifacts, macOS binaries, Windows containers, MSI/MSIX, Windows Service integration, package repositories, Kubernetes/Helm assets, or cloud resources.
