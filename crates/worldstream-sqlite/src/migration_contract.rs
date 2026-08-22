//! Stable `SQLite` migration identities and source checksums.
//!
//! The `SQLite` ledger stores a contiguous version, migration identifier, and
//! the exact reviewed source checksum for every applied migration. This module
//! keeps the reviewed migration bodies in one typed inventory and centralizes
//! the fail-closed contract checks used by startup and native backup paths.

use thiserror::Error;
use worldstream_core::Blake3DigestV1;

/// Stable logical migration history shared by the storage profiles.
pub const LOGICAL_HISTORY_ID: &str = "worldstream-storage-v1";

/// A `SQLite` migration body and its stable identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationDescriptor {
    /// One-based, contiguous migration version.
    pub version: i32,
    /// Stable logical identifier.
    pub id: &'static str,
    /// Backend-specific SQL body. It is immutable after release.
    pub sql: &'static str,
}

impl MigrationDescriptor {
    /// Returns the canonical BLAKE3-256 checksum of the exact migration body.
    #[must_use]
    pub fn checksum(self) -> Blake3DigestV1 {
        Blake3DigestV1::hash(self.sql.as_bytes())
    }
}

/// Migration history failures are deliberately explicit and fail closed.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MigrationVerificationError {
    #[error("migration history is not contiguous at version {version}")]
    NonContiguous { version: i64 },
    #[error("migration version {version} is unsupported")]
    Unsupported { version: i64 },
    #[error("migration version {version} has identity {found:?}, expected {expected:?}")]
    IdentityDrift {
        version: i64,
        expected: &'static str,
        found: String,
    },
    #[error("migration version {version} has checksum {found:?}, expected {expected:?}")]
    ChecksumDrift {
        version: i64,
        expected: String,
        found: String,
    },
}

/// Returns the complete ordered `SQLite` forward migration history.
#[must_use]
pub fn migration_history() -> [MigrationDescriptor; 8] {
    [
        MigrationDescriptor {
            version: 1,
            id: super::INITIAL_MIGRATION_ID,
            sql: super::INITIAL_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 2,
            id: super::AUTHORITY_MIGRATION_ID,
            sql: super::AUTHORITY_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 3,
            id: super::OBSERVATION_MIGRATION_ID,
            sql: super::OBSERVATION_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 4,
            id: super::ACTIVATION_MIGRATION_ID,
            sql: super::ACTIVATION_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 5,
            id: super::SNAPSHOT_MIGRATION_ID,
            sql: super::SNAPSHOT_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 6,
            id: super::CANONICAL_EXPORT_MIGRATION_ID,
            sql: super::CANONICAL_EXPORT_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 7,
            id: super::MIGRATION_CHECKSUMS_MIGRATION_ID,
            sql: super::MIGRATION_CHECKSUMS_MIGRATION_SCHEMA,
        },
        MigrationDescriptor {
            version: 8,
            id: super::DEPLOYMENT_IDENTITIES_MIGRATION_ID,
            sql: super::DEPLOYMENT_IDENTITIES_MIGRATION_SCHEMA,
        },
    ]
}

/// Verifies a stored `SQLite` migration prefix against the typed inventory.
///
/// A shorter contiguous prefix is valid because startup may apply its next
/// forward migration.  Any gap, unsupported version, or identity drift is
/// rejected before a migration body is executed.
///
/// # Errors
///
/// Returns an error when the stored prefix is gapped, unsupported, or has an
/// identity different from the reviewed descriptor at that version.
pub fn verify_migration_prefix(
    records: &[(i64, String)],
) -> Result<(), MigrationVerificationError> {
    let history = migration_history();
    if records.len() > history.len() {
        return Err(MigrationVerificationError::Unsupported {
            version: records[history.len()].0,
        });
    }

    for (index, (version, migration_id)) in records.iter().enumerate() {
        let expected = history[index];
        if *version != i64::from(expected.version) {
            return Err(MigrationVerificationError::NonContiguous { version: *version });
        }
        if migration_id != expected.id {
            return Err(MigrationVerificationError::IdentityDrift {
                version: *version,
                expected: expected.id,
                found: migration_id.clone(),
            });
        }
    }
    Ok(())
}

/// Verifies a persisted migration prefix including its durable source checksums.
///
/// The checksum is read from the ledger supplied by the caller and compared to
/// the reviewed descriptor. It is never accepted merely because the current
/// binary can synthesize the expected value.
///
/// # Errors
///
/// Returns an explicit error when the stored prefix is gapped, unsupported,
/// has identity drift, or contains a checksum different from the reviewed
/// migration body.
pub fn verify_migration_records(
    records: &[(i64, String, String)],
) -> Result<(), MigrationVerificationError> {
    let identities = records
        .iter()
        .map(|(version, migration_id, _)| (*version, migration_id.clone()))
        .collect::<Vec<_>>();
    verify_migration_prefix(&identities)?;
    for (index, (version, _migration_id, checksum)) in records.iter().enumerate() {
        let expected = migration_history()[index].checksum().to_string();
        if checksum != &expected {
            return Err(MigrationVerificationError::ChecksumDrift {
                version: *version,
                expected,
                found: checksum.clone(),
            });
        }
    }
    Ok(())
}
