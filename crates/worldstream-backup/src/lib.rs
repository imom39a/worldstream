//! Read-only metadata and verification for backend-native `WorldStream` backups.
//!
//! The immutable image verifier is provider-neutral. The native `SQLite` module
//! additionally performs bounded, fail-closed online backup and restore using
//! the bundled `SQLite` engine. This crate does not open `PostgreSQL`, contact a
//! provider, repair records, or change readiness outside the returned report.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod native_envelope;
pub mod native_restore;
pub mod native_sqlite;

pub use native_envelope::{
    NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1, NativeSqliteAuthoritativeMaterializationV1,
    NativeSqliteBackupEnvelopeV1, NativeSqliteEnvelopeCoverageV1, NativeSqliteEnvelopeError,
    NativeSqliteEnvelopeOriginV1, NativeSqliteNativeWitnessV1, NativeSqliteRequestLedgerV1,
    NativeSqliteRequestWitnessV1, NativeSqliteTimerRelationV1, native_evidence_digest,
    native_membership_digest,
};

pub use native_restore::{
    NativeRestoreCanonicalRowV1, NativeRestoreDurableDomainEvidenceV1,
    NativeRestoreDurableDomainV1, NativeRestoreEvidenceAdapter, NativeRestoreEvidenceV1,
    NativeRestoreRoomMembershipV1, NativeRestoreTargetEvidenceV1,
    POSTGRES_NATIVE_RESTORE_DURABLE_DOMAINS_V1, RestoreCommitWitnessV1, RestoreInstallOutcomeV1,
    RestoreLifecycleErrorV1, RestoreLifecycleProjectionV1, RestoreLifecycleStateV1,
    RestoreRoomLifecycleStateV1, max_native_restore_canonical_row_bytes, verify_native_restore,
    verify_native_restore_from_adapter,
};
pub use native_sqlite::{
    NativeSqliteCaptureWitnessV1, NativeSqliteRestoreInputProjectionV1,
    NativeSqliteRestoreProjectionError, NativeSqliteRoomDispositionV1,
    NativeSqliteRoomRestoreInputV1, assess_restore_readiness_with_target_metadata,
    bridge_restore_evidence_with_target_metadata, project_restore_input,
};

/// The only manifest schema understood by this crate.
pub const BACKUP_MANIFEST_SCHEMA_V1: &str = "worldstream/backup-manifest/v1";
/// The canonical hash suite used by the backup seam.
pub const HASH_SUITE_BLAKE3_V1: &str = "blake3/v1";
/// The paired snapshot identity used by the storage contracts.
pub const PAIRED_SNAPSHOT_SCHEMA_V1: &str = "worldstream/paired-snapshot/v1";

const MAX_TEXT_BYTES: usize = 512;
/// Runtime-compatible maximum deployment-lineage byte length.
pub(crate) const MAX_DEPLOYMENT_LINEAGE_BYTES: usize = 128;
/// Runtime-compatible JavaScript-safe storage epoch maximum.
pub(crate) const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// A fixed-size BLAKE3 digest rendered as lowercase hexadecimal when serialized.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DigestV1(String);

impl DigestV1 {
    /// Hashes canonical bytes with the frozen hash suite.
    #[must_use]
    pub fn hash(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    /// Constructs a digest after checking its canonical textual shape.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataError::InvalidDigest`] when the value is not a
    /// 64-character hexadecimal digest.
    pub fn parse(value: impl Into<String>) -> Result<Self, MetadataError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(MetadataError::InvalidDigest);
        }
        let value = value.to_ascii_lowercase();
        Ok(Self(value))
    }

    fn validate(&self) -> Result<(), MetadataError> {
        if self.0.len() != 64 || !self.0.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(MetadataError::InvalidDigest);
        }
        Ok(())
    }

    /// Returns the canonical hexadecimal digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The backend profile that produced a backup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BackendProfileV1 {
    /// The release-bundled `SQLite` profile.
    SqliteBundled,
    /// The supported `PostgreSQL` 17 primary profile.
    PostgresPrimary,
}

/// The native mechanism and consistent point supplied by an operator/native adapter.
///
/// These values are evidence fields, not a claim that this crate performed the
/// corresponding native backup or restore.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BackendNativePointV1 {
    /// `SQLite` online-backup metadata captured while the source was quiesced enough
    /// for the native online-backup API to define a consistent point.
    SqliteOnlineBackup {
        /// Redacted engine build identity, for example `sqlite-3.53.4-bundled`.
        engine_identity: String,
        /// Adapter-owned opaque native point identity.
        point_id: String,
    },
    /// `PostgreSQL` native snapshot/PITR/dump metadata captured by an operator.
    PostgresNative {
        /// Supported `PostgreSQL` major, expected to be 17 for this release.
        major: u16,
        /// Redacted engine identity, never a DSN or provider credential.
        engine_identity: String,
        /// Adapter-owned opaque native point identity.
        point_id: String,
        /// Native mechanism used by the operator or provider.
        mechanism: PostgresNativeMechanismV1,
    },
}

/// PostgreSQL-native mechanisms accepted as metadata by this seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PostgresNativeMechanismV1 {
    /// A consistent native snapshot.
    Snapshot,
    /// Point-in-time recovery evidence.
    PointInTimeRecovery,
    /// A native logical or physical dump/restore evidence point.
    Dump,
}

impl BackendNativePointV1 {
    fn backend(&self) -> BackendProfileV1 {
        match self {
            Self::SqliteOnlineBackup { .. } => BackendProfileV1::SqliteBundled,
            Self::PostgresNative { .. } => BackendProfileV1::PostgresPrimary,
        }
    }

    fn validate(&self) -> Result<(), MetadataError> {
        match self {
            Self::SqliteOnlineBackup {
                engine_identity,
                point_id,
            } => {
                validate_text(engine_identity)?;
                validate_text(point_id)?;
            }
            Self::PostgresNative {
                major,
                engine_identity,
                point_id,
                ..
            } => {
                if *major != 17 {
                    return Err(MetadataError::UnsupportedPostgresMajor(*major));
                }
                validate_text(engine_identity)?;
                validate_text(point_id)?;
            }
        }
        Ok(())
    }
}

/// Versioned identity and expected content contract for a backup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupManifestV1 {
    /// Manifest schema identifier.
    pub schema: String,
    /// Immutable backup identity.
    pub backup_id: String,
    /// Explicit deployment lineage recorded by the source deployment.
    ///
    /// This is deliberately separate from the global canonical-bytes digest:
    /// the lineage is an operator-owned identity, while the digest is a
    /// content witness. Neither is derived from a file path or timestamp.
    pub deployment_lineage: String,
    /// Storage epoch fenced by the source deployment.
    pub storage_epoch: u64,
    /// Backend selected by the source deployment.
    pub backend: BackendProfileV1,
    /// Native consistent point metadata.
    pub native_point: BackendNativePointV1,
    /// Logical migration and schema identity expected at restore.
    pub migration_contract: MigrationContractV1,
    /// Exact retained Activity Pack executor identities.
    pub expected_packs: Vec<PackIdentityV1>,
    /// Exact immutable resources referenced by those pack identities.
    pub expected_resources: Vec<ResourceIdentityV1>,
    /// Global digest over the manifest's expected deployment lineage.
    pub expected_global_digest: DigestV1,
}

