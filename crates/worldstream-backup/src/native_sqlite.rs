//! Bounded native checks and online backup/restore for a bundled `SQLite` file.
//!
//! Verification opens a file read-only. Backup and restore use `SQLite`'s
//! online-backup API into an atomic sibling before publishing the result. No
//! operation repairs a database or mutates the source file.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{
    Connection, OpenFlags, Row,
    backup::{Backup, StepResult},
    types::ValueRef,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_core::{
    CORE_SCHEMA_VERSION, CanonicalJsonV1, CompleteHeadV1, GenesisV1, TransitionV1,
};
use worldstream_sqlite_open::{ExactSqliteConnection, open_exact};

use crate::{
    BackendNativePointV1, BackendProfileV1, MAX_DEPLOYMENT_LINEAGE_BYTES, MAX_SAFE_INTEGER,
    NativeRestoreRoomMembershipV1, NativeRestoreTargetEvidenceV1,
};
use crate::{DigestV1, PAIRED_SNAPSHOT_SCHEMA_V1};

#[cfg(unix)]
type NativeFileIdentity = (u64, u64);

#[cfg(windows)]
type NativeFileIdentity = fs_id::FileID;

const REQUIRED_TABLES: &[&str] = &[
    "activation_decisions",
    "activation_intents",
    "activation_operation_receipts",
    "canonical_export_metadata",
    "capabilities",
    "capability_scopes",
    "authority_audit",
    "authority_change_receipts",
    "deployment_identity_metadata",
    "deployment_pack_identities",
    "deployment_resource_blobs",
    "deployment_resource_identities",
    "external_input_preparations",
    "integrity_incidents",
    "observation_consequences",
    "observation_frames",
    "room_genesis",
    "room_integrity",
    "room_materializations",
    "room_members",
    "room_snapshots",
    "rooms",
    "principals",
    "retired_authority_fences_v1",
    "runner_capability_memberships",
    "runners",
    "schema_migrations",
    "semantic_receipts",
    "source_transfer_lifecycle",
    "timers",
    "transitions",
];
const REQUIRED_MIGRATIONS: &[&str] = &[
    "0001-initial-storage-schema",
    "0002-operational-authority-v1",
    "0003-observation-delivery-v1",
    "0004-activation-work-v1",
    "0005-paired-snapshots-v1",
    "0006-canonical-export-metadata-v1",
    "0008-sqlite-migration-checksums-v1",
    "0009-deployment-identities-v1",
    "0010-transfer-recovery-completeness-v1",
    "0011-transfer-lifecycle-and-resource-identity-v1",
    "0012-transfer-backup-file-identity-v1",
    "0013-external-input-preparations-v1",
    "0014-observation-retention-v1",
    "0015-snapshot-cadence-v1",
    "0016-activation-backlog-policy-v1",
    "0017-stream-transfer-v2",
    "0018-checkpoint-operational-witness-v1",
    "0019-operational-history-roots-v2",
    "0020-current-timers-v2",
    "0021-checkpoint-operational-witness-v2",
    "0022-operational-history-mmr-v1",
    "0023-checkpoint-operational-witness-v3",
];
const REQUIRED_MIGRATION_CHECKSUMS: &[&str] = &[
    "blake3:dd07208c71d7165b93861883b25411b1e7c33a6be36fc2be28a638e1ab5cd763",
    "blake3:237088a0f888ef9f91a1010efd95e38a40170b0fc968229b886881937af805b0",
    "blake3:b74d06ed529d415a658eaede5067f24e02ff5de83ac07480a5df7de1645c8bcb",
    "blake3:dbe807e620fc77594e871b1ad90e89379498060fd025fbbaa318396158557d2b",
    "blake3:385af50337813e01ce0a97894fcb82868e130d69e671f64712f6701c0e9ddb23",
    "blake3:60de4825b3796865acff18f836dfa475640324b71d168350a8ea20c2e06206d5",
    "blake3:ed00960ddbbfbb6a6cb8fde52ce44631ce41c3e0b7dd2e46968552f7538eb33a",
    "blake3:2a9eed1343ed423c12593b19e922ffeb44e009432131f018ef3a3b440213debb",
    "blake3:e0a4033bba6de7949af577a9e75b4d1994df61b250f27f013f3c3667afe862b1",
    "blake3:cd0fe750ca3ba68d2dad7254a60dddb887912d40db270e5562a19b3b3cced0a3",
    "blake3:4605547211cde35f16fecf1d156b91d9ca24c39fc24b9fe875f29dcb491965b9",
    "blake3:2097e928196db3f2c572818b4ac87e436512df6f2e9f0cd098521a90366651f0",
    "blake3:153136e4d0fff3ffee1a02c0349fec8a2c1c907b636ce3396276177518225ac6",
    "blake3:db914b00013cc9d7a341eabe081411f6583893f036547ed9db2c35be3866e9d6",
    "blake3:495d58fdc81fee0b6b87d8973f4445b4892da22608e459033eaa333c0d4078a4",
    "blake3:aaa152c1107748f774197bd8a59600150e9d209c7e4eb14ec5e911394b23c3e2",
    "blake3:95b31dc300e31bbdafada55d7d7d9f6b3c05d0dd2e067c3f41e3f39a27655847",
    "blake3:7977311c54cfcdbec65d3846ddbd2071f53ca830464f5721092f5de958e0b98d",
    "blake3:00116356f2c4490438c9e923b8197ab7a3704c22d32f4d0d9e3912dfa41f8471",
    "blake3:52c9dbd493d058e5553c5b77a3ee4a5a1feecf8e6f881aedce36a4644662b8db",
    "blake3:e00976433df9a260598f3b1e766a2eaa076b54c0fcb88a18c3f623ea9b335c28",
    "blake3:fc24f79e96a6d4e8cbeef25b753ba1c4ac229265f68991c780502338fb12418c",
];
// These are reviewed shipped prefixes, rather than an arbitrary version
// range. A retained backup must present one exact contiguous ledger through
// the corresponding release boundary.
const SUPPORTED_MIGRATION_COUNTS: &[usize] = &[13, 15, 16, 17, 18, 19, 20, 21, 22];

/// The `SQLite` engine selected by the workspace's bundled rusqlite build.
pub const BUNDLED_SQLITE_VERSION: &str = "3.53.4";

/// Bounds applied independently to each native `SQLite` query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSqliteLimits {
    /// Maximum number of rows read from one logical table query.
    pub max_rows: usize,
    /// Maximum bytes captured from one native query's stdout.
    pub max_output_bytes: usize,
}

/// Per-row and schema bounds for the streaming retained-backup verifier.
///
/// Unlike `NativeSqliteLimits`, these bounds do not cap total rows or total
/// database bytes. The verifier advances each relation cursor incrementally,
/// so a complete deployment larger than the legacy bundle limits remains
/// verifiable without retaining all rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSqliteStreamingLimitsV2 {
    /// Maximum exact bytes admitted from one native row.
    pub max_row_bytes: usize,
    /// Maximum table names admitted from the schema inventory.
    pub max_tables: usize,
}

impl Default for NativeSqliteStreamingLimitsV2 {
    fn default() -> Self {
        Self {
            max_row_bytes: 16 * 1024 * 1024,
            max_tables: 1024,
        }
    }
}

/// Source-side evidence produced by a retained, incrementally verified
/// `SQLite` backup. It contains only bounded summaries; no `BackupImageV1` or
/// all-record collection is constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteStreamingVerificationV2 {
    /// Exact durable transfer-point digest, compatible with the source
    /// lifecycle `backup_digest` witness.
    transfer_point_digest: [u8; 32],
    /// Count of every modeled operational relation.
    operational_relation_counts: BTreeMap<String, u64>,
    /// Digest over exact modeled operational rows in scan order.
    operational_row_digest: [u8; 32],
}

impl NativeSqliteStreamingVerificationV2 {
    /// Returns the exact source lifecycle transfer-point digest.
    #[must_use]
    pub const fn transfer_point_digest(&self) -> [u8; 32] {
        self.transfer_point_digest
    }

    /// Returns the complete bounded operational relation inventory.
    #[must_use]
    pub fn operational_relation_counts(&self) -> &BTreeMap<String, u64> {
        &self.operational_relation_counts
    }

    /// Returns the exact modeled operational-row digest.
    #[must_use]
    pub const fn operational_row_digest(&self) -> [u8; 32] {
        self.operational_row_digest
    }
}

/// One exact `SQLite` value returned by the bounded native extraction seam.
///
/// BLOBs are never converted through text and NULL remains distinguishable from
/// an empty value. This type intentionally mirrors `SQLite`'s storage classes so
/// an adapter can construct the higher-level backup models without losing
/// bytes or changing a stored representation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeSqliteValueV1 {
    /// SQL NULL.
    Null,
    /// `SQLite` INTEGER, retained as the native signed value so corrupt negative
    /// counters cannot be silently converted into valid unsigned values.
    Integer(i64),
    /// `SQLite` TEXT, retained as UTF-8 text.
    Text(String),
    /// `SQLite` BLOB, retained byte-for-byte.
    Blob(Vec<u8>),
}

/// One exact row from a modeled operational `SQLite` table.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteRowV1 {
    /// Source table name.
    pub table: String,
    /// Values in the table's declared column order for the fixed query.
    pub values: Vec<NativeSqliteValueV1>,
}

/// Bounded, lossless extraction of the operational rows represented by the
/// backup contract. Resource payloads are exposed separately by
/// [`NativeSqliteRestoreEvidenceV1::resource_metadata`] because they are
/// canonical deployment bytes rather than provider-native operational rows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteOperationalRowsV1 {
    /// Rows grouped by stable table name and ordered by the table query.
    pub tables: BTreeMap<String, Vec<NativeSqliteRowV1>>,
}

/// Exact canonical bytes and integrity evidence extracted from the modeled
/// `SQLite` tables.  Optional fields are intentional: the current `SQLite`
/// schema does not persist a separately named authoritative materialization,
/// so an adapter must carry that absence to the provider-neutral restore
/// verifier instead of inventing values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteRestoreEvidenceV1 {
    /// Bounded native row extraction, including every operational ledger.
    pub operational: NativeSqliteOperationalRowsV1,
    /// Exact Genesis and Transition rows grouped by Room.
    pub canonical_records: BTreeMap<String, Vec<NativeSqliteCanonicalRecordV1>>,
    /// Exact current Core/Activity materialization bytes by Room.
    pub materializations: BTreeMap<String, (Vec<u8>, Vec<u8>)>,
    /// Newest valid paired snapshot per Room; corrupt newest snapshots fall
    /// back to the next valid snapshot and never mutate the source.
    pub newest_valid_snapshots: BTreeMap<String, NativeSqliteSnapshotEvidenceV1>,
    /// Exact integrity rows by Room: `(status, generation)`.
    pub integrity: BTreeMap<String, (String, u64)>,
    /// Present only when the explicit canonical export metadata row contains
    /// the operator-supplied lineage. It is never derived from a path, Room,
    /// or timestamp.
    pub deployment_lineage: Option<String>,
    /// Present only when the explicit canonical export metadata row contains
    /// a valid nonzero epoch.
    pub storage_epoch: Option<u64>,
    /// Present only when source-side migration checksums are persisted.
    pub migration_metadata: Option<Vec<NativeSqliteRowV1>>,
    /// Present only when source-side pack identities are persisted.
    pub pack_metadata: Option<Vec<NativeSqliteRowV1>>,
    /// Present only when source-side resource blobs are persisted.
    pub resource_metadata: Option<Vec<NativeSqliteRowV1>>,
    /// Exact backend profile observed at the native restore target, when an
    /// external adapter supplies it. The current `SQLite` schema cannot infer
    /// this target-side witness.
    pub backend: Option<BackendProfileV1>,
    /// Exact native restore point observed at the target, when supplied by an
    /// external adapter. No engine or point identity is invented here.
    pub native_point: Option<BackendNativePointV1>,
    /// Complete target-side Room membership, including isolated Rooms, when
    /// supplied by an external adapter. Source integrity rows are not
    /// promoted into target membership because they lack that witness.
    pub room_membership: Option<Vec<NativeRestoreRoomMembershipV1>>,
    /// Complete stored Room head witnesses, including fields not represented
    /// by the provider-neutral image model.
    pub room_heads: BTreeMap<String, NativeSqliteRoomHeadV1>,
}

/// Exact Room row witness retained by the native adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteRoomHeadV1 {
    /// Stored Room sequence.
    pub room_seq: u64,
    /// Stored lineage head digest.
    pub head_digest: DigestV1,
    /// Stored core schema identity.
    pub core_schema_version: String,
    /// Stored Activity Pack revision digest.
    pub pack_digest: DigestV1,
    /// Stored Core materialization digest.
    pub core_state_digest: DigestV1,
    /// Stored Activity materialization digest.
    pub activity_state_digest: DigestV1,
    /// Stored paired Authoritative State digest.
    pub authoritative_state_digest: DigestV1,
    /// Exact stored Complete Head bytes.
    pub complete_head_bytes: Vec<u8>,
}

/// One required target-side input to the provider-neutral native restore
/// verifier. These names intentionally mirror `NativeRestoreTargetEvidenceV1`
/// without constructing that value or claiming that a `BackupImageV1` exists.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NativeSqliteRestoreInputV1 {
    /// Explicit deployment lineage.
    DeploymentLineage,
    /// Target backend profile.
    Backend,
    /// Native engine/build and consistent restore-point identity.
    NativePoint,
    /// Target storage epoch.
    StorageEpoch,
    /// Exact migration/schema contract.
    MigrationContract,
    /// Exact Activity Pack identities.
    PackIdentities,
    /// Exact content-addressed resource identities.
    ResourceIdentities,
    /// Complete healthy and isolated Room membership.
    RoomMembership,
}

impl NativeSqliteRestoreInputV1 {
    const ALL: [Self; 8] = [
        Self::DeploymentLineage,
        Self::Backend,
        Self::NativePoint,
        Self::StorageEpoch,
        Self::MigrationContract,
        Self::PackIdentities,
        Self::ResourceIdentities,
        Self::RoomMembership,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::DeploymentLineage => "deployment_lineage",
            Self::Backend => "backend",
            Self::NativePoint => "native_point",
            Self::StorageEpoch => "storage_epoch",
            Self::MigrationContract => "migration_contract",
            Self::PackIdentities => "pack_identities",
            Self::ResourceIdentities => "resource_identities",
            Self::RoomMembership => "room_membership",
        }
    }
}

/// Presence of one required native-restore input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSqliteRestoreInputStatusV1 {
    /// Input represented by this status.
    pub input: NativeSqliteRestoreInputV1,
    /// Whether the exact input was supplied by the adapter.
    pub present: bool,
}

/// Deterministic, redacted metadata readiness assessment over native `SQLite`
/// evidence. `metadata_complete` does not mean restore-ready: a complete
/// `BackupImageV1` and exact resource bytes are still required by the
/// provider-neutral verifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteRestoreReadinessV1 {
    /// Presence status in the fixed verifier-input order.
    pub inputs: Vec<NativeSqliteRestoreInputStatusV1>,
    /// Inputs absent from the native evidence.
    pub missing: Vec<NativeSqliteRestoreInputV1>,
    /// Number of source integrity rows with an explicitly healthy status.
    pub healthy_room_count: usize,
    /// Number of source integrity rows explicitly marked isolated/faulted.
    pub isolated_room_count: usize,
    /// True only when every target-side input was supplied.
    pub metadata_complete: bool,
    /// Blocking findings; subjects contain only hashed input labels.
    pub diagnostics: Vec<NativeSqliteDiagnosticV1>,
}

/// Typed outcome of the native `SQLite` bridge. Even complete metadata cannot
/// become `NativeRestoreEvidenceV1` here because the `SQLite` schema does not
/// provide a manifest-backed `BackupImageV1` and resource bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeSqliteRestoreBridgeResultV1 {
    /// All target-side metadata inputs are present, but image conversion still
    /// requires an external immutable manifest/image provider.
    MetadataComplete {
        /// The complete metadata assessment.
        readiness: NativeSqliteRestoreReadinessV1,
    },
    /// At least one verifier input is absent; readiness is blocked explicitly.
    Incomplete {
        /// The bounded readiness assessment and missing-input diagnostics.
        readiness: NativeSqliteRestoreReadinessV1,
    },
}

/// The disposition assigned to a Room in a native restore projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSqliteRoomDispositionV1 {
    /// The extracted canonical and paired materialization evidence is complete.
    Healthy,
    /// The source explicitly recorded the Room as unavailable; its evidence is
    /// retained but must not be installed as a serving Room.
    Quarantined,
}

/// Lossless, provider-neutral restore input for one native `SQLite` Room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteRoomRestoreInputV1 {
    /// Stable Room identity.
    pub room_id: String,
    /// Exact Genesis and Transition bytes, in sequence order.
    pub canonical_records: Vec<NativeSqliteCanonicalRecordV1>,
    /// Exact current Core and Activity bytes, when present.
    pub materialization: Option<(Vec<u8>, Vec<u8>)>,
    /// Newest valid paired snapshot, when present.
    pub newest_valid_snapshot: Option<NativeSqliteSnapshotEvidenceV1>,
    /// Exact source integrity status and generation.
    pub integrity: (String, u64),
    /// Whether the Room may participate in restore serving.
    pub disposition: NativeSqliteRoomDispositionV1,
}

/// A typed native `SQLite` restore input. This is deliberately not a
/// `BackupImageV1`: manifest, resource, and target-side metadata remain
/// external evidence and are never fabricated by the `SQLite` adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteRestoreInputProjectionV1 {
    /// All bounded operational rows, byte-for-byte.
    pub operational: NativeSqliteOperationalRowsV1,
    /// Rooms in stable identity order, including quarantined Rooms.
    pub rooms: Vec<NativeSqliteRoomRestoreInputV1>,
    /// Metadata readiness inherited from the extracted evidence.
    pub readiness: NativeSqliteRestoreReadinessV1,
}

/// Errors raised while converting extracted `SQLite` evidence into typed restore
/// input. These errors are fail-closed and do not expose native paths or data.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum NativeSqliteRestoreProjectionError {
    /// A Room has no integrity witness and cannot be safely classified.
    #[error("native SQLite restore projection is missing Room integrity evidence")]
    MissingIntegrity,
    /// A source integrity status is not part of the reviewed contract.
    #[error("native SQLite restore projection has an unknown Room integrity status")]
    UnknownIntegrityStatus,
    /// A healthy Room lacks required canonical or paired materialization data.
    #[error("native SQLite restore projection has incomplete healthy Room evidence")]
    IncompleteHealthyRoom,
    /// A healthy Room contains a record whose stored digest does not match its bytes.
    #[error("native SQLite restore projection has corrupt canonical bytes")]
    CorruptCanonicalBytes,
    /// Native metadata required for a complete provider restore is absent.
    #[error("native SQLite restore projection is missing required metadata")]
    MissingMetadata(Vec<NativeSqliteRestoreInputV1>),
}

impl NativeSqliteRestoreInputProjectionV1 {
    /// Rejects a projection whose target-side metadata is not complete.
    ///
    /// # Errors
    ///
    /// Returns [`NativeSqliteRestoreProjectionError::MissingMetadata`] when
    /// one or more provider-restore inputs were absent from the source.
    pub fn require_complete_metadata(&self) -> Result<(), NativeSqliteRestoreProjectionError> {
        if self.readiness.metadata_complete {
            Ok(())
        } else {
            Err(NativeSqliteRestoreProjectionError::MissingMetadata(
                self.readiness.missing.clone(),
            ))
        }
    }
}

/// Assesses the exact native-restore metadata available from a `SQLite` bridge.
#[must_use]
pub fn assess_restore_readiness(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> NativeSqliteRestoreReadinessV1 {
    assess_restore_readiness_with_presence(
        evidence,
        [
            evidence.deployment_lineage.is_some(),
            evidence.backend.is_some(),
            evidence.native_point.is_some(),
            evidence.storage_epoch.is_some(),
            evidence.migration_metadata.is_some(),
            evidence.pack_metadata.is_some(),
            evidence.resource_metadata.is_some(),
            evidence.room_membership.is_some(),
        ],
    )
}

/// Assesses metadata supplied by an explicit native adapter witness.
///
/// The witness remains separate from `SQLite` row evidence so callers cannot
/// accidentally turn a present sidecar field into synthetic native rows.
#[must_use]
pub fn assess_restore_readiness_with_target_metadata(
    evidence: &NativeSqliteRestoreEvidenceV1,
    metadata: &NativeRestoreTargetEvidenceV1,
) -> NativeSqliteRestoreReadinessV1 {
    assess_restore_readiness_with_presence(
        evidence,
        [
            metadata.deployment_lineage.is_some(),
            metadata.backend.is_some(),
            metadata.native_point.is_some(),
            metadata.storage_epoch.is_some(),
            metadata.migration_contract.is_some(),
            metadata.pack_identities.is_some(),
            metadata.resource_identities.is_some(),
            metadata.room_membership.is_some(),
        ],
    )
}

fn assess_restore_readiness_with_presence(
    evidence: &NativeSqliteRestoreEvidenceV1,
    present: [bool; 8],
) -> NativeSqliteRestoreReadinessV1 {
    let mut inputs = Vec::with_capacity(NativeSqliteRestoreInputV1::ALL.len());
    let mut missing = Vec::new();
    let mut diagnostics = Vec::new();
    for (input, is_present) in NativeSqliteRestoreInputV1::ALL.into_iter().zip(present) {
        inputs.push(NativeSqliteRestoreInputStatusV1 {
            input,
            present: is_present,
        });
        if !is_present {
            missing.push(input);
            diagnostic(
                &mut diagnostics,
                "native_restore_input_missing",
                true,
                input.label(),
            );
        }
    }

    let (healthy_room_count, isolated_room_count) =
        evidence
            .integrity
            .values()
            .fold((0, 0), |(healthy, isolated), (status, _)| {
                if status.eq_ignore_ascii_case("healthy") || status.eq_ignore_ascii_case("active") {
                    (healthy + 1, isolated)
                } else if status.eq_ignore_ascii_case("faulted")
                    || status.eq_ignore_ascii_case("quarantined")
                    || status.eq_ignore_ascii_case("isolated")
                {
                    (healthy, isolated + 1)
                } else {
                    (healthy, isolated)
                }
            });

    NativeSqliteRestoreReadinessV1 {
        inputs,
        missing,
        healthy_room_count,
        isolated_room_count,
        metadata_complete: diagnostics.is_empty(),
        diagnostics,
    }
}

/// Returns a typed bridge outcome without fabricating a manifest or image.
#[must_use]
pub fn bridge_restore_evidence(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> NativeSqliteRestoreBridgeResultV1 {
    let readiness = assess_restore_readiness(evidence);
    bridge_restore_readiness(readiness)
}

/// Returns a typed bridge outcome for exact metadata supplied by an external
/// native adapter, without copying that metadata into synthetic `SQLite` rows.
#[must_use]
pub fn bridge_restore_evidence_with_target_metadata(
    evidence: &NativeSqliteRestoreEvidenceV1,
    metadata: &NativeRestoreTargetEvidenceV1,
) -> NativeSqliteRestoreBridgeResultV1 {
    let readiness = assess_restore_readiness_with_target_metadata(evidence, metadata);
    bridge_restore_readiness(readiness)
}

fn bridge_restore_readiness(
    readiness: NativeSqliteRestoreReadinessV1,
) -> NativeSqliteRestoreBridgeResultV1 {
    if readiness.metadata_complete {
        NativeSqliteRestoreBridgeResultV1::MetadataComplete { readiness }
    } else {
        NativeSqliteRestoreBridgeResultV1::Incomplete { readiness }
    }
}

/// Converts bounded native evidence into a typed restore input without
/// decoding, re-encoding, or inventing any bytes. Healthy Rooms must have an
/// integrity witness, a Genesis record, a contiguous canonical chain, a
/// materialization, and a valid paired snapshot. Explicitly faulted or
/// quarantined Rooms are retained in the result but marked `Quarantined` so a
/// caller can preserve them without allowing them to serve.
///
/// # Errors
///
/// Returns a projection error when a Room has no integrity witness, an
/// unrecognized integrity status, corrupt canonical bytes, or incomplete
/// healthy evidence.
#[allow(clippy::too_many_lines)]
pub fn project_restore_input(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> Result<NativeSqliteRestoreInputProjectionV1, NativeSqliteRestoreProjectionError> {
    let mut room_ids = BTreeSet::new();
    room_ids.extend(evidence.canonical_records.keys().cloned());
    room_ids.extend(evidence.materializations.keys().cloned());
    room_ids.extend(evidence.newest_valid_snapshots.keys().cloned());
    room_ids.extend(evidence.integrity.keys().cloned());

    let mut rooms = Vec::with_capacity(room_ids.len());
    for room_id in room_ids {
        let integrity = evidence
            .integrity
            .get(&room_id)
            .cloned()
            .ok_or(NativeSqliteRestoreProjectionError::MissingIntegrity)?;
        let disposition = match integrity.0.to_ascii_lowercase().as_str() {
            "healthy" | "active" => NativeSqliteRoomDispositionV1::Healthy,
            "faulted" | "quarantined" | "isolated" => NativeSqliteRoomDispositionV1::Quarantined,
            _ => return Err(NativeSqliteRestoreProjectionError::UnknownIntegrityStatus),
        };
        let canonical_records = evidence
            .canonical_records
            .get(&room_id)
            .cloned()
            .unwrap_or_default();
        if disposition == NativeSqliteRoomDispositionV1::Healthy {
            let mut expected_seq = 0_u64;
            let mut previous_digest = None;
            for record in &canonical_records {
                if record.room_id != room_id {
                    return Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes);
                }
                let record_digest = match record.room_seq {
                    0 => GenesisV1::from_canonical_bytes(&record.bytes)
                        .ok()
                        .and_then(|genesis| {
                            (genesis.room_id().to_string() == room_id).then(|| {
                                storage_digest_from_core(&genesis.genesis_hash().to_string())
                            })
                        }),
                    _ => TransitionV1::from_canonical_bytes(&record.bytes)
                        .ok()
                        .and_then(|transition| {
                            (transition.complete_head().room_id().to_string() == room_id
                                && transition.room_seq().get() == record.room_seq)
                                .then(|| {
                                    storage_digest_from_core(
                                        &transition.transition_hash().to_string(),
                                    )
                                })
                        }),
                };
                if record.room_seq != expected_seq
                    || record_digest.as_ref() != Some(&record.digest)
                    || (record.room_seq == 0) != record.previous_digest.is_none()
                    || (record.room_seq > 0 && record.previous_digest != previous_digest)
                {
                    return Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes);
                }
                previous_digest = Some(record.digest.clone());
                expected_seq = expected_seq.saturating_add(1);
            }
            let Some(last_record) = canonical_records.last() else {
                return Err(NativeSqliteRestoreProjectionError::IncompleteHealthyRoom);
            };
            let Some(last_head) = complete_head_for_record(last_record) else {
                return Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes);
            };
            let pack_digest = storage_digest_from_core(&last_head.pack_digest().to_string());
            let Some((core, activity)) = evidence.materializations.get(&room_id) else {
                return Err(NativeSqliteRestoreProjectionError::IncompleteHealthyRoom);
            };
            if last_head.room_id().to_string() != room_id
                || last_head.room_seq().get() != last_record.room_seq
                || storage_digest_from_core(&last_head.genesis_or_transition_hash().to_string())
                    != last_record.digest
                || canonical_core_materialization_hash(core, pack_digest.as_str()).as_ref()
                    != Some(&storage_digest_from_core(
                        &last_head.core_state_hash().to_string(),
                    ))
                || canonical_activity_materialization_hash(activity, pack_digest.as_str()).as_ref()
                    != Some(&storage_digest_from_core(
                        &last_head.activity_state_hash().to_string(),
                    ))
            {
                return Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes);
            }
            let Some(snapshot) = evidence.newest_valid_snapshots.get(&room_id) else {
                return Err(NativeSqliteRestoreProjectionError::IncompleteHealthyRoom);
            };
            if !snapshot_matches_records(snapshot, &canonical_records) {
                return Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes);
            }
        }
        rooms.push(NativeSqliteRoomRestoreInputV1 {
            room_id: room_id.clone(),
            canonical_records,
            materialization: evidence.materializations.get(&room_id).cloned(),
            newest_valid_snapshot: evidence.newest_valid_snapshots.get(&room_id).cloned(),
            integrity,
            disposition,
        });
    }

    Ok(NativeSqliteRestoreInputProjectionV1 {
        operational: evidence.operational.clone(),
        rooms,
        readiness: assess_restore_readiness(evidence),
    })
}

fn complete_head_for_record(record: &NativeSqliteCanonicalRecordV1) -> Option<CompleteHeadV1> {
    if record.room_seq == 0 {
        GenesisV1::from_canonical_bytes(&record.bytes)
            .ok()
            .map(|genesis| genesis.complete_head())
    } else {
        TransitionV1::from_canonical_bytes(&record.bytes)
            .ok()
            .map(|transition| transition.complete_head())
    }
}

fn snapshot_matches_records(
    snapshot: &NativeSqliteSnapshotEvidenceV1,
    records: &[NativeSqliteCanonicalRecordV1],
) -> bool {
    let Some(record) = records
        .iter()
        .find(|record| record.room_seq == snapshot.room_seq)
    else {
        return false;
    };
    let Ok(head) =
        CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&snapshot.complete_head_bytes)
    else {
        return false;
    };
    let pack_digest = storage_digest_from_core(&head.pack_digest().to_string());
    head.room_id().to_string() == snapshot.room_id
        && head.room_seq().get() == snapshot.room_seq
        && storage_digest_from_core(&head.genesis_or_transition_hash().to_string())
            == snapshot.lineage
        && snapshot.lineage == record.digest
        && canonical_core_materialization_hash(&snapshot.core_state_bytes, pack_digest.as_str())
            .as_ref()
            == Some(&storage_digest_from_core(
                &head.core_state_hash().to_string(),
            ))
        && canonical_activity_materialization_hash(
            &snapshot.activity_state_bytes,
            pack_digest.as_str(),
        )
        .as_ref()
            == Some(&storage_digest_from_core(
                &head.activity_state_hash().to_string(),
            ))
        && snapshot.core_state_hash == storage_digest_from_core(&head.core_state_hash().to_string())
        && snapshot.activity_state_hash
            == storage_digest_from_core(&head.activity_state_hash().to_string())
}

/// One exact canonical record extracted without decoding or re-encoding bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteCanonicalRecordV1 {
    /// Owning Room.
    pub room_id: String,
    /// Zero for Genesis, otherwise the committed Transition sequence.
    pub room_seq: u64,
    /// Stored record digest.
    pub digest: DigestV1,
    /// Previous lineage digest, absent only for Genesis.
    pub previous_digest: Option<DigestV1>,
    /// Exact stored canonical bytes.
    pub bytes: Vec<u8>,
}

/// One exact paired snapshot candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeSqliteSnapshotEvidenceV1 {
    /// Owning Room and snapshot sequence.
    pub room_id: String,
    pub room_seq: u64,
    /// Exact paired snapshot bytes.
    pub complete_head_bytes: Vec<u8>,
    /// Exact Core and Activity bytes.
    pub core_state_bytes: Vec<u8>,
    /// Exact Activity bytes.
    pub activity_state_bytes: Vec<u8>,
    /// Exact stored hashes and lineage witness.
    pub lineage: DigestV1,
    pub core_state_hash: DigestV1,
    pub activity_state_hash: DigestV1,
}

impl Default for NativeSqliteLimits {
    fn default() -> Self {
        Self {
            max_rows: 100_000,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

/// One stable, redacted native-verification finding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteDiagnosticV1 {
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Whether this finding blocks the canonical verification result.
    pub blocking: bool,
    /// Hash-derived subject; raw Room IDs are not returned.
    pub subject: String,
    /// Redacted operator action that can be taken without exposing native paths or payloads.
    pub action: &'static str,
}

/// Evidence returned after a bounded native `SQLite` read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteVerificationReportV1 {
    /// `SQLite` engine version reported by the native executable.
    pub engine_version: String,
    /// Whether the native connection reported query-only mode.
    pub query_only: bool,
    /// Exact result of `PRAGMA integrity_check`.
    pub integrity_check: String,
    /// Number of contiguous migration rows observed.
    pub migration_count: usize,
    /// Number of canonical Rooms inspected.
    pub room_count: usize,
    /// Number of canonical Transition rows inspected.
    pub transition_count: usize,
    /// Number of paired snapshots present in the restored file.
    pub snapshot_count: usize,
    /// Number of paired snapshots whose bytes and witnesses are valid.
    pub valid_snapshot_count: usize,
    /// Number of invalid or absent snapshots that remain disposable.
    pub disposable_snapshot_count: usize,
    /// True only when `SQLite` and canonical lineage checks passed.
    pub canonical_ready: bool,
    /// Per-room terminal lineage digests, when the room chain was valid.
    pub room_lineage_digests: BTreeMap<String, DigestV1>,
    /// Number of durable Timer rows inspected.
    pub timer_count: usize,
    /// Number of durable Observation Frame rows inspected.
    pub frame_count: usize,
    /// Number of semantic receipt rows inspected.
    pub semantic_receipt_count: usize,
    /// Number of Activation Intent rows inspected.
    pub activation_intent_count: usize,
    /// Number of Activation operation receipt rows inspected.
    pub activation_receipt_count: usize,
    /// Number of Activation decision rows inspected.
    pub activation_decision_count: usize,
    /// Number of observation consequence rows inspected.
    pub observation_consequence_count: usize,
    /// Stable diagnostics in discovery order.
    pub diagnostics: Vec<NativeSqliteDiagnosticV1>,
}

/// Errors at the native `SQLite` or bounded decoding boundary.
#[derive(Debug, Eq, Error, PartialEq)]
pub enum NativeSqliteError {
    /// The input is not a regular, non-symlink file.
    #[error("native SQLite input is not a regular file")]
    InvalidPath,
    /// Evidence admission found a journal mode or sidecar state that the exact
    /// retained-main VFS cannot bind safely.
    #[error(
        "native SQLite evidence requires a standalone DELETE-journal file with no journal, WAL, or SHM sidecars"
    )]
    UnsafeSidecarState,
    /// The native process could not be started or read.
    #[error("native SQLite process I/O failed: {0}")]
    Io(String),
    /// One query exceeded its bounded stdout allowance.
    #[error("native SQLite query output exceeded the configured bound")]
    OutputBoundExceeded,
    /// `SQLite` rejected the read-only query or reported a fatal open error.
    #[error("native SQLite query failed")]
    QueryFailed,
    /// A temporary or published file could not be removed after failure.
    #[error("native SQLite {what} cleanup failed")]
    CleanupFailed { what: &'static str },
    /// The native `SQLite` online-backup API failed.
    #[error("native SQLite {operation} failed")]
    NativeOperationFailed { operation: &'static str },
    /// A returned scalar/row did not match the fixed evidence shape.
    #[error("native SQLite evidence row is invalid: {what}")]
    InvalidRow { what: &'static str },
}

/// Canonical storage and file identifiers for one retained native artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSqliteFileIdentityV1 {
    /// Storage device or Windows volume serial number.
    pub storage_id: u64,
    /// File identifier within `storage_id`.
    pub file_id: u128,
}

/// Result of a bundled `SQLite` online-backup or restore operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteTransferReportV1 {
    /// Bundled `SQLite` engine version used by the operation.
    pub engine_version: String,
    /// Number of pages copied by the native online-backup API.
    pub page_count: u64,
    /// Read-only semantic verification of the source before copying.
    pub source_canonical_ready: bool,
    /// Read-only semantic verification of the published destination.
    pub destination_canonical_ready: bool,
    /// Exact published destination object retained through final validation.
    destination_identity: NativeSqliteFileIdentityV1,
    capture: NativeSqliteCaptureWitnessV1,
}

