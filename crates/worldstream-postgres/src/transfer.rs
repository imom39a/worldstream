//! PostgreSQL destination publication for the provider-neutral transfer flow.
//!
//! This module stages exact logical transfer bytes and publishes complete
//! canonical Room records at the target authority transition. The caller must
//! provide the target fingerprint observed for the intended deployment, and
//! the destination persists that fingerprint before accepting any chunk.

use std::collections::{BTreeMap, BTreeSet};

use postgres::Transaction;
use thiserror::Error;
use worldstream_transfer::{
    BackendFingerprintV1, BundleProfileV1, CanonicalRecordKindV1, DeploymentIdentityV1, DigestV1,
    MigrationIdentityV1, NativeSqliteBundleSummaryV1, NativeSqliteRoomPolicyV1,
    NativeSqliteTransferAdapterV1, NativeSqliteTransferError, PackIdentityV1, RecordKindV1,
    ResourceIdentityV1, ResourceKindV1, TargetFingerprintV1, TransferBundleV1,
    TransferChunkDispositionV1, TransferChunkV1, TransferDestinationV1, TransferError,
    TransferScopeV1,
};

use crate::{
    LOGICAL_HISTORY_ID, PostgresAdmin, PostgresRoomStore, PostgresRoomVerificationError,
    PostgresSchemaVerificationError, migration_history, schema_contract_fingerprint,
};

/// The runtime PostgreSQL destination's exact reviewed backend identity.
pub fn postgres_backend_fingerprint() -> Result<BackendFingerprintV1, PostgresTransferError> {
    let migrations = migration_history()
        .into_iter()
        .map(|migration| {
            let version = u32::try_from(migration.version)
                .map_err(|_| PostgresTransferError::InvalidProviderValue("migration version"))?;
            let checksum = DigestV1::from_bytes(migration.checksum().as_bytes())
                .map_err(PostgresTransferError::Contract)?;
            MigrationIdentityV1::new(version, migration.id, checksum)
                .map_err(PostgresTransferError::Contract)
        })
        .collect::<Result<Vec<_>, PostgresTransferError>>()?;
    let schema = worldstream_transfer::SchemaMigrationContractV1::new(
        LOGICAL_HISTORY_ID,
        DigestV1::from_bytes(schema_contract_fingerprint().as_bytes())?,
        migrations,
    )?;
    Ok(BackendFingerprintV1::new(
        BundleProfileV1::PostgresPrimary17,
        "postgresql-17",
        schema,
    )?)
}