impl BackupManifestV1 {
    /// Validates bounded metadata and cross-field identity before image verification.
    ///
    /// # Errors
    ///
    /// Returns a [`MetadataError`] when metadata is unsupported, malformed, or
    /// exceeds the configured verifier bounds.
    pub fn validate(&self, limits: &VerifierLimits) -> Result<(), MetadataError> {
        if self.schema != BACKUP_MANIFEST_SCHEMA_V1 {
            return Err(MetadataError::UnsupportedManifest(self.schema.clone()));
        }
        validate_text(&self.backup_id)?;
        validate_deployment_lineage(&self.deployment_lineage)?;
        if !(1..=MAX_SAFE_INTEGER).contains(&self.storage_epoch) {
            return Err(MetadataError::ZeroGeneration("storage_epoch"));
        }
        self.native_point.validate()?;
        if self.native_point.backend() != self.backend {
            return Err(MetadataError::BackendNativeMismatch);
        }
        self.migration_contract.validate()?;
        self.expected_global_digest.validate()?;
        bounded_len(
            self.migration_contract.records.len(),
            limits.max_ledger_rows,
            "migration records",
        )?;
        bounded_len(
            self.expected_packs.len(),
            limits.max_packs,
            "expected packs",
        )?;
        bounded_len(
            self.expected_resources.len(),
            limits.max_resources,
            "expected resources",
        )?;
        unique_packs(&self.expected_packs)?;
        unique_resources(&self.expected_resources)?;
        for pack in &self.expected_packs {
            pack.validate()?;
            for resource_id in &pack.resource_ids {
                if !self
                    .expected_resources
                    .iter()
                    .any(|resource| resource.resource_id == *resource_id)
                {
                    return Err(MetadataError::MissingExpectedResource(resource_id.clone()));
                }
            }
        }
        for resource in &self.expected_resources {
            resource.validate()?;
        }
        Ok(())
    }
}

/// The migration ledger and schema fingerprint expected by both storage adapters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MigrationContractV1 {
    /// One logical forward-only migration history.
    pub logical_history_id: String,
    /// Canonical schema contract fingerprint.
    pub schema_contract_fingerprint: DigestV1,
    /// Ordered migration identities and checksums.
    pub records: Vec<MigrationIdentityV1>,
}

impl MigrationContractV1 {
    fn validate(&self) -> Result<(), MetadataError> {
        validate_text(&self.logical_history_id)?;
        self.schema_contract_fingerprint.validate()?;
        if self.records.is_empty() {
            return Err(MetadataError::EmptyMigrationHistory);
        }
        let mut expected = 1_u32;
        for record in &self.records {
            if record.version != expected {
                return Err(MetadataError::NonContiguousMigration(record.version));
            }
            record.validate()?;
            expected = expected
                .checked_add(1)
                .ok_or(MetadataError::CounterOverflow("migration version"))?;
        }
        Ok(())
    }
}

/// A logical migration identity, independent of provider-specific catalog details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MigrationIdentityV1 {
    /// One-based contiguous version.
    pub version: u32,
    /// Immutable logical migration identifier.
    pub migration_id: String,
    /// Digest of the reviewed backend-specific migration body.
    pub checksum: DigestV1,
}

impl MigrationIdentityV1 {
    fn validate(&self) -> Result<(), MetadataError> {
        if self.version == 0 {
            return Err(MetadataError::ZeroGeneration("migration version"));
        }
        validate_text(&self.migration_id)?;
        self.checksum.validate()
    }
}

/// Exact executable identity retained for a Room's Activity Pack revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackIdentityV1 {
    /// Stable logical pack ID.
    pub pack_id: String,
    /// Exact semantic revision digest.
    pub revision_digest: DigestV1,
    /// Exact executor artifact digest.
    pub executor_digest: DigestV1,
    /// Exact schema bundle digest.
    pub schema_bundle_digest: DigestV1,
    /// Exact codec bundle digest.
    pub codec_bundle_digest: DigestV1,
    /// Resources required by this executor revision.
    pub resource_ids: Vec<String>,
}

impl PackIdentityV1 {
    fn validate(&self) -> Result<(), MetadataError> {
        validate_text(&self.pack_id)?;
        self.revision_digest.validate()?;
        self.executor_digest.validate()?;
        self.schema_bundle_digest.validate()?;
        self.codec_bundle_digest.validate()?;
        bounded_len(self.resource_ids.len(), 128, "pack resources")?;
        let mut ids = BTreeSet::new();
        for resource_id in &self.resource_ids {
            validate_text(resource_id)?;
            if !ids.insert(resource_id) {
                return Err(MetadataError::DuplicateIdentity(resource_id.clone()));
            }
        }
        Ok(())
    }
}

/// Expected content-addressed artifact metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceIdentityV1 {
    /// Stable resource identity within the manifest.
    pub resource_id: String,
    /// Resource kind, such as `executor` or `codec`.
    pub kind: String,
    /// Exact byte length.
    pub byte_len: u64,
    /// Exact BLAKE3 digest.
    pub digest: DigestV1,
}

impl ResourceIdentityV1 {
    fn validate(&self) -> Result<(), MetadataError> {
        validate_text(&self.resource_id)?;
        validate_text(&self.kind)?;
        self.digest.validate()?;
        if self.byte_len > usize::MAX as u64 {
            return Err(MetadataError::ResourceTooLarge(self.resource_id.clone()));
        }
        Ok(())
    }
}

/// Immutable resource bytes observed at the restored target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceBlobV1 {
    /// Resource identity from the manifest.
    pub resource_id: String,
    /// Bytes copied by a native restore adapter.
    pub bytes: Vec<u8>,
}

/// A stored canonical record whose bytes must never be decoded and re-encoded here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRecordV1 {
    /// Record kind used for diagnostics and relation checks.
    pub kind: CanonicalRecordKindV1,
    /// Room sequence; Genesis is sequence zero.
    pub room_seq: u64,
    /// Exact canonical bytes from storage.
    pub bytes: Vec<u8>,
    /// Stored BLAKE3 digest of `bytes`.
    pub digest: DigestV1,
    /// Previous Genesis/Transition digest, absent only for Genesis.
    pub previous_digest: Option<DigestV1>,
}

/// Canonical record categories needed by the restore verifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CanonicalRecordKindV1 {
    /// Sequence-zero creation record.
    Genesis,
    /// A committed Room transition.
    Transition,
}

/// Complete Head identity and state hashes for one Room.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompleteHeadV1 {
    /// Head's Room identity.
    pub room_id: String,
    /// Head sequence, zero for Genesis.
    pub room_seq: u64,
    /// Genesis/Transition digest at the Head.
    pub lineage_digest: DigestV1,
    /// Core schema identity.
    pub core_schema_version: String,
    /// Exact retained Activity Pack revision.
    pub pack_revision_digest: DigestV1,
    /// Hash of canonical Core State bytes.
    pub core_state_digest: DigestV1,
    /// Hash of canonical Activity State bytes.
    pub activity_state_digest: DigestV1,
    /// Hash of the canonical paired Authoritative State bytes.
    pub authoritative_state_digest: DigestV1,
}

/// Canonical materialization bytes paired with a Complete Head.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MaterializationV1 {
    /// Canonical Core State bytes.
    pub core_state_bytes: Vec<u8>,
    /// Canonical Activity State bytes.
    pub activity_state_bytes: Vec<u8>,
    /// Canonical paired Authoritative State bytes.
    pub authoritative_state_bytes: Vec<u8>,
}

