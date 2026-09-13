//! Keyset-backed source export for the manifest-bearing transfer stream.
//!
//! This module reads only a frozen retained SQLite backup.  It does not use
//! the live writer, construct a TransferBundleV1, or retain the deployment's
//! records in memory.  A source manifest is computed with one bounded scan,
//! then a fresh keyset cursor emits one record at a time for the writer.

use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use rusqlite::{OpenFlags, OptionalExtension, types::ValueRef};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_backup::native_sqlite::{
    NativeSqliteRowV1, NativeSqliteStreamingLimitsV2, NativeSqliteValueV1,
    verify_retained_file_streaming_v2,
};
use worldstream_sqlite_open::{ExactSqliteConnection, open_exact};
use worldstream_transfer::{
    BackendFingerprintV1, BundleProfileV1, CanonicalRecordKindV1, DeploymentIdentityV1, DigestV1,
    ExternalInputPreparationV1, LogicalRecordV1, ResourceIdentityV1, ResourceKindV1, TransferError,
    TransferStreamFooterAccumulatorV2, TransferStreamIdentityV2, TransferStreamManifestV2,
    TransferStreamNativeSummaryV2, TransferStreamRelationCountV2,
    TransferStreamSemanticAccumulatorV2, TransferStreamSemanticExpectationsV2,
    TransferStreamSourceV2, encode_native_sqlite_stream_row_v2,
};

use crate::{SqliteDeploymentIdentityErrorV1, read_deployment_identity};

const STAGE_LINEAGE: u16 = 0;
const STAGE_EPOCH: u16 = 1;
const STAGE_DEPLOYMENT_IDENTITY: u16 = 2;
const STAGE_RESOURCES: u16 = 3;
const STAGE_ROOM_GENESIS: u16 = 4;
const STAGE_TRANSITIONS: u16 = 5;
const STAGE_ROOM_HEADS: u16 = 6;
const STAGE_CORE: u16 = 7;
const STAGE_ACTIVITY: u16 = 8;
const STAGE_PACK_LOCKS: u16 = 9;
const STAGE_EXTERNAL_INPUTS: u16 = 10;
const STAGE_NATIVE: u16 = 11;

// This is the reviewed PostgreSQL publication dependency order. ExternalInput
// preparations are emitted in their canonical representation above rather
// than as an opaque native row.
const NATIVE_STREAM_TABLES: &[&str] = &[
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

/// Persistable keyset position for the frozen SQLite source projection.
///
/// The cursor is deliberately small: a relation index, last SQLite rowid,
/// and the next stream ordinal. Reopening a source at this cursor validates
/// the same frozen backup and resumes without replaying or storing prior rows.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SqliteTransferStreamCursorV2 {
    stage: u16,
    after_rowid: i64,
    next_ordinal: u64,
}

impl Default for SqliteTransferStreamCursorV2 {
    fn default() -> Self {
        Self {
            stage: STAGE_LINEAGE,
            after_rowid: 0,
            next_ordinal: 0,
        }
    }
}

impl SqliteTransferStreamCursorV2 {
    /// Returns the current compact relation stage.
    #[must_use]
    pub const fn stage(&self) -> u16 {
        self.stage
    }

    /// Returns the last native keyset rowid consumed in this stage.
    #[must_use]
    pub const fn after_rowid(&self) -> i64 {
        self.after_rowid
    }

    /// Returns the next exact stream ordinal.
    #[must_use]
    pub const fn next_ordinal(&self) -> u64 {
        self.next_ordinal
    }
}

