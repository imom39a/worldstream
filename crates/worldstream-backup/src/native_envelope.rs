//! Versioned native `SQLite` backup envelope and evidence adapter.
//!
//! `SQLite` is authoritative for canonical records, materializations, integrity,
//! membership, and the durable operational ledgers.  A small companion is
//! necessary for facts deliberately kept outside the database (installed pack
//! bytes, resource bytes, request bytes that are stored only by hash, fired
//! timer causality, authoritative-state bytes, and native point witnesses).
//! This module binds those inputs to an exact source/target extraction before
//! constructing the provider-neutral [`BackupImageV1`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_core::{CanonicalJsonV1, CompleteHeadV1 as CoreCompleteHead};
use worldstream_pack_bundle::{RetainedPackBundleArtifactV1, VerifiedPackBundleV1};

use crate::native_sqlite::{
    NativeSqliteCanonicalRecordV1, NativeSqliteOperationalRowsV1, NativeSqliteRestoreEvidenceV1,
    NativeSqliteRowV1, NativeSqliteValueV1,
};
use crate::{
    BACKUP_MANIFEST_SCHEMA_V1, BackendNativePointV1, BackendProfileV1, BackupImageV1,
    CanonicalRecordKindV1, CanonicalRecordV1, CompleteHeadV1, ContextRetentionV1, DigestV1,
    FrameV1, IntegrityStatusV1, IntegrityWitnessV1, MaterializationV1, MigrationContractV1,
    NativeRestoreEvidenceV1, NativeRestoreRoomMembershipV1, NativeRestoreTargetEvidenceV1,
    NativeSqliteCaptureWitnessV1, ReceiptKindV1, ReceiptV1, ResourceBlobV1, RoomImageV1,
    TimerStateV1, TimerV1, VerifierLimits,
};

/// The only companion envelope schema accepted by the native `SQLite` adapter.
pub const NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1: &str =
    "worldstream/native-sqlite-backup-envelope/v1";

const MAX_ENVELOPE_TEXT: usize = 512;
const REQUIRED_COVERAGE_TABLES: [&str; 22] = [
    "retired_authority_fences_v1",
    "principals",
    "runners",
    "capabilities",
    "capability_scopes",
    "runner_capability_memberships",
    "authority_change_receipts",
    "authority_audit",
    "room_operational_history_roots_v2",
    "room_operational_mmr_receipts_v1",
    "room_operational_mmr_nodes_v1",
    "room_integrity",
    "room_members",
    "timers",
    "observation_frames",
    "observation_consequences",
    "activation_decisions",
    "activation_intents",
    "activation_operation_receipts",
    "semantic_receipts",
    "external_input_preparations",
    "integrity_incidents",
];

const TRUSTED_COMPATIBILITY_JSON_DIGEST: &str =
    "11f4d608579a4e9fc9d78183eaa389095525c8674ecbedeba5da714b8da9ed1d";

type RequestKey = (NativeSqliteRequestLedgerV1, Vec<u8>);
type RequestMap = BTreeMap<RequestKey, Vec<u8>>;
type TimerKey = (String, String, u64);
type TimerRelationMap = BTreeMap<TimerKey, Option<u64>>;

/// Origin and declared coverage of a companion envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteEnvelopeOriginV1 {
    /// The bounded adapter or operator name that produced the envelope.
    pub producer: String,
    /// Opaque capture identity, never used as a database identity.
    pub capture_id: String,
    /// Human-readable declaration of the exact bounded coverage.
    pub coverage: String,
}

/// Source/target native witness carried by the companion envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteNativeWitnessV1 {
    /// Backend profile observed by the adapter.
    pub backend: BackendProfileV1,
    /// Opaque native point identity observed at this side of the restore.
    pub native_point: BackendNativePointV1,
    /// Explicit deployment lineage read from canonical export metadata.
    pub deployment_lineage: String,
    /// Explicit storage epoch read from canonical export metadata.
    pub storage_epoch: u64,
    /// Digest of the complete bounded native extraction.
    pub evidence_digest: DigestV1,
    /// Digest of every exact `room_members` row, including all columns.
    pub membership_digest: DigestV1,
}

/// A request byte set that the native schema intentionally stores only by hash.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum NativeSqliteRequestLedgerV1 {
    /// A semantic Room operation receipt.
    Semantic,
    /// An Activation operation receipt.
    Activation,
}

/// Exact companion request bytes and their native hash witness.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteRequestWitnessV1 {
    /// Ledger in which the request identity is unique.
    pub ledger: NativeSqliteRequestLedgerV1,
    /// Exact operation identity bytes (or the UTF-8 operation ID for Activation).
    pub identity_bytes: Vec<u8>,
    /// Exact canonical request bytes absent from the native ledger.
    pub request_bytes: Vec<u8>,
    /// Native stored request hash.
    pub request_digest: DigestV1,
}

/// Companion evidence for authoritative-state bytes not persisted by `SQLite`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteAuthoritativeMaterializationV1 {
    /// Owning Room.
    pub room_id: String,
    /// Exact source-side bytes from the authoritative-state adapter.
    pub source_bytes: Vec<u8>,
    /// Exact target-side bytes re-read after restore by that adapter.
    pub restored_bytes: Vec<u8>,
    /// Source digest declared by the adapter.
    pub source_digest: DigestV1,
    /// Target digest declared by the adapter.
    pub restored_digest: DigestV1,
}

/// The missing relation needed to prove a Fired Timer consumed one transition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteTimerRelationV1 {
    /// Owning Room.
    pub room_id: String,
    /// Timer identity.
    pub timer_id: String,
    /// Timer generation.
    pub generation: u64,
    /// Consuming transition, required only for a Fired timer.
    pub fired_transition_seq: Option<u64>,
}

/// Exact table coverage bound into the envelope digest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteEnvelopeCoverageV1 {
    /// Fixed operational table set copied byte-for-byte.
    pub tables: Vec<String>,
    /// Source extraction digest.
    pub source_evidence_digest: DigestV1,
    /// Restored-target extraction digest.
    pub restored_evidence_digest: DigestV1,
    /// Source complete-membership digest.
    pub source_membership_digest: DigestV1,
    /// Restored complete-membership digest.
    pub restored_membership_digest: DigestV1,
}

/// Versioned, digest-sealed companion inputs for a native `SQLite` restore.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteBackupEnvelopeV1 {
    /// Envelope schema identifier.
    pub schema: String,
    /// Origin and coverage declaration.
    pub origin: NativeSqliteEnvelopeOriginV1,
    /// Immutable manifest expected by the provider-neutral verifier.
    pub manifest: crate::BackupManifestV1,
    /// Exact expected resource bytes.
    pub resources: Vec<ResourceBlobV1>,
    /// Exact original portable Pack archives required by retained Rooms.
    /// Approval and selectability are deliberately absent.
    #[serde(default)]
    pub pack_bundles: Vec<RetainedPackBundleArtifactV1>,
    /// Request bytes absent from native `SQLite` ledgers.
    pub request_witnesses: Vec<NativeSqliteRequestWitnessV1>,
    /// Authoritative-state bytes absent from native `SQLite` materializations.
    pub authoritative_materializations: Vec<NativeSqliteAuthoritativeMaterializationV1>,
    /// Fired-timer causal relations absent from the timer table.
    pub timer_relations: Vec<NativeSqliteTimerRelationV1>,
    /// Source native witness.
    pub source: NativeSqliteNativeWitnessV1,
    /// Restored-target native witness.
    pub restored: NativeSqliteNativeWitnessV1,
    /// Exact coverage and source/target extraction witnesses.
    pub coverage: NativeSqliteEnvelopeCoverageV1,
    /// BLAKE3 digest over this envelope with this field replaced by zero.
    pub envelope_digest: DigestV1,
}