/// Result of a large-history `SQLite` restore verified with bounded streaming
/// scans.  It deliberately carries summaries and digests only: no
/// `BackupImageV1`, `NativeSqliteRestoreEvidenceV1`, or deployment-sized row
/// collection is constructed while restoring the file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteStreamingRestoreReportV2 {
    /// Bundled `SQLite` engine version used by the operation.
    pub engine_version: String,
    /// Number of pages copied by the native online-backup API.
    pub page_count: u64,
    /// Exact source durable transfer-point digest.
    pub source_transfer_point_digest: [u8; 32],
    /// Exact published destination durable transfer-point digest.
    pub destination_transfer_point_digest: [u8; 32],
    /// Complete bounded inventory of native operational relations.
    pub operational_relation_counts: BTreeMap<String, u64>,
    /// Digest over exact native operational rows in stable scan order.
    pub operational_row_digest: [u8; 32],
    destination_identity: NativeSqliteFileIdentityV1,
}

impl NativeSqliteStreamingRestoreReportV2 {
    /// Returns the exact published destination storage and file identifiers.
    #[must_use]
    pub const fn destination_identity(&self) -> NativeSqliteFileIdentityV1 {
        self.destination_identity
    }
}

/// Non-serializable witness minted only after the bundled online-backup API
/// and both bounded native extractions have succeeded.  A JSON envelope never
/// creates this authority: restore verification must receive this witness
/// from the actual transfer that produced its source/target files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteCaptureWitnessV1 {
    source_evidence_digest: DigestV1,
    target_evidence_digest: DigestV1,
    native_point: BackendNativePointV1,
    engine_version: String,
}

impl NativeSqliteCaptureWitnessV1 {
    /// Exact source extraction digest authenticated by the native operation.
    #[must_use]
    pub fn source_evidence_digest(&self) -> &DigestV1 {
        &self.source_evidence_digest
    }

    /// Exact target extraction digest authenticated by the native operation.
    #[must_use]
    pub fn target_evidence_digest(&self) -> &DigestV1 {
        &self.target_evidence_digest
    }

    /// Native point identity minted from the exact extracted content, never a
    /// path, timestamp, page count, or caller-provided label.
    #[must_use]
    pub fn native_point(&self) -> &BackendNativePointV1 {
        &self.native_point
    }

    /// Bundled engine identity observed during the transfer.
    #[must_use]
    pub fn engine_version(&self) -> &str {
        &self.engine_version
    }
}

impl NativeSqliteTransferReportV1 {
    /// Returns the non-serializable witness for this exact native transfer.
    #[must_use]
    pub fn capture_witness(&self) -> &NativeSqliteCaptureWitnessV1 {
        &self.capture
    }

    /// Returns the exact published destination storage and file identifiers.
    #[must_use]
    pub const fn destination_identity(&self) -> NativeSqliteFileIdentityV1 {
        self.destination_identity
    }
}

/// Bounds for one native online-backup or restore operation.
///
/// The page and step limits apply to both copy directions. A transfer that
/// cannot make progress before the step bound is exhausted fails closed and
/// does not publish its temporary destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSqliteTransferOptions {
    /// Number of `SQLite` pages requested per native backup step.
    pub pages_per_step: u32,
    /// Maximum number of native backup steps, including busy/locked attempts.
    pub max_steps: u32,
}

impl Default for NativeSqliteTransferOptions {
    fn default() -> Self {
        Self {
            pages_per_step: 128,
            max_steps: 1_000_000,
        }
    }
}

const OPERATIONAL_QUERIES: &[(&str, &str)] = &[
    (
        "retired_authority_fences_v1",
        "SELECT * FROM retired_authority_fences_v1 ORDER BY witness_id",
    ),
    (
        "principals",
        "SELECT * FROM principals ORDER BY principal_id",
    ),
    ("runners", "SELECT * FROM runners ORDER BY runner_id"),
    (
        "capabilities",
        "SELECT * FROM capabilities ORDER BY capability_id",
    ),
    (
        "capability_scopes",
        "SELECT * FROM capability_scopes ORDER BY capability_id, scope",
    ),
    (
        "runner_capability_memberships",
        "SELECT * FROM runner_capability_memberships ORDER BY capability_id, room_id, member_id",
    ),
    (
        "authority_change_receipts",
        "SELECT * FROM authority_change_receipts ORDER BY change_id",
    ),
    (
        "authority_audit",
        "SELECT * FROM authority_audit ORDER BY audit_seq",
    ),
    (
        "room_operational_history_roots_v2",
        "SELECT * FROM room_operational_history_roots_v2 ORDER BY room_id, domain",
    ),
    (
        "room_operational_mmr_receipts_v1",
        "SELECT * FROM room_operational_mmr_receipts_v1 ORDER BY room_id, domain",
    ),
    (
        "room_operational_mmr_nodes_v1",
        "SELECT * FROM room_operational_mmr_nodes_v1 ORDER BY room_id, domain, height, start_index",
    ),
    (
        "room_integrity",
        "SELECT * FROM room_integrity ORDER BY room_id",
    ),
    (
        "room_members",
        "SELECT * FROM room_members ORDER BY room_id, member_id",
    ),
    (
        "timers",
        "SELECT * FROM timers ORDER BY room_id, timer_id, generation",
    ),
    (
        "observation_frames",
        "SELECT * FROM observation_frames ORDER BY room_id, member_id, frame_seq",
    ),
    (
        "observation_consequences",
        "SELECT * FROM observation_consequences ORDER BY room_id, member_id, cause_room_seq",
    ),
    (
        "activation_decisions",
        "SELECT * FROM activation_decisions ORDER BY room_id, cause_room_seq, decision_id",
    ),
    (
        "activation_intents",
        "SELECT * FROM activation_intents ORDER BY activation_id",
    ),
    (
        "activation_operation_receipts",
        "SELECT * FROM activation_operation_receipts ORDER BY room_id, operation_id",
    ),
    (
        "semantic_receipts",
        "SELECT * FROM semantic_receipts ORDER BY operation_kind, operation_identity_bytes",
    ),
    (
        "external_input_preparations",
        "SELECT * FROM external_input_preparations ORDER BY operation_identity_bytes",
    ),
    (
        "integrity_incidents",
        "SELECT * FROM integrity_incidents ORDER BY room_id, incident_seq",
    ),
];

const OPTIONAL_OPERATIONAL_TABLES: &[&str] = &[
    "room_operational_history_roots_v2",
    "room_operational_mmr_receipts_v1",
    "room_operational_mmr_nodes_v1",
];

