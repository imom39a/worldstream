//! Explicit bridge from bounded native SQLite operational rows to transfer records.
//!
//! This module intentionally stops at the operational-row boundary. It does not
//! construct `BackupImageV1`, hydrate Core state, or contact a provider. Every
//! row is encoded with its SQLite storage class intact, while relation and
//! integrity checks prevent a partial or ambiguous row set from becoming a
//! transfer bundle.

#![allow(clippy::doc_markdown)]

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;
use worldstream_backup::native_sqlite::{
    NativeSqliteOperationalRowsV1, NativeSqliteRowV1, NativeSqliteValueV1,
};

use crate::{
    BackendFingerprintV1, CanonicalRecordKindV1, DeploymentIdentityV1, DigestV1, LogicalRecordV1,
    PackIdentityV1, ResourceIdentityV1, ResourceKindV1, ResourcePayloadV1, SessionStatePolicyV1,
    TransferBundleV1, TransferError,
};

const MANIFEST_MAGIC: &[u8; 8] = b"WSNSMF01";
const ROW_MAGIC: &[u8; 8] = b"WSNSROW1";
const FORMAT_VERSION: u16 = 1;
const MANIFEST_IDENTITY: &str = "native-sqlite/manifest/v1";
const DEPLOYMENT_IDENTITY: &str = "deployment/identity";
const ROW_IDENTITY_PREFIX: &str = "native-sqlite/row/";

const TABLES: &[(&str, usize)] = &[
    ("retired_authority_fences_v1", 6),
    ("principals", 4),
    ("runners", 4),
    ("capabilities", 10),
    ("capability_scopes", 2),
    ("runner_capability_memberships", 3),
    ("authority_change_receipts", 9),
    ("authority_audit", 12),
    ("activation_decisions", 5),
    ("activation_intents", 19),
    ("activation_operation_receipts", 9),
    ("observation_consequences", 6),
    ("observation_frames", 6),
    ("room_integrity", 3),
    ("room_members", 13),
    ("semantic_receipts", 12),
    ("timers", 6),
    ("integrity_incidents", 6),
];

/// The immutable inputs needed to build an operational-row transfer bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteTransferSpecV1 {
    /// Stable transfer bundle identifier.
    pub bundle_id: String,
    /// Deployment lineage identity.
    pub lineage_id: String,
    /// Source storage epoch.
    pub source_epoch: u64,
    /// Exact source backend fingerprint.
    pub source_backend: BackendFingerprintV1,
    /// Exact target backend fingerprint.
    pub target_backend: BackendFingerprintV1,
    /// Retained Activity Pack identity.
    pub pack: PackIdentityV1,
    /// Content-addressed resources bound to the transfer.
    pub resources: Vec<ResourceIdentityV1>,
    /// Exact resource payloads corresponding one-for-one with `resources`.
    /// An empty vector is authoritative only when `resources` is empty.
    pub resource_payloads: Vec<ResourcePayloadV1>,
    /// Session state policy carried by the transfer contract.
    pub session_state: SessionStatePolicyV1,
    /// Complete source-authenticated pack/resource identity set. Its
    /// presence is also the explicit empty-resource witness.
    pub deployment_identity: Option<DeploymentIdentityV1>,
}

impl NativeSqliteTransferSpecV1 {
    /// Builds a transfer specification without contacting either provider.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        bundle_id: impl Into<String>,
        lineage_id: impl Into<String>,
        source_epoch: u64,
        source_backend: BackendFingerprintV1,
        target_backend: BackendFingerprintV1,
        pack: PackIdentityV1,
        resources: Vec<ResourceIdentityV1>,
        session_state: SessionStatePolicyV1,
    ) -> Self {
        Self {
            bundle_id: bundle_id.into(),
            lineage_id: lineage_id.into(),
            source_epoch,
            source_backend,
            target_backend,
            pack: pack.clone(),
            resources: resources.clone(),
            resource_payloads: Vec::new(),
            session_state,
            deployment_identity: DeploymentIdentityV1::new(vec![pack], resources).ok(),
        }
    }

    /// Builds a transfer specification from the complete identity persisted by
    /// the source adapter. No caller-provided pack/resource projection is
    /// accepted separately.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_deployment_identity(
        bundle_id: impl Into<String>,
        lineage_id: impl Into<String>,
        source_epoch: u64,
        source_backend: BackendFingerprintV1,
        target_backend: BackendFingerprintV1,
        deployment_identity: DeploymentIdentityV1,
        session_state: SessionStatePolicyV1,
    ) -> Self {
        let pack = deployment_identity
            .packs()
            .first()
            .cloned()
            .unwrap_or_else(|| unreachable!("deployment identity validates non-empty packs"));
        Self {
            bundle_id: bundle_id.into(),
            lineage_id: lineage_id.into(),
            source_epoch,
            source_backend,
            target_backend,
            pack,
            resources: deployment_identity.resources().to_vec(),
            resource_payloads: Vec::new(),
            session_state,
            deployment_identity: Some(deployment_identity),
        }
    }

    /// Builds a source-authenticated specification with the complete identity
    /// set and every exact resource payload it names.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_deployment_resources(
        bundle_id: impl Into<String>,
        lineage_id: impl Into<String>,
        source_epoch: u64,
        source_backend: BackendFingerprintV1,
        target_backend: BackendFingerprintV1,
        deployment_identity: DeploymentIdentityV1,
        resource_payloads: Vec<ResourcePayloadV1>,
        session_state: SessionStatePolicyV1,
    ) -> Self {
        let mut spec = Self::new_with_deployment_identity(
            bundle_id,
            lineage_id,
            source_epoch,
            source_backend,
            target_backend,
            deployment_identity,
            session_state,
        );
        spec.resource_payloads = resource_payloads;
        spec
    }

    /// Returns the complete source identity set that must be carried by a
    /// whole-deployment bundle.
    #[must_use]
    pub fn deployment_identity(&self) -> Option<&DeploymentIdentityV1> {
        self.deployment_identity.as_ref()
    }
}

/// The policy observed for one source Room's durable integrity row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSqliteRoomPolicyV1 {
    /// Healthy Rooms are subject to all operational hash and relation checks.
    Healthy,
    /// Faulted or quarantined Rooms remain byte-preserved and isolated; their
    /// row-level semantic mismatch does not poison unrelated healthy Rooms.
    IsolatedCorrupt,
}

/// Bounded evidence summary embedded in and recoverable from a native bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSqliteBundleSummaryV1 {
    table_counts: BTreeMap<String, usize>,
    room_policies: BTreeMap<String, NativeSqliteRoomPolicyV1>,
    row_digest: DigestV1,
}

impl NativeSqliteBundleSummaryV1 {
    /// Returns the exact row count for each modeled operational table.
    #[must_use]
    pub fn table_counts(&self) -> &BTreeMap<String, usize> {
        &self.table_counts
    }

    /// Returns the healthy/isolated policy for every observed Room.
    #[must_use]
    pub fn room_policies(&self) -> &BTreeMap<String, NativeSqliteRoomPolicyV1> {
        &self.room_policies
    }

    /// Returns the digest over the exact encoded operational rows.
    #[must_use]
    pub const fn row_digest(&self) -> DigestV1 {
        self.row_digest
    }

    /// Returns whether this evidence contains an explicitly isolated Room.
    #[must_use]
    pub fn has_isolated_room(&self) -> bool {
        self.room_policies
            .values()
            .any(|policy| *policy == NativeSqliteRoomPolicyV1::IsolatedCorrupt)
    }
}