/// Errors at the digest-sealed companion/native translation boundary.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum NativeSqliteEnvelopeError {
    /// Envelope schema or bounded metadata is invalid.
    #[error("native SQLite backup envelope is invalid")]
    InvalidEnvelope,
    /// The self-sealing digest does not match the envelope bytes.
    #[error("native SQLite backup envelope digest mismatch")]
    EnvelopeDigestMismatch,
    /// Native source and target evidence differ.
    #[error("native SQLite source and target evidence mismatch")]
    NativeEvidenceMismatch,
    /// A required companion fact is absent or duplicated.
    #[error("native SQLite backup companion coverage is incomplete")]
    IncompleteCompanion,
    /// A companion fact is not bound to its stored digest.
    #[error("native SQLite backup companion digest mismatch")]
    CompanionDigestMismatch,
    /// A native row cannot be translated without inventing a value.
    #[error("native SQLite backup row translation failed")]
    InvalidNativeRow,
    /// Canonical evidence cannot produce a complete Room image.
    #[error("native SQLite backup Room evidence is incomplete")]
    IncompleteRoom,
    /// The sealed global lineage digest is not the digest of the full image.
    #[error("native SQLite backup global lineage digest mismatch")]
    GlobalDigestMismatch,
    /// The serialized companion exceeds the reviewed bound.
    #[error("native SQLite backup envelope exceeds its bound")]
    EnvelopeBoundExceeded,
    /// The envelope was not accompanied by the witness minted by the actual
    /// bundled online-backup operation.
    #[error("native SQLite backup capture witness is missing or does not match")]
    CaptureWitnessMismatch,
    /// Pack/resource facts are not covered by the checked-in compatibility
    /// authority and therefore cannot become restore evidence by resealing.
    #[error("native SQLite companion facts are not trusted by the compatibility authority")]
    UntrustedCompanion,
    /// A carried portable Pack archive is malformed, substituted, duplicated,
    /// unreferenced, or absent for a retained portable revision.
    #[error("native SQLite companion portable Pack Bundle set is invalid")]
    InvalidPackBundles,
}

impl NativeSqliteBackupEnvelopeV1 {
    /// Validates envelope metadata, coverage, and the self-sealing digest.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the schema, companion coverage, or digest
    /// seal is invalid.
    pub fn validate(&self, limits: VerifierLimits) -> Result<(), NativeSqliteEnvelopeError> {
        if self.schema != NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1
            || self.manifest.schema != BACKUP_MANIFEST_SCHEMA_V1
            || self.manifest.validate(&limits).is_err()
            || !valid_text(&self.origin.producer)
            || !valid_text(&self.origin.capture_id)
            || !valid_text(&self.origin.coverage)
            || self.coverage.tables
                != REQUIRED_COVERAGE_TABLES
                    .iter()
                    .map(|table| (*table).to_owned())
                    .collect::<Vec<_>>()
            || self.source.backend != BackendProfileV1::SqliteBundled
            || self.restored.backend != BackendProfileV1::SqliteBundled
            || self.source.native_point != self.manifest.native_point
            || self.restored.native_point != self.manifest.native_point
            || self.source.backend != self.restored.backend
            || self.source.native_point != self.restored.native_point
            || self.source.deployment_lineage != self.manifest.deployment_lineage
            || self.restored.deployment_lineage != self.manifest.deployment_lineage
            || self.source.storage_epoch != self.manifest.storage_epoch
            || self.restored.storage_epoch != self.manifest.storage_epoch
        {
            return Err(NativeSqliteEnvelopeError::InvalidEnvelope);
        }
        if self.envelope_bytes()?.len() > limits.max_object_bytes.saturating_mul(4) {
            return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
        }
        let mut unsigned = self.clone();
        unsigned.envelope_digest = DigestV1::hash(&[]);
        if DigestV1::hash(&unsigned.envelope_bytes()?) != self.envelope_digest {
            return Err(NativeSqliteEnvelopeError::EnvelopeDigestMismatch);
        }
        validate_resources(&self.resources, &self.manifest.expected_resources, limits)?;
        validate_pack_bundles(&self.pack_bundles, &self.manifest, limits)?;
        validate_requests(&self.request_witnesses, limits)?;
        validate_authoritative(&self.authoritative_materializations, limits)?;
        validate_timer_relations(&self.timer_relations, limits)?;
        Ok(())
    }

    /// Constructs complete provider-neutral restore evidence from source and
    /// restored native extractions plus the bounded companion inputs.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the capture witness, native rows, or
    /// required companion fact is missing or inconsistent.
    pub fn build_native_restore_evidence(
        &self,
        source: &NativeSqliteRestoreEvidenceV1,
        restored: &NativeSqliteRestoreEvidenceV1,
        capture: &NativeSqliteCaptureWitnessV1,
        limits: VerifierLimits,
    ) -> Result<NativeRestoreEvidenceV1, NativeSqliteEnvelopeError> {
        self.build_native_restore_evidence_inner(source, restored, capture, limits, true)
    }

    fn build_native_restore_evidence_inner(
        &self,
        source: &NativeSqliteRestoreEvidenceV1,
        restored: &NativeSqliteRestoreEvidenceV1,
        capture: &NativeSqliteCaptureWitnessV1,
        limits: VerifierLimits,
        check_global: bool,
    ) -> Result<NativeRestoreEvidenceV1, NativeSqliteEnvelopeError> {
        self.validate(limits)?;
        self.validate_capture(source, restored, capture)?;
        verify_native_witness(
            &self.source,
            source,
            &self.coverage.source_evidence_digest,
            &self.coverage.source_membership_digest,
        )?;
        verify_native_witness(
            &self.restored,
            restored,
            &self.coverage.restored_evidence_digest,
            &self.coverage.restored_membership_digest,
        )?;
        if source != restored {
            return Err(NativeSqliteEnvelopeError::NativeEvidenceMismatch);
        }
        if source.migration_metadata.is_none() {
            return Err(NativeSqliteEnvelopeError::IncompleteCompanion);
        }
        let migrations = migration_contract_from_native(source, &self.manifest.migration_contract)?;
        let mut request_map = request_map(&self.request_witnesses)?;
        let mut timer_map = timer_relation_map(&self.timer_relations)?;
        let authoritative = authoritative_map(&self.authoritative_materializations);
        let rooms = build_rooms(source, &authoritative, limits)?;
        let receipts = build_semantic_receipts(source, &mut request_map)?;
        let activation_receipts = build_activation_receipts(source, &mut request_map)?;
        let timers = build_timers(source, &mut timer_map)?;
        require_companion_maps_consumed(&request_map, &timer_map)?;
        let frames = build_frames(source)?;
        let activations = build_activations(source)?;
        let mut image = BackupImageV1 {
            manifest: self.manifest.clone(),
            restored_native_point: self.restored.native_point.clone(),
            migrations,
            resources: self.resources.clone(),
            rooms,
            receipts,
            timers,
            frames,
            activations,
            activation_receipts,
            source_global_digest: DigestV1::hash(&[]),
            restored_global_digest: DigestV1::hash(&[]),
        };
        let global_digest = image_global_digest(&image)?;
        if check_global && global_digest != self.manifest.expected_global_digest {
            return Err(NativeSqliteEnvelopeError::GlobalDigestMismatch);
        }
        image.source_global_digest = global_digest.clone();
        image.restored_global_digest = global_digest;
        let target = NativeRestoreTargetEvidenceV1 {
            backend: Some(self.restored.backend),
            deployment_lineage: Some(self.restored.deployment_lineage.clone()),
            storage_epoch: Some(self.restored.storage_epoch),
            native_point: Some(self.restored.native_point.clone()),
            migration_contract: Some(self.manifest.migration_contract.clone()),
            pack_identities: Some(self.manifest.expected_packs.clone()),
            resource_identities: Some(self.manifest.expected_resources.clone()),
            room_membership: Some(
                image
                    .rooms
                    .iter()
                    .map(|room| NativeRestoreRoomMembershipV1 {
                        room_id: room.room_id.clone(),
                        integrity: room.integrity.clone(),
                    })
                    .collect(),
            ),
            durable_domains: None,
        };
        Ok(NativeRestoreEvidenceV1::new(image, target))
    }