/// Extracts all modeled operational tables with a per-query row and byte bound.
///
/// The source is opened read-only and no canonical value is decoded or
/// re-encoded. Missing tables and malformed native values fail closed so a
/// caller cannot mistake a partial extraction for a complete one.
///
/// # Errors
///
/// Returns an error when the source is not a regular file, a modeled table is
/// absent, native reading fails, or the configured row/byte bounds are exceeded.
pub fn extract_operational_rows(
    path: &Path,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteOperationalRowsV1, NativeSqliteError> {
    extract_operational_rows_with_retained_hook(path, limits, |_| Ok(()))
}

fn extract_operational_rows_with_retained_hook<F>(
    path: &Path,
    limits: NativeSqliteLimits,
    after_retention: F,
) -> Result<NativeSqliteOperationalRowsV1, NativeSqliteError>
where
    F: FnOnce(&Path) -> Result<(), NativeSqliteError>,
{
    let retained = RetainedNativeSqliteSource::open(path)?;
    after_retention(path)?;
    extract_operational_rows_retained(path, &retained.file, limits)
}

/// Extracts operational rows from one exact retained `SQLite` file.
///
/// # Errors
///
/// Returns an error when the named path no longer identifies the retained
/// object or bounded extraction fails.
pub fn extract_operational_rows_retained(
    path: &Path,
    file: &File,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteOperationalRowsV1, NativeSqliteError> {
    let identity = native_file_identity(file)?;
    if native_path_identity(path).ok() != Some(identity) {
        return Err(NativeSqliteError::InvalidPath);
    }
    let coordinate = retained_native_query_coordinate(path, file, identity)?;
    let rows =
        extract_operational_rows_at_coordinate(path, coordinate.file(), coordinate.path(), limits)?;
    if native_file_identity(file)? != identity || native_path_identity(path).ok() != Some(identity)
    {
        return Err(NativeSqliteError::InvalidPath);
    }
    coordinate.revalidate()?;
    Ok(rows)
}

fn extract_operational_rows_at_coordinate(
    validation_path: &Path,
    file: &File,
    path: &Path,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteOperationalRowsV1, NativeSqliteError> {
    validate_limits(limits)?;
    validate_regular_file(validation_path)?;
    let actual_tables = rows(
        file,
        path,
        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name;",
        limits,
    )?
    .into_iter()
    .filter_map(|row| row.into_iter().next())
    .collect::<BTreeSet<_>>();
    let mut tables = BTreeMap::new();
    for (table, sql) in OPERATIONAL_QUERIES {
        if !actual_tables.contains(*table) && !OPTIONAL_OPERATIONAL_TABLES.contains(table) {
            return Err(NativeSqliteError::InvalidRow {
                what: "operational table missing",
            });
        }
        if !actual_tables.contains(*table) {
            tables.insert((*table).to_owned(), Vec::new());
            continue;
        }
        let values = native_rows(
            file,
            path,
            &format!("{sql} LIMIT {};", bounded_limit(limits.max_rows)?),
            limits,
        )?;
        tables.insert(
            (*table).to_owned(),
            values
                .into_iter()
                .map(|values| NativeSqliteRowV1 {
                    table: (*table).to_owned(),
                    values,
                })
                .collect(),
        );
    }
    Ok(NativeSqliteOperationalRowsV1 { tables })
}

/// Extracts the provider-side evidence available in the modeled `SQLite`
/// schema, read-only and with the same bounds as verification.
///
/// This is intentionally a raw evidence bridge.  It preserves exact BLOBs,
/// exposes the newest valid paired snapshot per Room, and leaves deployment
/// metadata/resource evidence absent when the source schema does not contain
/// it.  A caller can pass the returned values to a
/// [`NativeRestoreEvidenceV1`](crate::NativeRestoreEvidenceV1) builder; the
/// provider-neutral verifier will then block readiness for every absent
/// required field.
///
/// # Errors
///
/// Returns an error when the source is not a regular file, a modeled table
/// cannot be read losslessly, or a configured bound is exceeded.
#[allow(clippy::too_many_lines)]
pub fn extract_restore_evidence(
    path: &Path,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteRestoreEvidenceV1, NativeSqliteError> {
    extract_restore_evidence_with_retained_hook(path, limits, |_| Ok(()))
}

fn extract_restore_evidence_with_retained_hook<F>(
    path: &Path,
    limits: NativeSqliteLimits,
    after_retention: F,
) -> Result<NativeSqliteRestoreEvidenceV1, NativeSqliteError>
where
    F: FnOnce(&Path) -> Result<(), NativeSqliteError>,
{
    let retained = RetainedNativeSqliteSource::open(path)?;
    after_retention(path)?;
    extract_restore_evidence_retained(path, &retained.file, limits)
}

/// Extracts bounded restore evidence from one exact retained `SQLite` file.
/// The named path must remain bound to the retained object throughout.
///
/// # Errors
///
/// Returns an error when identity changes or exact bounded extraction fails.
pub fn extract_restore_evidence_retained(
    path: &Path,
    file: &File,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteRestoreEvidenceV1, NativeSqliteError> {
    let identity = native_file_identity(file)?;
    if native_path_identity(path).ok() != Some(identity) {
        return Err(NativeSqliteError::InvalidPath);
    }
    let coordinate = retained_native_query_coordinate(path, file, identity)?;
    let evidence =
        extract_restore_evidence_at_coordinate(path, coordinate.file(), coordinate.path(), limits)?;
    if native_file_identity(file)? != identity || native_path_identity(path).ok() != Some(identity)
    {
        return Err(NativeSqliteError::InvalidPath);
    }
    coordinate.revalidate()?;
    Ok(evidence)
}

#[allow(clippy::too_many_lines)]
fn extract_restore_evidence_at_coordinate(
    validation_path: &Path,
    file: &File,
    path: &Path,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteRestoreEvidenceV1, NativeSqliteError> {
    validate_limits(limits)?;
    validate_regular_file(validation_path)?;
    let operational = extract_operational_rows_at_coordinate(validation_path, file, path, limits)?;
    let actual_tables = rows(
        file,
        path,
        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name;",
        limits,
    )?
    .into_iter()
    .filter_map(|row| row.into_iter().next())
    .collect::<BTreeSet<_>>();

    // Optional metadata is source evidence when explicitly persisted. Older
    // databases remain incomplete; no epoch is derived from a Room or file
    // timestamp.
    let (deployment_lineage, storage_epoch) = if actual_tables.contains("canonical_export_metadata")
    {
        let metadata = native_rows(
            file,
            path,
            "SELECT deployment_lineage, storage_epoch FROM canonical_export_metadata WHERE metadata_id = 1 LIMIT 2;",
            limits,
        )?;
        if metadata.is_empty() {
            (None, None)
        } else if metadata.len() != 1 {
            return Err(NativeSqliteError::InvalidRow {
                what: "canonical export metadata cardinality",
            });
        } else {
            let lineage = native_deployment_lineage(&metadata[0], 0)?;
            let epoch =
                u64::try_from(native_integer(&metadata[0], 1, "storage epoch")?).map_err(|_| {
                    NativeSqliteError::InvalidRow {
                        what: "storage epoch",
                    }
                })?;
            if !(1..=MAX_SAFE_INTEGER).contains(&epoch) {
                return Err(NativeSqliteError::InvalidRow {
                    what: "storage epoch",
                });
            }
            (Some(lineage), Some(epoch))
        }
    } else {
        (None, None)
    };

    // A migration contract is emitted only from the durable three-column
    // ledger, and only after every ordered row matches the reviewed ID and
    // checksum bytes. Missing, duplicate, gapped, out-of-order, or tampered
    // evidence remains explicitly absent and therefore blocks readiness.
    let migration_metadata = if actual_tables.contains("schema_migrations") {
        let columns = rows(
            file,
            path,
            "SELECT name FROM pragma_table_info('schema_migrations');",
            limits,
        )?;
        let has_checksum = columns
            .iter()
            .any(|row| row.first().is_some_and(|name| name == "source_checksum"));
        if has_checksum {
            let rows = native_rows(
                file,
                path,
                &format!(
                    "SELECT version, migration_id, source_checksum \
                     FROM schema_migrations ORDER BY version, rowid LIMIT {};",
                    bounded_limit(limits.max_rows)?
                ),
                limits,
            )?;
            let valid = SUPPORTED_MIGRATION_COUNTS.contains(&rows.len())
                && rows.iter().enumerate().all(|(index, row)| {
                    native_integer(row, 0, "migration version")
                        .ok()
                        .and_then(|version| usize::try_from(version).ok())
                        == Some(index + 1)
                        && native_text(row, 1, "migration ID").ok()
                            == Some(REQUIRED_MIGRATIONS[index])
                        && native_text(row, 2, "migration checksum").ok()
                            == Some(REQUIRED_MIGRATION_CHECKSUMS[index])
                });
            valid.then(|| {
                rows.into_iter()
                    .map(|values| NativeSqliteRowV1 {
                        table: "schema_migrations".to_owned(),
                        values,
                    })
                    .collect()
            })
        } else {
            None
        }
    } else {
        None
    };

    let identity_witness_present = if actual_tables.contains("deployment_identity_metadata") {
        let rows = native_rows(
            file,
            path,
            "SELECT metadata_id, pack_set_digest, resource_set_digest, canonical_bytes FROM deployment_identity_metadata WHERE metadata_id = 1 LIMIT 2;",
            limits,
        )?;
        rows.len() == 1
            && native_integer(&rows[0], 0, "deployment identity witness").ok() == Some(1)
            && native_blob(&rows[0], 1, "deployment pack-set digest")
                .is_ok_and(|value| value.len() == 32)
            && native_blob(&rows[0], 2, "deployment resource-set digest")
                .is_ok_and(|value| value.len() == 32)
            && native_blob(&rows[0], 3, "deployment identity bytes")
                .is_ok_and(|value| !value.is_empty())
    } else {
        false
    };
    let pack_metadata =
        if identity_witness_present && actual_tables.contains("deployment_pack_identities") {
            let rows = native_rows(
                file,
                path,
                &format!(
                    "SELECT pack_id, revision, pack_digest \
                     FROM deployment_pack_identities ORDER BY pack_id, revision LIMIT {};",
                    bounded_limit(limits.max_rows)?
                ),
                limits,
            )?;
            (!rows.is_empty()).then(|| {
                rows.into_iter()
                    .map(|values| NativeSqliteRowV1 {
                        table: "deployment_pack_identities".to_owned(),
                        values,
                    })
                    .collect()
            })
        } else {
            None
        };
    let resource_metadata = if identity_witness_present
        && actual_tables.contains("deployment_resource_identities")
        && actual_tables.contains("deployment_resource_blobs")
    {
        let rows = native_rows(
            file,
            path,
            &format!(
                "SELECT i.resource_kind, i.resource_identity, i.size_bytes, i.resource_digest, \
                        b.resource_bytes, b.resource_digest \
                 FROM deployment_resource_identities i \
                 LEFT JOIN deployment_resource_blobs b \
                   ON b.resource_kind = i.resource_kind \
                  AND b.resource_identity = i.resource_identity \
                 ORDER BY i.resource_kind, i.resource_identity LIMIT {};",
                bounded_limit(limits.max_rows)?
            ),
            limits,
        )?;
        for row in &rows {
            let size = usize::try_from(native_integer(row, 2, "resource size")?).map_err(|_| {
                NativeSqliteError::InvalidRow {
                    what: "resource size",
                }
            })?;
            let identity_digest = native_blob(row, 3, "resource identity digest")?;
            let bytes = native_blob(row, 4, "resource bytes")?;
            let payload_digest = native_blob(row, 5, "resource payload digest")?;
            if size != bytes.len()
                || identity_digest != payload_digest
                || identity_digest != blake3::hash(bytes).as_bytes()
            {
                return Err(NativeSqliteError::InvalidRow {
                    what: "resource payload identity",
                });
            }
        }
        Some(
            rows.into_iter()
                .map(|values| NativeSqliteRowV1 {
                    table: "deployment_resources".to_owned(),
                    values,
                })
                .collect(),
        )
    } else {
        None
    };

    let genesis = native_rows(
        file,
        path,
        &format!(
            "SELECT room_id, genesis_bytes \
             FROM room_genesis \
             ORDER BY room_id LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
    )?;
    let transitions = native_rows(
        file,
        path,
        &format!(
            "SELECT room_id, room_seq, transition_hash, previous_lineage_hash, transition_bytes \
             FROM transitions ORDER BY room_id, room_seq LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
    )?;
    let mut canonical_records = BTreeMap::<String, Vec<NativeSqliteCanonicalRecordV1>>::new();
    for row in genesis {
        let room_id = native_text(&row, 0, "genesis room")?.to_owned();
        let bytes = native_blob(&row, 1, "genesis bytes")?.to_vec();
        let digest = canonical_genesis_hash(&bytes).ok_or(NativeSqliteError::InvalidRow {
            what: "genesis bytes",
        })?;
        canonical_records
            .entry(room_id.clone())
            .or_default()
            .push(NativeSqliteCanonicalRecordV1 {
                room_id,
                room_seq: 0,
                digest,
                previous_digest: None,
                bytes,
            });
    }
    for row in transitions {
        let room_id = native_text(&row, 0, "transition room")?.to_owned();
        let room_seq = native_integer(&row, 1, "transition sequence")?;
        let digest = parse_storage_digest(
            native_text(&row, 2, "transition digest")?,
            "transition digest",
        )?;
        let previous_digest = parse_storage_digest(
            native_text(&row, 3, "previous transition digest")?,
            "previous transition digest",
        )?;
        let bytes = native_blob(&row, 4, "transition bytes")?.to_vec();
        canonical_records
            .entry(room_id.clone())
            .or_default()
            .push(NativeSqliteCanonicalRecordV1 {
                room_id,
                room_seq: u64::try_from(room_seq).map_err(|_| NativeSqliteError::InvalidRow {
                    what: "transition sequence",
                })?,
                digest,
                previous_digest: Some(previous_digest),
                bytes,
            });
    }
    for records in canonical_records.values_mut() {
        records.sort_by_key(|record| record.room_seq);
    }

    let materializations = native_rows(
        file,
        path,
        &format!(
            "SELECT room_id, core_state_bytes, activity_state_bytes \
             FROM room_materializations ORDER BY room_id LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
    )?
    .into_iter()
    .map(|row| {
        Ok((
            native_text(&row, 0, "materialization room")?.to_owned(),
            (
                native_blob(&row, 1, "core materialization")?.to_vec(),
                native_blob(&row, 2, "activity materialization")?.to_vec(),
            ),
        ))
    })
    .collect::<Result<BTreeMap<_, _>, NativeSqliteError>>()?;

    let integrity =
        native_rows(
            file,
            path,
            &format!(
                "SELECT room_id, status, generation FROM room_integrity ORDER BY room_id LIMIT {};",
                bounded_limit(limits.max_rows)?
            ),
            limits,
        )?
        .into_iter()
        .map(|row| {
            let generation = u64::try_from(native_integer(&row, 2, "integrity generation")?)
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "integrity generation",
                })?;
            Ok((
                native_text(&row, 0, "integrity room")?.to_owned(),
                (
                    native_text(&row, 1, "integrity status")?.to_owned(),
                    generation,
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, NativeSqliteError>>()?;

    let room_rows = if actual_tables.contains("rooms") {
        rows(
            file,
            path,
            &format!(
                "SELECT room_id, CAST(room_seq AS TEXT), genesis_or_transition_hash, core_schema_version, \
                 pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, \
                 hex(complete_head_bytes) FROM rooms ORDER BY room_id LIMIT {};",
                bounded_limit(limits.max_rows)?
            ),
            limits,
        )?
    } else {
        Vec::new()
    };
    let rooms = room_rows
        .iter()
        .map(|row| RoomRow::parse(row))
        .collect::<Result<Vec<_>, _>>()?;
    let room_heads = rooms
        .iter()
        .map(|room| {
            Ok((
                room.id.clone(),
                NativeSqliteRoomHeadV1 {
                    room_seq: room.room_seq,
                    head_digest: room.head.clone(),
                    core_schema_version: room.core_schema.clone(),
                    pack_digest: parse_storage_digest(&room.pack_digest, "pack digest")?,
                    core_state_digest: parse_storage_digest(&room.core_hash, "core hash")?,
                    activity_state_digest: parse_storage_digest(
                        &room.activity_hash,
                        "activity hash",
                    )?,
                    authoritative_state_digest: parse_storage_digest(
                        &room.authoritative_hash,
                        "authoritative hash",
                    )?,
                    complete_head_bytes: room.complete_head.clone(),
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, NativeSqliteError>>()?;
    let snapshots = if actual_tables.contains("room_snapshots") {
        rows(
            file,
            path,
            &format!(
                "SELECT room_id, CAST(room_seq AS TEXT), snapshot_schema_version, \
                 genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, \
                 activity_state_hash, authoritative_state_hash, hex(complete_head_bytes), \
                 hex(core_state_bytes), hex(activity_state_bytes) FROM room_snapshots \
                 ORDER BY room_id, room_seq LIMIT {};",
                bounded_limit(limits.max_rows)?
            ),
            limits,
        )?
    } else {
        Vec::new()
    };
    let mut newest_valid_snapshots = BTreeMap::new();
    for row in snapshots {
        let snapshot = SnapshotRow::parse(&row)?;
        let valid = rooms
            .iter()
            .find(|room| room.id == snapshot.room_id)
            .is_some_and(|room| {
                snapshot.schema == PAIRED_SNAPSHOT_SCHEMA_V1
                    && snapshot.room_seq <= room.room_seq
                    && snapshot.authoritative_hash == room.authoritative_hash
                    && complete_head_matches_snapshot(&snapshot)
                    && canonical_core_materialization_hash(
                        &snapshot.core_bytes,
                        &snapshot.pack_digest,
                    )
                    .as_ref()
                        == Some(&parse_digest_value(&snapshot.core_hash))
                    && canonical_activity_materialization_hash(
                        &snapshot.activity_bytes,
                        &snapshot.pack_digest,
                    )
                    .as_ref()
                        == Some(&parse_digest_value(&snapshot.activity_hash))
            });
        if valid {
            let candidate = NativeSqliteSnapshotEvidenceV1 {
                room_id: snapshot.room_id.clone(),
                room_seq: snapshot.room_seq,
                complete_head_bytes: snapshot.complete_head,
                core_state_bytes: snapshot.core_bytes,
                activity_state_bytes: snapshot.activity_bytes,
                lineage: snapshot.lineage,
                core_state_hash: parse_digest_value(&snapshot.core_hash),
                activity_state_hash: parse_digest_value(&snapshot.activity_hash),
            };
            newest_valid_snapshots
                .entry(snapshot.room_id)
                .and_modify(|current: &mut NativeSqliteSnapshotEvidenceV1| {
                    if candidate.room_seq > current.room_seq {
                        *current = candidate.clone();
                    }
                })
                .or_insert(candidate);
        }
    }

    Ok(NativeSqliteRestoreEvidenceV1 {
        operational,
        canonical_records,
        materializations,
        newest_valid_snapshots,
        integrity,
        deployment_lineage,
        storage_epoch,
        migration_metadata,
        pack_metadata,
        resource_metadata,
        backend: None,
        native_point: None,
        room_membership: None,
        room_heads,
    })
}

/// Verifies a copied `SQLite` file without mutating it.
///
/// # Errors
///
/// Returns an error when the input is not a regular file, a native query
/// fails, a query exceeds its bound, or the fixed native evidence shape cannot
/// be decoded.
#[allow(clippy::too_many_lines)]
pub fn verify_file(
    path: &Path,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteVerificationReportV1, NativeSqliteError> {
    verify_file_with_retained_hook(path, limits, |_| Ok(()))
}

fn verify_file_with_retained_hook<F>(
    path: &Path,
    limits: NativeSqliteLimits,
    after_retention: F,
) -> Result<NativeSqliteVerificationReportV1, NativeSqliteError>
where
    F: FnOnce(&Path) -> Result<(), NativeSqliteError>,
{
    let retained = RetainedNativeSqliteSource::open(path)?;
    after_retention(path)?;
    verify_retained_file(path, &retained.file, limits)
}

/// Verifies one exact retained `SQLite` file without reopening its mutable
/// pathname on Unix. The named path must still identify the retained object
/// before and after the complete bounded verification.
///
/// # Errors
///
/// Returns an error when the retained object and named path differ, or when
/// bounded native verification rejects the exact object.
pub fn verify_retained_file(
    path: &Path,
    file: &File,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteVerificationReportV1, NativeSqliteError> {
    let identity = native_file_identity(file)?;
    if native_path_identity(path).ok() != Some(identity) {
        return Err(NativeSqliteError::InvalidPath);
    }
    let coordinate = retained_native_query_coordinate(path, file, identity)?;
    let report = verify_file_at_coordinate(path, coordinate.file(), coordinate.path(), limits)?;
    if native_file_identity(file)? != identity || native_path_identity(path).ok() != Some(identity)
    {
        return Err(NativeSqliteError::InvalidPath);
    }
    coordinate.revalidate()?;
    Ok(report)
}

/// Recomputes the exact logical digest persisted by the `SQLite` transfer-point
/// lifecycle from one retained, standalone backup object.
///
/// The lifecycle table itself is deliberately excluded, matching the producer:
/// the copied backup contains `source_authoritative` while the live source is
/// advanced to `transfer_pending` after capture. Every other user schema item
/// and stored value participates in the digest.
///
/// # Errors
///
/// Returns an error when the retained main object or its parent changes, the
/// database is not standalone DELETE-journal evidence, a sidecar exists, a
/// query exceeds `limits`, or the exact logical rows cannot be read.
pub fn durable_transfer_point_digest_retained(
    path: &Path,
    file: &File,
    limits: NativeSqliteLimits,
) -> Result<[u8; 32], NativeSqliteError> {
    validate_limits(limits)?;
    let identity = native_file_identity(file)?;
    if native_path_identity(path).ok() != Some(identity) {
        return Err(NativeSqliteError::InvalidPath);
    }
    let coordinate = retained_native_query_coordinate(path, file, identity)?;
    let connection = open_native_read_connection(coordinate.file(), coordinate.path())?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .and_then(|()| connection.execute_batch("PRAGMA query_only=ON; BEGIN;"))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let digest = durable_transfer_point_digest_at_connection(&connection, limits)?;
    drop(connection);
    coordinate.revalidate()?;
    Ok(digest)
}

/// Incrementally verifies one retained `SQLite` backup and derives the exact
/// transfer-point/native-operational manifest inputs for a v2 stream export.
///
/// The scan never constructs `BackupImageV1`, `NativeSqliteOperationalRowsV1`, or
/// a full record vector. It hashes rows as they are read and applies only
/// per-row/schema bounds, allowing a source to exceed legacy 100k/64 MiB
/// bundle limits while still proving that its named backup object stayed
/// stable through the complete scan.
pub fn verify_retained_file_streaming_v2(
    path: &Path,
    file: &File,
    limits: NativeSqliteStreamingLimitsV2,
) -> Result<NativeSqliteStreamingVerificationV2, NativeSqliteError> {
    if limits.max_row_bytes == 0 || limits.max_tables == 0 {
        return Err(NativeSqliteError::OutputBoundExceeded);
    }
    let identity = native_file_identity(file)?;
    if native_path_identity(path).ok() != Some(identity) {
        return Err(NativeSqliteError::InvalidPath);
    }
    let coordinate = retained_native_query_coordinate(path, file, identity)?;
    let connection = open_native_read_connection(coordinate.file(), coordinate.path())?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .and_then(|()| connection.execute_batch("PRAGMA query_only=ON; BEGIN;"))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", (), |row| row.get(0))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    if integrity != "ok" {
        return Err(NativeSqliteError::InvalidRow {
            what: "streaming integrity check",
        });
    }
    let actual_tables = connection
        .prepare(
            r"
SELECT name FROM sqlite_schema
WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
ORDER BY name",
        )
        .and_then(|mut statement| {
            statement
                .query_map((), |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    if actual_tables.len() > limits.max_tables {
        return Err(NativeSqliteError::OutputBoundExceeded);
    }
    let actual_set = actual_tables.iter().cloned().collect::<BTreeSet<_>>();
    if REQUIRED_TABLES
        .iter()
        .any(|table| !actual_set.contains(*table))
    {
        return Err(NativeSqliteError::InvalidRow {
            what: "streaming required table",
        });
    }
    verify_streaming_migration_contract_at_connection(&connection, limits)?;
    let transfer_point_digest =
        durable_transfer_point_digest_streaming_at_connection(&connection, &actual_tables, limits)?;
    let mut operational_hasher = blake3::Hasher::new();
    operational_hasher.update(b"worldstream/sqlite-stream-operational/v2");
    let mut operational_relation_counts = BTreeMap::new();
    for (table, _) in OPERATIONAL_QUERIES {
        if !actual_set.contains(*table) {
            operational_relation_counts.insert((*table).to_owned(), 0);
            continue;
        }
        let quoted = table.replace('"', r#""""#);
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY rowid"))
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        let columns = statement.column_count();
        let mut rows = statement
            .query(())
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        let mut count = 0_u64;
        while let Some(row) = rows.next().map_err(|_| NativeSqliteError::QueryFailed)? {
            operational_hasher.update(table.as_bytes());
            digest_streaming_native_row(&mut operational_hasher, row, columns, limits)?;
            count = count
                .checked_add(1)
                .ok_or(NativeSqliteError::OutputBoundExceeded)?;
        }
        operational_relation_counts.insert((*table).to_owned(), count);
    }
    drop(connection);
    if native_file_identity(file)? != identity || native_path_identity(path).ok() != Some(identity)
    {
        return Err(NativeSqliteError::InvalidPath);
    }
    coordinate.revalidate()?;
    Ok(NativeSqliteStreamingVerificationV2 {
        transfer_point_digest,
        operational_relation_counts,
        operational_row_digest: *operational_hasher.finalize().as_bytes(),
    })
}

/// Verifies the small migration ledger incrementally without treating its
/// row count as proof of the reviewed contract.  The accepted counts are
/// deliberately enumerated release prefixes, and every row must carry the
/// matching contiguous version, logical ID, and source checksum.
fn verify_streaming_migration_contract_at_connection(
    connection: &Connection,
    limits: NativeSqliteStreamingLimitsV2,
) -> Result<(), NativeSqliteError> {
    let mut statement = connection
        .prepare(
            "SELECT version, migration_id, source_checksum \
             FROM schema_migrations ORDER BY version, rowid",
        )
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut rows = statement
        .query(())
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut count = 0_usize;
    while let Some(row) = rows.next().map_err(|_| NativeSqliteError::QueryFailed)? {
        let Some(expected_id) = REQUIRED_MIGRATIONS.get(count) else {
            return Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            });
        };
        let expected_checksum =
            REQUIRED_MIGRATION_CHECKSUMS
                .get(count)
                .ok_or(NativeSqliteError::InvalidRow {
                    what: "streaming migration contract",
                })?;
        let ValueRef::Integer(version) =
            row.get_ref(0).map_err(|_| NativeSqliteError::QueryFailed)?
        else {
            return Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            });
        };
        let ValueRef::Text(migration_id) =
            row.get_ref(1).map_err(|_| NativeSqliteError::QueryFailed)?
        else {
            return Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            });
        };
        let ValueRef::Text(checksum) =
            row.get_ref(2).map_err(|_| NativeSqliteError::QueryFailed)?
        else {
            return Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            });
        };
        if migration_id.len() > limits.max_row_bytes || checksum.len() > limits.max_row_bytes {
            return Err(NativeSqliteError::OutputBoundExceeded);
        }
        if usize::try_from(version).ok() != Some(count + 1)
            || migration_id != expected_id.as_bytes()
            || checksum != expected_checksum.as_bytes()
        {
            return Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            });
        }
        count = count
            .checked_add(1)
            .ok_or(NativeSqliteError::OutputBoundExceeded)?;
    }
    if !SUPPORTED_MIGRATION_COUNTS.contains(&count) {
        return Err(NativeSqliteError::InvalidRow {
            what: "streaming migration contract",
        });
    }
    Ok(())
}

fn durable_transfer_point_digest_streaming_at_connection(
    connection: &Connection,
    tables: &[String],
    limits: NativeSqliteStreamingLimitsV2,
) -> Result<[u8; 32], NativeSqliteError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/sqlite-transfer-point/v1");
    let mut statement = connection
        .prepare(
            r"
SELECT type, name, tbl_name, sql FROM sqlite_schema
WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
ORDER BY type, name, tbl_name",
        )
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut rows = statement
        .query(())
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    while let Some(row) = rows.next().map_err(|_| NativeSqliteError::QueryFailed)? {
        for index in 0..4 {
            let ValueRef::Text(value) = row
                .get_ref(index)
                .map_err(|_| NativeSqliteError::QueryFailed)?
            else {
                return Err(NativeSqliteError::InvalidRow {
                    what: "streaming digest schema",
                });
            };
            if value.len() > limits.max_row_bytes {
                return Err(NativeSqliteError::OutputBoundExceeded);
            }
            digest_native_length(&mut hasher, value.len())?;
            hasher.update(value);
        }
    }
    drop(rows);
    drop(statement);
    for table in tables {
        if table == "source_transfer_lifecycle" {
            continue;
        }
        digest_native_length(&mut hasher, table.len())?;
        hasher.update(table.as_bytes());
        let quoted = table.replace('"', r#""""#);
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY rowid"))
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        let columns = statement.column_count();
        let mut rows = statement
            .query(())
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        while let Some(row) = rows.next().map_err(|_| NativeSqliteError::QueryFailed)? {
            hasher.update(b"row");
            digest_streaming_native_row(&mut hasher, row, columns, limits)?;
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

fn digest_streaming_native_row(
    hasher: &mut blake3::Hasher,
    row: &Row<'_>,
    columns: usize,
    limits: NativeSqliteStreamingLimitsV2,
) -> Result<(), NativeSqliteError> {
    let mut row_bytes = 0_usize;
    for index in 0..columns {
        match row
            .get_ref(index)
            .map_err(|_| NativeSqliteError::QueryFailed)?
        {
            ValueRef::Null => {
                hasher.update(&[0]);
            }
            ValueRef::Integer(value) => {
                row_bytes = row_bytes.saturating_add(std::mem::size_of::<i64>());
                hasher.update(&[1]);
                hasher.update(&value.to_le_bytes());
            }
            ValueRef::Real(value) => {
                row_bytes = row_bytes.saturating_add(std::mem::size_of::<f64>());
                hasher.update(&[2]);
                hasher.update(&value.to_bits().to_le_bytes());
            }
            ValueRef::Text(value) => {
                row_bytes = row_bytes.saturating_add(value.len());
                hasher.update(&[3]);
                digest_native_length(hasher, value.len())?;
                hasher.update(value);
            }
            ValueRef::Blob(value) => {
                row_bytes = row_bytes.saturating_add(value.len());
                hasher.update(&[4]);
                digest_native_length(hasher, value.len())?;
                hasher.update(value);
            }
        }
        if row_bytes > limits.max_row_bytes {
            return Err(NativeSqliteError::OutputBoundExceeded);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn durable_transfer_point_digest_at_connection(
    connection: &Connection,
    limits: NativeSqliteLimits,
) -> Result<[u8; 32], NativeSqliteError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/sqlite-transfer-point/v1");

    let mut schema_statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema \
             WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
             ORDER BY type, name, tbl_name",
        )
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut schema_rows = schema_statement
        .query(())
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut schema_count = 0_usize;
    let mut schema_bytes = 0_usize;
    while let Some(row) = schema_rows
        .next()
        .map_err(|_| NativeSqliteError::QueryFailed)?
    {
        schema_count = schema_count.saturating_add(1);
        if schema_count > limits.max_rows {
            return Err(NativeSqliteError::OutputBoundExceeded);
        }
        for index in 0..4 {
            let value = row
                .get_ref(index)
                .map_err(|_| NativeSqliteError::QueryFailed)?;
            let ValueRef::Text(value) = value else {
                return Err(NativeSqliteError::InvalidRow {
                    what: "transfer digest schema value",
                });
            };
            schema_bytes = schema_bytes.saturating_add(value.len());
            if schema_bytes > limits.max_output_bytes {
                return Err(NativeSqliteError::OutputBoundExceeded);
            }
            digest_native_length(&mut hasher, value.len())?;
            hasher.update(value);
        }
    }
    drop(schema_rows);
    drop(schema_statement);

    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name <> 'source_transfer_lifecycle' \
             ORDER BY name",
        )
        .and_then(|mut statement| {
            statement
                .query_map((), |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let table_name_bytes = tables.iter().map(String::len).sum::<usize>();
    if tables.len() > limits.max_rows || table_name_bytes > limits.max_output_bytes {
        return Err(NativeSqliteError::OutputBoundExceeded);
    }

    for table in tables {
        digest_native_length(&mut hasher, table.len())?;
        hasher.update(table.as_bytes());
        let quoted = table.replace('"', "\"\"");
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY rowid"))
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        let column_count = statement.column_count();
        let mut rows = statement
            .query(())
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        let mut row_count = 0_usize;
        let mut value_bytes = 0_usize;
        while let Some(row) = rows.next().map_err(|_| NativeSqliteError::QueryFailed)? {
            row_count = row_count.saturating_add(1);
            if row_count > limits.max_rows {
                return Err(NativeSqliteError::OutputBoundExceeded);
            }
            hasher.update(b"row");
            for index in 0..column_count {
                match row
                    .get_ref(index)
                    .map_err(|_| NativeSqliteError::QueryFailed)?
                {
                    ValueRef::Null => {
                        hasher.update(&[0]);
                    }
                    ValueRef::Integer(value) => {
                        value_bytes = value_bytes.saturating_add(std::mem::size_of::<i64>());
                        hasher.update(&[1]);
                        hasher.update(&value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        value_bytes = value_bytes.saturating_add(std::mem::size_of::<f64>());
                        hasher.update(&[2]);
                        hasher.update(&value.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(value) => {
                        value_bytes = value_bytes.saturating_add(value.len());
                        hasher.update(&[3]);
                        digest_native_length(&mut hasher, value.len())?;
                        hasher.update(value);
                    }
                    ValueRef::Blob(value) => {
                        value_bytes = value_bytes.saturating_add(value.len());
                        hasher.update(&[4]);
                        digest_native_length(&mut hasher, value.len())?;
                        hasher.update(value);
                    }
                }
                if value_bytes > limits.max_output_bytes {
                    return Err(NativeSqliteError::OutputBoundExceeded);
                }
            }
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

fn digest_native_length(
    hasher: &mut blake3::Hasher,
    value: usize,
) -> Result<(), NativeSqliteError> {
    let value = u64::try_from(value).map_err(|_| NativeSqliteError::InvalidRow {
        what: "transfer digest length",
    })?;
    hasher.update(&value.to_le_bytes());
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn verify_file_at_coordinate(
    validation_path: &Path,
    file: &File,
    path: &Path,
    limits: NativeSqliteLimits,
) -> Result<NativeSqliteVerificationReportV1, NativeSqliteError> {
    validate_limits(limits)?;
    let metadata =
        fs::symlink_metadata(validation_path).map_err(|_| NativeSqliteError::InvalidPath)?;
    if !metadata.file_type().is_file() {
        return Err(NativeSqliteError::InvalidPath);
    }

    let query_only = scalar(
        file,
        path,
        "SELECT CAST(query_only AS TEXT) FROM pragma_query_only;",
        limits,
    )? == "1";
    let engine_version = scalar(file, path, "SELECT sqlite_version();", limits)?;
    let integrity_check = scalar(file, path, "PRAGMA integrity_check;", limits)?;
    let mut diagnostics = Vec::new();
    if !query_only {
        diagnostic(&mut diagnostics, "native_not_query_only", true, "database");
    }
    if integrity_check != "ok" {
        diagnostic(
            &mut diagnostics,
            "sqlite_integrity_check_failed",
            true,
            "database",
        );
    }

    let tables = rows(
        file,
        path,
        "SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name;",
        limits,
    )?;
    let actual_tables = tables
        .iter()
        .filter_map(|row| row.first())
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    for required in REQUIRED_TABLES {
        if !actual_tables.contains(required) {
            diagnostic(&mut diagnostics, "required_table_missing", true, required);
        }
    }

    if actual_tables.contains("source_transfer_lifecycle") {
        let lifecycle_valid = scalar(
            file,
            path,
            r"SELECT CAST(
                 (SELECT count(*) FROM source_transfer_lifecycle) = 1 AND EXISTS(
                     SELECT 1 FROM source_transfer_lifecycle WHERE lifecycle_id = 1 AND (
                         (state = 'source_authoritative'
                          AND source_epoch IS NULL AND target_epoch IS NULL
                          AND backup_path IS NULL AND backup_digest IS NULL
                          AND bundle_hash IS NULL AND target_fingerprint IS NULL
                          AND ((last_aborted_bundle_hash IS NULL
                                AND last_aborted_target_fingerprint IS NULL)
                               OR (length(last_aborted_bundle_hash) = 32
                                   AND length(last_aborted_target_fingerprint) = 32)))
                         OR
                         (state = 'transfer_pending'
                          AND source_epoch BETWEEN 1 AND 9007199254740991
                          AND target_epoch = source_epoch + 1
                          AND backup_path IS NOT NULL AND length(backup_path) > 0
                          AND length(backup_digest) = 32
                          AND ((bundle_hash IS NULL AND target_fingerprint IS NULL)
                               OR (length(bundle_hash) = 32 AND length(target_fingerprint) = 32))
                          AND last_aborted_bundle_hash IS NULL
                          AND last_aborted_target_fingerprint IS NULL)
                         OR
                         (state = 'source_retired'
                          AND source_epoch BETWEEN 1 AND 9007199254740991
                          AND target_epoch = source_epoch + 1
                          AND backup_path IS NOT NULL AND length(backup_path) > 0
                          AND length(backup_digest) = 32
                          AND length(bundle_hash) = 32 AND length(target_fingerprint) = 32
                          AND last_aborted_bundle_hash IS NULL
                          AND last_aborted_target_fingerprint IS NULL)
                     )
                 ) AS TEXT);",
            limits,
        )? == "1";
        if !lifecycle_valid {
            diagnostic(
                &mut diagnostics,
                "source_transfer_lifecycle_mismatch",
                true,
                "source_transfer_lifecycle",
            );
        }
    }

    let migration_columns = rows_if_table(
        file,
        path,
        "SELECT name FROM pragma_table_info('schema_migrations');",
        limits,
        &actual_tables,
        "schema_migrations",
    )?;
    let has_persisted_checksums = migration_columns
        .iter()
        .any(|row| row.first().is_some_and(|name| name == "source_checksum"));
    let migration_query = if has_persisted_checksums {
        format!(
            "SELECT CAST(version AS TEXT), migration_id, source_checksum \
             FROM schema_migrations ORDER BY version, rowid LIMIT {};",
            bounded_limit(limits.max_rows)?
        )
    } else {
        format!(
            "SELECT CAST(version AS TEXT), migration_id \
             FROM schema_migrations ORDER BY version, rowid LIMIT {};",
            bounded_limit(limits.max_rows)?
        )
    };
    let migrations = rows_if_table(
        file,
        path,
        &migration_query,
        limits,
        &actual_tables,
        "schema_migrations",
    )?;
    let migration_count = migrations.len();
    for (index, row) in migrations.iter().enumerate() {
        let valid = REQUIRED_MIGRATIONS.get(index).is_some_and(|expected| {
            row.len() == 3
                && row[0].parse::<usize>().ok() == Some(index + 1)
                && row[1] == *expected
                && row[2] == REQUIRED_MIGRATION_CHECKSUMS[index]
        });
        if !valid {
            diagnostic(
                &mut diagnostics,
                "migration_contract_mismatch",
                true,
                "schema",
            );
            break;
        }
    }
    if !SUPPORTED_MIGRATION_COUNTS.contains(&migrations.len()) {
        diagnostic(
            &mut diagnostics,
            "migration_contract_mismatch",
            true,
            "schema",
        );
    }

    let export_metadata = rows_if_table(
        file,
        path,
        &format!(
            "SELECT CAST(metadata_id AS TEXT), deployment_lineage, CAST(storage_epoch AS TEXT) \
             FROM canonical_export_metadata ORDER BY metadata_id LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
        &actual_tables,
        "canonical_export_metadata",
    )?;
    if export_metadata.len() != 1
        || export_metadata[0].len() != 3
        || export_metadata[0][0] != "1"
        || !is_valid_deployment_lineage(&export_metadata[0][1])
        || export_metadata[0][2]
            .parse::<u64>()
            .ok()
            .is_none_or(|epoch| !(1..=MAX_SAFE_INTEGER).contains(&epoch))
    {
        diagnostic(
            &mut diagnostics,
            "canonical_export_metadata_mismatch",
            true,
            "canonical export metadata",
        );
    }

    let room_rows = rows_if_table(
        file,
        path,
        &format!(
            "SELECT room_id, CAST(room_seq AS TEXT), genesis_or_transition_hash, core_schema_version, \
             pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, \
             hex(complete_head_bytes) \
             FROM rooms ORDER BY room_id LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
        &actual_tables,
        "rooms",
    )?;
    let genesis_rows = rows_if_table(
        file,
        path,
        &format!(
            "SELECT room_id, hex(genesis_bytes) FROM room_genesis ORDER BY room_id LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
        &actual_tables,
        "room_genesis",
    )?;
    let transition_rows = rows_if_table(
        file,
        path,
        &format!(
            "SELECT room_id, CAST(room_seq AS TEXT), transition_hash, previous_lineage_hash, \
             hex(transition_bytes), core_schema_version, pack_digest, core_state_hash, \
             activity_state_hash, authoritative_state_hash FROM transitions \
             ORDER BY room_id, room_seq LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
        &actual_tables,
        "transitions",
    )?;
    let materialization_rows = rows_if_table(
        file,
        path,
        &format!(
            "SELECT room_id, hex(core_state_bytes), hex(activity_state_bytes) \
             FROM room_materializations ORDER BY room_id LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
        &actual_tables,
        "room_materializations",
    )?;
    let snapshot_rows = rows_if_table(
        file,
        path,
        &format!(
            "SELECT room_id, CAST(room_seq AS TEXT), snapshot_schema_version, \
             genesis_or_transition_hash, core_schema_version, pack_digest, \
             core_state_hash, activity_state_hash, authoritative_state_hash, \
             hex(complete_head_bytes), hex(core_state_bytes), hex(activity_state_bytes) \
             FROM room_snapshots ORDER BY room_id, room_seq LIMIT {};",
            bounded_limit(limits.max_rows)?
        ),
        limits,
        &actual_tables,
        "room_snapshots",
    )?;

    let mut rooms = BTreeMap::new();
    for row in room_rows {
        let room = RoomRow::parse(&row)?;
        if rooms.insert(room.id.clone(), room).is_some() {
            diagnostic(&mut diagnostics, "duplicate_room", true, "room");
        }
    }
    let mut genesis = BTreeMap::new();
    for row in genesis_rows {
        let (room_id, bytes) = two_fields(&row, "genesis")?;
        if genesis
            .insert(room_id.to_owned(), decode_hex(bytes, "genesis bytes")?)
            .is_some()
        {
            diagnostic(&mut diagnostics, "duplicate_genesis", true, "room");
        }
    }
    let mut transitions: BTreeMap<String, Vec<TransitionRow>> = BTreeMap::new();
    for row in transition_rows {
        let transition = TransitionRow::parse(&row)?;
        transitions
            .entry(transition.room_id.clone())
            .or_default()
            .push(transition);
    }
    let mut materializations = BTreeMap::new();
    for row in materialization_rows {
        let (room_id, core, activity) = three_fields(&row, "materialization")?;
        materializations.insert(
            room_id.to_owned(),
            (
                decode_hex(core, "core bytes")?,
                decode_hex(activity, "activity bytes")?,
            ),
        );
    }

    let mut room_lineage_digests = BTreeMap::new();
    for (room_id, room) in &rooms {
        let mut valid = true;
        let Some(genesis_bytes) = genesis.get(room_id) else {
            diagnostic(&mut diagnostics, "genesis_missing", true, room_id);
            continue;
        };
        let Some(genesis_digest) = canonical_genesis_hash(genesis_bytes) else {
            diagnostic(&mut diagnostics, "lineage_hash_mismatch", true, room_id);
            continue;
        };
        let room_transitions = transitions.get(room_id).cloned().unwrap_or_default();
        if usize::try_from(room.room_seq).ok() != Some(room_transitions.len()) {
            diagnostic(&mut diagnostics, "transition_count_mismatch", true, room_id);
            valid = false;
        }
        let mut previous = genesis_digest.clone();
        for (index, transition) in room_transitions.iter().enumerate() {
            let expected_seq = index + 1;
            if transition.room_seq != expected_seq as u64
                || canonical_transition_hash(&transition.bytes).as_ref() != Some(&transition.hash)
                || transition.previous != previous
                || transition.core_schema != room.core_schema
                || transition.pack_digest != room.pack_digest
                || transition.core_hash != room.core_hash
                || transition.activity_hash != room.activity_hash
                || transition.authoritative_hash != room.authoritative_hash
                || DigestV1::parse(&transition.core_hash).is_err()
                || DigestV1::parse(&transition.activity_hash).is_err()
                || DigestV1::parse(&transition.authoritative_hash).is_err()
            {
                diagnostic(&mut diagnostics, "lineage_hash_mismatch", true, room_id);
                valid = false;
            }
            previous = transition.hash.clone();
        }
        if room.head != previous
            || room.core_schema != CORE_SCHEMA_VERSION
            || DigestV1::parse(&room.pack_digest).is_err()
            || DigestV1::parse(&room.core_hash).is_err()
            || DigestV1::parse(&room.activity_hash).is_err()
            || DigestV1::parse(&room.authoritative_hash).is_err()
            || !complete_head_matches_room(room)
        {
            diagnostic(&mut diagnostics, "complete_head_mismatch", true, room_id);
            valid = false;
        }
        match materializations.get(room_id) {
            Some((core, activity))
                if canonical_core_materialization_hash(core, &room.pack_digest).as_ref()
                    == Some(&parse_digest_value(&room.core_hash))
                    && canonical_activity_materialization_hash(activity, &room.pack_digest)
                        .as_ref()
                        == Some(&parse_digest_value(&room.activity_hash)) => {}
            _ => {
                diagnostic(
                    &mut diagnostics,
                    "materialization_hash_mismatch",
                    true,
                    room_id,
                );
                valid = false;
            }
        }
        if valid {
            room_lineage_digests.insert(room_id.clone(), room.head.clone());
        }
    }
    for room_id in transitions.keys() {
        if !rooms.contains_key(room_id) {
            diagnostic(&mut diagnostics, "orphan_transition", true, room_id);
        }
    }

    let mut valid_snapshot_count = 0;
    let mut disposable_snapshot_count = 0;
    let mut snapshot_ids = BTreeSet::new();
    for row in &snapshot_rows {
        let snapshot = SnapshotRow::parse(row)?;
        if !snapshot_ids.insert((snapshot.room_id.clone(), snapshot.room_seq)) {
            diagnostic(
                &mut diagnostics,
                "duplicate_paired_snapshot",
                true,
                &snapshot.room_id,
            );
        }
        let valid = rooms.get(&snapshot.room_id).is_some_and(|room| {
            let expected_lineage = if snapshot.room_seq == 0 {
                genesis
                    .get(&snapshot.room_id)
                    .and_then(|bytes| canonical_genesis_hash(bytes))
            } else {
                transitions
                    .get(&snapshot.room_id)
                    .and_then(|items| items.iter().find(|item| item.room_seq == snapshot.room_seq))
                    .map(|item| item.hash.clone())
            };
            snapshot.schema == PAIRED_SNAPSHOT_SCHEMA_V1
                && snapshot.room_seq <= room.room_seq
                && expected_lineage.as_ref() == Some(&snapshot.lineage)
                && snapshot.core_schema == room.core_schema
                && snapshot.pack_digest == room.pack_digest
                && canonical_core_materialization_hash(&snapshot.core_bytes, &snapshot.pack_digest)
                    .as_ref()
                    == Some(&parse_digest_value(&snapshot.core_hash))
                && canonical_activity_materialization_hash(
                    &snapshot.activity_bytes,
                    &snapshot.pack_digest,
                )
                .as_ref()
                    == Some(&parse_digest_value(&snapshot.activity_hash))
                && snapshot.authoritative_hash == room.authoritative_hash
                && complete_head_matches_snapshot(&snapshot)
        });
        if valid {
            valid_snapshot_count += 1;
        } else {
            disposable_snapshot_count += 1;
            diagnostic(
                &mut diagnostics,
                "paired_snapshot_disposable",
                false,
                &snapshot.room_id,
            );
        }
    }
    if snapshot_rows.is_empty() {
        diagnostic(
            &mut diagnostics,
            "paired_snapshots_absent_disposable",
            false,
            "snapshots",
        );
    }

    let operational = if OPERATIONAL_QUERIES.iter().all(|(table, _)| {
        actual_tables.contains(*table) || OPTIONAL_OPERATIONAL_TABLES.contains(table)
    }) {
        if let Ok(rows) =
            extract_operational_rows_at_coordinate(validation_path, file, path, limits)
        {
            Some(rows)
        } else {
            diagnostic(
                &mut diagnostics,
                "operational_extraction_failed",
                true,
                "operational ledgers",
            );
            None
        }
    } else {
        None
    };
    let operational_counts = operational
        .as_ref()
        .map_or_else(OperationalCounts::default, |rows| {
            verify_operational_rows(rows, &rooms, &transitions, &mut diagnostics)
        });

    let transition_count = transitions.values().map(Vec::len).sum();
    let canonical_ready = diagnostics.iter().all(|item| !item.blocking);
    Ok(NativeSqliteVerificationReportV1 {
        engine_version,
        query_only,
        integrity_check,
        migration_count,
        room_count: rooms.len(),
        transition_count,
        snapshot_count: snapshot_rows.len(),
        valid_snapshot_count,
        disposable_snapshot_count,
        canonical_ready,
        room_lineage_digests,
        timer_count: operational_counts.timer_count,
        frame_count: operational_counts.frame_count,
        semantic_receipt_count: operational_counts.semantic_receipt_count,
        activation_intent_count: operational_counts.activation_intent_count,
        activation_receipt_count: operational_counts.activation_receipt_count,
        activation_decision_count: operational_counts.activation_decision_count,
        observation_consequence_count: operational_counts.observation_consequence_count,
        diagnostics,
    })
}

fn complete_head_matches_snapshot(snapshot: &SnapshotRow) -> bool {
    let Ok(head) = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&snapshot.complete_head)
    else {
        return false;
    };
    head.room_id().to_string() == snapshot.room_id
        && head.room_seq().get() == snapshot.room_seq
        && storage_digest_from_core(&head.genesis_or_transition_hash().to_string())
            == snapshot.lineage
        && head.core_schema_version() == snapshot.core_schema
        && storage_digest_from_core(&head.pack_digest().to_string())
            == parse_digest_value(&snapshot.pack_digest)
        && storage_digest_from_core(&head.core_state_hash().to_string())
            == parse_digest_value(&snapshot.core_hash)
        && storage_digest_from_core(&head.activity_state_hash().to_string())
            == parse_digest_value(&snapshot.activity_hash)
        && storage_digest_from_core(&head.authoritative_state_hash().to_string())
            == parse_digest_value(&snapshot.authoritative_hash)
}

fn complete_head_matches_room(room: &RoomRow) -> bool {
    let Ok(head) = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&room.complete_head) else {
        return false;
    };
    head.room_id().to_string() == room.id
        && head.room_seq().get() == room.room_seq
        && storage_digest_from_core(&head.genesis_or_transition_hash().to_string()) == room.head
        && head.core_schema_version() == room.core_schema
        && storage_digest_from_core(&head.pack_digest().to_string())
            == parse_digest_value(&room.pack_digest)
        && storage_digest_from_core(&head.core_state_hash().to_string())
            == parse_digest_value(&room.core_hash)
        && storage_digest_from_core(&head.activity_state_hash().to_string())
            == parse_digest_value(&room.activity_hash)
        && storage_digest_from_core(&head.authoritative_state_hash().to_string())
            == parse_digest_value(&room.authoritative_hash)
}

#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, Default)]
struct OperationalCounts {
    timer_count: usize,
    frame_count: usize,
    semantic_receipt_count: usize,
    activation_intent_count: usize,
    activation_receipt_count: usize,
    activation_decision_count: usize,
    observation_consequence_count: usize,
}

#[allow(clippy::too_many_lines)]
fn verify_operational_rows(
    evidence: &NativeSqliteOperationalRowsV1,
    rooms: &BTreeMap<String, RoomRow>,
    transitions: &BTreeMap<String, Vec<TransitionRow>>,
    diagnostics: &mut Vec<NativeSqliteDiagnosticV1>,
) -> OperationalCounts {
    let mut counts = OperationalCounts::default();
    let room_exists = |room_id: &str| rooms.contains_key(room_id);
    let transition_exists = |room_id: &str, seq: i64| {
        u64::try_from(seq).is_ok_and(|seq| {
            transitions
                .get(room_id)
                .is_some_and(|items| items.iter().any(|item| item.room_seq == seq))
        })
    };

    let mut integrity_ids = BTreeSet::new();
    for row in table_rows(evidence, "room_integrity") {
        let valid = row.values.len() == 3
            && text_value(&row.values, 0).is_some_and(|room_id| {
                let status = text_value(&row.values, 1);
                let generation = integer_value(&row.values, 2);
                room_exists(room_id)
                    && integrity_ids.insert(room_id.to_owned())
                    && matches!(status, Some("healthy" | "faulted" | "quarantined"))
                    && generation.is_some_and(|value| value > 0)
            });
        if !valid {
            diagnostic(diagnostics, "integrity_row_mismatch", true, "integrity");
        }
    }
    for room_id in rooms.keys() {
        if !integrity_ids.contains(room_id) {
            diagnostic(diagnostics, "integrity_row_missing", true, room_id);
        }
    }

    let mut member_heads = BTreeMap::new();
    let mut member_ids = BTreeSet::new();
    let mut live_principals = BTreeSet::new();
    for row in table_rows(evidence, "room_members") {
        let valid = row.values.len() == 14
            && text_value(&row.values, 0).is_some_and(|room_id| {
                let member_id = text_value(&row.values, 1);
                let principal_id = text_value(&row.values, 2);
                let principal_kind = text_value(&row.values, 3);
                let standing = text_value(&row.values, 4);
                let access_mode = text_value(&row.values, 5);
                let role = text_value_or_null(&row.values, 6);
                let membership_bytes = blob_value(&row.values, 7);
                let frame_head = integer_value(&row.values, 8);
                let membership_generation = integer_value(&row.values, 9);
                let retained_frame_floor = integer_value(&row.values, 10);
                let last_ack_frame_seq = integer_value_or_null(&row.values, 11);
                let reset_required_through = integer_value_or_null(&row.values, 12);
                let reset_generation = integer_value(&row.values, 13);
                let key = (room_id.to_owned(), member_id.unwrap_or_default().to_owned());
                room_exists(room_id)
                    && member_id.is_some_and(|value| !value.is_empty())
                    && principal_id.is_some_and(|value| !value.is_empty())
                    && matches!(principal_kind, Some("human" | "agent"))
                    && matches!(standing, Some("enabled" | "suspended" | "departed"))
                    && matches!(access_mode, Some("participant" | "spectator" | "operator"))
                    && ((access_mode == Some("participant")) == role.is_some())
                    && membership_bytes.is_some()
                    && frame_head.is_some_and(|value| value >= 0)
                    && membership_generation.is_some_and(|value| value > 0)
                    && retained_frame_floor.is_some_and(|value| {
                        frame_head.is_some_and(|head| value >= 1 && value <= head.saturating_add(1))
                    })
                    && last_ack_frame_seq.is_some_and(|value| {
                        value.is_none_or(|value| {
                            frame_head.is_some_and(|head| value >= 1 && value <= head)
                        })
                    })
                    && reset_required_through.is_some_and(|value| {
                        value.is_none_or(|value| {
                            frame_head.is_some_and(|head| value >= 0 && value <= head)
                        })
                    })
                    && reset_generation
                        .is_some_and(|value| (0..=9_007_199_254_740_991).contains(&value))
                    && member_ids.insert(key.clone())
                    && (standing == Some("departed")
                        || live_principals.insert((
                            room_id.to_owned(),
                            principal_id.unwrap_or_default().to_owned(),
                        )))
                    && member_heads
                        .insert(
                            key,
                            u64::try_from(frame_head.unwrap_or_default()).unwrap_or_default(),
                        )
                        .is_none()
            });
        if !valid {
            diagnostic(diagnostics, "member_row_mismatch", true, "membership");
        }
    }

    let mut timer_ids = BTreeSet::new();
    let mut scheduled_timer_ids = BTreeSet::new();
    if let Some(rows) = evidence.tables.get("timers") {
        counts.timer_count = rows.len();
        for row in rows {
            let valid = row.values.len() == 6
                && text_value(&row.values, 0).is_some_and(|room_id| {
                    let timer_id = text_value(&row.values, 1);
                    let generation = integer_value(&row.values, 2);
                    let scheduled_for = text_value(&row.values, 3);
                    let payload = blob_value(&row.values, 4);
                    let state = text_value(&row.values, 5);
                    let identity = (
                        room_id.to_owned(),
                        timer_id.unwrap_or_default().to_owned(),
                        generation.unwrap_or_default(),
                    );
                    room_exists(room_id)
                        && timer_id.is_some_and(|value| !value.is_empty())
                        && generation.is_some_and(|value| value > 0)
                        && scheduled_for.is_some_and(|value| !value.is_empty())
                        && payload.is_some()
                        && matches!(state, Some("scheduled" | "cancelled" | "fired"))
                        && timer_ids.insert(identity)
                        && (!matches!(state, Some("scheduled"))
                            || scheduled_timer_ids.insert((
                                room_id.to_owned(),
                                timer_id.unwrap_or_default().to_owned(),
                            )))
                });
            if !valid {
                diagnostic(diagnostics, "timer_row_mismatch", true, "timer");
            }
        }
    }

    let mut frame_ids = BTreeSet::new();
    if let Some(rows) = evidence.tables.get("observation_frames") {
        counts.frame_count = rows.len();
        for row in rows {
            let valid = matches!(row.values.len(), 7 | 8)
                && text_value(&row.values, 0).is_some_and(|room_id| {
                    let member_id = text_value(&row.values, 1);
                    let frame_seq = integer_value(&row.values, 2);
                    let cause_seq = integer_value(&row.values, 3);
                    let payload_hash = text_value(&row.values, 4);
                    let payload = blob_value(&row.values, 5);
                    let retained_at = text_value(&row.values, 6);
                    let identity = (
                        room_id.to_owned(),
                        member_id.unwrap_or_default().to_owned(),
                        frame_seq.unwrap_or_default(),
                    );
                    let expected_hash = payload.map(DigestV1::hash);
                    room_exists(room_id)
                        && member_id.is_some_and(|value| !value.is_empty())
                        && frame_seq.is_some_and(|value| value > 0)
                        && cause_seq.is_some_and(|value| transition_exists(room_id, value))
                        && member_heads.contains_key(&(
                            room_id.to_owned(),
                            member_id.unwrap_or_default().to_owned(),
                        ))
                        && frame_ids.insert(identity)
                        && payload_hash.and_then(|value| DigestV1::parse(value.to_owned()).ok())
                            == expected_hash
                        && retained_at.is_some_and(|value| !value.is_empty())
                        && frame_seq.is_some_and(|value| {
                            member_heads
                                .get(&(
                                    room_id.to_owned(),
                                    member_id.unwrap_or_default().to_owned(),
                                ))
                                .is_some_and(|head| {
                                    u64::try_from(value).is_ok_and(|value| value <= *head)
                                })
                        })
                        && (row.values.len() == 7
                            || integer_value_or_null(&row.values, 7)
                                .is_some_and(|value| value.is_none_or(|value| value >= 0)))
                });
            if !valid {
                diagnostic(diagnostics, "frame_row_mismatch", true, "frame");
            }
        }
    }

    if let Some(rows) = evidence.tables.get("observation_consequences") {
        counts.observation_consequence_count = rows.len();
        for row in rows {
            let valid = matches!(row.values.len(), 6 | 7)
                && text_value(&row.values, 0).is_some_and(|room_id| {
                    let consequence_kind = text_value(&row.values, 3);
                    let payload = blob_value_or_null(&row.values, 4);
                    let projection_hash = text_value_or_null(&row.values, 5);
                    room_exists(room_id)
                        && text_value(&row.values, 1).is_some_and(|value| !value.is_empty())
                        && integer_value(&row.values, 2)
                            .is_some_and(|value| transition_exists(room_id, value))
                        && matches!(consequence_kind, Some("reset_required" | "visibility_lost"))
                        && is_blob_or_null(&row.values, 4)
                        && is_text_or_null(&row.values, 5)
                        && ((consequence_kind == Some("reset_required"))
                            == (payload.is_some() && projection_hash.is_some()))
                        && (row.values.len() == 6
                            || integer_value_or_null(&row.values, 6)
                                .is_some_and(|value| value.is_none_or(|value| value >= 0)))
                });
            if !valid {
                diagnostic(
                    diagnostics,
                    "observation_consequence_mismatch",
                    true,
                    "consequence",
                );
            }
        }
    }

    let mut decision_ids = BTreeSet::new();
    if let Some(rows) = evidence.tables.get("activation_decisions") {
        counts.activation_decision_count = rows.len();
        for row in rows {
            let valid = matches!(row.values.len(), 5 | 6)
                && text_value(&row.values, 0).is_some_and(|room_id| {
                    let decision_id = text_value(&row.values, 2);
                    let target_member_id = text_value_or_null(&row.values, 3);
                    room_exists(room_id)
                        && integer_value(&row.values, 1)
                            .is_some_and(|value| transition_exists(room_id, value))
                        && decision_id.is_some_and(|value| !value.is_empty())
                        && decision_id.is_some_and(|value| {
                            decision_ids.insert((
                                room_id.to_owned(),
                                integer_value(&row.values, 1).unwrap_or_default(),
                                value.to_owned(),
                            ))
                        })
                        && target_member_id.is_none_or(|value| {
                            member_ids.contains(&(room_id.to_owned(), value.to_owned()))
                        })
                        && blob_value(&row.values, 4).is_some()
                        && (row.values.len() == 5
                            || integer_value_or_null(&row.values, 5)
                                .is_some_and(|value| value.is_none_or(|value| value >= 0)))
                });
            if !valid {
                diagnostic(
                    diagnostics,
                    "activation_decision_mismatch",
                    true,
                    "Activation decision",
                );
            }
        }
    }

    verify_mmr_sidecar(evidence, rooms, diagnostics);

    let mut activation_ids = BTreeSet::new();
    if let Some(rows) = evidence.tables.get("activation_intents") {
        counts.activation_intent_count = rows.len();
        for row in rows {
            let valid = (row.values.len() == 19 || row.values.len() == 24)
                && text_value(&row.values, 0).is_some_and(|activation_id| {
                    let room_id = text_value(&row.values, 1);
                    let state = text_value(&row.values, 10);
                    let leased = state == Some("leased");
                    let runner = text_value(&row.values, 13);
                    let claim = text_value(&row.values, 14);
                    let until = text_value(&row.values, 15);
                    let context_hash = blob_value(&row.values, 16);
                    let context = blob_value(&row.values, 17);
                    let retired = integer_value(&row.values, 18);
                    room_id.is_some_and(|room_id| {
                        room_exists(room_id)
                            && integer_value(&row.values, 2)
                                .is_some_and(|value| transition_exists(room_id, value))
                            && text_value(&row.values, 4).is_some_and(|value| !value.is_empty())
                            && matches!(
                                state,
                                Some("pending" | "leased" | "completed" | "expired" | "cancelled")
                            )
                            && integer_value(&row.values, 11).is_some_and(|value| value > 0)
                            && integer_value(&row.values, 12).is_some_and(|value| value >= 0)
                            && text_value(&row.values, 3).is_some_and(|value| !value.is_empty())
                            && activation_ids.insert(activation_id.to_owned())
                            && (leased == (runner.is_some() && claim.is_some() && until.is_some()))
                            && context_hash
                                .is_some_and(|value| value.len() == 32)
                                .then_some(())
                                .is_some()
                                == (context_hash.is_some()
                                    && (context.is_some() || retired == Some(1)))
                            && context.is_none_or(|value| {
                                context_hash
                                    .is_some_and(|hash| blake3::hash(value).as_bytes() == hash)
                            })
                            && (retired == Some(0) || retired == Some(1))
                            // Rows created before migration 0016 retain the
                            // original 19-column shape and remain valid.
                            && (row.values.len() == 19
                                || (text_value(&row.values, 19).is_some()
                                    && integer_value(&row.values, 20)
                                        .is_some_and(|value| value >= 0)
                                    && (text_value_or_null(&row.values, 21).is_none()
                                        || matches!(
                                            text_value_or_null(&row.values, 21),
                                            Some(
                                                "superseded_refresh"
                                                    | "refresh_capacity_exceeded"
                                                    | "refresh_age_exceeded"
                                                    | "completed"
                                                    | "expired"
                                                    | "cancelled"
                                            )
                                        ))
                                    && text_value_or_null(&row.values, 22)
                                        .is_none_or(|value| !value.is_empty())
                                    && text_value_or_null(&row.values, 23)
                                        .is_none_or(|value| !value.is_empty())))
                            && (!leased
                                || integer_value(&row.values, 12).is_some_and(|value| value > 0))
                    })
                });
            if !valid {
                diagnostic(
                    diagnostics,
                    "activation_intent_mismatch",
                    true,
                    "Activation Intent",
                );
            }
        }
    }

    if let Some(rows) = evidence.tables.get("activation_operation_receipts") {
        counts.activation_receipt_count = rows.len();
        for row in rows {
            let valid = row.values.len() == 9
                && text_value(&row.values, 0).is_some_and(|room_id| {
                    let context_hash = blob_value_or_null(&row.values, 7);
                    let context_bytes = blob_value_or_null(&row.values, 8);
                    room_exists(room_id)
                        && text_value(&row.values, 1).is_some_and(|value| !value.is_empty())
                        && matches!(
                            text_value(&row.values, 2),
                            Some("offer" | "claim" | "renew" | "complete" | "release")
                        )
                        && blob_value(&row.values, 3).is_some_and(|value| value.len() == 32)
                        && text_value_or_null(&row.values, 4)
                            .is_none_or(|value| activation_ids.contains(value))
                        && text_value(&row.values, 5).is_some_and(|value| !value.is_empty())
                        && blob_value(&row.values, 6).is_some()
                        && is_blob_or_null(&row.values, 7)
                        && is_blob_or_null(&row.values, 8)
                        && context_hash.is_none_or(|value| value.len() == 32)
                        && (context_hash.is_none()
                            || context_bytes.is_none_or(|bytes| {
                                context_hash
                                    .is_some_and(|hash| blake3::hash(bytes).as_bytes() == hash)
                            }))
                });
            if !valid {
                diagnostic(
                    diagnostics,
                    "activation_receipt_mismatch",
                    true,
                    "Activation receipt",
                );
            }
        }
    }

    if let Some(rows) = evidence.tables.get("semantic_receipts") {
        counts.semantic_receipt_count = rows.len();
        let mut receipt_ids = BTreeSet::new();
        for row in rows {
            let valid = row.values.len() == 12
                && text_value(&row.values, 0).is_some_and(|room_id| {
                    room_exists(room_id)
                        && matches!(
                            text_value(&row.values, 1),
                            Some("action" | "administration" | "timer_fired" | "external_input")
                        )
                        && blob_value(&row.values, 2).is_some_and(|value| {
                            !value.is_empty() && receipt_ids.insert(value.to_vec())
                        })
                        && text_value(&row.values, 3) == Some("worldstream/operation-receipt/v1")
                        && blob_value(&row.values, 4).is_some_and(|value| value.len() == 32)
                        && is_blob_or_null(&row.values, 5)
                        && blob_value(&row.values, 6).is_some()
                        && blob_value(&row.values, 7).is_some()
                        && matches!(
                            text_value(&row.values, 8),
                            Some(
                                "genesis_created"
                                    | "transition_committed"
                                    | "rejection_recorded"
                                    | "no_change_recorded"
                            )
                        )
                        && transition_or_null(&row.values, 9).is_some_and(|seq| {
                            (text_value(&row.values, 8) == Some("transition_committed"))
                                == seq.is_some()
                                && seq.is_none_or(|value| transition_exists(room_id, value))
                        })
                        && (text_value(&row.values, 8) != Some("genesis_created")
                            || blob_value_or_null(&row.values, 5).is_none())
                        && blob_value(&row.values, 10).is_some()
                        && text_value(&row.values, 11).is_some_and(|value| !value.is_empty())
                });
            if !valid {
                diagnostic(
                    diagnostics,
                    "semantic_receipt_mismatch",
                    true,
                    "semantic receipt",
                );
            }
        }
    }

    counts
}

fn verify_mmr_sidecar(
    evidence: &NativeSqliteOperationalRowsV1,
    rooms: &BTreeMap<String, RoomRow>,
    diagnostics: &mut Vec<NativeSqliteDiagnosticV1>,
) {
    let roots = table_rows(evidence, "room_operational_history_roots_v2");
    let receipts = table_rows(evidence, "room_operational_mmr_receipts_v1");
    let nodes = table_rows(evidence, "room_operational_mmr_nodes_v1");
    let domains = [
        ("frames", "observation_frames", 7),
        ("consequences", "observation_consequences", 6),
        ("activation_decisions", "activation_decisions", 5),
    ];
    let mut root_keys = BTreeSet::new();
    for row in roots {
        let valid = row.values.len() == 4
            && text_value(&row.values, 0).is_some_and(|room| rooms.contains_key(room))
            && text_value(&row.values, 1)
                .is_some_and(|domain| domains.iter().any(|(expected, _, _)| expected == &domain))
            && integer_value(&row.values, 2).is_some_and(|count| count >= 0)
            && blob_value(&row.values, 3).is_some_and(|hash| hash.len() == 32)
            && root_keys.insert((
                text_value(&row.values, 0).unwrap_or_default().to_owned(),
                text_value(&row.values, 1).unwrap_or_default().to_owned(),
            ));
        if !valid {
            diagnostic(
                diagnostics,
                "operational_history_root_mismatch",
                true,
                "frozen V2 root",
            );
        }
    }
    let mut receipt_keys = BTreeSet::new();
    for row in receipts {
        let valid = row.values.len() == 4
            && text_value(&row.values, 0).is_some_and(|room| rooms.contains_key(room))
            && text_value(&row.values, 1)
                .is_some_and(|domain| domains.iter().any(|(expected, _, _)| expected == &domain))
            && integer_value(&row.values, 2).is_some_and(|count| count >= 0)
            && blob_value(&row.values, 3).is_some_and(|hash| hash.len() == 32)
            && receipt_keys.insert((
                text_value(&row.values, 0).unwrap_or_default().to_owned(),
                text_value(&row.values, 1).unwrap_or_default().to_owned(),
            ));
        if !valid {
            diagnostic(
                diagnostics,
                "operational_mmr_receipt_mismatch",
                true,
                "MMR receipt",
            );
        }
    }
    for row in nodes {
        let key = (
            text_value(&row.values, 0).unwrap_or_default().to_owned(),
            text_value(&row.values, 1).unwrap_or_default().to_owned(),
        );
        let valid = row.values.len() == 5
            && receipt_keys.contains(&key)
            && integer_value(&row.values, 2).is_some_and(|height| (0..64).contains(&height))
            && integer_value(&row.values, 3).is_some_and(|start| start >= 0)
            && blob_value(&row.values, 4).is_some_and(|hash| hash.len() == 32);
        if !valid {
            diagnostic(
                diagnostics,
                "operational_mmr_node_mismatch",
                true,
                "MMR node",
            );
        }
    }
    for room in rooms.keys() {
        for (domain, table, index) in domains {
            let root = roots.iter().find(|row| {
                text_value(&row.values, 0) == Some(room.as_str())
                    && text_value(&row.values, 1) == Some(domain)
            });
            let receipt = receipts.iter().find(|row| {
                text_value(&row.values, 0) == Some(room.as_str())
                    && text_value(&row.values, 1) == Some(domain)
            });
            let rows = table_rows(evidence, table)
                .iter()
                .filter(|row| text_value(&row.values, 0) == Some(room.as_str()))
                .collect::<Vec<_>>();
            if let Some(root) = root {
                if integer_value(&root.values, 2).and_then(|value| usize::try_from(value).ok())
                    != Some(rows.len())
                {
                    diagnostic(
                        diagnostics,
                        "operational_history_root_count_mismatch",
                        true,
                        room,
                    );
                }
            }
            let Some(receipt) = receipt else {
                if rows.iter().any(|row| {
                    integer_value_or_null(&row.values, index).is_some_and(|value| value.is_some())
                }) {
                    diagnostic(diagnostics, "operational_mmr_inventory_partial", true, room);
                }
                continue;
            };
            if root.and_then(|root| integer_value(&root.values, 2))
                != integer_value(&receipt.values, 2)
            {
                diagnostic(
                    diagnostics,
                    "operational_mmr_root_count_mismatch",
                    true,
                    room,
                );
            }
            let Some(count) =
                integer_value(&receipt.values, 2).and_then(|value| usize::try_from(value).ok())
            else {
                continue;
            };
            let mut indexes = rows
                .iter()
                .filter_map(|row| integer_value_or_null(&row.values, index).flatten())
                .collect::<Vec<_>>();
            indexes.sort_unstable();
            let expected = (0..count).map(|value| value as i64).collect::<Vec<_>>();
            if rows.len() != count || indexes != expected {
                diagnostic(
                    diagnostics,
                    "operational_mmr_leaf_inventory_mismatch",
                    true,
                    room,
                );
            }
        }
    }
}

fn table_rows<'a>(
    evidence: &'a NativeSqliteOperationalRowsV1,
    table: &str,
) -> &'a [NativeSqliteRowV1] {
    evidence.tables.get(table).map_or(&[], Vec::as_slice)
}

fn text_value(values: &[NativeSqliteValueV1], index: usize) -> Option<&str> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Some(value),
        _ => None,
    }
}

fn text_value_or_null(values: &[NativeSqliteValueV1], index: usize) -> Option<&str> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Some(value),
        Some(NativeSqliteValueV1::Null) => None,
        _ => Some("<invalid>"),
    }
}

fn is_text_or_null(values: &[NativeSqliteValueV1], index: usize) -> bool {
    matches!(
        values.get(index),
        Some(NativeSqliteValueV1::Text(_) | NativeSqliteValueV1::Null)
    )
}

fn integer_value(values: &[NativeSqliteValueV1], index: usize) -> Option<i64> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Some(*value),
        _ => None,
    }
}

#[allow(clippy::option_option)]
fn integer_value_or_null(values: &[NativeSqliteValueV1], index: usize) -> Option<Option<i64>> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Some(Some(*value)),
        Some(NativeSqliteValueV1::Null) => Some(None),
        _ => None,
    }
}

fn blob_value(values: &[NativeSqliteValueV1], index: usize) -> Option<&[u8]> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Some(value),
        _ => None,
    }
}

fn blob_value_or_null(values: &[NativeSqliteValueV1], index: usize) -> Option<&[u8]> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Some(value),
        Some(NativeSqliteValueV1::Null) => None,
        _ => Some(&[]),
    }
}

fn is_blob_or_null(values: &[NativeSqliteValueV1], index: usize) -> bool {
    matches!(
        values.get(index),
        Some(NativeSqliteValueV1::Blob(_) | NativeSqliteValueV1::Null)
    )
}

#[allow(clippy::option_option)]
fn transition_or_null(values: &[NativeSqliteValueV1], index: usize) -> Option<Option<i64>> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Some(Some(*value)),
        Some(NativeSqliteValueV1::Null) => Some(None),
        _ => None,
    }
}

fn storage_digest_from_core(value: &str) -> DigestV1 {
    DigestV1::parse(value.strip_prefix("blake3:").unwrap_or(value).to_owned())
        .unwrap_or_else(|_| DigestV1::hash(&[]))
}

fn parse_digest_value(value: &str) -> DigestV1 {
    DigestV1::parse(value.to_owned()).unwrap_or_else(|_| DigestV1::hash(&[]))
}

/// Creates an immutable `SQLite` backup using the bundled online-backup API.
///
/// The source is semantically verified before any destination is created. The
/// destination must not already exist; publication is an atomic no-replace
/// sibling link, and the published file is verified again before this returns.
///
/// # Errors
///
/// Returns an error when the source is not a regular file, the destination
/// already exists, the native backup fails, or either semantic verification
/// fails.
pub fn backup_file(
    source: &Path,
    destination: &Path,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError> {
    backup_file_with_options(source, destination, NativeSqliteTransferOptions::default())
}

/// Creates a backup with an explicit bounded native transfer budget.
///
/// # Errors
///
/// Returns an error when the source or destination is unusable, the transfer
/// budget is invalid or exhausted, native `SQLite` copying fails, cleanup
/// fails, or semantic verification rejects the result.
pub fn backup_file_with_options(
    source: &Path,
    destination: &Path,
    options: NativeSqliteTransferOptions,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError> {
    transfer_file(source, destination, "backup", options, None)
}

/// Restores a `SQLite` backup into a new empty path using the bundled
/// online-backup API.
///
/// Restore has the same fail-closed contract as [`backup_file`]: it refuses to
/// overwrite an existing path and publishes only after destination
/// verification succeeds.
///
/// # Errors
///
/// Returns an error when the source is not a regular file, the destination
/// already exists, the native restore fails, or either semantic verification
/// fails.
pub fn restore_file(
    source: &Path,
    destination: &Path,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError> {
    restore_file_with_options(source, destination, NativeSqliteTransferOptions::default())
}

/// Restores a backup with an explicit bounded native transfer budget.
///
/// # Errors
///
/// Returns an error when the source or destination is unusable, the transfer
/// budget is invalid or exhausted, native `SQLite` copying fails, cleanup
/// fails, or semantic verification rejects the result.
pub fn restore_file_with_options(
    source: &Path,
    destination: &Path,
    options: NativeSqliteTransferOptions,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError> {
    transfer_file(source, destination, "restore", options, None)
}

/// Restores a large retained `SQLite` backup with bounded streaming
/// verification before and after publication.  This is the restore companion
/// to [`verify_retained_file_streaming_v2`]: it has no total-row or
/// total-database-byte cap and never creates a legacy backup image or a
/// deployment-sized record collection.
///
/// The destination is published only after its exact streaming transfer-point
/// and native operational digests equal the retained source.
pub fn restore_file_streaming_v2(
    source: &Path,
    destination: &Path,
) -> Result<NativeSqliteStreamingRestoreReportV2, NativeSqliteError> {
    restore_file_streaming_v2_with_options(
        source,
        destination,
        NativeSqliteTransferOptions::default(),
        NativeSqliteStreamingLimitsV2::default(),
    )
}

/// Restores a large retained `SQLite` backup with explicit native-copy and
/// per-row streaming limits.  Limits constrain each native backup step and
/// each admitted row/schema element, never the total deployment history.
#[allow(clippy::too_many_lines)]
pub fn restore_file_streaming_v2_with_options(
    source: &Path,
    destination: &Path,
    options: NativeSqliteTransferOptions,
    limits: NativeSqliteStreamingLimitsV2,
) -> Result<NativeSqliteStreamingRestoreReportV2, NativeSqliteError> {
    const OPERATION: &str = "streaming restore";

    validate_transfer_options(options)?;
    if limits.max_row_bytes == 0 || limits.max_tables == 0 {
        return Err(NativeSqliteError::OutputBoundExceeded);
    }
    let retained_source = RetainedNativeSqliteSource::open(source)?;
    let publication_parent = validate_new_destination(destination)?;
    let source_verification =
        verify_retained_file_streaming_v2(source, &retained_source.file, limits)?;
    retained_source.revalidate()?;

    let (temporary, mut temporary_file) = temporary_destination(&publication_parent, destination)?;
    let temporary_identity = native_file_identity(&temporary_file)?;
    let temporary_name = temporary
        .file_name()
        .ok_or(NativeSqliteError::InvalidPath)?
        .to_owned();
    publication_parent.require_named()?;
    publication_parent.require_relative_identity(&temporary_name, temporary_identity)?;
    let page_count = match bundled_online_backup(
        source,
        &temporary,
        &retained_source,
        &publication_parent,
        &temporary_name,
        temporary_identity,
        &temporary_file,
        OPERATION,
        options,
        None,
    ) {
        Ok(page_count) => page_count,
        Err(error) => {
            return Err(if scrub_native_file(&temporary_file, temporary_identity) {
                error
            } else {
                NativeSqliteError::CleanupFailed {
                    what: "temporary retained-handle cleanup",
                }
            });
        }
    };
    retained_source.revalidate()?;
    temporary_file = publication_parent
        .reopen_staging_for_publication(&temporary_name, &temporary_file, temporary_identity)
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    let mut published = None;
    let mut source_moved = false;
    let result = (|| {
        publication_parent.require_named()?;
        publication_parent.require_relative_identity(&temporary_name, temporary_identity)?;
        let expected_fingerprint = native_file_fingerprint(&mut temporary_file)?;
        let (mut publication_file, publication_identity) = prepare_native_publication_source(
            &publication_parent,
            &mut temporary_file,
            expected_fingerprint,
        )?;
        source_moved = publish_native_source_without_replacement(
            &publication_parent,
            &mut publication_file,
            publication_identity,
            expected_fingerprint,
            &mut published,
        )?;

        publication_parent.require_named()?;
        let final_file = published.as_ref().ok_or(NativeSqliteError::CleanupFailed {
            what: "published destination handle",
        })?;
        let destination_verification =
            verify_retained_file_streaming_v2(destination, final_file, limits)?;
        retained_source.revalidate()?;
        if source_verification.transfer_point_digest()
            != destination_verification.transfer_point_digest()
            || source_verification.operational_relation_counts()
                != destination_verification.operational_relation_counts()
            || source_verification.operational_row_digest()
                != destination_verification.operational_row_digest()
        {
            return Err(NativeSqliteError::NativeOperationFailed {
                operation: OPERATION,
            });
        }
        let destination_identity = public_native_file_identity(native_file_identity(final_file)?);
        publication_parent.require_named()?;
        publication_parent.require_relative_identity(&temporary_name, temporary_identity)?;
        require_native_file_fingerprint(
            &mut temporary_file,
            temporary_identity,
            expected_fingerprint,
        )?;
        require_native_file_fingerprint(
            &mut publication_file,
            publication_identity,
            expected_fingerprint,
        )?;
        if let Some(final_file) = published.as_mut() {
            let final_identity = native_file_identity(final_file)?;
            require_native_file_fingerprint(final_file, final_identity, expected_fingerprint)?;
            publication_parent
                .require_relative_identity(publication_parent.name(), final_identity)?;
        }
        finish_native_publication_source(
            &publication_parent,
            &temporary_name,
            &temporary_file,
            temporary_identity,
            source_moved,
        )?;
        publication_parent.sync()?;
        publication_parent.require_named()?;
        Ok(NativeSqliteStreamingRestoreReportV2 {
            engine_version: rusqlite::version().to_owned(),
            page_count,
            source_transfer_point_digest: source_verification.transfer_point_digest(),
            destination_transfer_point_digest: destination_verification.transfer_point_digest(),
            operational_relation_counts: source_verification.operational_relation_counts().clone(),
            operational_row_digest: source_verification.operational_row_digest(),
            destination_identity,
        })
    })();
    result.map_err(|error| {
        cleanup_native_publication(
            &temporary,
            temporary_identity,
            &temporary_file,
            &publication_parent,
            published.as_ref(),
            source_moved,
            error,
        )
    })
}

fn transfer_file(
    source: &Path,
    destination: &Path,
    operation: &'static str,
    options: NativeSqliteTransferOptions,
    interrupt_after_steps: Option<u32>,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError> {
    transfer_file_with_publish_hook(
        source,
        destination,
        operation,
        options,
        interrupt_after_steps,
        |_| Ok(()),
    )
}

#[allow(clippy::too_many_lines)]
fn transfer_file_with_publish_hook<F>(
    source: &Path,
    destination: &Path,
    operation: &'static str,
    options: NativeSqliteTransferOptions,
    interrupt_after_steps: Option<u32>,
    before_publish: F,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError>
where
    F: FnOnce(&Path) -> Result<(), NativeSqliteError>,
{
    transfer_file_with_hooks(
        source,
        destination,
        operation,
        options,
        interrupt_after_steps,
        |_, _| Ok(()),
        before_publish,
    )
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
fn transfer_file_with_hooks<F, G>(
    source: &Path,
    destination: &Path,
    operation: &'static str,
    options: NativeSqliteTransferOptions,
    interrupt_after_steps: Option<u32>,
    before_native_open: F,
    before_publish: G,
) -> Result<NativeSqliteTransferReportV1, NativeSqliteError>
where
    F: FnOnce(&Path, &Path) -> Result<(), NativeSqliteError>,
    G: FnOnce(&Path) -> Result<(), NativeSqliteError>,
{
    validate_transfer_options(options)?;
    let retained_source = RetainedNativeSqliteSource::open(source)?;
    let publication_parent = validate_new_destination(destination)?;
    let source_report =
        verify_retained_file(source, &retained_source.file, NativeSqliteLimits::default())?;
    if !source_report.canonical_ready {
        return Err(NativeSqliteError::NativeOperationFailed { operation });
    }
    retained_source.revalidate()?;

    let (temporary, mut temporary_file) = temporary_destination(&publication_parent, destination)?;
    let temporary_identity = native_file_identity(&temporary_file)?;
    let temporary_name = temporary
        .file_name()
        .ok_or(NativeSqliteError::InvalidPath)?
        .to_owned();
    before_native_open(source, &temporary)?;
    retained_source.revalidate()?;
    publication_parent.require_named()?;
    publication_parent.require_relative_identity(&temporary_name, temporary_identity)?;
    let page_count = match bundled_online_backup(
        source,
        &temporary,
        &retained_source,
        &publication_parent,
        &temporary_name,
        temporary_identity,
        &temporary_file,
        operation,
        options,
        interrupt_after_steps,
    ) {
        Ok(page_count) => page_count,
        Err(error) => {
            return Err(if scrub_native_file(&temporary_file, temporary_identity) {
                error
            } else {
                NativeSqliteError::CleanupFailed {
                    what: "temporary retained-handle cleanup",
                }
            });
        }
    };
    retained_source.revalidate()?;
    temporary_file = publication_parent
        .reopen_staging_for_publication(&temporary_name, &temporary_file, temporary_identity)
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    let mut published = None;
    let mut source_moved = false;
    let result = (|| {
        before_publish(destination)?;
        publication_parent.require_named()?;
        publication_parent.require_relative_identity(&temporary_name, temporary_identity)?;
        let expected_fingerprint = native_file_fingerprint(&mut temporary_file)?;
        let (mut publication_file, publication_identity) = prepare_native_publication_source(
            &publication_parent,
            &mut temporary_file,
            expected_fingerprint,
        )?;
        source_moved = publish_native_source_without_replacement(
            &publication_parent,
            &mut publication_file,
            publication_identity,
            expected_fingerprint,
            &mut published,
        )?;

        publication_parent.require_named()?;
        let final_file = published.as_ref().ok_or(NativeSqliteError::CleanupFailed {
            what: "published destination handle",
        })?;
        let destination_report =
            match verify_retained_file(destination, final_file, NativeSqliteLimits::default()) {
                Ok(report) if report.canonical_ready => report,
                Ok(_) => {
                    return Err(NativeSqliteError::NativeOperationFailed { operation });
                }
                Err(error) => return Err(error),
            };
        retained_source.revalidate()?;
        let source_evidence = extract_restore_evidence_retained(
            source,
            &retained_source.file,
            NativeSqliteLimits::default(),
        )?;
        retained_source.revalidate()?;
        let destination_evidence = extract_restore_evidence_retained(
            destination,
            final_file,
            NativeSqliteLimits::default(),
        )?;
        let destination_identity = public_native_file_identity(native_file_identity(final_file)?);
        let source_evidence_digest = native_extraction_digest(&source_evidence)?;
        let target_evidence_digest = native_extraction_digest(&destination_evidence)?;
        if source_evidence_digest != target_evidence_digest {
            return Err(NativeSqliteError::NativeOperationFailed { operation });
        }
        publication_parent.require_named()?;
        publication_parent.require_relative_identity(&temporary_name, temporary_identity)?;
        require_native_file_fingerprint(
            &mut temporary_file,
            temporary_identity,
            expected_fingerprint,
        )?;
        require_native_file_fingerprint(
            &mut publication_file,
            publication_identity,
            expected_fingerprint,
        )?;
        if let Some(final_file) = published.as_mut() {
            let final_identity = native_file_identity(final_file)?;
            require_native_file_fingerprint(final_file, final_identity, expected_fingerprint)?;
            publication_parent
                .require_relative_identity(publication_parent.name(), final_identity)?;
        }
        finish_native_publication_source(
            &publication_parent,
            &temporary_name,
            &temporary_file,
            temporary_identity,
            source_moved,
        )?;
        publication_parent.sync()?;
        publication_parent.require_named()?;
        let engine_version = rusqlite::version().to_owned();
        let native_point = BackendNativePointV1::SqliteOnlineBackup {
            engine_identity: engine_version.clone(),
            point_id: format!(
                "sqlite-online-backup-v1:{}",
                source_evidence_digest.as_str()
            ),
        };
        Ok(NativeSqliteTransferReportV1 {
            engine_version: rusqlite::version().to_owned(),
            page_count,
            source_canonical_ready: source_report.canonical_ready,
            destination_canonical_ready: destination_report.canonical_ready,
            destination_identity,
            capture: NativeSqliteCaptureWitnessV1 {
                source_evidence_digest,
                target_evidence_digest,
                native_point: native_point.clone(),
                engine_version,
            },
        })
    })();
    result.map_err(|error| {
        cleanup_native_publication(
            &temporary,
            temporary_identity,
            &temporary_file,
            &publication_parent,
            published.as_ref(),
            source_moved,
            error,
        )
    })
}

fn native_extraction_digest(
    evidence: &NativeSqliteRestoreEvidenceV1,
) -> Result<DigestV1, NativeSqliteError> {
    serde_json::to_vec(evidence)
        .map(|bytes| DigestV1::hash(&bytes))
        .map_err(|_| NativeSqliteError::NativeOperationFailed {
            operation: "native evidence digest",
        })
}

fn require_standalone_delete_journal(connection: &Connection) -> rusqlite::Result<()> {
    let mode: String = connection.query_row("PRAGMA journal_mode=DELETE", (), |row| row.get(0))?;
    if mode != "delete" {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn bundled_online_backup(
    source: &Path,
    destination: &Path,
    retained_source: &RetainedNativeSqliteSource,
    publication_parent: &NativePublicationParent,
    temporary_name: &OsStr,
    temporary_identity: NativeFileIdentity,
    temporary_file: &File,
    operation: &'static str,
    options: NativeSqliteTransferOptions,
    interrupt_after_steps: Option<u32>,
) -> Result<u64, NativeSqliteError> {
    let canonical_source = fs::canonicalize(source).map_err(|_| NativeSqliteError::InvalidPath)?;
    let canonical_destination =
        fs::canonicalize(destination).map_err(|_| NativeSqliteError::InvalidPath)?;
    let source_authority = retained_source
        .file
        .try_clone()
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    let source_connection = open_exact(
        source_authority,
        &canonical_source,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    retained_source.revalidate()?;
    source_connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    let destination_authority = temporary_file
        .try_clone()
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    let mut destination_connection = open_exact(
        destination_authority,
        &canonical_destination,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    publication_parent.require_named()?;
    publication_parent.require_relative_identity(temporary_name, temporary_identity)?;
    destination_connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    destination_connection
        .execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=FULL;")
        .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    let backup = Backup::new(&source_connection, &mut destination_connection)
        .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    let pages_per_step = i32::try_from(options.pages_per_step)
        .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
    let mut completed_steps = 0_u32;
    for _ in 0..options.max_steps {
        let result = backup
            .step(pages_per_step)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
        match result {
            StepResult::Done => {
                let page_count = backup.progress().pagecount;
                drop(backup);
                require_standalone_delete_journal(&destination_connection)
                    .map_err(|_| NativeSqliteError::NativeOperationFailed { operation })?;
                drop(destination_connection);
                publication_parent.require_relative_identity(temporary_name, temporary_identity)?;
                temporary_file
                    .sync_all()
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
                return u64::try_from(page_count)
                    .map_err(|_| NativeSqliteError::NativeOperationFailed { operation });
            }
            StepResult::More | StepResult::Busy | StepResult::Locked => {
                completed_steps = completed_steps.saturating_add(1);
                if interrupt_after_steps.is_some_and(|limit| completed_steps >= limit) {
                    return Err(NativeSqliteError::NativeOperationFailed { operation });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                return Err(NativeSqliteError::NativeOperationFailed { operation });
            }
        }
    }
    Err(NativeSqliteError::NativeOperationFailed { operation })
}

struct RetainedNativeSqliteSource {
    path: std::path::PathBuf,
    file: File,
    identity: NativeFileIdentity,
}

impl RetainedNativeSqliteSource {
    fn open(path: &Path) -> Result<Self, NativeSqliteError> {
        validate_regular_file(path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
            let flags = i32::try_from(flags.bits()).map_err(|_| NativeSqliteError::InvalidPath)?;
            options.custom_flags(flags);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let file = options
            .open(path)
            .map_err(|_| NativeSqliteError::InvalidPath)?;
        let identity = native_file_identity(&file)?;
        let retained = Self {
            path: path.to_owned(),
            file,
            identity,
        };
        retained.revalidate()?;
        Ok(retained)
    }

    fn revalidate(&self) -> Result<(), NativeSqliteError> {
        if native_file_identity(&self.file).ok() != Some(self.identity)
            || native_path_identity(&self.path).ok() != Some(self.identity)
        {
            return Err(NativeSqliteError::InvalidPath);
        }
        Ok(())
    }
}

fn validate_regular_file(path: &Path) -> Result<(), NativeSqliteError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| NativeSqliteError::InvalidPath)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(NativeSqliteError::InvalidPath);
    }
    Ok(())
}

fn validate_transfer_options(
    options: NativeSqliteTransferOptions,
) -> Result<(), NativeSqliteError> {
    if options.pages_per_step == 0 || i32::try_from(options.pages_per_step).is_err() {
        return Err(NativeSqliteError::InvalidRow {
            what: "invalid pages per step",
        });
    }
    if options.max_steps == 0 {
        return Err(NativeSqliteError::InvalidRow {
            what: "invalid maximum steps",
        });
    }
    Ok(())
}

struct NativePublicationParent {
    path: std::path::PathBuf,
    name: OsString,
    directory: File,
    identity: NativeFileIdentity,
}

impl NativePublicationParent {
    fn open(destination: &Path) -> Result<Self, NativeSqliteError> {
        let path = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        let name = destination
            .file_name()
            .ok_or(NativeSqliteError::InvalidPath)?
            .to_owned();
        #[cfg(unix)]
        let directory = {
            use std::os::unix::fs::OpenOptionsExt as _;
            let flags = rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC;
            let flags = i32::try_from(flags.bits()).map_err(|_| NativeSqliteError::InvalidPath)?;
            OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&path)
                .map_err(|_| NativeSqliteError::InvalidPath)?
        };
        #[cfg(windows)]
        let directory = worldstream_windows_handle::open_pinned_directory(&path)
            .map_err(|_| NativeSqliteError::InvalidPath)?;
        #[cfg(not(any(unix, windows)))]
        return Err(NativeSqliteError::InvalidPath);
        if !directory
            .metadata()
            .map_err(|_| NativeSqliteError::InvalidPath)?
            .is_dir()
        {
            return Err(NativeSqliteError::InvalidPath);
        }
        let identity = native_file_identity(&directory)?;
        let parent = Self {
            path,
            name,
            directory,
            identity,
        };
        parent.require_named()?;
        parent.require_relative_absent(parent.name())?;
        Ok(parent)
    }

    fn name(&self) -> &OsStr {
        &self.name
    }

    fn directory(&self) -> &File {
        &self.directory
    }

    fn path_for(&self, name: &OsStr) -> std::path::PathBuf {
        self.path.join(name)
    }

    fn create_staging(&self, name: &OsStr) -> io::Result<File> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            let descriptor = rustix::fs::openat(
                &self.directory,
                name,
                OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(io::Error::from)?;
            return Ok(File::from(descriptor));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::{
                Foundation::{GENERIC_READ, GENERIC_WRITE},
                Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE},
            };
            return OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .create_new(true)
                .open(self.path_for(name));
        }
        #[allow(unreachable_code)]
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported native SQLite staging platform",
        ))
    }

    fn reopen_staging_for_publication(
        &self,
        name: &OsStr,
        staging: &File,
        expected: NativeFileIdentity,
    ) -> io::Result<File> {
        #[cfg(not(windows))]
        {
            let _ = (self, name, expected);
            staging.try_clone()
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::{
                Foundation::{GENERIC_READ, GENERIC_WRITE},
                Storage::FileSystem::{DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE},
            };

            let path = self.path_for(name);
            let publication = match OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) => {
                    return if scrub_native_file(staging, expected) {
                        Err(error)
                    } else {
                        Err(io::Error::other(format!(
                            "opening the native SQLite publication handle failed ({error}); exact staging cleanup was unsafe"
                        )))
                    };
                }
            };
            let validation = (|| {
                if native_file_identity(&publication)
                    .map_err(|error| io::Error::other(error.to_string()))?
                    != expected
                {
                    return Err(io::Error::other("native SQLite staging identity changed"));
                }
                self.require_relative_identity(name, expected)
                    .map_err(|error| io::Error::other(error.to_string()))
            })();
            if let Err(error) = validation {
                if !cleanup_rejected_staging_reopen(staging, expected, &publication) {
                    return Err(io::Error::other(format!(
                        "native SQLite staging validation failed ({error}); exact staging cleanup was unsafe"
                    )));
                }
                return Err(error);
            }
            Ok(publication)
        }
    }

    fn require_named(&self) -> Result<(), NativeSqliteError> {
        if native_directory_path_identity(&self.path).ok() != Some(self.identity) {
            return Err(NativeSqliteError::InvalidPath);
        }
        Ok(())
    }

    fn sync(&self) -> Result<(), NativeSqliteError> {
        self.directory
            .sync_all()
            .map_err(|error| NativeSqliteError::Io(error.to_string()))
    }

    fn open_relative_read(&self, name: &OsStr) -> io::Result<File> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            let descriptor = rustix::fs::openat(
                &self.directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            return Ok(File::from(descriptor));
        }
        #[cfg(windows)]
        {
            return File::open(self.path_for(name));
        }
        #[allow(unreachable_code)]
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported relative native publication open",
        ))
    }

    fn require_relative_identity(
        &self,
        name: &OsStr,
        expected: NativeFileIdentity,
    ) -> Result<(), NativeSqliteError> {
        let file = self
            .open_relative_read(name)
            .map_err(|_| NativeSqliteError::InvalidPath)?;
        if native_file_identity(&file)? != expected {
            return Err(NativeSqliteError::InvalidPath);
        }
        Ok(())
    }

    fn require_relative_absent(&self, name: &OsStr) -> Result<(), NativeSqliteError> {
        match self.open_relative_read(name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            _ => Err(NativeSqliteError::InvalidPath),
        }
    }
}

fn validate_new_destination(path: &Path) -> Result<NativePublicationParent, NativeSqliteError> {
    NativePublicationParent::open(path)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeFileFingerprint {
    byte_len: u64,
    digest: [u8; 32],
}

fn prepare_native_publication_source(
    parent: &NativePublicationParent,
    staging: &mut File,
    expected: NativeFileFingerprint,
) -> Result<(File, NativeFileIdentity), NativeSqliteError> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{Mode, OFlags};
        use std::io::Write as _;

        let descriptor = rustix::fs::openat(
            parent.directory(),
            ".",
            OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let mut publication = File::from(descriptor);
        staging
            .seek(SeekFrom::Start(0))
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let mut remaining = expected.byte_len;
        let mut buffer = vec![0_u8; 64 * 1024];
        while remaining != 0 {
            let limit = usize::try_from(remaining.min(buffer.len() as u64))
                .map_err(|_| NativeSqliteError::InvalidPath)?;
            let read = staging
                .read(&mut buffer[..limit])
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
            if read == 0 {
                return Err(NativeSqliteError::InvalidPath);
            }
            publication
                .write_all(&buffer[..read])
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
            remaining = remaining.saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
        }
        let mut extra = [0_u8; 1];
        if staging
            .read(&mut extra)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?
            != 0
        {
            return Err(NativeSqliteError::InvalidPath);
        }
        publication
            .sync_all()
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let identity = native_file_identity(&publication)?;
        require_native_file_fingerprint(&mut publication, identity, expected)?;
        return Ok((publication, identity));
    }
    #[cfg(not(target_os = "linux"))]
    {
        parent.require_named()?;
        let mut publication = staging
            .try_clone()
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let identity = native_file_identity(&publication)?;
        require_native_file_fingerprint(&mut publication, identity, expected)?;
        Ok((publication, identity))
    }
}

#[cfg(test)]
fn publish_without_replacement(
    source: &mut File,
    source_identity: NativeFileIdentity,
    temporary: &Path,
    destination: &Path,
    expected: NativeFileFingerprint,
    published: &mut Option<File>,
) -> Result<bool, NativeSqliteError> {
    publish_without_replacement_with_hook(
        source,
        source_identity,
        temporary,
        destination,
        expected,
        published,
        |_| Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn publish_without_replacement_with_hook<F>(
    source: &mut File,
    source_identity: NativeFileIdentity,
    temporary: &Path,
    destination: &Path,
    expected: NativeFileFingerprint,
    published: &mut Option<File>,
    before_publish: F,
) -> Result<bool, NativeSqliteError>
where
    F: FnOnce(&Path) -> Result<(), NativeSqliteError>,
{
    let publication_parent = validate_new_destination(destination)?;
    require_native_file_fingerprint(source, source_identity, expected)?;
    let temporary_name = temporary
        .file_name()
        .ok_or(NativeSqliteError::InvalidPath)?;
    publication_parent.require_relative_identity(temporary_name, source_identity)?;
    before_publish(temporary)?;
    publication_parent.require_named()?;
    publication_parent.require_relative_identity(temporary_name, source_identity)?;
    publish_native_source_without_replacement(
        &publication_parent,
        source,
        source_identity,
        expected,
        published,
    )
}

fn publish_native_source_without_replacement(
    publication_parent: &NativePublicationParent,
    source: &mut File,
    source_identity: NativeFileIdentity,
    expected: NativeFileFingerprint,
    published: &mut Option<File>,
) -> Result<bool, NativeSqliteError> {
    require_native_file_fingerprint(source, source_identity, expected)?;
    publication_parent.require_named()?;
    let (source_moved, mut final_file) =
        publish_native_handle_noreplace(source, publication_parent)?;
    let final_identity = native_file_identity(&final_file)?;
    *published = Some(
        final_file
            .try_clone()
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?,
    );
    require_native_file_fingerprint(source, source_identity, expected)?;
    require_native_file_fingerprint(&mut final_file, final_identity, expected)?;
    publication_parent.require_relative_identity(publication_parent.name(), final_identity)?;
    publication_parent.require_named()?;
    Ok(source_moved)
}

fn publish_native_handle_noreplace(
    source: &File,
    publication_parent: &NativePublicationParent,
) -> Result<(bool, File), NativeSqliteError> {
    #[cfg(target_os = "linux")]
    let result: io::Result<(bool, File)> = (|| {
        use std::os::fd::AsRawFd as _;

        let descriptor_path = format!("/proc/self/fd/{}", source.as_raw_fd());
        rustix::fs::linkat(
            rustix::fs::CWD,
            descriptor_path.as_str(),
            publication_parent.directory(),
            publication_parent.name(),
            rustix::fs::AtFlags::SYMLINK_FOLLOW,
        )
        .map_err(io::Error::from)?;
        publication_parent
            .require_named()
            .map_err(|error| io::Error::other(error.to_string()))?;
        source.try_clone().map(|file| (false, file))
    })();
    #[cfg(target_os = "macos")]
    let result: io::Result<(bool, File)> = (|| {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::fclonefileat(
            source,
            publication_parent.directory(),
            publication_parent.name(),
            rustix::fs::CloneFlags::empty(),
        )
        .map_err(io::Error::from)?;
        let descriptor = rustix::fs::openat(
            publication_parent.directory(),
            publication_parent.name(),
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let final_file = File::from(descriptor);
        if let Err(error) = publication_parent.require_named() {
            if let Ok(identity) = native_file_identity(&final_file) {
                let _ = scrub_native_file(&final_file, identity);
            }
            return Err(io::Error::other(error.to_string()));
        }
        Ok((false, final_file))
    })();
    #[cfg(windows)]
    let result: io::Result<(bool, File)> = (|| {
        worldstream_windows_handle::rename_noreplace_at(
            source,
            publication_parent.directory(),
            publication_parent.name(),
        )?;
        source.try_clone().map(|file| (true, file))
    })();
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let result: io::Result<(bool, File)> = Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported native SQLite publication platform",
    ));

    match result {
        Ok(published) => Ok(published),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(NativeSqliteError::InvalidPath)
        }
        Err(error) => Err(NativeSqliteError::Io(error.to_string())),
    }
}

fn native_file_fingerprint(file: &mut File) -> Result<NativeFileFingerprint, NativeSqliteError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    let mut byte_len = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
            .ok_or(NativeSqliteError::InvalidPath)?;
        hasher.update(&buffer[..read]);
    }
    Ok(NativeFileFingerprint {
        byte_len,
        digest: *hasher.finalize().as_bytes(),
    })
}

fn require_native_file_fingerprint(
    file: &mut File,
    identity: NativeFileIdentity,
    expected: NativeFileFingerprint,
) -> Result<(), NativeSqliteError> {
    if native_file_identity(file)? != identity || native_file_fingerprint(file)? != expected {
        return Err(NativeSqliteError::InvalidPath);
    }
    Ok(())
}

#[cfg(unix)]
fn native_file_identity(file: &File) -> Result<NativeFileIdentity, NativeSqliteError> {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata()
        .map(|metadata| (metadata.dev(), metadata.ino()))
        .map_err(|error| NativeSqliteError::Io(error.to_string()))
}

#[cfg(windows)]
fn native_file_identity(file: &File) -> Result<NativeFileIdentity, NativeSqliteError> {
    fs_id::FileID::new(file).map_err(|error| NativeSqliteError::Io(error.to_string()))
}

#[cfg(unix)]
const fn public_native_file_identity(identity: NativeFileIdentity) -> NativeSqliteFileIdentityV1 {
    NativeSqliteFileIdentityV1 {
        storage_id: identity.0,
        file_id: identity.1 as u128,
    }
}

#[cfg(windows)]
const fn public_native_file_identity(identity: NativeFileIdentity) -> NativeSqliteFileIdentityV1 {
    NativeSqliteFileIdentityV1 {
        storage_id: identity.storage_id(),
        file_id: identity.internal_file_id(),
    }
}

#[cfg(unix)]
fn native_file_has_single_link(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata().is_ok_and(|metadata| metadata.nlink() == 1)
}

#[cfg(windows)]
fn native_file_has_single_link(file: &File) -> bool {
    worldstream_windows_handle::hard_link_count(file).is_ok_and(|count| count == 1)
}

fn scrub_native_file(file: &File, expected: NativeFileIdentity) -> bool {
    if native_file_identity(file).ok() != Some(expected) || !native_file_has_single_link(file) {
        return false;
    }
    if file.set_len(0).and_then(|()| file.sync_all()).is_err() {
        return false;
    }
    native_file_identity(file).ok() == Some(expected)
        && native_file_has_single_link(file)
        && file.metadata().is_ok_and(|metadata| metadata.len() == 0)
}

/// A rejected pathname reopen is never cleanup authority. It may identify an
/// unrelated same-owner victim installed after the retained staging object was
/// admitted. Only the retained expected staging handle may be scrubbed.
#[cfg(any(test, windows))]
fn cleanup_rejected_staging_reopen(
    staging: &File,
    expected: NativeFileIdentity,
    _rejected_reopen: &File,
) -> bool {
    scrub_native_file(staging, expected)
}

#[cfg(unix)]
fn native_path_identity(path: &Path) -> io::Result<NativeFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(io::Error::other(
            "native SQLite artifact is not a regular file",
        ));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn native_path_identity(path: &Path) -> io::Result<NativeFileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(io::Error::other(
            "native SQLite artifact is not a regular file",
        ));
    }
    fs_id::FileID::new(path)
}

#[cfg(unix)]
fn native_directory_path_identity(path: &Path) -> io::Result<NativeFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(io::Error::other(
            "native SQLite publication parent is not a directory",
        ));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn native_directory_path_identity(path: &Path) -> io::Result<NativeFileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(io::Error::other(
            "native SQLite publication parent is not a directory",
        ));
    }
    fs_id::FileID::new(path)
}

fn finish_native_publication_source(
    parent: &NativePublicationParent,
    temporary_name: &OsStr,
    temporary_file: &File,
    identity: NativeFileIdentity,
    source_moved: bool,
) -> Result<(), NativeSqliteError> {
    if source_moved {
        return parent.require_relative_absent(temporary_name);
    }
    parent.require_relative_identity(temporary_name, identity)?;
    if !scrub_native_file(temporary_file, identity) {
        return Err(NativeSqliteError::CleanupFailed {
            what: "temporary retained-handle scrub",
        });
    }
    parent.require_relative_identity(temporary_name, identity)?;
    if temporary_file
        .metadata()
        .map_err(|_| NativeSqliteError::CleanupFailed {
            what: "temporary retained-handle metadata",
        })?
        .len()
        != 0
    {
        return Err(NativeSqliteError::CleanupFailed {
            what: "temporary retained-handle scrub",
        });
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn remove_published_source_with_hook<F>(
    temporary: &Path,
    identity: NativeFileIdentity,
    source_moved: bool,
    after_identity_check: F,
) -> Result<(), NativeSqliteError>
where
    F: FnOnce() -> Result<(), NativeSqliteError>,
{
    if source_moved {
        if fs::symlink_metadata(temporary).is_ok() {
            return Err(NativeSqliteError::CleanupFailed {
                what: "renamed temporary remained",
            });
        }
        return Ok(());
    }
    if native_path_identity(temporary).ok() != Some(identity) {
        return Err(NativeSqliteError::CleanupFailed {
            what: "temporary identity changed",
        });
    }
    after_identity_check()?;
    if native_path_identity(temporary).ok() != Some(identity) {
        return Err(NativeSqliteError::CleanupFailed {
            what: "temporary identity changed",
        });
    }
    Err(NativeSqliteError::CleanupFailed {
        what: "pathname cleanup is forbidden",
    })
}

fn cleanup_native_publication(
    temporary: &Path,
    temporary_identity: NativeFileIdentity,
    temporary_file: &File,
    publication_parent: &NativePublicationParent,
    published_file: Option<&File>,
    source_moved: bool,
    error: NativeSqliteError,
) -> NativeSqliteError {
    let mut complete = scrub_native_file(temporary_file, temporary_identity);
    if let Some(final_file) = published_file {
        let final_identity = native_file_identity(final_file).ok();
        complete &= final_identity.is_some_and(|identity| scrub_native_file(final_file, identity));
        complete &= final_identity.is_some_and(|identity| {
            publication_parent
                .require_relative_identity(publication_parent.name(), identity)
                .is_ok()
        }) && final_file
            .metadata()
            .is_ok_and(|metadata| metadata.len() == 0);
    }
    if source_moved {
        let temporary_name = temporary.file_name();
        complete &= temporary_name
            .is_some_and(|name| publication_parent.require_relative_absent(name).is_ok());
    } else {
        complete &= native_file_identity(temporary_file).ok() == Some(temporary_identity)
            && temporary_file
                .metadata()
                .is_ok_and(|metadata| metadata.len() == 0);
    }
    complete &= publication_parent.sync().is_ok();
    if complete {
        error
    } else {
        NativeSqliteError::CleanupFailed {
            what: "retained native publication cleanup",
        }
    }
}

fn temporary_destination(
    parent: &NativePublicationParent,
    destination: &Path,
) -> Result<(std::path::PathBuf, File), NativeSqliteError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(NativeSqliteError::InvalidPath)?;
    for _ in 0..100_u16 {
        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let mut nonce = String::with_capacity(bytes.len().saturating_mul(2));
        for byte in bytes {
            nonce.push(char::from(HEX[usize::from(byte >> 4)]));
            nonce.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        let name = format!(".{file_name}.worldstream-{nonce}.tmp");
        let candidate = parent.path_for(OsStr::new(&name));
        match parent.create_staging(OsStr::new(&name)) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(NativeSqliteError::Io(error.to_string())),
        }
    }
    Err(NativeSqliteError::Io(
        "could not allocate a unique SQLite temporary path".to_owned(),
    ))
}

#[derive(Clone, Debug)]
struct RoomRow {
    id: String,
    room_seq: u64,
    head: DigestV1,
    core_schema: String,
    pack_digest: String,
    core_hash: String,
    activity_hash: String,
    authoritative_hash: String,
    complete_head: Vec<u8>,
}

impl RoomRow {
    fn parse(row: &[String]) -> Result<Self, NativeSqliteError> {
        if row.len() != 9 {
            return Err(NativeSqliteError::InvalidRow { what: "room" });
        }
        Ok(Self {
            id: row[0].clone(),
            room_seq: parse_u64(&row[1], "room sequence")?,
            head: parse_storage_digest(&row[2], "room head")?,
            core_schema: row[3].clone(),
            pack_digest: storage_digest(&parse_storage_digest(&row[4], "pack digest")?),
            core_hash: storage_digest(&parse_storage_digest(&row[5], "core state hash")?),
            activity_hash: storage_digest(&parse_storage_digest(&row[6], "activity state hash")?),
            authoritative_hash: storage_digest(&parse_storage_digest(
                &row[7],
                "authoritative state hash",
            )?),
            complete_head: decode_hex(&row[8], "complete head")?,
        })
    }
}

#[derive(Clone, Debug)]
struct TransitionRow {
    room_id: String,
    room_seq: u64,
    hash: DigestV1,
    previous: DigestV1,
    bytes: Vec<u8>,
    core_schema: String,
    pack_digest: String,
    core_hash: String,
    activity_hash: String,
    authoritative_hash: String,
}

impl TransitionRow {
    fn parse(row: &[String]) -> Result<Self, NativeSqliteError> {
        if row.len() != 10 {
            return Err(NativeSqliteError::InvalidRow { what: "transition" });
        }
        Ok(Self {
            room_id: row[0].clone(),
            room_seq: parse_u64(&row[1], "transition sequence")?,
            hash: parse_storage_digest(&row[2], "transition hash")?,
            previous: parse_storage_digest(&row[3], "previous lineage hash")?,
            bytes: decode_hex(&row[4], "transition bytes")?,
            core_schema: row[5].clone(),
            pack_digest: storage_digest(&parse_storage_digest(&row[6], "pack digest")?),
            core_hash: storage_digest(&parse_storage_digest(&row[7], "core state hash")?),
            activity_hash: storage_digest(&parse_storage_digest(&row[8], "activity state hash")?),
            authoritative_hash: storage_digest(&parse_storage_digest(
                &row[9],
                "authoritative state hash",
            )?),
        })
    }
}

#[derive(Clone, Debug)]
struct SnapshotRow {
    room_id: String,
    room_seq: u64,
    schema: String,
    lineage: DigestV1,
    core_schema: String,
    pack_digest: String,
    core_hash: String,
    activity_hash: String,
    authoritative_hash: String,
    complete_head: Vec<u8>,
    core_bytes: Vec<u8>,
    activity_bytes: Vec<u8>,
}

impl SnapshotRow {
    fn parse(row: &[String]) -> Result<Self, NativeSqliteError> {
        if row.len() != 12 {
            return Err(NativeSqliteError::InvalidRow { what: "snapshot" });
        }
        Ok(Self {
            room_id: row[0].clone(),
            room_seq: parse_u64(&row[1], "snapshot sequence")?,
            schema: row[2].clone(),
            lineage: parse_storage_digest(&row[3], "snapshot lineage")?,
            core_schema: row[4].clone(),
            pack_digest: storage_digest(&parse_storage_digest(&row[5], "snapshot pack digest")?),
            core_hash: storage_digest(&parse_storage_digest(&row[6], "snapshot core hash")?),
            activity_hash: storage_digest(&parse_storage_digest(
                &row[7],
                "snapshot activity hash",
            )?),
            authoritative_hash: storage_digest(&parse_storage_digest(
                &row[8],
                "snapshot authoritative hash",
            )?),
            complete_head: decode_hex(&row[9], "snapshot complete head")?,
            core_bytes: decode_hex(&row[10], "snapshot core")?,
            activity_bytes: decode_hex(&row[11], "snapshot activity")?,
        })
    }
}

fn validate_limits(limits: NativeSqliteLimits) -> Result<(), NativeSqliteError> {
    if limits.max_rows == 0 || limits.max_output_bytes == 0 {
        return Err(NativeSqliteError::InvalidRow {
            what: "zero limits",
        });
    }
    Ok(())
}

fn bounded_limit(max_rows: usize) -> Result<usize, NativeSqliteError> {
    max_rows
        .checked_add(1)
        .ok_or(NativeSqliteError::InvalidRow { what: "row limit" })
}

fn scalar(
    file: &File,
    path: &Path,
    sql: &str,
    limits: NativeSqliteLimits,
) -> Result<String, NativeSqliteError> {
    let result = rows(file, path, sql, limits)?;
    if result.len() != 1 || result[0].len() != 1 {
        return Err(NativeSqliteError::InvalidRow { what: "scalar" });
    }
    Ok(result[0][0].clone())
}

fn rows(
    file: &File,
    path: &Path,
    sql: &str,
    limits: NativeSqliteLimits,
) -> Result<Vec<Vec<String>>, NativeSqliteError> {
    let connection = open_native_read_connection(file, path)?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let column_count = statement.column_count();
    let mut result = Vec::new();
    let rows = statement
        .query_map([], |row| row_to_strings(row, column_count))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    for row in rows {
        result.push(row.map_err(|_| NativeSqliteError::InvalidRow {
            what: "native SQLite value",
        })?);
        if result.len() > limits.max_rows {
            return Err(NativeSqliteError::OutputBoundExceeded);
        }
    }
    let approximate_bytes = result.iter().flatten().map(String::len).sum::<usize>();
    if approximate_bytes > limits.max_output_bytes {
        return Err(NativeSqliteError::OutputBoundExceeded);
    }
    Ok(result)
}

fn rows_if_table(
    file: &File,
    path: &Path,
    sql: &str,
    limits: NativeSqliteLimits,
    actual_tables: &BTreeSet<&str>,
    table: &str,
) -> Result<Vec<Vec<String>>, NativeSqliteError> {
    if actual_tables.contains(table) {
        rows(file, path, sql, limits)
    } else {
        Ok(Vec::new())
    }
}

fn native_rows(
    file: &File,
    path: &Path,
    sql: &str,
    limits: NativeSqliteLimits,
) -> Result<Vec<Vec<NativeSqliteValueV1>>, NativeSqliteError> {
    let connection = open_native_read_connection(file, path)?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    let column_count = statement.column_count();
    let mut result = Vec::new();
    let mut accounted_bytes = 0_usize;
    let rows = statement
        .query_map([], |row| native_row_to_values(row, column_count))
        .map_err(|_| NativeSqliteError::QueryFailed)?;
    for row in rows {
        let values = row.map_err(|_| NativeSqliteError::InvalidRow {
            what: "native SQLite value",
        })?;
        accounted_bytes =
            accounted_bytes.saturating_add(values.iter().map(native_value_bytes).sum::<usize>());
        result.push(values);
        if result.len() > limits.max_rows || accounted_bytes > limits.max_output_bytes {
            return Err(NativeSqliteError::OutputBoundExceeded);
        }
    }
    Ok(result)
}

fn open_native_read_connection(
    file: &File,
    path: &Path,
) -> Result<ExactSqliteConnection, NativeSqliteError> {
    let retained = file
        .try_clone()
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    open_exact(
        retained,
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| NativeSqliteError::QueryFailed)
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeDirectoryMutationWitness {
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

struct RetainedNativeQueryParent {
    path: PathBuf,
    directory: File,
    identity: NativeFileIdentity,
    #[cfg(unix)]
    mutation: NativeDirectoryMutationWitness,
}

impl RetainedNativeQueryParent {
    fn open(database: &Path) -> Result<Self, NativeSqliteError> {
        let path = database
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or(NativeSqliteError::InvalidPath)?
            .to_owned();
        #[cfg(unix)]
        let directory = {
            use std::os::unix::fs::OpenOptionsExt as _;

            let flags = rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC;
            let flags = i32::try_from(flags.bits()).map_err(|_| NativeSqliteError::InvalidPath)?;
            OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&path)
                .map_err(|_| NativeSqliteError::InvalidPath)?
        };
        #[cfg(windows)]
        let directory = worldstream_windows_handle::open_pinned_directory(&path)
            .map_err(|_| NativeSqliteError::InvalidPath)?;
        let identity = native_file_identity(&directory)?;
        #[cfg(unix)]
        let mutation = native_directory_mutation_witness(&directory)?;
        let retained = Self {
            path,
            directory,
            identity,
            #[cfg(unix)]
            mutation,
        };
        retained.require_named_and_unchanged()?;
        Ok(retained)
    }

    fn require_named_and_unchanged(&self) -> Result<(), NativeSqliteError> {
        if native_file_identity(&self.directory).ok() != Some(self.identity)
            || native_directory_path_identity(&self.path).ok() != Some(self.identity)
        {
            return Err(NativeSqliteError::UnsafeSidecarState);
        }
        #[cfg(unix)]
        if native_directory_mutation_witness(&self.directory).ok() != Some(self.mutation) {
            return Err(NativeSqliteError::UnsafeSidecarState);
        }
        Ok(())
    }
}

#[cfg(unix)]
fn native_directory_mutation_witness(
    directory: &File,
) -> Result<NativeDirectoryMutationWitness, NativeSqliteError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = directory
        .metadata()
        .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
    Ok(NativeDirectoryMutationWitness {
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

struct RetainedNativeQueryCoordinate {
    validation_path: PathBuf,
    file: File,
    identity: NativeFileIdentity,
    parent: RetainedNativeQueryParent,
    #[cfg(windows)]
    sidecars: RetainedNativeSidecarReservations,
    path: PathBuf,
}

impl RetainedNativeQueryCoordinate {
    fn file(&self) -> &File {
        &self.file
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn revalidate(&self) -> Result<(), NativeSqliteError> {
        if native_file_identity(&self.file)? != self.identity
            || native_path_identity(&self.validation_path).ok() != Some(self.identity)
        {
            return Err(NativeSqliteError::InvalidPath);
        }
        require_standalone_delete_header(&self.file)?;
        self.parent.require_named_and_unchanged()?;
        #[cfg(unix)]
        require_native_query_sidecars_absent(&self.path)?;
        #[cfg(windows)]
        self.sidecars.revalidate()?;
        Ok(())
    }
}

#[cfg(windows)]
struct RetainedNativeSidecarReservation {
    path: PathBuf,
    file: File,
    identity: NativeFileIdentity,
}

#[cfg(windows)]
struct RetainedNativeSidecarReservations {
    reservations: Vec<RetainedNativeSidecarReservation>,
}

#[cfg(windows)]
impl RetainedNativeSidecarReservations {
    fn reserve(database: &Path) -> Result<Self, NativeSqliteError> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::{
                DELETE, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE,
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            },
        };

        let mut reservations = Vec::with_capacity(3);
        for suffix in ["-journal", "-wal", "-shm"] {
            let path = native_query_sidecar_path(database, suffix)?;
            let file = OpenOptions::new()
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(
                    FILE_ATTRIBUTE_TEMPORARY
                        | FILE_FLAG_DELETE_ON_CLOSE
                        | FILE_FLAG_OPEN_REPARSE_POINT,
                )
                .create_new(true)
                .open(&path)
                .map_err(|_| NativeSqliteError::UnsafeSidecarState)?;
            file.set_len(0)
                .and_then(|()| file.sync_all())
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
            let identity = native_file_identity(&file)?;
            if native_path_identity(&path).ok() != Some(identity) {
                return Err(NativeSqliteError::UnsafeSidecarState);
            }
            reservations.push(RetainedNativeSidecarReservation {
                path,
                file,
                identity,
            });
        }
        Ok(Self { reservations })
    }

    fn revalidate(&self) -> Result<(), NativeSqliteError> {
        for reservation in &self.reservations {
            if native_file_identity(&reservation.file).ok() != Some(reservation.identity)
                || native_path_identity(&reservation.path).ok() != Some(reservation.identity)
                || !reservation
                    .file
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() == 0)
            {
                return Err(NativeSqliteError::UnsafeSidecarState);
            }
        }
        Ok(())
    }
}

fn retained_native_query_coordinate(
    path: &Path,
    file: &File,
    identity: NativeFileIdentity,
) -> Result<RetainedNativeQueryCoordinate, NativeSqliteError> {
    retained_native_query_coordinate_with_hooks(path, file, identity, || Ok(()), || Ok(()))
}

fn retained_native_query_coordinate_with_hooks<F, G>(
    path: &Path,
    file: &File,
    identity: NativeFileIdentity,
    after_sidecar_preflight: F,
    after_main_bind: G,
) -> Result<RetainedNativeQueryCoordinate, NativeSqliteError>
where
    F: FnOnce() -> Result<(), NativeSqliteError>,
    G: FnOnce() -> Result<(), NativeSqliteError>,
{
    if native_file_identity(file)? != identity || native_path_identity(path).ok() != Some(identity)
    {
        return Err(NativeSqliteError::InvalidPath);
    }
    require_standalone_delete_header(file)?;
    let canonical_path = fs::canonicalize(path).map_err(|_| NativeSqliteError::InvalidPath)?;
    let parent = RetainedNativeQueryParent::open(&canonical_path)?;
    require_native_query_sidecars_absent(&canonical_path)?;
    after_sidecar_preflight()?;
    parent.require_named_and_unchanged()?;
    require_native_query_sidecars_absent(&canonical_path)?;
    #[cfg(windows)]
    let sidecars = RetainedNativeSidecarReservations::reserve(&canonical_path)?;

    // This connection exists only to establish the deterministic post-MAIN_DB
    // admission boundary used by the production checks and regression hooks.
    // The exact VFS binds MAIN_DB; the retained DELETE header proves SQLite
    // cannot consult WAL/SHM, while the absent rollback journal and parent
    // mutation witness fence the remaining sidecar surface.
    let bound = open_native_read_connection(file, &canonical_path)?;
    after_main_bind()?;
    drop(bound);

    let coordinate = RetainedNativeQueryCoordinate {
        validation_path: path.to_owned(),
        file: file
            .try_clone()
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?,
        identity,
        parent,
        #[cfg(windows)]
        sidecars,
        path: canonical_path,
    };
    coordinate.revalidate()?;
    Ok(coordinate)
}

fn require_standalone_delete_header(file: &File) -> Result<(), NativeSqliteError> {
    let mut header = [0_u8; 20];
    read_native_exact_at(file, &mut header, 0)?;
    if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
        return Err(NativeSqliteError::UnsafeSidecarState);
    }
    Ok(())
}

#[cfg(unix)]
fn read_native_exact_at(
    file: &File,
    buffer: &mut [u8],
    offset: u64,
) -> Result<(), NativeSqliteError> {
    use std::os::unix::fs::FileExt as _;

    file.read_exact_at(buffer, offset)
        .map_err(|error| NativeSqliteError::Io(error.to_string()))
}

#[cfg(windows)]
fn read_native_exact_at(
    file: &File,
    buffer: &mut [u8],
    offset: u64,
) -> Result<(), NativeSqliteError> {
    use std::os::windows::fs::FileExt as _;

    let mut filled = 0_usize;
    while filled < buffer.len() {
        let position = offset
            .checked_add(u64::try_from(filled).map_err(|_| NativeSqliteError::InvalidPath)?)
            .ok_or(NativeSqliteError::InvalidPath)?;
        let read = file
            .seek_read(&mut buffer[filled..], position)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        if read == 0 {
            return Err(NativeSqliteError::UnsafeSidecarState);
        }
        filled = filled.saturating_add(read);
    }
    Ok(())
}

fn native_query_sidecar_path(database: &Path, suffix: &str) -> Result<PathBuf, NativeSqliteError> {
    let name = database.file_name().ok_or(NativeSqliteError::InvalidPath)?;
    let mut sidecar = name.to_os_string();
    sidecar.push(suffix);
    Ok(database.with_file_name(sidecar))
}

fn require_native_query_sidecars_absent(database: &Path) -> Result<(), NativeSqliteError> {
    for suffix in ["-journal", "-wal", "-shm"] {
        let sidecar = native_query_sidecar_path(database, suffix)?;
        match fs::symlink_metadata(sidecar) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(NativeSqliteError::UnsafeSidecarState),
        }
    }
    Ok(())
}

fn native_row_to_values(
    row: &Row<'_>,
    column_count: usize,
) -> rusqlite::Result<Vec<NativeSqliteValueV1>> {
    (0..column_count)
        .map(|index| match row.get_ref(index)? {
            ValueRef::Null => Ok(NativeSqliteValueV1::Null),
            ValueRef::Integer(value) => Ok(NativeSqliteValueV1::Integer(value)),
            ValueRef::Real(_) => Err(rusqlite::Error::InvalidColumnType(
                index,
                "REAL".to_owned(),
                rusqlite::types::Type::Real,
            )),
            ValueRef::Text(value) => Ok(NativeSqliteValueV1::Text(
                String::from_utf8_lossy(value).into_owned(),
            )),
            ValueRef::Blob(value) => Ok(NativeSqliteValueV1::Blob(value.to_vec())),
        })
        .collect()
}

fn native_value_bytes(value: &NativeSqliteValueV1) -> usize {
    match value {
        NativeSqliteValueV1::Null => 1,
        NativeSqliteValueV1::Integer(_) => std::mem::size_of::<i64>(),
        NativeSqliteValueV1::Text(value) => value.len(),
        NativeSqliteValueV1::Blob(value) => value.len(),
    }
}

fn native_text<'a>(
    values: &'a [NativeSqliteValueV1],
    index: usize,
    what: &'static str,
) -> Result<&'a str, NativeSqliteError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Ok(value),
        _ => Err(NativeSqliteError::InvalidRow { what }),
    }
}

fn native_deployment_lineage(
    values: &[NativeSqliteValueV1],
    index: usize,
) -> Result<String, NativeSqliteError> {
    let value = native_text(values, index, "deployment lineage")?;
    if !is_valid_deployment_lineage(value) {
        return Err(NativeSqliteError::InvalidRow {
            what: "deployment lineage",
        });
    }
    Ok(value.to_owned())
}

fn is_valid_deployment_lineage(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_DEPLOYMENT_LINEAGE_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'/'))
}