/// Closed failures from source stream construction or cursor advancement.
#[derive(Debug, Error)]
pub enum SqliteTransferStreamErrorV2 {
    /// The retained backup object could not be admitted and incrementally
    /// verified.
    #[error("SQLite stream backup verification failed")]
    BackupVerification,
    /// The exact retained SQLite object could not be opened read-only.
    #[error("SQLite stream source could not open the retained backup")]
    Open,
    /// A retained source query returned malformed evidence.
    #[error("SQLite stream source evidence is malformed")]
    Corrupt,
    /// A query against the retained source failed.
    #[error("SQLite stream source query failed")]
    Query,
    /// The manifest, stream record, or checksum contract rejected evidence.
    #[error(transparent)]
    Contract(#[from] TransferError),
    /// The frozen source metadata did not match the stream identity.
    #[error("SQLite stream source identity differs from the retained backup")]
    IdentityMismatch,
    /// A caller attempted to export from a noninitial cursor into a fresh
    /// stream file.
    #[error("SQLite stream cursor must begin at ordinal zero for a fresh export")]
    NonInitialCursor,
}

/// A verified, keyset-backed source for a v2 transfer stream.
pub struct SqliteTransferStreamSourceV2 {
    manifest: TransferStreamManifestV2,
    cursor: RecordCursorV2,
}

impl SqliteTransferStreamSourceV2 {
    /// Opens and verifies a frozen backup, computes its manifest in a bounded
    /// pre-scan, and returns a fresh source cursor.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        backup_path: impl AsRef<Path>,
        expected_backup_digest: DigestV1,
        identity: &TransferStreamIdentityV2,
        source_backend: BackendFingerprintV1,
        target_backend: BackendFingerprintV1,
    ) -> Result<Self, SqliteTransferStreamErrorV2> {
        Self::open_at_cursor(
            backup_path,
            expected_backup_digest,
            identity,
            source_backend,
            target_backend,
            SqliteTransferStreamCursorV2::default(),
        )
    }

    /// Reopens the same retained backup at a durable keyset cursor.
    ///
    /// A caller using this for a new stream file must pass the default cursor;
    /// noninitial cursors are intended for source-side resumable transport
    /// sessions that preserve the already-written prefix.
    #[allow(clippy::too_many_arguments)]
    pub fn open_at_cursor(
        backup_path: impl AsRef<Path>,
        expected_backup_digest: DigestV1,
        identity: &TransferStreamIdentityV2,
        source_backend: BackendFingerprintV1,
        target_backend: BackendFingerprintV1,
        cursor: SqliteTransferStreamCursorV2,
    ) -> Result<Self, SqliteTransferStreamErrorV2> {
        let path = admitted_backup_path(backup_path.as_ref())?;
        let file = open_admitted_backup(&path)?;
        let verification = verify_retained_file_streaming_v2(
            &path,
            &file,
            NativeSqliteStreamingLimitsV2::default(),
        )
        .map_err(|_| SqliteTransferStreamErrorV2::BackupVerification)?;
        let actual_backup_digest = DigestV1::from_bytes(&verification.transfer_point_digest())?;
        if actual_backup_digest != expected_backup_digest {
            return Err(SqliteTransferStreamErrorV2::IdentityMismatch);
        }
        if source_backend.profile() != BundleProfileV1::SqliteBundled
            || target_backend.profile() != BundleProfileV1::PostgresPrimary17
        {
            return Err(SqliteTransferStreamErrorV2::IdentityMismatch);
        }
        let mut pre_scan = RecordCursorV2::open(
            file.try_clone()
                .map_err(|_| SqliteTransferStreamErrorV2::Open)?,
            &path,
            SqliteTransferStreamCursorV2::default(),
        )?;
        let (lineage, epoch) = pre_scan.metadata();
        if lineage != identity.lineage_id() || epoch != identity.source_epoch() {
            return Err(SqliteTransferStreamErrorV2::IdentityMismatch);
        }
        let deployment_identity = pre_scan.deployment_identity.clone();
        let room_counts = pre_scan.room_counts()?;
        let mut complete = TransferStreamFooterAccumulatorV2::new(identity, None)?;
        let mut semantic = TransferStreamSemanticAccumulatorV2::new();
        while let Some(record) = pre_scan.next_record()? {
            complete.observe(&record)?;
            semantic.observe(&record)?;
        }
        let relations = verification
            .operational_relation_counts()
            .iter()
            .map(|(relation, rows)| TransferStreamRelationCountV2::new(relation, *rows))
            .collect::<Result<Vec<_>, _>>()?;
        let native_summary = TransferStreamNativeSummaryV2::new(
            relations,
            DigestV1::from_bytes(&verification.operational_row_digest())?,
        )?;
        let expectations = TransferStreamSemanticExpectationsV2::new(
            semantic.canonical_record_count(),
            semantic.native_operational_record_count(),
            room_counts.0,
            room_counts.1,
            room_counts.2,
            semantic.canonical_digest(),
            semantic.native_operational_digest(),
        )?;
        let manifest = TransferStreamManifestV2::new(
            deployment_identity,
            actual_backup_digest,
            source_backend.digest()?,
            target_backend.digest()?,
            native_summary,
            expectations,
            complete.record_count(),
            complete.record_bytes(),
            complete.digest(),
            worldstream_transfer::SessionStatePolicyV1::InvalidateAndRebuild,
        )?;
        let cursor = RecordCursorV2::open(file, &path, cursor)?;
        Ok(Self { manifest, cursor })
    }