    /// Computes the digest used to seal a manifest's global lineage field.
    /// This helper is intended for an adapter that has already captured both
    /// source and target evidence; it never uses a path, timestamp, or page
    /// count as identity.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the capture witness or image evidence is
    /// incomplete or inconsistent.
    pub fn computed_global_digest(
        &self,
        source: &NativeSqliteRestoreEvidenceV1,
        restored: &NativeSqliteRestoreEvidenceV1,
        capture: &NativeSqliteCaptureWitnessV1,
        limits: VerifierLimits,
    ) -> Result<DigestV1, NativeSqliteEnvelopeError> {
        let mut candidate = self.clone();
        candidate.manifest.expected_global_digest = DigestV1::hash(&[]);
        candidate.envelope_digest = DigestV1::hash(&candidate.unsigned_envelope_bytes()?);
        let evidence = candidate
            .build_native_restore_evidence_inner(source, restored, capture, limits, false)?;
        image_global_digest(&evidence.image)
    }

    /// Seals a newly assembled envelope after source and target witnesses have
    /// been captured. This is an adapter-construction operation; verification
    /// itself never mutates an envelope.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the capture witness, trusted companion,
    /// or complete source/target evidence is invalid.
    pub fn seal(
        &mut self,
        source: &NativeSqliteRestoreEvidenceV1,
        restored: &NativeSqliteRestoreEvidenceV1,
        capture: &NativeSqliteCaptureWitnessV1,
        limits: VerifierLimits,
    ) -> Result<(), NativeSqliteEnvelopeError> {
        let global = self.computed_global_digest(source, restored, capture, limits)?;
        self.manifest.expected_global_digest = global;
        self.envelope_digest = DigestV1::hash(&self.unsigned_envelope_bytes()?);
        Ok(())
    }

    fn envelope_bytes(&self) -> Result<Vec<u8>, NativeSqliteEnvelopeError> {
        serde_json::to_vec(self).map_err(|_| NativeSqliteEnvelopeError::InvalidEnvelope)
    }

    fn unsigned_envelope_bytes(&self) -> Result<Vec<u8>, NativeSqliteEnvelopeError> {
        let mut unsigned = self.clone();
        unsigned.envelope_digest = DigestV1::hash(&[]);
        unsigned.envelope_bytes()
    }

    fn validate_capture(
        &self,
        source: &NativeSqliteRestoreEvidenceV1,
        restored: &NativeSqliteRestoreEvidenceV1,
        capture: &NativeSqliteCaptureWitnessV1,
    ) -> Result<(), NativeSqliteEnvelopeError> {
        let expected_point_id = format!(
            "sqlite-online-backup-v1:{}",
            capture.source_evidence_digest().as_str()
        );
        let point_is_minted = matches!(
            capture.native_point(),
            BackendNativePointV1::SqliteOnlineBackup {
                engine_identity,
                point_id
            } if engine_identity == capture.engine_version() && point_id == &expected_point_id
        );
        let origin_matches = self.origin.producer == "worldstream-native-sqlite-adapter"
            && self.origin.capture_id == expected_point_id;
        if capture.source_evidence_digest() != &native_evidence_digest(source)?
            || capture.target_evidence_digest() != &native_evidence_digest(restored)?
            || self.source.evidence_digest != *capture.source_evidence_digest()
            || self.restored.evidence_digest != *capture.target_evidence_digest()
            || self.manifest.native_point != *capture.native_point()
            || self.source.native_point != *capture.native_point()
            || self.restored.native_point != *capture.native_point()
            || !point_is_minted
            || !origin_matches
        {
            return Err(NativeSqliteEnvelopeError::CaptureWitnessMismatch);
        }
        validate_trusted_manifest(&self.manifest, &self.resources, &self.pack_bundles)?;
        Ok(())
    }
}

fn validate_trusted_manifest(
    manifest: &crate::BackupManifestV1,
    resources: &[ResourceBlobV1],
    pack_bundles: &[RetainedPackBundleArtifactV1],
) -> Result<(), NativeSqliteEnvelopeError> {
    let compatibility = include_bytes!("../../../compatibility.json");
    if DigestV1::hash(compatibility).as_str() != TRUSTED_COMPATIBILITY_JSON_DIGEST {
        return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
    }
    let root: serde_json::Value = serde_json::from_slice(compatibility)
        .map_err(|_| NativeSqliteEnvelopeError::UntrustedCompanion)?;
    validate_trusted_product_and_storage(&root, manifest)?;
    let pack_executors = root
        .get("pack_executors")
        .and_then(serde_json::Value::as_array)
        .ok_or(NativeSqliteEnvelopeError::UntrustedCompanion)?;
    validate_expected_pack_trust(manifest, pack_bundles, pack_executors)?;
    validate_portable_pack_inventory(manifest, pack_bundles)?;
    validate_expected_resource_trust(manifest, resources, pack_executors)
}

fn validate_trusted_product_and_storage(
    root: &serde_json::Value,
    manifest: &crate::BackupManifestV1,
) -> Result<(), NativeSqliteEnvelopeError> {
    if root
        .get("contracts")
        .and_then(|contracts| contracts.get("product"))
        .and_then(serde_json::Value::as_str)
        != Some(env!("CARGO_PKG_VERSION"))
    {
        return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
    }
    let storage = root
        .get("migrations")
        .ok_or(NativeSqliteEnvelopeError::UntrustedCompanion)?;
    if manifest.migration_contract.logical_history_id
        != storage
            .get("logical_history_id")
            .and_then(serde_json::Value::as_str)
            .ok_or(NativeSqliteEnvelopeError::UntrustedCompanion)?
        || manifest
            .migration_contract
            .schema_contract_fingerprint
            .as_str()
            != storage
                .get("schema_contract_fingerprint")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| value.strip_prefix("blake3:"))
                .ok_or(NativeSqliteEnvelopeError::UntrustedCompanion)?
    {
        return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
    }
    Ok(())
}