fn native_integer(
    values: &[NativeSqliteValueV1],
    index: usize,
    what: &'static str,
) -> Result<i64, NativeSqliteError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Ok(*value),
        _ => Err(NativeSqliteError::InvalidRow { what }),
    }
}

fn native_blob<'a>(
    values: &'a [NativeSqliteValueV1],
    index: usize,
    what: &'static str,
) -> Result<&'a [u8], NativeSqliteError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Ok(value),
        _ => Err(NativeSqliteError::InvalidRow { what }),
    }
}

fn row_to_strings(row: &Row<'_>, column_count: usize) -> rusqlite::Result<Vec<String>> {
    (0..column_count)
        .map(|index| row.get::<_, String>(index))
        .collect()
}

fn parse_u64(value: &str, what: &'static str) -> Result<u64, NativeSqliteError> {
    value
        .parse()
        .map_err(|_| NativeSqliteError::InvalidRow { what })
}

fn parse_digest(value: &str, what: &'static str) -> Result<DigestV1, NativeSqliteError> {
    DigestV1::parse(value.to_owned()).map_err(|_| NativeSqliteError::InvalidRow { what })
}

fn parse_storage_digest(value: &str, what: &'static str) -> Result<DigestV1, NativeSqliteError> {
    parse_digest(value.strip_prefix("blake3:").unwrap_or(value), what)
}