/// Operational Room integrity status copied byte-for-byte from the source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IntegrityStatusV1 {
    /// Healthy Rooms must pass full semantic verification.
    Healthy,
    /// A pre-existing operational fault may remain isolated.
    Faulted,
    /// A pre-existing canonical-integrity quarantine may remain isolated.
    Quarantined,
}

/// Integrity state and the explicit isolation witness required for an exception.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IntegrityWitnessV1 {
    /// Status recorded before native backup.
    pub source_status: IntegrityStatusV1,
    /// Status observed after restore.
    pub restored_status: IntegrityStatusV1,
    /// Monotonic source generation.
    pub source_generation: u64,
    /// Monotonic restored generation.
    pub restored_generation: u64,
    /// Whether the source explicitly isolated the Room before backup.
    pub source_isolated: bool,
    /// Whether the restored target kept the Room isolated.
    pub restored_isolated: bool,
}

/// A Room's canonical and operational evidence in the immutable image seam.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoomImageV1 {
    /// Stable Room identity.
    pub room_id: String,
    /// Integrity witness captured at both sides of the native restore.
    pub integrity: IntegrityWitnessV1,
    /// Complete Head after restore.
    pub head: CompleteHeadV1,
    /// Genesis followed by ordered Transitions.
    pub records: Vec<CanonicalRecordV1>,
    /// Paired current materialization cache.
    pub materialization: MaterializationV1,
    /// Digest of the source Room bytes before native restore.
    pub source_bytes_digest: DigestV1,
    /// Digest of the restored Room bytes at verification time.
    pub restored_bytes_digest: DigestV1,
}

/// A semantic receipt preserving the exact request and result bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptV1 {
    /// Receipt family.
    pub kind: ReceiptKindV1,
    /// Stable operation identity bytes.
    pub identity_bytes: Vec<u8>,
    /// Canonical request bytes.
    pub request_bytes: Vec<u8>,
    /// Whether the native ledger persisted the canonical request bytes.
    ///
    /// Some provider schemas retain only the canonical request hash. Those
    /// adapters must leave `request_bytes` empty and carry the exact stored
    /// hash in `request_digest`; they may not invent or reconstruct bytes.
    pub request_bytes_available: bool,
    /// Stored canonical request hash.
    pub request_digest: DigestV1,
    /// Receipt resolution bytes.
    pub result_bytes: Vec<u8>,
    /// Stored receipt/result hash.
    pub result_digest: DigestV1,
    /// Optional Room and transition relation.
    pub room_id: Option<String>,
    /// Optional committed sequence for a transition resolution.
    pub transition_seq: Option<u64>,
    /// Activation identity for an Activation operation receipt.
    pub activation_id: Option<String>,
    /// Exact Activation operation kind, absent for semantic Room receipts.
    pub operation_kind: Option<String>,
}

/// Receipt families retained by the two operational ledgers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReceiptKindV1 {
    /// Room Commit semantic receipt.
    Semantic,
    /// Activation offer/claim/lease operation receipt.
    ActivationOperation,
}

/// Durable timer witness; `scheduled_for` is semantic time, not detection time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimerV1 {
    /// Owning Room.
    pub room_id: String,
    /// Logical timer identity.
    pub timer_id: String,
    /// Never-reused host generation.
    pub generation: u64,
    /// Immutable semantic schedule value.
    pub scheduled_for: String,
    /// Exact payload bytes.
    pub payload_bytes: Vec<u8>,
    /// Stored payload digest.
    pub payload_digest: DigestV1,
    /// Durable timer state.
    pub state: TimerStateV1,
    /// Consuming transition when state is Fired.
    pub fired_transition_seq: Option<u64>,
}

/// Durable timer lifecycle value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TimerStateV1 {
    /// Still an obligation.
    Scheduled,
    /// Cancelled by a canonical Advance.
    Cancelled,
    /// Consumed by one `TimerFired` Advance.
    Fired,
}

/// Durable Membership-addressed Observation Frame witness.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrameV1 {
    /// Owning Room.
    pub room_id: String,
    /// Addressed Membership.
    pub member_id: String,
    /// Never-reused per-Membership frame sequence.
    pub frame_seq: u64,
    /// Causal Room Transition sequence.
    pub cause_room_seq: u64,
    /// Exact authorized frame bytes.
    pub payload_bytes: Vec<u8>,
    /// Stored payload digest.
    pub payload_digest: DigestV1,
    /// Durable Membership frame head.
    pub frame_head: u64,
    /// Durable Cursor, if acknowledged.
    pub cursor: Option<u64>,
}

/// Operational Activation state needed for consistency verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ActivationStateV1 {
    /// Awaiting a Runner claim.
    Pending,
    /// Held by one fenced lease.
    Leased,
    /// Completed.
    Completed,
    /// Lease or intent expired.
    Expired,
    /// Cancelled by an authoritative fence.
    Cancelled,
}

/// Retained Invocation Context or its immutable tombstone.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContextRetentionV1 {
    /// Context bytes remain available and are hash-checked.
    Retained(Vec<u8>),
    /// Context bytes were retired but the original hash remains.
    Tombstone,
    /// No context was ever committed.
    None,
}

/// Durable Activation Intent witness.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActivationV1 {
    /// Stable Activation identity.
    pub activation_id: String,
    /// Owning Room.
    pub room_id: String,
    /// Causal Room sequence.
    pub cause_room_seq: u64,
    /// Target Membership.
    pub target_member_id: String,
    /// Durable state.
    pub state: ActivationStateV1,
    /// Intent generation fence.
    pub intent_generation: u64,
    /// Lease generation fence; zero means no live lease.
    pub lease_generation: u64,
    /// Runner ID while leased.
    pub runner_id: Option<String>,
    /// Claim ID while leased.
    pub claim_id: Option<String>,
    /// Lease deadline while leased.
    pub lease_until: Option<String>,
    /// Original context hash when context exists or was retired.
    pub context_digest: Option<DigestV1>,
    /// Context bytes, tombstone, or no context.
    pub context: ContextRetentionV1,
}

/// One backup image presented by a native restore adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupImageV1 {
    /// Immutable manifest captured with the native backup.
    pub manifest: BackupManifestV1,
    /// Native identity observed at the restored target.
    pub restored_native_point: BackendNativePointV1,
    /// Migration ledger observed at the restored target.
    pub migrations: MigrationContractV1,
    /// Restored content-addressed resources.
    pub resources: Vec<ResourceBlobV1>,
    /// Restored Rooms.
    pub rooms: Vec<RoomImageV1>,
    /// Semantic receipts across all Rooms.
    pub receipts: Vec<ReceiptV1>,
    /// Timer ledger across all Rooms.
    pub timers: Vec<TimerV1>,
    /// Observation Frames across all Rooms.
    pub frames: Vec<FrameV1>,
    /// Activation Intents across all Rooms.
    pub activations: Vec<ActivationV1>,
    /// Activation operation receipts represented by exact request/result bytes.
    pub activation_receipts: Vec<ReceiptV1>,
    /// Global digest captured at the source and restored target.
    pub source_global_digest: DigestV1,
    /// Global digest observed at verification time.
    pub restored_global_digest: DigestV1,
}