fn validate_expected_pack_trust(
    manifest: &crate::BackupManifestV1,
    pack_bundles: &[RetainedPackBundleArtifactV1],
    pack_executors: &[serde_json::Value],
) -> Result<(), NativeSqliteEnvelopeError> {
    if manifest.expected_packs.is_empty() {
        return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
    }
    for pack in &manifest.expected_packs {
        let embedded = pack_executors.iter().any(|candidate| {
            candidate.get("pack_id").and_then(serde_json::Value::as_str)
                == Some(pack.pack_id.as_str())
                && digest_field(candidate, "revision_digest", &pack.revision_digest)
                && digest_field(candidate, "executor_artifact_digest", &pack.executor_digest)
                && digest_field(
                    candidate,
                    "schema_bundle_digest",
                    &pack.schema_bundle_digest,
                )
                && digest_field(candidate, "codec_bundle_digest", &pack.codec_bundle_digest)
                && candidate.get("status").and_then(serde_json::Value::as_str) == Some("resolved")
                && candidate
                    .get("runnable_for_retained_rooms")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
        });
        let portable = pack_bundles.iter().any(|artifact| {
            artifact
                .verify()
                .is_ok_and(|verified| portable_pack_matches(pack, &verified))
        });
        if embedded == portable {
            return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
        }
        for resource_id in &pack.resource_ids {
            if !manifest
                .expected_resources
                .iter()
                .any(|resource| &resource.resource_id == resource_id)
            {
                return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
            }
        }
    }
    Ok(())
}

fn validate_portable_pack_inventory(
    manifest: &crate::BackupManifestV1,
    pack_bundles: &[RetainedPackBundleArtifactV1],
) -> Result<(), NativeSqliteEnvelopeError> {
    for artifact in pack_bundles {
        let verified = artifact
            .verify()
            .map_err(|_| NativeSqliteEnvelopeError::InvalidPackBundles)?;
        if !manifest
            .expected_packs
            .iter()
            .any(|pack| portable_pack_matches(pack, &verified))
        {
            return Err(NativeSqliteEnvelopeError::InvalidPackBundles);
        }
    }
    Ok(())
}

fn validate_expected_resource_trust(
    manifest: &crate::BackupManifestV1,
    resources: &[ResourceBlobV1],
    pack_executors: &[serde_json::Value],
) -> Result<(), NativeSqliteEnvelopeError> {
    for resource in &manifest.expected_resources {
        let trusted = pack_executors.iter().any(|candidate| {
            let field = match resource.kind.as_str() {
                "executor" => "executor_artifact_digest",
                "schema" => "schema_bundle_digest",
                "codec" => "codec_bundle_digest",
                _ => return false,
            };
            digest_field(candidate, field, &resource.digest)
        });
        if !trusted
            || !resources.iter().any(|blob| {
                blob.resource_id == resource.resource_id
                    && DigestV1::hash(&blob.bytes) == resource.digest
            })
        {
            return Err(NativeSqliteEnvelopeError::UntrustedCompanion);
        }
    }
    Ok(())
}

fn validate_pack_bundles(
    bundles: &[RetainedPackBundleArtifactV1],
    manifest: &crate::BackupManifestV1,
    limits: VerifierLimits,
) -> Result<(), NativeSqliteEnvelopeError> {
    if bundles.len() > limits.max_packs {
        return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
    }
    let mut previous = None;
    let mut physical = BTreeSet::new();
    let mut semantic = BTreeSet::new();
    let mut total_bytes = 0_usize;
    for artifact in bundles {
        let order = (
            artifact.revision_digest.to_string(),
            artifact.bundle_digest.to_string(),
        );
        if previous.as_ref().is_some_and(|previous| previous >= &order)
            || !semantic.insert(order.0.clone())
            || !physical.insert(order.1.clone())
        {
            return Err(NativeSqliteEnvelopeError::InvalidPackBundles);
        }
        previous = Some(order);
        total_bytes = total_bytes
            .checked_add(artifact.archive_bytes.len())
            .ok_or(NativeSqliteEnvelopeError::EnvelopeBoundExceeded)?;
        let verified = artifact
            .verify()
            .map_err(|_| NativeSqliteEnvelopeError::InvalidPackBundles)?;
        if !manifest
            .expected_packs
            .iter()
            .any(|pack| portable_pack_matches(pack, &verified))
        {
            return Err(NativeSqliteEnvelopeError::InvalidPackBundles);
        }
    }
    if total_bytes > limits.max_object_bytes.saturating_mul(4) {
        return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
    }
    Ok(())
}

fn portable_pack_matches(
    expected: &crate::PackIdentityV1,
    verified: &VerifiedPackBundleV1,
) -> bool {
    let descriptor = verified.descriptor();
    let lock = verified.revision_lock();
    expected.pack_id == descriptor.pack_id
        && strip_blake3(&verified.revision_digest().to_string())
            == Some(expected.revision_digest.as_str())
        && strip_blake3(&verified.component_digest().to_string())
            == Some(expected.executor_digest.as_str())
        && strip_blake3(&lock.schema_bundle_digest.to_string())
            == Some(expected.schema_bundle_digest.as_str())
        && strip_blake3(&lock.codec_bundle_digest.to_string())
            == Some(expected.codec_bundle_digest.as_str())
}

fn strip_blake3(value: &str) -> Option<&str> {
    value.strip_prefix("blake3:")
}

fn digest_field(value: &serde_json::Value, field: &str, expected: &DigestV1) -> bool {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.strip_prefix("blake3:"))
        == Some(expected.as_str())
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ENVELOPE_TEXT && !value.chars().any(char::is_control)
}

fn validate_resources(
    resources: &[ResourceBlobV1],
    expected: &[crate::ResourceIdentityV1],
    limits: VerifierLimits,
) -> Result<(), NativeSqliteEnvelopeError> {
    if resources.len() > limits.max_resources {
        return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
    }
    let mut seen = BTreeSet::new();
    for resource in resources {
        if !seen.insert(&resource.resource_id)
            || resource.bytes.len() > limits.max_object_bytes
            || !expected.iter().any(|item| {
                item.resource_id == resource.resource_id
                    && item.byte_len == resource.bytes.len() as u64
                    && item.digest == DigestV1::hash(&resource.bytes)
            })
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
    }
    if expected
        .iter()
        .any(|item| !seen.contains(&item.resource_id))
    {
        return Err(NativeSqliteEnvelopeError::IncompleteCompanion);
    }
    Ok(())
}

fn validate_requests(
    requests: &[NativeSqliteRequestWitnessV1],
    limits: VerifierLimits,
) -> Result<(), NativeSqliteEnvelopeError> {
    if requests.len() > limits.max_ledger_rows {
        return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
    }
    let mut identities = BTreeSet::new();
    for request in requests {
        if request.identity_bytes.is_empty()
            || request.request_bytes.len() > limits.max_object_bytes
            || DigestV1::hash(&request.request_bytes) != request.request_digest
            || !identities.insert((request.ledger, request.identity_bytes.clone()))
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
    }
    Ok(())
}

fn validate_authoritative(
    values: &[NativeSqliteAuthoritativeMaterializationV1],
    limits: VerifierLimits,
) -> Result<(), NativeSqliteEnvelopeError> {
    if values.len() > limits.max_rooms {
        return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
    }
    let mut ids = BTreeSet::new();
    for value in values {
        if value.room_id.is_empty()
            || value.source_bytes.len() > limits.max_object_bytes
            || value.restored_bytes.len() > limits.max_object_bytes
            || DigestV1::hash(&value.source_bytes) != value.source_digest
            || DigestV1::hash(&value.restored_bytes) != value.restored_digest
            || value.source_digest != value.restored_digest
            || value.source_bytes != value.restored_bytes
            || !ids.insert(&value.room_id)
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
    }
    Ok(())
}