fn storage_digest(value: &DigestV1) -> String {
    value.as_str().to_owned()
}

/// Computes the persisted Genesis hash from typed canonical bytes.
#[must_use]
pub fn canonical_genesis_hash(bytes: &[u8]) -> Option<DigestV1> {
    GenesisV1::from_canonical_bytes(bytes)
        .ok()
        .map(|genesis| storage_digest_from_core(&genesis.genesis_hash().to_string()))
}

pub(crate) fn canonical_transition_hash(bytes: &[u8]) -> Option<DigestV1> {
    TransitionV1::from_canonical_bytes(bytes)
        .ok()
        .map(|transition| storage_digest_from_core(&transition.transition_hash().to_string()))
}

/// Computes the domain-separated Core materialization hash used by `SQLite`.
#[must_use]
pub fn canonical_core_materialization_hash(bytes: &[u8], _pack_digest: &str) -> Option<DigestV1> {
    let core = CanonicalJsonV1::from_canonical_bytes(bytes).ok()?;
    let value = serde_json::json!({
        "domain": "worldstream/core-state/v1",
        "core_schema": CORE_SCHEMA_VERSION,
        "core": core,
    });
    let canonical = CanonicalJsonV1::parse(&serde_json::to_vec(&value).ok()?).ok()?;
    Some(DigestV1::hash(&canonical.to_bytes().ok()?))
}