/// Errors returned while turning bounded native rows into a transfer bundle.
#[derive(Debug, Eq, Error, PartialEq)]
pub enum NativeSqliteTransferError {
    /// A required operational table was not present in the bounded evidence.
    #[error("native SQLite transfer relation is missing: {relation}")]
    MissingRelation { relation: &'static str },
    /// A row did not have the fixed native column shape.
    #[error(
        "native SQLite transfer row shape is invalid for {table}: expected {expected}, got {actual}"
    )]
    InvalidRowShape {
        table: &'static str,
        expected: usize,
        actual: usize,
    },
    /// The row's embedded table name differed from its map relation.
    #[error("native SQLite transfer row table identity is inconsistent")]
    RowTableMismatch,
    /// A row value had an invalid storage class or semantic value.
    #[error("native SQLite transfer row is invalid in {table}: {what}")]
    InvalidRow {
        table: &'static str,
        what: &'static str,
    },
    /// A row identity occurred twice with identical bytes.
    #[error("native SQLite transfer row is duplicated in {table}")]
    DuplicateRow {
        table: &'static str,
        subject: DigestV1,
    },
    /// A row identity occurred twice with different exact bytes.
    #[error("native SQLite transfer row conflicts in {table}")]
    ConflictingRow {
        table: &'static str,
        subject: DigestV1,
    },
    /// A cross-table relation was not present or was inconsistent.
    #[error("native SQLite transfer relation is invalid in {table}: {what}")]
    InvalidRelation {
        table: &'static str,
        what: &'static str,
    },
    /// The manifest did not describe the exact row set.
    #[error("native SQLite transfer manifest mismatch: {what}")]
    ManifestMismatch { what: &'static str },
    /// The bundle wire contract rejected the assembled records.
    #[error(transparent)]
    Contract(#[from] TransferError),
    /// Canonical source evidence was absent or did not cover the operational
    /// Rooms. The adapter never synthesizes these records from operational
    /// rows because doing so would lose source semantics.
    #[error("native SQLite canonical evidence is invalid: {what}")]
    CanonicalEvidence { what: &'static str },
    /// A required canonical record kind was absent from the source evidence.
    #[error("native SQLite canonical evidence is missing {kind:?}")]
    MissingCanonicalRecord { kind: CanonicalRecordKindV1 },
}

#[derive(Clone)]
struct PreparedRow {
    table: &'static str,
    identity: String,
    room_id: Option<String>,
    values: Vec<NativeSqliteValueV1>,
    bytes: Vec<u8>,
}

/// Adapter operations for the bounded native SQLite evidence seam.
pub struct NativeSqliteTransferAdapterV1;

impl NativeSqliteTransferAdapterV1 {
    /// Builds a transfer bundle from the exact bounded operational rows.
    ///
    /// This is an operational-row bridge only. Canonical Room Genesis,
    /// Transitions, materializations, resource blobs, and the full
    /// `BackupImageV1` translation remain separate provider evidence.
    pub fn from_operational_rows(
        rows: &NativeSqliteOperationalRowsV1,
        spec: &NativeSqliteTransferSpecV1,
    ) -> Result<TransferBundleV1, NativeSqliteTransferError> {
        let summary = Self::summary_from_rows(rows)?;
        Self::assemble_bundle(rows, spec, &summary, &[])
    }

    /// Builds a complete transfer bundle when an independent source adapter
    /// supplies canonical Room evidence.
    ///
    /// The records are accepted only as already-hashed `LogicalRecordV1`
    /// values. Their bytes are copied without decoding or rewriting. This
    /// method requires deployment lineage, storage epoch, and the canonical
    /// Genesis, Head, Core materialization, and Activity materialization for
    /// every Room represented by the operational evidence. It intentionally
    /// does not derive any of those records from operational rows.
    pub fn from_operational_rows_with_canonical_records(
        rows: &NativeSqliteOperationalRowsV1,
        spec: &NativeSqliteTransferSpecV1,
        canonical_records: &[LogicalRecordV1],
    ) -> Result<TransferBundleV1, NativeSqliteTransferError> {
        let summary = Self::summary_from_rows(rows)?;
        validate_canonical_records(canonical_records, &summary)?;
        Self::assemble_bundle(rows, spec, &summary, canonical_records)
    }

    /// Validates the exact native operational bundle and returns its manifest.
    ///
    /// PostgreSQL destinations call this before verification and again before
    /// finalization. A non-native logical fixture is rejected rather than being
    /// silently treated as a complete operational import.
    pub fn validate_bundle(
        bundle: &TransferBundleV1,
    ) -> Result<NativeSqliteBundleSummaryV1, NativeSqliteTransferError> {
        let manifests = bundle
            .records()
            .iter()
            .filter(|record| record.identity() == MANIFEST_IDENTITY)
            .collect::<Vec<_>>();
        if manifests.len() != 1 {
            return Err(NativeSqliteTransferError::MissingRelation {
                relation: "native-sqlite manifest",
            });
        }
        let manifest = manifests[0];
        if manifest.kind() != crate::RecordKindV1::Canonical(CanonicalRecordKindV1::SchemaManifest)
        {
            return Err(NativeSqliteTransferError::ManifestMismatch {
                what: "manifest record kind",
            });
        }
        let expected = decode_manifest(manifest.bytes())?;
        // Empty modeled relations are valid source evidence.  Bundle records
        // contain rows, not zero-row table markers, so seed every relation
        // before reconstructing the operational map; otherwise a round-trip
        // validation incorrectly reports an empty table as missing.
        let mut tables = TABLES
            .iter()
            .map(|(table, _)| ((*table).to_owned(), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let mut operational_row_count = 0usize;
        let mut canonical_records = Vec::new();
        for record in bundle.records() {
            if record.identity() == MANIFEST_IDENTITY {
                continue;
            }
            if record.kind()
                == crate::RecordKindV1::Canonical(CanonicalRecordKindV1::NativeOperationalRow)
            {
                if !record.identity().starts_with(ROW_IDENTITY_PREFIX) {
                    return Err(NativeSqliteTransferError::ManifestMismatch {
                        what: "operational row identity",
                    });
                }
            } else {
                if !matches!(record.kind(), crate::RecordKindV1::Canonical(_)) {
                    return Err(NativeSqliteTransferError::ManifestMismatch {
                        what: "unexpected derived record",
                    });
                }
                canonical_records.push(record.clone());
                continue;
            }
            let row = decode_row(record.bytes())?;
            let Some((table, _)) = TABLES.iter().find(|(name, _)| *name == row.table) else {
                return Err(NativeSqliteTransferError::MissingRelation {
                    relation: "unknown operational table",
                });
            };
            let identity = row_identity(table, &row.values)?;
            if record.identity() != identity {
                return Err(NativeSqliteTransferError::ManifestMismatch {
                    what: "row identity",
                });
            }
            tables.entry(row.table.clone()).or_default().push(row);
            operational_row_count =
                operational_row_count
                    .checked_add(1)
                    .ok_or(TransferError::BoundExceeded {
                        what: "native operational rows",
                    })?;
        }
        if operational_row_count + canonical_records.len() + 1 != bundle.records().len() {
            return Err(NativeSqliteTransferError::ManifestMismatch {
                what: "record count",
            });
        }
        let rows = NativeSqliteOperationalRowsV1 { tables };
        let actual = Self::summary_from_rows(&rows)?;
        if actual != expected {
            return Err(NativeSqliteTransferError::ManifestMismatch {
                what: "table counts, Room policy, or row digest",
            });
        }
        if !canonical_records.is_empty() {
            if canonical_records
                .iter()
                .filter(|record| record.identity() == DEPLOYMENT_IDENTITY)
                .count()
                != 1
            {
                return Err(NativeSqliteTransferError::CanonicalEvidence {
                    what: "deployment identity cardinality",
                });
            }
            validate_canonical_records(&canonical_records, &actual)?;
        }
        Ok(actual)
    }

    /// Returns whether a bundle carries the native operational marker.
    #[must_use]
    pub fn is_native_bundle(bundle: &TransferBundleV1) -> bool {
        bundle
            .records()
            .iter()
            .any(|record| record.identity() == MANIFEST_IDENTITY)
    }

    /// Validates rows and computes the exact summary used by the manifest.
    pub fn summary_from_rows(
        rows: &NativeSqliteOperationalRowsV1,
    ) -> Result<NativeSqliteBundleSummaryV1, NativeSqliteTransferError> {
        let prepared = prepare_rows(rows)?;
        let mut room_policies = BTreeMap::new();
        for row in &prepared {
            if row.table != "room_integrity" {
                continue;
            }
            let status = text(&row.values, 1, row.table)?;
            let generation = integer(&row.values, 2, row.table)?;
            if generation <= 0 {
                return Err(NativeSqliteTransferError::InvalidRow {
                    table: row.table,
                    what: "integrity generation",
                });
            }
            let policy = match status {
                "healthy" => NativeSqliteRoomPolicyV1::Healthy,
                "faulted" | "quarantined" => NativeSqliteRoomPolicyV1::IsolatedCorrupt,
                _ => {
                    return Err(NativeSqliteTransferError::InvalidRow {
                        table: row.table,
                        what: "integrity status",
                    });
                }
            };
            let room_id = row
                .room_id
                .clone()
                .ok_or(NativeSqliteTransferError::InvalidRow {
                    table: row.table,
                    what: "Room identity",
                })?;
            if room_policies.insert(room_id, policy).is_some() {
                return Err(NativeSqliteTransferError::DuplicateRow {
                    table: row.table,
                    subject: subject(&row.identity),
                });
            }
        }
        validate_relations(&prepared, &room_policies)?;
        let table_counts = TABLES
            .iter()
            .map(|(table, _)| {
                (
                    (*table).to_owned(),
                    prepared.iter().filter(|row| row.table == *table).count(),
                )
            })
            .collect();
        let row_digest = rows_digest(&prepared);
        Ok(NativeSqliteBundleSummaryV1 {
            table_counts,
            room_policies,
            row_digest,
        })
    }

    fn assemble_bundle(
        rows: &NativeSqliteOperationalRowsV1,
        spec: &NativeSqliteTransferSpecV1,
        summary: &NativeSqliteBundleSummaryV1,
        canonical_records: &[LogicalRecordV1],
    ) -> Result<TransferBundleV1, NativeSqliteTransferError> {
        let mut records = Vec::with_capacity(canonical_records.len() + 2);
        for record in canonical_records {
            let crate::RecordKindV1::Canonical(kind) = record.kind() else {
                return Err(NativeSqliteTransferError::CanonicalEvidence {
                    what: "derived record",
                });
            };
            records.push((kind, record.identity().to_owned(), record.bytes().to_vec()));
        }
        records.push((
            CanonicalRecordKindV1::ArtifactMetadata,
            DEPLOYMENT_IDENTITY.to_owned(),
            spec.deployment_identity
                .as_ref()
                .ok_or(NativeSqliteTransferError::CanonicalEvidence {
                    what: "missing complete deployment identity",
                })?
                .canonical_bytes()?,
        ));
        let deployment_identity = spec.deployment_identity.as_ref().ok_or(
            NativeSqliteTransferError::CanonicalEvidence {
                what: "missing complete deployment identity",
            },
        )?;
        let mut payloads = spec.resource_payloads.iter().collect::<Vec<_>>();
        payloads.sort_by(|left, right| {
            (left.identity().kind(), left.identity().identity())
                .cmp(&(right.identity().kind(), right.identity().identity()))
        });
        if payloads.len() != deployment_identity.resources().len()
            || payloads
                .iter()
                .map(|payload| payload.identity())
                .ne(deployment_identity.resources().iter())
        {
            return Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "resource payload set",
            });
        }
        for payload in payloads {
            payload.identity().verify_bytes(payload.bytes())?;
            records.push((
                CanonicalRecordKindV1::ArtifactBytes,
                resource_record_identity(payload.identity()),
                payload.bytes().to_vec(),
            ));
        }
        records.push((
            CanonicalRecordKindV1::SchemaManifest,
            MANIFEST_IDENTITY.to_owned(),
            encode_manifest(summary),
        ));
        let prepared = prepare_rows(rows)?;
        for row in prepared {
            records.push((
                CanonicalRecordKindV1::NativeOperationalRow,
                row.identity,
                row.bytes,
            ));
        }
        records.sort_by(|left, right| (left.0 as u8, &left.1).cmp(&(right.0 as u8, &right.1)));
        let records = records
            .into_iter()
            .enumerate()
            .map(|(ordinal, (kind, identity, bytes))| {
                LogicalRecordV1::canonical(
                    u64::try_from(ordinal).map_err(|_| TransferError::BoundExceeded {
                        what: "native operational ordinal",
                    })?,
                    kind,
                    identity,
                    &bytes,
                )
            })
            .collect::<Result<Vec<_>, TransferError>>()?;
        TransferBundleV1::new_with_backend_fingerprints(
            spec.bundle_id.clone(),
            spec.lineage_id.clone(),
            spec.source_epoch,
            spec.source_backend.clone(),
            spec.target_backend.clone(),
            spec.pack.clone(),
            spec.resources.clone(),
            spec.session_state,
            records,
        )
        .map_err(Into::into)
    }
}

fn validate_canonical_records(
    records: &[LogicalRecordV1],
    summary: &NativeSqliteBundleSummaryV1,
) -> Result<(), NativeSqliteTransferError> {
    let mut seen = BTreeSet::new();
    let mut deployment_lineage = 0usize;
    let mut storage_epoch = 0usize;
    let mut deployment_identity = None;
    let mut resource_payloads = BTreeMap::<String, &[u8]>::new();
    let mut room_kinds = BTreeMap::<String, BTreeSet<CanonicalRecordKindV1>>::new();

    for record in records {
        let crate::RecordKindV1::Canonical(kind) = record.kind() else {
            return Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "derived record",
            });
        };
        if matches!(
            kind,
            CanonicalRecordKindV1::SchemaManifest | CanonicalRecordKindV1::NativeOperationalRow
        ) {
            return Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "adapter-owned record kind",
            });
        }
        if !seen.insert((kind, record.identity())) {
            return Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "duplicate canonical record",
            });
        }
        validate_canonical_identity(kind, record.identity())?;
        match kind {
            CanonicalRecordKindV1::DeploymentLineage => deployment_lineage += 1,
            CanonicalRecordKindV1::StorageEpoch => storage_epoch += 1,
            CanonicalRecordKindV1::ArtifactMetadata if record.identity() == DEPLOYMENT_IDENTITY => {
                let identity =
                    DeploymentIdentityV1::from_canonical_bytes(record.bytes()).map_err(|_| {
                        NativeSqliteTransferError::CanonicalEvidence {
                            what: "deployment identity encoding",
                        }
                    })?;
                if deployment_identity.replace(identity).is_some() {
                    return Err(NativeSqliteTransferError::CanonicalEvidence {
                        what: "deployment identity cardinality",
                    });
                }
            }
            CanonicalRecordKindV1::ArtifactBytes => {
                resource_payloads.insert(record.identity().to_owned(), record.bytes());
            }
            _ => {
                if let Some(room_id) = canonical_room_id(record.identity())? {
                    if !summary.room_policies.contains_key(&room_id) {
                        return relation("canonical evidence", "canonical Room is absent");
                    }
                    room_kinds.entry(room_id).or_default().insert(kind);
                }
            }
        }
    }

    if deployment_lineage == 0
        && storage_epoch == 0
        && deployment_identity.is_some()
        && room_kinds.is_empty()
    {
        verify_resource_payload_records(
            deployment_identity
                .as_ref()
                .unwrap_or_else(|| unreachable!()),
            &resource_payloads,
        )?;
        return Ok(());
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
    if let Some(identity) = &deployment_identity {
        verify_resource_payload_records(identity, &resource_payloads)?;
    } else if !resource_payloads.is_empty() {
        return Err(NativeSqliteTransferError::CanonicalEvidence {
            what: "resource payloads without deployment identity",
        });
    }
    for room_id in summary.room_policies.keys() {
        let kinds = room_kinds.get(room_id);
        for kind in [
            CanonicalRecordKindV1::RoomGenesis,
            CanonicalRecordKindV1::RoomHead,
            CanonicalRecordKindV1::CoreMaterialization,
            CanonicalRecordKindV1::ActivityMaterialization,
            CanonicalRecordKindV1::ArtifactMetadata,
        ] {
            if !kinds.is_some_and(|kinds| kinds.contains(&kind)) {
                return Err(NativeSqliteTransferError::MissingCanonicalRecord { kind });
            }
        }
    }
    Ok(())
}