fn validate_timer_relations(
    values: &[NativeSqliteTimerRelationV1],
    limits: VerifierLimits,
) -> Result<(), NativeSqliteEnvelopeError> {
    if values.len() > limits.max_ledger_rows {
        return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
    }
    let mut ids = BTreeSet::new();
    for value in values {
        if value.room_id.is_empty()
            || value.timer_id.is_empty()
            || value.generation == 0
            || !ids.insert((&value.room_id, &value.timer_id, value.generation))
            || value.fired_transition_seq.is_some_and(|seq| seq == 0)
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
    }
    Ok(())
}

fn verify_native_witness(
    witness: &NativeSqliteNativeWitnessV1,
    evidence: &NativeSqliteRestoreEvidenceV1,
    expected_evidence_digest: &DigestV1,
    expected_membership_digest: &DigestV1,
) -> Result<(), NativeSqliteEnvelopeError> {
    if witness.evidence_digest != *expected_evidence_digest
        || witness.evidence_digest != native_evidence_digest(evidence)?
        || witness.membership_digest != *expected_membership_digest
        || witness.membership_digest != native_membership_digest(&evidence.operational)?
        || evidence.deployment_lineage.as_ref() != Some(&witness.deployment_lineage)
        || evidence.storage_epoch != Some(witness.storage_epoch)
    {
        return Err(NativeSqliteEnvelopeError::NativeEvidenceMismatch);
    }
    Ok(())
}

/// Computes the digest of all extracted native values, including exact BLOBs.
///
/// # Errors
///
/// Returns an error if the bounded extraction cannot be serialized.
pub fn native_evidence_digest(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    serde_json::to_vec(evidence)
        .map(|bytes| DigestV1::hash(&bytes))
        .map_err(|_| NativeSqliteEnvelopeError::InvalidEnvelope)
}

/// Computes the exact-membership-table digest used by a native witness.
///
/// # Errors
///
/// Returns an error if the membership rows cannot be serialized.
pub fn native_membership_digest(
    operational: &NativeSqliteOperationalRowsV1,
) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    let rows = operational.tables.get("room_members");
    serde_json::to_vec(&rows)
        .map(|bytes| DigestV1::hash(&bytes))
        .map_err(|_| NativeSqliteEnvelopeError::InvalidEnvelope)
}

fn migration_contract_from_native(
    evidence: &NativeSqliteRestoreEvidenceV1,
    expected: &MigrationContractV1,
) -> Result<MigrationContractV1, NativeSqliteEnvelopeError> {
    let rows = evidence
        .migration_metadata
        .as_ref()
        .ok_or(NativeSqliteEnvelopeError::IncompleteCompanion)?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        if row.values.len() != 3 {
            return Err(NativeSqliteEnvelopeError::InvalidNativeRow);
        }
        let version = integer(&row.values, 0)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?;
        let migration_id = text(&row.values, 1)
            .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
            .to_owned();
        let checksum = digest_blob_or_text(&row.values, 2)?;
        records.push(crate::MigrationIdentityV1 {
            version,
            migration_id,
            checksum,
        });
    }
    let actual = MigrationContractV1 {
        logical_history_id: expected.logical_history_id.clone(),
        schema_contract_fingerprint: expected.schema_contract_fingerprint.clone(),
        records,
    };
    if &actual != expected {
        return Err(NativeSqliteEnvelopeError::NativeEvidenceMismatch);
    }
    Ok(actual)
}

fn request_map(
    requests: &[NativeSqliteRequestWitnessV1],
) -> Result<RequestMap, NativeSqliteEnvelopeError> {
    let mut result = BTreeMap::new();
    for request in requests {
        if result
            .insert(
                (request.ledger, request.identity_bytes.clone()),
                request.request_bytes.clone(),
            )
            .is_some()
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
    }
    Ok(result)
}

fn timer_relation_map(
    relations: &[NativeSqliteTimerRelationV1],
) -> Result<TimerRelationMap, NativeSqliteEnvelopeError> {
    let mut result = BTreeMap::new();
    for relation in relations {
        if result
            .insert(
                (
                    relation.room_id.clone(),
                    relation.timer_id.clone(),
                    relation.generation,
                ),
                relation.fired_transition_seq,
            )
            .is_some()
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
    }
    Ok(result)
}

fn require_companion_maps_consumed(
    requests: &RequestMap,
    timers: &TimerRelationMap,
) -> Result<(), NativeSqliteEnvelopeError> {
    if !requests.is_empty() || !timers.is_empty() {
        return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
    }
    Ok(())
}

fn authoritative_map(
    values: &[NativeSqliteAuthoritativeMaterializationV1],
) -> BTreeMap<String, NativeSqliteAuthoritativeMaterializationV1> {
    values
        .iter()
        .map(|value| (value.room_id.clone(), value.clone()))
        .collect()
}

fn build_rooms(
    evidence: &NativeSqliteRestoreEvidenceV1,
    authoritative: &BTreeMap<String, NativeSqliteAuthoritativeMaterializationV1>,
    limits: VerifierLimits,
) -> Result<Vec<RoomImageV1>, NativeSqliteEnvelopeError> {
    if evidence.room_heads.len() > limits.max_rooms
        || evidence.room_heads.len() != evidence.integrity.len()
        || evidence.room_heads.keys().any(|room_id| {
            !evidence.canonical_records.contains_key(room_id)
                || !evidence.materializations.contains_key(room_id)
                || !authoritative.contains_key(room_id)
        })
        || evidence
            .canonical_records
            .keys()
            .any(|room_id| !evidence.room_heads.contains_key(room_id))
        || authoritative
            .keys()
            .any(|room_id| !evidence.room_heads.contains_key(room_id))
    {
        return Err(NativeSqliteEnvelopeError::IncompleteRoom);
    }
    let mut rooms = Vec::with_capacity(evidence.room_heads.len());
    for (room_id, stored_head) in &evidence.room_heads {
        let integrity = evidence
            .integrity
            .get(room_id)
            .ok_or(NativeSqliteEnvelopeError::IncompleteRoom)?;
        let status = integrity_status(&integrity.0)?;
        let head = backup_head(&stored_head.complete_head_bytes)?;
        let records = evidence
            .canonical_records
            .get(room_id)
            .ok_or(NativeSqliteEnvelopeError::IncompleteRoom)?
            .iter()
            .map(backup_record)
            .collect::<Vec<_>>();
        if records.len() > limits.max_records_per_room {
            return Err(NativeSqliteEnvelopeError::EnvelopeBoundExceeded);
        }
        let (core, activity) = evidence
            .materializations
            .get(room_id)
            .cloned()
            .ok_or(NativeSqliteEnvelopeError::IncompleteRoom)?;
        let auth = authoritative
            .get(room_id)
            .ok_or(NativeSqliteEnvelopeError::IncompleteRoom)?;
        let source_room_digest = room_evidence_digest(evidence, room_id, &auth.source_bytes)?;
        let restored_room_digest = room_evidence_digest(evidence, room_id, &auth.restored_bytes)?;
        if source_room_digest != restored_room_digest
            || stored_head.room_seq != head.room_seq
            || stored_head.head_digest != head.lineage_digest
            || stored_head.core_schema_version != head.core_schema_version
            || stored_head.pack_digest != head.pack_revision_digest
            || stored_head.core_state_digest != head.core_state_digest
            || stored_head.activity_state_digest != head.activity_state_digest
            || stored_head.authoritative_state_digest != head.authoritative_state_digest
        {
            return Err(NativeSqliteEnvelopeError::NativeEvidenceMismatch);
        }
        rooms.push(RoomImageV1 {
            room_id: room_id.clone(),
            integrity: IntegrityWitnessV1 {
                source_status: status,
                restored_status: status,
                source_generation: integrity.1,
                restored_generation: integrity.1,
                source_isolated: status != IntegrityStatusV1::Healthy,
                restored_isolated: status != IntegrityStatusV1::Healthy,
            },
            head,
            records,
            materialization: MaterializationV1 {
                core_state_bytes: core,
                activity_state_bytes: activity,
                authoritative_state_bytes: auth.source_bytes.clone(),
            },
            source_bytes_digest: source_room_digest,
            restored_bytes_digest: restored_room_digest,
        });
    }
    Ok(rooms)
}