/// Computes the domain-separated Activity materialization hash used by `SQLite`.
#[must_use]
pub fn canonical_activity_materialization_hash(
    bytes: &[u8],
    pack_digest: &str,
) -> Option<DigestV1> {
    let activity = CanonicalJsonV1::from_canonical_bytes(bytes).ok()?;
    let value = serde_json::json!({
        "domain": "worldstream/activity-state/v1",
        "pack_digest": format!("blake3:{pack_digest}"),
        "activity": activity,
    });
    let canonical = CanonicalJsonV1::parse(&serde_json::to_vec(&value).ok()?).ok()?;
    Some(DigestV1::hash(&canonical.to_bytes().ok()?))
}

/// Computes the domain-separated Authoritative State hash used by `SQLite`.
#[must_use]
pub fn canonical_authoritative_materialization_hash(
    bytes: &[u8],
    pack_digest: &str,
    core_state_digest: &DigestV1,
    activity_state_digest: &DigestV1,
) -> Option<DigestV1> {
    let authoritative = CanonicalJsonV1::from_canonical_bytes(bytes).ok()?;
    let value = serde_json::json!({
        "domain": "worldstream/authoritative-state/v1",
        "core_schema": CORE_SCHEMA_VERSION,
        "pack_digest": format!("blake3:{pack_digest}"),
        "core_state_hash": format!("blake3:{}", core_state_digest.as_str()),
        "activity_state_hash": format!("blake3:{}", activity_state_digest.as_str()),
        "authoritative": authoritative,
    });
    let canonical = CanonicalJsonV1::parse(&serde_json::to_vec(&value).ok()?).ok()?;
    Some(DigestV1::hash(&canonical.to_bytes().ok()?))
}

fn decode_hex(value: &str, what: &'static str) -> Result<Vec<u8>, NativeSqliteError> {
    if !value.len().is_multiple_of(2) {
        return Err(NativeSqliteError::InvalidRow { what });
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| NativeSqliteError::InvalidRow { what })
        })
        .collect()
}

fn two_fields<'a>(
    row: &'a [String],
    what: &'static str,
) -> Result<(&'a str, &'a str), NativeSqliteError> {
    if row.len() != 2 {
        return Err(NativeSqliteError::InvalidRow { what });
    }
    Ok((&row[0], &row[1]))
}

fn three_fields<'a>(
    row: &'a [String],
    what: &'static str,
) -> Result<(&'a str, &'a str, &'a str), NativeSqliteError> {
    if row.len() != 3 {
        return Err(NativeSqliteError::InvalidRow { what });
    }
    Ok((&row[0], &row[1], &row[2]))
}

fn diagnostic(
    diagnostics: &mut Vec<NativeSqliteDiagnosticV1>,
    code: &'static str,
    blocking: bool,
    subject: &str,
) {
    let subject = blake3::hash(subject.as_bytes()).to_hex().to_string();
    diagnostics.push(NativeSqliteDiagnosticV1 {
        code,
        blocking,
        subject: format!("subject:{}", &subject[..12]),
        action: action_for_diagnostic(code),
    });
}

