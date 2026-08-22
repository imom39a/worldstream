# IMO-49 migration contract — Wave 7

Date: 2026-08-21
Lane: parent-orchestrated SQLite migration-contract implementation

## Outcome

SQLite now exposes a typed six-entry `MigrationDescriptor` inventory in
`crates/worldstream-sqlite/src/migration_contract.rs`. Each descriptor keeps
the immutable version, ID, and exact SQL body together and computes the same
canonical unkeyed BLAKE3-256 digest used by PostgreSQL
`MigrationDescriptor::checksum`.

Startup migration verification now consumes that inventory. It accepts only an
empty or contiguous forward prefix with the reviewed IDs, then applies the
next bodies from the same descriptors. Gaps, unsupported/future versions, and
identity drift fail before a body is executed. Existing transaction atomicity,
restart retry behavior, schema verification, and forward-only/no-downgrade
behavior are unchanged.

## SQLite checksum inventory

These are BLAKE3 digests of the exact raw migration-body bytes:

| Version | SQLite ID | Checksum |
|---:|---|---|
| 1 | `0001-initial-storage-schema` | `blake3:dd07208c71d7165b93861883b25411b1e7c33a6be36fc2be28a638e1ab5cd763` |
| 2 | `0002-operational-authority-v1` | `blake3:237088a0f888ef9f91a1010efd95e38a40170b0fc968229b886881937af805b0` |
| 3 | `0003-observation-delivery-v1` | `blake3:b74d06ed529d415a658eaede5067f24e02ff5de83ac07480a5df7de1645c8bcb` |
| 4 | `0004-activation-work-v1` | `blake3:dbe807e620fc77594e871b1ad90e89379498060fd025fbbaa318396158557d2b` |
| 5 | `0005-paired-snapshots-v1` | `blake3:385af50337813e01ce0a97894fcb82868e130d69e671f64712f6701c0e9ddb23` |
| 6 | `0006-canonical-export-metadata-v1` | `blake3:60de4825b3796865acff18f836dfa475640324b71d168350a8ea20c2e06206d5` |

The PostgreSQL manifest entries now also cover all seven reviewed descriptors:

| Version | PostgreSQL ID | Checksum |
|---:|---|---|
| 1 | `0001-initial-storage-schema` | `blake3:cda5b4edaed76bd2beac2df1f5cd89dbcdcf72fd2424dc4576042370f61471a6` |
| 2 | `0002-operational-authority-v1` | `blake3:b8f4ec2e2d47de2d9979c21505da698fc9b768f53ea4e8ef9429b8f042be9175` |
| 3 | `0003-kernel-conformance-v1` | `blake3:cc5bd1a11223932ebe239ce7de97d88d6390b8651f90f118bf069d3f3798da0d` |
| 4 | `0004-kernel-parity-witnesses-v1` | `blake3:eba8944ea435d27e737a22164ed314fc62d08a76eb068e5c7eedb9ae2fc6d4ea` |
| 5 | `0005-transfer-publication-v1` | `blake3:a2d43f76d7986a17d8a975be9f0bcbc2dcf16290b01b08dd5ef606f21ffc2cea` |
| 6 | `0006-transfer-target-fence-v1` | `blake3:7a0b8e471f67d33d6610bb39b96fb64bd1a0cd56a3d1557ad2f843e89d0078d5` |
| 7 | `0007-deployment-metadata-v1` | `blake3:ebc57f9b917e3655f1753c5543d517f8642d5ff9de43ef0cc7b3795d9c43dc03` |

The compatibility manifest keeps its existing PostgreSQL descriptor IDs as
the entry IDs. SQLite checksums are aligned by migration version because the
two backends retain their existing backend-specific IDs; no canonical ID was
renamed. The seventh entry intentionally has an empty SQLite checksum because
SQLite currently ends at version six.

## Tests and commands

```text
cargo fmt --all -- --check
  pass

cargo test --locked -p worldstream-sqlite --lib migration -- --nocapture
  6 passed; 0 failed

python3 scripts/manifest-evidence-wave6.py
  manifest evidence wave6 validation: pass
  manifest parity: semantic_equal=True, canonical_mirror_equal=True
  SQLite migration bodies: 6

python3 scripts/verify-manifest.py
  compatibility manifest verified (specification-only, release_ready=false)

python3 - <<'PY'  # read-only source-to-manifest parity check
  migration manifest parity: sqlite=6 postgres=7 version-aligned; sqlite-v7=empty
PY
```

The focused tests prove contiguous IDs, deterministic checksums, all valid
prefixes, identity drift, mixed/gapped prefixes, future identities, and
downgraded versions fail closed. Existing migration interruption/restart and
native-backup migration tests also passed in the same filtered run.

## Runtime persistence boundary

The existing SQLite `schema_migrations` table stores only `version` and
`migration_id`; it has no checksum column. Adding one in this lane would
require rewriting the already-canonical schema history or introducing a new
forward migration solely to describe the ledger itself, which would change
the accepted migration contract and risk old-database restart behavior. This
lane therefore verifies persisted identity against the typed source inventory
and publishes deterministic source checksums, but does not claim persisted
SQLite checksum verification. A separately reviewed ledger-extension
migration and upgrade evidence are required before that stronger property can
be added safely.

## Residual evidence

`compatibility.toml` remains `manifest_kind = "specification"` and
`release_ready = false`. The migration checksum field was removed from
`unresolved_required_fields`; release/artifact/evidence digests remain
unresolved. Native release profiles, crash/power-loss migration evidence,
full cross-backend conformance, transfer/restore evidence, and signatures/SBOM
provenance remain external release gates.