    /// Returns the current durable source keyset position.
    #[must_use]
    pub fn cursor(&self) -> &SqliteTransferStreamCursorV2 {
        &self.cursor.cursor
    }

    /// Refuses use of a partial source position for a new stream file.
    pub fn require_initial_cursor(&self) -> Result<(), SqliteTransferStreamErrorV2> {
        if self.cursor.cursor.next_ordinal != 0 {
            return Err(SqliteTransferStreamErrorV2::NonInitialCursor);
        }
        Ok(())
    }
}

impl TransferStreamSourceV2 for SqliteTransferStreamSourceV2 {
    type Error = SqliteTransferStreamErrorV2;

    fn manifest(&self) -> &TransferStreamManifestV2 {
        &self.manifest
    }

    fn next_stream_record(&mut self) -> Result<Option<LogicalRecordV1>, Self::Error> {
        self.cursor.next_record()
    }
}

struct RecordCursorV2 {
    connection: ExactSqliteConnection,
    deployment_identity: DeploymentIdentityV1,
    lineage: String,
    epoch: u64,
    cursor: SqliteTransferStreamCursorV2,
}

impl RecordCursorV2 {
    fn open(
        file: File,
        path: &Path,
        cursor: SqliteTransferStreamCursorV2,
    ) -> Result<Self, SqliteTransferStreamErrorV2> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection =
            open_exact(file, path, flags).map_err(|_| SqliteTransferStreamErrorV2::Open)?;
        connection
            .execute_batch("PRAGMA query_only=ON;")
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let (lineage, epoch): (String, i64) = connection
            .query_row(
                "SELECT deployment_lineage, storage_epoch FROM canonical_export_metadata WHERE metadata_id = 1",
                (),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?;
        let epoch = u64::try_from(epoch)
            .ok()
            .filter(|epoch| *epoch > 0)
            .ok_or(SqliteTransferStreamErrorV2::Corrupt)?;
        let deployment_identity =
            read_deployment_identity(&connection).map_err(map_identity_error)?;
        Ok(Self {
            connection,
            deployment_identity,
            lineage,
            epoch,
            cursor,
        })
    }

    fn metadata(&self) -> (&str, u64) {
        (&self.lineage, self.epoch)
    }