/// Explicit bounds for all verifier work and returned diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifierLimits {
    /// Maximum pack identities.
    pub max_packs: usize,
    /// Maximum resource identities/blobs.
    pub max_resources: usize,
    /// Maximum Rooms.
    pub max_rooms: usize,
    /// Maximum canonical records per Room.
    pub max_records_per_room: usize,
    /// Maximum operational rows in each ledger.
    pub max_ledger_rows: usize,
    /// Maximum canonical bytes in one record/payload/materialization.
    pub max_object_bytes: usize,
    /// Maximum diagnostics returned.
    pub max_diagnostics: usize,
}

impl Default for VerifierLimits {
    fn default() -> Self {
        Self {
            max_packs: 256,
            max_resources: 4_096,
            max_rooms: 100_000,
            max_records_per_room: 100_000,
            max_ledger_rows: 1_000_000,
            max_object_bytes: 16 * 1024 * 1024,
            max_diagnostics: 256,
        }
    }
}

/// High-level consistency class for one actionable diagnostic.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ConsistencyClassV1 {
    /// Manifest identity, backend point, or global deployment lineage.
    Manifest,
    /// Canonical record bytes and hash chain.
    CanonicalRecord,
    /// Migration history and schema fingerprint.
    Migration,
    /// Semantic receipts and request/result hashes.
    Receipt,
    /// Timer identity, generation, payload, and consuming relation.
    Timer,
    /// Observation Frame identity, payload, cursor, and causal relation.
    Frame,
    /// Activation Intent, lease/context, and operation receipt relation.
    Activation,
    /// Room integrity state and isolation witness.
    Integrity,
    /// Cross-ledger relation that cannot be attributed to one record class.
    OperationalRelation,
    /// Content-addressed resource bytes.
    Resource,
}

/// Whether a diagnostic blocks the whole restored deployment or is an allowed room exception.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiagnosticDispositionV1 {
    /// A global or newly introduced mismatch blocks readiness.
    Blocking,
    /// A byte-preserved, explicitly pre-existing isolated unhealthy Room is allowed to remain isolated.
    PermittedPreExistingIsolation,
}

/// Redacted, bounded, operator-actionable verifier diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticV1 {
    /// Consistency class.
    pub class: ConsistencyClassV1,
    /// Stable machine-readable code.
    pub code: String,
    /// Redacted subject, never raw bytes, DSNs, credentials, or payload text.
    pub subject: String,
    /// Short operator action.
    pub action: String,
    /// Blocking or permitted disposition.
    pub disposition: DiagnosticDispositionV1,
}

impl DiagnosticV1 {
    fn blocking(class: ConsistencyClassV1, code: &str, subject: &str, action: &str) -> Self {
        Self {
            class,
            code: code.to_owned(),
            subject: redact_subject(subject),
            action: action.to_owned(),
            disposition: DiagnosticDispositionV1::Blocking,
        }
    }

    fn permitted(class: ConsistencyClassV1, code: &str, subject: &str, action: &str) -> Self {
        Self {
            class,
            code: code.to_owned(),
            subject: redact_subject(subject),
            action: action.to_owned(),
            disposition: DiagnosticDispositionV1::PermittedPreExistingIsolation,
        }
    }
}

/// Deployment readiness after verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReadinessV1 {
    /// All global evidence and every healthy Room passed.
    Ready,
    /// At least one blocking diagnostic exists; serving must remain disabled.
    NotReady,
}

/// Per-Room verification disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RoomDispositionV1 {
    /// Healthy and semantically verified.
    Verified,
    /// Byte-preserved and explicitly pre-existing isolated Faulted/Quarantined.
    IsolatedPreExisting,
    /// Not verified and cannot be served.
    Blocked,
}

/// Complete immutable verification result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerificationReportV1 {
    /// Overall readiness classification.
    pub readiness: ReadinessV1,
    /// Per-Room dispositions by stable Room ID.
    pub rooms: BTreeMap<String, RoomDispositionV1>,
    /// Bounded diagnostics, ordered by discovery.
    pub diagnostics: Vec<DiagnosticV1>,
    #[serde(skip)]
    blocking_detected: bool,
}

impl VerificationReportV1 {
    /// Returns true only when readiness is explicitly ready.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self.readiness, ReadinessV1::Ready)
    }

    pub(crate) fn add_blocking_diagnostic(
        &mut self,
        limit: usize,
        class: ConsistencyClassV1,
        code: &str,
        subject: &str,
        action: &str,
    ) {
        push_diagnostic(
            self,
            limit,
            DiagnosticV1::blocking(class, code, subject, action),
        );
        self.readiness = ReadinessV1::NotReady;
    }
}

/// Metadata and validation failures before semantic image verification.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum MetadataError {
    /// A versioned schema is not implemented.
    #[error("unsupported backup manifest schema: {0}")]
    UnsupportedManifest(String),
    /// A digest was not a lowercase/uppercase 64-character hexadecimal value.
    #[error("digest is not canonical BLAKE3 text")]
    InvalidDigest,
    /// A bounded text field was empty or too large.
    #[error("metadata text is empty or exceeds the bounded limit")]
    InvalidText,
    /// Native point and selected backend disagree.
    #[error("backend-native point does not match selected backend")]
    BackendNativeMismatch,
    /// `PostgreSQL` metadata names a non-supported major.
    #[error("unsupported PostgreSQL major: {0}")]
    UnsupportedPostgresMajor(u16),
    /// A generation/version was zero.
    #[error("{0} must be nonzero")]
    ZeroGeneration(&'static str),
    /// A bounded collection exceeded its configured limit.
    #[error("{what} exceeds verifier bound")]
    BoundExceeded { what: &'static str },
    /// Migration history is empty.
    #[error("migration history is empty")]
    EmptyMigrationHistory,
    /// Migration versions are not contiguous.
    #[error("migration history is not contiguous at version {0}")]
    NonContiguousMigration(u32),
    /// A checked counter could not advance.
    #[error("counter overflow while validating {0}")]
    CounterOverflow(&'static str),
    /// A manifest identity was duplicated.
    #[error("duplicate identity: {0}")]
    DuplicateIdentity(String),
    /// A pack references a resource absent from the manifest.
    #[error("pack references missing expected resource: {0}")]
    MissingExpectedResource(String),
    /// A resource length cannot be represented by the verifier.
    #[error("resource is too large: {0}")]
    ResourceTooLarge(String),
}

/// Verifies an immutable native-restore image without side effects.
///
/// The function only reads `image` and returns a bounded report. It never
/// modifies the image, starts a timer, delivers a frame, claims an Activation,
/// changes integrity state, writes a receipt, or marks a target ready.
#[must_use]
pub fn verify_restore(image: &BackupImageV1, limits: VerifierLimits) -> VerificationReportV1 {
    let mut report = VerificationReportV1 {
        readiness: ReadinessV1::NotReady,
        rooms: BTreeMap::new(),
        diagnostics: Vec::new(),
        blocking_detected: false,
    };
    if image.manifest.validate(&limits).is_err() {
        push_diagnostic(
            &mut report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Manifest,
                "manifest_invalid",
                "deployment",
                "repair or recapture the backup manifest before restoring",
            ),
        );
        return report;
    }
    check_native_point(image, &mut report, &limits);
    check_migrations(image, &mut report, &limits);
    check_resources(image, &mut report, &limits);
    check_global_digest(image, &mut report, &limits);

    if bounded_len(image.rooms.len(), limits.max_rooms, "Rooms").is_err() {
        push_diagnostic(
            &mut report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Manifest,
                "room_bound_exceeded",
                "deployment",
                "restore into a bounded target or increase the reviewed verifier limit",
            ),
        );
    } else {
        for room in &image.rooms {
            verify_room(room, image, &mut report, &limits);
        }
    }
    check_receipts(image, &mut report, &limits);
    check_timers(image, &mut report, &limits);
    check_frames(image, &mut report, &limits);
    check_activations(image, &mut report, &limits);
    report.readiness = if report.blocking_detected {
        ReadinessV1::NotReady
    } else {
        ReadinessV1::Ready
    };
    report
}