fn verify_resource_payload_records(
    identity: &DeploymentIdentityV1,
    payloads: &BTreeMap<String, &[u8]>,
) -> Result<(), NativeSqliteTransferError> {
    if payloads.len() != identity.resources().len() {
        return Err(NativeSqliteTransferError::CanonicalEvidence {
            what: "resource payload cardinality",
        });
    }
    for resource in identity.resources() {
        let record_identity = resource_record_identity(resource);
        let bytes =
            payloads
                .get(&record_identity)
                .ok_or(NativeSqliteTransferError::CanonicalEvidence {
                    what: "resource payload absent",
                })?;
        resource
            .verify_bytes(bytes)
            .map_err(|_| NativeSqliteTransferError::CanonicalEvidence {
                what: "resource payload identity mismatch",
            })?;
    }
    Ok(())
}

fn resource_record_identity(resource: &ResourceIdentityV1) -> String {
    let kind = match resource.kind() {
        ResourceKindV1::Artifact => "artifact",
        ResourceKindV1::Codec => "codec",
        ResourceKindV1::Schema => "schema",
    };
    format!("deployment/resource/{kind}/{}", resource.identity())
}

fn validate_canonical_identity(
    kind: CanonicalRecordKindV1,
    identity: &str,
) -> Result<(), NativeSqliteTransferError> {
    let valid = match kind {
        CanonicalRecordKindV1::DeploymentLineage => identity == "deployment/lineage",
        CanonicalRecordKindV1::StorageEpoch => identity == "deployment/epoch",
        CanonicalRecordKindV1::RoomGenesis => room_identity_has_suffix(identity, "genesis"),
        CanonicalRecordKindV1::RoomHead => room_identity_has_suffix(identity, "head"),
        CanonicalRecordKindV1::CoreMaterialization => {
            room_identity_has_suffix(identity, "core")
                || room_identity_has_suffix(identity, "core-materialization")
        }
        CanonicalRecordKindV1::ActivityMaterialization => {
            room_identity_has_suffix(identity, "activity")
                || room_identity_has_suffix(identity, "activity-materialization")
        }
        CanonicalRecordKindV1::RoomTransition => room_transition_identity(identity),
        CanonicalRecordKindV1::ArtifactMetadata => {
            !identity.starts_with("room/")
                || room_identity_has_suffix(identity, "pack-revision-lock")
        }
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(NativeSqliteTransferError::CanonicalEvidence {
            what: "canonical identity/kind mismatch",
        })
    }
}

fn room_identity_has_suffix(identity: &str, suffix: &str) -> bool {
    let Some(rest) = identity.strip_prefix("room/") else {
        return false;
    };
    let mut components = rest.split('/');
    matches!((components.next(), components.next(), components.next()),
        (Some(room_id), Some(actual_suffix), None)
            if !room_id.is_empty() && actual_suffix == suffix)
}

fn room_transition_identity(identity: &str) -> bool {
    let Some(rest) = identity.strip_prefix("room/") else {
        return false;
    };
    let mut components = rest.split('/');
    let (Some(room_id), Some(kind), Some(sequence)) =
        (components.next(), components.next(), components.next())
    else {
        return false;
    };
    !room_id.is_empty()
        && kind == "transition"
        && components.next().is_none()
        && sequence.parse::<u64>().is_ok_and(|sequence| sequence > 0)
}

fn canonical_room_id(identity: &str) -> Result<Option<String>, NativeSqliteTransferError> {
    let Some(rest) = identity.strip_prefix("room/") else {
        return Ok(None);
    };
    let room_id = rest.split('/').next().unwrap_or_default();
    if room_id.is_empty() {
        return Err(NativeSqliteTransferError::CanonicalEvidence {
            what: "empty canonical Room identity",
        });
    }
    Ok(Some(room_id.to_owned()))
}