    fn room_counts(&self) -> Result<(u64, u64, u64), SqliteTransferStreamErrorV2> {
        let mut rooms = 0_u64;
        let mut healthy = 0_u64;
        let mut isolated = 0_u64;
        let mut statement = self
            .connection
            .prepare("SELECT status, count(*) FROM room_integrity GROUP BY status")
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let rows = statement
            .query_map((), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        for row in rows {
            let (status, count) = row.map_err(|_| SqliteTransferStreamErrorV2::Query)?;
            let count = u64::try_from(count).map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?;
            rooms = rooms
                .checked_add(count)
                .ok_or(SqliteTransferStreamErrorV2::Corrupt)?;
            match status.as_str() {
                "healthy" => {
                    healthy = healthy
                        .checked_add(count)
                        .ok_or(SqliteTransferStreamErrorV2::Corrupt)?
                }
                "faulted" | "quarantined" => {
                    isolated = isolated
                        .checked_add(count)
                        .ok_or(SqliteTransferStreamErrorV2::Corrupt)?;
                }
                _ => return Err(SqliteTransferStreamErrorV2::Corrupt),
            }
        }
        Ok((rooms, healthy, isolated))
    }

    fn next_record(&mut self) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        loop {
            let stage = self.cursor.stage;
            let found = match stage {
                STAGE_LINEAGE => Some(self.emit(
                    CanonicalRecordKindV1::DeploymentLineage,
                    "deployment/lineage".to_owned(),
                    self.lineage.as_bytes().to_vec(),
                )?),
                STAGE_EPOCH => Some(self.emit(
                    CanonicalRecordKindV1::StorageEpoch,
                    "deployment/epoch".to_owned(),
                    self.epoch.to_string().into_bytes(),
                )?),
                STAGE_DEPLOYMENT_IDENTITY => Some(self.emit(
                    CanonicalRecordKindV1::ArtifactMetadata,
                    "deployment/identity".to_owned(),
                    self.deployment_identity.canonical_bytes()?,
                )?),
                STAGE_RESOURCES => self.next_resource()?,
                STAGE_ROOM_GENESIS => self.next_room_genesis()?,
                STAGE_TRANSITIONS => self.next_transition()?,
                STAGE_ROOM_HEADS => self.next_room_value(
                    CanonicalRecordKindV1::RoomHead,
                    "complete_head_bytes",
                    "head",
                )?,
                STAGE_CORE => self.next_room_value(
                    CanonicalRecordKindV1::CoreMaterialization,
                    "core_state_bytes",
                    "core",
                )?,
                STAGE_ACTIVITY => self.next_room_value(
                    CanonicalRecordKindV1::ActivityMaterialization,
                    "activity_state_bytes",
                    "activity",
                )?,
                STAGE_PACK_LOCKS => self.next_pack_lock()?,
                STAGE_EXTERNAL_INPUTS => self.next_external_input()?,
                value if value >= STAGE_NATIVE => self.next_native(value - STAGE_NATIVE)?,
                _ => return Err(SqliteTransferStreamErrorV2::Corrupt),
            };
            if let Some(record) = found {
                if stage <= STAGE_DEPLOYMENT_IDENTITY {
                    self.advance_stage();
                }
                return Ok(Some(record));
            }
            if stage
                < STAGE_NATIVE
                    + u16::try_from(NATIVE_STREAM_TABLES.len())
                        .map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?
            {
                self.advance_stage();
                continue;
            }
            return Ok(None);
        }
    }

    fn emit(
        &mut self,
        kind: CanonicalRecordKindV1,
        identity: String,
        bytes: Vec<u8>,
    ) -> Result<LogicalRecordV1, SqliteTransferStreamErrorV2> {
        let record = LogicalRecordV1::canonical(self.cursor.next_ordinal, kind, identity, &bytes)?;
        self.cursor.next_ordinal = self
            .cursor
            .next_ordinal
            .checked_add(1)
            .ok_or(SqliteTransferStreamErrorV2::Corrupt)?;
        Ok(record)
    }

    fn advance_stage(&mut self) {
        self.cursor.stage = self.cursor.stage.saturating_add(1);
        self.cursor.after_rowid = 0;
    }

    fn next_resource(&mut self) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let row = self
            .connection
            .query_row(
                "SELECT rowid, resource_kind, resource_identity, resource_bytes, resource_digest FROM deployment_resource_blobs WHERE rowid > ?1 ORDER BY rowid LIMIT 1",
                [self.cursor.after_rowid],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some((rowid, kind, resource_identity, bytes, digest)) = row else {
            return Ok(None);
        };
        let resource = ResourceIdentityV1::from_persisted_parts(
            parse_resource_kind(&kind)?,
            resource_identity.clone(),
            u64::try_from(bytes.len()).map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?,
            DigestV1::from_bytes(&digest)?,
        )?;
        if !self
            .deployment_identity
            .resources()
            .iter()
            .any(|expected| expected == &resource)
        {
            return Err(SqliteTransferStreamErrorV2::Corrupt);
        }
        resource.verify_bytes(&bytes)?;
        self.cursor.after_rowid = rowid;
        Ok(Some(self.emit(
            CanonicalRecordKindV1::ArtifactBytes,
            resource_record_identity(&resource),
            bytes,
        )?))
    }