fn backup_record(record: &NativeSqliteCanonicalRecordV1) -> CanonicalRecordV1 {
    CanonicalRecordV1 {
        kind: if record.room_seq == 0 {
            CanonicalRecordKindV1::Genesis
        } else {
            CanonicalRecordKindV1::Transition
        },
        room_seq: record.room_seq,
        bytes: record.bytes.clone(),
        digest: record.digest.clone(),
        previous_digest: record.previous_digest.clone(),
    }
}

fn backup_head(bytes: &[u8]) -> Result<CompleteHeadV1, NativeSqliteEnvelopeError> {
    let head = CanonicalJsonV1::decode_canonical::<CoreCompleteHead>(bytes)
        .map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)?;
    Ok(CompleteHeadV1 {
        room_id: head.room_id().to_string(),
        room_seq: head.room_seq().get(),
        lineage_digest: core_digest(&head.genesis_or_transition_hash().to_string()),
        core_schema_version: head.core_schema_version().to_owned(),
        pack_revision_digest: core_digest(&head.pack_digest().to_string()),
        core_state_digest: core_digest(&head.core_state_hash().to_string()),
        activity_state_digest: core_digest(&head.activity_state_hash().to_string()),
        authoritative_state_digest: core_digest(&head.authoritative_state_hash().to_string()),
    })
}

fn core_digest(value: &str) -> DigestV1 {
    DigestV1::parse(value.strip_prefix("blake3:").unwrap_or(value).to_owned())
        .unwrap_or_else(|_| DigestV1::hash(&[]))
}

fn integrity_status(value: &str) -> Result<IntegrityStatusV1, NativeSqliteEnvelopeError> {
    match value {
        "healthy" => Ok(IntegrityStatusV1::Healthy),
        "faulted" => Ok(IntegrityStatusV1::Faulted),
        "quarantined" => Ok(IntegrityStatusV1::Quarantined),
        _ => Err(NativeSqliteEnvelopeError::InvalidNativeRow),
    }
}

fn room_evidence_digest(
    evidence: &NativeSqliteRestoreEvidenceV1,
    room_id: &str,
    authoritative: &[u8],
) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    let mut operational = BTreeMap::<String, Vec<NativeSqliteRowV1>>::new();
    for (table, rows) in &evidence.operational.tables {
        let room_index = match table.as_str() {
            "activation_intents" => 1,
            _ => 0,
        };
        operational.insert(
            table.clone(),
            rows.iter()
                .filter(|row| text(&row.values, room_index) == Some(room_id))
                .cloned()
                .collect(),
        );
    }
    let material = (
        room_id,
        evidence.canonical_records.get(room_id),
        evidence.materializations.get(room_id),
        evidence.newest_valid_snapshots.get(room_id),
        evidence.integrity.get(room_id),
        authoritative,
        operational,
    );
    serde_json::to_vec(&material)
        .map(|bytes| DigestV1::hash(&bytes))
        .map_err(|_| NativeSqliteEnvelopeError::InvalidEnvelope)
}

fn build_semantic_receipts(
    evidence: &NativeSqliteRestoreEvidenceV1,
    requests: &mut BTreeMap<(NativeSqliteRequestLedgerV1, Vec<u8>), Vec<u8>>,
) -> Result<Vec<ReceiptV1>, NativeSqliteEnvelopeError> {
    let mut result = Vec::new();
    for row in rows(evidence, "semantic_receipts") {
        let identity = blob(&row.values, 2)?;
        let request = requests
            .remove(&(NativeSqliteRequestLedgerV1::Semantic, identity.to_vec()))
            .ok_or(NativeSqliteEnvelopeError::IncompleteCompanion)?;
        let request_digest = digest_bytes(&row.values, 4)?;
        if DigestV1::hash(&request) != request_digest {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
        let room_id = text(&row.values, 0)
            .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
            .to_owned();
        let transition_seq = optional_integer(&row.values, 9)?
            .map(|value| {
                u64::try_from(value).map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)
            })
            .transpose()?;
        let result_bytes = blob(&row.values, 10)?.to_vec();
        result.push(ReceiptV1 {
            kind: ReceiptKindV1::Semantic,
            identity_bytes: identity.to_vec(),
            request_bytes: request,
            request_bytes_available: true,
            request_digest,
            result_digest: DigestV1::hash(&result_bytes),
            result_bytes,
            room_id: Some(room_id),
            transition_seq,
            activation_id: None,
            operation_kind: None,
        });
    }
    Ok(result)
}