fn prepare_rows(
    rows: &NativeSqliteOperationalRowsV1,
) -> Result<Vec<PreparedRow>, NativeSqliteTransferError> {
    for (table, _) in TABLES {
        if !rows.tables.contains_key(*table) {
            return Err(NativeSqliteTransferError::MissingRelation { relation: table });
        }
    }
    let mut prepared = Vec::new();
    let mut seen = BTreeMap::<String, (DigestV1, Vec<u8>, &'static str)>::new();
    for (table, expected) in TABLES {
        let values = rows
            .tables
            .get(*table)
            .ok_or(NativeSqliteTransferError::MissingRelation { relation: table })?;
        if values.len() > 100_000 {
            return Err(NativeSqliteTransferError::InvalidRow {
                table,
                what: "row bound",
            });
        }
        for row in values {
            if row.table != *table {
                return Err(NativeSqliteTransferError::RowTableMismatch);
            }
            if row.values.len() != *expected {
                return Err(NativeSqliteTransferError::InvalidRowShape {
                    table,
                    expected: *expected,
                    actual: row.values.len(),
                });
            }
            let identity = row_identity(table, &row.values)?;
            let room_id = room_id(table, &row.values)?;
            let bytes = encode_row(table, &row.values)?;
            let digest = DigestV1::hash(&bytes);
            if let Some((prior_digest, prior_bytes, prior_table)) = seen.get(&identity) {
                if *prior_digest == digest && *prior_bytes == bytes {
                    return Err(NativeSqliteTransferError::DuplicateRow {
                        table: prior_table,
                        subject: subject(&identity),
                    });
                }
                return Err(NativeSqliteTransferError::ConflictingRow {
                    table,
                    subject: subject(&identity),
                });
            }
            seen.insert(identity.clone(), (digest, bytes.clone(), table));
            prepared.push(PreparedRow {
                table,
                identity,
                room_id,
                values: row.values.clone(),
                bytes,
            });
        }
    }
    Ok(prepared)
}

#[allow(clippy::too_many_lines)]
fn validate_relations(
    rows: &[PreparedRow],
    room_policies: &BTreeMap<String, NativeSqliteRoomPolicyV1>,
) -> Result<(), NativeSqliteTransferError> {
    let members = rows
        .iter()
        .filter(|row| row.table == "room_members")
        .map(|row| {
            Ok::<_, NativeSqliteTransferError>((
                required_room_id(row)?.to_owned(),
                text(&row.values, 1, row.table)?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let decisions = rows
        .iter()
        .filter(|row| row.table == "activation_decisions")
        .map(|row| {
            Ok::<_, NativeSqliteTransferError>((
                required_room_id(row)?.to_owned(),
                integer(&row.values, 1, row.table)?,
                text(&row.values, 2, row.table)?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let activations = rows
        .iter()
        .filter(|row| row.table == "activation_intents")
        .map(|row| text(&row.values, 0, row.table).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let principals = rows
        .iter()
        .filter(|row| row.table == "principals")
        .map(|row| text(&row.values, 0, row.table).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let runners = rows
        .iter()
        .filter(|row| row.table == "runners")
        .map(|row| text(&row.values, 0, row.table).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let capabilities = rows
        .iter()
        .filter(|row| row.table == "capabilities")
        .map(|row| text(&row.values, 0, row.table).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let capability_profiles = rows
        .iter()
        .filter(|row| row.table == "capabilities")
        .map(|row| {
            Ok::<_, NativeSqliteTransferError>((
                text(&row.values, 0, row.table)?.to_owned(),
                text(&row.values, 3, row.table)?.to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let capability_owners = rows
        .iter()
        .filter(|row| row.table == "capabilities")
        .map(|row| {
            Ok::<_, NativeSqliteTransferError>((
                text(&row.values, 0, row.table)?.to_owned(),
                text(&row.values, 2, row.table)?.to_owned(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut authority_receipts = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| row.table == "authority_change_receipts")
    {
        let change_id = text(&row.values, 0, row.table)?.to_owned();
        if authority_receipts.insert(change_id, row).is_some() {
            return relation(row.table, "unique authority change Receipt");
        }
    }
    let mut authority_audits = BTreeMap::new();
    for row in rows.iter().filter(|row| row.table == "authority_audit") {
        let change_id = text(&row.values, 1, row.table)?.to_owned();
        if authority_audits.insert(change_id, row).is_some() {
            return relation(row.table, "unique audit for authority change Receipt");
        }
    }
    if authority_receipts.keys().ne(authority_audits.keys()) {
        return relation(
            "authority_audit",
            "one-to-one authority change Receipt audit pairing",
        );
    }
    let authority_target_exists =
        |target_kind: &str, target_id: &str, secondary_target: Option<&str>| match target_kind {
            "bootstrap" => secondary_target.is_some_and(|capability_id| {
                principals.contains(target_id)
                    && capabilities.contains(capability_id)
                    && capability_owners.get(capability_id).map(String::as_str) == Some(target_id)
            }),
            "principal" => secondary_target.is_none() && principals.contains(target_id),
            "capability" => secondary_target.is_none() && capabilities.contains(target_id),
            "runner" => secondary_target.is_none() && runners.contains(target_id),
            _ => false,
        };

    for row in rows {
        let policy =
            if let Some(room_id) = row.room_id.as_deref() {
                Some(room_policies.get(room_id).ok_or(
                    NativeSqliteTransferError::MissingRelation {
                        relation: "room_integrity for operational Room",
                    },
                )?)
            } else {
                None
            };
        let isolated =
            policy.is_some_and(|policy| *policy == NativeSqliteRoomPolicyV1::IsolatedCorrupt);
        match row.table {
            "retired_authority_fences_v1" => {
                if integer(&row.values, 2, row.table)? <= 0
                    || blob(&row.values, 4, row.table)?.len() != 32
                    || !matches!(integer(&row.values, 5, row.table)?, 0 | 1)
                {
                    return invalid(row.table, "authority fence");
                }
                let _ = text(&row.values, 0, row.table)?;
                let _ = text(&row.values, 1, row.table)?;
                let _ = blob(&row.values, 3, row.table)?;
            }
            "principals" => {
                if !matches!(text(&row.values, 1, row.table)?, "human" | "agent")
                    || !matches!(text(&row.values, 2, row.table)?, "enabled" | "disabled")
                    || integer(&row.values, 3, row.table)? <= 0
                {
                    return invalid(row.table, "Principal authority fact");
                }
            }
            "runners" => {
                if !principals.contains(text(&row.values, 1, row.table)?)
                    || !matches!(text(&row.values, 2, row.table)?, "enabled" | "revoked")
                    || integer(&row.values, 3, row.table)? <= 0
                {
                    return relation(row.table, "Runner owner Principal");
                }
            }
            "capabilities" => {
                let profile = text(&row.values, 3, row.table)?;
                let target_room = optional_text(&row.values, 4, row.table)?;
                let target_member = optional_text(&row.values, 5, row.table)?;
                let runner = optional_text(&row.values, 6, row.table)?;
                if blob(&row.values, 1, row.table)?.len() != 32
                    || !principals.contains(text(&row.values, 2, row.table)?)
                    || !matches!(profile, "room_member" | "host_operator" | "runner_control")
                    || integer(&row.values, 7, row.table)? <= 0
                {
                    return relation(row.table, "Capability authority fact");
                }
                let profile_relation_valid = match profile {
                    "room_member" => {
                        target_room
                            .zip(target_member)
                            .is_some_and(|(room_id, member_id)| {
                                runner.is_none()
                                    && room_policies.contains_key(room_id)
                                    && members.contains(&(room_id.to_owned(), member_id.to_owned()))
                            })
                    }
                    "host_operator" => {
                        target_member.is_none()
                            && runner.is_none()
                            && target_room.is_none_or(|room_id| room_policies.contains_key(room_id))
                    }
                    "runner_control" => {
                        target_room.is_none()
                            && target_member.is_none()
                            && runner.is_some_and(|runner_id| runners.contains(runner_id))
                    }
                    _ => false,
                };
                if !profile_relation_valid {
                    return relation(row.table, "Capability target relation");
                }
                let _ = optional_text(&row.values, 8, row.table)?;
                let _ = optional_text(&row.values, 9, row.table)?;
            }
            "capability_scopes" => {
                if !capabilities.contains(text(&row.values, 0, row.table)?)
                    || !matches!(
                        text(&row.values, 1, row.table)?,
                        "room:attach"
                            | "room:act"
                            | "room:observe_public"
                            | "room:observe_member"
                            | "room:replay"
                            | "activation:offer_receive"
                            | "activation:claim"
                            | "activation:complete"
                            | "operator:room_admin"
                            | "operator:backup"
                    )
                {
                    return relation(row.table, "Capability scope owner");
                }
            }
            "runner_capability_memberships" => {
                let capability_id = text(&row.values, 0, row.table)?;
                let room_id = text(&row.values, 1, row.table)?;
                let member_id = text(&row.values, 2, row.table)?;
                if !capabilities.contains(capability_id)
                    || capability_profiles.get(capability_id).map(String::as_str)
                        != Some("runner_control")
                    || !room_policies.contains_key(room_id)
                    || !members.contains(&(room_id.to_owned(), member_id.to_owned()))
                {
                    return relation(row.table, "Runner Capability Membership");
                }
            }
            "authority_change_receipts" => {
                let actor = optional_text(&row.values, 1, row.table)?;
                let result_kind = text(&row.values, 3, row.table)?;
                let target_kind = text(&row.values, 4, row.table)?;
                let target_id = text(&row.values, 5, row.table)?;
                let secondary_target = optional_text(&row.values, 6, row.table)?;
                let generation = integer(&row.values, 7, row.table)?;
                if blob(&row.values, 2, row.table)?.len() != 32
                    || !matches!(
                        result_kind,
                        "authority_bootstrapped"
                            | "principal_created"
                            | "capability_registered"
                            | "capability_narrowed"
                            | "capability_revoked"
                            | "principal_status_changed"
                            | "runner_registered"
                            | "runner_revoked"
                    )
                    || !matches!(
                        target_kind,
                        "bootstrap" | "principal" | "capability" | "runner"
                    )
                    || generation <= 0
                    || if result_kind == "authority_bootstrapped" {
                        actor.is_some()
                            || target_kind != "bootstrap"
                            || secondary_target.is_none()
                            || generation != 1
                    } else {
                        actor.is_none_or(|actor| !principals.contains(actor))
                            || target_kind == "bootstrap"
                            || secondary_target.is_some()
                    }
                {
                    return invalid(row.table, "authority change Receipt");
                }
                if !authority_target_exists(target_kind, target_id, secondary_target) {
                    return relation(row.table, "authority change target entity");
                }
                let _ = text(&row.values, 8, row.table)?;
            }
            "authority_audit" => {
                let actor = optional_text(&row.values, 2, row.table)?;
                let target_kind = text(&row.values, 3, row.table)?;
                let target_id = text(&row.values, 4, row.table)?;
                let secondary_target = optional_text(&row.values, 5, row.table)?;
                let change_kind = text(&row.values, 6, row.table)?;
                let prior_generation = optional_integer(&row.values, 7, row.table)?;
                let resulting_generation = integer(&row.values, 8, row.table)?;
                if integer(&row.values, 0, row.table)? <= 0
                    || !matches!(
                        target_kind,
                        "bootstrap" | "principal" | "capability" | "runner"
                    )
                    || !authority_target_exists(target_kind, target_id, secondary_target)
                    || !matches!(
                        change_kind,
                        "bootstrap_authority"
                            | "create_principal"
                            | "register_capability"
                            | "register_runner"
                            | "narrow_capability"
                            | "revoke_capability"
                            | "revoke_runner"
                            | "set_principal_status"
                    )
                    || !match prior_generation {
                        None => resulting_generation == 1,
                        Some(prior) => prior > 0 && resulting_generation == prior + 1,
                    }
                    || if change_kind == "bootstrap_authority" {
                        actor.is_some() || target_kind != "bootstrap" || secondary_target.is_none()
                    } else {
                        actor.is_none_or(|actor| !principals.contains(actor))
                            || target_kind == "bootstrap"
                            || secondary_target.is_some()
                    }
                    || blob(&row.values, 11, row.table)?.len() != 32
                {
                    return relation(row.table, "authority audit Receipt");
                }
                let _ = text(&row.values, 9, row.table)?;
                let _ = optional_text(&row.values, 10, row.table)?;
            }
            "room_integrity" => {}
            "room_members" => {
                let frame_head = integer(&row.values, 8, row.table)?;
                let membership_generation = integer(&row.values, 9, row.table)?;
                let retained_frame_floor = integer(&row.values, 10, row.table)?;
                let last_ack = optional_integer(&row.values, 11, row.table)?;
                let reset_required = optional_integer(&row.values, 12, row.table)?;
                let access_mode = text(&row.values, 5, row.table)?;
                let role = optional_text(&row.values, 6, row.table)?;
                if !matches!(text(&row.values, 3, row.table)?, "human" | "agent")
                    || !matches!(
                        text(&row.values, 4, row.table)?,
                        "enabled" | "suspended" | "departed"
                    )
                    || !matches!(access_mode, "participant" | "spectator" | "operator")
                    || (access_mode == "participant") != role.is_some()
                    || !matches!(row.values.get(7), Some(NativeSqliteValueV1::Blob(_)))
                    || frame_head < 0
                    || membership_generation <= 0
                    || retained_frame_floor <= 0
                    || last_ack.is_some_and(|value| value <= 0)
                    || reset_required.is_some_and(|value| value < 0)
                {
                    return Err(NativeSqliteTransferError::InvalidRow {
                        table: row.table,
                        what: "Membership authority/delivery shape",
                    });
                }
            }
            "timers" => {
                if !isolated {
                    let state = text(&row.values, 5, row.table)?;
                    if !matches!(state, "scheduled" | "cancelled" | "fired")
                        || text(&row.values, 3, row.table)?.is_empty()
                        || !matches!(row.values.get(4), Some(NativeSqliteValueV1::Blob(_)))
                    {
                        return invalid(row.table, "timer payload or state");
                    }
                }
            }
            "observation_frames" => {
                let member = text(&row.values, 1, row.table)?;
                if !members.contains(&(required_room_id(row)?.to_owned(), member.to_owned())) {
                    return relation(row.table, "frame Membership");
                }
                let cause = integer(&row.values, 3, row.table)?;
                if cause <= 0 {
                    return invalid(row.table, "frame causal sequence");
                }
                if !isolated {
                    let hash = parse_digest(text(&row.values, 4, row.table)?)?;
                    let payload = blob(&row.values, 5, row.table)?;
                    if hash != DigestV1::hash(payload) {
                        return invalid(row.table, "frame payload hash");
                    }
                }
            }
            "observation_consequences" => {
                let member = text(&row.values, 1, row.table)?;
                if !members.contains(&(required_room_id(row)?.to_owned(), member.to_owned())) {
                    return relation(row.table, "consequence Membership");
                }
                if integer(&row.values, 2, row.table)? <= 0 {
                    return invalid(row.table, "consequence causal sequence");
                }
            }
            "activation_decisions" => {
                if integer(&row.values, 1, row.table)? <= 0
                    || !matches!(row.values.get(4), Some(NativeSqliteValueV1::Blob(_)))
                {
                    return invalid(row.table, "Activation decision");
                }
                if let Some(target) = optional_text(&row.values, 3, row.table)?
                    && !members.contains(&(required_room_id(row)?.to_owned(), target.to_owned()))
                {
                    return relation(row.table, "decision target Membership");
                }
            }
            "activation_intents" => {
                let cause = integer(&row.values, 2, row.table)?;
                let decision = text(&row.values, 3, row.table)?;
                if !isolated
                    && !decisions.contains(&(
                        required_room_id(row)?.to_owned(),
                        cause,
                        decision.to_owned(),
                    ))
                {
                    return relation(row.table, "Activation decision");
                }
                if !isolated {
                    validate_activation_context(row)?;
                }
            }
            "activation_operation_receipts" => {
                if let Some(activation_id) = optional_text(&row.values, 4, row.table)?
                    && !activations.contains(activation_id)
                {
                    return relation(row.table, "Activation Intent");
                }
                if !isolated {
                    validate_activation_receipt(row)?;
                }
            }
            "semantic_receipts" => {
                if !isolated {
                    validate_semantic_receipt(row)?;
                }
            }
            "integrity_incidents" => {
                if integer(&row.values, 1, row.table)? <= 0
                    || integer(&row.values, 2, row.table)? <= 0
                    || !matches!(
                        text(&row.values, 3, row.table)?,
                        "healthy" | "faulted" | "quarantined"
                    )
                {
                    return invalid(row.table, "integrity incident");
                }
                let _ = text(&row.values, 4, row.table)?;
                let _ = optional_blob(&row.values, 5, row.table)?;
            }
            _ => return Err(NativeSqliteTransferError::RowTableMismatch),
        }
    }

    for (change_id, receipt) in authority_receipts {
        let audit =
            authority_audits
                .get(&change_id)
                .ok_or(NativeSqliteTransferError::InvalidRelation {
                    table: "authority_audit",
                    what: "authority change Receipt audit pairing",
                })?;
        let receipt_actor = optional_text(&receipt.values, 1, receipt.table)?;
        let receipt_request_hash = blob(&receipt.values, 2, receipt.table)?;
        let result_kind = text(&receipt.values, 3, receipt.table)?;
        let receipt_target_kind = text(&receipt.values, 4, receipt.table)?;
        let receipt_target_id = text(&receipt.values, 5, receipt.table)?;
        let receipt_secondary = optional_text(&receipt.values, 6, receipt.table)?;
        let receipt_generation = integer(&receipt.values, 7, receipt.table)?;
        let receipt_checked_at = text(&receipt.values, 8, receipt.table)?;
        let audit_actor = optional_text(&audit.values, 2, audit.table)?;
        let audit_target_kind = text(&audit.values, 3, audit.table)?;
        let audit_target_id = text(&audit.values, 4, audit.table)?;
        let audit_secondary = optional_text(&audit.values, 5, audit.table)?;
        let change_kind = text(&audit.values, 6, audit.table)?;
        let audit_prior_generation = optional_integer(&audit.values, 7, audit.table)?;
        let audit_generation = integer(&audit.values, 8, audit.table)?;
        let audit_checked_at = text(&audit.values, 9, audit.table)?;
        let audit_reason = optional_text(&audit.values, 10, audit.table)?;
        let audit_request_hash = blob(&audit.values, 11, audit.table)?;
        if receipt_actor != audit_actor
            || receipt_request_hash != audit_request_hash
            || receipt_target_kind != audit_target_kind
            || receipt_target_id != audit_target_id
            || receipt_secondary != audit_secondary
            || receipt_generation != audit_generation
            || receipt_checked_at != audit_checked_at
        {
            return relation(
                "authority_audit",
                "authority change Receipt audit field agreement",
            );
        }
        let expected = match result_kind {
            "authority_bootstrapped" => ("bootstrap", "bootstrap_authority", None, false),
            "principal_created" => ("principal", "create_principal", None, false),
            "capability_registered" => ("capability", "register_capability", None, false),
            "runner_registered" => ("runner", "register_runner", None, false),
            "capability_narrowed" => (
                "capability",
                "narrow_capability",
                receipt_generation.checked_sub(1),
                true,
            ),
            "capability_revoked" => (
                "capability",
                "revoke_capability",
                receipt_generation.checked_sub(1),
                true,
            ),
            "principal_status_changed" => (
                "principal",
                "set_principal_status",
                receipt_generation.checked_sub(1),
                true,
            ),
            "runner_revoked" => (
                "runner",
                "revoke_runner",
                receipt_generation.checked_sub(1),
                true,
            ),
            _ => return invalid(receipt.table, "authority change Receipt result"),
        };
        if receipt_target_kind != expected.0
            || change_kind != expected.1
            || audit_prior_generation != expected.2
            || audit_reason.is_some() != expected.3
            || (expected.2.is_none() && receipt_generation != 1)
        {
            return relation(
                "authority_audit",
                "authority change result and audit change mapping",
            );
        }
    }
    Ok(())
}

fn validate_activation_context(row: &PreparedRow) -> Result<(), NativeSqliteTransferError> {
    let state = text(&row.values, 10, row.table)?;
    if !matches!(
        state,
        "pending" | "leased" | "completed" | "expired" | "cancelled"
    ) {
        return invalid(row.table, "Activation state");
    }
    let runner = optional_text(&row.values, 13, row.table)?;
    let claim = optional_text(&row.values, 14, row.table)?;
    let until = optional_text(&row.values, 15, row.table)?;
    if (state == "leased") != (runner.is_some() && claim.is_some() && until.is_some()) {
        return invalid(row.table, "Activation lease tuple");
    }
    let hash = optional_blob(&row.values, 16, row.table)?;
    let context = optional_blob(&row.values, 17, row.table)?;
    let retired = integer(&row.values, 18, row.table)?;
    if retired != 0 && retired != 1 {
        return invalid(row.table, "context tombstone marker");
    }
    if let Some(context) = context {
        let Some(hash) = hash else {
            return invalid(row.table, "context hash missing");
        };
        if hash != DigestV1::hash(context).as_bytes() {
            return invalid(row.table, "context hash");
        }
    } else if retired == 1 && hash.is_none_or(|value| value.len() != 32) {
        return invalid(row.table, "context tombstone hash");
    } else if retired == 0 && hash.is_some() {
        return invalid(row.table, "unretained context hash");
    }
    Ok(())
}

fn validate_activation_receipt(row: &PreparedRow) -> Result<(), NativeSqliteTransferError> {
    if !matches!(
        text(&row.values, 2, row.table)?,
        "offer" | "claim" | "renew" | "complete" | "release"
    ) || optional_blob(&row.values, 7, row.table)?.is_some_and(|value| value.len() != 32)
        || optional_blob(&row.values, 7, row.table)?.is_some()
            != optional_blob(&row.values, 8, row.table)?.is_some()
    {
        return invalid(row.table, "Activation receipt");
    }
    if !matches!(row.values.get(3), Some(NativeSqliteValueV1::Blob(value)) if value.len() == 32)
        || !matches!(row.values.get(6), Some(NativeSqliteValueV1::Blob(_)))
    {
        return invalid(row.table, "Activation receipt hashes or result");
    }
    Ok(())
}

fn validate_semantic_receipt(row: &PreparedRow) -> Result<(), NativeSqliteTransferError> {
    if !matches!(
        text(&row.values, 1, row.table)?,
        "action" | "administration" | "timer_fired" | "external_input"
    ) || text(&row.values, 3, row.table)? != "worldstream/operation-receipt/v1"
        || !matches!(row.values.get(4), Some(NativeSqliteValueV1::Blob(value)) if value.len() == 32)
        || !matches!(row.values.get(6), Some(NativeSqliteValueV1::Blob(_)))
        || !matches!(row.values.get(7), Some(NativeSqliteValueV1::Blob(_)))
        || !matches!(
            text(&row.values, 8, row.table)?,
            "genesis_created"
                | "transition_committed"
                | "rejection_recorded"
                | "no_change_recorded"
        )
    {
        return invalid(row.table, "semantic receipt");
    }
    Ok(())
}

fn row_identity(
    table: &'static str,
    values: &[NativeSqliteValueV1],
) -> Result<String, NativeSqliteTransferError> {
    let key = match table {
        "retired_authority_fences_v1" => {
            format!("authority/fence/{}", text(values, 0, table)?)
        }
        "principals" => format!("authority/principal/{}", text(values, 0, table)?),
        "runners" => format!("authority/runner/{}", text(values, 0, table)?),
        "capabilities" => format!("authority/capability/{}", text(values, 0, table)?),
        "capability_scopes" => format!(
            "authority/capability/{}/scope/{}",
            text(values, 0, table)?,
            text(values, 1, table)?
        ),
        "runner_capability_memberships" => format!(
            "authority/capability/{}/membership/{}/{}",
            text(values, 0, table)?,
            text(values, 1, table)?,
            text(values, 2, table)?
        ),
        "authority_change_receipts" => {
            format!("authority/change/{}", text(values, 0, table)?)
        }
        "authority_audit" => format!("authority/audit/{}", integer(values, 0, table)?),
        "room_integrity" => format!("room/{}", text(values, 0, table)?),
        "room_members" => format!(
            "room/{}/member/{}",
            text(values, 0, table)?,
            text(values, 1, table)?
        ),
        "timers" => format!(
            "room/{}/timer/{}/{}",
            text(values, 0, table)?,
            text(values, 1, table)?,
            integer(values, 2, table)?
        ),
        "observation_frames" => format!(
            "room/{}/member/{}/frame/{}",
            text(values, 0, table)?,
            text(values, 1, table)?,
            integer(values, 2, table)?
        ),
        "observation_consequences" => format!(
            "room/{}/member/{}/consequence/{}",
            text(values, 0, table)?,
            text(values, 1, table)?,
            integer(values, 2, table)?
        ),
        "activation_decisions" => format!(
            "room/{}/decision/{}/{}",
            text(values, 0, table)?,
            integer(values, 1, table)?,
            text(values, 2, table)?
        ),
        "activation_intents" => format!(
            "room/{}/activation/{}",
            text(values, 1, table)?,
            text(values, 0, table)?
        ),
        "activation_operation_receipts" => format!(
            "room/{}/activation-receipt/{}",
            text(values, 0, table)?,
            text(values, 1, table)?
        ),
        "semantic_receipts" => format!(
            "room/{}/semantic-receipt/{}",
            text(values, 0, table)?,
            hex(blob(values, 2, table)?)
        ),
        "integrity_incidents" => format!(
            "room/{}/integrity-incident/{}",
            text(values, 0, table)?,
            integer(values, 1, table)?
        ),
        _ => return Err(NativeSqliteTransferError::RowTableMismatch),
    };
    if key.split('/').any(str::is_empty) {
        return invalid(table, "empty identity component");
    }
    Ok(format!("{ROW_IDENTITY_PREFIX}{table}/{key}"))
}

fn room_id(
    table: &'static str,
    values: &[NativeSqliteValueV1],
) -> Result<Option<String>, NativeSqliteTransferError> {
    let index = match table {
        "activation_intents" => Some(1),
        "room_integrity"
        | "room_members"
        | "timers"
        | "observation_frames"
        | "observation_consequences"
        | "activation_decisions"
        | "activation_operation_receipts"
        | "semantic_receipts"
        | "integrity_incidents" => Some(0),
        "retired_authority_fences_v1"
        | "principals"
        | "runners"
        | "capabilities"
        | "capability_scopes"
        | "runner_capability_memberships"
        | "authority_change_receipts"
        | "authority_audit" => None,
        _ => return Err(NativeSqliteTransferError::RowTableMismatch),
    };
    index
        .map(|index| text(values, index, table).map(str::to_owned))
        .transpose()
}

fn required_room_id(row: &PreparedRow) -> Result<&str, NativeSqliteTransferError> {
    row.room_id
        .as_deref()
        .ok_or(NativeSqliteTransferError::InvalidRow {
            table: row.table,
            what: "Room identity",
        })
}

fn encode_row(
    table: &'static str,
    values: &[NativeSqliteValueV1],
) -> Result<Vec<u8>, NativeSqliteTransferError> {
    let mut output = Vec::new();
    output.extend_from_slice(ROW_MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    put_string(&mut output, table)?;
    put_u32(
        &mut output,
        u32::try_from(values.len()).map_err(|_| NativeSqliteTransferError::InvalidRow {
            table,
            what: "value count",
        })?,
    );
    for value in values {
        match value {
            NativeSqliteValueV1::Null => output.push(0),
            NativeSqliteValueV1::Integer(value) => {
                output.push(1);
                output.extend_from_slice(&value.to_le_bytes());
            }
            NativeSqliteValueV1::Text(value) => {
                output.push(2);
                put_string(&mut output, value)?;
            }
            NativeSqliteValueV1::Blob(value) => {
                output.push(3);
                put_blob(&mut output, value)?;
            }
        }
    }
    Ok(output)
}

fn decode_row(bytes: &[u8]) -> Result<NativeSqliteRowV1, NativeSqliteTransferError> {
    let mut reader = Reader::new(bytes);
    if reader.take(8)? != ROW_MAGIC || reader.u16()? != FORMAT_VERSION {
        return Err(NativeSqliteTransferError::ManifestMismatch {
            what: "row wire version",
        });
    }
    let table = reader.string()?;
    let expected = TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .map(|(_, count)| *count)
        .ok_or(NativeSqliteTransferError::MissingRelation {
            relation: "unknown operational table",
        })?;
    let count = reader.u32()? as usize;
    if count != expected {
        return Err(NativeSqliteTransferError::InvalidRowShape {
            table: table_for_error(&table),
            expected,
            actual: count,
        });
    }
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(match reader.u8()? {
            0 => NativeSqliteValueV1::Null,
            1 => NativeSqliteValueV1::Integer(reader.i64()?),
            2 => NativeSqliteValueV1::Text(reader.string()?),
            3 => NativeSqliteValueV1::Blob(reader.blob()?),
            _ => {
                return Err(NativeSqliteTransferError::ManifestMismatch {
                    what: "row storage class",
                });
            }
        });
    }
    if !reader.done() {
        return Err(NativeSqliteTransferError::ManifestMismatch {
            what: "row trailing bytes",
        });
    }
    Ok(NativeSqliteRowV1 { table, values })
}

fn encode_manifest(summary: &NativeSqliteBundleSummaryV1) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(MANIFEST_MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    put_u32(&mut output, u32::try_from(TABLES.len()).unwrap_or(u32::MAX));
    for (table, _) in TABLES {
        put_string(&mut output, table).unwrap_or_else(|_| unreachable!());
        put_u64(
            &mut output,
            u64::try_from(*summary.table_counts.get(*table).unwrap_or(&0)).unwrap_or(u64::MAX),
        );
    }
    put_u32(
        &mut output,
        u32::try_from(summary.room_policies.len()).unwrap_or(u32::MAX),
    );
    for (room, policy) in &summary.room_policies {
        put_string(&mut output, room).unwrap_or_else(|_| unreachable!());
        output.push(match policy {
            NativeSqliteRoomPolicyV1::Healthy => 1,
            NativeSqliteRoomPolicyV1::IsolatedCorrupt => 2,
        });
    }
    output.extend_from_slice(&summary.row_digest.as_bytes());
    output
}

fn decode_manifest(bytes: &[u8]) -> Result<NativeSqliteBundleSummaryV1, NativeSqliteTransferError> {
    let mut reader = Reader::new(bytes);
    if reader.take(8)? != MANIFEST_MAGIC || reader.u16()? != FORMAT_VERSION {
        return Err(NativeSqliteTransferError::ManifestMismatch {
            what: "manifest wire version",
        });
    }
    if reader.u32()? as usize != TABLES.len() {
        return Err(NativeSqliteTransferError::ManifestMismatch {
            what: "manifest table count",
        });
    }
    let mut table_counts = BTreeMap::new();
    for (expected, _) in TABLES {
        if reader.string()? != *expected {
            return Err(NativeSqliteTransferError::ManifestMismatch {
                what: "manifest table order",
            });
        }
        let count = usize::try_from(reader.u64()?).map_err(|_| {
            NativeSqliteTransferError::ManifestMismatch {
                what: "manifest row count",
            }
        })?;
        table_counts.insert((*expected).to_owned(), count);
    }
    let room_count = reader.u32()? as usize;
    let mut room_policies = BTreeMap::new();
    for _ in 0..room_count {
        let room = reader.string()?;
        let policy = match reader.u8()? {
            1 => NativeSqliteRoomPolicyV1::Healthy,
            2 => NativeSqliteRoomPolicyV1::IsolatedCorrupt,
            _ => {
                return Err(NativeSqliteTransferError::ManifestMismatch {
                    what: "manifest Room policy",
                });
            }
        };
        if room_policies.insert(room, policy).is_some() {
            return Err(NativeSqliteTransferError::ManifestMismatch {
                what: "duplicate manifest Room",
            });
        }
    }
    let row_digest = DigestV1::from_bytes(reader.take(32)?)?;
    if !reader.done() {
        return Err(NativeSqliteTransferError::ManifestMismatch {
            what: "manifest trailing bytes",
        });
    }
    Ok(NativeSqliteBundleSummaryV1 {
        table_counts,
        room_policies,
        row_digest,
    })
}

fn rows_digest(rows: &[PreparedRow]) -> DigestV1 {
    let mut ordered = rows.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.identity.cmp(&right.identity));
    let mut bytes = Vec::new();
    for row in ordered {
        put_u64(
            &mut bytes,
            u64::try_from(row.bytes.len()).unwrap_or(u64::MAX),
        );
        bytes.extend_from_slice(&row.bytes);
    }
    DigestV1::hash(&bytes)
}

fn subject(identity: &str) -> DigestV1 {
    DigestV1::hash(identity.as_bytes())
}

fn text<'a>(
    values: &'a [NativeSqliteValueV1],
    index: usize,
    table: &'static str,
) -> Result<&'a str, NativeSqliteTransferError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) if !value.is_empty() => Ok(value),
        _ => invalid(table, "text value"),
    }
}

fn optional_text<'a>(
    values: &'a [NativeSqliteValueV1],
    index: usize,
    table: &'static str,
) -> Result<Option<&'a str>, NativeSqliteTransferError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Text(value)) => Ok(Some(value)),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => invalid(table, "optional text value"),
    }
}

fn integer(
    values: &[NativeSqliteValueV1],
    index: usize,
    table: &'static str,
) -> Result<i64, NativeSqliteTransferError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Ok(*value),
        _ => invalid(table, "integer value"),
    }
}

fn optional_integer(
    values: &[NativeSqliteValueV1],
    index: usize,
    table: &'static str,
) -> Result<Option<i64>, NativeSqliteTransferError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Integer(value)) => Ok(Some(*value)),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => invalid(table, "optional integer value"),
    }
}

fn blob<'a>(
    values: &'a [NativeSqliteValueV1],
    index: usize,
    table: &'static str,
) -> Result<&'a [u8], NativeSqliteTransferError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Ok(value),
        _ => invalid(table, "blob value"),
    }
}

fn optional_blob<'a>(
    values: &'a [NativeSqliteValueV1],
    index: usize,
    table: &'static str,
) -> Result<Option<&'a [u8]>, NativeSqliteTransferError> {
    match values.get(index) {
        Some(NativeSqliteValueV1::Blob(value)) => Ok(Some(value)),
        Some(NativeSqliteValueV1::Null) => Ok(None),
        _ => invalid(table, "optional blob value"),
    }
}

fn parse_digest(value: &str) -> Result<DigestV1, NativeSqliteTransferError> {
    let Some(value) = value.strip_prefix("blake3:") else {
        return invalid("observation_frames", "digest text");
    };
    if value.len() != 64 {
        return invalid("observation_frames", "digest text");
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_digit(pair[0]).ok_or(NativeSqliteTransferError::InvalidRow {
            table: "observation_frames",
            what: "digest text",
        })?;
        let low = hex_digit(pair[1]).ok_or(NativeSqliteTransferError::InvalidRow {
            table: "observation_frames",
            what: "digest text",
        })?;
        bytes[index] = (high << 4) | low;
    }
    DigestV1::from_bytes(&bytes).map_err(Into::into)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn invalid<T>(table: &'static str, what: &'static str) -> Result<T, NativeSqliteTransferError> {
    Err(NativeSqliteTransferError::InvalidRow { table, what })
}

fn relation<T>(table: &'static str, what: &'static str) -> Result<T, NativeSqliteTransferError> {
    Err(NativeSqliteTransferError::InvalidRelation { table, what })
}

fn table_for_error(table: &str) -> &'static str {
    TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .map_or("native operational row", |(name, _)| name)
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), NativeSqliteTransferError> {
    let length =
        u32::try_from(value.len()).map_err(|_| NativeSqliteTransferError::ManifestMismatch {
            what: "string length",
        })?;
    put_u32(output, length);
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_blob(output: &mut Vec<u8>, value: &[u8]) -> Result<(), NativeSqliteTransferError> {
    put_u64(
        output,
        u64::try_from(value.len()).map_err(|_| NativeSqliteTransferError::ManifestMismatch {
            what: "blob length",
        })?,
    );
    output.extend_from_slice(value);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], NativeSqliteTransferError> {
        let end = self.position.checked_add(length).ok_or(
            NativeSqliteTransferError::ManifestMismatch {
                what: "wire length",
            },
        )?;
        if end > self.bytes.len() {
            return Err(NativeSqliteTransferError::ManifestMismatch {
                what: "truncated wire bytes",
            });
        }
        let result = &self.bytes[self.position..end];
        self.position = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, NativeSqliteTransferError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, NativeSqliteTransferError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(
            |_| NativeSqliteTransferError::ManifestMismatch { what: "u16" },
        )?))
    }

    fn u32(&mut self) -> Result<u32, NativeSqliteTransferError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(
            |_| NativeSqliteTransferError::ManifestMismatch { what: "u32" },
        )?))
    }

    fn u64(&mut self) -> Result<u64, NativeSqliteTransferError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(
            |_| NativeSqliteTransferError::ManifestMismatch { what: "u64" },
        )?))
    }

    fn i64(&mut self) -> Result<i64, NativeSqliteTransferError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().map_err(
            |_| NativeSqliteTransferError::ManifestMismatch { what: "i64" },
        )?))
    }

    fn string(&mut self) -> Result<String, NativeSqliteTransferError> {
        let length = usize::try_from(self.u32()?).map_err(|_| {
            NativeSqliteTransferError::ManifestMismatch {
                what: "string length",
            }
        })?;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| NativeSqliteTransferError::ManifestMismatch {
            what: "UTF-8 string",
        })
    }

    fn blob(&mut self) -> Result<Vec<u8>, NativeSqliteTransferError> {
        let length = usize::try_from(self.u64()?).map_err(|_| {
            NativeSqliteTransferError::ManifestMismatch {
                what: "blob length",
            }
        })?;
        Ok(self.take(length)?.to_vec())
    }

    fn done(&self) -> bool {
        self.position == self.bytes.len()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
#[allow(clippy::too_many_lines)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::BundleProfileV1;
    use worldstream_backup::native_sqlite::NativeSqliteValueV1 as Value;

    fn row(table: &str, values: Vec<Value>) -> NativeSqliteRowV1 {
        NativeSqliteRowV1 {
            table: table.to_owned(),
            values,
        }
    }

    fn healthy_rows() -> NativeSqliteOperationalRowsV1 {
        let payload = b"frame payload".to_vec();
        let frame_hash = format!("blake3:{}", DigestV1::hash(&payload));
        let mut tables = BTreeMap::new();
        tables.insert(
            "room_integrity".to_owned(),
            vec![row(
                "room_integrity",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("healthy".to_owned()),
                    Value::Integer(1),
                ],
            )],
        );
        tables.insert(
            "room_members".to_owned(),
            vec![row(
                "room_members",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("member-1".to_owned()),
                    Value::Text("principal-1".to_owned()),
                    Value::Text("human".to_owned()),
                    Value::Text("enabled".to_owned()),
                    Value::Text("participant".to_owned()),
                    Value::Text("counter".to_owned()),
                    Value::Blob(b"membership".to_vec()),
                    Value::Integer(1),
                    Value::Integer(1),
                    Value::Integer(1),
                    Value::Null,
                    Value::Null,
                ],
            )],
        );
        tables.insert(
            "timers".to_owned(),
            vec![row(
                "timers",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("timer-1".to_owned()),
                    Value::Integer(1),
                    Value::Text("2026-01-01T00:00:00Z".to_owned()),
                    Value::Blob(b"timer".to_vec()),
                    Value::Text("scheduled".to_owned()),
                ],
            )],
        );
        tables.insert(
            "observation_frames".to_owned(),
            vec![row(
                "observation_frames",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("member-1".to_owned()),
                    Value::Integer(1),
                    Value::Integer(1),
                    Value::Text(frame_hash),
                    Value::Blob(payload),
                ],
            )],
        );
        tables.insert(
            "observation_consequences".to_owned(),
            vec![row(
                "observation_consequences",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("member-1".to_owned()),
                    Value::Integer(1),
                    Value::Text("reset_required".to_owned()),
                    Value::Blob(b"reset".to_vec()),
                    Value::Text(DigestV1::hash(b"reset").to_string()),
                ],
            )],
        );
        tables.insert(
            "activation_decisions".to_owned(),
            vec![row(
                "activation_decisions",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Integer(1),
                    Value::Text("decision-1".to_owned()),
                    Value::Text("member-1".to_owned()),
                    Value::Blob(b"decision".to_vec()),
                ],
            )],
        );
        tables.insert(
            "activation_intents".to_owned(),
            vec![row(
                "activation_intents",
                vec![
                    Value::Text("activation-1".to_owned()),
                    Value::Text("room-1".to_owned()),
                    Value::Integer(1),
                    Value::Text("decision-1".to_owned()),
                    Value::Text("member-1".to_owned()),
                    Value::Text("reason".to_owned()),
                    Value::Text("dedupe".to_owned()),
                    Value::Integer(1),
                    Value::Text("2026-01-01T00:00:00Z".to_owned()),
                    Value::Integer(1),
                    Value::Text("pending".to_owned()),
                    Value::Integer(1),
                    Value::Integer(0),
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Null,
                    Value::Integer(0),
                ],
            )],
        );
        tables.insert(
            "activation_operation_receipts".to_owned(),
            vec![row(
                "activation_operation_receipts",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("operation-1".to_owned()),
                    Value::Text("offer".to_owned()),
                    Value::Blob(vec![1; 32]),
                    Value::Text("activation-1".to_owned()),
                    Value::Text("accepted".to_owned()),
                    Value::Blob(b"result".to_vec()),
                    Value::Null,
                    Value::Null,
                ],
            )],
        );
        tables.insert(
            "semantic_receipts".to_owned(),
            vec![row(
                "semantic_receipts",
                vec![
                    Value::Text("room-1".to_owned()),
                    Value::Text("action".to_owned()),
                    Value::Blob(b"identity-1".to_vec()),
                    Value::Text("worldstream/operation-receipt/v1".to_owned()),
                    Value::Blob(vec![2; 32]),
                    Value::Null,
                    Value::Blob(b"input".to_vec()),
                    Value::Blob(b"time".to_vec()),
                    Value::Text("genesis_created".to_owned()),
                    Value::Null,
                    Value::Blob(b"receipt".to_vec()),
                    Value::Text("2026-01-01T00:00:00Z".to_owned()),
                ],
            )],
        );
        for table in [
            "retired_authority_fences_v1",
            "principals",
            "runners",
            "capabilities",
            "capability_scopes",
            "runner_capability_memberships",
            "authority_change_receipts",
            "authority_audit",
            "integrity_incidents",
        ] {
            tables.insert(table.to_owned(), Vec::new());
        }
        NativeSqliteOperationalRowsV1 { tables }
    }

    fn authority_rows() -> NativeSqliteOperationalRowsV1 {
        let mut rows = healthy_rows();
        rows.tables.insert(
            "principals".to_owned(),
            vec![row(
                "principals",
                vec![
                    Value::Text("principal-1".to_owned()),
                    Value::Text("human".to_owned()),
                    Value::Text("enabled".to_owned()),
                    Value::Integer(1),
                ],
            )],
        );
        rows.tables.insert(
            "capabilities".to_owned(),
            vec![row(
                "capabilities",
                vec![
                    Value::Text("capability-1".to_owned()),
                    Value::Blob(vec![7; 32]),
                    Value::Text("principal-1".to_owned()),
                    Value::Text("host_operator".to_owned()),
                    Value::Text("room-1".to_owned()),
                    Value::Null,
                    Value::Null,
                    Value::Integer(1),
                    Value::Null,
                    Value::Null,
                ],
            )],
        );
        rows.tables.insert(
            "authority_change_receipts".to_owned(),
            vec![row(
                "authority_change_receipts",
                vec![
                    Value::Text("change-bootstrap".to_owned()),
                    Value::Null,
                    Value::Blob(vec![8; 32]),
                    Value::Text("authority_bootstrapped".to_owned()),
                    Value::Text("bootstrap".to_owned()),
                    Value::Text("principal-1".to_owned()),
                    Value::Text("capability-1".to_owned()),
                    Value::Integer(1),
                    Value::Text("2026-01-01T00:00:00Z".to_owned()),
                ],
            )],
        );
        rows.tables.insert(
            "authority_audit".to_owned(),
            vec![row(
                "authority_audit",
                vec![
                    Value::Integer(1),
                    Value::Text("change-bootstrap".to_owned()),
                    Value::Null,
                    Value::Text("bootstrap".to_owned()),
                    Value::Text("principal-1".to_owned()),
                    Value::Text("capability-1".to_owned()),
                    Value::Text("bootstrap_authority".to_owned()),
                    Value::Null,
                    Value::Integer(1),
                    Value::Text("2026-01-01T00:00:00Z".to_owned()),
                    Value::Null,
                    Value::Blob(vec![8; 32]),
                ],
            )],
        );
        rows
    }

    fn fixture_spec() -> NativeSqliteTransferSpecV1 {
        let schema = crate::SchemaMigrationContractV1::new(
            "worldstream-storage-v1",
            DigestV1::hash(b"schema"),
            vec![
                crate::MigrationIdentityV1::new(1, "migration-1", DigestV1::hash(b"migration"))
                    .expect("test migration"),
            ],
        )
        .expect("test schema");
        let source = BackendFingerprintV1::new(
            BundleProfileV1::SqliteBundled,
            "sqlite-bundled",
            schema.clone(),
        )
        .expect("test source");
        let target =
            BackendFingerprintV1::new(BundleProfileV1::PostgresPrimary17, "postgresql-17", schema)
                .expect("test target");
        NativeSqliteTransferSpecV1::new(
            "bundle/native",
            "lineage/native",
            7,
            source,
            target,
            PackIdentityV1::new("pack", "r1", DigestV1::hash(b"pack")).expect("test pack"),
            Vec::new(),
            SessionStatePolicyV1::InvalidateAndRebuild,
        )
    }

    fn canonical_records() -> Vec<LogicalRecordV1> {
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
            (
                CanonicalRecordKindV1::ArtifactMetadata,
                "room/room-1/pack-revision-lock",
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(ordinal, (kind, identity))| {
            LogicalRecordV1::canonical(ordinal as u64, kind, identity, identity.as_bytes())
                .expect("canonical fixture")
        })
        .collect()
    }

    #[test]
    fn adapter_preserves_exact_operational_rows_and_manifest_hash() {
        let rows = healthy_rows();
        let bundle = NativeSqliteTransferAdapterV1::from_operational_rows(&rows, &fixture_spec())
            .expect("native bundle");
        let summary =
            NativeSqliteTransferAdapterV1::validate_bundle(&bundle).expect("valid bundle");
        assert_eq!(summary.table_counts().values().sum::<usize>(), 9);
        assert_eq!(
            summary.room_policies()["room-1"],
            NativeSqliteRoomPolicyV1::Healthy
        );
        assert_eq!(
            summary.row_digest(),
            NativeSqliteTransferAdapterV1::validate_bundle(&bundle)
                .expect("repeat")
                .row_digest()
        );
        assert!(bundle.records().iter().any(|record| {
            record.identity().contains("activation-intent")
                || record.identity().contains("activation/")
        }));
    }

    #[test]
    fn adapter_rejects_missing_and_duplicate_or_conflicting_rows() {
        let mut missing = healthy_rows();
        missing.tables.remove("timers");
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&missing),
            Err(NativeSqliteTransferError::MissingRelation { relation: "timers" })
        ));

        let mut duplicate = healthy_rows();
        let timer = duplicate.tables["timers"][0].clone();
        duplicate
            .tables
            .get_mut("timers")
            .expect("timers")
            .push(timer.clone());
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&duplicate),
            Err(NativeSqliteTransferError::DuplicateRow {
                table: "timers",
                ..
            })
        ));

        let mut conflict = healthy_rows();
        let mut timer = conflict.tables["timers"][0].clone();
        timer.values[5] = Value::Text("cancelled".to_owned());
        conflict
            .tables
            .get_mut("timers")
            .expect("timers")
            .push(timer);
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&conflict),
            Err(NativeSqliteTransferError::ConflictingRow {
                table: "timers",
                ..
            })
        ));
    }

    #[test]
    fn authority_receipt_and_audit_are_exactly_relation_bound() {
        NativeSqliteTransferAdapterV1::summary_from_rows(&authority_rows())
            .expect("valid bootstrap receipt/audit pair");

        let mut missing_target = authority_rows();
        missing_target
            .tables
            .get_mut("authority_change_receipts")
            .expect("receipts")[0]
            .values[5] = Value::Text("principal-missing".to_owned());
        missing_target
            .tables
            .get_mut("authority_audit")
            .expect("audit")[0]
            .values[4] = Value::Text("principal-missing".to_owned());
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&missing_target),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_change_receipts",
                what: "authority change target entity"
            })
        ));

        let mut unpaired = authority_rows();
        unpaired
            .tables
            .get_mut("authority_audit")
            .expect("audit")
            .clear();
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&unpaired),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_audit",
                what: "one-to-one authority change Receipt audit pairing"
            })
        ));

        let mut duplicate_audit = authority_rows();
        let mut second = duplicate_audit.tables["authority_audit"][0].clone();
        second.values[0] = Value::Integer(2);
        duplicate_audit
            .tables
            .get_mut("authority_audit")
            .expect("audit")
            .push(second);
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&duplicate_audit),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_audit",
                what: "unique audit for authority change Receipt"
            })
        ));

        let mut mismatched_fields = authority_rows();
        mismatched_fields
            .tables
            .get_mut("authority_audit")
            .expect("audit")[0]
            .values[11] = Value::Blob(vec![9; 32]);
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&mismatched_fields),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_audit",
                what: "authority change Receipt audit field agreement"
            })
        ));

        let mut incoherent_mapping = authority_rows();
        let receipt = &mut incoherent_mapping
            .tables
            .get_mut("authority_change_receipts")
            .expect("receipts")[0]
            .values;
        receipt[1] = Value::Text("principal-1".to_owned());
        receipt[3] = Value::Text("capability_narrowed".to_owned());
        receipt[4] = Value::Text("capability".to_owned());
        receipt[5] = Value::Text("capability-1".to_owned());
        receipt[6] = Value::Null;
        receipt[7] = Value::Integer(2);
        let audit = &mut incoherent_mapping
            .tables
            .get_mut("authority_audit")
            .expect("audit")[0]
            .values;
        audit[2] = Value::Text("principal-1".to_owned());
        audit[3] = Value::Text("capability".to_owned());
        audit[4] = Value::Text("capability-1".to_owned());
        audit[5] = Value::Null;
        audit[6] = Value::Text("register_capability".to_owned());
        audit[7] = Value::Integer(1);
        audit[8] = Value::Integer(2);
        audit[10] = Value::Text("reason".to_owned());
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&incoherent_mapping),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_audit",
                what: "authority change result and audit change mapping"
            })
        ));

        let mut incoherent_target_kind = incoherent_mapping;
        incoherent_target_kind
            .tables
            .get_mut("authority_change_receipts")
            .expect("receipts")[0]
            .values[3] = Value::Text("principal_status_changed".to_owned());
        incoherent_target_kind
            .tables
            .get_mut("authority_audit")
            .expect("audit")[0]
            .values[6] = Value::Text("set_principal_status".to_owned());
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&incoherent_target_kind),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_audit",
                what: "authority change result and audit change mapping"
            })
        ));
    }

    #[test]
    fn authority_bootstrap_capability_must_belong_to_bootstrap_principal() {
        let mut rows = authority_rows();
        rows.tables
            .get_mut("principals")
            .expect("principals")
            .push(row(
                "principals",
                vec![
                    Value::Text("principal-2".to_owned()),
                    Value::Text("human".to_owned()),
                    Value::Text("enabled".to_owned()),
                    Value::Integer(1),
                ],
            ));
        rows.tables.get_mut("capabilities").expect("capabilities")[0].values[2] =
            Value::Text("principal-2".to_owned());
        assert!(matches!(
            NativeSqliteTransferAdapterV1::summary_from_rows(&rows),
            Err(NativeSqliteTransferError::InvalidRelation {
                table: "authority_change_receipts",
                what: "authority change target entity"
            })
        ));
    }

    #[test]
    fn isolated_corrupt_room_is_preserved_without_poisoning_healthy_policy() {
        let mut rows = healthy_rows();
        rows.tables.get_mut("room_integrity").expect("integrity")[0].values[1] =
            Value::Text("quarantined".to_owned());
        rows.tables.get_mut("observation_frames").expect("frames")[0].values[4] =
            Value::Text("not-the-payload-hash".to_owned());
        let summary = NativeSqliteTransferAdapterV1::summary_from_rows(&rows).expect("isolated");
        assert_eq!(
            summary.room_policies()["room-1"],
            NativeSqliteRoomPolicyV1::IsolatedCorrupt
        );
        assert!(summary.has_isolated_room());
    }

    #[test]
    fn adapter_composes_exact_canonical_evidence_without_deriving_it() {
        let rows = healthy_rows();
        let canonical = canonical_records();
        let bundle = NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
            &rows,
            &fixture_spec(),
            &canonical,
        )
        .expect("complete native bundle");

        let summary = NativeSqliteTransferAdapterV1::validate_bundle(&bundle).expect("valid");
        assert_eq!(summary.table_counts().values().sum::<usize>(), 9);
        assert_eq!(bundle.records().len(), canonical.len() + 2 + 9);
        let identity_record = bundle
            .records()
            .iter()
            .find(|record| record.identity() == DEPLOYMENT_IDENTITY)
            .expect("complete deployment identity carried");
        assert_eq!(
            DeploymentIdentityV1::from_canonical_bytes(identity_record.bytes())
                .expect("canonical identity"),
            fixture_spec()
                .deployment_identity
                .expect("fixture identity")
        );
        for source in &canonical {
            let carried = bundle
                .records()
                .iter()
                .find(|record| {
                    record.kind() == source.kind() && record.identity() == source.identity()
                })
                .expect("canonical record carried");
            assert_eq!(carried.bytes(), source.bytes());
            assert_eq!(carried.digest(), source.digest());
        }
    }

    #[test]
    fn adapter_round_trip_preserves_empty_operational_relations() {
        let mut rows = healthy_rows();
        for table in [
            "activation_decisions",
            "activation_intents",
            "activation_operation_receipts",
            "observation_consequences",
            "observation_frames",
            "timers",
        ] {
            rows.tables.get_mut(table).expect("modeled table").clear();
        }
        let bundle = NativeSqliteTransferAdapterV1::from_operational_rows(&rows, &fixture_spec())
            .expect("bundle with empty operational relations");
        let summary = NativeSqliteTransferAdapterV1::validate_bundle(&bundle)
            .expect("empty operational relations remain valid");
        for table in [
            "activation_decisions",
            "activation_intents",
            "activation_operation_receipts",
            "observation_consequences",
            "observation_frames",
            "timers",
        ] {
            assert_eq!(summary.table_counts()[table], 0);
        }
    }

    #[test]
    fn adapter_rejects_incomplete_canonical_room_evidence() {
        let mut canonical = canonical_records();
        canonical.retain(|record| {
            record.kind()
                != crate::RecordKindV1::Canonical(CanonicalRecordKindV1::CoreMaterialization)
        });
        assert!(matches!(
            NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
                &healthy_rows(),
                &fixture_spec(),
                &canonical,
            ),
            Err(NativeSqliteTransferError::MissingCanonicalRecord {
                kind: CanonicalRecordKindV1::CoreMaterialization
            })
        ));
    }

    #[test]
    fn adapter_rejects_canonical_kind_identity_mismatch() {
        let mut canonical = canonical_records();
        canonical[2] = LogicalRecordV1::canonical(
            canonical[2].ordinal(),
            CanonicalRecordKindV1::RoomGenesis,
            "room/room-1/head",
            b"wrong identity",
        )
        .expect("mismatched identity fixture");
        assert!(matches!(
            NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
                &healthy_rows(),
                &fixture_spec(),
                &canonical,
            ),
            Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "canonical identity/kind mismatch"
            })
        ));
    }

    #[test]
    fn isolated_room_carries_exact_canonical_bytes_without_semantic_promotion() {
        let mut rows = healthy_rows();
        rows.tables.get_mut("room_integrity").expect("integrity")[0].values[1] =
            Value::Text("quarantined".to_owned());
        let canonical = canonical_records();
        let bundle = NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
            &rows,
            &fixture_spec(),
            &canonical,
        )
        .expect("isolated operational evidence remains transferable");
        let summary = NativeSqliteTransferAdapterV1::validate_bundle(&bundle).expect("valid");
        assert!(summary.has_isolated_room());
        for source in canonical
            .iter()
            .filter(|record| record.identity().starts_with("room/room-1/"))
        {
            let carried = bundle
                .records()
                .iter()
                .find(|record| {
                    record.kind() == source.kind() && record.identity() == source.identity()
                })
                .expect("isolated canonical bytes carried");
            assert_eq!(carried.bytes(), source.bytes());
            assert_eq!(carried.digest(), source.digest());
        }
    }

    #[test]
    fn adapter_rejects_incomplete_isolated_room_canonical_bytes() {
        let mut rows = healthy_rows();
        rows.tables.get_mut("room_integrity").expect("integrity")[0].values[1] =
            Value::Text("quarantined".to_owned());
        let mut canonical = canonical_records();
        canonical.retain(|record| {
            record.kind() != crate::RecordKindV1::Canonical(CanonicalRecordKindV1::RoomHead)
        });
        assert!(matches!(
            NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
                &rows,
                &fixture_spec(),
                &canonical,
            ),
            Err(NativeSqliteTransferError::MissingCanonicalRecord {
                kind: CanonicalRecordKindV1::RoomHead
            })
        ));
    }

    #[test]
    fn adapter_rejects_derived_records_as_canonical_evidence() {
        let mut canonical = canonical_records();
        canonical.push(
            LogicalRecordV1::derived(
                99,
                crate::DerivedRecordKindV1::PairedSnapshot,
                "room/room-1/snapshot",
                b"derived",
            )
            .expect("derived fixture"),
        );
        assert!(matches!(
            NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
                &healthy_rows(),
                &fixture_spec(),
                &canonical,
            ),
            Err(NativeSqliteTransferError::CanonicalEvidence {
                what: "derived record"
            })
        ));
    }
}