fn check_native_point(
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    if image.restored_native_point != image.manifest.native_point {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Manifest,
                "native_point_mismatch",
                "backend-native point",
                "restore the exact native point recorded by the manifest",
            ),
        );
    }
}

fn check_migrations(
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    if image.migrations != image.manifest.migration_contract {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Migration,
                "migration_contract_mismatch",
                "migration ledger",
                "restore the exact forward-only migration prefix and schema fingerprint",
            ),
        );
    }
}

fn check_resources(
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    if bounded_len(image.resources.len(), limits.max_resources, "resources").is_err() {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Resource,
                "resource_bound_exceeded",
                "resource set",
                "restore into a bounded artifact target",
            ),
        );
        return;
    }
    let mut actual = BTreeMap::new();
    for resource in &image.resources {
        if actual
            .insert(resource.resource_id.as_str(), resource)
            .is_some()
        {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Resource,
                    "duplicate_resource",
                    "resource identity",
                    "remove the duplicate resource row and restore again",
                ),
            );
            continue;
        }
        let Some(expected) = image
            .manifest
            .expected_resources
            .iter()
            .find(|expected| expected.resource_id == resource.resource_id)
        else {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Resource,
                    "unexpected_resource",
                    "resource identity",
                    "restore only resources named by the immutable manifest",
                ),
            );
            continue;
        };
        if resource.bytes.len() > limits.max_object_bytes
            || resource.bytes.len() as u64 != expected.byte_len
            || DigestV1::hash(&resource.bytes) != expected.digest
        {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Resource,
                    "resource_digest_mismatch",
                    &resource.resource_id,
                    "recopy the exact content-addressed resource bytes",
                ),
            );
        }
    }
    for expected in &image.manifest.expected_resources {
        if !actual.contains_key(expected.resource_id.as_str()) {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Resource,
                    "resource_missing",
                    &expected.resource_id,
                    "restore the missing immutable resource before readiness",
                ),
            );
        }
    }
}

fn check_global_digest(
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    if image.source_global_digest != image.restored_global_digest
        || image.restored_global_digest != image.manifest.expected_global_digest
    {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Manifest,
                "global_lineage_mismatch",
                "deployment lineage",
                "discard the target and repeat native restore from the immutable backup",
            ),
        );
    }
}

#[allow(clippy::too_many_lines)]
fn verify_room(
    room: &RoomImageV1,
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    if report.rooms.contains_key(&room.room_id) {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Integrity,
                "duplicate_room",
                "room identity",
                "restore one immutable Room root per Room ID",
            ),
        );
        return;
    }
    if room.head.room_id != room.room_id {
        report
            .rooms
            .insert(room.room_id.clone(), RoomDispositionV1::Blocked);
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::CanonicalRecord,
                "head_room_mismatch",
                &room.room_id,
                "restore the Complete Head belonging to this Room",
            ),
        );
        return;
    }
    if room.integrity.source_status != room.integrity.restored_status
        || room.integrity.source_generation != room.integrity.restored_generation
        || room.integrity.source_isolated != room.integrity.restored_isolated
    {
        report
            .rooms
            .insert(room.room_id.clone(), RoomDispositionV1::Blocked);
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Integrity,
                "integrity_witness_mismatch",
                &room.room_id,
                "discard the target; preserve the source integrity status and generation exactly",
            ),
        );
        return;
    }
    if let Err(error) = bounded_len(
        room.records.len(),
        limits.max_records_per_room,
        "canonical records",
    ) {
        report
            .rooms
            .insert(room.room_id.clone(), RoomDispositionV1::Blocked);
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::CanonicalRecord,
                "record_bound_exceeded",
                &room.room_id,
                &format!("restore within the reviewed record bound: {error}"),
            ),
        );
        return;
    }
    if !room_source_is_byte_preserved(room) {
        report
            .rooms
            .insert(room.room_id.clone(), RoomDispositionV1::Blocked);
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::Integrity,
                "new_room_bytes_mismatch",
                &room.room_id,
                "discard the target; backup/restore changed Room bytes",
            ),
        );
        return;
    }
    if is_pre_existing_isolation(room) {
        report
            .rooms
            .insert(room.room_id.clone(), RoomDispositionV1::IsolatedPreExisting);
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::permitted(
                ConsistencyClassV1::Integrity,
                "pre_existing_isolation_preserved",
                &room.room_id,
                "keep the Room isolated; investigate or repair it through the separate verifier-repair contract",
            ),
        );
        return;
    }
    report
        .rooms
        .insert(room.room_id.clone(), RoomDispositionV1::Verified);
    verify_canonical_room(room, image, report, limits);
}

fn room_source_is_byte_preserved(room: &RoomImageV1) -> bool {
    room.source_bytes_digest == room.restored_bytes_digest
}

fn is_pre_existing_isolation(room: &RoomImageV1) -> bool {
    matches!(
        (room.integrity.source_status, room.integrity.restored_status),
        (IntegrityStatusV1::Faulted, IntegrityStatusV1::Faulted)
            | (
                IntegrityStatusV1::Quarantined,
                IntegrityStatusV1::Quarantined
            )
    ) && room.integrity.source_generation == room.integrity.restored_generation
        && room.integrity.source_isolated
        && room.integrity.restored_isolated
}

fn verify_canonical_room(
    room: &RoomImageV1,
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    let Some(genesis) = room.records.first() else {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::CanonicalRecord,
                "genesis_missing",
                &room.room_id,
                "restore the complete Genesis-to-Head canonical lineage",
            ),
        );
        return;
    };
    if genesis.kind != CanonicalRecordKindV1::Genesis
        || genesis.room_seq != 0
        || genesis.previous_digest.is_some()
    {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::CanonicalRecord,
                "genesis_shape_invalid",
                &room.room_id,
                "restore a sequence-zero Genesis with no predecessor",
            ),
        );
    }
    let mut previous = None;
    for (index, record) in room.records.iter().enumerate() {
        let expected_seq = index as u64;
        if record.room_seq != expected_seq
            || (index > 0 && record.kind != CanonicalRecordKindV1::Transition)
            || record.bytes.len() > limits.max_object_bytes
            || !canonical_record_hash_matches(record)
            || record.previous_digest != previous
        {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::CanonicalRecord,
                    "canonical_record_mismatch",
                    &room.room_id,
                    "restore exact canonical record bytes and the complete predecessor chain",
                ),
            );
            break;
        }
        previous = Some(record.digest.clone());
    }
    let expected_head_seq = room.records.len().saturating_sub(1) as u64;
    if room.head.room_seq != expected_head_seq
        || room.head.lineage_digest != previous.unwrap_or_else(|| DigestV1::hash(&[]))
        || !image
            .manifest
            .expected_packs
            .iter()
            .any(|pack| pack.revision_digest == room.head.pack_revision_digest)
    {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::CanonicalRecord,
                "complete_head_mismatch",
                &room.room_id,
                "restore the Complete Head that binds the verified canonical lineage",
            ),
        );
    }
    if room.head.core_schema_version.is_empty()
        || room.materialization.core_state_bytes.len() > limits.max_object_bytes
        || room.materialization.activity_state_bytes.len() > limits.max_object_bytes
        || room.materialization.authoritative_state_bytes.len() > limits.max_object_bytes
        || !materialization_hash_matches(room)
    {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                ConsistencyClassV1::CanonicalRecord,
                "materialization_hash_mismatch",
                &room.room_id,
                "rebuild or restore the exact post-commit materialization bytes",
            ),
        );
    }
}