fn build_activation_receipts(
    evidence: &NativeSqliteRestoreEvidenceV1,
    requests: &mut BTreeMap<(NativeSqliteRequestLedgerV1, Vec<u8>), Vec<u8>>,
) -> Result<Vec<ReceiptV1>, NativeSqliteEnvelopeError> {
    let mut result = Vec::new();
    for row in rows(evidence, "activation_operation_receipts") {
        let operation_id = text(&row.values, 1)
            .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
            .as_bytes()
            .to_vec();
        let request = requests
            .remove(&(
                NativeSqliteRequestLedgerV1::Activation,
                operation_id.clone(),
            ))
            .ok_or(NativeSqliteEnvelopeError::IncompleteCompanion)?;
        let request_digest = digest_bytes(&row.values, 3)?;
        if DigestV1::hash(&request) != request_digest {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
        let result_bytes = blob(&row.values, 6)?.to_vec();
        result.push(ReceiptV1 {
            kind: ReceiptKindV1::ActivationOperation,
            identity_bytes: operation_id,
            request_bytes: request,
            request_bytes_available: true,
            request_digest,
            result_digest: DigestV1::hash(&result_bytes),
            result_bytes,
            room_id: text(&row.values, 0).map(str::to_owned),
            transition_seq: None,
            activation_id: optional_text(&row.values, 4).map(str::to_owned),
            operation_kind: text(&row.values, 2).map(str::to_owned),
        });
    }
    Ok(result)
}

fn build_timers(
    evidence: &NativeSqliteRestoreEvidenceV1,
    relations: &mut BTreeMap<(String, String, u64), Option<u64>>,
) -> Result<Vec<TimerV1>, NativeSqliteEnvelopeError> {
    let mut result = Vec::new();
    for row in rows(evidence, "timers") {
        let room_id = text(&row.values, 0).ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?;
        let timer_id = text(&row.values, 1).ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?;
        let generation = u64_value(&row.values, 2)?;
        let state = match text(&row.values, 5) {
            Some("scheduled") => TimerStateV1::Scheduled,
            Some("cancelled") => TimerStateV1::Cancelled,
            Some("fired") => TimerStateV1::Fired,
            _ => return Err(NativeSqliteEnvelopeError::InvalidNativeRow),
        };
        let relation = relations.remove(&(room_id.to_owned(), timer_id.to_owned(), generation));
        let fired_transition_seq = relation.flatten();
        if matches!(state, TimerStateV1::Fired) && relation.is_none() {
            return Err(NativeSqliteEnvelopeError::IncompleteCompanion);
        }
        if !matches!(state, TimerStateV1::Fired)
            && relation.is_some_and(|transition| transition.is_some())
        {
            return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
        }
        let payload = blob(&row.values, 4)?.to_vec();
        result.push(TimerV1 {
            room_id: room_id.to_owned(),
            timer_id: timer_id.to_owned(),
            generation,
            scheduled_for: text(&row.values, 3)
                .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
                .to_owned(),
            payload_digest: DigestV1::hash(&payload),
            payload_bytes: payload,
            state,
            fired_transition_seq,
        });
    }
    Ok(result)
}

fn build_frames(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> Result<Vec<FrameV1>, NativeSqliteEnvelopeError> {
    let members = rows(evidence, "room_members")
        .iter()
        .map(|row| {
            let key = (
                text(&row.values, 0)
                    .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
                    .to_owned(),
                text(&row.values, 1)
                    .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
                    .to_owned(),
            );
            let head = u64_value(&row.values, 8)?;
            let cursor = optional_integer(&row.values, 11)?
                .map(|value| {
                    u64::try_from(value).map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)
                })
                .transpose()?;
            Ok((key, (head, cursor)))
        })
        .collect::<Result<BTreeMap<_, _>, NativeSqliteEnvelopeError>>()?;
    rows(evidence, "observation_frames")
        .iter()
        .map(|row| {
            let room_id =
                text(&row.values, 0).ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?;
            let member_id =
                text(&row.values, 1).ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?;
            let payload = blob(&row.values, 5)?.to_vec();
            let (frame_head, cursor) = members
                .get(&(room_id.to_owned(), member_id.to_owned()))
                .copied()
                .ok_or(NativeSqliteEnvelopeError::IncompleteRoom)?;
            let stored_digest = digest_text(&row.values, 4)?;
            if stored_digest != DigestV1::hash(&payload) {
                return Err(NativeSqliteEnvelopeError::CompanionDigestMismatch);
            }
            Ok(FrameV1 {
                room_id: room_id.to_owned(),
                member_id: member_id.to_owned(),
                frame_seq: u64_value(&row.values, 2)?,
                cause_room_seq: u64_value(&row.values, 3)?,
                payload_digest: stored_digest,
                payload_bytes: payload,
                frame_head,
                cursor,
            })
        })
        .collect()
}

fn build_activations(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> Result<Vec<crate::ActivationV1>, NativeSqliteEnvelopeError> {
    rows(evidence, "activation_intents")
        .iter()
        .map(|row| {
            let context_hash = optional_blob(&row.values, 16)?
                .map(|bytes| {
                    DigestV1::parse(hex_encode(bytes))
                        .map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)
                })
                .transpose()?;
            let context_bytes = optional_blob(&row.values, 17)?.map(ToOwned::to_owned);
            let retired = integer(&row.values, 18) == Some(1);
            let context = match (context_hash.as_ref(), context_bytes) {
                (None, None) => ContextRetentionV1::None,
                (Some(_), Some(bytes)) => ContextRetentionV1::Retained(bytes),
                (Some(_), None) if retired => ContextRetentionV1::Tombstone,
                _ => return Err(NativeSqliteEnvelopeError::InvalidNativeRow),
            };
            let state = match text(&row.values, 10) {
                Some("pending") => crate::ActivationStateV1::Pending,
                Some("leased") => crate::ActivationStateV1::Leased,
                Some("completed") => crate::ActivationStateV1::Completed,
                Some("expired") => crate::ActivationStateV1::Expired,
                Some("cancelled") => crate::ActivationStateV1::Cancelled,
                _ => return Err(NativeSqliteEnvelopeError::InvalidNativeRow),
            };
            Ok(crate::ActivationV1 {
                activation_id: text(&row.values, 0)
                    .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
                    .to_owned(),
                room_id: text(&row.values, 1)
                    .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
                    .to_owned(),
                cause_room_seq: u64_value(&row.values, 2)?,
                target_member_id: text(&row.values, 4)
                    .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?
                    .to_owned(),
                state,
                intent_generation: u64_value(&row.values, 11)?,
                lease_generation: u64_value(&row.values, 12)?,
                runner_id: optional_text(&row.values, 13).map(str::to_owned),
                claim_id: optional_text(&row.values, 14).map(str::to_owned),
                lease_until: optional_text(&row.values, 15).map(str::to_owned),
                context_digest: context_hash,
                context,
            })
        })
        .collect()
}

fn image_global_digest(image: &BackupImageV1) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    let mut normalized = image.clone();
    normalized.manifest.expected_global_digest = DigestV1::hash(&[]);
    normalized.source_global_digest = DigestV1::hash(&[]);
    normalized.restored_global_digest = DigestV1::hash(&[]);
    serde_json::to_vec(&normalized)
        .map(|bytes| DigestV1::hash(&bytes))
        .map_err(|_| NativeSqliteEnvelopeError::InvalidEnvelope)
}

fn rows<'a>(evidence: &'a NativeSqliteRestoreEvidenceV1, table: &str) -> &'a [NativeSqliteRowV1] {
    evidence
        .operational
        .tables
        .get(table)
        .map_or(&[], Vec::as_slice)
}

fn text(values: &[NativeSqliteValueV1], index: usize) -> Option<&str> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Some(value),
        _ => None,
    }
}

fn optional_text(values: &[NativeSqliteValueV1], index: usize) -> Option<&str> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Some(value),
        Some(NativeSqliteValueV1::Null) => None,
        _ => Some("<invalid>"),
    }
}

fn integer(values: &[NativeSqliteValueV1], index: usize) -> Option<i64> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Some(*value),
        _ => None,
    }
}

fn optional_integer(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<Option<i64>, NativeSqliteEnvelopeError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Ok(Some(*value)),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => Err(NativeSqliteEnvelopeError::InvalidNativeRow),
    }
}

fn u64_value(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<u64, NativeSqliteEnvelopeError> {
    integer(values, index)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)
}

fn blob(values: &[NativeSqliteValueV1], index: usize) -> Result<&[u8], NativeSqliteEnvelopeError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Ok(value),
        _ => Err(NativeSqliteEnvelopeError::InvalidNativeRow),
    }
}

fn optional_blob(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<Option<&[u8]>, NativeSqliteEnvelopeError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Ok(Some(value)),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => Err(NativeSqliteEnvelopeError::InvalidNativeRow),
    }
}

fn digest_bytes(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    let bytes = blob(values, index)?;
    if bytes.len() != 32 {
        return Err(NativeSqliteEnvelopeError::InvalidNativeRow);
    }
    DigestV1::parse(hex_encode(bytes)).map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)
}