/// Errors returned by the PostgreSQL transfer destination.
#[derive(Debug, Error)]
pub enum PostgresTransferError {
    /// A transfer contract or exact-byte check failed before publication.
    #[error(transparent)]
    Contract(#[from] TransferError),
    /// Opening the runtime connection failed.
    #[error("PostgreSQL transfer connection failed: {0}")]
    Connection(#[source] postgres::Error),
    /// A destination SQL operation failed.
    #[error("PostgreSQL transfer SQL failed: {0}")]
    Sql(#[source] postgres::Error),
    /// Runtime schema verification failed before destination verification.
    #[error("PostgreSQL transfer schema verification failed: {0}")]
    Schema(#[source] PostgresSchemaVerificationError),
    /// The target fingerprint is not the reviewed PostgreSQL adapter contract.
    #[error("PostgreSQL transfer target mismatch: {0}")]
    TargetMismatch(&'static str),
    /// The same chunk key was already stored with different exact bytes.
    #[error("PostgreSQL transfer chunk identity conflicts with stored bytes")]
    ChunkConflict,
    /// A chunk overlaps an already stored range without being an exact replay.
    #[error("PostgreSQL transfer chunk overlaps a stored range")]
    OverlappingChunk,
    /// A durable import marker was required but was absent.
    #[error("PostgreSQL transfer import marker is absent")]
    MissingImport,
    /// A destination operation was attempted in the wrong durable state.
    #[error("PostgreSQL transfer import is in state {actual:?}; expected {expected}")]
    InvalidImportState {
        expected: &'static str,
        actual: String,
    },
    /// A source rollback was attempted after target finalization/authority.
    #[error("PostgreSQL transfer rollback is refused after target finalization")]
    RollbackRefused,
    /// The target-wide transfer fence was absent during a verification step.
    #[error("PostgreSQL transfer target fence is absent")]
    MissingTargetFence,
    /// Native operational-row validation failed before target publication.
    #[error("PostgreSQL transfer native operational evidence failed: {0}")]
    Native(#[source] NativeSqliteTransferError),
    /// Canonical Room bytes cannot be published without losing exact parity.
    #[error("PostgreSQL transfer canonical Room evidence failed: {0}")]
    Canonical(&'static str),
    /// The source deployment identity was absent, partial, or inconsistent
    /// with the whole-deployment bundle fence.
    #[error("PostgreSQL transfer deployment identity failed: {0}")]
    DeploymentIdentity(&'static str),
    /// The existing PostgreSQL provider verifier rejected hydrated target rows.
    #[error("PostgreSQL transfer hydrated target semantic verification failed: {0}")]
    Semantic(#[source] PostgresRoomVerificationError),
    /// A provider integer could not represent a bounded transfer value.
    #[error("PostgreSQL transfer value is outside the provider integer range: {0}")]
    InvalidProviderValue(&'static str),
    /// A first import found durable user truth on the purported empty target.
    #[error("PostgreSQL transfer target is not empty in durable domain {domain}")]
    TargetNotEmpty { domain: String },
    /// An explicitly aborted target must be discarded, not silently reused.
    #[error("PostgreSQL transfer target was aborted and must be discarded")]
    TargetAborted,
}

const TARGET_PREFLIGHT_ADVISORY_KEY: i64 = 6_291_328_795_568_100_166;
const AUTHORITY_STATE_COUNT_SQL: &str =
    "SELECT count(*)::bigint FROM worldstream_authority_state WHERE authority_id = true";

const TARGET_PREFLIGHT_LOCK_SQL: &str = r"
LOCK TABLE
    worldstream_operation_guards,
    worldstream_room_roots,
    worldstream_genesis,
    worldstream_materializations,
    worldstream_members,
    worldstream_timers,
    worldstream_transitions,
    worldstream_frames,
    worldstream_observation_consequences,
    worldstream_activation_decisions,
    worldstream_activation_intents,
    worldstream_activation_operation_receipts,
    worldstream_room_snapshots,
    worldstream_semantic_receipts,
    worldstream_integrity_incidents,
    worldstream_authority_fences,
    worldstream_authority_state,
    worldstream_authority_principals,
    worldstream_authority_runners,
    worldstream_authority_capabilities,
    worldstream_authority_capability_scopes,
    worldstream_authority_runner_capability_memberships,
    worldstream_authority_change_receipts,
    worldstream_authority_audit,
    worldstream_transfer_imports,
    worldstream_transfer_chunks,
    worldstream_transfer_target_fence,
    worldstream_deployment_metadata,
    worldstream_deployment_identity_metadata,
    worldstream_deployment_pack_identities,
    worldstream_deployment_resource_identities,
    worldstream_deployment_resource_blobs,
    worldstream_retired_authority_fences_v1
IN ACCESS EXCLUSIVE MODE;
";

const TARGET_EMPTY_DOMAIN_COUNTS_SQL: &str = r"
SELECT domain, row_count FROM (
    SELECT 'worldstream_operation_guards' AS domain, count(*)::bigint AS row_count FROM worldstream_operation_guards
    UNION ALL SELECT 'worldstream_room_roots', count(*)::bigint FROM worldstream_room_roots
    UNION ALL SELECT 'worldstream_genesis', count(*)::bigint FROM worldstream_genesis
    UNION ALL SELECT 'worldstream_materializations', count(*)::bigint FROM worldstream_materializations
    UNION ALL SELECT 'worldstream_members', count(*)::bigint FROM worldstream_members
    UNION ALL SELECT 'worldstream_timers', count(*)::bigint FROM worldstream_timers
    UNION ALL SELECT 'worldstream_transitions', count(*)::bigint FROM worldstream_transitions
    UNION ALL SELECT 'worldstream_frames', count(*)::bigint FROM worldstream_frames
    UNION ALL SELECT 'worldstream_observation_consequences', count(*)::bigint FROM worldstream_observation_consequences
    UNION ALL SELECT 'worldstream_activation_decisions', count(*)::bigint FROM worldstream_activation_decisions
    UNION ALL SELECT 'worldstream_activation_intents', count(*)::bigint FROM worldstream_activation_intents
    UNION ALL SELECT 'worldstream_activation_operation_receipts', count(*)::bigint FROM worldstream_activation_operation_receipts
    UNION ALL SELECT 'worldstream_room_snapshots', count(*)::bigint FROM worldstream_room_snapshots
    UNION ALL SELECT 'worldstream_semantic_receipts', count(*)::bigint FROM worldstream_semantic_receipts
    UNION ALL SELECT 'worldstream_integrity_incidents', count(*)::bigint FROM worldstream_integrity_incidents
    UNION ALL SELECT 'worldstream_authority_fences', count(*)::bigint FROM worldstream_authority_fences
    UNION ALL SELECT 'worldstream_authority_principals', count(*)::bigint FROM worldstream_authority_principals
    UNION ALL SELECT 'worldstream_authority_runners', count(*)::bigint FROM worldstream_authority_runners
    UNION ALL SELECT 'worldstream_authority_capabilities', count(*)::bigint FROM worldstream_authority_capabilities
    UNION ALL SELECT 'worldstream_authority_capability_scopes', count(*)::bigint FROM worldstream_authority_capability_scopes
    UNION ALL SELECT 'worldstream_authority_runner_capability_memberships', count(*)::bigint FROM worldstream_authority_runner_capability_memberships
    UNION ALL SELECT 'worldstream_authority_change_receipts', count(*)::bigint FROM worldstream_authority_change_receipts
    UNION ALL SELECT 'worldstream_authority_audit', count(*)::bigint FROM worldstream_authority_audit
    UNION ALL SELECT 'worldstream_transfer_imports', count(*)::bigint FROM worldstream_transfer_imports
    UNION ALL SELECT 'worldstream_transfer_chunks', count(*)::bigint FROM worldstream_transfer_chunks
    UNION ALL SELECT 'worldstream_transfer_target_fence', count(*)::bigint FROM worldstream_transfer_target_fence
    UNION ALL SELECT 'worldstream_deployment_metadata', count(*)::bigint FROM worldstream_deployment_metadata
    UNION ALL SELECT 'worldstream_deployment_identity_metadata', count(*)::bigint FROM worldstream_deployment_identity_metadata
    UNION ALL SELECT 'worldstream_deployment_pack_identities', count(*)::bigint FROM worldstream_deployment_pack_identities
    UNION ALL SELECT 'worldstream_deployment_resource_identities', count(*)::bigint FROM worldstream_deployment_resource_identities
    UNION ALL SELECT 'worldstream_deployment_resource_blobs', count(*)::bigint FROM worldstream_deployment_resource_blobs
    UNION ALL SELECT 'worldstream_retired_authority_fences_v1', count(*)::bigint FROM worldstream_retired_authority_fences_v1
) counts ORDER BY domain;
";

// The importing fence remains committed and therefore visible to every other
// transaction while this transaction removes it from its own MVCC view. That
// lets the offline administrator erase a verified-but-not-finalized target
// without opening a serving/write window. TRUNCATE is intentional here: three
// transferred fact tables reject row DELETEs because their contents are
// immutable during normal operation, while an aborted deployment must discard
// the entire isolated target atomically.
const DISCARD_TRANSFER_TARGET_SQL: &str = r"
TRUNCATE TABLE
    worldstream_operation_guards,
    worldstream_room_roots,
    worldstream_genesis,
    worldstream_materializations,
    worldstream_members,
    worldstream_timers,
    worldstream_transitions,
    worldstream_frames,
    worldstream_observation_consequences,
    worldstream_activation_decisions,
    worldstream_activation_intents,
    worldstream_activation_operation_receipts,
    worldstream_room_snapshots,
    worldstream_semantic_receipts,
    worldstream_integrity_incidents,
    worldstream_authority_fences,
    worldstream_authority_state,
    worldstream_authority_principals,
    worldstream_authority_runners,
    worldstream_authority_capabilities,
    worldstream_authority_capability_scopes,
    worldstream_authority_runner_capability_memberships,
    worldstream_authority_change_receipts,
    worldstream_authority_audit,
    worldstream_transfer_chunks,
    worldstream_transfer_imports,
    worldstream_deployment_metadata,
    worldstream_deployment_identity_metadata,
    worldstream_deployment_pack_identities,
    worldstream_deployment_resource_blobs,
    worldstream_deployment_resource_identities,
    worldstream_retired_authority_fences_v1
RESTART IDENTITY;
INSERT INTO worldstream_authority_state(authority_id) VALUES (true);
";

/// An offline direct-admin implementation of `TransferDestinationV1`.
///
/// The staging rows are written with ordinary transactions and are safe for a
/// transaction pooler. The adapter never issues DDL; migrations must have been
/// applied before this explicitly offline transfer authority is constructed.
pub struct PostgresTransferDestination<'a> {
    admin: &'a PostgresAdmin,
    bundle: TransferBundleV1,
    bundle_hash: DigestV1,
    target: TargetFingerprintV1,
    target_digest: DigestV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AbortFenceStateV1 {
    Missing,
    Importing,
    AlreadyAborted,
}

impl<'a> PostgresTransferDestination<'a> {
    /// Binds a destination to one exact bundle and observed target fence.
    ///
    /// This constructor is deliberately side-effect free. Callers requiring
    /// live-provider evidence must call [`PostgresAdmin::verify_schema`] before
    /// starting the import; `verify_complete` repeats that read-only check
    /// before accepting finalization.
    pub fn new(
        admin: &'a PostgresAdmin,
        bundle: &TransferBundleV1,
        target: TargetFingerprintV1,
    ) -> Result<Self, PostgresTransferError> {
        bundle.verify_target(&target)?;
        let expected_backend = postgres_backend_fingerprint()?;
        if target.backend() != &expected_backend {
            return Err(PostgresTransferError::TargetMismatch(
                "backend/schema/migration fingerprint",
            ));
        }
        if NativeSqliteTransferAdapterV1::is_native_bundle(bundle) {
            NativeSqliteTransferAdapterV1::validate_bundle(bundle)
                .map_err(PostgresTransferError::Native)?;
        }
        if bundle.scope() == TransferScopeV1::WholeDeployment {
            let identity = extract_deployment_identity(bundle.records())?;
            verify_bundle_deployment_identity(bundle, &identity)?;
        }
        Ok(Self {
            admin,
            bundle: bundle.clone(),
            bundle_hash: bundle.bundle_hash()?,
            target_digest: target.fingerprint_digest()?,
            target,
        })
    }

    /// Returns the target fence bound to this destination.
    #[must_use]
    pub const fn target(&self) -> &TargetFingerprintV1 {
        &self.target
    }

    fn bundle_key(&self) -> [u8; 32] {
        self.bundle_hash.as_bytes()
    }

    fn target_key(&self) -> [u8; 32] {
        self.target_digest.as_bytes()
    }

    fn verify_target_fence_bytes(
        expected_bundle: DigestV1,
        expected_target: DigestV1,
        stored: Option<(&[u8], &[u8])>,
    ) -> Result<(), PostgresTransferError> {
        let Some((stored_bundle, stored_target)) = stored else {
            return Err(PostgresTransferError::MissingTargetFence);
        };
        if stored_bundle != expected_bundle.as_bytes() {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target bundle fence",
            ));
        }
        if stored_target != expected_target.as_bytes() {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        Ok(())
    }

    fn verify_target_fence(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        let Some(row) = transaction
            .query_opt(
                "SELECT bundle_hash, target_fingerprint, state FROM worldstream_transfer_target_fence WHERE fence_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
        else {
            return Err(PostgresTransferError::MissingTargetFence);
        };
        let stored_bundle: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        let stored_target: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        let state: String = row.try_get(2).map_err(PostgresTransferError::Sql)?;
        if state == "aborted" {
            return Err(PostgresTransferError::TargetAborted);
        }
        if state != "importing" {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fence state",
            ));
        }
        Self::verify_target_fence_bytes(
            self.bundle_hash,
            self.target_digest,
            Some((&stored_bundle, &stored_target)),
        )
    }

    fn remove_target_fence_for_offline_verification(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        self.verify_target_fence(transaction)?;
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let removed = transaction
            .execute(
                "DELETE FROM worldstream_transfer_target_fence WHERE fence_id = true AND bundle_hash = $1 AND target_fingerprint = $2 AND state = 'importing'",
                &[&bundle_hash.as_slice(), &target_digest.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?;
        if removed != 1 {
            return Err(PostgresTransferError::MissingTargetFence);
        }
        Ok(())
    }

    fn restore_target_fence_after_offline_verification(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let restored = transaction
            .execute(
                "INSERT INTO worldstream_transfer_target_fence(fence_id, bundle_hash, target_fingerprint, state) VALUES (true, $1, $2, 'importing')",
                &[&bundle_hash.as_slice(), &target_digest.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?;
        if restored != 1 {
            return Err(PostgresTransferError::MissingTargetFence);
        }
        self.verify_target_fence(transaction)
    }

    /// Hydrates and semantically verifies the target without ever making the
    /// target serving-visible. PostgreSQL MVCC keeps the committed fence
    /// visible to every other transaction while this transaction temporarily
    /// removes it from its own view so the fence triggers permit hydration.
    /// The same exact fence is restored before the verified marker commits;
    /// any failure rolls both hydration and fence removal back together.
    fn hydrate_and_verify_behind_fence(
        &self,
        transaction: &mut Transaction<'_>,
        summary: Option<&NativeSqliteBundleSummaryV1>,
    ) -> Result<(), PostgresTransferError> {
        self.remove_target_fence_for_offline_verification(transaction)?;
        self.publish_canonical_rooms(transaction)?;
        self.verify_hydrated_target(transaction, summary)?;
        self.restore_target_fence_after_offline_verification(transaction)
    }

    /// Reconfirms an existing provider-derived marker against exact native
    /// rows and executable replay. This rejects `verified`/`finalized` rows
    /// written by older code that only proved staged chunk completeness.
    fn reconfirm_hydrated_target_behind_fence(
        &self,
        transaction: &mut Transaction<'_>,
        summary: Option<&NativeSqliteBundleSummaryV1>,
    ) -> Result<(), PostgresTransferError> {
        self.remove_target_fence_for_offline_verification(transaction)?;
        self.verify_native_operational_rows(transaction)?;
        self.verify_hydrated_target(transaction, summary)?;
        self.restore_target_fence_after_offline_verification(transaction)
    }

    fn chunk_state_accepts_chunk(state: &str) -> bool {
        state == "pending"
    }

    fn checked_i64(value: usize, what: &'static str) -> Result<i64, PostgresTransferError> {
        i64::try_from(value).map_err(|_| PostgresTransferError::InvalidProviderValue(what))
    }

    fn validate_empty_domain_counts(
        counts: &[(String, i64)],
        authority_state_count: i64,
        expected_target_fence_count: i64,
    ) -> Result<(), PostgresTransferError> {
        if authority_state_count != 1 {
            return Err(PostgresTransferError::TargetNotEmpty {
                domain: "worldstream_authority_state".to_owned(),
            });
        }
        if let Some((domain, _)) = counts.iter().find(|(domain, count)| {
            let expected = if domain == "worldstream_transfer_target_fence" {
                expected_target_fence_count
            } else {
                0
            };
            *count != expected
        }) {
            return Err(PostgresTransferError::TargetNotEmpty {
                domain: domain.clone(),
            });
        }
        Ok(())
    }

    fn preflight_empty_target(
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        // The table locks make the empty check and first target-fence write one
        // provider transaction. A concurrent runtime write either precedes
        // the counts and is rejected as preseed, or waits until the fence is
        // visible and is rejected by the migration-installed write triggers.
        transaction
            .batch_execute(TARGET_PREFLIGHT_LOCK_SQL)
            .map_err(PostgresTransferError::Sql)?;
        Self::verify_empty_target_domains(transaction, 0)
    }

    fn verify_empty_target_domains(
        transaction: &mut Transaction<'_>,
        expected_target_fence_count: i64,
    ) -> Result<(), PostgresTransferError> {
        let authority_state_count: i64 = transaction
            .query_one(AUTHORITY_STATE_COUNT_SQL, &[])
            .map_err(PostgresTransferError::Sql)?
            .try_get(0)
            .map_err(PostgresTransferError::Sql)?;
        let counts = transaction
            .query(TARGET_EMPTY_DOMAIN_COUNTS_SQL, &[])
            .map_err(PostgresTransferError::Sql)?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get::<_, String>(0)
                        .map_err(PostgresTransferError::Sql)?,
                    row.try_get::<_, i64>(1)
                        .map_err(PostgresTransferError::Sql)?,
                ))
            })
            .collect::<Result<Vec<_>, PostgresTransferError>>()?;
        Self::validate_empty_domain_counts(
            &counts,
            authority_state_count,
            expected_target_fence_count,
        )
    }

    fn lock_target_preflight(
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        transaction
            .query_one(
                "SELECT pg_advisory_xact_lock($1)",
                &[&TARGET_PREFLIGHT_ADVISORY_KEY],
            )
            .map_err(PostgresTransferError::Sql)?;
        Ok(())
    }

    fn ensure_import(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<String, PostgresTransferError> {
        let bundle_hash = self.bundle_key();
        let target_fingerprint = self.target_key();
        Self::lock_target_preflight(transaction)?;
        let mut fence = transaction
            .query_opt(
                "SELECT bundle_hash, target_fingerprint, state FROM worldstream_transfer_target_fence WHERE fence_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?;
        if fence.is_none() {
            Self::preflight_empty_target(transaction)?;
            transaction
                .execute(
                    "INSERT INTO worldstream_transfer_target_fence(fence_id, bundle_hash, target_fingerprint, state) VALUES (true, $1, $2, 'importing')",
                    &[&bundle_hash.as_slice(), &target_fingerprint.as_slice()],
                )
                .map_err(PostgresTransferError::Sql)?;
            fence = transaction
                .query_opt(
                    "SELECT bundle_hash, target_fingerprint, state FROM worldstream_transfer_target_fence WHERE fence_id = true FOR UPDATE",
                    &[],
                )
                .map_err(PostgresTransferError::Sql)?;
        }
        let fence = fence.ok_or(PostgresTransferError::MissingTargetFence)?;
        let stored_bundle_hash: Vec<u8> = fence.try_get(0).map_err(PostgresTransferError::Sql)?;
        let stored_target_fingerprint: Vec<u8> =
            fence.try_get(1).map_err(PostgresTransferError::Sql)?;
        let fence_state: String = fence.try_get(2).map_err(PostgresTransferError::Sql)?;
        if fence_state == "aborted" {
            return Err(PostgresTransferError::TargetAborted);
        }
        if fence_state != "importing" {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fence state",
            ));
        }
        if stored_bundle_hash != bundle_hash {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target bundle fence",
            ));
        }
        Self::verify_target_fence_bytes(
            self.bundle_hash,
            self.target_digest,
            Some((&stored_bundle_hash, &stored_target_fingerprint)),
        )?;
        transaction
            .execute(
                "INSERT INTO worldstream_transfer_imports(bundle_hash, target_fingerprint, state) VALUES ($1, $2, 'pending') ON CONFLICT (bundle_hash) DO NOTHING",
                &[&bundle_hash.as_slice(), &target_fingerprint.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?;
        let row = transaction
            .query_one(
                "SELECT target_fingerprint, state FROM worldstream_transfer_imports WHERE bundle_hash = $1 FOR UPDATE",
                &[&bundle_hash.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?;
        let stored_fingerprint: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        if stored_fingerprint != target_fingerprint {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        let state: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        Ok(state)
    }

    fn verify_staged_chunks(
        &self,
        transaction: &mut Transaction<'_>,
        bundle: &TransferBundleV1,
    ) -> Result<(), PostgresTransferError> {
        if bundle.bundle_hash()? != self.bundle_hash {
            return Err(PostgresTransferError::TargetMismatch("bundle hash"));
        }
        Self::verify_native_semantic_evidence(bundle)?;
        let rows = transaction
            .query(
                "SELECT chunk_start, chunk_end, chunk_digest, records_bytes FROM worldstream_transfer_chunks WHERE bundle_hash = $1 ORDER BY chunk_start",
                &[&self.bundle_key().as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?;
        let mut next = 0_usize;
        for row in rows {
            let start = usize::try_from(
                row.try_get::<_, i64>(0)
                    .map_err(PostgresTransferError::Sql)?,
            )
            .map_err(|_| PostgresTransferError::InvalidProviderValue("chunk start"))?;
            let end = usize::try_from(
                row.try_get::<_, i64>(1)
                    .map_err(PostgresTransferError::Sql)?,
            )
            .map_err(|_| PostgresTransferError::InvalidProviderValue("chunk end"))?;
            if start != next || start >= end || end > bundle.records().len() {
                return Err(PostgresTransferError::TargetMismatch("chunk coverage"));
            }
            let expected = bundle.chunk(start, end)?;
            let stored_digest: Vec<u8> = row.try_get(2).map_err(PostgresTransferError::Sql)?;
            let stored_bytes: Vec<u8> = row.try_get(3).map_err(PostgresTransferError::Sql)?;
            if stored_digest != expected.digest().as_bytes()
                || stored_bytes != expected.canonical_bytes()?
            {
                return Err(PostgresTransferError::ChunkConflict);
            }
            next = end;
        }
        if next != bundle.records().len() {
            return Err(PostgresTransferError::TargetMismatch(
                "incomplete chunk coverage",
            ));
        }
        Ok(())
    }

    /// Operational rows are valid staging evidence, but they are not a
    /// complete source-of-truth transfer for healthy Rooms. Keep that
    /// distinction at the publication boundary: healthy native Rooms cannot
    /// be verified, finalized, or made authoritative without the independent
    /// canonical evidence accepted by the SQLite adapter contract; isolated
    /// Rooms retain only their explicitly isolated operational disposition.
    fn verify_native_semantic_evidence(
        bundle: &TransferBundleV1,
    ) -> Result<Option<NativeSqliteBundleSummaryV1>, PostgresTransferError> {
        if !NativeSqliteTransferAdapterV1::is_native_bundle(bundle) {
            return Ok(None);
        }
        let summary = NativeSqliteTransferAdapterV1::validate_bundle(bundle)
            .map_err(PostgresTransferError::Native)?;
        let room_ids = summary
            .room_policies()
            .iter()
            .filter_map(|(room_id, policy)| {
                (*policy == NativeSqliteRoomPolicyV1::Healthy).then_some(room_id.as_str())
            })
            .collect::<Vec<_>>();
        require_native_canonical_evidence(bundle.records(), &room_ids)
            .map_err(PostgresTransferError::Native)
            .map(|()| Some(summary))
    }

    fn persist_identity_metadata(
        transaction: &mut Transaction<'_>,
        identity: &DeploymentIdentityV1,
        canonical_bytes: &[u8],
    ) -> Result<(), PostgresTransferError> {
        let identity_digest = identity.digest().as_bytes();
        let pack_set_digest = deployment_pack_set_digest(identity).as_bytes();
        let resource_set_digest = deployment_resource_set_digest(identity).as_bytes();
        let stored = transaction
            .query_opt(
                "SELECT identity_digest, pack_set_digest, resource_set_digest, canonical_bytes FROM worldstream_deployment_identity_metadata WHERE target_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?;
        if let Some(row) = stored {
            let matches = row
                .try_get::<_, Vec<u8>>(0)
                .map_err(PostgresTransferError::Sql)?
                == identity_digest
                && row
                    .try_get::<_, Vec<u8>>(1)
                    .map_err(PostgresTransferError::Sql)?
                    == pack_set_digest
                && row
                    .try_get::<_, Vec<u8>>(2)
                    .map_err(PostgresTransferError::Sql)?
                    == resource_set_digest
                && row
                    .try_get::<_, Vec<u8>>(3)
                    .map_err(PostgresTransferError::Sql)?
                    == canonical_bytes;
            if !matches {
                return Err(PostgresTransferError::DeploymentIdentity(
                    "persisted identity metadata mismatch",
                ));
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO worldstream_deployment_identity_metadata(target_id, identity_digest, pack_set_digest, resource_set_digest, canonical_bytes) VALUES (true, $1, $2, $3, $4)",
                    &[
                        &identity_digest.as_slice(),
                        &pack_set_digest.as_slice(),
                        &resource_set_digest.as_slice(),
                        &canonical_bytes,
                    ],
                )
                .map_err(PostgresTransferError::Sql)?;
        }
        Ok(())
    }

    fn persist_identity_rows(
        transaction: &mut Transaction<'_>,
        identity: &DeploymentIdentityV1,
    ) -> Result<(), PostgresTransferError> {
        for pack in identity.packs() {
            transaction
                .execute(
                    "INSERT INTO worldstream_deployment_pack_identities(pack_id, revision, pack_digest) VALUES ($1, $2, $3) ON CONFLICT (pack_id, revision) DO NOTHING",
                    &[&pack.pack_id(), &pack.revision(), &pack.digest().as_bytes().as_slice()],
                )
                .map_err(PostgresTransferError::Sql)?;
        }
        for resource in identity.resources() {
            let size = i64::try_from(resource.size_bytes())
                .map_err(|_| PostgresTransferError::InvalidProviderValue("resource size"))?;
            transaction
                .execute(
                    "INSERT INTO worldstream_deployment_resource_identities(resource_kind, resource_identity, size_bytes, resource_digest) VALUES ($1, $2, $3, $4) ON CONFLICT (resource_kind, resource_identity) DO NOTHING",
                    &[
                        &resource_kind_name(resource.kind()),
                        &resource.identity(),
                        &size,
                        &resource.digest().as_bytes().as_slice(),
                    ],
                )
                .map_err(PostgresTransferError::Sql)?;
        }
        Ok(())
    }

    fn load_persisted_packs(
        transaction: &mut Transaction<'_>,
    ) -> Result<Vec<PackIdentityV1>, PostgresTransferError> {
        transaction
            .query(
                "SELECT pack_id, revision, pack_digest FROM worldstream_deployment_pack_identities ORDER BY pack_id, revision",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
            .into_iter()
            .map(|row| {
                let digest: Vec<u8> = row.try_get(2).map_err(PostgresTransferError::Sql)?;
                PackIdentityV1::new(
                    row.try_get::<_, String>(0).map_err(PostgresTransferError::Sql)?,
                    row.try_get::<_, String>(1).map_err(PostgresTransferError::Sql)?,
                    DigestV1::from_bytes(&digest)?,
                )
                .map_err(PostgresTransferError::Contract)
            })
            .collect()
    }

    fn load_persisted_resources(
        transaction: &mut Transaction<'_>,
    ) -> Result<Vec<ResourceIdentityV1>, PostgresTransferError> {
        transaction
            .query(
                "SELECT resource_kind, resource_identity, size_bytes, resource_digest FROM worldstream_deployment_resource_identities ORDER BY resource_kind, resource_identity",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
            .into_iter()
            .map(|row| {
                let kind = match row
                    .try_get::<_, String>(0)
                    .map_err(PostgresTransferError::Sql)?
                    .as_str()
                {
                    "artifact" => ResourceKindV1::Artifact,
                    "codec" => ResourceKindV1::Codec,
                    "schema" => ResourceKindV1::Schema,
                    _ => {
                        return Err(PostgresTransferError::DeploymentIdentity(
                            "unknown persisted resource kind",
                        ));
                    }
                };
                let size = u64::try_from(
                    row.try_get::<_, i64>(2).map_err(PostgresTransferError::Sql)?,
                )
                .map_err(|_| PostgresTransferError::InvalidProviderValue("resource size"))?;
                let digest: Vec<u8> = row.try_get(3).map_err(PostgresTransferError::Sql)?;
                ResourceIdentityV1::from_persisted_parts(
                    kind,
                    row.try_get::<_, String>(1).map_err(PostgresTransferError::Sql)?,
                    size,
                    DigestV1::from_bytes(&digest)?,
                )
                .map_err(PostgresTransferError::Contract)
            })
            .collect()
    }

    fn verify_persisted_identity(
        transaction: &mut Transaction<'_>,
        identity: &DeploymentIdentityV1,
    ) -> Result<(), PostgresTransferError> {
        let persisted = DeploymentIdentityV1::new(
            Self::load_persisted_packs(transaction)?,
            Self::load_persisted_resources(transaction)?,
        )
        .map_err(PostgresTransferError::Contract)?;
        if persisted != *identity {
            return Err(PostgresTransferError::DeploymentIdentity(
                "persisted identity rows mismatch",
            ));
        }
        Ok(())
    }

    /// Persists and then reconstructs the complete source identity witness.
    /// This runs inside the same transaction as canonical Room publication;
    /// a mismatch therefore rolls back the entire target hydration.
    fn persist_deployment_identity(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<Option<DeploymentIdentityV1>, PostgresTransferError> {
        if self.bundle.scope() != TransferScopeV1::WholeDeployment {
            return Ok(None);
        }
        let identity = extract_deployment_identity(self.bundle.records())?;
        verify_bundle_deployment_identity(&self.bundle, &identity)?;
        let canonical_bytes = identity
            .canonical_bytes()
            .map_err(|_| PostgresTransferError::DeploymentIdentity("identity encoding"))?;
        Self::persist_identity_metadata(transaction, &identity, &canonical_bytes)?;
        Self::persist_identity_rows(transaction, &identity)?;
        self.persist_resource_payloads(transaction, &identity)?;
        Self::verify_persisted_identity(transaction, &identity)?;
        Ok(Some(identity))
    }

    fn persist_resource_payloads(
        &self,
        transaction: &mut Transaction<'_>,
        identity: &DeploymentIdentityV1,
    ) -> Result<(), PostgresTransferError> {
        let mut seen = BTreeSet::new();
        for resource in identity.resources() {
            let record_identity = resource_record_identity(resource);
            let record = self
                .bundle
                .records()
                .iter()
                .find(|record| {
                    record.kind() == RecordKindV1::Canonical(CanonicalRecordKindV1::ArtifactBytes)
                        && record.identity() == record_identity
                })
                .ok_or(PostgresTransferError::DeploymentIdentity(
                    "resource payload is absent",
                ))?;
            if !seen.insert(record_identity) {
                return Err(PostgresTransferError::DeploymentIdentity(
                    "duplicate resource payload",
                ));
            }
            resource.verify_bytes(record.bytes()).map_err(|_| {
                PostgresTransferError::DeploymentIdentity("resource payload identity mismatch")
            })?;
            let kind = resource_kind_name(resource.kind());
            let digest = resource.digest().as_bytes();
            if let Some(row) = transaction
                .query_opt(
                    "SELECT resource_bytes, resource_digest FROM worldstream_deployment_resource_blobs WHERE resource_kind = $1 AND resource_identity = $2 FOR UPDATE",
                    &[&kind, &resource.identity()],
                )
                .map_err(PostgresTransferError::Sql)?
            {
                let bytes: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
                let stored_digest: Vec<u8> =
                    row.try_get(1).map_err(PostgresTransferError::Sql)?;
                if bytes != record.bytes() || stored_digest != digest {
                    return Err(PostgresTransferError::DeploymentIdentity(
                        "persisted resource payload mismatch",
                    ));
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO worldstream_deployment_resource_blobs(resource_kind, resource_identity, resource_bytes, resource_digest) VALUES ($1, $2, $3, $4)",
                        &[&kind, &resource.identity(), &record.bytes(), &digest.as_slice()],
                    )
                    .map_err(PostgresTransferError::Sql)?;
            }
        }
        let stored_count = usize::try_from(
            transaction
                .query_one(
                    "SELECT count(*) FROM worldstream_deployment_resource_blobs",
                    &[],
                )
                .map_err(PostgresTransferError::Sql)?
                .try_get::<_, i64>(0)
                .map_err(PostgresTransferError::Sql)?,
        )
        .map_err(|_| PostgresTransferError::InvalidProviderValue("resource payload count"))?;
        if stored_count != identity.resources().len() {
            return Err(PostgresTransferError::DeploymentIdentity(
                "persisted resource payload cardinality",
            ));
        }
        Ok(())
    }

    /// Reconstructs the complete deployment identity without repairing any
    /// missing row. Persisted verification markers are only provider evidence
    /// when every normalized identity row and resource byte already exists
    /// exactly as it did in the hydration transaction.
    fn verify_deployment_identity(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        if self.bundle.scope() != TransferScopeV1::WholeDeployment {
            return Ok(());
        }
        let identity = extract_deployment_identity(self.bundle.records())?;
        verify_bundle_deployment_identity(&self.bundle, &identity)?;
        let canonical_bytes = identity
            .canonical_bytes()
            .map_err(|_| PostgresTransferError::DeploymentIdentity("identity encoding"))?;
        let metadata = transaction
            .query_opt(
                "SELECT identity_digest, pack_set_digest, resource_set_digest, canonical_bytes FROM worldstream_deployment_identity_metadata WHERE target_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
            .ok_or(PostgresTransferError::DeploymentIdentity(
                "persisted identity metadata is absent",
            ))?;
        if metadata
            .try_get::<_, Vec<u8>>(0)
            .map_err(PostgresTransferError::Sql)?
            != identity.digest().as_bytes()
            || metadata
                .try_get::<_, Vec<u8>>(1)
                .map_err(PostgresTransferError::Sql)?
                != deployment_pack_set_digest(&identity).as_bytes()
            || metadata
                .try_get::<_, Vec<u8>>(2)
                .map_err(PostgresTransferError::Sql)?
                != deployment_resource_set_digest(&identity).as_bytes()
            || metadata
                .try_get::<_, Vec<u8>>(3)
                .map_err(PostgresTransferError::Sql)?
                != canonical_bytes
        {
            return Err(PostgresTransferError::DeploymentIdentity(
                "persisted identity metadata mismatch",
            ));
        }
        Self::verify_persisted_identity(transaction, &identity)?;
        self.verify_resource_payloads(transaction, &identity)
    }

    fn verify_resource_payloads(
        &self,
        transaction: &mut Transaction<'_>,
        identity: &DeploymentIdentityV1,
    ) -> Result<(), PostgresTransferError> {
        for resource in identity.resources() {
            let record_identity = resource_record_identity(resource);
            let record = self
                .bundle
                .records()
                .iter()
                .find(|record| {
                    record.kind() == RecordKindV1::Canonical(CanonicalRecordKindV1::ArtifactBytes)
                        && record.identity() == record_identity
                })
                .ok_or(PostgresTransferError::DeploymentIdentity(
                    "resource payload is absent",
                ))?;
            resource.verify_bytes(record.bytes()).map_err(|_| {
                PostgresTransferError::DeploymentIdentity("resource payload identity mismatch")
            })?;
            let stored = transaction
                .query_opt(
                    "SELECT resource_bytes, resource_digest FROM worldstream_deployment_resource_blobs WHERE resource_kind = $1 AND resource_identity = $2",
                    &[&resource_kind_name(resource.kind()), &resource.identity()],
                )
                .map_err(PostgresTransferError::Sql)?
                .ok_or(PostgresTransferError::DeploymentIdentity(
                    "persisted resource payload is absent",
                ))?;
            if stored
                .try_get::<_, Vec<u8>>(0)
                .map_err(PostgresTransferError::Sql)?
                != record.bytes()
                || stored
                    .try_get::<_, Vec<u8>>(1)
                    .map_err(PostgresTransferError::Sql)?
                    != resource.digest().as_bytes()
            {
                return Err(PostgresTransferError::DeploymentIdentity(
                    "persisted resource payload mismatch",
                ));
            }
        }
        let stored_count = usize::try_from(
            transaction
                .query_one(
                    "SELECT count(*) FROM worldstream_deployment_resource_blobs",
                    &[],
                )
                .map_err(PostgresTransferError::Sql)?
                .try_get::<_, i64>(0)
                .map_err(PostgresTransferError::Sql)?,
        )
        .map_err(|_| PostgresTransferError::InvalidProviderValue("resource payload count"))?;
        if stored_count != identity.resources().len() {
            return Err(PostgresTransferError::DeploymentIdentity(
                "persisted resource payload cardinality",
            ));
        }
        Ok(())
    }

    fn commit_transaction(transaction: Transaction<'_>) -> Result<(), PostgresTransferError> {
        transaction.commit().map_err(PostgresTransferError::Sql)
    }

    fn lock_abort_fence(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<AbortFenceStateV1, PostgresTransferError> {
        let Some(row) = transaction
            .query_opt(
                "SELECT bundle_hash, target_fingerprint, state FROM worldstream_transfer_target_fence WHERE fence_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
        else {
            return Ok(AbortFenceStateV1::Missing);
        };
        let stored_bundle: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        let stored_target: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        Self::verify_target_fence_bytes(
            self.bundle_hash,
            self.target_digest,
            Some((&stored_bundle, &stored_target)),
        )?;
        match row
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?
            .as_str()
        {
            "importing" => Ok(AbortFenceStateV1::Importing),
            "aborted" => Ok(AbortFenceStateV1::AlreadyAborted),
            _ => Err(PostgresTransferError::TargetMismatch(
                "persisted target fence state",
            )),
        }
    }

    fn verify_aborted_import_empty(
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        // The exact aborted fence was locked and verified by the caller. Every
        // other durable target domain must be empty before that fence can act
        // as source-restoration evidence; checking only the staging tables
        // would leave a previously hydrated deployment behind.
        Self::verify_empty_target_domains(transaction, 1)
    }

    fn discard_import_behind_fence(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        self.remove_target_fence_for_offline_verification(transaction)?;
        transaction
            .batch_execute(DISCARD_TRANSFER_TARGET_SQL)
            .map_err(PostgresTransferError::Sql)?;
        // Reuse the first-import emptiness proof while the importing fence is
        // absent only from this transaction's view. Concurrent transactions
        // continue to observe the committed importing fence until commit.
        Self::preflight_empty_target(transaction)?;
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let inserted = transaction
            .execute(
                "INSERT INTO worldstream_transfer_target_fence(fence_id, bundle_hash, target_fingerprint, state) VALUES (true, $1, $2, 'aborted')",
                &[&bundle_hash.as_slice(), &target_digest.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?;
        if inserted != 1 || self.lock_abort_fence(transaction)? != AbortFenceStateV1::AlreadyAborted
        {
            return Err(PostgresTransferError::MissingTargetFence);
        }
        Self::verify_aborted_import_empty(transaction)
    }

    fn hydrated_room_dispositions(
        &self,
        summary: Option<&NativeSqliteBundleSummaryV1>,
    ) -> Result<
        (
            BTreeSet<String>,
            BTreeMap<String, (NativeSqliteRoomPolicyV1, String, i64)>,
        ),
        PostgresTransferError,
    > {
        let mut healthy = BTreeSet::new();
        let mut isolated = BTreeMap::new();
        if let Some(summary) = summary {
            for (room_id, policy) in summary.room_policies() {
                if *policy == NativeSqliteRoomPolicyV1::Healthy {
                    healthy.insert(room_id.clone());
                }
            }
            for record in self.bundle.records() {
                if record.kind()
                    != RecordKindV1::Canonical(CanonicalRecordKindV1::NativeOperationalRow)
                {
                    continue;
                }
                let row = decode_native_operational_row(record.bytes())?;
                if row.table != "room_integrity" {
                    continue;
                }
                let room_id = native_text(&row, 0)?;
                let status = native_text(&row, 1)?;
                let generation = native_integer(&row, 2)?;
                if summary.room_policies().get(&room_id)
                    == Some(&NativeSqliteRoomPolicyV1::IsolatedCorrupt)
                {
                    isolated.insert(
                        room_id,
                        (
                            NativeSqliteRoomPolicyV1::IsolatedCorrupt,
                            status,
                            generation,
                        ),
                    );
                }
            }
            if isolated.len()
                != summary
                    .room_policies()
                    .values()
                    .filter(|policy| **policy == NativeSqliteRoomPolicyV1::IsolatedCorrupt)
                    .count()
            {
                return Err(PostgresTransferError::Canonical(
                    "isolated integrity evidence is incomplete",
                ));
            }
        } else {
            for record in self.bundle.records() {
                if !matches!(
                    record.kind(),
                    RecordKindV1::Canonical(
                        CanonicalRecordKindV1::RoomGenesis
                            | CanonicalRecordKindV1::RoomHead
                            | CanonicalRecordKindV1::CoreMaterialization
                            | CanonicalRecordKindV1::ActivityMaterialization
                            | CanonicalRecordKindV1::ArtifactMetadata
                            | CanonicalRecordKindV1::RoomTransition
                    )
                ) {
                    continue;
                }
                if let Some(room_id) = canonical_room_id(record.identity()) {
                    healthy.insert(room_id);
                }
            }
        }
        Ok((healthy, isolated))
    }

    fn verify_hydrated_target(
        &self,
        transaction: &mut Transaction<'_>,
        summary: Option<&NativeSqliteBundleSummaryV1>,
    ) -> Result<(), PostgresTransferError> {
        self.verify_deployment_identity(transaction)?;
        let metadata = extract_deployment_metadata(self.bundle.records())?;
        let target_epoch = i64::try_from(self.target.storage_epoch())
            .map_err(|_| PostgresTransferError::InvalidProviderValue("target epoch"))?;
        let row = transaction
            .query_opt(
                "SELECT deployment_lineage_bytes, storage_epoch_bytes, storage_epoch FROM worldstream_deployment_metadata WHERE target_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
            .ok_or(PostgresTransferError::Canonical(
                "deployment metadata is absent",
            ))?;
        let actual_lineage: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        let actual_epoch_bytes: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        let actual_epoch: i64 = row.try_get(2).map_err(PostgresTransferError::Sql)?;
        if actual_lineage != metadata.deployment_lineage
            || actual_epoch_bytes != metadata.storage_epoch_bytes
            || actual_epoch != target_epoch
        {
            return Err(PostgresTransferError::Canonical(
                "deployment metadata mismatch",
            ));
        }
        let (healthy, isolated) = self.hydrated_room_dispositions(summary)?;
        let registry = worldstream_core::builtin_worldstream_registry()
            .map_err(|_| PostgresTransferError::Canonical("retained Pack registry"))?;
        for room_id in healthy {
            let verification = PostgresRoomStore::verify_room_in_transaction(transaction, &room_id)
                .map_err(PostgresTransferError::Semantic)?;
            if verification.integrity_status != "healthy" {
                return Err(PostgresTransferError::Canonical(
                    "healthy Room was not published healthy",
                ));
            }
            if verification.verify_executable_replay(&registry).is_err() {
                return Err(PostgresTransferError::Canonical(
                    "healthy Room executable replay",
                ));
            }
        }
        for (room_id, (policy, expected_status, expected_generation)) in isolated {
            if policy != NativeSqliteRoomPolicyV1::IsolatedCorrupt
                || !matches!(expected_status.as_str(), "faulted" | "quarantined")
            {
                return Err(PostgresTransferError::Canonical(
                    "isolated Room disposition",
                ));
            }
            let row = transaction
                .query_opt(
                    "SELECT integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE",
                    &[&room_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .ok_or(PostgresTransferError::Canonical(
                    "isolated Room is not pre-existing",
                ))?;
            let actual_generation: i64 = row.try_get(0).map_err(PostgresTransferError::Sql)?;
            let actual_status: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
            if actual_generation != expected_generation || actual_status != expected_status {
                return Err(PostgresTransferError::Canonical(
                    "isolated Room disposition mismatch",
                ));
            }
        }
        if let Some(summary) = summary {
            verify_native_row_cardinalities(transaction, summary)?;
        }
        Ok(())
    }

    /// Publishes canonical Room bytes into the existing Core-owned tables.
    ///
    /// This deliberately runs in the pre-retirement verification transaction
    /// while the target remains serving-fenced. Existing rows are compared
    /// before any update, so a retry is idempotent while a different target
    /// bundle fails closed.
    #[allow(clippy::too_many_lines)]
    fn publish_canonical_rooms(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<BTreeSet<String>, PostgresTransferError> {
        self.persist_deployment_identity(transaction)?;
        let metadata = extract_deployment_metadata(self.bundle.records())?;
        let target_epoch = i64::try_from(self.target.storage_epoch())
            .map_err(|_| PostgresTransferError::InvalidProviderValue("target epoch"))?;
        if let Some(row) = transaction
            .query_opt(
                "SELECT deployment_lineage_bytes, storage_epoch_bytes, storage_epoch FROM worldstream_deployment_metadata WHERE target_id = true FOR UPDATE",
                &[],
            )
            .map_err(PostgresTransferError::Sql)?
        {
            let stored_lineage: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
            let stored_epoch_bytes: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
            let stored_epoch: i64 = row.try_get(2).map_err(PostgresTransferError::Sql)?;
            if stored_lineage != metadata.deployment_lineage
                || stored_epoch_bytes != metadata.storage_epoch_bytes
                || stored_epoch != target_epoch
            {
                return Err(PostgresTransferError::Canonical(
                    "deployment metadata mismatch",
                ));
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO worldstream_deployment_metadata(target_id, deployment_lineage_bytes, storage_epoch_bytes, storage_epoch) VALUES (true, $1, $2, $3)",
                    &[&metadata.deployment_lineage, &metadata.storage_epoch_bytes, &target_epoch],
                )
                .map_err(PostgresTransferError::Sql)?;
        }

        let mut rooms = BTreeMap::<String, CanonicalRoomBytes>::new();
        let mut created_roots = BTreeSet::new();
        for record in self.bundle.records() {
            let RecordKindV1::Canonical(kind) = record.kind() else {
                continue;
            };
            let Some(room_id) = canonical_room_id(record.identity()) else {
                continue;
            };
            let room = rooms.entry(room_id).or_default();
            match kind {
                CanonicalRecordKindV1::RoomGenesis => {
                    set_exact(&mut room.genesis, record.bytes(), "duplicate Room Genesis")?;
                }
                CanonicalRecordKindV1::RoomHead => {
                    set_exact(&mut room.head, record.bytes(), "duplicate Room Head")?;
                }
                CanonicalRecordKindV1::CoreMaterialization => {
                    set_exact(
                        &mut room.core,
                        record.bytes(),
                        "duplicate Core materialization",
                    )?;
                }
                CanonicalRecordKindV1::ActivityMaterialization => {
                    set_exact(
                        &mut room.activity,
                        record.bytes(),
                        "duplicate Activity materialization",
                    )?;
                }
                CanonicalRecordKindV1::ArtifactMetadata => {
                    if record.identity().ends_with("/pack-revision-lock") {
                        set_exact(
                            &mut room.pack_revision_lock,
                            record.bytes(),
                            "duplicate pack revision lock",
                        )?;
                    }
                }
                CanonicalRecordKindV1::RoomTransition => {
                    let sequence = canonical_room_sequence(record.identity())?;
                    if room
                        .transitions
                        .insert(sequence, record.bytes().to_vec())
                        .is_some()
                    {
                        return Err(PostgresTransferError::Canonical(
                            "duplicate Room Transition",
                        ));
                    }
                }
                _ => {}
            }
        }

        let published_rooms = rooms.keys().cloned().collect::<BTreeSet<_>>();
        for (room_id, room) in rooms {
            let pack_revision_lock = required_pack_revision_lock(&room)?.to_vec();
            let (genesis, head, core, activity, pack_revision_lock) =
                match (room.genesis, room.head, room.core, room.activity) {
                    (Some(genesis), Some(head), Some(core), Some(activity)) => {
                        (genesis, head, core, activity, pack_revision_lock)
                    }
                    _ => return Err(PostgresTransferError::Canonical("incomplete Room bytes")),
                };

            if let Some(row) = transaction
                .query_opt(
                    "SELECT pack_revision_lock_bytes, genesis_bytes FROM worldstream_genesis WHERE room_id = $1 FOR UPDATE",
                    &[&room_id],
                )
                .map_err(PostgresTransferError::Sql)?
            {
                let stored_pack_lock: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
                let stored_genesis: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
                if stored_pack_lock != pack_revision_lock || stored_genesis != genesis {
                    return Err(PostgresTransferError::Canonical(
                        "Genesis or pack revision lock bytes mismatch",
                    ));
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO worldstream_genesis(room_id, pack_revision_lock_bytes, genesis_bytes) VALUES ($1, $2, $3)",
                        &[&room_id, &pack_revision_lock, &genesis],
                    )
                    .map_err(PostgresTransferError::Sql)?;
            }

            if let Some(row) = transaction
                .query_opt(
                    "SELECT head_bytes FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE",
                    &[&room_id],
                )
                .map_err(PostgresTransferError::Sql)?
            {
                let stored_head: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
                if stored_head != head {
                    return Err(PostgresTransferError::Canonical("Head bytes mismatch"));
                }
            } else {
                let generation = i64::try_from(self.target.storage_epoch())
                    .map_err(|_| PostgresTransferError::InvalidProviderValue("target epoch"))?;
                transaction
                    .execute(
                        "INSERT INTO worldstream_room_roots(room_id, head_bytes, integrity_generation, integrity_status) VALUES ($1, $2, $3, 'healthy')",
                        &[&room_id, &head, &generation],
                    )
                    .map_err(PostgresTransferError::Sql)?;
                created_roots.insert(room_id.clone());
            }

            if let Some(row) = transaction
                .query_opt(
                    "SELECT core_state_bytes, activity_state_bytes FROM worldstream_materializations WHERE room_id = $1 FOR UPDATE",
                    &[&room_id],
                )
                .map_err(PostgresTransferError::Sql)?
            {
                let stored_core: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
                let stored_activity: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
                if stored_core != core || stored_activity != activity {
                    return Err(PostgresTransferError::Canonical("materialization bytes mismatch"));
                }
            } else {
                transaction
                    .execute(
                        "INSERT INTO worldstream_materializations(room_id, core_state_bytes, activity_state_bytes) VALUES ($1, $2, $3)",
                        &[&room_id, &core, &activity],
                    )
                    .map_err(PostgresTransferError::Sql)?;
            }

            for (room_seq, transition) in room.transitions {
                if room_seq <= 0 {
                    return Err(PostgresTransferError::Canonical("Room Transition sequence"));
                }
                if let Some(row) = transaction
                    .query_opt(
                        "SELECT transition_bytes FROM worldstream_transitions WHERE room_id = $1 AND room_seq = $2 FOR UPDATE",
                        &[&room_id, &room_seq],
                    )
                    .map_err(PostgresTransferError::Sql)?
                {
                    let stored: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
                    if stored != transition {
                        return Err(PostgresTransferError::Canonical(
                            "Transition bytes mismatch",
                        ));
                    }
                } else {
                    transaction
                        .execute(
                            "INSERT INTO worldstream_transitions(room_id, room_seq, transition_bytes) VALUES ($1, $2, $3)",
                            &[&room_id, &room_seq, &transition],
                        )
                        .map_err(PostgresTransferError::Sql)?;
                }
            }
        }
        self.publish_native_operational_rows(
            transaction,
            &created_roots,
            NativePublicationMode::Hydrate,
        )?;
        Ok(published_rooms)
    }

    /// Publishes the exact native operational rows carried by a native SQLite
    /// bundle. The wire format is deliberately decoded here rather than
    /// reconstructed from canonical Room bytes: operational counters, leases,
    /// cursors, and receipts are provider evidence in their own right.
    fn publish_native_operational_rows(
        &self,
        transaction: &mut Transaction<'_>,
        created_roots: &BTreeSet<String>,
        mode: NativePublicationMode,
    ) -> Result<(), PostgresTransferError> {
        let mut rows = Vec::new();
        for record in self.bundle.records() {
            if record.kind() != RecordKindV1::Canonical(CanonicalRecordKindV1::NativeOperationalRow)
            {
                continue;
            }
            rows.push(decode_native_operational_row(record.bytes())?);
        }
        for table in PUBLICATION_ORDER {
            for row in rows.iter().filter(|row| row.table == *table) {
                if mode.requires_existing_row() {
                    ensure_native_operational_row_present(transaction, row)?;
                }
                match row.table.as_str() {
                    "retired_authority_fences_v1" => {
                        publish_retired_authority_fence(transaction, row)?;
                    }
                    "principals" => publish_authority_principal(transaction, row)?,
                    "runners" => publish_authority_runner(transaction, row)?,
                    "capabilities" => publish_authority_capability(transaction, row)?,
                    "capability_scopes" => publish_authority_capability_scope(transaction, row)?,
                    "runner_capability_memberships" => {
                        publish_runner_capability_membership(transaction, row)?;
                    }
                    "authority_change_receipts" => {
                        publish_authority_change_receipt(transaction, row)?;
                    }
                    "authority_audit" => publish_authority_audit(transaction, row)?,
                    "room_integrity" => publish_room_integrity(transaction, row, created_roots)?,
                    "room_members" => publish_room_member(transaction, row)?,
                    "timers" => publish_timer(transaction, row)?,
                    "observation_frames" => publish_observation_frame(transaction, row)?,
                    "observation_consequences" => {
                        publish_observation_consequence(transaction, row)?;
                    }
                    "activation_decisions" => publish_activation_decision(transaction, row)?,
                    "activation_intents" => publish_activation_intent(transaction, row)?,
                    "activation_operation_receipts" => {
                        publish_activation_receipt(transaction, row)?;
                    }
                    "semantic_receipts" => {
                        publish_semantic_receipt(transaction, row, mode)?;
                    }
                    "integrity_incidents" => publish_integrity_incident(transaction, row)?,
                    _ => {
                        return Err(PostgresTransferError::Canonical(
                            "unknown operational table",
                        ));
                    }
                }
            }
        }
        if rows.len()
            != PUBLICATION_ORDER
                .iter()
                .map(|table| rows.iter().filter(|row| row.table == *table).count())
                .sum::<usize>()
        {
            return Err(PostgresTransferError::Canonical(
                "unknown operational table",
            ));
        }
        Ok(())
    }

    fn verify_native_operational_rows(
        &self,
        transaction: &mut Transaction<'_>,
    ) -> Result<(), PostgresTransferError> {
        self.publish_native_operational_rows(
            transaction,
            &BTreeSet::new(),
            NativePublicationMode::VerifyOnly,
        )
    }
}

fn verify_native_row_cardinalities(
    transaction: &mut Transaction<'_>,
    summary: &NativeSqliteBundleSummaryV1,
) -> Result<(), PostgresTransferError> {
    const COUNTS: &[(&str, &str)] = &[
        (
            "retired_authority_fences_v1",
            "SELECT count(*) FROM worldstream_retired_authority_fences_v1",
        ),
        (
            "principals",
            "SELECT count(*) FROM worldstream_authority_principals",
        ),
        (
            "runners",
            "SELECT count(*) FROM worldstream_authority_runners",
        ),
        (
            "capabilities",
            "SELECT count(*) FROM worldstream_authority_capabilities",
        ),
        (
            "capability_scopes",
            "SELECT count(*) FROM worldstream_authority_capability_scopes",
        ),
        (
            "runner_capability_memberships",
            "SELECT count(*) FROM worldstream_authority_runner_capability_memberships",
        ),
        (
            "authority_change_receipts",
            "SELECT count(*) FROM worldstream_authority_change_receipts",
        ),
        (
            "authority_audit",
            "SELECT count(*) FROM worldstream_authority_audit",
        ),
        (
            "room_integrity",
            "SELECT count(*) FROM worldstream_room_roots",
        ),
        ("room_members", "SELECT count(*) FROM worldstream_members"),
        ("timers", "SELECT count(*) FROM worldstream_timers"),
        (
            "observation_frames",
            "SELECT count(*) FROM worldstream_frames",
        ),
        (
            "observation_consequences",
            "SELECT count(*) FROM worldstream_observation_consequences",
        ),
        (
            "activation_decisions",
            "SELECT count(*) FROM worldstream_activation_decisions",
        ),
        (
            "activation_intents",
            "SELECT count(*) FROM worldstream_activation_intents",
        ),
        (
            "activation_operation_receipts",
            "SELECT count(*) FROM worldstream_activation_operation_receipts",
        ),
        (
            "semantic_receipts",
            "SELECT count(*) FROM worldstream_semantic_receipts",
        ),
        (
            "semantic_receipts",
            "SELECT count(*) FROM worldstream_operation_guards",
        ),
        (
            "integrity_incidents",
            "SELECT count(*) FROM worldstream_integrity_incidents",
        ),
    ];
    for (source_table, query) in COUNTS {
        let expected =
            *summary
                .table_counts()
                .get(*source_table)
                .ok_or(PostgresTransferError::Canonical(
                    "native table count is absent",
                ))?;
        let actual = usize::try_from(
            transaction
                .query_one(*query, &[])
                .map_err(PostgresTransferError::Sql)?
                .try_get::<_, i64>(0)
                .map_err(PostgresTransferError::Sql)?,
        )
        .map_err(|_| PostgresTransferError::InvalidProviderValue("native table count"))?;
        if actual != expected {
            return Err(PostgresTransferError::Canonical(
                "native table cardinality mismatch",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativePublicationMode {
    Hydrate,
    VerifyOnly,
}

const PUBLICATION_ORDER: &[&str] = &[
    "retired_authority_fences_v1",
    "principals",
    "runners",
    "capabilities",
    "capability_scopes",
    "runner_capability_memberships",
    "authority_change_receipts",
    "authority_audit",
    "room_integrity",
    "room_members",
    "timers",
    "observation_frames",
    "observation_consequences",
    "activation_decisions",
    "activation_intents",
    "activation_operation_receipts",
    "semantic_receipts",
    "integrity_incidents",
];

impl NativePublicationMode {
    const fn requires_existing_row(self) -> bool {
        matches!(self, Self::VerifyOnly)
    }
}

struct DeploymentMetadataBytes {
    deployment_lineage: Vec<u8>,
    storage_epoch_bytes: Vec<u8>,
}

const DEPLOYMENT_IDENTITY: &str = "deployment/identity";

fn extract_deployment_identity(
    records: &[worldstream_transfer::LogicalRecordV1],
) -> Result<DeploymentIdentityV1, PostgresTransferError> {
    let records = records.iter().filter(|record| {
        record.kind() == RecordKindV1::Canonical(CanonicalRecordKindV1::ArtifactMetadata)
            && record.identity() == DEPLOYMENT_IDENTITY
    });
    let mut identity = None;
    for record in records {
        if identity.is_some() {
            return Err(PostgresTransferError::DeploymentIdentity(
                "duplicate deployment identity record",
            ));
        }
        identity = Some(
            DeploymentIdentityV1::from_canonical_bytes(record.bytes()).map_err(|_| {
                PostgresTransferError::DeploymentIdentity("non-canonical identity bytes")
            })?,
        );
    }
    identity.ok_or(PostgresTransferError::DeploymentIdentity(
        "missing deployment identity record",
    ))
}

fn verify_bundle_deployment_identity(
    bundle: &TransferBundleV1,
    identity: &DeploymentIdentityV1,
) -> Result<(), PostgresTransferError> {
    // The bundle header carries the canonical primary pack while the bound
    // deployment identity record carries the complete ordered pack set.
    let Some(bundle_pack) = bundle.pack() else {
        return Err(PostgresTransferError::DeploymentIdentity(
            "whole-deployment pack header is absent",
        ));
    };
    if identity.packs().first() != Some(bundle_pack) {
        return Err(PostgresTransferError::DeploymentIdentity(
            "pack identity differs from bundle header",
        ));
    }
    if identity.resources() != bundle.resources() {
        return Err(PostgresTransferError::DeploymentIdentity(
            "resource identity differs from bundle header",
        ));
    }
    Ok(())
}

fn resource_record_identity(resource: &ResourceIdentityV1) -> String {
    format!(
        "deployment/resource/{}/{}",
        resource_kind_name(resource.kind()),
        resource.identity()
    )
}

fn resource_kind_name(kind: ResourceKindV1) -> &'static str {
    match kind {
        ResourceKindV1::Artifact => "artifact",
        ResourceKindV1::Codec => "codec",
        ResourceKindV1::Schema => "schema",
    }
}

fn resource_kind_tag(kind: ResourceKindV1) -> u8 {
    match kind {
        ResourceKindV1::Artifact => 1,
        ResourceKindV1::Codec => 2,
        ResourceKindV1::Schema => 3,
    }
}

fn deployment_pack_set_digest(identity: &DeploymentIdentityV1) -> DigestV1 {
    let mut bytes = b"worldstream/deployment-pack-set/v1".to_vec();
    for pack in identity.packs() {
        bytes.extend_from_slice(pack.pack_id().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(pack.revision().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&pack.digest().as_bytes());
    }
    DigestV1::hash(&bytes)
}

fn deployment_resource_set_digest(identity: &DeploymentIdentityV1) -> DigestV1 {
    let mut bytes = b"worldstream/deployment-resource-set/v1".to_vec();
    for resource in identity.resources() {
        bytes.push(resource_kind_tag(resource.kind()));
        bytes.extend_from_slice(resource.identity().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&resource.size_bytes().to_le_bytes());
        bytes.extend_from_slice(&resource.digest().as_bytes());
    }
    DigestV1::hash(&bytes)
}

fn extract_deployment_metadata(
    records: &[worldstream_transfer::LogicalRecordV1],
) -> Result<DeploymentMetadataBytes, PostgresTransferError> {
    let mut deployment_lineage = None;
    let mut storage_epoch_bytes = None;
    for record in records {
        let RecordKindV1::Canonical(kind) = record.kind() else {
            continue;
        };
        match kind {
            CanonicalRecordKindV1::DeploymentLineage => {
                if deployment_lineage.is_some() {
                    return Err(PostgresTransferError::Canonical(
                        "duplicate DeploymentLineage",
                    ));
                }
                set_exact(
                    &mut deployment_lineage,
                    record.bytes(),
                    "DeploymentLineage bytes",
                )?;
            }
            CanonicalRecordKindV1::StorageEpoch => {
                if storage_epoch_bytes.is_some() {
                    return Err(PostgresTransferError::Canonical("duplicate StorageEpoch"));
                }
                set_exact(
                    &mut storage_epoch_bytes,
                    record.bytes(),
                    "StorageEpoch bytes",
                )?;
            }
            _ => {}
        }
    }
    Ok(DeploymentMetadataBytes {
        deployment_lineage: deployment_lineage.ok_or(PostgresTransferError::Canonical(
            "missing DeploymentLineage",
        ))?,
        storage_epoch_bytes: storage_epoch_bytes
            .ok_or(PostgresTransferError::Canonical("missing StorageEpoch"))?,
    })
}

#[derive(Default)]
struct CanonicalRoomBytes {
    genesis: Option<Vec<u8>>,
    head: Option<Vec<u8>>,
    core: Option<Vec<u8>>,
    activity: Option<Vec<u8>>,
    pack_revision_lock: Option<Vec<u8>>,
    transitions: BTreeMap<i64, Vec<u8>>,
}

fn set_exact(
    slot: &mut Option<Vec<u8>>,
    bytes: &[u8],
    what: &'static str,
) -> Result<(), PostgresTransferError> {
    if slot.as_deref().is_some_and(|stored| stored != bytes) {
        return Err(PostgresTransferError::Canonical(what));
    }
    *slot = Some(bytes.to_vec());
    Ok(())
}

fn required_pack_revision_lock(room: &CanonicalRoomBytes) -> Result<&[u8], PostgresTransferError> {
    room.pack_revision_lock
        .as_deref()
        .ok_or(PostgresTransferError::Canonical(
            "missing pack revision lock bytes",
        ))
}

fn canonical_room_id(identity: &str) -> Option<String> {
    let mut parts = identity.strip_prefix("room/")?.split('/');
    let room_id = parts.next()?.to_owned();
    (!room_id.is_empty()).then_some(room_id)
}

fn canonical_room_sequence(identity: &str) -> Result<i64, PostgresTransferError> {
    let parts = identity
        .strip_prefix("room/")
        .and_then(|value| value.split('/').next_back())
        .ok_or(PostgresTransferError::Canonical("Room Transition identity"))?;
    let sequence = parts
        .parse::<i64>()
        .map_err(|_| PostgresTransferError::Canonical("Room Transition sequence"))?;
    if sequence <= 0 || !identity.contains("/transition/") {
        return Err(PostgresTransferError::Canonical("Room Transition identity"));
    }
    Ok(sequence)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NativeValue {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
}

struct NativeRow {
    table: String,
    values: Vec<NativeValue>,
}

fn decode_native_operational_row(bytes: &[u8]) -> Result<NativeRow, PostgresTransferError> {
    let mut reader = NativeReader::new(bytes);
    if reader.take(8)? != b"WSNSROW1" || reader.u16()? != 1 {
        return Err(PostgresTransferError::Canonical("native row wire version"));
    }
    let table = reader.string()?;
    let expected = match table.as_str() {
        "principals" | "runners" => 4,
        "capabilities" => 10,
        "capability_scopes" => 2,
        "runner_capability_memberships" | "room_integrity" => 3,
        "authority_change_receipts" | "activation_operation_receipts" => 9,
        "authority_audit" | "semantic_receipts" => 12,
        "activation_decisions" => 5,
        "activation_intents" => 19,
        "retired_authority_fences_v1"
        | "observation_consequences"
        | "observation_frames"
        | "timers"
        | "integrity_incidents" => 6,
        "room_members" => 13,
        _ => {
            return Err(PostgresTransferError::Canonical(
                "unknown operational table",
            ));
        }
    };
    let count = reader.u32()? as usize;
    if count != expected {
        return Err(PostgresTransferError::Canonical(
            "native operational row shape",
        ));
    }
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(match reader.u8()? {
            0 => NativeValue::Null,
            1 => NativeValue::Integer(reader.i64()?),
            2 => NativeValue::Text(reader.string()?),
            3 => NativeValue::Blob(reader.blob()?),
            _ => return Err(PostgresTransferError::Canonical("native row storage class")),
        });
    }
    if !reader.done() {
        return Err(PostgresTransferError::Canonical(
            "native row trailing bytes",
        ));
    }
    Ok(NativeRow { table, values })
}

fn native_text(row: &NativeRow, index: usize) -> Result<String, PostgresTransferError> {
    match row.values.get(index) {
        Some(NativeValue::Text(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(PostgresTransferError::Canonical("native row text value")),
    }
}

fn native_optional_text(
    row: &NativeRow,
    index: usize,
) -> Result<Option<String>, PostgresTransferError> {
    match row.values.get(index) {
        Some(NativeValue::Text(value)) => Ok(Some(value.clone())),
        Some(NativeValue::Null) => Ok(None),
        _ => Err(PostgresTransferError::Canonical(
            "native row optional text value",
        )),
    }
}

fn native_integer(row: &NativeRow, index: usize) -> Result<i64, PostgresTransferError> {
    match row.values.get(index) {
        Some(NativeValue::Integer(value)) => Ok(*value),
        _ => Err(PostgresTransferError::Canonical("native row integer value")),
    }
}

fn native_blob(row: &NativeRow, index: usize) -> Result<Vec<u8>, PostgresTransferError> {
    match row.values.get(index) {
        Some(NativeValue::Blob(value)) => Ok(value.clone()),
        _ => Err(PostgresTransferError::Canonical("native row blob value")),
    }
}

fn native_optional_blob(
    row: &NativeRow,
    index: usize,
) -> Result<Option<Vec<u8>>, PostgresTransferError> {
    match row.values.get(index) {
        Some(NativeValue::Blob(value)) => Ok(Some(value.clone())),
        Some(NativeValue::Null) => Ok(None),
        _ => Err(PostgresTransferError::Canonical(
            "native row optional blob value",
        )),
    }
}

fn native_blake3(value: &str) -> Result<Vec<u8>, PostgresTransferError> {
    let value = value
        .strip_prefix("blake3:")
        .ok_or(PostgresTransferError::Canonical("native digest text"))?;
    if value.len() != 64 {
        return Err(PostgresTransferError::Canonical("native digest text"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            u8::try_from((high << 4) | low)
                .map_err(|_| PostgresTransferError::Canonical("native digest text"))
        })
        .collect()
}

fn hex_digit(value: u8) -> Result<u32, PostgresTransferError> {
    match value {
        b'0'..=b'9' => Ok(u32::from(value - b'0')),
        b'a'..=b'f' => Ok(u32::from(value - b'a' + 10)),
        _ => Err(PostgresTransferError::Canonical("native digest text")),
    }
}

#[allow(clippy::too_many_lines)]
fn ensure_native_operational_row_present(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let present = match row.table.as_str() {
        "retired_authority_fences_v1" => {
            let witness_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_retired_authority_fences_v1 WHERE witness_id = $1",
                    &[&witness_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "principals" => {
            let principal_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_principals WHERE principal_id = $1",
                    &[&principal_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "runners" => {
            let runner_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_runners WHERE runner_id = $1",
                    &[&runner_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "capabilities" => {
            let capability_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_capabilities WHERE capability_id = $1",
                    &[&capability_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "capability_scopes" => {
            let capability_id = native_text(row, 0)?;
            let scope = native_text(row, 1)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_capability_scopes WHERE capability_id = $1 AND scope = $2",
                    &[&capability_id, &scope],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "runner_capability_memberships" => {
            let capability_id = native_text(row, 0)?;
            let room_id = native_text(row, 1)?;
            let member_id = native_text(row, 2)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_runner_capability_memberships WHERE capability_id = $1 AND room_id = $2 AND member_id = $3",
                    &[&capability_id, &room_id, &member_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "authority_change_receipts" => {
            let change_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_change_receipts WHERE change_id = $1",
                    &[&change_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "authority_audit" => {
            let audit_seq = native_integer(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_authority_audit WHERE audit_seq = $1",
                    &[&audit_seq],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "room_integrity" => {
            let room_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_room_roots WHERE room_id = $1",
                    &[&room_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "room_members" => {
            let room_id = native_text(row, 0)?;
            let member_id = native_text(row, 1)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_members WHERE room_id = $1 AND member_id = $2",
                    &[&room_id, &member_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "timers" => {
            let room_id = native_text(row, 0)?;
            let timer_id = native_text(row, 1)?;
            let generation = native_integer(row, 2)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_timers WHERE room_id = $1 AND timer_id = $2 AND generation = $3",
                    &[&room_id, &timer_id, &generation],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "observation_frames" => {
            let room_id = native_text(row, 0)?;
            let member_id = native_text(row, 1)?;
            let frame_seq = native_integer(row, 2)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq = $3",
                    &[&room_id, &member_id, &frame_seq],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "observation_consequences" => {
            let room_id = native_text(row, 0)?;
            let member_id = native_text(row, 1)?;
            let cause_room_seq = native_integer(row, 2)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_observation_consequences WHERE room_id = $1 AND member_id = $2 AND cause_room_seq = $3",
                    &[&room_id, &member_id, &cause_room_seq],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "activation_decisions" => {
            let room_id = native_text(row, 0)?;
            let cause_room_seq = native_integer(row, 1)?;
            let decision_id = native_text(row, 2)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_activation_decisions WHERE room_id = $1 AND cause_room_seq = $2 AND decision_id = $3",
                    &[&room_id, &cause_room_seq, &decision_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "activation_intents" => {
            let activation_id = native_text(row, 0)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_activation_intents WHERE activation_id = $1",
                    &[&activation_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "activation_operation_receipts" => {
            let room_id = native_text(row, 0)?;
            let operation_id = native_text(row, 1)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_activation_operation_receipts WHERE room_id = $1 AND operation_id = $2",
                    &[&room_id, &operation_id],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "semantic_receipts" => {
            let identity_bytes = native_blob(row, 2)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_semantic_receipts WHERE identity_bytes = $1",
                    &[&identity_bytes],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        "integrity_incidents" => {
            let room_id = native_text(row, 0)?;
            let incident_seq = native_integer(row, 1)?;
            transaction
                .query_opt(
                    "SELECT 1 FROM worldstream_integrity_incidents WHERE room_id = $1 AND incident_seq = $2",
                    &[&room_id, &incident_seq],
                )
                .map_err(PostgresTransferError::Sql)?
                .is_some()
        }
        _ => {
            return Err(PostgresTransferError::Canonical(
                "unknown operational table",
            ));
        }
    };
    if !present {
        return Err(PostgresTransferError::Canonical(
            "authoritative native row is missing",
        ));
    }
    Ok(())
}

fn publish_retired_authority_fence(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let witness_id = native_text(row, 0)?;
    let principal = native_text(row, 1)?;
    let generation = native_integer(row, 2)?;
    let revocation_bytes = native_blob(row, 3)?;
    let revocation_hash = native_blob(row, 4)?;
    let active = native_integer(row, 5)? != 0;
    transaction
        .execute(
            "INSERT INTO worldstream_retired_authority_fences_v1(witness_id, authenticated_principal, generation, scope_revocation_bytes, scope_revocation_hash, active) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (witness_id) DO NOTHING",
            &[&witness_id, &principal, &generation, &revocation_bytes, &revocation_hash, &active],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT authenticated_principal, generation, scope_revocation_bytes, scope_revocation_hash, active FROM worldstream_retired_authority_fences_v1 WHERE witness_id = $1",
            &[&witness_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, bool>(4)
            .map_err(PostgresTransferError::Sql)?,
    ) != (
        principal,
        generation,
        revocation_bytes,
        revocation_hash,
        active,
    ) {
        return Err(PostgresTransferError::Canonical("authority fence mismatch"));
    }
    Ok(())
}

fn publish_authority_principal(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let principal_id = native_text(row, 0)?;
    let kind = native_text(row, 1)?;
    let status = native_text(row, 2)?;
    let generation = native_integer(row, 3)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_principals(principal_id, principal_kind, authority_status, principal_generation) VALUES ($1, $2, $3, $4) ON CONFLICT (principal_id) DO NOTHING",
            &[&principal_id, &kind, &status, &generation],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT principal_kind, authority_status, principal_generation FROM worldstream_authority_principals WHERE principal_id = $1",
            &[&principal_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(2)
            .map_err(PostgresTransferError::Sql)?,
    ) != (kind, status, generation)
    {
        return Err(PostgresTransferError::Canonical("Principal mismatch"));
    }
    Ok(())
}

fn publish_authority_runner(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let runner_id = native_text(row, 0)?;
    let owner = native_text(row, 1)?;
    let status = native_text(row, 2)?;
    let generation = native_integer(row, 3)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_runners(runner_id, owner_principal_id, authority_status, runner_generation) VALUES ($1, $2, $3, $4) ON CONFLICT (runner_id) DO NOTHING",
            &[&runner_id, &owner, &status, &generation],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT owner_principal_id, authority_status, runner_generation FROM worldstream_authority_runners WHERE runner_id = $1",
            &[&runner_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(2)
            .map_err(PostgresTransferError::Sql)?,
    ) != (owner, status, generation)
    {
        return Err(PostgresTransferError::Canonical("Runner mismatch"));
    }
    Ok(())
}

fn publish_authority_capability(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let capability_id = native_text(row, 0)?;
    let token_hash = native_blob(row, 1)?;
    let principal_id = native_text(row, 2)?;
    let profile = native_text(row, 3)?;
    let target_room = native_optional_text(row, 4)?;
    let target_member = native_optional_text(row, 5)?;
    let runner = native_optional_text(row, 6)?;
    let generation = native_integer(row, 7)?;
    let expires_at = native_optional_text(row, 8)?;
    let revoked_at = native_optional_text(row, 9)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_capabilities(capability_id, token_hash, principal_id, profile_kind, target_room_id, target_member_id, runner_id, authority_generation, expires_at, revoked_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (capability_id) DO NOTHING",
            &[&capability_id, &token_hash, &principal_id, &profile, &target_room, &target_member, &runner, &generation, &expires_at, &revoked_at],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT token_hash, principal_id, profile_kind, target_room_id, target_member_id, runner_id, authority_generation, expires_at, revoked_at FROM worldstream_authority_capabilities WHERE capability_id = $1",
            &[&capability_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, Vec<u8>>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(4)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(5)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(6)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(7)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(8)
            .map_err(PostgresTransferError::Sql)?,
    ) != (
        token_hash,
        principal_id,
        profile,
        target_room,
        target_member,
        runner,
        generation,
        expires_at,
        revoked_at,
    ) {
        return Err(PostgresTransferError::Canonical("Capability mismatch"));
    }
    Ok(())
}

fn publish_authority_capability_scope(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let capability_id = native_text(row, 0)?;
    let scope = native_text(row, 1)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_capability_scopes(capability_id, scope) VALUES ($1, $2) ON CONFLICT (capability_id, scope) DO NOTHING",
            &[&capability_id, &scope],
        )
        .map_err(PostgresTransferError::Sql)?;
    Ok(())
}

fn publish_runner_capability_membership(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let capability_id = native_text(row, 0)?;
    let room_id = native_text(row, 1)?;
    let member_id = native_text(row, 2)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_runner_capability_memberships(capability_id, room_id, member_id) VALUES ($1, $2, $3) ON CONFLICT (capability_id, room_id, member_id) DO NOTHING",
            &[&capability_id, &room_id, &member_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    Ok(())
}

fn publish_authority_change_receipt(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let change_id = native_text(row, 0)?;
    let authenticated = native_optional_text(row, 1)?;
    let request_hash = native_blob(row, 2)?;
    let result_kind = native_text(row, 3)?;
    let target_kind = native_text(row, 4)?;
    let target_id = native_text(row, 5)?;
    let secondary = native_optional_text(row, 6)?;
    let generation = native_integer(row, 7)?;
    let checked_at = native_text(row, 8)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_change_receipts(change_id, authenticated_principal, request_hash, result_kind, target_kind, target_id, secondary_target_id, resulting_generation, checked_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (change_id) DO NOTHING",
            &[&change_id, &authenticated, &request_hash, &result_kind, &target_kind, &target_id, &secondary, &generation, &checked_at],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT authenticated_principal, request_hash, result_kind, target_kind, target_id, secondary_target_id, resulting_generation, checked_at FROM worldstream_authority_change_receipts WHERE change_id = $1",
            &[&change_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, Option<String>>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(4)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(5)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(6)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(7)
            .map_err(PostgresTransferError::Sql)?,
    ) != (
        authenticated,
        request_hash,
        result_kind,
        target_kind,
        target_id,
        secondary,
        generation,
        checked_at,
    ) {
        return Err(PostgresTransferError::Canonical(
            "authority Receipt mismatch",
        ));
    }
    Ok(())
}

fn publish_authority_audit(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let audit_seq = native_integer(row, 0)?;
    let change_id = native_text(row, 1)?;
    let actor = native_optional_text(row, 2)?;
    let target_kind = native_text(row, 3)?;
    let target_id = native_text(row, 4)?;
    let secondary = native_optional_text(row, 5)?;
    let change_kind = native_text(row, 6)?;
    let prior = match row.values.get(7) {
        Some(NativeValue::Integer(value)) => Some(*value),
        Some(NativeValue::Null) => None,
        _ => {
            return Err(PostgresTransferError::Canonical(
                "authority audit generation",
            ));
        }
    };
    let resulting = native_integer(row, 8)?;
    let checked_at = native_text(row, 9)?;
    let reason = native_optional_text(row, 10)?;
    let request_hash = native_blob(row, 11)?;
    transaction
        .execute(
            "INSERT INTO worldstream_authority_audit(audit_seq, change_id, actor_principal_id, target_kind, target_id, secondary_target_id, change_kind, prior_generation, resulting_generation, checked_at, reason_code, request_hash) OVERRIDING SYSTEM VALUE VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) ON CONFLICT (audit_seq) DO NOTHING",
            &[&audit_seq, &change_id, &actor, &target_kind, &target_id, &secondary, &change_kind, &prior, &resulting, &checked_at, &reason, &request_hash],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT change_id, actor_principal_id, target_kind, target_id, secondary_target_id, change_kind, prior_generation, resulting_generation, checked_at, reason_code, request_hash FROM worldstream_authority_audit WHERE audit_seq = $1",
            &[&audit_seq],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(4)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(5)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<i64>>(6)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(7)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(8)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(9)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(10)
            .map_err(PostgresTransferError::Sql)?,
    ) != (
        change_id,
        actor,
        target_kind,
        target_id,
        secondary,
        change_kind,
        prior,
        resulting,
        checked_at,
        reason,
        request_hash,
    ) {
        return Err(PostgresTransferError::Canonical("authority audit mismatch"));
    }
    Ok(())
}

fn publish_integrity_incident(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let incident_seq = native_integer(row, 1)?;
    let generation = native_integer(row, 2)?;
    let status = native_text(row, 3)?;
    let reason = native_text(row, 4)?;
    let details = native_optional_blob(row, 5)?;
    transaction
        .execute(
            "INSERT INTO worldstream_integrity_incidents(room_id, incident_seq, generation, status, reason_code, details_bytes) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (room_id, incident_seq) DO NOTHING",
            &[&room_id, &incident_seq, &generation, &status, &reason, &details],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT generation, status, reason_code, details_bytes FROM worldstream_integrity_incidents WHERE room_id = $1 AND incident_seq = $2",
            &[&room_id, &incident_seq],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, i64>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<Vec<u8>>>(3)
            .map_err(PostgresTransferError::Sql)?,
    ) != (generation, status, reason, details)
    {
        return Err(PostgresTransferError::Canonical(
            "integrity incident mismatch",
        ));
    }
    Ok(())
}

fn publish_room_integrity(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
    created_roots: &BTreeSet<String>,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let status = native_text(row, 1)?;
    let generation = native_integer(row, 2)?;
    if generation <= 0 || !matches!(status.as_str(), "healthy" | "faulted" | "quarantined") {
        return Err(PostgresTransferError::Canonical("native integrity row"));
    }
    let Some(existing) = transaction
        .query_opt(
            "SELECT integrity_generation, integrity_status FROM worldstream_room_roots WHERE room_id = $1 FOR UPDATE",
            &[&room_id],
        )
        .map_err(PostgresTransferError::Sql)?
    else {
        return Err(PostgresTransferError::Canonical(
            "isolated Room is not pre-existing",
        ));
    };
    let stored_generation: i64 = existing.try_get(0).map_err(PostgresTransferError::Sql)?;
    let stored_status: String = existing.try_get(1).map_err(PostgresTransferError::Sql)?;
    if stored_generation == generation && stored_status == status {
        return Ok(());
    }
    if !created_roots.contains(&room_id) {
        return Err(PostgresTransferError::Canonical(
            "integrity witness mismatch",
        ));
    }
    transaction
        .execute(
            "UPDATE worldstream_room_roots SET integrity_generation = $1, integrity_status = $2 WHERE room_id = $3",
            &[&generation, &status, &room_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    Ok(())
}

fn publish_room_member(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let member_id = native_text(row, 1)?;
    let membership = native_blob(row, 7)?;
    let frame_head = native_integer(row, 8)?;
    let membership_generation = native_integer(row, 9)?;
    let retained_frame_floor = native_integer(row, 10)?;
    let last_ack_frame_seq = match row.values.get(11) {
        Some(NativeValue::Integer(value)) => Some(*value),
        Some(NativeValue::Null) => None,
        _ => return Err(PostgresTransferError::Canonical("native member ack")),
    };
    let reset_required_through = match row.values.get(12) {
        Some(NativeValue::Integer(value)) => Some(*value),
        Some(NativeValue::Null) => None,
        _ => return Err(PostgresTransferError::Canonical("native member reset")),
    };
    transaction
        .execute(
            "INSERT INTO worldstream_members(room_id, member_id, membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (room_id, member_id) DO NOTHING",
            &[&room_id, &member_id, &membership, &frame_head, &membership_generation, &retained_frame_floor, &last_ack_frame_seq, &reset_required_through],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT membership_bytes, frame_head, membership_generation, retained_frame_floor, last_ack_frame_seq, reset_required_through FROM worldstream_members WHERE room_id = $1 AND member_id = $2 FOR UPDATE",
            &[&room_id, &member_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    let actual: (Vec<u8>, i64, i64, i64, Option<i64>, Option<i64>) = (
        stored.try_get(0).map_err(PostgresTransferError::Sql)?,
        stored.try_get(1).map_err(PostgresTransferError::Sql)?,
        stored.try_get(2).map_err(PostgresTransferError::Sql)?,
        stored.try_get(3).map_err(PostgresTransferError::Sql)?,
        stored.try_get(4).map_err(PostgresTransferError::Sql)?,
        stored.try_get(5).map_err(PostgresTransferError::Sql)?,
    );
    if actual
        != (
            membership,
            frame_head,
            membership_generation,
            retained_frame_floor,
            last_ack_frame_seq,
            reset_required_through,
        )
    {
        return Err(PostgresTransferError::Canonical("membership row mismatch"));
    }
    Ok(())
}

fn publish_timer(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let timer_id = native_text(row, 1)?;
    let generation = native_integer(row, 2)?;
    let scheduled_for = native_text(row, 3)?;
    let payload = native_blob(row, 4)?;
    let state = native_text(row, 5)?;
    transaction
        .execute(
            "INSERT INTO worldstream_timers(room_id, timer_id, generation, scheduled_for, payload_bytes, state) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (room_id, timer_id, generation) DO NOTHING",
            &[&room_id, &timer_id, &generation, &scheduled_for, &payload, &state],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT scheduled_for, payload_bytes, state FROM worldstream_timers WHERE room_id = $1 AND timer_id = $2 AND generation = $3",
            &[&room_id, &timer_id, &generation],
        )
        .map_err(PostgresTransferError::Sql)?;
    if stored
        .try_get::<_, String>(0)
        .map_err(PostgresTransferError::Sql)?
        != scheduled_for
        || stored
            .try_get::<_, Vec<u8>>(1)
            .map_err(PostgresTransferError::Sql)?
            != payload
        || stored
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?
            != state
    {
        return Err(PostgresTransferError::Canonical("timer row mismatch"));
    }
    Ok(())
}

fn publish_observation_frame(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let member_id = native_text(row, 1)?;
    let frame_seq = native_integer(row, 2)?;
    let cause_room_seq = native_integer(row, 3)?;
    let payload_hash = native_blake3(&native_text(row, 4)?)?;
    let payload = native_blob(row, 5)?;
    transaction
        .execute(
            "INSERT INTO worldstream_frames(room_id, member_id, frame_seq, cause_room_seq, payload_bytes, payload_hash) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (room_id, member_id, frame_seq) DO NOTHING",
            &[&room_id, &member_id, &frame_seq, &cause_room_seq, &payload, &payload_hash],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT cause_room_seq, payload_bytes, payload_hash FROM worldstream_frames WHERE room_id = $1 AND member_id = $2 AND frame_seq = $3",
            &[&room_id, &member_id, &frame_seq],
        )
        .map_err(PostgresTransferError::Sql)?;
    if stored
        .try_get::<_, i64>(0)
        .map_err(PostgresTransferError::Sql)?
        != cause_room_seq
        || stored
            .try_get::<_, Vec<u8>>(1)
            .map_err(PostgresTransferError::Sql)?
            != payload
        || stored
            .try_get::<_, Vec<u8>>(2)
            .map_err(PostgresTransferError::Sql)?
            != payload_hash
    {
        return Err(PostgresTransferError::Canonical(
            "observation frame mismatch",
        ));
    }
    Ok(())
}

fn publish_observation_consequence(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let member_id = native_text(row, 1)?;
    let cause_room_seq = native_integer(row, 2)?;
    let consequence_kind = native_text(row, 3)?;
    let payload = native_optional_blob(row, 4)?;
    let projection_hash = match row.values.get(5) {
        Some(NativeValue::Text(value)) => Some(native_blake3(value)?),
        Some(NativeValue::Null) => None,
        _ => return Err(PostgresTransferError::Canonical("native consequence hash")),
    };
    transaction
        .execute(
            "INSERT INTO worldstream_observation_consequences(room_id, member_id, cause_room_seq, consequence_kind, payload_bytes, projection_hash) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (room_id, member_id, cause_room_seq) DO NOTHING",
            &[&room_id, &member_id, &cause_room_seq, &consequence_kind, &payload, &projection_hash],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT consequence_kind, payload_bytes, projection_hash FROM worldstream_observation_consequences WHERE room_id = $1 AND member_id = $2 AND cause_room_seq = $3",
            &[&room_id, &member_id, &cause_room_seq],
        )
        .map_err(PostgresTransferError::Sql)?;
    if stored
        .try_get::<_, String>(0)
        .map_err(PostgresTransferError::Sql)?
        != consequence_kind
        || stored
            .try_get::<_, Option<Vec<u8>>>(1)
            .map_err(PostgresTransferError::Sql)?
            != payload
        || stored
            .try_get::<_, Option<Vec<u8>>>(2)
            .map_err(PostgresTransferError::Sql)?
            != projection_hash
    {
        return Err(PostgresTransferError::Canonical(
            "observation consequence mismatch",
        ));
    }
    Ok(())
}

fn publish_activation_decision(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let cause_room_seq = native_integer(row, 1)?;
    let decision_id = native_text(row, 2)?;
    let target_member_id = native_optional_text(row, 3)?;
    let decision = native_blob(row, 4)?;
    transaction
        .execute(
            "INSERT INTO worldstream_activation_decisions(room_id, cause_room_seq, decision_id, target_member_id, decision_bytes) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (room_id, cause_room_seq, decision_id) DO NOTHING",
            &[&room_id, &cause_room_seq, &decision_id, &target_member_id, &decision],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT target_member_id, decision_bytes FROM worldstream_activation_decisions WHERE room_id = $1 AND cause_room_seq = $2 AND decision_id = $3",
            &[&room_id, &cause_room_seq, &decision_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if stored
        .try_get::<_, Option<String>>(0)
        .map_err(PostgresTransferError::Sql)?
        != target_member_id
        || stored
            .try_get::<_, Vec<u8>>(1)
            .map_err(PostgresTransferError::Sql)?
            != decision
    {
        return Err(PostgresTransferError::Canonical(
            "activation decision mismatch",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn publish_activation_intent(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let activation_id = native_text(row, 0)?;
    let room_id = native_text(row, 1)?;
    let cause_room_seq = native_integer(row, 2)?;
    let decision_id = native_text(row, 3)?;
    let target_member_id = native_text(row, 4)?;
    let reason_code = native_text(row, 5)?;
    let deduplication_key = native_text(row, 6)?;
    let priority = native_integer(row, 7)?;
    let semantic_deadline = native_optional_text(row, 8)?;
    let policy_revision = native_integer(row, 9)?;
    let state = native_text(row, 10)?;
    let intent_generation = native_integer(row, 11)?;
    let lease_generation = native_integer(row, 12)?;
    let runner_id = native_optional_text(row, 13)?;
    let claim_id = native_optional_text(row, 14)?;
    let lease_until = native_optional_text(row, 15)?;
    let context_hash = native_optional_blob(row, 16)?;
    let context_bytes = native_optional_blob(row, 17)?;
    let context_retired = native_integer(row, 18)? != 0;
    transaction
        .execute(
            "INSERT INTO worldstream_activation_intents(activation_id, room_id, cause_room_seq, decision_id, target_member_id, reason_code, deduplication_key, priority, semantic_deadline, policy_revision, state, intent_generation, lease_generation, runner_id, claim_id, lease_until, context_hash, context_bytes, context_retired) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19) ON CONFLICT (activation_id) DO NOTHING",
            &[&activation_id, &room_id, &cause_room_seq, &decision_id, &target_member_id, &reason_code, &deduplication_key, &priority, &semantic_deadline, &policy_revision, &state, &intent_generation, &lease_generation, &runner_id, &claim_id, &lease_until, &context_hash, &context_bytes, &context_retired],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT room_id, cause_room_seq, decision_id, target_member_id, reason_code, deduplication_key, priority, semantic_deadline, policy_revision, state, intent_generation, lease_generation, runner_id, claim_id, lease_until, context_hash, context_bytes, context_retired FROM worldstream_activation_intents WHERE activation_id = $1",
            &[&activation_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    let actual = (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(4)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(5)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(6)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(7)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(8)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(9)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(10)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, i64>(11)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(12)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(13)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(14)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<Vec<u8>>>(15)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<Vec<u8>>>(16)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, bool>(17)
            .map_err(PostgresTransferError::Sql)?,
    );
    if actual.0 != room_id
        || actual.1 != cause_room_seq
        || actual.2 != decision_id
        || actual.3 != target_member_id
        || actual.4 != reason_code
        || actual.5 != deduplication_key
        || actual.6 != priority
        || actual.7 != semantic_deadline
        || actual.8 != policy_revision
        || actual.9 != state
        || actual.10 != intent_generation
        || actual.11 != lease_generation
        || actual.12 != runner_id
        || actual.13 != claim_id
        || actual.14 != lease_until
        || actual.15 != context_hash
        || actual.16 != context_bytes
        || actual.17 != context_retired
    {
        return Err(PostgresTransferError::Canonical(
            "activation intent mismatch",
        ));
    }
    Ok(())
}

fn publish_activation_receipt(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let operation_id = native_text(row, 1)?;
    let operation_kind = native_text(row, 2)?;
    let request_hash = native_blob(row, 3)?;
    let activation_id = native_optional_text(row, 4)?;
    let result_code = native_text(row, 5)?;
    let result_bytes = native_blob(row, 6)?;
    let context_hash = native_optional_blob(row, 7)?;
    let context_bytes = native_optional_blob(row, 8)?;
    transaction
        .execute(
            "INSERT INTO worldstream_activation_operation_receipts(room_id, operation_id, operation_kind, canonical_request_hash, activation_id, result_code, result_bytes, context_hash, context_bytes) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (room_id, operation_id) DO NOTHING",
            &[&room_id, &operation_id, &operation_kind, &request_hash, &activation_id, &result_code, &result_bytes, &context_hash, &context_bytes],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT operation_kind, canonical_request_hash, activation_id, result_code, result_bytes, context_hash, context_bytes FROM worldstream_activation_operation_receipts WHERE room_id = $1 AND operation_id = $2",
            &[&room_id, &operation_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(4)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<Vec<u8>>>(5)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<Vec<u8>>>(6)
            .map_err(PostgresTransferError::Sql)?,
    ) != (
        operation_kind,
        request_hash,
        activation_id,
        result_code,
        result_bytes,
        context_hash,
        context_bytes,
    ) {
        return Err(PostgresTransferError::Canonical(
            "activation receipt mismatch",
        ));
    }
    Ok(())
}

fn publish_semantic_receipt(
    transaction: &mut Transaction<'_>,
    row: &NativeRow,
    mode: NativePublicationMode,
) -> Result<(), PostgresTransferError> {
    let room_id = native_text(row, 0)?;
    let operation_kind = native_text(row, 1)?;
    let identity_bytes = native_blob(row, 2)?;
    let _codec_id = native_text(row, 3)?;
    let request_hash = native_blob(row, 4)?;
    let basis_head = native_optional_blob(row, 5)?;
    let semantic_input = native_blob(row, 6)?;
    let semantic_time = native_blob(row, 7)?;
    let resolution_kind = native_text(row, 8)?;
    let transition_seq = match row.values.get(9) {
        Some(NativeValue::Integer(value)) => Some(*value),
        Some(NativeValue::Null) => None,
        _ => {
            return Err(PostgresTransferError::Canonical(
                "native receipt transition",
            ));
        }
    };
    let receipt = native_blob(row, 10)?;
    let _committed_at = native_text(row, 11)?;
    transaction
        .execute(
            "INSERT INTO worldstream_semantic_receipts(identity_bytes, operation_kind, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, receipt_bytes, room_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (identity_bytes) DO NOTHING",
            &[&identity_bytes, &operation_kind, &request_hash, &basis_head, &semantic_input, &semantic_time, &resolution_kind, &transition_seq, &receipt, &room_id],
        )
        .map_err(PostgresTransferError::Sql)?;
    let stored = transaction
        .query_one(
            "SELECT operation_kind, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, receipt_bytes, room_id FROM worldstream_semantic_receipts WHERE identity_bytes = $1",
            &[&identity_bytes],
        )
        .map_err(PostgresTransferError::Sql)?;
    if (
        stored
            .try_get::<_, String>(0)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(1)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<Vec<u8>>>(2)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(3)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(4)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, String>(5)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<i64>>(6)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Vec<u8>>(7)
            .map_err(PostgresTransferError::Sql)?,
        stored
            .try_get::<_, Option<String>>(8)
            .map_err(PostgresTransferError::Sql)?,
    ) != (
        operation_kind.clone(),
        request_hash.clone(),
        basis_head.clone(),
        semantic_input.clone(),
        semantic_time.clone(),
        resolution_kind.clone(),
        transition_seq,
        receipt.clone(),
        Some(room_id.clone()),
    ) {
        return Err(PostgresTransferError::Canonical(
            "semantic receipt mismatch",
        ));
    }
    publish_or_verify_semantic_receipt_guard(
        transaction,
        mode,
        &identity_bytes,
        &request_hash,
        &room_id,
        &receipt,
    )
}

// SQLite serializes exactly-once operation state in `semantic_receipts`,
// while PostgreSQL resolves and serializes retries through the paired
// operation-guard row. Hydration must therefore derive the guard from the
// exact immutable receipt in this same transaction. Verify-only replay is
// deliberately read-only for this relation: a missing guard is corruption,
// not something verification may repair.
fn publish_or_verify_semantic_receipt_guard(
    transaction: &mut Transaction<'_>,
    mode: NativePublicationMode,
    identity_bytes: &[u8],
    request_hash: &[u8],
    room_id: &str,
    receipt: &[u8],
) -> Result<(), PostgresTransferError> {
    if mode == NativePublicationMode::Hydrate {
        transaction
            .execute(
                "INSERT INTO worldstream_operation_guards(identity_bytes, request_hash, room_id, receipt_bytes) VALUES ($1, $2, $3, $4) ON CONFLICT (identity_bytes) DO NOTHING",
                &[&identity_bytes, &request_hash, &room_id, &receipt],
            )
            .map_err(PostgresTransferError::Sql)?;
    }
    let guard = transaction
        .query_opt(
            "SELECT request_hash, room_id, receipt_bytes FROM worldstream_operation_guards WHERE identity_bytes = $1 FOR SHARE",
            &[&identity_bytes],
        )
        .map_err(PostgresTransferError::Sql)?
        .ok_or(PostgresTransferError::Canonical(
            "semantic receipt operation guard missing",
        ))?;
    let stored_request_hash = guard
        .try_get::<_, Vec<u8>>(0)
        .map_err(PostgresTransferError::Sql)?;
    let stored_room_id = guard
        .try_get::<_, Option<String>>(1)
        .map_err(PostgresTransferError::Sql)?;
    let stored_receipt = guard
        .try_get::<_, Option<Vec<u8>>>(2)
        .map_err(PostgresTransferError::Sql)?;
    if !semantic_receipt_guard_matches(
        &stored_request_hash,
        stored_room_id.as_deref(),
        stored_receipt.as_deref(),
        request_hash,
        room_id,
        receipt,
    ) {
        return Err(PostgresTransferError::Canonical(
            "semantic receipt operation guard mismatch",
        ));
    }
    Ok(())
}

fn semantic_receipt_guard_matches(
    stored_request_hash: &[u8],
    stored_room_id: Option<&str>,
    stored_receipt: Option<&[u8]>,
    expected_request_hash: &[u8],
    expected_room_id: &str,
    expected_receipt: &[u8],
) -> bool {
    stored_request_hash == expected_request_hash
        && stored_room_id == Some(expected_room_id)
        && stored_receipt == Some(expected_receipt)
}

struct NativeReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> NativeReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], PostgresTransferError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(PostgresTransferError::Canonical("native row length"))?;
        if end > self.bytes.len() {
            return Err(PostgresTransferError::Canonical("native row truncation"));
        }
        let value = &self.bytes[self.position..end];
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, PostgresTransferError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PostgresTransferError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(
            |_| PostgresTransferError::Canonical("native row integer"),
        )?))
    }

    fn u32(&mut self) -> Result<u32, PostgresTransferError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(
            |_| PostgresTransferError::Canonical("native row integer"),
        )?))
    }

    fn i64(&mut self) -> Result<i64, PostgresTransferError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().map_err(
            |_| PostgresTransferError::Canonical("native row integer"),
        )?))
    }

    fn string(&mut self) -> Result<String, PostgresTransferError> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| PostgresTransferError::Canonical("native row string length"))?;
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| PostgresTransferError::Canonical("native row UTF-8"))
    }

    fn blob(&mut self) -> Result<Vec<u8>, PostgresTransferError> {
        let length =
            usize::try_from(u64::from_le_bytes(self.take(8)?.try_into().map_err(
                |_| PostgresTransferError::Canonical("native row blob length"),
            )?))
            .map_err(|_| PostgresTransferError::Canonical("native row blob length"))?;
        Ok(self.take(length)?.to_vec())
    }

    const fn done(&self) -> bool {
        self.position == self.bytes.len()
    }
}

impl TransferDestinationV1 for PostgresTransferDestination<'_> {
    type Error = PostgresTransferError;

    fn apply_chunk(
        &mut self,
        chunk: &TransferChunkV1,
    ) -> Result<TransferChunkDispositionV1, Self::Error> {
        if chunk.bundle_hash() != self.bundle_hash {
            return Err(PostgresTransferError::TargetMismatch("bundle hash"));
        }
        let records_bytes = chunk.canonical_bytes()?;
        let start = Self::checked_i64(chunk.start(), "chunk start")?;
        let end = Self::checked_i64(chunk.end(), "chunk end")?;
        let bundle_hash = self.bundle_key();
        let mut client = self
            .admin
            .connect()
            .map_err(PostgresTransferError::Connection)?;
        let mut transaction = client.transaction().map_err(PostgresTransferError::Sql)?;
        let state = self.ensure_import(&mut transaction)?;
        if !Self::chunk_state_accepts_chunk(&state) {
            return Err(PostgresTransferError::InvalidImportState {
                expected: "pending",
                actual: state,
            });
        }
        if let Some(row) = transaction
            .query_opt(
                "SELECT chunk_end, chunk_digest, records_bytes FROM worldstream_transfer_chunks WHERE bundle_hash = $1 AND chunk_start = $2 FOR UPDATE",
                &[&bundle_hash.as_slice(), &start],
            )
            .map_err(PostgresTransferError::Sql)?
        {
            let stored_end: i64 = row.try_get(0).map_err(PostgresTransferError::Sql)?;
            let stored_digest: Vec<u8> = row.try_get(1).map_err(PostgresTransferError::Sql)?;
            let stored_bytes: Vec<u8> = row.try_get(2).map_err(PostgresTransferError::Sql)?;
            if stored_end == end
                && stored_digest == chunk.digest().as_bytes()
                && stored_bytes == records_bytes
            {
                Self::commit_transaction(transaction)?;
                return Ok(TransferChunkDispositionV1::AlreadyApplied);
            }
            return Err(PostgresTransferError::ChunkConflict);
        }
        if transaction
            .query_opt(
                "SELECT 1 FROM worldstream_transfer_chunks WHERE bundle_hash = $1 AND chunk_start < $2 AND chunk_end > $3 FOR UPDATE",
                &[&bundle_hash.as_slice(), &end, &start],
            )
            .map_err(PostgresTransferError::Sql)?
            .is_some()
        {
            return Err(PostgresTransferError::OverlappingChunk);
        }
        transaction
            .execute(
                "INSERT INTO worldstream_transfer_chunks(bundle_hash, chunk_start, chunk_end, chunk_digest, records_bytes) VALUES ($1, $2, $3, $4, $5)",
                &[
                    &bundle_hash.as_slice(),
                    &start,
                    &end,
                    &chunk.digest().as_bytes().as_slice(),
                    &records_bytes,
                ],
            )
            .map_err(PostgresTransferError::Sql)?;
        Self::commit_transaction(transaction)?;
        Ok(TransferChunkDispositionV1::Applied)
    }

    fn verify_complete(
        &mut self,
        bundle: &TransferBundleV1,
        target: &TargetFingerprintV1,
    ) -> Result<(), Self::Error> {
        bundle.verify_target(target)?;
        if target != &self.target {
            return Err(PostgresTransferError::TargetMismatch(
                "bound target fingerprint",
            ));
        }
        self.admin
            .verify_schema()
            .map_err(PostgresTransferError::Schema)?;
        let native_summary = Self::verify_native_semantic_evidence(bundle)?;
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let mut client = self
            .admin
            .connect()
            .map_err(PostgresTransferError::Connection)?;
        let mut transaction = client.transaction().map_err(PostgresTransferError::Sql)?;
        let row = transaction
            .query_opt(
                "SELECT target_fingerprint, state FROM worldstream_transfer_imports WHERE bundle_hash = $1 FOR UPDATE",
                &[&bundle_hash.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?
            .ok_or(PostgresTransferError::MissingImport)?;
        let stored_target: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        if stored_target != target_digest {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        let state: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        if state != "pending"
            && state != "verified"
            && !matches!(state.as_str(), "finalized" | "authoritative")
        {
            return Err(PostgresTransferError::InvalidImportState {
                expected: "pending or verified",
                actual: state,
            });
        }
        if state != "authoritative" {
            self.verify_target_fence(&mut transaction)?;
        }
        self.verify_staged_chunks(&mut transaction, bundle)?;
        match state.as_str() {
            "pending" => {
                // Hydration, executable replay, the exact importing fence,
                // and this provider-derived verified marker commit together.
                // Source retirement is not attempted by the coordinator until
                // this method and record_finalization both return successfully.
                self.hydrate_and_verify_behind_fence(&mut transaction, native_summary.as_ref())?;
                let updated = transaction
                    .execute(
                        "UPDATE worldstream_transfer_imports SET state = 'verified' WHERE bundle_hash = $1 AND target_fingerprint = $2 AND state = 'pending'",
                        &[&bundle_hash.as_slice(), &target_digest.as_slice()],
                    )
                    .map_err(PostgresTransferError::Sql)?;
                if updated != 1 {
                    return Err(PostgresTransferError::InvalidImportState {
                        expected: "pending",
                        actual: state,
                    });
                }
            }
            // Never trust the label alone: older binaries could persist these
            // states before hydration. Exact operational-row verification and
            // full replay turn either label into current provider evidence.
            "verified" | "finalized" => self.reconfirm_hydrated_target_behind_fence(
                &mut transaction,
                native_summary.as_ref(),
            )?,
            "authoritative" => {
                self.verify_native_operational_rows(&mut transaction)?;
                self.verify_hydrated_target(&mut transaction, native_summary.as_ref())?;
            }
            actual => {
                return Err(PostgresTransferError::InvalidImportState {
                    expected: "pending or verified",
                    actual: actual.to_owned(),
                });
            }
        }
        Self::commit_transaction(transaction)
    }

    fn record_finalization(&mut self, target: &TargetFingerprintV1) -> Result<(), Self::Error> {
        if target != &self.target {
            return Err(PostgresTransferError::TargetMismatch(
                "finalization target fingerprint",
            ));
        }
        let native_summary = Self::verify_native_semantic_evidence(&self.bundle)?;
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let mut client = self
            .admin
            .connect()
            .map_err(PostgresTransferError::Connection)?;
        let mut transaction = client.transaction().map_err(PostgresTransferError::Sql)?;
        let row = transaction
            .query_opt(
                "SELECT target_fingerprint, state FROM worldstream_transfer_imports WHERE bundle_hash = $1 FOR UPDATE",
                &[&bundle_hash.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?
            .ok_or(PostgresTransferError::MissingImport)?;
        let stored_target: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        if stored_target != target_digest {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        let state: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        if state != "authoritative" {
            self.verify_target_fence(&mut transaction)?;
        }
        match state.as_str() {
            "verified" => {
                self.verify_staged_chunks(&mut transaction, &self.bundle)?;
                self.reconfirm_hydrated_target_behind_fence(
                    &mut transaction,
                    native_summary.as_ref(),
                )?;
                let updated = transaction
                    .execute(
                        "UPDATE worldstream_transfer_imports SET state = 'finalized' WHERE bundle_hash = $1 AND target_fingerprint = $2 AND state = 'verified'",
                        &[&bundle_hash.as_slice(), &target_digest.as_slice()],
                    )
                    .map_err(PostgresTransferError::Sql)?;
                if updated != 1 {
                    return Err(PostgresTransferError::InvalidImportState {
                        expected: "verified",
                        actual: state,
                    });
                }
            }
            "finalized" => {
                self.verify_staged_chunks(&mut transaction, &self.bundle)?;
                self.reconfirm_hydrated_target_behind_fence(
                    &mut transaction,
                    native_summary.as_ref(),
                )?;
            }
            "authoritative" => {
                self.verify_native_operational_rows(&mut transaction)?;
                self.verify_hydrated_target(&mut transaction, native_summary.as_ref())?;
            }
            _ => {
                return Err(PostgresTransferError::InvalidImportState {
                    expected: "verified",
                    actual: state,
                });
            }
        }
        Self::commit_transaction(transaction)
    }

    fn reconcile_finalization_after_definite_source_failure(
        &mut self,
        target: &TargetFingerprintV1,
    ) -> Result<(), Self::Error> {
        if target != &self.target {
            return Err(PostgresTransferError::TargetMismatch(
                "finalization reconciliation target fingerprint",
            ));
        }
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let mut client = self
            .admin
            .connect()
            .map_err(PostgresTransferError::Connection)?;
        let mut transaction = client.transaction().map_err(PostgresTransferError::Sql)?;
        // The serialized session is only a retry hint: it can still say
        // `TargetVerified` when the provider's finalization transaction won
        // immediately before a process stop. Serialize with first publication
        // and reconcile every exact non-serving provider disposition.
        Self::lock_target_preflight(&mut transaction)?;
        let fence_state = self.lock_abort_fence(&mut transaction)?;
        if fence_state == AbortFenceStateV1::AlreadyAborted {
            Self::verify_aborted_import_empty(&mut transaction)?;
            Self::commit_transaction(transaction)?;
            return Ok(());
        }
        let Some(row) = transaction
            .query_opt(
                "SELECT target_fingerprint, state FROM worldstream_transfer_imports WHERE bundle_hash = $1 FOR UPDATE",
                &[&bundle_hash.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?
        else {
            if fence_state == AbortFenceStateV1::Importing {
                return Err(PostgresTransferError::MissingImport);
            }
            Self::commit_transaction(transaction)?;
            return Ok(());
        };
        if fence_state == AbortFenceStateV1::Missing {
            return Err(PostgresTransferError::MissingTargetFence);
        }
        let stored_target: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        if stored_target != target_digest {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        let state: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        match state.as_str() {
            "finalized" => {
                self.verify_target_fence(&mut transaction)?;
                let updated = transaction
                    .execute(
                        "UPDATE worldstream_transfer_imports SET state = 'verified' WHERE bundle_hash = $1 AND target_fingerprint = $2 AND state = 'finalized'",
                        &[&bundle_hash.as_slice(), &target_digest.as_slice()],
                    )
                    .map_err(PostgresTransferError::Sql)?;
                if updated != 1 {
                    return Err(PostgresTransferError::InvalidImportState {
                        expected: "finalized",
                        actual: state,
                    });
                }
            }
            "pending" | "verified" => self.verify_target_fence(&mut transaction)?,
            "authoritative" => return Err(PostgresTransferError::RollbackRefused),
            actual => {
                return Err(PostgresTransferError::InvalidImportState {
                    expected: "pending, verified, or finalized",
                    actual: actual.to_owned(),
                });
            }
        }
        Self::commit_transaction(transaction)
    }

    fn accept_target_write(&mut self, target: &TargetFingerprintV1) -> Result<(), Self::Error> {
        if target != &self.target {
            return Err(PostgresTransferError::TargetMismatch(
                "authority target fingerprint",
            ));
        }
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let mut client = self
            .admin
            .connect()
            .map_err(PostgresTransferError::Connection)?;
        let mut transaction = client.transaction().map_err(PostgresTransferError::Sql)?;
        let row = transaction
            .query_opt(
                "SELECT target_fingerprint, state FROM worldstream_transfer_imports WHERE bundle_hash = $1 FOR UPDATE",
                &[&bundle_hash.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?
            .ok_or(PostgresTransferError::MissingImport)?;
        let stored_target: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        if stored_target != target_digest {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        let state: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        match state.as_str() {
            "finalized" => {
                self.verify_target_fence(&mut transaction)?;
                // Hydration and full replay were committed with the exact
                // verified marker before source retirement. The importing
                // fence made that state immutable to runtime writes, so the
                // post-retirement operation only publishes that authority by
                // atomically removing the fence and advancing the marker.
                let removed = transaction
                    .execute(
                        "DELETE FROM worldstream_transfer_target_fence WHERE fence_id = true AND bundle_hash = $1 AND target_fingerprint = $2 AND state = 'importing'",
                        &[&bundle_hash.as_slice(), &target_digest.as_slice()],
                    )
                    .map_err(PostgresTransferError::Sql)?;
                if removed != 1 {
                    return Err(PostgresTransferError::MissingTargetFence);
                }
                let updated = transaction
                    .execute(
                        "UPDATE worldstream_transfer_imports SET state = 'authoritative' WHERE bundle_hash = $1 AND target_fingerprint = $2 AND state = 'finalized'",
                        &[&bundle_hash.as_slice(), &target_digest.as_slice()],
                    )
                    .map_err(PostgresTransferError::Sql)?;
                if updated != 1 {
                    return Err(PostgresTransferError::InvalidImportState {
                        expected: "finalized",
                        actual: state,
                    });
                }
            }
            "authoritative" => {
                if transaction
                    .query_opt(
                        "SELECT 1 FROM worldstream_transfer_target_fence WHERE fence_id = true FOR UPDATE",
                        &[],
                    )
                    .map_err(PostgresTransferError::Sql)?
                    .is_some()
                {
                    return Err(PostgresTransferError::TargetMismatch(
                        "authoritative target retains a transfer fence",
                    ));
                }
            }
            actual => {
                return Err(PostgresTransferError::InvalidImportState {
                    expected: "finalized",
                    actual: actual.to_owned(),
                });
            }
        }
        Self::commit_transaction(transaction)
    }

    fn abort_import(&mut self, target: &TargetFingerprintV1) -> Result<(), Self::Error> {
        if target != &self.target {
            return Err(PostgresTransferError::TargetMismatch(
                "abort target fingerprint",
            ));
        }
        let bundle_hash = self.bundle_key();
        let target_digest = self.target_key();
        let mut client = self
            .admin
            .connect()
            .map_err(PostgresTransferError::Connection)?;
        let mut transaction = client.transaction().map_err(PostgresTransferError::Sql)?;
        // Serialize the missing-state preflight and tombstone write with the
        // first chunk publisher. Whichever transaction wins leaves one exact
        // provider state for the loser to validate rather than permitting a
        // restarted publisher to race a source-authority restoration.
        Self::lock_target_preflight(&mut transaction)?;
        let fence_state = self.lock_abort_fence(&mut transaction)?;
        if fence_state == AbortFenceStateV1::AlreadyAborted {
            Self::verify_aborted_import_empty(&mut transaction)?;
            Self::commit_transaction(transaction)?;
            return Ok(());
        }
        let Some(row) = transaction
            .query_opt(
                "SELECT target_fingerprint, state FROM worldstream_transfer_imports WHERE bundle_hash = $1 FOR UPDATE",
                &[&bundle_hash.as_slice()],
            )
            .map_err(PostgresTransferError::Sql)?
        else {
            if fence_state == AbortFenceStateV1::Importing {
                return Err(PostgresTransferError::MissingImport);
            }
            // Even an abort that arrives before the first chunk must leave a
            // durable, exact tombstone. The same empty-target locks used by
            // first publication prove that this target has no serving truth,
            // and keep the check plus tombstone in one provider transaction.
            Self::preflight_empty_target(&mut transaction)?;
            let inserted = transaction
                .execute(
                    "INSERT INTO worldstream_transfer_target_fence(fence_id, bundle_hash, target_fingerprint, state) VALUES (true, $1, $2, 'aborted')",
                    &[&bundle_hash.as_slice(), &target_digest.as_slice()],
                )
                .map_err(PostgresTransferError::Sql)?;
            if inserted != 1
                || self.lock_abort_fence(&mut transaction)?
                    != AbortFenceStateV1::AlreadyAborted
            {
                return Err(PostgresTransferError::MissingTargetFence);
            }
            Self::verify_aborted_import_empty(&mut transaction)?;
            Self::commit_transaction(transaction)?;
            return Ok(());
        };
        if fence_state == AbortFenceStateV1::Missing {
            return Err(PostgresTransferError::MissingTargetFence);
        }
        let stored_target: Vec<u8> = row.try_get(0).map_err(PostgresTransferError::Sql)?;
        if stored_target != target_digest {
            return Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ));
        }
        let state: String = row.try_get(1).map_err(PostgresTransferError::Sql)?;
        if matches!(state.as_str(), "finalized" | "authoritative") {
            return Err(PostgresTransferError::RollbackRefused);
        }
        if state != "pending" && state != "verified" {
            return Err(PostgresTransferError::InvalidImportState {
                expected: "pending or verified",
                actual: state,
            });
        }
        self.discard_import_behind_fence(&mut transaction)?;
        Self::commit_transaction(transaction)
    }
}

fn require_native_canonical_evidence(
    records: &[worldstream_transfer::LogicalRecordV1],
    room_ids: &[&str],
) -> Result<(), NativeSqliteTransferError> {
    let mut deployment_lineage = 0_usize;
    let mut storage_epoch = 0_usize;
    let mut room_kinds = BTreeMap::<&str, BTreeSet<CanonicalRecordKindV1>>::new();

    for record in records {
        let RecordKindV1::Canonical(kind) = record.kind() else {
            continue;
        };
        match kind {
            CanonicalRecordKindV1::DeploymentLineage => deployment_lineage += 1,
            CanonicalRecordKindV1::StorageEpoch => storage_epoch += 1,
            CanonicalRecordKindV1::RoomGenesis
            | CanonicalRecordKindV1::RoomHead
            | CanonicalRecordKindV1::CoreMaterialization
            | CanonicalRecordKindV1::ActivityMaterialization => {
                let room_id = record
                    .identity()
                    .strip_prefix("room/")
                    .and_then(|identity| identity.split('/').next())
                    .filter(|room_id| !room_id.is_empty())
                    .ok_or(NativeSqliteTransferError::CanonicalEvidence {
                        what: "canonical Room identity",
                    })?;
                room_kinds.entry(room_id).or_default().insert(kind);
            }
            _ => {}
        }
    }

    if deployment_lineage != 1 {
        return Err(NativeSqliteTransferError::CanonicalEvidence {
            what: "deployment lineage cardinality",
        });
    }
    if storage_epoch != 1 {
        return Err(NativeSqliteTransferError::CanonicalEvidence {
            what: "storage epoch cardinality",
        });
    }
    for room_id in room_ids {
        let kinds = room_kinds.get(room_id);
        for kind in [
            CanonicalRecordKindV1::RoomGenesis,
            CanonicalRecordKindV1::RoomHead,
            CanonicalRecordKindV1::CoreMaterialization,
            CanonicalRecordKindV1::ActivityMaterialization,
        ] {
            if !kinds.is_some_and(|kinds| kinds.contains(&kind)) {
                return Err(NativeSqliteTransferError::MissingCanonicalRecord { kind });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldstream_transfer::{
        BackendFingerprintV1, CanonicalRecordKindV1, DeploymentIdentityV1, LogicalRecordV1,
        PackIdentityV1, ResourceIdentityV1, ResourceKindV1, SessionStatePolicyV1,
    };

    fn fixture_bundle() -> Result<TransferBundleV1, PostgresTransferError> {
        let target_backend = postgres_backend_fingerprint()?;
        let source_backend = BackendFingerprintV1::new(
            BundleProfileV1::SqliteBundled,
            "sqlite-bundled",
            target_backend.schema().clone(),
        )?;
        let pack = PackIdentityV1::new("transfer-fixture", "r1", DigestV1::hash(b"pack"))?;
        let resource = ResourceIdentityV1::from_bytes(
            ResourceKindV1::Artifact,
            "artifact/fixture",
            b"artifact",
        )?;
        let deployment_identity =
            DeploymentIdentityV1::new(vec![pack.clone()], vec![resource.clone()])?;
        let deployment_identity_bytes = deployment_identity.canonical_bytes()?;
        let record = LogicalRecordV1::canonical(
            0,
            CanonicalRecordKindV1::DeploymentLineage,
            "deployment/fixture",
            b"lineage\0bytes",
        )?;
        let identity_record = LogicalRecordV1::canonical(
            1,
            CanonicalRecordKindV1::ArtifactMetadata,
            "deployment/identity",
            &deployment_identity_bytes,
        )?;
        Ok(TransferBundleV1::new_with_backend_fingerprints(
            "bundle/fixture",
            "lineage/fixture",
            1,
            source_backend,
            target_backend,
            pack,
            vec![resource],
            SessionStatePolicyV1::InvalidateAndRebuild,
            vec![record, identity_record],
        )?)
    }

    fn fixture_admin() -> Result<PostgresAdmin, crate::PostgresConfigError> {
        PostgresAdmin::new(crate::PostgresConnectionConfig::direct_admin(
            "host=localhost user=admin sslmode=require",
        )?)
    }

    #[test]
    fn reviewed_backend_fingerprint_includes_transfer_migration()
    -> Result<(), Box<dyn std::error::Error>> {
        let fingerprint = postgres_backend_fingerprint()?;
        assert_eq!(fingerprint.profile(), BundleProfileV1::PostgresPrimary17);
        assert_eq!(fingerprint.schema().migrations().len(), 11);
        assert_eq!(fingerprint.schema().migrations()[5].version(), 6);
        assert_eq!(fingerprint.schema().migrations()[10].version(), 11);
        Ok(())
    }

    #[test]
    fn target_empty_preflight_rejects_exact_and_conflicting_preseed_domains() {
        let empty = [
            ("worldstream_room_roots".to_owned(), 0),
            ("worldstream_deployment_resource_blobs".to_owned(), 0),
            ("worldstream_transfer_target_fence".to_owned(), 0),
        ];
        assert!(PostgresTransferDestination::validate_empty_domain_counts(&empty, 1, 0).is_ok());
        let aborted = [
            ("worldstream_room_roots".to_owned(), 0),
            ("worldstream_deployment_resource_blobs".to_owned(), 0),
            ("worldstream_transfer_target_fence".to_owned(), 1),
        ];
        assert!(PostgresTransferDestination::validate_empty_domain_counts(&aborted, 1, 1).is_ok());
        for domain in [
            "worldstream_room_roots",
            "worldstream_deployment_resource_blobs",
        ] {
            let counts = [(domain.to_owned(), 1)];
            assert!(matches!(
                PostgresTransferDestination::validate_empty_domain_counts(&counts, 1, 0),
                Err(PostgresTransferError::TargetNotEmpty { domain: found }) if found == domain
            ));
        }
        assert!(matches!(
            PostgresTransferDestination::validate_empty_domain_counts(&empty, 0, 0),
            Err(PostgresTransferError::TargetNotEmpty { domain })
                if domain == "worldstream_authority_state"
        ));
        assert!(matches!(
            PostgresTransferDestination::validate_empty_domain_counts(&empty, 2, 0),
            Err(PostgresTransferError::TargetNotEmpty { domain })
                if domain == "worldstream_authority_state"
        ));
        assert!(matches!(
            PostgresTransferDestination::validate_empty_domain_counts(&aborted, 1, 0),
            Err(PostgresTransferError::TargetNotEmpty { domain })
                if domain == "worldstream_transfer_target_fence"
        ));
    }

    #[test]
    fn aborted_target_discard_covers_every_user_truth_domain() {
        let discarded_domains = [
            "worldstream_operation_guards",
            "worldstream_room_roots",
            "worldstream_genesis",
            "worldstream_materializations",
            "worldstream_members",
            "worldstream_timers",
            "worldstream_transitions",
            "worldstream_frames",
            "worldstream_observation_consequences",
            "worldstream_activation_decisions",
            "worldstream_activation_intents",
            "worldstream_activation_operation_receipts",
            "worldstream_room_snapshots",
            "worldstream_semantic_receipts",
            "worldstream_integrity_incidents",
            "worldstream_authority_fences",
            "worldstream_authority_state",
            "worldstream_authority_principals",
            "worldstream_authority_runners",
            "worldstream_authority_capabilities",
            "worldstream_authority_capability_scopes",
            "worldstream_authority_runner_capability_memberships",
            "worldstream_authority_change_receipts",
            "worldstream_authority_audit",
            "worldstream_transfer_chunks",
            "worldstream_transfer_imports",
            "worldstream_deployment_metadata",
            "worldstream_deployment_identity_metadata",
            "worldstream_deployment_pack_identities",
            "worldstream_deployment_resource_blobs",
            "worldstream_deployment_resource_identities",
            "worldstream_retired_authority_fences_v1",
        ];

        let mut expected_schema_domains =
            discarded_domains.iter().copied().collect::<BTreeSet<_>>();
        expected_schema_domains.insert("worldstream_schema_migrations");
        expected_schema_domains.insert("worldstream_transfer_target_fence");
        let schema_domains = crate::migrations::SCHEMA_FINGERPRINT_MATERIAL
            .split(");")
            .filter_map(|domain| domain.split_once('(').map(|(name, _)| name))
            .collect::<BTreeSet<_>>();
        assert_eq!(schema_domains, expected_schema_domains);

        for domain in discarded_domains {
            if domain == "worldstream_authority_state" {
                assert!(
                    AUTHORITY_STATE_COUNT_SQL.contains(domain),
                    "the authority singleton proof omitted {domain}"
                );
            } else {
                assert!(
                    TARGET_EMPTY_DOMAIN_COUNTS_SQL.contains(domain),
                    "the target emptiness proof omitted {domain}"
                );
            }
            assert!(
                TARGET_PREFLIGHT_LOCK_SQL.contains(domain),
                "the target transaction lock omitted {domain}"
            );
            assert!(
                DISCARD_TRANSFER_TARGET_SQL.contains(domain),
                "the aborted-target discard omitted {domain}"
            );
        }
        assert!(TARGET_EMPTY_DOMAIN_COUNTS_SQL.contains("worldstream_transfer_target_fence"));
        assert!(!DISCARD_TRANSFER_TARGET_SQL.contains("worldstream_transfer_target_fence"));
        assert!(!DISCARD_TRANSFER_TARGET_SQL.contains("worldstream_schema_migrations"));
        assert!(
            DISCARD_TRANSFER_TARGET_SQL
                .contains("INSERT INTO worldstream_authority_state(authority_id) VALUES (true)")
        );
    }

    #[test]
    fn destination_binds_healthy_bundle_and_rejects_target_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let admin = fixture_admin()?;
        let destination = PostgresTransferDestination::new(&admin, &bundle, target.clone())?;
        assert_eq!(destination.target(), &target);

        let wrong_pack = PackIdentityV1::new("transfer-fixture", "r2", DigestV1::hash(b"pack"))?;
        let wrong_target = TargetFingerprintV1::with_pack(&target, wrong_pack)?;
        assert!(matches!(
            PostgresTransferDestination::new(&admin, &bundle, wrong_target),
            Err(PostgresTransferError::Contract(
                TransferError::TargetMismatch { .. }
            ))
        ));
        Ok(())
    }

    #[test]
    fn only_pending_imports_accept_more_chunks() {
        assert!(PostgresTransferDestination::chunk_state_accepts_chunk(
            "pending"
        ));
        assert!(!PostgresTransferDestination::chunk_state_accepts_chunk(
            "verified"
        ));
        assert!(!PostgresTransferDestination::chunk_state_accepts_chunk(
            "finalized"
        ));
        assert!(!PostgresTransferDestination::chunk_state_accepts_chunk(
            "authoritative"
        ));
    }

    #[test]
    fn target_fence_rejects_missing_and_mismatched_identity() -> Result<(), TransferError> {
        let bundle = fixture_bundle().map_err(|_| TransferError::InvalidValue {
            what: "fixture bundle",
        })?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let bundle_hash = bundle.bundle_hash()?;
        let target_hash = target.fingerprint_digest()?;
        assert!(matches!(
            PostgresTransferDestination::verify_target_fence_bytes(bundle_hash, target_hash, None),
            Err(PostgresTransferError::MissingTargetFence)
        ));
        let wrong_bundle = [0_u8; 32];
        assert!(matches!(
            PostgresTransferDestination::verify_target_fence_bytes(
                bundle_hash,
                target_hash,
                Some((&wrong_bundle, &target_hash.as_bytes())),
            ),
            Err(PostgresTransferError::TargetMismatch(
                "persisted target bundle fence",
            ))
        ));
        let wrong_target = [1_u8; 32];
        assert!(matches!(
            PostgresTransferDestination::verify_target_fence_bytes(
                bundle_hash,
                target_hash,
                Some((&bundle_hash.as_bytes(), &wrong_target)),
            ),
            Err(PostgresTransferError::TargetMismatch(
                "persisted target fingerprint",
            ))
        ));
        Ok(())
    }

    #[test]
    fn abort_refuses_a_caller_supplied_wrong_target_before_provider_access()
    -> Result<(), Box<dyn std::error::Error>> {
        let bundle = fixture_bundle()?;
        let target = TargetFingerprintV1::for_bundle(&bundle)?;
        let wrong_pack = PackIdentityV1::new(
            "transfer-fixture",
            "wrong-abort-target",
            DigestV1::hash(b"wrong-abort-target"),
        )?;
        let wrong_target = TargetFingerprintV1::with_pack(&target, wrong_pack)?;
        let admin = fixture_admin()?;
        let mut destination = PostgresTransferDestination::new(&admin, &bundle, target.clone())?;

        assert!(matches!(
            destination.abort_import(&wrong_target),
            Err(PostgresTransferError::TargetMismatch(
                "abort target fingerprint",
            ))
        ));
        Ok(())
    }

    fn canonical_evidence_fixture() -> Result<Vec<LogicalRecordV1>, TransferError> {
        [
            (
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/lineage",
            ),
            (CanonicalRecordKindV1::StorageEpoch, "deployment/epoch"),
            (CanonicalRecordKindV1::RoomGenesis, "room/room-1/genesis"),
            (CanonicalRecordKindV1::RoomHead, "room/room-1/head"),
            (
                CanonicalRecordKindV1::CoreMaterialization,
                "room/room-1/core-materialization",
            ),
            (
                CanonicalRecordKindV1::ActivityMaterialization,
                "room/room-1/activity-materialization",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(ordinal, (kind, identity))| {
            LogicalRecordV1::canonical(ordinal as u64, kind, identity, identity.as_bytes())
        })
        .collect()
    }

    #[test]
    fn native_semantic_gate_rejects_operational_only_staging() {
        assert!(matches!(
            require_native_canonical_evidence(&[], &["room-1"]),
            Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "deployment lineage cardinality"
            })
        ));
    }

    #[test]
    fn native_semantic_gate_accepts_complete_canonical_evidence() -> Result<(), TransferError> {
        let records = canonical_evidence_fixture()?;
        assert!(require_native_canonical_evidence(&records, &["room-1"]).is_ok());
        Ok(())
    }

    #[test]
    fn native_semantic_gate_rejects_incomplete_room_evidence() -> Result<(), TransferError> {
        let mut records = canonical_evidence_fixture()?;
        records.retain(|record| {
            record.kind() != RecordKindV1::Canonical(CanonicalRecordKindV1::CoreMaterialization)
        });
        assert!(matches!(
            require_native_canonical_evidence(&records, &["room-1"]),
            Err(NativeSqliteTransferError::MissingCanonicalRecord {
                kind: CanonicalRecordKindV1::CoreMaterialization
            })
        ));
        Ok(())
    }

    #[test]
    fn native_semantic_gate_preserves_isolated_room_without_canonical_promotion()
    -> Result<(), TransferError> {
        let records = [
            LogicalRecordV1::canonical(
                0,
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/lineage",
                b"lineage",
            )?,
            LogicalRecordV1::canonical(
                1,
                CanonicalRecordKindV1::StorageEpoch,
                "deployment/epoch",
                b"7",
            )?,
        ];
        assert!(require_native_canonical_evidence(&records, &[]).is_ok());
        Ok(())
    }

    #[test]
    fn pre_retirement_native_reconfirmation_requires_existing_rows() {
        assert!(NativePublicationMode::VerifyOnly.requires_existing_row());
        assert!(!NativePublicationMode::Hydrate.requires_existing_row());
    }

    #[test]
    fn semantic_receipt_guard_requires_exact_retry_state() {
        let request_hash = [7_u8; 32];
        let receipt = b"canonical-receipt";
        assert!(semantic_receipt_guard_matches(
            &request_hash,
            Some("room-a"),
            Some(receipt),
            &request_hash,
            "room-a",
            receipt,
        ));
        assert!(!semantic_receipt_guard_matches(
            &[8_u8; 32],
            Some("room-a"),
            Some(receipt),
            &request_hash,
            "room-a",
            receipt,
        ));
        assert!(!semantic_receipt_guard_matches(
            &request_hash,
            Some("room-b"),
            Some(receipt),
            &request_hash,
            "room-a",
            receipt,
        ));
        assert!(!semantic_receipt_guard_matches(
            &request_hash,
            Some("room-a"),
            None,
            &request_hash,
            "room-a",
            receipt,
        ));
        assert!(!semantic_receipt_guard_matches(
            &request_hash,
            Some("room-a"),
            Some(b"different-receipt"),
            &request_hash,
            "room-a",
            receipt,
        ));
    }

    #[test]
    fn room_transition_identity_requires_positive_sequence() {
        assert!(canonical_room_sequence("room/r-1/transition/1").is_ok());
        assert!(canonical_room_sequence("room/r-1/transition/0").is_err());
        assert!(canonical_room_sequence("room/r-1/transition/not-a-sequence").is_err());
        assert!(canonical_room_sequence("room/r-1/head/1").is_err());
    }

    #[test]
    fn canonical_bytes_are_persisted_without_reencoding() {
        let mut room = CanonicalRoomBytes::default();
        let genesis = [0, 1, 2, 255];
        assert!(set_exact(&mut room.genesis, &genesis, "duplicate Room Genesis").is_ok());
        assert_eq!(room.genesis.as_deref(), Some(genesis.as_slice()));
    }

    #[test]
    fn pack_revision_lock_is_required_and_never_derived_from_pack_identity() {
        let room = CanonicalRoomBytes::default();
        assert!(matches!(
            required_pack_revision_lock(&room),
            Err(PostgresTransferError::Canonical(
                "missing pack revision lock bytes"
            ))
        ));

        let room = CanonicalRoomBytes {
            pack_revision_lock: Some(vec![0, 1, 2, 255]),
            ..Default::default()
        };
        assert!(required_pack_revision_lock(&room).is_ok_and(|bytes| bytes == [0, 1, 2, 255]));
    }

    #[test]
    fn deployment_metadata_extracts_exact_bytes() -> Result<(), TransferError> {
        let records = canonical_evidence_fixture()?;
        let metadata = extract_deployment_metadata(&records)
            .map_err(|_| TransferError::InvalidValue { what: "metadata" })?;
        assert_eq!(metadata.deployment_lineage, b"deployment/lineage");
        assert_eq!(metadata.storage_epoch_bytes, b"deployment/epoch");
        Ok(())
    }

    #[test]
    fn deployment_metadata_requires_exactly_one_record_of_each_kind() -> Result<(), TransferError> {
        let mut records = canonical_evidence_fixture()?;
        records.retain(|record| {
            record.kind() != RecordKindV1::Canonical(CanonicalRecordKindV1::StorageEpoch)
        });
        assert!(matches!(
            extract_deployment_metadata(&records),
            Err(PostgresTransferError::Canonical("missing StorageEpoch"))
        ));

        let duplicate = LogicalRecordV1::canonical(
            records.len() as u64,
            CanonicalRecordKindV1::DeploymentLineage,
            "deployment/lineage-duplicate",
            b"deployment/lineage",
        )?;
        records.push(duplicate);
        assert!(matches!(
            extract_deployment_metadata(&records),
            Err(PostgresTransferError::Canonical(
                "duplicate DeploymentLineage"
            ))
        ));
        Ok(())
    }

    #[test]
    fn duplicate_canonical_import_with_different_bytes_is_rejected() {
        let mut room = CanonicalRoomBytes::default();
        assert!(set_exact(&mut room.head, b"head-v1", "duplicate Room Head").is_ok());
        assert!(matches!(
            set_exact(&mut room.head, b"head-v2", "duplicate Room Head"),
            Err(PostgresTransferError::Canonical("duplicate Room Head"))
        ));
    }

    #[test]
    fn canonical_room_identity_mismatch_is_not_persistable() {
        assert_eq!(canonical_room_id("room/r-1/head").as_deref(), Some("r-1"));
        assert_eq!(canonical_room_id("deployment/epoch"), None);
        assert_eq!(canonical_room_id("room//head"), None);
    }

    #[test]
    fn rollback_refuses_after_authority() {
        assert!(matches!(
            PostgresTransferError::RollbackRefused,
            PostgresTransferError::RollbackRefused
        ));
    }
}