fn action_for_diagnostic(code: &'static str) -> &'static str {
    match code {
        "native_not_query_only" => "reopen the source through the read-only native verifier",
        "sqlite_integrity_check_failed" => {
            "discard the damaged file and restore from a verified native backup"
        }
        "required_table_missing" => "restore the exact WorldStream schema before retrying",
        "canonical_export_metadata_mismatch" => {
            "restore one nonzero canonical export metadata row with its deployment lineage"
        }
        "migration_contract_mismatch" => {
            "restore the exact forward-only migration prefix before retrying"
        }
        "source_transfer_lifecycle_mismatch" => {
            "restore the exact singleton SQLite source-transfer authority state"
        }
        "duplicate_room" | "duplicate_genesis" => {
            "discard the target and recapture one immutable Room root"
        }
        "duplicate_paired_snapshot" => {
            "discard the target and restore one immutable paired snapshot per Room sequence"
        }
        "genesis_missing" => "restore the missing immutable Room Genesis",
        "transition_count_mismatch" | "orphan_transition" => {
            "discard the target and restore the complete Room transition set"
        }
        "lineage_hash_mismatch" | "complete_head_mismatch" => {
            "discard the target and restore the exact Genesis-to-Head lineage"
        }
        "materialization_hash_mismatch" => {
            "discard the target and restore the exact Core and Activity materialization"
        }
        "operational_extraction_failed" => {
            "discard the partial target and repeat extraction from a verified native point"
        }
        "integrity_row_missing" | "integrity_row_mismatch" => {
            "restore the complete Room integrity witness ledger"
        }
        "member_row_mismatch" => "restore the complete Membership witness ledger",
        "timer_row_mismatch" => "restore exact durable Timer rows and payload bytes",
        "frame_row_mismatch" => {
            "restore exact Observation Frame bytes, hashes, and causal relations"
        }
        "observation_consequence_mismatch" => {
            "restore exact observation consequence rows and causal relations"
        }
        "activation_decision_mismatch" => {
            "restore exact Activation decision bytes and causal relations"
        }
        "activation_intent_mismatch" => {
            "restore exact Activation Intent lease and context witnesses"
        }
        "activation_receipt_mismatch" => {
            "restore exact Activation operation receipt bytes and relations"
        }
        "semantic_receipt_mismatch" => {
            "restore exact semantic receipt rows; request bytes require source evidence"
        }
        "paired_snapshot_disposable" | "paired_snapshots_absent_disposable" => {
            "rebuild the disposable paired snapshot cache from verified canonical history"
        }
        _ => "inspect the bounded native verifier report before retrying",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BACKUP_MANIFEST_SCHEMA_V1, BackendNativePointV1, BackendProfileV1, BackupManifestV1,
        DigestV1, IntegrityStatusV1, IntegrityWitnessV1, MigrationContractV1, MigrationIdentityV1,
        NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1, NativeSqliteAuthoritativeMaterializationV1,
        NativeSqliteBackupEnvelopeV1, NativeSqliteEnvelopeCoverageV1, NativeSqliteEnvelopeOriginV1,
        NativeSqliteNativeWitnessV1, NativeSqliteTimerRelationV1, ResourceBlobV1,
        ResourceIdentityV1, VerifierLimits, native_evidence_digest, native_membership_digest,
    };
    use rusqlite::{Connection, params};
    use worldstream_core::{
        AccessModeV1, ActionId, CoreTraceV1, MembershipStandingV1, MembershipV1,
        PackGenesisRequestV1, ParticipantActionV1, PrincipalKindV1, RecordedStimulusV1,
        builtin_counter_registry, builtin_worldstream_registry, counter_v2_digest,
    };

    fn fixture_path(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "worldstream-native-sqlite-{}-{name}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&directory);
        directory.join("database.sqlite3")
    }

    #[allow(clippy::too_many_lines)]
    fn create_fixture(name: &str) -> Result<std::path::PathBuf, NativeSqliteError> {
        let path = fixture_path(name);
        let _ = fs::remove_file(&path);
        let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let member_id = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
        let principal_id = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
        let registry = builtin_counter_registry().map_err(|_| NativeSqliteError::InvalidRow {
            what: "fixture registry",
        })?;
        let membership = MembershipV1::new(
            member_id
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture member",
                })?,
            principal_id
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture principal",
                })?,
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .map_err(|_| NativeSqliteError::InvalidRow {
            what: "fixture membership",
        })?;
        let request = PackGenesisRequestV1 {
            room_id: room_id.parse().map_err(|_| NativeSqliteError::InvalidRow {
                what: "fixture room",
            })?,
            pack_digest: counter_v2_digest(),
            configuration: CanonicalJsonV1::parse(br#"{"initial_value":0,"maximum_value":4}"#)
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture configuration",
                })?,
            room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture seed",
                })?,
            created_at: "2026-08-15T12:00:00Z".parse().map_err(|_| {
                NativeSqliteError::InvalidRow {
                    what: "fixture time",
                }
            })?,
            initial_core_state: worldstream_core::CoreRoomStateV1::active([membership]).map_err(
                |_| NativeSqliteError::InvalidRow {
                    what: "fixture core",
                },
            )?,
        };
        let prepared = registry
            .prepare_genesis_for_new_room(&request)
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "fixture genesis",
            })?;
        let mut trace =
            CoreTraceV1::create_from_retained_for_conformance(prepared).map_err(|_| {
                NativeSqliteError::InvalidRow {
                    what: "fixture trace",
                }
            })?;
        let action_definition = trace
            .retained_pack()
            .ok_or(NativeSqliteError::InvalidRow {
                what: "fixture pack",
            })?
            .descriptor()
            .actions
            .iter()
            .find(|action| action.action_type == "increment")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "fixture action",
            })?;
        let action = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: member_id
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture member",
                })?,
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FE0"
                .parse::<ActionId>()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture action id",
                })?,
            action_type: "increment".to_owned(),
            payload_schema_digest: action_definition.payload_schema.schema_digest.clone(),
            canonical_payload: CanonicalJsonV1::parse(br"{}").map_err(|_| {
                NativeSqliteError::InvalidRow {
                    what: "fixture payload",
                }
            })?,
            exact_basis_head: trace.head().clone(),
            admitted_at: "2026-08-15T12:00:01Z".parse().map_err(|_| {
                NativeSqliteError::InvalidRow {
                    what: "fixture action time",
                }
            })?,
        });
        trace
            .advance_for_conformance(action)
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "fixture transition",
            })?;
        let genesis = trace
            .genesis_bytes()
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "fixture genesis bytes",
            })?;
        let transition = trace
            .transitions()
            .first()
            .ok_or(NativeSqliteError::InvalidRow {
                what: "fixture transition",
            })?
            .canonical_bytes()
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "fixture transition bytes",
            })?;
        let head = trace
            .head()
            .canonical_bytes()
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "fixture head",
            })?;
        let core =
            trace
                .core_state()
                .canonical_bytes()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture core bytes",
                })?;
        let activity =
            trace
                .activity_state()
                .to_bytes()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "fixture activity bytes",
                })?;
        let genesis_hash = trace.genesis().genesis_hash().to_string();
        let transition_hash = trace.transitions()[0].transition_hash().to_string();
        let core_hash = trace.head().core_state_hash().to_string();
        let activity_hash = trace.head().activity_state_hash().to_string();
        let authoritative_hash = trace.head().authoritative_state_hash().to_string();
        let pack_digest = trace.head().pack_digest().to_string();
        let core_schema = CORE_SCHEMA_VERSION;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations(version INTEGER, migration_id TEXT, source_checksum TEXT);\
                 CREATE TABLE rooms(room_id TEXT, room_seq INTEGER, genesis_or_transition_hash TEXT,\
                 core_schema_version TEXT, pack_digest TEXT, core_state_hash TEXT,\
                 activity_state_hash TEXT, authoritative_state_hash TEXT, complete_head_bytes BLOB);\
                 CREATE TABLE room_genesis(room_id TEXT, genesis_bytes BLOB);\
                 CREATE TABLE transitions(room_id TEXT, room_seq INTEGER, transition_hash TEXT,\
                 previous_lineage_hash TEXT, transition_bytes BLOB, core_schema_version TEXT,\
                 pack_digest TEXT, core_state_hash TEXT, activity_state_hash TEXT,\
                 authoritative_state_hash TEXT);\
                 CREATE TABLE room_materializations(room_id TEXT, core_state_bytes BLOB, activity_state_bytes BLOB);\
                 CREATE TABLE room_integrity(room_id TEXT, status TEXT, generation INTEGER);\
                 CREATE TABLE room_members(room_id TEXT, member_id TEXT, principal_id TEXT, principal_kind TEXT, standing TEXT, access_mode TEXT, role TEXT, membership_bytes BLOB, frame_head INTEGER, membership_generation INTEGER, retained_frame_floor INTEGER, last_ack_frame_seq INTEGER, reset_required_through INTEGER, reset_generation INTEGER);\
                 CREATE TABLE timers(room_id TEXT, timer_id TEXT, generation INTEGER, scheduled_for TEXT, payload_bytes BLOB, state TEXT);\
                 CREATE TABLE observation_frames(room_id TEXT, member_id TEXT, frame_seq INTEGER, cause_room_seq INTEGER, payload_hash TEXT, payload_bytes BLOB, retained_at TEXT);\
                 CREATE TABLE observation_consequences(room_id TEXT, member_id TEXT, cause_room_seq INTEGER, consequence_kind TEXT, payload_bytes BLOB, projection_hash TEXT);\
                 CREATE TABLE activation_decisions(room_id TEXT, cause_room_seq INTEGER, decision_id TEXT, target_member_id TEXT, decision_bytes BLOB);\
                 CREATE TABLE activation_intents(activation_id TEXT, room_id TEXT, cause_room_seq INTEGER, decision_id TEXT, target_member_id TEXT, reason_code TEXT, deduplication_key TEXT, priority INTEGER, semantic_deadline TEXT, policy_revision INTEGER, state TEXT, intent_generation INTEGER, lease_generation INTEGER, runner_id TEXT, claim_id TEXT, lease_until TEXT, context_hash BLOB, context_bytes BLOB, context_retired INTEGER, created_at TEXT, attention_bytes INTEGER, terminal_disposition TEXT, superseded_by_activation_id TEXT, terminal_at TEXT);\
                 CREATE TABLE activation_operation_receipts(room_id TEXT, operation_id TEXT, operation_kind TEXT, canonical_request_hash BLOB, activation_id TEXT, result_code TEXT, result_bytes BLOB, context_hash BLOB, context_bytes BLOB);\
                 CREATE TABLE semantic_receipts(room_id TEXT, operation_kind TEXT, operation_identity_bytes BLOB, codec_id TEXT, canonical_request_hash BLOB, basis_complete_head_bytes BLOB, semantic_input_bytes BLOB, semantic_time_bytes BLOB, resolution_kind TEXT, transition_seq INTEGER, stored_resolution_bytes BLOB, committed_at TEXT);\
                 CREATE TABLE external_input_preparations(operation_identity_bytes BLOB, canonical_request_hash BLOB, recorded_at TEXT);\
                 CREATE TABLE room_snapshots(room_id TEXT, room_seq INTEGER, snapshot_schema_version TEXT,\
                 genesis_or_transition_hash TEXT, core_schema_version TEXT, pack_digest TEXT,\
                 core_state_hash TEXT, activity_state_hash TEXT, authoritative_state_hash TEXT,\
                 complete_head_bytes BLOB, core_state_bytes BLOB, activity_state_bytes BLOB);\
                 CREATE TABLE canonical_export_metadata(metadata_id INTEGER PRIMARY KEY, deployment_lineage TEXT, storage_epoch INTEGER);\
                 CREATE TABLE retired_authority_fences_v1(witness_id TEXT, authenticated_principal TEXT, generation INTEGER, scope_revocation_bytes BLOB, scope_revocation_hash BLOB, active INTEGER);\
                 CREATE TABLE principals(principal_id TEXT, principal_kind TEXT, authority_status TEXT, principal_generation INTEGER);\
                 CREATE TABLE runners(runner_id TEXT, owner_principal_id TEXT, authority_status TEXT, runner_generation INTEGER);\
                 CREATE TABLE capabilities(capability_id TEXT, token_hash BLOB, principal_id TEXT, profile_kind TEXT, target_room_id TEXT, target_member_id TEXT, runner_id TEXT, authority_generation INTEGER, expires_at TEXT, revoked_at TEXT);\
                 CREATE TABLE capability_scopes(capability_id TEXT, scope TEXT);\
                 CREATE TABLE runner_capability_memberships(capability_id TEXT, room_id TEXT, member_id TEXT);\
                 CREATE TABLE authority_change_receipts(change_id TEXT, authenticated_principal TEXT, request_hash BLOB, result_kind TEXT, target_kind TEXT, target_id TEXT, secondary_target_id TEXT, resulting_generation INTEGER, checked_at TEXT);\
                 CREATE TABLE authority_audit(audit_seq INTEGER, change_id TEXT, actor_principal_id TEXT, target_kind TEXT, target_id TEXT, secondary_target_id TEXT, change_kind TEXT, prior_generation INTEGER, resulting_generation INTEGER, checked_at TEXT, reason_code TEXT, request_hash BLOB);\
                 CREATE TABLE integrity_incidents(room_id TEXT, incident_seq INTEGER, generation INTEGER, status TEXT, reason_code TEXT, details_bytes BLOB);\
                 CREATE TABLE deployment_identity_metadata(metadata_id INTEGER, pack_set_digest BLOB, resource_set_digest BLOB, canonical_bytes BLOB);\
                 CREATE TABLE deployment_pack_identities(pack_id TEXT, revision TEXT, pack_digest BLOB);\
                 CREATE TABLE deployment_resource_identities(resource_kind TEXT, resource_identity TEXT, size_bytes INTEGER, resource_digest BLOB);\
                 CREATE TABLE deployment_resource_blobs(resource_kind TEXT, resource_identity TEXT, resource_bytes BLOB, resource_digest BLOB);\
                 CREATE TABLE source_transfer_lifecycle(lifecycle_id INTEGER, state TEXT, source_epoch INTEGER, target_epoch INTEGER, backup_path TEXT, backup_digest BLOB, bundle_hash BLOB, target_fingerprint BLOB, last_aborted_bundle_hash BLOB, last_aborted_target_fingerprint BLOB, backup_storage_id TEXT, backup_file_id TEXT);\
                 INSERT INTO source_transfer_lifecycle VALUES (1, 'source_authoritative', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);\
                 INSERT INTO schema_migrations VALUES\
                 (1, '0001-initial-storage-schema', 'blake3:dd07208c71d7165b93861883b25411b1e7c33a6be36fc2be28a638e1ab5cd763'),\
                 (2, '0002-operational-authority-v1', 'blake3:237088a0f888ef9f91a1010efd95e38a40170b0fc968229b886881937af805b0'),\
                 (3, '0003-observation-delivery-v1', 'blake3:b74d06ed529d415a658eaede5067f24e02ff5de83ac07480a5df7de1645c8bcb'),\
                 (4, '0004-activation-work-v1', 'blake3:dbe807e620fc77594e871b1ad90e89379498060fd025fbbaa318396158557d2b'),\
                 (5, '0005-paired-snapshots-v1', 'blake3:385af50337813e01ce0a97894fcb82868e130d69e671f64712f6701c0e9ddb23'),\
                 (6, '0006-canonical-export-metadata-v1', 'blake3:60de4825b3796865acff18f836dfa475640324b71d168350a8ea20c2e06206d5'),\
                 (7, '0008-sqlite-migration-checksums-v1', 'blake3:ed00960ddbbfbb6a6cb8fde52ce44631ce41c3e0b7dd2e46968552f7538eb33a'),\
                 (8, '0009-deployment-identities-v1', 'blake3:2a9eed1343ed423c12593b19e922ffeb44e009432131f018ef3a3b440213debb'),\
                 (9, '0010-transfer-recovery-completeness-v1', 'blake3:e0a4033bba6de7949af577a9e75b4d1994df61b250f27f013f3c3667afe862b1'),\
                 (10, '0011-transfer-lifecycle-and-resource-identity-v1', 'blake3:cd0fe750ca3ba68d2dad7254a60dddb887912d40db270e5562a19b3b3cced0a3'),\
                 (11, '0012-transfer-backup-file-identity-v1', 'blake3:4605547211cde35f16fecf1d156b91d9ca24c39fc24b9fe875f29dcb491965b9'),\
                 (12, '0013-external-input-preparations-v1', 'blake3:2097e928196db3f2c572818b4ac87e436512df6f2e9f0cd098521a90366651f0'),\
                 (13, '0014-observation-retention-v1', 'blake3:153136e4d0fff3ffee1a02c0349fec8a2c1c907b636ce3396276177518225ac6');\
                 INSERT INTO canonical_export_metadata VALUES (1, 'deployment/fixture', 7);",
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "fixture" })?;
        connection
            .execute(
                "INSERT INTO room_integrity VALUES (?1, 'healthy', 1)",
                params![room_id],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO rooms VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    room_id,
                    transition_hash,
                    core_schema,
                    pack_digest,
                    core_hash,
                    activity_hash,
                    authoritative_hash,
                    head,
                ],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO room_genesis VALUES (?1, ?2)",
                params![room_id, genesis],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO transitions VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    room_id,
                    transition_hash,
                    genesis_hash,
                    transition,
                    core_schema,
                    pack_digest,
                    core_hash,
                    activity_hash,
                    authoritative_hash,
                ],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO room_materializations VALUES (?1, ?2, ?3)",
                params![room_id, core, activity],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO room_snapshots VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    room_id,
                    PAIRED_SNAPSHOT_SCHEMA_V1,
                    transition_hash,
                    core_schema,
                    pack_digest,
                    core_hash,
                    activity_hash,
                    authoritative_hash,
                    head,
                    core,
                    activity,
                ],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "fixture" })?;
        drop(connection);
        Ok(path)
    }

    fn create_large_fixture(name: &str) -> Result<std::path::PathBuf, NativeSqliteError> {
        let path = create_fixture(name)?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch("CREATE TABLE copy_fault_filler(value BLOB);")
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        for _ in 0..32 {
            connection
                .execute(
                    "INSERT INTO copy_fault_filler VALUES (?1)",
                    params![vec![0_u8; 4096]],
                )
                .map_err(|_| NativeSqliteError::NativeOperationFailed {
                    operation: "fixture",
                })?;
        }
        drop(connection);
        Ok(path)
    }

    #[allow(clippy::too_many_lines)]
    fn add_isolated_fixture_room(path: &std::path::Path) -> Result<(), NativeSqliteError> {
        let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        let member_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
        let principal_id = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
        let registry = builtin_counter_registry().map_err(|_| NativeSqliteError::InvalidRow {
            what: "isolated fixture registry",
        })?;
        let membership = MembershipV1::new(
            member_id
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture member",
                })?,
            principal_id
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture principal",
                })?,
            PrincipalKindV1::Human,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some("counter".to_owned()),
        )
        .map_err(|_| NativeSqliteError::InvalidRow {
            what: "isolated fixture membership",
        })?;
        let request = PackGenesisRequestV1 {
            room_id: room_id.parse().map_err(|_| NativeSqliteError::InvalidRow {
                what: "isolated fixture Room",
            })?,
            pack_digest: counter_v2_digest(),
            configuration: CanonicalJsonV1::parse(br#"{"initial_value":0,"maximum_value":4}"#)
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture configuration",
                })?,
            room_seed: "hex:1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100"
                .parse()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture seed",
                })?,
            created_at: "2026-08-15T12:01:00Z".parse().map_err(|_| {
                NativeSqliteError::InvalidRow {
                    what: "isolated fixture time",
                }
            })?,
            initial_core_state: worldstream_core::CoreRoomStateV1::active([membership]).map_err(
                |_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture core",
                },
            )?,
        };
        let prepared = registry
            .prepare_genesis_for_new_room(&request)
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "isolated fixture genesis",
            })?;
        let trace = CoreTraceV1::create_from_retained_for_conformance(prepared).map_err(|_| {
            NativeSqliteError::InvalidRow {
                what: "isolated fixture trace",
            }
        })?;
        let genesis = trace
            .genesis_bytes()
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "isolated fixture genesis bytes",
            })?;
        let head = trace
            .head()
            .canonical_bytes()
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "isolated fixture head",
            })?;
        let core =
            trace
                .core_state()
                .canonical_bytes()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture core bytes",
                })?;
        let activity =
            trace
                .activity_state()
                .to_bytes()
                .map_err(|_| NativeSqliteError::InvalidRow {
                    what: "isolated fixture activity bytes",
                })?;
        let genesis_hash = trace.genesis().genesis_hash().to_string();
        let core_hash = trace.head().core_state_hash().to_string();
        let activity_hash = trace.head().activity_state_hash().to_string();
        let authoritative_hash = trace.head().authoritative_state_hash().to_string();
        let pack_digest = trace.head().pack_digest().to_string();
        let connection = Connection::open(path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "INSERT INTO room_integrity VALUES (?1, 'quarantined', 4)",
                params![room_id],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "isolated fixture integrity",
            })?;
        connection
            .execute(
                "INSERT INTO rooms VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    room_id,
                    genesis_hash,
                    CORE_SCHEMA_VERSION,
                    pack_digest,
                    core_hash,
                    activity_hash,
                    authoritative_hash,
                    head,
                ],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "isolated fixture Room",
            })?;
        connection
            .execute(
                "INSERT INTO room_genesis VALUES (?1, ?2)",
                params![room_id, genesis],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "isolated fixture genesis",
            })?;
        connection
            .execute(
                "INSERT INTO room_materializations VALUES (?1, ?2, ?3)",
                params![room_id, core, activity],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "isolated fixture materialization",
            })?;
        connection
            .execute(
                "INSERT INTO room_snapshots VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    room_id,
                    PAIRED_SNAPSHOT_SCHEMA_V1,
                    genesis_hash,
                    CORE_SCHEMA_VERSION,
                    pack_digest,
                    core_hash,
                    activity_hash,
                    authoritative_hash,
                    head,
                    core,
                    activity,
                ],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "isolated fixture snapshot",
            })?;
        Ok(())
    }

    fn populate_operational_fixture(path: &std::path::Path) -> Result<(), NativeSqliteError> {
        let connection = Connection::open(path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let member_id = "member-1";
        let frame_payload = b"frame-payload";
        let frame_hash = DigestV1::hash(frame_payload).as_str().to_owned();
        connection
            .execute(
                "INSERT INTO room_members VALUES (?1, ?2, 'principal-1', 'human', 'enabled', 'spectator', NULL, ?3, 1, 1, 1, NULL, NULL, 0)",
                params![room_id, member_id, b"membership".as_slice()],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO timers VALUES (?1, 'timer-1', 1, '2025-01-01T00:00:00Z', ?2, 'scheduled')",
                params![room_id, b"timer-payload".as_slice()],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO observation_frames VALUES (?1, ?2, 1, 1, ?3, ?4, '2026-08-15T12:00:00Z')",
                params![room_id, member_id, frame_hash, frame_payload.as_slice()],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO observation_consequences VALUES (?1, ?2, 1, 'visibility_lost', NULL, NULL)",
                params![room_id, member_id],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO activation_decisions VALUES (?1, 1, 'decision-1', ?2, ?3)",
                params![room_id, member_id, b"decision".as_slice()],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO activation_intents VALUES ('activation-1', ?1, 1, 'decision-1', ?2, 'reason', 'dedup', 0, NULL, 1, 'pending', 1, 0, NULL, NULL, NULL, NULL, NULL, 0, '2026-01-01T00:00:00Z', 128, NULL, NULL, NULL)",
                params![room_id, member_id],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO activation_operation_receipts VALUES (?1, 'operation-1', 'offer', ?2, NULL, 'accepted', ?3, NULL, NULL)",
                params![room_id, [0x11_u8; 32], b"result".as_slice()],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO semantic_receipts VALUES (?1, 'action', ?2, 'worldstream/operation-receipt/v1', ?3, NULL, ?4, ?5, 'no_change_recorded', NULL, ?6, '2025-01-01T00:00:00Z')",
                params![
                    room_id,
                    b"identity".as_slice(),
                    [0x22_u8; 32],
                    b"input".as_slice(),
                    b"time".as_slice(),
                    b"result".as_slice()
                ],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        Ok(())
    }

    #[allow(
        clippy::expect_used,
        clippy::panic,
        clippy::too_many_lines,
        clippy::unwrap_used
    )]
    fn fixture_envelope(
        source: &NativeSqliteRestoreEvidenceV1,
        restored: &NativeSqliteRestoreEvidenceV1,
        capture: &crate::NativeSqliteCaptureWitnessV1,
    ) -> NativeSqliteBackupEnvelopeV1 {
        let room = source
            .room_heads
            .get("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .expect("fixture Room head");
        let migration_rows = source
            .migration_metadata
            .as_ref()
            .expect("fixture migration checksums");
        let records = migration_rows
            .iter()
            .map(|row| MigrationIdentityV1 {
                version: match row.values[0] {
                    NativeSqliteValueV1::Integer(value) => u32::try_from(value).unwrap(),
                    _ => panic!("fixture migration version"),
                },
                migration_id: match &row.values[1] {
                    NativeSqliteValueV1::Text(value) => value.clone(),
                    _ => panic!("fixture migration id"),
                },
                checksum: match &row.values[2] {
                    NativeSqliteValueV1::Text(value) => {
                        DigestV1::parse(value.strip_prefix("blake3:").unwrap_or(value).to_owned())
                            .unwrap()
                    }
                    _ => panic!("fixture migration checksum"),
                },
            })
            .collect();
        let native_point = capture.native_point().clone();
        let executor_bytes = trusted_counter_v2_executor_bytes();
        let executor_digest = DigestV1::hash(&executor_bytes);
        let manifest = BackupManifestV1 {
            schema: BACKUP_MANIFEST_SCHEMA_V1.to_owned(),
            backup_id: "fixture-native-envelope".to_owned(),
            deployment_lineage: source.deployment_lineage.clone().unwrap(),
            storage_epoch: source.storage_epoch.unwrap(),
            backend: BackendProfileV1::SqliteBundled,
            native_point: native_point.clone(),
            migration_contract: MigrationContractV1 {
                logical_history_id: "worldstream-storage-v1".to_owned(),
                schema_contract_fingerprint: DigestV1::parse(
                    "ce0d34c5f7da9ffcfcd4d8ff52586cf1bd3e566edb74de1ac131cacae0dd8c22".to_owned(),
                )
                .unwrap(),
                records,
            },
            expected_packs: vec![crate::PackIdentityV1 {
                pack_id: "worldstream.counter".to_owned(),
                revision_digest: room.pack_digest.clone(),
                executor_digest: executor_digest.clone(),
                schema_bundle_digest: DigestV1::parse(
                    "0d4f81253a26d5a4ac85ae461e37426df905c555ea2071b9834a631d4b2e99ee".to_owned(),
                )
                .unwrap(),
                codec_bundle_digest: DigestV1::parse(
                    "67b814baf1511b6c29ffe9862eeeb2b130988972c9c3a2c2ab6ff1f7018a6b30".to_owned(),
                )
                .unwrap(),
                resource_ids: vec!["fixture-executor".to_owned()],
            }],
            expected_resources: vec![ResourceIdentityV1 {
                resource_id: "fixture-executor".to_owned(),
                kind: "executor".to_owned(),
                byte_len: executor_bytes.len() as u64,
                digest: executor_digest,
            }],
            expected_global_digest: DigestV1::hash(&[]),
        };
        let authoritative = authoritative_fixture_bytes(room);
        let authoritative_digest = DigestV1::hash(&authoritative);
        let source_witness = NativeSqliteNativeWitnessV1 {
            backend: BackendProfileV1::SqliteBundled,
            native_point: native_point.clone(),
            deployment_lineage: source.deployment_lineage.clone().unwrap(),
            storage_epoch: source.storage_epoch.unwrap(),
            evidence_digest: native_evidence_digest(source).unwrap(),
            membership_digest: native_membership_digest(&source.operational).unwrap(),
        };
        let restored_witness = NativeSqliteNativeWitnessV1 {
            backend: BackendProfileV1::SqliteBundled,
            native_point: native_point.clone(),
            deployment_lineage: restored.deployment_lineage.clone().unwrap(),
            storage_epoch: restored.storage_epoch.unwrap(),
            evidence_digest: native_evidence_digest(restored).unwrap(),
            membership_digest: native_membership_digest(&restored.operational).unwrap(),
        };
        let tables = OPERATIONAL_QUERIES
            .iter()
            .map(|(table, _)| (*table).to_owned())
            .collect();
        NativeSqliteBackupEnvelopeV1 {
            schema: NATIVE_SQLITE_BACKUP_ENVELOPE_SCHEMA_V1.to_owned(),
            origin: NativeSqliteEnvelopeOriginV1 {
                producer: "worldstream-native-sqlite-adapter".to_owned(),
                capture_id: match &native_point {
                    BackendNativePointV1::SqliteOnlineBackup { point_id, .. } => point_id.clone(),
                    BackendNativePointV1::PostgresNative { .. } => "invalid".to_owned(),
                },
                coverage: "online-backup plus exact companion witnesses".to_owned(),
            },
            manifest,
            resources: vec![ResourceBlobV1 {
                resource_id: "fixture-executor".to_owned(),
                bytes: executor_bytes,
            }],
            pack_bundles: Vec::new(),
            request_witnesses: Vec::new(),
            authoritative_materializations: vec![NativeSqliteAuthoritativeMaterializationV1 {
                room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
                source_bytes: authoritative.clone(),
                restored_bytes: authoritative,
                source_digest: authoritative_digest.clone(),
                restored_digest: authoritative_digest,
            }],
            timer_relations: Vec::<NativeSqliteTimerRelationV1>::new(),
            source: source_witness.clone(),
            restored: restored_witness,
            coverage: NativeSqliteEnvelopeCoverageV1 {
                tables,
                source_evidence_digest: source_witness.evidence_digest.clone(),
                restored_evidence_digest: native_evidence_digest(restored).unwrap(),
                source_membership_digest: source_witness.membership_digest.clone(),
                restored_membership_digest: native_membership_digest(&restored.operational)
                    .unwrap(),
            },
            envelope_digest: DigestV1::hash(&[]),
        }
    }

    #[allow(clippy::unwrap_used)]
    fn authoritative_fixture_bytes(head: &NativeSqliteRoomHeadV1) -> Vec<u8> {
        let value = serde_json::json!({
            "domain": "worldstream/authoritative-state/v1",
            "core_schema": head.core_schema_version,
            "pack_digest": format!("blake3:{}", head.pack_digest.as_str()),
            "core_state_hash": format!("blake3:{}", head.core_state_digest.as_str()),
            "activity_state_hash": format!("blake3:{}", head.activity_state_digest.as_str()),
        });
        let bytes = serde_json::to_vec(&value).unwrap();
        worldstream_core::CanonicalJsonV1::parse(&bytes)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    fn trusted_counter_v2_executor_bytes() -> Vec<u8> {
        let source = include_bytes!(
            "../../worldstream-core/src/retained_executor_artifacts/counter-v1-v2.rs"
        );
        let mut artifact = b"worldstream/counter-executor-source/v1\0".to_vec();
        artifact.extend_from_slice(b"2.0.0");
        artifact.push(0);
        let mut canonical = Vec::with_capacity(source.len());
        let mut index = 0;
        while index < source.len() {
            if source[index] == b'\r' {
                canonical.push(b'\n');
                if source.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            } else {
                canonical.push(source[index]);
            }
            index += 1;
        }
        artifact.extend_from_slice(&canonical);
        artifact
    }

    fn nonempty_temporary_entries(
        destination: &std::path::Path,
    ) -> Result<Vec<std::path::PathBuf>, NativeSqliteError> {
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(NativeSqliteError::InvalidPath)?;
        let prefix = format!(".{file_name}.worldstream-");
        let entries = fs::read_dir(parent)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .filter(|entry| entry.metadata().is_ok_and(|metadata| metadata.len() != 0))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        Ok(entries)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum NativeSqliteFaultInjection {
        /// Models an interleaving in which another writer creates the target
        /// after preflight but before the atomic no-replacement publication.
        DestinationExistsBeforePublish,
    }

    fn transfer_file_with_fault_injection(
        source: &std::path::Path,
        destination: &std::path::Path,
        operation: &'static str,
        options: NativeSqliteTransferOptions,
        fault: NativeSqliteFaultInjection,
    ) -> Result<NativeSqliteTransferReportV1, NativeSqliteError> {
        transfer_file_with_publish_hook(
            source,
            destination,
            operation,
            options,
            None,
            move |destination| match fault {
                NativeSqliteFaultInjection::DestinationExistsBeforePublish => {
                    fs::write(destination, b"destination-wins")
                        .map_err(|error| NativeSqliteError::Io(error.to_string()))
                }
            },
        )
    }

    #[cfg(unix)]
    #[test]
    fn public_read_entrypoints_remain_bound_to_the_initially_retained_file()
    -> Result<(), NativeSqliteError> {
        fn substitute(
            path: &Path,
            held: &Path,
            replacement: &Path,
        ) -> Result<(), NativeSqliteError> {
            fs::rename(path, held).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
            fs::rename(replacement, path).map_err(|error| NativeSqliteError::Io(error.to_string()))
        }

        let verify = create_fixture("public-verify-retention")?;
        let verify_replacement = create_fixture("public-verify-replacement")?;
        let verify_held = verify.with_extension("held");
        let verify_replacement_bytes = fs::read(&verify_replacement)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(
            verify_file_with_retained_hook(&verify, NativeSqliteLimits::default(), |path| {
                substitute(path, &verify_held, &verify_replacement)
            },),
            Err(NativeSqliteError::InvalidPath)
        );
        assert_eq!(
            fs::read(&verify).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            verify_replacement_bytes
        );

        let evidence = create_fixture("public-evidence-retention")?;
        let evidence_replacement = create_fixture("public-evidence-replacement")?;
        let evidence_held = evidence.with_extension("held");
        let evidence_replacement_bytes = fs::read(&evidence_replacement)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(
            extract_restore_evidence_with_retained_hook(
                &evidence,
                NativeSqliteLimits::default(),
                |path| substitute(path, &evidence_held, &evidence_replacement),
            ),
            Err(NativeSqliteError::InvalidPath)
        );
        assert_eq!(
            fs::read(&evidence).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            evidence_replacement_bytes
        );

        let operational = create_fixture("public-operational-retention")?;
        let operational_replacement = create_fixture("public-operational-replacement")?;
        let operational_held = operational.with_extension("held");
        let operational_replacement_bytes = fs::read(&operational_replacement)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(
            extract_operational_rows_with_retained_hook(
                &operational,
                NativeSqliteLimits::default(),
                |path| substitute(path, &operational_held, &operational_replacement),
            ),
            Err(NativeSqliteError::InvalidPath)
        );
        assert_eq!(
            fs::read(&operational).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            operational_replacement_bytes
        );

        for path in [
            verify,
            verify_held,
            evidence,
            evidence_held,
            operational,
            operational_held,
        ] {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }

    #[test]
    fn native_fixture_verifies_read_only_lineage_and_snapshot() -> Result<(), NativeSqliteError> {
        let path = create_fixture("valid")?;
        let before = fs::read(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let report = verify_file(&path, NativeSqliteLimits::default())?;
        let after = fs::read(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(before, after);
        assert!(report.canonical_ready);
        assert!(report.query_only);
        assert_eq!(report.room_count, 1);
        assert_eq!(report.transition_count, 1);
        assert_eq!(report.valid_snapshot_count, 1);
        assert_eq!(report.disposable_snapshot_count, 0);
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn streaming_retained_verifier_matches_the_lifecycle_digest_without_a_bundle_image()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("streaming-verifier")?;
        let file = File::open(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let streaming = verify_retained_file_streaming_v2(
            &path,
            &file,
            NativeSqliteStreamingLimitsV2::default(),
        )?;
        let legacy_digest =
            durable_transfer_point_digest_retained(&path, &file, NativeSqliteLimits::default())?;
        assert_eq!(streaming.transfer_point_digest(), legacy_digest);
        assert_eq!(
            streaming.operational_relation_counts().len(),
            OPERATIONAL_QUERIES.len()
        );
        assert!(
            streaming
                .operational_relation_counts()
                .contains_key("external_input_preparations")
        );
        assert_ne!(streaming.operational_row_digest(), [0_u8; 32]);
        assert_eq!(
            verify_retained_file_streaming_v2(
                &path,
                &file,
                NativeSqliteStreamingLimitsV2 {
                    max_row_bytes: 1,
                    max_tables: 1024,
                },
            ),
            Err(NativeSqliteError::OutputBoundExceeded)
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn streaming_retained_verifier_requires_the_reviewed_stream_migration_ledger()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("streaming-migration-ledger")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch(
                "INSERT INTO schema_migrations VALUES \
                 (14, '0015-snapshot-cadence-v1', \
                  'blake3:db914b00013cc9d7a341eabe081411f6583893f036547ed9db2c35be3866e9d6');",
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "insert snapshot cadence migration",
            })?;
        drop(connection);
        let file = File::open(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(
            verify_retained_file_streaming_v2(
                &path,
                &file,
                NativeSqliteStreamingLimitsV2::default(),
            ),
            Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            })
        );
        drop(file);

        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch(
                "INSERT INTO schema_migrations VALUES \
                 (15, '0016-activation-backlog-policy-v1', \
                  'blake3:495d58fdc81fee0b6b87d8973f4445b4892da22608e459033eaa333c0d4078a4'), \
                 (16, '0017-stream-transfer-v2', \
                  'blake3:aaa152c1107748f774197bd8a59600150e9d209c7e4eb14ec5e911394b23c3e2');",
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "insert stream migration",
            })?;
        drop(connection);
        let file = File::open(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert!(
            verify_retained_file_streaming_v2(
                &path,
                &file,
                NativeSqliteStreamingLimitsV2::default(),
            )
            .is_ok()
        );
        drop(file);

        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE schema_migrations SET source_checksum = 'tampered' WHERE version = 16",
                (),
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "tamper stream migration",
            })?;
        drop(connection);
        let file = File::open(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(
            verify_retained_file_streaming_v2(
                &path,
                &file,
                NativeSqliteStreamingLimitsV2::default(),
            ),
            Err(NativeSqliteError::InvalidRow {
                what: "streaming migration contract",
            })
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn streaming_retained_verifier_accepts_the_current_reviewed_migration_tail() {
        assert_eq!(REQUIRED_MIGRATIONS.len(), 22);
        assert_eq!(REQUIRED_MIGRATION_CHECKSUMS.len(), 22);
        assert_eq!(REQUIRED_MIGRATIONS[17], "0019-operational-history-roots-v2");
        assert_eq!(REQUIRED_MIGRATIONS[18], "0020-current-timers-v2");
        assert_eq!(
            REQUIRED_MIGRATIONS[19],
            "0021-checkpoint-operational-witness-v2"
        );
        assert_eq!(REQUIRED_MIGRATIONS[20], "0022-operational-history-mmr-v1");
        assert_eq!(
            REQUIRED_MIGRATIONS[21],
            "0023-checkpoint-operational-witness-v3"
        );
        assert!(SUPPORTED_MIGRATION_COUNTS.contains(&22));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn sealed_native_envelope_constructs_and_verifies_real_online_restore()
    -> Result<(), NativeSqliteError> {
        let source_path = create_fixture("sealed-envelope-source")?;
        let backup_path = fixture_path("sealed-envelope-backup");
        let target_path = fixture_path("sealed-envelope-target");
        let _ = fs::remove_file(&backup_path);
        let _ = fs::remove_file(&target_path);
        let before =
            fs::read(&source_path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        backup_file(&source_path, &backup_path)?;
        let transfer = restore_file(&backup_path, &target_path)?;
        let source = extract_restore_evidence(&source_path, NativeSqliteLimits::default())?;
        let restored = extract_restore_evidence(&target_path, NativeSqliteLimits::default())?;
        let registry =
            builtin_worldstream_registry().map_err(|_| NativeSqliteError::InvalidRow {
                what: "production pack registry",
            })?;
        let records =
            source
                .canonical_records
                .values()
                .next()
                .ok_or(NativeSqliteError::InvalidRow {
                    what: "fixture lineage",
                })?;
        let genesis = records.first().ok_or(NativeSqliteError::InvalidRow {
            what: "fixture genesis",
        })?;
        let transitions = records
            .iter()
            .skip(1)
            .map(|record| record.bytes.clone())
            .collect::<Vec<_>>();
        CoreTraceV1::replay(&registry, &genesis.bytes, &transitions).map_err(|_| {
            NativeSqliteError::InvalidRow {
                what: "fixture pure replay",
            }
        })?;
        let mut envelope = fixture_envelope(&source, &restored, transfer.capture_witness());
        envelope
            .seal(
                &source,
                &restored,
                transfer.capture_witness(),
                VerifierLimits::default(),
            )
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "envelope seal",
            })?;
        let native = envelope
            .build_native_restore_evidence(
                &source,
                &restored,
                transfer.capture_witness(),
                VerifierLimits::default(),
            )
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "envelope build",
            })?;
        let report = crate::verify_native_restore(&native, VerifierLimits::default());
        assert!(report.is_ready());
        assert_eq!(report.rooms.len(), 1);
        assert!(report.diagnostics.is_empty());
        assert_eq!(
            before,
            fs::read(&source_path).map_err(|error| NativeSqliteError::Io(error.to_string()))?
        );

        let mut tampered = envelope;
        tampered.resources[0].bytes[0] ^= 1;
        assert!(matches!(
            tampered.build_native_restore_evidence(
                &source,
                &restored,
                transfer.capture_witness(),
                VerifierLimits::default(),
            ),
            Err(crate::NativeSqliteEnvelopeError::EnvelopeDigestMismatch
                | crate::NativeSqliteEnvelopeError::CompanionDigestMismatch)
        ));
        let _ = fs::remove_file(source_path);
        let _ = fs::remove_file(backup_path);
        let _ = fs::remove_file(target_path);
        Ok(())
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn native_envelope_rejects_missing_tampered_stale_and_target_witnesses()
    -> Result<(), NativeSqliteError> {
        let source_path = create_fixture("sealed-envelope-negative-source")?;
        let backup_path = fixture_path("sealed-envelope-negative-backup");
        let target_path = fixture_path("sealed-envelope-negative-target");
        let _ = fs::remove_file(&backup_path);
        let _ = fs::remove_file(&target_path);
        backup_file(&source_path, &backup_path)?;
        let transfer = restore_file(&backup_path, &target_path)?;
        let source = extract_restore_evidence(&source_path, NativeSqliteLimits::default())?;
        let restored = extract_restore_evidence(&target_path, NativeSqliteLimits::default())?;
        let capture = transfer.capture_witness();
        let mut envelope = fixture_envelope(&source, &restored, capture);
        envelope
            .seal(&source, &restored, capture, VerifierLimits::default())
            .map_err(|_| NativeSqliteError::InvalidRow {
                what: "negative envelope seal",
            })?;

        let mut missing_resource = envelope.clone();
        missing_resource.resources.clear();
        assert!(matches!(
            missing_resource.seal(&source, &restored, capture, VerifierLimits::default()),
            Err(crate::NativeSqliteEnvelopeError::IncompleteCompanion)
        ));

        let mut missing_authoritative = envelope.clone();
        missing_authoritative.authoritative_materializations.clear();
        assert!(matches!(
            missing_authoritative.seal(&source, &restored, capture, VerifierLimits::default()),
            Err(crate::NativeSqliteEnvelopeError::IncompleteRoom
                | crate::NativeSqliteEnvelopeError::IncompleteCompanion)
        ));

        let mut tampered = envelope.clone();
        tampered.resources[0].bytes[0] ^= 1;
        assert!(matches!(
            tampered.build_native_restore_evidence(
                &source,
                &restored,
                capture,
                VerifierLimits::default()
            ),
            Err(crate::NativeSqliteEnvelopeError::EnvelopeDigestMismatch
                | crate::NativeSqliteEnvelopeError::CompanionDigestMismatch)
        ));

        let mut stale_origin = envelope.clone();
        stale_origin.origin.producer = "caller-asserted".to_owned();
        assert!(matches!(
            stale_origin.seal(&source, &restored, capture, VerifierLimits::default()),
            Err(crate::NativeSqliteEnvelopeError::CaptureWitnessMismatch)
        ));

        let mut stale_global = envelope.clone();
        stale_global.manifest.expected_global_digest = DigestV1::hash(&[]);
        assert!(matches!(
            stale_global.build_native_restore_evidence(
                &source,
                &restored,
                capture,
                VerifierLimits::default()
            ),
            Err(crate::NativeSqliteEnvelopeError::EnvelopeDigestMismatch
                | crate::NativeSqliteEnvelopeError::GlobalDigestMismatch)
        ));

        let mut changed_target = restored.clone();
        changed_target.storage_epoch = Some(
            restored
                .storage_epoch
                .expect("fixture storage epoch")
                .saturating_add(1),
        );
        assert!(matches!(
            envelope.build_native_restore_evidence(
                &source,
                &changed_target,
                capture,
                VerifierLimits::default()
            ),
            Err(crate::NativeSqliteEnvelopeError::CaptureWitnessMismatch
                | crate::NativeSqliteEnvelopeError::NativeEvidenceMismatch)
        ));

        let mut changed_source = source.clone();
        changed_source
            .canonical_records
            .values_mut()
            .next()
            .expect("fixture canonical room")
            .first_mut()
            .expect("fixture genesis")
            .bytes[0] ^= 1;
        assert!(matches!(
            envelope.build_native_restore_evidence(
                &changed_source,
                &restored,
                capture,
                VerifierLimits::default()
            ),
            Err(crate::NativeSqliteEnvelopeError::CaptureWitnessMismatch)
        ));

        let _ = fs::remove_file(source_path);
        let _ = fs::remove_file(backup_path);
        let _ = fs::remove_file(target_path);
        Ok(())
    }

    #[test]
    fn migration_checksum_witness_blocks_tamper_missing_duplicate_and_order_drift()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("migration-checksum-boundaries")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE schema_migrations SET source_checksum = 'tampered' WHERE version = 3",
                (),
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "tamper",
            })?;
        drop(connection);
        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "migration_contract_mismatch")
        );
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert!(evidence.migration_metadata.is_none());

        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "INSERT INTO schema_migrations VALUES (7, '0008-sqlite-migration-checksums-v1', ?1)",
                [REQUIRED_MIGRATION_CHECKSUMS[6]],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "duplicate" })?;
        drop(connection);
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert!(evidence.migration_metadata.is_none());

        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute("DELETE FROM schema_migrations", ())
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "clear" })?;
        connection
            .execute(
                "INSERT INTO schema_migrations VALUES (1, '0001-initial-storage-schema', ?1)",
                [REQUIRED_MIGRATION_CHECKSUMS[0]],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "order" })?;
        drop(connection);
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert!(evidence.migration_metadata.is_none());
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn sqlite_checksum_manifest_edge_does_not_relabel_postgres_deployment_metadata() {
        let authored = include_str!("../../../compatibility.toml");
        assert!(authored.contains(
            "id = \"0007-deployment-metadata-v1\"\nsqlite_checksum = \"\"\npostgresql_checksum = \"blake3:ebc57f9b917e3655f1753c5543d517f8642d5ff9de43ef0cc7b3795d9c43dc03\""
        ));
        assert!(authored.contains(
            "id = \"0008-sqlite-migration-checksums-v1\"\nsqlite_checksum = \"blake3:ed00960ddbbfbb6a6cb8fde52ce44631ce41c3e0b7dd2e46968552f7538eb33a\"\npostgresql_checksum = \"\""
        ));
        let mirror =
            serde_json::from_str::<serde_json::Value>(include_str!("../../../compatibility.json"));
        assert!(mirror.is_ok());
        let mirror = mirror.unwrap_or_default();
        let entries_value = &mirror["migrations"]["entries"];
        assert!(entries_value.is_array(), "compatibility migration entries");
        let Some(entries) = entries_value.as_array() else {
            return;
        };
        let deployment = entries
            .iter()
            .find(|entry| entry["id"] == "0007-deployment-metadata-v1");
        assert!(deployment.is_some(), "PostgreSQL deployment metadata edge");
        let Some(deployment) = deployment else { return };
        assert_eq!(deployment["sqlite_checksum"], "");
        assert_eq!(
            deployment["postgresql_checksum"],
            "blake3:ebc57f9b917e3655f1753c5543d517f8642d5ff9de43ef0cc7b3795d9c43dc03"
        );
        let sqlite = entries
            .iter()
            .find(|entry| entry["id"] == "0008-sqlite-migration-checksums-v1");
        assert!(sqlite.is_some(), "SQLite checksum edge");
        let Some(sqlite) = sqlite else { return };
        assert_eq!(
            sqlite["sqlite_checksum"],
            "blake3:ed00960ddbbfbb6a6cb8fde52ce44631ce41c3e0b7dd2e46968552f7538eb33a"
        );
        assert_eq!(sqlite["postgresql_checksum"], "");
    }

    #[test]
    fn restore_evidence_extracts_complete_modeled_fixture_without_invention()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-complete")?;
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert_eq!(
            evidence.deployment_lineage.as_deref(),
            Some("deployment/fixture")
        );
        assert_eq!(evidence.storage_epoch, Some(7));
        let records = evidence
            .canonical_records
            .get("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "canonical Room evidence",
            })?;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].room_seq, 0);
        assert!(GenesisV1::from_canonical_bytes(&records[0].bytes).is_ok());
        assert_eq!(records[1].room_seq, 1);
        assert!(TransitionV1::from_canonical_bytes(&records[1].bytes).is_ok());
        assert_eq!(evidence.newest_valid_snapshots.len(), 1);
        assert_eq!(evidence.storage_epoch, Some(7));
        assert_eq!(evidence.migration_metadata.as_ref().map(Vec::len), Some(13));
        assert_eq!(evidence.pack_metadata, None);
        assert_eq!(evidence.resource_metadata, None);
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_projection_preserves_complete_healthy_room_bytes() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-projection-healthy")?;
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        let projection =
            project_restore_input(&evidence).map_err(|_| NativeSqliteError::InvalidRow {
                what: "healthy restore projection",
            })?;
        assert_eq!(projection.rooms.len(), 1);
        let room = &projection.rooms[0];
        assert_eq!(room.disposition, NativeSqliteRoomDispositionV1::Healthy);
        assert!(GenesisV1::from_canonical_bytes(&room.canonical_records[0].bytes).is_ok());
        assert!(TransitionV1::from_canonical_bytes(&room.canonical_records[1].bytes).is_ok());
        assert_eq!(
            room.materialization
                .as_ref()
                .map(|value| value.0.as_slice()),
            evidence
                .materializations
                .get("01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .map(|value| value.0.as_slice())
        );
        assert_eq!(
            room.newest_valid_snapshot
                .as_ref()
                .map(|value| value.activity_state_bytes.as_slice()),
            evidence
                .materializations
                .get("01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .map(|value| value.1.as_slice())
        );
        assert_eq!(
            projection.operational.tables.len(),
            OPERATIONAL_QUERIES.len()
        );
        assert!(matches!(
            projection.require_complete_metadata(),
            Err(NativeSqliteRestoreProjectionError::MissingMetadata(_))
        ));
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_projection_rejects_missing_integrity_and_corrupt_healthy_bytes()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-projection-invalid")?;
        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        evidence.integrity.clear();
        assert_eq!(
            project_restore_input(&evidence),
            Err(NativeSqliteRestoreProjectionError::MissingIntegrity)
        );

        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        let Some(records) = evidence
            .canonical_records
            .get_mut("01ARZ3NDEKTSV4RRFFQ69G5FAV")
        else {
            return Err(NativeSqliteError::InvalidRow {
                what: "fixture Room",
            });
        };
        let Some(record) = records.get_mut(1) else {
            return Err(NativeSqliteError::InvalidRow {
                what: "fixture transition",
            });
        };
        record.bytes = b"tampered".to_vec();
        assert_eq!(
            project_restore_input(&evidence),
            Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes)
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_projection_rechecks_materialization_snapshot_and_record_owner()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-projection-cross-checks")?;

        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        evidence
            .materializations
            .get_mut("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "fixture materialization",
            })?
            .0[0] ^= 0xff;
        assert_eq!(
            project_restore_input(&evidence),
            Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes)
        );

        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        evidence
            .newest_valid_snapshots
            .get_mut("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "fixture snapshot",
            })?
            .core_state_bytes[0] ^= 0xff;
        assert_eq!(
            project_restore_input(&evidence),
            Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes)
        );

        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        evidence
            .canonical_records
            .get_mut("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "fixture records",
            })?[0]
            .room_id = "different-room".to_owned();
        assert_eq!(
            project_restore_input(&evidence),
            Err(NativeSqliteRestoreProjectionError::CorruptCanonicalBytes)
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_projection_quarantines_unhealthy_room_without_dropping_evidence()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-projection-quarantine")?;
        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        let Some(integrity) = evidence.integrity.get_mut("01ARZ3NDEKTSV4RRFFQ69G5FAV") else {
            return Err(NativeSqliteError::InvalidRow {
                what: "fixture Room",
            });
        };
        integrity.0 = "quarantined".to_owned();
        evidence.canonical_records.clear();
        evidence.materializations.clear();
        evidence.newest_valid_snapshots.clear();
        let projection =
            project_restore_input(&evidence).map_err(|_| NativeSqliteError::InvalidRow {
                what: "quarantined restore projection",
            })?;
        assert_eq!(projection.rooms.len(), 1);
        assert_eq!(
            projection.rooms[0].disposition,
            NativeSqliteRoomDispositionV1::Quarantined
        );
        assert!(projection.rooms[0].canonical_records.is_empty());
        assert_eq!(projection.rooms[0].integrity.0, "quarantined");
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_evidence_preserves_exact_blob_bytes() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-blobs")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "INSERT INTO room_members VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAV', 'blob-member', 'principal-blob', 'human', 'enabled', 'spectator', NULL, ?1, 0, 1, 1, NULL, NULL, 0)",
                params![vec![0_u8, 255, 1, 128]],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "fixture" })?;
        drop(connection);
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        let member = evidence.operational.tables["room_members"]
            .iter()
            .find(|row| {
                row.values.get(1) == Some(&NativeSqliteValueV1::Text("blob-member".to_owned()))
            })
            .ok_or(NativeSqliteError::InvalidRow {
                what: "BLOB member",
            })?;
        assert_eq!(
            member.values[7],
            NativeSqliteValueV1::Blob(vec![0, 255, 1, 128])
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_evidence_falls_back_from_corrupt_newest_snapshot() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-snapshot-fallback")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "INSERT INTO room_snapshots SELECT * FROM room_snapshots WHERE rowid = (SELECT min(rowid) FROM room_snapshots)",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "fixture" })?;
        connection
            .execute(
                "UPDATE room_snapshots SET core_state_bytes = x'00' WHERE rowid = (SELECT max(rowid) FROM room_snapshots)",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "fixture" })?;
        drop(connection);
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        let snapshot = evidence
            .newest_valid_snapshots
            .get("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "snapshot fallback",
            })?;
        assert_eq!(snapshot.room_seq, 1);
        assert_eq!(
            snapshot.core_state_bytes,
            evidence
                .materializations
                .get("01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .map(|value| value.0.clone())
                .ok_or(NativeSqliteError::InvalidRow {
                    what: "fixture materialization",
                })?
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_evidence_reports_absent_source_metadata() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-absent-metadata")?;
        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert_eq!(evidence.storage_epoch, Some(7));
        assert_eq!(evidence.migration_metadata.as_ref().map(Vec::len), Some(13));
        assert!(evidence.pack_metadata.is_none());
        assert!(evidence.resource_metadata.is_none());
        let readiness = assess_restore_readiness(&evidence);
        assert!(!readiness.metadata_complete);
        assert_eq!(readiness.missing.len(), 5);
        assert_eq!(readiness.diagnostics.len(), 5);
        assert!(
            readiness
                .diagnostics
                .iter()
                .all(|item| item.blocking && item.subject.starts_with("subject:"))
        );
        assert!(matches!(
            bridge_restore_evidence(&evidence),
            NativeSqliteRestoreBridgeResultV1::Incomplete { .. }
        ));
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_evidence_reads_explicit_storage_epoch_without_deriving_it()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-storage-epoch")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch(
                "UPDATE canonical_export_metadata SET deployment_lineage = 'deployment/test', storage_epoch = 7 WHERE metadata_id = 1;",
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "metadata fixture",
            })?;
        drop(connection);

        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert_eq!(
            evidence.deployment_lineage.as_deref(),
            Some("deployment/test")
        );
        assert_eq!(evidence.storage_epoch, Some(7));
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn invalid_deployment_lineage_is_not_a_metadata_witness() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-invalid-lineage")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE canonical_export_metadata SET deployment_lineage = 'deployment with spaces' WHERE metadata_id = 1",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "metadata fixture",
            })?;
        drop(connection);

        assert_eq!(
            extract_restore_evidence(&path, NativeSqliteLimits::default()),
            Err(NativeSqliteError::InvalidRow {
                what: "deployment lineage",
            })
        );
        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == "canonical_export_metadata_mismatch" })
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn deployment_identity_boundaries_and_tampering_fail_closed() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-identity-boundaries")?;
        let max_safe_integer =
            i64::try_from(MAX_SAFE_INTEGER).map_err(|_| NativeSqliteError::InvalidRow {
                what: "safe integer test bound",
            })?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;

        let lineage_128 = "a".repeat(MAX_DEPLOYMENT_LINEAGE_BYTES);
        connection
            .execute(
                "UPDATE canonical_export_metadata SET deployment_lineage = ?1, storage_epoch = ?2 WHERE metadata_id = 1",
                params![lineage_128, max_safe_integer],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "metadata boundary fixture",
            })?;
        drop(connection);

        let evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        assert_eq!(
            evidence.deployment_lineage.as_deref().map(str::len),
            Some(MAX_DEPLOYMENT_LINEAGE_BYTES)
        );
        assert_eq!(evidence.storage_epoch, Some(MAX_SAFE_INTEGER));
        assert!(verify_file(&path, NativeSqliteLimits::default())?.canonical_ready);

        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE canonical_export_metadata SET deployment_lineage = ?1 WHERE metadata_id = 1",
                params!["a".repeat(MAX_DEPLOYMENT_LINEAGE_BYTES + 1)],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "lineage tamper fixture",
            })?;
        drop(connection);
        assert_eq!(
            extract_restore_evidence(&path, NativeSqliteLimits::default()),
            Err(NativeSqliteError::InvalidRow {
                what: "deployment lineage",
            })
        );
        assert!(!verify_file(&path, NativeSqliteLimits::default())?.canonical_ready);

        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE canonical_export_metadata SET deployment_lineage = ?1, storage_epoch = ?2 WHERE metadata_id = 1",
                params!["deployment/test", max_safe_integer + 1],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "epoch tamper fixture",
            })?;
        drop(connection);
        assert_eq!(
            extract_restore_evidence(&path, NativeSqliteLimits::default()),
            Err(NativeSqliteError::InvalidRow {
                what: "storage epoch",
            })
        );
        assert!(!verify_file(&path, NativeSqliteLimits::default())?.canonical_ready);

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn zero_storage_epoch_is_not_a_metadata_witness() -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-evidence-zero-storage-epoch")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE canonical_export_metadata SET storage_epoch = 0 WHERE metadata_id = 1;",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "metadata fixture",
            })?;
        drop(connection);

        assert_eq!(
            extract_restore_evidence(&path, NativeSqliteLimits::default()),
            Err(NativeSqliteError::InvalidRow {
                what: "storage epoch",
            })
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn canonical_export_metadata_is_required_by_native_readiness() -> Result<(), NativeSqliteError>
    {
        let path = create_fixture("restore-evidence-missing-export-metadata")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch("DROP TABLE canonical_export_metadata;")
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "metadata fixture",
            })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "required_table_missing"
                && diagnostic.subject.starts_with("subject:")
        }));
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn source_transfer_lifecycle_is_required_and_shape_checked() -> Result<(), NativeSqliteError> {
        let path = create_fixture("source-transfer-lifecycle-shape")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE source_transfer_lifecycle SET state = 'transfer_pending', \
                 source_epoch = 7, target_epoch = 8, backup_path = NULL, \
                 backup_digest = zeroblob(32)",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "lifecycle fixture",
            })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "source_transfer_lifecycle_mismatch"
                && diagnostic.blocking
                && diagnostic.subject.starts_with("subject:")
        }));
        let _ = fs::remove_file(path);

        let retired = create_fixture("source-transfer-retired-witnesses")?;
        let connection = Connection::open(&retired)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE source_transfer_lifecycle SET state = 'source_retired', \
                 source_epoch = 7, target_epoch = 8, backup_path = '/verified/source.sqlite3', \
                 backup_digest = zeroblob(32), bundle_hash = NULL, target_fingerprint = NULL",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "retired lifecycle fixture",
            })?;
        drop(connection);
        let report = verify_file(&retired, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "source_transfer_lifecycle_mismatch" && diagnostic.blocking
        }));
        let _ = fs::remove_file(retired);
        Ok(())
    }

    #[test]
    fn restore_readiness_distinguishes_complete_metadata_without_fabricating_an_image()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-readiness-complete")?;
        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        evidence.backend = Some(BackendProfileV1::SqliteBundled);
        evidence.native_point = Some(BackendNativePointV1::SqliteOnlineBackup {
            engine_identity: "sqlite-3.53.4-bundled".to_owned(),
            point_id: "point-redacted".to_owned(),
        });
        evidence.storage_epoch = Some(7);
        evidence.migration_metadata = Some(vec![]);
        evidence.pack_metadata = Some(vec![]);
        evidence.resource_metadata = Some(vec![]);
        evidence.room_membership = Some(vec![]);

        let readiness = assess_restore_readiness(&evidence);
        assert!(readiness.metadata_complete);
        assert!(readiness.missing.is_empty());
        assert!(readiness.diagnostics.is_empty());
        assert!(matches!(
            bridge_restore_evidence(&evidence),
            NativeSqliteRestoreBridgeResultV1::MetadataComplete { .. }
        ));
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn restore_readiness_preserves_healthy_and_isolated_room_separation()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-readiness-rooms")?;
        let mut evidence = extract_restore_evidence(&path, NativeSqliteLimits::default())?;
        evidence
            .integrity
            .insert("healthy-room".to_owned(), ("healthy".to_owned(), 2));
        evidence
            .integrity
            .insert("isolated-room".to_owned(), ("quarantined".to_owned(), 4));
        let readiness = assess_restore_readiness(&evidence);
        assert_eq!(readiness.healthy_room_count, 2);
        assert_eq!(readiness.isolated_room_count, 1);
        assert!(!readiness.metadata_complete);
        assert!(
            readiness
                .diagnostics
                .iter()
                .all(|item| !item.subject.contains("healthy-room")
                    && !item.subject.contains("isolated-room"))
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn explicit_metadata_bridge_accepts_healthy_and_pre_existing_isolated_rooms()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("restore-readiness-bridge-isolated")?;
        let restored = fixture_path("restore-readiness-bridge-isolated-restored");
        let _ = fs::remove_file(&restored);
        add_isolated_fixture_room(&path)?;

        restore_file(&path, &restored)?;
        let mut evidence = extract_restore_evidence(&restored, NativeSqliteLimits::default())?;
        evidence.backend = Some(BackendProfileV1::SqliteBundled);
        evidence.native_point = Some(BackendNativePointV1::SqliteOnlineBackup {
            engine_identity: BUNDLED_SQLITE_VERSION.to_owned(),
            point_id: "explicit-test-adapter-point".to_owned(),
        });
        evidence.storage_epoch = Some(7);
        evidence.migration_metadata = Some(vec![]);
        evidence.pack_metadata = Some(vec![]);
        evidence.resource_metadata = Some(vec![]);
        evidence.room_membership = Some(vec![
            NativeRestoreRoomMembershipV1 {
                room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
                integrity: IntegrityWitnessV1 {
                    source_status: IntegrityStatusV1::Healthy,
                    restored_status: IntegrityStatusV1::Healthy,
                    source_generation: 1,
                    restored_generation: 1,
                    source_isolated: false,
                    restored_isolated: false,
                },
            },
            NativeRestoreRoomMembershipV1 {
                room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
                integrity: IntegrityWitnessV1 {
                    source_status: IntegrityStatusV1::Quarantined,
                    restored_status: IntegrityStatusV1::Quarantined,
                    source_generation: 4,
                    restored_generation: 4,
                    source_isolated: true,
                    restored_isolated: true,
                },
            },
        ]);

        let readiness = assess_restore_readiness(&evidence);
        assert!(readiness.metadata_complete);
        assert_eq!(readiness.healthy_room_count, 1);
        assert_eq!(readiness.isolated_room_count, 1);
        assert!(matches!(
            bridge_restore_evidence(&evidence),
            NativeSqliteRestoreBridgeResultV1::MetadataComplete { .. }
        ));

        let projection =
            project_restore_input(&evidence).map_err(|_| NativeSqliteError::InvalidRow {
                what: "healthy and isolated restore projection",
            })?;
        assert_eq!(projection.rooms.len(), 2);
        assert_eq!(
            projection.rooms[0].disposition,
            NativeSqliteRoomDispositionV1::Healthy
        );
        assert_eq!(
            projection.rooms[1].disposition,
            NativeSqliteRoomDispositionV1::Quarantined
        );
        assert!(projection.require_complete_metadata().is_ok());
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(restored);
        Ok(())
    }

    #[test]
    fn operational_extraction_is_lossless_and_verifier_covers_every_modeled_ledger()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("operational")?;
        populate_operational_fixture(&path)?;
        let before = fs::read(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        let extracted = extract_operational_rows(&path, NativeSqliteLimits::default())?;
        assert_eq!(extracted.tables["timers"].len(), 1);
        assert_eq!(extracted.tables["observation_frames"].len(), 1);
        assert_eq!(extracted.tables["activation_intents"].len(), 1);
        assert_eq!(extracted.tables["activation_operation_receipts"].len(), 1);
        assert_eq!(extracted.tables["semantic_receipts"].len(), 1);
        assert_eq!(
            extracted.tables["activation_operation_receipts"][0].values[3],
            NativeSqliteValueV1::Blob(vec![0x11; 32])
        );
        assert_eq!(
            extracted.tables["activation_intents"][0].values[16],
            NativeSqliteValueV1::Null
        );
        assert_eq!(
            extracted.tables["observation_consequences"][0].values[4],
            NativeSqliteValueV1::Null
        );

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(report.canonical_ready);
        assert_eq!(report.timer_count, 1);
        assert_eq!(report.frame_count, 1);
        assert_eq!(report.semantic_receipt_count, 1);
        assert_eq!(report.activation_intent_count, 1);
        assert_eq!(report.activation_receipt_count, 1);
        assert_eq!(report.activation_decision_count, 1);
        assert_eq!(report.observation_consequence_count, 1);
        assert_eq!(
            before,
            fs::read(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn operational_hash_corruption_is_blocking_and_bounded_without_mutation()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("operational-corrupt")?;
        populate_operational_fixture(&path)?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute("UPDATE observation_frames SET payload_bytes = x'00'", [])
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        connection
            .execute(
                "INSERT INTO timers VALUES ('01ARZ3NDEKTSV4RRFFQ69G5FAV', 'timer-2', 1, '2025-01-02T00:00:00Z', x'02', 'scheduled')",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        drop(connection);
        let before = fs::read(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|item| item.code == "frame_row_mismatch")
        );
        assert_eq!(report.timer_count, 2);
        assert_eq!(
            before,
            fs::read(&path).map_err(|error| NativeSqliteError::Io(error.to_string()))?
        );
        assert_eq!(
            extract_operational_rows(
                &path,
                NativeSqliteLimits {
                    max_rows: 1,
                    ..NativeSqliteLimits::default()
                }
            ),
            Err(NativeSqliteError::OutputBoundExceeded)
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn missing_operational_table_returns_a_redacted_blocking_report()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("operational-missing")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection.execute("DROP TABLE timers", []).map_err(|_| {
            NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            }
        })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|item| item.code == "required_table_missing")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "missing table diagnostic",
            })?;
        assert!(diagnostic.blocking);
        assert!(diagnostic.subject.starts_with("subject:"));
        assert!(!diagnostic.subject.contains("timers"));
        assert_eq!(
            extract_operational_rows(&path, NativeSqliteLimits::default()),
            Err(NativeSqliteError::InvalidRow {
                what: "operational table missing"
            })
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn deleted_or_corrupt_snapshot_is_disposable_after_canonical_verification()
    -> Result<(), NativeSqliteError> {
        let path = create_fixture("disposable")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute("DELETE FROM room_snapshots", [])
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "test" })?;
        drop(connection);
        let deleted = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(deleted.canonical_ready);
        assert_eq!(deleted.snapshot_count, 0);
        assert!(
            deleted
                .diagnostics
                .iter()
                .any(|item| item.code == "paired_snapshots_absent_disposable" && !item.blocking)
        );

        let _ = create_fixture("disposable")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute("UPDATE room_snapshots SET core_state_bytes = x'00'", [])
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "test" })?;
        drop(connection);
        let corrupt = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(corrupt.canonical_ready);
        assert_eq!(corrupt.valid_snapshot_count, 0);
        assert!(
            corrupt
                .diagnostics
                .iter()
                .any(|item| item.code == "paired_snapshot_disposable" && !item.blocking)
        );
        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn malformed_snapshot_complete_head_is_disposable() -> Result<(), NativeSqliteError> {
        let path = create_fixture("snapshot-head-corrupt")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute("UPDATE room_snapshots SET complete_head_bytes = x'00'", [])
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "test" })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(report.canonical_ready);
        assert_eq!(report.valid_snapshot_count, 0);
        assert_eq!(report.disposable_snapshot_count, 1);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|item| item.code == "paired_snapshot_disposable" && !item.blocking)
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn malformed_membership_shape_is_blocking() -> Result<(), NativeSqliteError> {
        let path = create_fixture("membership-shape-corrupt")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "INSERT INTO room_members VALUES (?1, 'member-2', 'principal-1', 'human', 'enabled', 'spectator', 'unexpected-role', x'01', 0, 1, 1, NULL, NULL, 0)",
                params!["01ARZ3NDEKTSV4RRFFQ69G5FAV"],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "test" })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|item| item.code == "member_row_mismatch" && item.blocking)
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn retained_activation_context_hash_mismatch_is_blocking() -> Result<(), NativeSqliteError> {
        let path = create_fixture("activation-context-corrupt")?;
        populate_operational_fixture(&path)?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute(
                "UPDATE activation_operation_receipts SET context_hash = x'11', context_bytes = x'636f6e74657874'",
                [],
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "test" })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        assert!(!report.canonical_ready);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|item| item.code == "activation_receipt_mismatch" && item.blocking)
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn bundled_online_backup_and_restore_publish_verified_files() -> Result<(), NativeSqliteError> {
        let source = create_fixture("copy-source")?;
        let backup = fixture_path("copy-backup");
        let restored = fixture_path("copy-restored");
        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&restored);

        let backup_report = backup_file(&source, &backup)?;
        assert_eq!(backup_report.engine_version, BUNDLED_SQLITE_VERSION);
        assert!(backup_report.source_canonical_ready);
        assert!(backup_report.destination_canonical_ready);
        assert!(nonempty_temporary_entries(&backup)?.is_empty());
        let restore_report = restore_file(&backup, &restored)?;
        assert!(restore_report.destination_canonical_ready);
        assert!(nonempty_temporary_entries(&restored)?.is_empty());
        let backup_bytes =
            fs::read(&backup).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let restored_bytes =
            fs::read(&restored).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        assert_eq!(backup_bytes, restored_bytes);
        assert_eq!(
            backup_file(&source, &backup),
            Err(NativeSqliteError::InvalidPath)
        );

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(restored);
        Ok(())
    }

    #[test]
    fn streaming_restore_publishes_only_matching_bounded_verification()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("streaming-restore-source")?;
        let backup = fixture_path("streaming-restore-backup");
        let restored = fixture_path("streaming-restore-target");
        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&restored);

        backup_file(&source, &backup)?;
        let report = restore_file_streaming_v2(&backup, &restored)?;
        assert_eq!(
            report.source_transfer_point_digest,
            report.destination_transfer_point_digest
        );
        assert!(!report.operational_relation_counts.is_empty());
        assert!(nonempty_temporary_entries(&restored)?.is_empty());

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(restored);
        Ok(())
    }

    #[test]
    fn wal_active_writer_is_rejected_without_mutating_native_evidence()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("wal-source")?;
        let backup = fixture_path("wal-backup");
        let wal = source.with_file_name(format!(
            "{}-wal",
            source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(NativeSqliteError::InvalidPath)?
        ));
        let shm = source.with_file_name(format!(
            "{}-shm",
            source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(NativeSqliteError::InvalidPath)?
        ));
        let _ = fs::remove_file(&backup);
        let readonly = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| NativeSqliteError::QueryFailed)?;
        assert!(
            readonly
                .execute("CREATE TABLE read_only_must_reject(value TEXT)", [])
                .is_err()
        );
        drop(readonly);

        let writer = Connection::open(&source)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        writer
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = FULL;
                 CREATE TABLE wal_evidence(value TEXT);
                 INSERT INTO wal_evidence VALUES ('committed');
                 BEGIN IMMEDIATE;
                 INSERT INTO wal_evidence VALUES ('uncommitted');",
            )
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        assert!(wal.is_file());
        assert!(shm.is_file());
        let database_before =
            fs::read(&source).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let wal_before =
            fs::read(&wal).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let shm_before =
            fs::read(&shm).map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        assert_eq!(
            verify_file(&source, NativeSqliteLimits::default()),
            Err(NativeSqliteError::UnsafeSidecarState)
        );
        assert_eq!(
            backup_file(&source, &backup),
            Err(NativeSqliteError::UnsafeSidecarState)
        );
        assert!(!backup.exists());
        assert_eq!(
            database_before,
            fs::read(&source).map_err(|error| NativeSqliteError::Io(error.to_string()))?
        );
        assert_eq!(
            wal_before,
            fs::read(&wal).map_err(|error| NativeSqliteError::Io(error.to_string()))?
        );
        assert_eq!(
            shm_before,
            fs::read(&shm).map_err(|error| NativeSqliteError::Io(error.to_string()))?
        );

        drop(writer);
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(shm);
        Ok(())
    }

    #[test]
    fn wal_header_is_rejected_even_after_named_sidecars_are_removed()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("wal-header-only")?;
        let wal = native_query_sidecar_path(&source, "-wal")?;
        let shm = native_query_sidecar_path(&source, "-shm")?;
        let connection = Connection::open(&source)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|_| NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            })?;
        drop(connection);
        let _ = fs::remove_file(&wal);
        let _ = fs::remove_file(&shm);

        assert_eq!(
            verify_file(&source, NativeSqliteLimits::default()),
            Err(NativeSqliteError::UnsafeSidecarState)
        );

        let _ = fs::remove_file(source);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn existing_sidecar_victim_is_rejected_without_mutation_or_snapshot_leak()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("existing-sidecar-victim")?;
        let victim = fixture_path("existing-sidecar-victim-bytes");
        let wal = native_query_sidecar_path(&source, "-wal")?;
        let sentinel = b"unrelated same-owner WAL-name victim";
        let _ = fs::remove_file(&victim);
        let _ = fs::remove_file(&wal);
        fs::write(&victim, sentinel).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        fs::hard_link(&victim, &wal).map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        assert_eq!(
            verify_file(&source, NativeSqliteLimits::default()),
            Err(NativeSqliteError::UnsafeSidecarState)
        );
        assert_eq!(
            fs::read(&victim).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            sentinel
        );
        let leaked_snapshot = fs::read_dir(source.parent().ok_or(NativeSqliteError::InvalidPath)?)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".worldstream-retained-query-")
            });
        assert!(!leaked_snapshot);

        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(victim);
        let _ = fs::remove_file(source);
        Ok(())
    }

    #[test]
    fn sidecar_install_after_preflight_is_rejected_before_query() -> Result<(), NativeSqliteError> {
        let source = create_fixture("sidecar-after-preflight")?;
        let wal = native_query_sidecar_path(&source, "-wal")?;
        let retained = RetainedNativeSqliteSource::open(&source)?;
        let result = retained_native_query_coordinate_with_hooks(
            &source,
            &retained.file,
            retained.identity,
            || {
                fs::write(&wal, b"post-preflight WAL victim")
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))
            },
            || Ok(()),
        );

        assert!(matches!(result, Err(NativeSqliteError::UnsafeSidecarState)));
        assert_eq!(
            fs::read(&wal).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"post-preflight WAL victim"
        );
        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(source);
        Ok(())
    }

    #[test]
    fn sidecar_install_after_main_bind_is_rejected_without_mutation()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("sidecar-after-main-bind")?;
        let wal = native_query_sidecar_path(&source, "-wal")?;
        let retained = RetainedNativeSqliteSource::open(&source)?;
        let result = retained_native_query_coordinate_with_hooks(
            &source,
            &retained.file,
            retained.identity,
            || Ok(()),
            || {
                fs::write(&wal, b"post-bind WAL victim")
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))
            },
        );

        assert!(matches!(result, Err(NativeSqliteError::UnsafeSidecarState)));
        assert_eq!(
            fs::read(&wal).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"post-bind WAL victim"
        );
        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(source);
        Ok(())
    }

    #[test]
    fn transient_sidecar_swap_after_main_bind_changes_the_retained_parent_witness()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("transient-sidecar-after-main-bind")?;
        let shm = native_query_sidecar_path(&source, "-shm")?;
        let retained = RetainedNativeSqliteSource::open(&source)?;
        let result = retained_native_query_coordinate_with_hooks(
            &source,
            &retained.file,
            retained.identity,
            || Ok(()),
            || {
                fs::write(&shm, b"transient SHM victim")
                    .and_then(|()| fs::remove_file(&shm))
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))
            },
        );

        assert!(matches!(result, Err(NativeSqliteError::UnsafeSidecarState)));
        assert!(!shm.exists());
        let _ = fs::remove_file(source);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_retained_sidecar_reservations_make_transient_substitution_impossible()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("windows-retained-sidecar-reservations")?;
        let wal = native_query_sidecar_path(&source, "-wal")?;
        let retained = RetainedNativeSqliteSource::open(&source)?;
        let coordinate = retained_native_query_coordinate_with_hooks(
            &source,
            &retained.file,
            retained.identity,
            || Ok(()),
            || {
                if fs::remove_file(&wal).is_ok()
                    || OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&wal)
                        .is_ok()
                {
                    return Err(NativeSqliteError::UnsafeSidecarState);
                }
                Ok(())
            },
        )?;
        coordinate.revalidate()?;
        drop(coordinate);
        assert!(!wal.exists());
        let _ = fs::remove_file(source);
        Ok(())
    }

    #[test]
    fn rejected_staging_reopen_scrubs_only_the_retained_expected_authority()
    -> Result<(), NativeSqliteError> {
        let staging_path = fixture_path("rejected-reopen-staging");
        let victim_path = fixture_path("rejected-reopen-victim");
        let sentinel = b"unrelated same-owner reopen victim";
        let _ = fs::remove_file(&staging_path);
        let _ = fs::remove_file(&victim_path);
        fs::write(&staging_path, b"sensitive retained staging bytes")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        fs::write(&victim_path, sentinel)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let staging = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&staging_path)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let victim = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&victim_path)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let staging_identity = native_file_identity(&staging)?;

        assert!(cleanup_rejected_staging_reopen(
            &staging,
            staging_identity,
            &victim
        ));
        assert_eq!(
            fs::metadata(&staging_path)
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?
                .len(),
            0
        );
        assert_eq!(
            fs::read(&victim_path).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            sentinel
        );

        let _ = fs::remove_file(staging_path);
        let _ = fs::remove_file(victim_path);
        Ok(())
    }

    #[test]
    fn exclusive_lock_fails_closed_without_publishing() -> Result<(), NativeSqliteError> {
        let source = create_fixture("exclusive-lock")?;
        let destination = fixture_path("exclusive-lock-destination");
        let _ = fs::remove_file(&destination);
        let writer = Connection::open(&source)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        writer.execute_batch("BEGIN EXCLUSIVE;").map_err(|_| {
            NativeSqliteError::NativeOperationFailed {
                operation: "fixture",
            }
        })?;

        assert!(matches!(
            verify_file(&source, NativeSqliteLimits::default()),
            Err(NativeSqliteError::QueryFailed)
        ));
        assert!(matches!(
            backup_file_with_options(
                &source,
                &destination,
                NativeSqliteTransferOptions {
                    pages_per_step: 1,
                    max_steps: 2,
                }
            ),
            Err(NativeSqliteError::QueryFailed)
        ));
        assert!(!destination.exists());
        assert!(nonempty_temporary_entries(&destination)?.is_empty());

        drop(writer);
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(destination);
        Ok(())
    }

    #[test]
    fn interrupted_backup_and_restore_clean_partial_native_outputs() -> Result<(), NativeSqliteError>
    {
        let source = create_large_fixture("interrupted-source")?;
        let backup = fixture_path("interrupted-backup");
        let restored = fixture_path("interrupted-restored");
        let options = NativeSqliteTransferOptions {
            pages_per_step: 1,
            max_steps: 64,
        };
        let backup_result = transfer_file(&source, &backup, "backup", options, Some(1));
        assert_eq!(
            backup_result,
            Err(NativeSqliteError::NativeOperationFailed {
                operation: "backup"
            })
        );
        assert!(!backup.exists());
        assert!(nonempty_temporary_entries(&backup)?.is_empty());

        backup_file(&source, &backup)?;
        let restore_result = transfer_file(&backup, &restored, "restore", options, Some(1));
        assert_eq!(
            restore_result,
            Err(NativeSqliteError::NativeOperationFailed {
                operation: "restore"
            })
        );
        assert!(!restored.exists());
        assert!(nonempty_temporary_entries(&restored)?.is_empty());

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(restored);
        Ok(())
    }

    #[test]
    fn existing_and_unusable_destinations_are_rejected_without_replacement()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("destination-source")?;
        let existing = fixture_path("existing-destination");
        let existing_directory = fixture_path("existing-destination-directory");
        let missing_parent = fixture_path("missing-destination-parent").join("destination.db");
        let link_temporary = fixture_path("link-temporary");
        let link_destination = fixture_path("link-destination");
        let _ = fs::remove_file(&existing);
        let _ = fs::remove_dir(&existing_directory);
        let _ = fs::remove_file(&link_temporary);
        let _ = fs::remove_file(&link_destination);
        let _ = fs::remove_dir(fixture_path("missing-destination-parent"));
        fs::write(&existing, b"must remain")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        fs::create_dir(&existing_directory)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        assert_eq!(
            backup_file(&source, &existing),
            Err(NativeSqliteError::InvalidPath)
        );
        assert_eq!(
            fs::read(&existing).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"must remain"
        );
        assert_eq!(
            backup_file(&source, &existing_directory),
            Err(NativeSqliteError::InvalidPath)
        );
        assert_eq!(
            backup_file(&source, &missing_parent),
            Err(NativeSqliteError::InvalidPath)
        );

        fs::write(&link_temporary, b"temporary")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        fs::write(&link_destination, b"existing")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let mut link_source = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&link_temporary)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let link_identity = native_file_identity(&link_source)?;
        let link_fingerprint = native_file_fingerprint(&mut link_source)?;
        let mut published = None;
        assert_eq!(
            publish_without_replacement(
                &mut link_source,
                link_identity,
                &link_temporary,
                &link_destination,
                link_fingerprint,
                &mut published,
            ),
            Err(NativeSqliteError::InvalidPath)
        );
        assert!(published.is_none());
        assert_eq!(
            fs::read(&link_destination)
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"existing"
        );

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(existing);
        let _ = fs::remove_dir(existing_directory);
        let _ = fs::remove_file(link_temporary);
        let _ = fs::remove_file(link_destination);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_publication_never_publishes_a_substituted_temporary_name()
    -> Result<(), NativeSqliteError> {
        use std::io::Write as _;

        let destination = fixture_path("descriptor-publication-destination");
        let held = fixture_path("descriptor-publication-held");
        let _ = fs::remove_file(&destination);
        let _ = fs::remove_file(&held);
        let publication_parent = NativePublicationParent::open(&destination)?;
        let (temporary, mut source) = temporary_destination(&publication_parent, &destination)?;
        source
            .write_all(b"admitted native bytes")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let source_identity = native_file_identity(&source)?;
        source
            .sync_all()
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let fingerprint = native_file_fingerprint(&mut source)?;
        let mut published = None;

        let result = publish_without_replacement_with_hook(
            &mut source,
            source_identity,
            &temporary,
            &destination,
            fingerprint,
            &mut published,
            |partial| {
                fs::rename(partial, &held)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
                fs::write(partial, b"substituted native bytes")
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))
            },
        );

        assert_eq!(result, Err(NativeSqliteError::InvalidPath));
        assert!(!destination.exists());
        let cleanup = cleanup_native_publication(
            &temporary,
            source_identity,
            &source,
            &publication_parent,
            published.as_ref(),
            false,
            NativeSqliteError::InvalidPath,
        );
        assert_eq!(cleanup, NativeSqliteError::InvalidPath);
        assert!(!destination.exists());
        assert_eq!(
            fs::metadata(&held)
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?
                .len(),
            0
        );
        assert_eq!(
            fs::read(&temporary).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"substituted native bytes"
        );

        let _ = fs::remove_file(destination);
        let _ = fs::remove_file(held);
        let _ = fs::remove_file(temporary);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_open_never_mutates_a_substituted_staging_hardlink() -> Result<(), NativeSqliteError> {
        let source = create_fixture("pre-open-staging-source")?;
        let destination = fixture_path("pre-open-staging-destination");
        let held = fixture_path("pre-open-staging-held");
        let victim = fixture_path("pre-open-staging-victim");
        for path in [&destination, &held, &victim] {
            let _ = fs::remove_file(path);
        }
        let sentinel = b"unrelated same-owner staging victim";
        fs::write(&victim, sentinel).map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        let result = transfer_file_with_hooks(
            &source,
            &destination,
            "backup",
            NativeSqliteTransferOptions::default(),
            None,
            |_, temporary| {
                fs::rename(temporary, &held)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
                fs::hard_link(&victim, temporary)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))
            },
            |_| Ok(()),
        );

        assert_eq!(result, Err(NativeSqliteError::InvalidPath));
        assert!(!destination.exists());
        assert_eq!(
            fs::read(&victim).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            sentinel
        );

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(held);
        let _ = fs::remove_file(destination);
        let _ = fs::remove_file(victim);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_open_rejects_source_replacement_after_initial_verification()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("pre-open-source-admitted")?;
        let held = fixture_path("pre-open-source-held");
        let alternate = create_fixture("pre-open-source-alternate")?;
        let destination = fixture_path("pre-open-source-destination");
        let _ = fs::remove_file(&held);
        let _ = fs::remove_file(&destination);
        let alternate_before =
            fs::read(&alternate).map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        let result = transfer_file_with_hooks(
            &source,
            &destination,
            "backup",
            NativeSqliteTransferOptions::default(),
            None,
            |source, _| {
                fs::rename(source, &held)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
                fs::rename(&alternate, source)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))
            },
            |_| Ok(()),
        );

        assert_eq!(result, Err(NativeSqliteError::InvalidPath));
        assert!(!destination.exists());
        assert_eq!(
            fs::read(&source).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            alternate_before
        );

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(held);
        let _ = fs::remove_file(destination);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_publication_rejects_destination_parent_substitution() -> Result<(), NativeSqliteError>
    {
        let source = create_fixture("parent-substitution-source")?;
        let root = fixture_path("parent-substitution-root");
        let admitted_parent = root.join("admitted");
        let held_parent = root.join("held-admitted");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&admitted_parent)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let destination = admitted_parent.join("backup.sqlite3");
        let options = NativeSqliteTransferOptions::default();

        let result =
            transfer_file_with_publish_hook(&source, &destination, "backup", options, None, |_| {
                let staged = nonempty_temporary_entries(&destination)?;
                let staged_name = staged
                    .first()
                    .and_then(|path| path.file_name())
                    .ok_or(NativeSqliteError::InvalidPath)?
                    .to_owned();
                fs::rename(&admitted_parent, &held_parent)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
                fs::create_dir(&admitted_parent)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
                fs::rename(
                    held_parent.join(&staged_name),
                    admitted_parent.join(&staged_name),
                )
                .map_err(|error| NativeSqliteError::Io(error.to_string()))
            });

        assert!(result.is_err());
        assert!(
            !destination.exists()
                || fs::metadata(&destination)
                    .map_err(|error| NativeSqliteError::Io(error.to_string()))?
                    .len()
                    == 0,
            "publication reached the replacement parent"
        );
        let _ = fs::remove_file(source);
        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_cleanup_never_unlinks_a_substituted_name() -> Result<(), NativeSqliteError> {
        let temporary = fixture_path("cleanup-substitution-partial");
        let held = fixture_path("cleanup-substitution-held");
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_file(&held);
        fs::write(&temporary, b"admitted native bytes")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let source = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let identity = native_file_identity(&source)?;
        let replacement = b"same-owner replacement";

        let result = remove_published_source_with_hook(&temporary, identity, false, || {
            fs::rename(&temporary, &held)
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
            fs::write(&temporary, replacement)
                .map_err(|error| NativeSqliteError::Io(error.to_string()))
        });

        assert!(result.is_err());
        assert_eq!(
            fs::read(&temporary).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            replacement
        );
        let _ = fs::remove_file(temporary);
        let _ = fs::remove_file(held);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_cleanup_refuses_to_truncate_a_hardlink_alias() -> Result<(), NativeSqliteError> {
        let destination = fixture_path("cleanup-hardlink-destination");
        let publication_parent = NativePublicationParent::open(&destination)?;
        let (temporary, _source) = temporary_destination(&publication_parent, &destination)?;
        fs::write(&temporary, b"retained staging bytes")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let source = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let identity = native_file_identity(&source)?;
        let alias = fixture_path("cleanup-hardlink-alias");
        let _ = fs::remove_file(&alias);
        fs::hard_link(&temporary, &alias)
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        let result = cleanup_native_publication(
            &temporary,
            identity,
            &source,
            &publication_parent,
            None,
            false,
            NativeSqliteError::InvalidPath,
        );

        assert!(matches!(
            result,
            NativeSqliteError::CleanupFailed {
                what: "retained native publication cleanup"
            }
        ));
        assert_eq!(
            fs::read(&alias).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"retained staging bytes"
        );

        let _ = fs::remove_file(alias);
        let _ = fs::remove_file(temporary);
        Ok(())
    }

    #[test]
    fn destination_appearing_before_publish_is_rejected_without_replacement_and_cleans_temp()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("publish-race-source")?;
        let destination = fixture_path("publish-race-destination");
        let _ = fs::remove_file(&destination);
        let options = NativeSqliteTransferOptions {
            pages_per_step: 1,
            max_steps: 64,
        };

        let result = transfer_file_with_fault_injection(
            &source,
            &destination,
            "backup",
            options,
            NativeSqliteFaultInjection::DestinationExistsBeforePublish,
        );

        assert_eq!(
            result,
            Err(NativeSqliteError::InvalidPath),
            "a destination created after validation must still fail closed"
        );
        assert_eq!(
            fs::read(&destination).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"destination-wins"
        );
        assert!(nonempty_temporary_entries(&destination)?.is_empty());

        let backup = fixture_path("publish-race-restore-source");
        let restore_destination = fixture_path("publish-race-restore-destination");
        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&restore_destination);
        backup_file(&source, &backup)?;
        let backup_before =
            fs::read(&backup).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        let restore_result = transfer_file_with_fault_injection(
            &backup,
            &restore_destination,
            "restore",
            options,
            NativeSqliteFaultInjection::DestinationExistsBeforePublish,
        );
        assert_eq!(restore_result, Err(NativeSqliteError::InvalidPath));
        assert_eq!(
            fs::read(&restore_destination)
                .map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"destination-wins"
        );
        assert_eq!(
            fs::read(&backup).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            backup_before
        );
        assert!(nonempty_temporary_entries(&restore_destination)?.is_empty());

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(destination);
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(restore_destination);
        Ok(())
    }

    #[test]
    fn restore_rejects_existing_destination_and_preserves_source_and_target()
    -> Result<(), NativeSqliteError> {
        let source = create_fixture("restore-side-effect-source")?;
        let backup = fixture_path("restore-side-effect-backup");
        let existing = fixture_path("restore-side-effect-existing");
        let restored = fixture_path("restore-side-effect-restored");
        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&existing);
        let _ = fs::remove_file(&restored);
        backup_file(&source, &backup)?;
        let source_before =
            fs::read(&backup).map_err(|error| NativeSqliteError::Io(error.to_string()))?;
        fs::write(&existing, b"do-not-replace")
            .map_err(|error| NativeSqliteError::Io(error.to_string()))?;

        assert_eq!(
            restore_file(&backup, &existing),
            Err(NativeSqliteError::InvalidPath)
        );
        assert_eq!(
            fs::read(&existing).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            b"do-not-replace"
        );
        assert_eq!(
            fs::read(&backup).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            source_before
        );

        let report = restore_file(&backup, &restored)?;
        assert!(report.destination_canonical_ready);
        assert_eq!(
            fs::read(&backup).map_err(|error| NativeSqliteError::Io(error.to_string()))?,
            source_before
        );
        assert!(nonempty_temporary_entries(&restored)?.is_empty());

        let _ = fs::remove_file(source);
        let _ = fs::remove_file(backup);
        let _ = fs::remove_file(existing);
        let _ = fs::remove_file(restored);
        Ok(())
    }

    #[test]
    fn native_diagnostics_are_actionable_and_redacted() -> Result<(), NativeSqliteError> {
        let path = create_fixture("diagnostic-action")?;
        let connection = Connection::open(&path)
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "open" })?;
        connection
            .execute("UPDATE transitions SET transition_bytes = x'00'", [])
            .map_err(|_| NativeSqliteError::NativeOperationFailed { operation: "test" })?;
        drop(connection);

        let report = verify_file(&path, NativeSqliteLimits::default())?;
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|item| item.code == "lineage_hash_mismatch")
            .ok_or(NativeSqliteError::InvalidRow {
                what: "lineage diagnostic",
            })?;
        assert!(diagnostic.blocking);
        assert!(!diagnostic.action.is_empty());
        assert!(diagnostic.subject.starts_with("subject:"));
        assert!(!diagnostic.subject.contains("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(!diagnostic.action.contains(path.to_string_lossy().as_ref()));

        let _ = fs::remove_file(path);
        Ok(())
    }
}