    fn next_room_genesis(
        &mut self,
    ) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let row = self
            .connection
            .query_row(
                "SELECT rowid, room_id, genesis_bytes FROM room_genesis WHERE rowid > ?1 ORDER BY rowid LIMIT 1",
                [self.cursor.after_rowid],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?)),
            )
            .optional()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some((rowid, room_id, bytes)) = row else {
            return Ok(None);
        };
        self.cursor.after_rowid = rowid;
        Ok(Some(self.emit(
            CanonicalRecordKindV1::RoomGenesis,
            format!("room/{room_id}/genesis"),
            bytes,
        )?))
    }

    fn next_transition(&mut self) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let row = self
            .connection
            .query_row(
                "SELECT rowid, room_id, room_seq, transition_bytes FROM transitions WHERE rowid > ?1 ORDER BY rowid LIMIT 1",
                [self.cursor.after_rowid],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some((rowid, room_id, sequence, bytes)) = row else {
            return Ok(None);
        };
        if sequence <= 0 {
            return Err(SqliteTransferStreamErrorV2::Corrupt);
        }
        self.cursor.after_rowid = rowid;
        Ok(Some(self.emit(
            CanonicalRecordKindV1::RoomTransition,
            format!("room/{room_id}/transition/{sequence}"),
            bytes,
        )?))
    }

    fn next_room_value(
        &mut self,
        kind: CanonicalRecordKindV1,
        column: &str,
        suffix: &str,
    ) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let query = match column {
            "complete_head_bytes" => {
                "SELECT r.rowid, r.room_id, r.complete_head_bytes FROM rooms r WHERE r.rowid > ?1 ORDER BY r.rowid LIMIT 1"
            }
            "core_state_bytes" => {
                "SELECT r.rowid, r.room_id, m.core_state_bytes FROM rooms r JOIN room_materializations m ON m.room_id = r.room_id WHERE r.rowid > ?1 ORDER BY r.rowid LIMIT 1"
            }
            "activity_state_bytes" => {
                "SELECT r.rowid, r.room_id, m.activity_state_bytes FROM rooms r JOIN room_materializations m ON m.room_id = r.room_id WHERE r.rowid > ?1 ORDER BY r.rowid LIMIT 1"
            }
            _ => return Err(SqliteTransferStreamErrorV2::Corrupt),
        };
        let row = self
            .connection
            .query_row(query, [self.cursor.after_rowid], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .optional()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some((rowid, room_id, bytes)) = row else {
            return Ok(None);
        };
        self.cursor.after_rowid = rowid;
        Ok(Some(self.emit(
            kind,
            format!("room/{room_id}/{suffix}"),
            bytes,
        )?))
    }

    fn next_pack_lock(&mut self) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let row = self
            .connection
            .query_row(
                "SELECT rowid, room_id, pack_revision_lock_bytes FROM room_genesis WHERE rowid > ?1 ORDER BY rowid LIMIT 1",
                [self.cursor.after_rowid],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Vec<u8>>(2)?)),
            )
            .optional()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some((rowid, room_id, bytes)) = row else {
            return Ok(None);
        };
        self.cursor.after_rowid = rowid;
        Ok(Some(self.emit(
            CanonicalRecordKindV1::ArtifactMetadata,
            format!("room/{room_id}/pack-revision-lock"),
            bytes,
        )?))
    }

    fn next_external_input(
        &mut self,
    ) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let row = self
            .connection
            .query_row(
                "SELECT rowid, operation_identity_bytes, canonical_request_hash, recorded_at FROM external_input_preparations WHERE rowid > ?1 ORDER BY rowid LIMIT 1",
                [self.cursor.after_rowid],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some((rowid, identity_bytes, request_hash, recorded_at)) = row else {
            return Ok(None);
        };
        let operation = worldstream_core::CanonicalJsonV1::decode_canonical::<
            worldstream_core::OperationIdentityV1,
        >(&identity_bytes)
        .map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?;
        let worldstream_core::OperationIdentityV1::ExternalInput(operation) = operation else {
            return Err(SqliteTransferStreamErrorV2::Corrupt);
        };
        let preparation = ExternalInputPreparationV1::new(
            identity_bytes.clone(),
            DigestV1::from_bytes(&request_hash)?,
            recorded_at,
        )?;
        self.cursor.after_rowid = rowid;
        Ok(Some(self.emit(
            CanonicalRecordKindV1::ExternalInputPreparation,
            format!(
                "room/{}/external-input-preparation/{}",
                operation.room_id,
                hex(&identity_bytes)
            ),
            preparation.canonical_bytes()?,
        )?))
    }

    fn next_native(
        &mut self,
        index: u16,
    ) -> Result<Option<LogicalRecordV1>, SqliteTransferStreamErrorV2> {
        let Some(table) = NATIVE_STREAM_TABLES.get(usize::from(index)) else {
            return Ok(None);
        };
        let quoted = table.replace('"', r#""""#);
        let query =
            format!("SELECT rowid, * FROM \"{quoted}\" WHERE rowid > ?1 ORDER BY rowid LIMIT 1");
        let mut statement = self
            .connection
            .prepare(&query)
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let columns = statement.column_count();
        let mut rows = statement
            .query([self.cursor.after_rowid])
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let Some(row) = rows
            .next()
            .map_err(|_| SqliteTransferStreamErrorV2::Query)?
        else {
            return Ok(None);
        };
        let rowid: i64 = row.get(0).map_err(|_| SqliteTransferStreamErrorV2::Query)?;
        let mut values = Vec::with_capacity(columns.saturating_sub(1));
        for column in 1..columns {
            values.push(native_value(
                row.get_ref(column)
                    .map_err(|_| SqliteTransferStreamErrorV2::Query)?,
            )?);
        }
        self.cursor.after_rowid = rowid;
        let (identity, bytes) = encode_native_sqlite_stream_row_v2(&NativeSqliteRowV1 {
            table: (*table).to_owned(),
            values,
        })
        .map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?;
        drop(rows);
        drop(statement);
        Ok(Some(self.emit(
            CanonicalRecordKindV1::NativeOperationalRow,
            identity,
            bytes,
        )?))
    }
}

