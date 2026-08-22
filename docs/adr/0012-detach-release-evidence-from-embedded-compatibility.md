# ADR 0012: Detach Release Evidence from Embedded Compatibility Identity

Date: 2026-08-21

Status: Accepted

## Context

ADR 0011 requires every runtime artifact to embed the canonical compatibility
manifest and also requires that manifest to contain the SHA-256 of every final
archive and image. That interface is not implementable: changing an embedded
archive digest changes the archive bytes and therefore changes the digest
again. Evidence reports that bind the final binary create the same cycle when
their digests are compiled into that binary.

The bundled `SQLite` build identity also varies by native target. A single
cross-platform binary digest cannot truthfully identify the Linux, Windows,
and OCI builds. The portable embedded identity is the reviewed bundled source
inventory; each concrete build belongs in its artifact provenance.

## Decision

The release system has one deep verification module with two inputs:

1. The embedded compatibility contract identifies product, wire/config/storage
   schemas, toolchains, bundled source inputs, migrations, supported profiles,
   and exact retained pack executors.
2. A detached `release-manifest.json`, signatures, SBOM, and provenance bind
   the exact bytes of every archive, image, checksum file, evidence report, and
   platform build.

`release_artifacts` rows in the embedded manifest declare the closed artifact
ID/profile inventory with `status = "detached"`, an empty in-band digest, and
`digest_location = "release-manifest.json"`. Evidence rows use the equivalent
detached location. The release verifier accepts no artifact or evidence status
from those declarations; it recomputes every subject digest through the
detached release-manifest seam and verifies the signature, SPDX SBOM, and SLSA
provenance over the same closed subject set.

`release_ready = true` in the embedded manifest means the compatibility
contract has no unresolved implementation identity and is eligible to be
built. It does not mean an arbitrary copy of a binary is a verified release.
A distribution is verified only when the detached release bundle passes the
release verifier under the configured signing identity and issuer policy.

The portable `SQLite` contract records the exact reviewed bundled source
inventory digest. Per-platform compiled binary identities, compiler arguments,
and source-to-artifact relationships are recorded in SBOM and provenance.

## Consequences

- Published archive and image hashes are exact and non-self-referential.
- Runtime `/version` reports compatibility/build eligibility without claiming
  that detached supply-chain evidence accompanied the running binary.
- Missing, extra, substituted, or stale artifact/evidence subjects fail the
  release verifier even when the embedded contract is otherwise valid.
- Fixture manifests and structural-only checks cannot acquire release status.
- ADR 0011 remains authoritative for supported platforms, recovery, and hard
  gates; this ADR replaces only its in-band artifact/evidence digest placement.
