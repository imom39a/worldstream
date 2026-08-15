# ADR 0011: Gate Releases on Compatibility, Recovery, and Supply-Chain Evidence

Date: 2026-08-15

Status: Accepted

## Context

Engine support, migrations, backup, transfer, native platforms, containers, and build provenance affect whether acknowledged WorldStream state can be recovered. Treating them as packaging details would leave the durability promise untestable.

## Decision

Every release publishes and embeds its canonical Storage Compatibility Manifest. Release evidence covers the exact bundled SQLite build, supported PostgreSQL 17 patch policy, schema and migration checksums, retained canonical/receipt codec readers, exact Activity Pack executors, transfer and recovery formats, supported connection modes, and the platform matrix.

Backup remains backend-native: WorldStream owns SQLite backup/restore orchestration, while PostgreSQL uses operator/provider-native snapshot, PITR, dump, or restore facilities. Every restored deployment must then pass the read-only WorldStream semantic verifier. Global lineage or manifest failures block readiness; a byte-preserved Room already recorded as faulted or quarantined may remain isolated without blocking verified healthy Rooms.

The release matrix is native Linux x86-64, native Windows x64, Linux/amd64 OCI, and a macOS source-build quickstart. Releases include checksums, Sigstore signatures, SPDX SBOM, SLSA provenance, configuration and secret-handling checks, health/readiness/version contracts, vendor-neutral telemetry evidence, migrations, transfer, restore, failure injection, and backend conformance. Performance results are reference measurements, not universal SLAs.

## Consequences

Native Windows is a supported product path, not a cross-compilation claim. No cloud account is required to certify a release, and provider verification is scoped to a dated migration plus backup/restore/replay drill rather than provider plans, HA, SLA, regions, or durability promises.

The frozen releases do not ship ARM64 release artifacts, macOS binaries, Windows containers, MSI/MSIX, Windows Service integration, package repositories, Kubernetes/Helm assets, cloud resources, or release-pipeline implementation.