pub(crate) fn canonical_record_hash_matches(record: &CanonicalRecordV1) -> bool {
    if DigestV1::hash(&record.bytes) == record.digest {
        return true;
    }
    match record.kind {
        CanonicalRecordKindV1::Genesis => {
            native_sqlite::canonical_genesis_hash(&record.bytes).as_ref() == Some(&record.digest)
        }
        CanonicalRecordKindV1::Transition => {
            native_sqlite::canonical_transition_hash(&record.bytes).as_ref() == Some(&record.digest)
        }
    }
}

pub(crate) fn materialization_hash_matches(room: &RoomImageV1) -> bool {
    let pack_digest = room.head.pack_revision_digest.as_str();
    let core = DigestV1::hash(&room.materialization.core_state_bytes)
        == room.head.core_state_digest
        || native_sqlite::canonical_core_materialization_hash(
            &room.materialization.core_state_bytes,
            pack_digest,
        )
        .as_ref()
            == Some(&room.head.core_state_digest);
    let activity = DigestV1::hash(&room.materialization.activity_state_bytes)
        == room.head.activity_state_digest
        || native_sqlite::canonical_activity_materialization_hash(
            &room.materialization.activity_state_bytes,
            pack_digest,
        )
        .as_ref()
            == Some(&room.head.activity_state_digest);
    let authoritative = DigestV1::hash(&room.materialization.authoritative_state_bytes)
        == room.head.authoritative_state_digest
        || native_sqlite::canonical_authoritative_materialization_hash(
            &room.materialization.authoritative_state_bytes,
            pack_digest,
            &room.head.core_state_digest,
            &room.head.activity_state_digest,
        )
        .as_ref()
            == Some(&room.head.authoritative_state_digest);
    core && activity && authoritative
}

fn check_receipts(
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    check_row_bound(
        image.receipts.len(),
        "semantic receipts",
        ConsistencyClassV1::Receipt,
        report,
        limits,
    );
    check_receipt_rows(&image.receipts, false, image, report, limits);
    check_row_bound(
        image.activation_receipts.len(),
        "Activation operation receipts",
        ConsistencyClassV1::Activation,
        report,
        limits,
    );
    check_receipt_rows(&image.activation_receipts, true, image, report, limits);
}

fn check_receipt_rows(
    receipts: &[ReceiptV1],
    activation: bool,
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    let mut identities = BTreeSet::new();
    for receipt in receipts {
        let class = if activation {
            ConsistencyClassV1::Activation
        } else {
            ConsistencyClassV1::Receipt
        };
        let kind_invalid = if activation {
            receipt.kind != ReceiptKindV1::ActivationOperation
        } else {
            receipt.kind != ReceiptKindV1::Semantic
        };
        let activation_relation_invalid = if activation {
            match (
                receipt.operation_kind.as_deref(),
                receipt.activation_id.as_ref(),
            ) {
                (Some("offer"), None) => false,
                (Some("claim" | "renew" | "complete" | "release"), Some(activation_id)) => !image
                    .activations
                    .iter()
                    .any(|activation| &activation.activation_id == activation_id),
                _ => true,
            }
        } else {
            receipt.activation_id.is_some() || receipt.operation_kind.is_some()
        };
        let request_invalid = if receipt.request_bytes_available {
            DigestV1::hash(&receipt.request_bytes) != receipt.request_digest
        } else {
            !receipt.request_bytes.is_empty()
        };
        if !identities.insert(&receipt.identity_bytes)
            || receipt.identity_bytes.is_empty()
            || kind_invalid
            || activation_relation_invalid
            || receipt.request_bytes.len() > limits.max_object_bytes
            || receipt.result_bytes.len() > limits.max_object_bytes
            || request_invalid
            || DigestV1::hash(&receipt.result_bytes) != receipt.result_digest
            || receipt
                .room_id
                .as_ref()
                .is_some_and(|room_id| !image.rooms.iter().any(|room| &room.room_id == room_id))
            || receipt.transition_seq.is_some_and(|seq| {
                receipt.room_id.is_none() || !room_has_seq(image, receipt.room_id.as_deref(), seq)
            })
        {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    class,
                    "receipt_consistency_mismatch",
                    if activation {
                        "Activation receipt"
                    } else {
                        "semantic receipt"
                    },
                    "restore the exact immutable operation identity, request, result, and Room relation",
                ),
            );
        }
    }
}

fn check_timers(image: &BackupImageV1, report: &mut VerificationReportV1, limits: &VerifierLimits) {
    check_row_bound(
        image.timers.len(),
        "timers",
        ConsistencyClassV1::Timer,
        report,
        limits,
    );
    let mut identities = BTreeSet::new();
    let mut scheduled_ids = BTreeSet::new();
    for timer in &image.timers {
        let duplicate = !identities.insert((&timer.room_id, &timer.timer_id, timer.generation));
        let duplicate_scheduled = matches!(timer.state, TimerStateV1::Scheduled)
            && !scheduled_ids.insert((&timer.room_id, &timer.timer_id));
        let relation = !room_has_seq(
            image,
            Some(&timer.room_id),
            timer.fired_transition_seq.unwrap_or(0),
        );
        let invalid = duplicate
            || duplicate_scheduled
            || timer.room_id.is_empty()
            || timer.timer_id.is_empty()
            || timer.generation == 0
            || timer.scheduled_for.is_empty()
            || timer.payload_bytes.len() > limits.max_object_bytes
            || DigestV1::hash(&timer.payload_bytes) != timer.payload_digest
            || (matches!(timer.state, TimerStateV1::Fired)
                && (timer.fired_transition_seq.is_none() || relation))
            || (!matches!(timer.state, TimerStateV1::Fired)
                && timer.fired_transition_seq.is_some());
        if invalid {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Timer,
                    "timer_consistency_mismatch",
                    "timer identity",
                    "restore exact timer IDs, generations, scheduled_for values, and consuming relations",
                ),
            );
        }
    }
}

fn check_frames(image: &BackupImageV1, report: &mut VerificationReportV1, limits: &VerifierLimits) {
    check_row_bound(
        image.frames.len(),
        "Frames",
        ConsistencyClassV1::Frame,
        report,
        limits,
    );
    let mut identities = BTreeSet::new();
    for frame in &image.frames {
        let invalid = !identities.insert((&frame.room_id, &frame.member_id, frame.frame_seq))
            || frame.room_id.is_empty()
            || frame.member_id.is_empty()
            || frame.frame_seq == 0
            || frame.cause_room_seq == 0
            || !room_has_seq(image, Some(&frame.room_id), frame.cause_room_seq)
            || frame.payload_bytes.len() > limits.max_object_bytes
            || DigestV1::hash(&frame.payload_bytes) != frame.payload_digest
            || frame.frame_seq > frame.frame_head
            || frame.cursor.is_some_and(|cursor| cursor > frame.frame_head);
        if invalid {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Frame,
                    "frame_consistency_mismatch",
                    "observation frame identity",
                    "restore exact Membership frame sequence, causal sequence, payload, and Cursor witness",
                ),
            );
        }
    }
}