fn digest_text(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    let value = text(values, index).ok_or(NativeSqliteEnvelopeError::InvalidNativeRow)?;
    DigestV1::parse(value.strip_prefix("blake3:").unwrap_or(value).to_owned())
        .map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)
}

fn digest_blob_or_text(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<DigestV1, NativeSqliteEnvelopeError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(_)) => digest_text(values, index),
        Some(NativeSqliteValueV1::Blob(value)) if value.len() == 32 => {
            DigestV1::parse(hex_encode(value))
                .map_err(|_| NativeSqliteEnvelopeError::InvalidNativeRow)
        }
        _ => Err(NativeSqliteEnvelopeError::InvalidNativeRow),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod compatibility_authority_tests {
    use std::collections::BTreeMap;

    use super::{
        DigestV1, NativeSqliteOperationalRowsV1, NativeSqliteRequestLedgerV1,
        NativeSqliteRequestWitnessV1, NativeSqliteRestoreEvidenceV1, NativeSqliteRowV1,
        NativeSqliteTimerRelationV1, NativeSqliteValueV1, TRUSTED_COMPATIBILITY_JSON_DIGEST,
        build_activation_receipts, build_semantic_receipts, build_timers, request_map,
        require_companion_maps_consumed, timer_relation_map,
    };

    #[test]
    fn trusted_compatibility_mirror_digest_is_current() {
        assert_eq!(
            DigestV1::hash(include_bytes!("../../../compatibility.json")).as_str(),
            TRUSTED_COMPATIBILITY_JSON_DIGEST
        );
    }

    #[test]
    fn exact_request_and_timer_witnesses_are_consumed() {
        let request_bytes = b"exact-request".to_vec();
        let identity = b"operation-identity".to_vec();
        let request = NativeSqliteRequestWitnessV1 {
            ledger: NativeSqliteRequestLedgerV1::Semantic,
            identity_bytes: identity.clone(),
            request_bytes: request_bytes.clone(),
            request_digest: DigestV1::hash(&request_bytes),
        };
        let timer = NativeSqliteTimerRelationV1 {
            room_id: "room-1".to_owned(),
            timer_id: "timer-1".to_owned(),
            generation: 1,
            fired_transition_seq: Some(1),
        };
        let mut tables = BTreeMap::new();
        tables.insert(
            "semantic_receipts".to_owned(),
            vec![NativeSqliteRowV1 {
                table: "semantic_receipts".to_owned(),
                values: vec![
                    NativeSqliteValueV1::Text("room-1".to_owned()),
                    NativeSqliteValueV1::Text("action".to_owned()),
                    NativeSqliteValueV1::Blob(identity),
                    NativeSqliteValueV1::Text("worldstream/operation-receipt/v1".to_owned()),
                    NativeSqliteValueV1::Blob(blake3::hash(&request_bytes).as_bytes().to_vec()),
                    NativeSqliteValueV1::Null,
                    NativeSqliteValueV1::Blob(Vec::new()),
                    NativeSqliteValueV1::Blob(Vec::new()),
                    NativeSqliteValueV1::Text("no_change_recorded".to_owned()),
                    NativeSqliteValueV1::Null,
                    NativeSqliteValueV1::Blob(b"result".to_vec()),
                    NativeSqliteValueV1::Text("2026-08-22T00:00:00Z".to_owned()),
                ],
            }],
        );
        tables.insert(
            "timers".to_owned(),
            vec![NativeSqliteRowV1 {
                table: "timers".to_owned(),
                values: vec![
                    NativeSqliteValueV1::Text("room-1".to_owned()),
                    NativeSqliteValueV1::Text("timer-1".to_owned()),
                    NativeSqliteValueV1::Integer(1),
                    NativeSqliteValueV1::Text("2026-08-22T00:00:00Z".to_owned()),
                    NativeSqliteValueV1::Blob(b"payload".to_vec()),
                    NativeSqliteValueV1::Text("fired".to_owned()),
                ],
            }],
        );
        let evidence = empty_evidence(tables);
        let mut requests =
            request_map(&[request]).unwrap_or_else(|error| unreachable!("request map: {error:?}"));
        let mut timers = timer_relation_map(&[timer])
            .unwrap_or_else(|error| unreachable!("timer map: {error:?}"));

        assert_eq!(
            build_semantic_receipts(&evidence, &mut requests)
                .unwrap_or_else(|error| unreachable!("semantic receipt: {error:?}"))
                .len(),
            1
        );
        assert_eq!(
            build_timers(&evidence, &mut timers)
                .unwrap_or_else(|error| unreachable!("timer: {error:?}"))
                .len(),
            1
        );
        assert!(
            requests.is_empty(),
            "the exact request key must be consumed"
        );
        assert!(timers.is_empty(), "the exact timer key must be consumed");
        assert!(require_companion_maps_consumed(&requests, &timers).is_ok());
    }

    #[test]
    fn unreferenced_request_and_timer_witnesses_fail_closed() {
        let request_bytes = b"unreferenced-request".to_vec();
        let mut requests = request_map(&[NativeSqliteRequestWitnessV1 {
            ledger: NativeSqliteRequestLedgerV1::Semantic,
            identity_bytes: b"unreferenced-operation".to_vec(),
            request_digest: DigestV1::hash(&request_bytes),
            request_bytes,
        }])
        .unwrap_or_else(|error| unreachable!("request map: {error:?}"));
        let mut timers = timer_relation_map(&[NativeSqliteTimerRelationV1 {
            room_id: "unreferenced-room".to_owned(),
            timer_id: "unreferenced-timer".to_owned(),
            generation: 1,
            fired_transition_seq: Some(1),
        }])
        .unwrap_or_else(|error| unreachable!("timer map: {error:?}"));
        let evidence = empty_evidence(BTreeMap::new());

        assert!(
            build_semantic_receipts(&evidence, &mut requests)
                .unwrap_or_else(|error| unreachable!("no semantic rows: {error:?}"))
                .is_empty()
        );
        assert!(
            build_activation_receipts(&evidence, &mut requests)
                .unwrap_or_else(|error| unreachable!("no activation rows: {error:?}"))
                .is_empty()
        );
        assert!(
            build_timers(&evidence, &mut timers)
                .unwrap_or_else(|error| unreachable!("no timer rows: {error:?}"))
                .is_empty()
        );
        assert_eq!(
            require_companion_maps_consumed(&requests, &timers),
            Err(super::NativeSqliteEnvelopeError::CompanionDigestMismatch)
        );
    }

    fn empty_evidence(
        tables: BTreeMap<String, Vec<NativeSqliteRowV1>>,
    ) -> NativeSqliteRestoreEvidenceV1 {
        NativeSqliteRestoreEvidenceV1 {
            operational: NativeSqliteOperationalRowsV1 { tables },
            canonical_records: BTreeMap::new(),
            canonical_pack_revision_locks: BTreeMap::new(),
            materializations: BTreeMap::new(),
            newest_valid_snapshots: BTreeMap::new(),
            integrity: BTreeMap::new(),
            deployment_lineage: None,
            storage_epoch: None,
            migration_metadata: None,
            pack_metadata: None,
            resource_metadata: None,
            backend: None,
            native_point: None,
            room_membership: None,
            room_heads: BTreeMap::new(),
        }
    }
}