fn admitted_backup_path(path: &Path) -> Result<PathBuf, SqliteTransferStreamErrorV2> {
    let metadata = fs::symlink_metadata(path).map_err(|_| SqliteTransferStreamErrorV2::Open)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(SqliteTransferStreamErrorV2::Open);
    }
    path.canonicalize()
        .map_err(|_| SqliteTransferStreamErrorV2::Open)
}

fn open_admitted_backup(path: &Path) -> Result<File, SqliteTransferStreamErrorV2> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
        options.custom_flags(
            i32::try_from(flags.bits()).map_err(|_| SqliteTransferStreamErrorV2::Open)?,
        );
    }
    options
        .open(path)
        .map_err(|_| SqliteTransferStreamErrorV2::Open)
}

fn native_value(value: ValueRef<'_>) -> Result<NativeSqliteValueV1, SqliteTransferStreamErrorV2> {
    match value {
        ValueRef::Null => Ok(NativeSqliteValueV1::Null),
        ValueRef::Integer(value) => Ok(NativeSqliteValueV1::Integer(value)),
        ValueRef::Text(value) => Ok(NativeSqliteValueV1::Text(
            std::str::from_utf8(value)
                .map_err(|_| SqliteTransferStreamErrorV2::Corrupt)?
                .to_owned(),
        )),
        ValueRef::Blob(value) => Ok(NativeSqliteValueV1::Blob(value.to_vec())),
        ValueRef::Real(_) => Err(SqliteTransferStreamErrorV2::Corrupt),
    }
}

fn resource_record_identity(resource: &ResourceIdentityV1) -> String {
    let kind = match resource.kind() {
        ResourceKindV1::Artifact => "artifact",
        ResourceKindV1::Codec => "codec",
        ResourceKindV1::Schema => "schema",
    };
    format!("deployment/resource/{kind}/{}", resource.identity())
}

fn parse_resource_kind(value: &str) -> Result<ResourceKindV1, SqliteTransferStreamErrorV2> {
    match value {
        "artifact" => Ok(ResourceKindV1::Artifact),
        "codec" => Ok(ResourceKindV1::Codec),
        "schema" => Ok(ResourceKindV1::Schema),
        _ => Err(SqliteTransferStreamErrorV2::Corrupt),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn map_identity_error(error: SqliteDeploymentIdentityErrorV1) -> SqliteTransferStreamErrorV2 {
    match error {
        SqliteDeploymentIdentityErrorV1::Invalid(error) => {
            SqliteTransferStreamErrorV2::Contract(error)
        }
        SqliteDeploymentIdentityErrorV1::Conflict | SqliteDeploymentIdentityErrorV1::Corrupt => {
            SqliteTransferStreamErrorV2::Corrupt
        }
        SqliteDeploymentIdentityErrorV1::StorageUnavailable => SqliteTransferStreamErrorV2::Query,
    }
}