fn check_activations(
    image: &BackupImageV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    check_row_bound(
        image.activations.len(),
        "Activation Intents",
        ConsistencyClassV1::Activation,
        report,
        limits,
    );
    let mut identities = BTreeSet::new();
    for activation in &image.activations {
        let leased = matches!(activation.state, ActivationStateV1::Leased);
        let has_runner_claim = activation.runner_id.is_some() && activation.claim_id.is_some();
        let partial_runner_claim = activation.runner_id.is_some() != activation.claim_id.is_some();
        let context_ok = match (&activation.context_digest, &activation.context) {
            (None, ContextRetentionV1::None) | (Some(_), ContextRetentionV1::Tombstone) => true,
            (Some(digest), ContextRetentionV1::Retained(bytes)) => {
                bytes.len() <= limits.max_object_bytes && DigestV1::hash(bytes) == *digest
            }
            _ => false,
        };
        let invalid = !identities.insert(&activation.activation_id)
            || activation.activation_id.is_empty()
            || activation.room_id.is_empty()
            || activation.target_member_id.is_empty()
            || activation.cause_room_seq == 0
            || !room_has_seq(image, Some(&activation.room_id), activation.cause_room_seq)
            || activation.intent_generation == 0
            || partial_runner_claim
            || (leased
                && (!has_runner_claim
                    || activation.lease_until.is_none()
                    || activation.lease_generation == 0))
            || (!leased
                && (activation.lease_until.is_some()
                    || (has_runner_claim
                        && !matches!(activation.state, ActivationStateV1::Completed))))
            || !context_ok;
        if invalid {
            push_diagnostic(
                report,
                limits.max_diagnostics,
                DiagnosticV1::blocking(
                    ConsistencyClassV1::Activation,
                    "activation_consistency_mismatch",
                    "Activation Intent",
                    "restore exact intent, lease-generation, witness, and context-retention relations",
                ),
            );
        }
    }
}

fn check_row_bound(
    count: usize,
    what: &'static str,
    class: ConsistencyClassV1,
    report: &mut VerificationReportV1,
    limits: &VerifierLimits,
) {
    if count > limits.max_ledger_rows {
        push_diagnostic(
            report,
            limits.max_diagnostics,
            DiagnosticV1::blocking(
                class,
                "ledger_bound_exceeded",
                what,
                "restore into a bounded target or use a reviewed larger verifier limit",
            ),
        );
    }
}

fn room_has_seq(image: &BackupImageV1, room_id: Option<&str>, seq: u64) -> bool {
    let Some(room_id) = room_id else {
        return false;
    };
    image
        .rooms
        .iter()
        .find(|room| room.room_id == room_id)
        .is_some_and(|room| {
            seq > 0
                && seq <= room.head.room_seq
                && room.records.iter().any(|record| {
                    record.kind == CanonicalRecordKindV1::Transition && record.room_seq == seq
                })
        })
}

fn unique_packs(packs: &[PackIdentityV1]) -> Result<(), MetadataError> {
    let mut ids = BTreeSet::new();
    for pack in packs {
        if !ids.insert((&pack.pack_id, &pack.revision_digest)) {
            return Err(MetadataError::DuplicateIdentity(format!(
                "{}@{}",
                pack.pack_id,
                pack.revision_digest.as_str()
            )));
        }
    }
    Ok(())
}

fn unique_resources(resources: &[ResourceIdentityV1]) -> Result<(), MetadataError> {
    let mut ids = BTreeSet::new();
    for resource in resources {
        if !ids.insert(&resource.resource_id) {
            return Err(MetadataError::DuplicateIdentity(
                resource.resource_id.clone(),
            ));
        }
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), MetadataError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(MetadataError::InvalidText);
    }
    Ok(())
}

fn validate_deployment_lineage(value: &str) -> Result<(), MetadataError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_DEPLOYMENT_LINEAGE_BYTES
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || bytes.iter().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'/'))
        })
    {
        return Err(MetadataError::InvalidText);
    }
    Ok(())
}

fn bounded_len(count: usize, limit: usize, what: &'static str) -> Result<(), MetadataError> {
    if count > limit {
        return Err(MetadataError::BoundExceeded { what });
    }
    Ok(())
}

fn push_diagnostic(report: &mut VerificationReportV1, limit: usize, diagnostic: DiagnosticV1) {
    if diagnostic.disposition == DiagnosticDispositionV1::Blocking {
        report.blocking_detected = true;
    }
    if report.diagnostics.len() < limit {
        report.diagnostics.push(diagnostic);
    }
}

fn redact_subject(subject: &str) -> String {
    let digest = blake3::hash(subject.as_bytes()).to_hex();
    format!("subject:{}", &digest[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACK_DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const GLOBAL_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn digest(value: &str) -> DigestV1 {
        DigestV1::parse(value.to_owned()).unwrap_or_else(|_| DigestV1::hash(value.as_bytes()))
    }

    fn migration_contract() -> MigrationContractV1 {
        MigrationContractV1 {
            logical_history_id: "worldstream-storage-v1".to_owned(),
            schema_contract_fingerprint: digest(PACK_DIGEST),
            records: vec![MigrationIdentityV1 {
                version: 1,
                migration_id: "0001-initial-storage-schema".to_owned(),
                checksum: digest(GLOBAL_DIGEST),
            }],
        }
    }

    fn manifest() -> BackupManifestV1 {
        BackupManifestV1 {
            schema: BACKUP_MANIFEST_SCHEMA_V1.to_owned(),
            backup_id: "backup-imo-52".to_owned(),
            deployment_lineage: "deployment/test".to_owned(),
            storage_epoch: 7,
            backend: BackendProfileV1::SqliteBundled,
            native_point: BackendNativePointV1::SqliteOnlineBackup {
                engine_identity: "sqlite-3.53.4-bundled".to_owned(),
                point_id: "point-1".to_owned(),
            },
            migration_contract: migration_contract(),
            expected_packs: vec![PackIdentityV1 {
                pack_id: "worldstream.counter".to_owned(),
                revision_digest: digest(PACK_DIGEST),
                executor_digest: digest(GLOBAL_DIGEST),
                schema_bundle_digest: digest(PACK_DIGEST),
                codec_bundle_digest: digest(GLOBAL_DIGEST),
                resource_ids: vec!["counter-executor".to_owned()],
            }],
            expected_resources: vec![ResourceIdentityV1 {
                resource_id: "counter-executor".to_owned(),
                kind: "executor".to_owned(),
                byte_len: 8,
                digest: DigestV1::hash(b"executor"),
            }],
            expected_global_digest: digest(GLOBAL_DIGEST),
        }
    }

    #[test]
    fn manifest_deployment_identity_matches_runtime_boundaries() {
        let mut manifest = manifest();
        manifest.deployment_lineage = "a".repeat(MAX_DEPLOYMENT_LINEAGE_BYTES);
        manifest.storage_epoch = MAX_SAFE_INTEGER;
        assert!(manifest.validate(&VerifierLimits::default()).is_ok());

        manifest.deployment_lineage = "a".repeat(MAX_DEPLOYMENT_LINEAGE_BYTES + 1);
        assert_eq!(
            manifest.validate(&VerifierLimits::default()),
            Err(MetadataError::InvalidText)
        );

        manifest.deployment_lineage = "deployment/test".to_owned();
        manifest.storage_epoch = MAX_SAFE_INTEGER + 1;
        assert_eq!(
            manifest.validate(&VerifierLimits::default()),
            Err(MetadataError::ZeroGeneration("storage_epoch"))
        );
    }

    #[test]
    fn manifest_retains_distinct_revisions_of_one_pack() {
        let mut manifest = manifest();
        let mut retained_revision = manifest.expected_packs[0].clone();
        retained_revision.revision_digest = DigestV1::hash(b"retained counter revision");
        manifest.expected_packs.push(retained_revision.clone());
        assert!(manifest.validate(&VerifierLimits::default()).is_ok());

        manifest.expected_packs.push(retained_revision);
        assert!(matches!(
            manifest.validate(&VerifierLimits::default()),
            Err(MetadataError::DuplicateIdentity(_))
        ));
    }

    fn room(status: IntegrityStatusV1) -> RoomImageV1 {
        let genesis_bytes = b"genesis".to_vec();
        let transition_bytes = b"transition".to_vec();
        let genesis_digest = DigestV1::hash(&genesis_bytes);
        let transition_digest = DigestV1::hash(&transition_bytes);
        let core = b"core".to_vec();
        let activity = b"activity".to_vec();
        let authoritative = b"authoritative".to_vec();
        RoomImageV1 {
            room_id: "room-1".to_owned(),
            integrity: IntegrityWitnessV1 {
                source_status: status,
                restored_status: status,
                source_generation: 2,
                restored_generation: 2,
                source_isolated: !matches!(status, IntegrityStatusV1::Healthy),
                restored_isolated: !matches!(status, IntegrityStatusV1::Healthy),
            },
            head: CompleteHeadV1 {
                room_id: "room-1".to_owned(),
                room_seq: 1,
                lineage_digest: transition_digest.clone(),
                core_schema_version: "worldstream/core/v1".to_owned(),
                pack_revision_digest: digest(PACK_DIGEST),
                core_state_digest: DigestV1::hash(&core),
                activity_state_digest: DigestV1::hash(&activity),
                authoritative_state_digest: DigestV1::hash(&authoritative),
            },
            records: vec![
                CanonicalRecordV1 {
                    kind: CanonicalRecordKindV1::Genesis,
                    room_seq: 0,
                    bytes: genesis_bytes,
                    digest: genesis_digest.clone(),
                    previous_digest: None,
                },
                CanonicalRecordV1 {
                    kind: CanonicalRecordKindV1::Transition,
                    room_seq: 1,
                    bytes: transition_bytes,
                    digest: transition_digest,
                    previous_digest: Some(genesis_digest),
                },
            ],
            materialization: MaterializationV1 {
                core_state_bytes: core,
                activity_state_bytes: activity,
                authoritative_state_bytes: authoritative,
            },
            source_bytes_digest: digest(GLOBAL_DIGEST),
            restored_bytes_digest: digest(GLOBAL_DIGEST),
        }
    }

    fn clean_image() -> BackupImageV1 {
        let manifest = manifest();
        BackupImageV1 {
            restored_native_point: manifest.native_point.clone(),
            migrations: manifest.migration_contract.clone(),
            resources: vec![ResourceBlobV1 {
                resource_id: "counter-executor".to_owned(),
                bytes: b"executor".to_vec(),
            }],
            rooms: vec![room(IntegrityStatusV1::Healthy)],
            receipts: Vec::new(),
            timers: Vec::new(),
            frames: Vec::new(),
            activations: Vec::new(),
            activation_receipts: Vec::new(),
            source_global_digest: digest(GLOBAL_DIGEST),
            restored_global_digest: digest(GLOBAL_DIGEST),
            manifest,
        }
    }

    #[test]
    fn clean_fixture_is_ready() {
        let report = verify_restore(&clean_image(), VerifierLimits::default());
        assert!(report.is_ready());
        assert_eq!(
            report.rooms.get("room-1"),
            Some(&RoomDispositionV1::Verified)
        );
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn missing_resource_fixture_blocks_readiness() {
        let mut image = clean_image();
        image.resources.clear();
        let report = verify_restore(&image, VerifierLimits::default());
        assert_eq!(report.readiness, ReadinessV1::NotReady);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "resource_missing"
                && diagnostic.class == ConsistencyClassV1::Resource
        }));
    }

    #[test]
    fn new_mismatch_fixture_blocks_even_when_room_was_previously_unhealthy() {
        let mut image = clean_image();
        image.rooms[0].integrity.source_status = IntegrityStatusV1::Faulted;
        image.rooms[0].integrity.restored_status = IntegrityStatusV1::Faulted;
        image.rooms[0].integrity.source_isolated = true;
        image.rooms[0].integrity.restored_isolated = true;
        image.rooms[0].restored_bytes_digest = digest(PACK_DIGEST);
        let report = verify_restore(&image, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "new_room_bytes_mismatch")
        );
    }

    #[test]
    fn global_corruption_fixture_blocks_readiness() {
        let mut image = clean_image();
        image.restored_global_digest = digest(PACK_DIGEST);
        let report = verify_restore(&image, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "global_lineage_mismatch")
        );
    }

    #[test]
    fn invalid_operational_relation_fixture_is_actionable() {
        let mut image = clean_image();
        image.frames.push(FrameV1 {
            room_id: "room-1".to_owned(),
            member_id: "member-1".to_owned(),
            frame_seq: 1,
            cause_room_seq: 1,
            payload_bytes: b"frame".to_vec(),
            payload_digest: DigestV1::hash(b"different"),
            frame_head: 1,
            cursor: Some(2),
        });
        let report = verify_restore(&image, VerifierLimits::default());
        assert!(!report.is_ready());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.class == ConsistencyClassV1::Frame
                && diagnostic.code == "frame_consistency_mismatch"
        }));
    }

    #[test]
    fn permitted_pre_existing_isolation_does_not_block_otherwise_clean_deployment() {
        let mut image = clean_image();
        image.rooms[0] = room(IntegrityStatusV1::Quarantined);
        let report = verify_restore(&image, VerifierLimits::default());
        assert!(report.is_ready());
        assert_eq!(
            report.rooms.get("room-1"),
            Some(&RoomDispositionV1::IsolatedPreExisting)
        );
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.disposition == DiagnosticDispositionV1::PermittedPreExistingIsolation
        }));
    }

    #[test]
    fn completed_activation_retains_its_fence_without_being_a_live_lease() {
        let mut image = clean_image();
        image.activations.push(ActivationV1 {
            activation_id: "activation-1".to_owned(),
            room_id: "room-1".to_owned(),
            cause_room_seq: 1,
            target_member_id: "member-1".to_owned(),
            state: ActivationStateV1::Completed,
            intent_generation: 2,
            lease_generation: 3,
            runner_id: Some("runner-1".to_owned()),
            claim_id: Some("claim-1".to_owned()),
            lease_until: None,
            context_digest: None,
            context: ContextRetentionV1::None,
        });
        let report = verify_restore(&image, VerifierLimits::default());
        assert!(report.is_ready());
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn verifier_is_side_effect_free_and_diagnostics_are_redacted() {
        let image = clean_image();
        let before = serde_json::to_vec(&image).unwrap_or_default();
        let report = verify_restore(&image, VerifierLimits::default());
        let after = serde_json::to_vec(&image).unwrap_or_default();
        assert_eq!(before, after);
        assert!(
            report
                .diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.subject.contains("room-1"))
        );
    }
}
