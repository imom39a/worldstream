//! Durable offline SQLite-to-PostgreSQL operator coordination.
//!
//! Serialized sessions are retry hints only. Every command revalidates the
//! exact bundle and source fence; provider operations reconfirm their durable
//! markers before authority can advance.

use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_backup::native_sqlite::{
    NativeSqliteLimits, durable_transfer_point_digest_retained, extract_operational_rows_retained,
    verify_retained_file,
};
use worldstream_core::{CanonicalJsonV1, PackDigestV1, PackRevisionLockV1};
use worldstream_pack_bundle::{PackBundleStoreV1, RetainedPackBundleArtifactV1};
use worldstream_postgres::{
    PostgresAdmin, PostgresStreamDestinationV2, PostgresTransferDestination,
    postgres_backend_fingerprint,
};
#[cfg(windows)]
use worldstream_runtime::create_owner_only_renameable_file;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use worldstream_sqlite::{
    SqliteCanonicalExportV1, SqliteCanonicalRecordKindV1, SqliteRoomStore,
    SqliteSourceTransferStateV1, SqliteSourceTransferStatusV1, SqliteTransferStreamSourceV2,
};
use worldstream_transfer::{
    BackendFingerprintV1, BundleProfileV1, CanonicalRecordKindV1, DigestV1, LogicalRecordV1,
    NativeSqliteTransferAdapterV1, NativeSqliteTransferSpecV1, SessionStatePolicyV1,
    TargetFingerprintV1, TransferBundleV1, TransferImportSessionV1, TransferStateV1,
    TransferStreamCheckpointV2, TransferStreamIdentityV2, TransferStreamLimitsV2,
    TransferStreamReaderV2, TransferStreamSourceV2, abort_stream_whole_deployment_v2,
    abort_whole_deployment, export_stream_v2, finalize_stream_whole_deployment_v2,
    finalize_whole_deployment, import_and_finalize_stream_v2,
};

use crate::operator_storage::{PublicationParent, publish_open_file_noreplace};

const STATE_SCHEMA: &str = "worldstream/operator-transfer-state/v1";
const RESULT_SCHEMA: &str = "worldstream/operator-transfer-result/v1";
const STATE_PREFIX: &str = "transfer-state-v1-";
const OPERATION_LOCK_FILE: &str = "transfer-operation-v1.lock";
const BEGIN_INTENT_FILE: &str = "transfer-begin-intent-v1.json";
const BEGIN_INTENT_SCHEMA: &str = "worldstream/operator-transfer-begin-intent/v1";
const MAX_STATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNK_RECORDS: usize = 1_024;
const MAX_STATE_DIRECTORY_ENTRIES: usize = 100_000;

#[cfg(unix)]
type OperatorFileIdentity = (u64, u64);

#[cfg(windows)]
type OperatorFileIdentity = fs_id::FileID;

/// Redacted output from one transfer operator step.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TransferOperatorResultV1 {
    /// Stable JSON schema.
    pub schema: &'static str,
    /// Successful operator disposition.
    pub status: &'static str,
    /// Explicit operator operation.
    pub operation: &'static str,
    /// Durable local coordinator phase.
    pub phase: &'static str,
    /// Exact deterministic bundle digest.
    pub bundle_hash: String,
    /// Exact expected provider target fingerprint.
    pub target_fingerprint: String,
    /// Frozen `SQLite` storage epoch.
    pub source_epoch: u64,
    /// Next `PostgreSQL` storage epoch.
    pub target_epoch: u64,
    /// Number of bounded logical records in the bundle.
    pub record_count: usize,
    /// First record ordinal not represented by the retry hint.
    pub next_ordinal: usize,
    /// Whether every bundle record has been checkpointed.
    pub chunks_complete: bool,
}

/// Redacted receipt for restoring exact transfer-carried portable archives to
/// one target-local retained-only Pack store.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TransferPackRestoreResultV1 {
    pub schema: &'static str,
    pub status: &'static str,
    pub operation: &'static str,
    pub bundle_hash: String,
    pub restored_bundle_count: usize,
    pub approval_records_imported: bool,
    pub restart_required: bool,
}

/// Redacted receipt for a completed, manifest-bearing v2 source stream.
///
/// The stream remains an offline artifact and does not itself advance target
/// authority. Its header and manifest digests are the durable identifiers used
/// by an importer to resume only this exact frozen source projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TransferStreamExportResultV2 {
    /// Stable response schema.
    pub schema: &'static str,
    /// Successful operator disposition.
    pub status: &'static str,
    /// Explicit source operation.
    pub operation: &'static str,
    /// Authenticated complete stream-header digest.
    pub stream_header_digest: String,
    /// Authenticated manifest digest.
    pub manifest_digest: String,
    /// Source lifecycle's exact frozen backup digest.
    pub backup_digest: String,
    /// Frozen SQLite Storage Epoch.
    pub source_epoch: u64,
    /// Number of records sealed by the stream footer.
    pub record_count: u64,
    /// Total exact payload bytes sealed by the stream footer.
    pub record_bytes: u64,
}

/// Redacted receipt for a v2 authority transition completed from an exact
/// manifest-bearing stream. The source and target are named only by the
/// stream-bound digests and Storage Epochs they durably verified.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TransferStreamAuthorityResultV2 {
    /// Stable response schema.
    pub schema: &'static str,
    /// Successful operator disposition.
    pub status: &'static str,
    /// Explicit authority operation.
    pub operation: &'static str,
    /// Authenticated stream header digest.
    pub stream_header_digest: String,
    /// Authenticated source manifest digest.
    pub manifest_digest: String,
    /// Frozen SQLite source epoch.
    pub source_epoch: u64,
    /// Matching PostgreSQL target epoch.
    pub target_epoch: u64,
}

/// Redacted closed failures from the transfer operator.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TransferOperatorError {
    /// An argument or filesystem layout is outside the reviewed contract.
    #[error("transfer operator argument or artifact layout was rejected: {0}")]
    InvalidArgument(&'static str),
    /// `SQLite` source evidence or its durable authority fence was rejected.
    #[error("transfer operator SQLite source verification failed closed: {0}")]
    Source(&'static str),
    /// The deterministic bundle was absent, corrupt, or mismatched.
    #[error("transfer operator bundle verification failed closed: {0}")]
    Bundle(&'static str),
    /// Immutable retry state was absent, corrupt, or mismatched.
    #[error("transfer operator checkpoint verification failed closed: {0}")]
    State(&'static str),
    /// `PostgreSQL` rejected a provider operation.
    #[error("transfer operator PostgreSQL operation failed closed: {0}")]
    Destination(&'static str),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OperatorPhaseV1 {
    Begun,
    Importing,
    ChunksComplete,
    Finalizing,
    TargetAuthoritative,
    Aborting,
    SourceAuthoritative,
}

impl OperatorPhaseV1 {
    const fn label(self) -> &'static str {
        match self {
            Self::Begun => "begun",
            Self::Importing => "importing",
            Self::ChunksComplete => "chunks_complete",
            Self::Finalizing => "finalizing",
            Self::TargetAuthoritative => "target_authoritative",
            Self::Aborting => "aborting",
            Self::SourceAuthoritative => "source_authoritative",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StateGenerationV1 {
    schema: String,
    sequence: u64,
    phase: OperatorPhaseV1,
    source_path_digest: String,
    backup_digest: String,
    bundle_hash: String,
    target_fingerprint: String,
    record_count: usize,
    next_ordinal: usize,
    session_hex: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BeginIntentV1 {
    schema: String,
    source_path_digest: String,
    source_file_identity_digest: String,
    backup_name_digest: String,
    bundle_name_digest: String,
    transfer_id_digest: String,
}

struct TransferBindingV1 {
    source_path_digest: DigestV1,
    backup_digest: DigestV1,
    bundle_hash: DigestV1,
    target_fingerprint: DigestV1,
}

struct LoadedTransferV1 {
    source: SqliteRoomStore,
    retained_source: RetainedSourceV1,
    backup_path: Option<PathBuf>,
    bundle: TransferBundleV1,
    target: TargetFingerprintV1,
    binding: TransferBindingV1,
    generation: StateGenerationV1,
    session: TransferImportSessionV1,
    state_dir: PathBuf,
}

struct RetainedSourceV1 {
    path: PathBuf,
    file: File,
    identity: OperatorFileIdentity,
}

struct RetainedBackupV1 {
    path: PathBuf,
    file: File,
    identity: OperatorFileIdentity,
}

impl RetainedBackupV1 {
    fn open(
        path: &Path,
        status: &SqliteSourceTransferStatusV1,
    ) -> Result<Self, TransferOperatorError> {
        validate_owner_only_file(path)
            .map_err(|_| TransferOperatorError::Source("owner-only verified backup"))?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;

            let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
            options.custom_flags(
                i32::try_from(flags.bits())
                    .map_err(|_| TransferOperatorError::Source("verified backup flags"))?,
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            options.share_mode(FILE_SHARE_READ);
        }
        let file = options
            .open(path)
            .map_err(|_| TransferOperatorError::Source("open verified backup"))?;
        let identity = operator_file_identity(&file)
            .map_err(|_| TransferOperatorError::Source("verified backup identity"))?;
        let retained = Self {
            path: path.to_owned(),
            file,
            identity,
        };
        retained.require_status_identity(status)?;
        retained.revalidate()?;
        Ok(retained)
    }

    fn require_status_identity(
        &self,
        status: &SqliteSourceTransferStatusV1,
    ) -> Result<(), TransferOperatorError> {
        if !operator_identity_matches_parts(
            self.identity,
            status.backup_storage_id(),
            status.backup_file_id(),
        ) {
            return Err(TransferOperatorError::Source("persisted backup identity"));
        }
        Ok(())
    }

    fn revalidate(&self) -> Result<(), TransferOperatorError> {
        validate_owner_only_file(&self.path)
            .map_err(|_| TransferOperatorError::Source("owner-only verified backup"))?;
        if operator_file_identity(&self.file).ok() != Some(self.identity)
            || operator_path_identity(&self.path).ok() != Some(self.identity)
        {
            return Err(TransferOperatorError::Source(
                "verified backup identity changed",
            ));
        }
        Ok(())
    }
}

impl RetainedSourceV1 {
    fn open(path: &Path) -> Result<Self, TransferOperatorError> {
        validate_owner_only_file(path)
            .map_err(|_| TransferOperatorError::Source("owner-only SQLite source"))?;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;

            let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
            options.custom_flags(
                i32::try_from(flags.bits())
                    .map_err(|_| TransferOperatorError::Source("retained SQLite source flags"))?,
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let file = options
            .open(path)
            .map_err(|_| TransferOperatorError::Source("open retained SQLite source"))?;
        let identity = operator_file_identity(&file)
            .map_err(|_| TransferOperatorError::Source("SQLite source identity"))?;
        let retained = Self {
            path: path.to_owned(),
            file,
            identity,
        };
        retained.revalidate()?;
        Ok(retained)
    }

    fn revalidate(&self) -> Result<(), TransferOperatorError> {
        validate_owner_only_file(&self.path)
            .map_err(|_| TransferOperatorError::Source("owner-only SQLite source"))?;
        if operator_file_identity(self.file()).ok() != Some(self.identity)
            || operator_path_identity(&self.path).ok() != Some(self.identity)
        {
            return Err(TransferOperatorError::Source(
                "SQLite source identity changed",
            ));
        }
        Ok(())
    }

    fn identity_digest(&self) -> DigestV1 {
        operator_identity_digest(self.identity)
    }

    fn open_store(&self) -> Result<SqliteRoomStore, TransferOperatorError> {
        let retained = self
            .file
            .try_clone()
            .map_err(|_| TransferOperatorError::Source("clone retained SQLite source"))?;
        let store = SqliteRoomStore::open_with_retained_file(&self.path, retained)
            .map_err(|_| TransferOperatorError::Source("open controlled source"))?;
        let identity = store.database_identity();
        if !operator_identity_matches_parts(
            self.identity,
            Some(identity.storage_id()),
            Some(identity.file_id()),
        ) {
            return Err(TransferOperatorError::Source(
                "controlled source identity mismatch",
            ));
        }
        Ok(store)
    }

    fn file(&self) -> &File {
        &self.file
    }
}

/// Process-scoped guard for one state directory. The persistent lock file is
/// owner-only; the kernel lock/share denial is released automatically after a
/// process stop, so a crash cannot strand a pathname token.
struct OperationLockV1 {
    directory_path: PathBuf,
    directory_identity: OperatorFileIdentity,
    directory: File,
    path: PathBuf,
    identity: OperatorFileIdentity,
    file: File,
}

impl OperationLockV1 {
    fn acquire(state_dir: &Path) -> Result<Self, TransferOperatorError> {
        let directory = open_exclusive_operation_directory(state_dir)?;
        let directory_identity = operator_file_identity(&directory)
            .map_err(|_| TransferOperatorError::State("operation directory identity"))?;
        validate_operation_directory_identity(&directory, state_dir, directory_identity)?;
        let path = state_dir.join(OPERATION_LOCK_FILE);
        if validate_owner_only_file(&path).is_err() {
            match create_owner_only_file(&path) {
                Ok(file) => {
                    file.sync_all()
                        .map_err(|_| TransferOperatorError::State("sync operation lock"))?;
                    drop(file);
                    sync_directory(state_dir)?;
                }
                Err(_) => validate_owner_only_file(&path)
                    .map_err(|_| TransferOperatorError::State("owner-only operation lock"))?,
            }
        }
        validate_owner_only_file(&path)
            .map_err(|_| TransferOperatorError::State("owner-only operation lock"))?;
        let file = open_exclusive_operation_lock(&path)?;
        validate_lock_identity(&file, &path)?;
        let identity = operator_file_identity(&file)
            .map_err(|_| TransferOperatorError::State("operation lock identity"))?;
        let guard = Self {
            directory_path: state_dir.to_owned(),
            directory_identity,
            directory,
            path,
            identity,
            file,
        };
        guard.revalidate()?;
        Ok(guard)
    }

    fn revalidate(&self) -> Result<(), TransferOperatorError> {
        validate_operation_directory_identity(
            &self.directory,
            &self.directory_path,
            self.directory_identity,
        )?;
        if operator_file_identity(&self.file).ok() != Some(self.identity) {
            return Err(TransferOperatorError::State("operation lock identity"));
        }
        validate_lock_identity(&self.file, &self.path)
    }
}

fn open_exclusive_operation_directory(path: &Path) -> Result<File, TransferOperatorError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        let flags = rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC;
        let flags = i32::try_from(flags.bits())
            .map_err(|_| TransferOperatorError::State("operation directory"))?;
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(path)
            .map_err(|_| TransferOperatorError::State("operation directory"))?;
        rustix::fs::flock(
            &directory,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .map_err(|_| TransferOperatorError::State("operation already in progress"))?;
        return Ok(directory);
    }
    #[cfg(windows)]
    {
        return worldstream_windows_handle::open_pinned_directory(path)
            .map_err(|_| TransferOperatorError::State("operation directory"));
    }
    #[allow(unreachable_code)]
    Err(TransferOperatorError::State("operation directory"))
}

fn validate_operation_directory_identity(
    directory: &File,
    path: &Path,
    expected: OperatorFileIdentity,
) -> Result<(), TransferOperatorError> {
    if operator_file_identity(directory).ok() != Some(expected)
        || operator_directory_path_identity(path).ok() != Some(expected)
    {
        return Err(TransferOperatorError::State("operation directory identity"));
    }
    Ok(())
}

fn open_exclusive_operation_lock(path: &Path) -> Result<File, TransferOperatorError> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(0);
    }
    let file = options
        .open(path)
        .map_err(|_| TransferOperatorError::State("operation already in progress"))?;
    #[cfg(unix)]
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| TransferOperatorError::State("operation already in progress"))?;
    Ok(file)
}

fn validate_lock_identity(file: &File, path: &Path) -> Result<(), TransferOperatorError> {
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::State("owner-only operation lock"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let open = file
            .metadata()
            .map_err(|_| TransferOperatorError::State("operation lock identity"))?;
        let named = fs::symlink_metadata(path)
            .map_err(|_| TransferOperatorError::State("operation lock identity"))?;
        if named.file_type().is_symlink()
            || !named.file_type().is_file()
            || open.dev() != named.dev()
            || open.ino() != named.ino()
        {
            return Err(TransferOperatorError::State("operation lock identity"));
        }
    }
    #[cfg(windows)]
    if fs_id::FileID::new(file).ok() != fs_id::FileID::new(path).ok() {
        return Err(TransferOperatorError::State("operation lock identity"));
    }
    Ok(())
}

/// Freezes `SQLite`, creates its verified backup, and publishes the exact bundle
/// plus the first immutable resumable session generation.
///
/// # Errors
///
/// Returns a closed error when an argument, owner-only artifact, source
/// witness, bundle, or immutable checkpoint does not satisfy the transfer
/// contract.
pub fn begin_transfer(
    sqlite: &Path,
    backup: &Path,
    bundle_path: &Path,
    state_dir: &Path,
    transfer_id: &str,
) -> Result<TransferOperatorResultV1, TransferOperatorError> {
    begin_transfer_with_hooks(
        sqlite,
        backup,
        bundle_path,
        state_dir,
        transfer_id,
        || Ok(()),
        || Ok(()),
    )
}

/// Restores only exact portable Pack archive bytes carried by a verified
/// transfer bundle. Every archive is retained-only; source approval and
/// selectability are never represented by the transfer wire contract.
///
/// # Errors
///
/// Returns a closed error if the transfer artifact, target data directory, or
/// any staged Pack archive fails complete verification.
pub fn restore_transfer_pack_bundles(
    bundle_path: &Path,
    target_data_dir: &Path,
) -> Result<TransferPackRestoreResultV1, TransferOperatorError> {
    let bundle = read_canonical_bundle(bundle_path)?;
    let bundle_hash = bundle
        .bundle_hash()
        .map_err(|_| TransferOperatorError::Bundle("bundle digest"))?;
    let target_data_dir = prepare_data_directory(target_data_dir)
        .map_err(|_| TransferOperatorError::State("target data directory"))?;
    let store = PackBundleStoreV1::open(target_data_dir.join("activity-packs"))
        .map_err(|_| TransferOperatorError::Bundle("target portable Pack store"))?;
    let restored_at = format!("transfer-restore:{bundle_hash}");
    for artifact in bundle.portable_pack_bundles() {
        store
            .restore_retained(artifact, restored_at.clone())
            .map_err(|_| TransferOperatorError::Bundle("target portable Pack restore"))?;
    }
    Ok(TransferPackRestoreResultV1 {
        schema: "worldstream/transfer-pack-restore-result/v1",
        status: "ok",
        operation: "restore_pack_bundles",
        bundle_hash: bundle_hash.to_string(),
        restored_bundle_count: bundle.portable_pack_bundles().len(),
        approval_records_imported: false,
        restart_required: true,
    })
}

#[cfg(test)]
fn begin_transfer_with_hook<F>(
    sqlite: &Path,
    backup: &Path,
    bundle_path: &Path,
    state_dir: &Path,
    transfer_id: &str,
    after_source_pending: F,
) -> Result<TransferOperatorResultV1, TransferOperatorError>
where
    F: FnOnce() -> Result<(), TransferOperatorError>,
{
    begin_transfer_with_hooks(
        sqlite,
        backup,
        bundle_path,
        state_dir,
        transfer_id,
        || Ok(()),
        after_source_pending,
    )
}

#[allow(clippy::too_many_arguments)]
fn begin_transfer_with_hooks<F, G>(
    sqlite: &Path,
    backup: &Path,
    bundle_path: &Path,
    state_dir: &Path,
    transfer_id: &str,
    before_source_open: F,
    after_source_pending: G,
) -> Result<TransferOperatorResultV1, TransferOperatorError>
where
    F: FnOnce() -> Result<(), TransferOperatorError>,
    G: FnOnce() -> Result<(), TransferOperatorError>,
{
    if transfer_id.is_empty() || transfer_id.len() > 256 {
        return Err(TransferOperatorError::InvalidArgument(
            "transfer identifier",
        ));
    }
    let state_dir = prepare_data_directory(state_dir)
        .map_err(|_| TransferOperatorError::State("owner-only state directory"))?;
    let operation_lock = OperationLockV1::acquire(&state_dir)?;
    let backup = artifact_path_in_state_dir(backup, &state_dir)?;
    let bundle_path = artifact_path_in_state_dir(bundle_path, &state_dir)?;
    let source_path = canonical_regular_file(sqlite, "SQLite source")?;
    let retained_source = RetainedSourceV1::open(&source_path)?;
    let source_path_digest = path_digest(&source_path);
    let intent = BeginIntentV1 {
        schema: BEGIN_INTENT_SCHEMA.to_owned(),
        source_path_digest: source_path_digest.to_string(),
        source_file_identity_digest: retained_source.identity_digest().to_string(),
        backup_name_digest: artifact_name_digest(&backup)?.to_string(),
        bundle_name_digest: artifact_name_digest(&bundle_path)?.to_string(),
        transfer_id_digest: DigestV1::hash(transfer_id.as_bytes()).to_string(),
    };
    operation_lock.revalidate()?;
    let intent_created = publish_or_confirm_begin_intent(&state_dir, &intent)?;
    validate_state_directory_inventory(&state_dir, &intent)?;
    retained_source.revalidate()?;
    before_source_open()?;
    operation_lock.revalidate()?;
    retained_source.revalidate()?;
    let source = retained_source.open_store()?;
    retained_source.revalidate()?;

    let status = source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("read authority fence"))?;
    retained_source.revalidate()?;
    operation_lock.revalidate()?;
    let status =
        match status.state() {
            SqliteSourceTransferStateV1::SourceAuthoritative => source
                .begin_source_transfer(&backup)
                .map_err(|_| TransferOperatorError::Source("begin verified transfer point"))?,
            SqliteSourceTransferStateV1::TransferPending => {
                if intent_created || status.backup_path() != Some(backup.as_path()) {
                    return Err(TransferOperatorError::Source("pending backup identity"));
                }
                status
            }
            SqliteSourceTransferStateV1::SourceRetired => {
                return Err(TransferOperatorError::Source("source is retired"));
            }
        };
    retained_source.revalidate()?;
    after_source_pending()?;
    operation_lock.revalidate()?;
    retained_source.revalidate()?;
    let backup_digest = status
        .backup_digest()
        .ok_or(TransferOperatorError::Source("backup digest"))?;
    let pack_store_root = source_path
        .parent()
        .ok_or(TransferOperatorError::Source("SQLite source parent"))?
        .join("activity-packs");
    let bundle = build_frozen_bundle(&source, &backup, &status, transfer_id, &pack_store_root)?;
    retained_source.revalidate()?;
    let target = TargetFingerprintV1::for_bundle(&bundle)
        .map_err(|_| TransferOperatorError::Bundle("target fingerprint"))?;
    let bundle_hash = bundle
        .bundle_hash()
        .map_err(|_| TransferOperatorError::Bundle("bundle digest"))?;
    let target_fingerprint = target
        .fingerprint_digest()
        .map_err(|_| TransferOperatorError::Bundle("target digest"))?;
    let binding = TransferBindingV1 {
        source_path_digest,
        backup_digest,
        bundle_hash,
        target_fingerprint,
    };

    operation_lock.revalidate()?;
    publish_or_confirm_bundle(&bundle_path, &bundle)?;
    if let Some((generation, session)) =
        load_latest_generation(&state_dir, &bundle, &target, &binding)?
    {
        return operator_result("begin", generation.phase, &bundle, &target, &session);
    }

    let mut session = TransferImportSessionV1::begin(&bundle)
        .map_err(|_| TransferOperatorError::State("initial session"))?;
    session
        .verify_target(&target)
        .map_err(|_| TransferOperatorError::State("initial target fence"))?;
    operation_lock.revalidate()?;
    persist_generation(
        &state_dir,
        None,
        OperatorPhaseV1::Begun,
        &binding,
        &bundle,
        &session,
    )?;
    operator_result("begin", OperatorPhaseV1::Begun, &bundle, &target, &session)
}

/// Applies one bounded chunk and persists immediately after its provider
/// commit. The target remains fenced and `SQLite` remains transfer-pending, so
/// each invocation is an explicit restart/recovery boundary.
///
/// # Errors
///
/// Returns a closed error when the source or retry state no longer matches the
/// admitted bundle, or when the destination cannot reconfirm and commit the
/// bounded chunk.
pub fn resume_transfer(
    sqlite: &Path,
    bundle_path: &Path,
    state_dir: &Path,
    admin: &PostgresAdmin,
    chunk_records: usize,
) -> Result<TransferOperatorResultV1, TransferOperatorError> {
    if !(1..=MAX_CHUNK_RECORDS).contains(&chunk_records) {
        return Err(TransferOperatorError::InvalidArgument("chunk record bound"));
    }
    let prepared_state_dir = prepare_data_directory(state_dir)
        .map_err(|_| TransferOperatorError::State("owner-only state directory"))?;
    let operation_lock = OperationLockV1::acquire(&prepared_state_dir)?;
    admin
        .verify_schema()
        .map_err(|_| TransferOperatorError::Destination("schema verification"))?;
    let mut loaded = load_transfer(sqlite, bundle_path, state_dir)?;
    require_resumable_phase(loaded.generation.phase)?;
    revalidate_pending_source(&loaded)?;
    operation_lock.revalidate()?;
    let mut destination =
        PostgresTransferDestination::new(admin, &loaded.bundle, loaded.target.clone())
            .map_err(|_| TransferOperatorError::Destination("bind exact target"))?;
    if loaded.session.next_ordinal() < loaded.bundle.records().len() {
        let start = loaded.session.next_ordinal();
        let end = start
            .saturating_add(chunk_records)
            .min(loaded.bundle.records().len());
        let chunk = loaded
            .bundle
            .chunk(start, end)
            .map_err(|_| TransferOperatorError::Bundle("bounded chunk"))?;
        loaded
            .session
            .apply_chunk(&mut destination, &chunk)
            .map_err(|_| TransferOperatorError::Destination("apply or reconfirm chunk"))?;
        let phase = if loaded.session.is_complete() {
            OperatorPhaseV1::ChunksComplete
        } else {
            OperatorPhaseV1::Importing
        };
        operation_lock.revalidate()?;
        loaded.generation = persist_generation(
            &loaded.state_dir,
            Some(loaded.generation.sequence),
            phase,
            &loaded.binding,
            &loaded.bundle,
            &loaded.session,
        )?;
    }
    loaded.retained_source.revalidate()?;
    let phase = if loaded.session.is_complete() {
        OperatorPhaseV1::ChunksComplete
    } else {
        loaded.generation.phase
    };
    operator_result(
        "resume",
        phase,
        &loaded.bundle,
        &loaded.target,
        &loaded.session,
    )
}

fn require_resumable_phase(phase: OperatorPhaseV1) -> Result<(), TransferOperatorError> {
    if matches!(
        phase,
        OperatorPhaseV1::TargetAuthoritative | OperatorPhaseV1::SourceAuthoritative
    ) {
        return Err(TransferOperatorError::State(
            "terminal phase requires explicit finalize or abort reconfirmation",
        ));
    }
    Ok(())
}

/// Performs the explicit whole-deployment authority handoff. A durable
/// pre-handoff generation is fsynced before provider/source coordination and a
/// target-authoritative generation is fsynced immediately after it returns.
///
/// # Errors
///
/// Returns a closed error unless every chunk, source witness, provider
/// witness, and durable authority transition can be reconfirmed exactly.
pub fn finalize_transfer(
    sqlite: &Path,
    bundle_path: &Path,
    state_dir: &Path,
    admin: &PostgresAdmin,
) -> Result<TransferOperatorResultV1, TransferOperatorError> {
    let prepared_state_dir = prepare_data_directory(state_dir)
        .map_err(|_| TransferOperatorError::State("owner-only state directory"))?;
    let operation_lock = OperationLockV1::acquire(&prepared_state_dir)?;
    admin
        .verify_schema()
        .map_err(|_| TransferOperatorError::Destination("schema verification"))?;
    let mut loaded = load_transfer(sqlite, bundle_path, state_dir)?;
    revalidate_finalization_source(&loaded)?;
    if !loaded.session.is_complete() {
        return Err(TransferOperatorError::State("chunks are incomplete"));
    }
    operation_lock.revalidate()?;
    let mut destination =
        PostgresTransferDestination::new(admin, &loaded.bundle, loaded.target.clone())
            .map_err(|_| TransferOperatorError::Destination("bind exact target"))?;
    finalize_loaded(&mut loaded, &mut destination, || Ok(()))
}

fn finalize_loaded<D, F>(
    loaded: &mut LoadedTransferV1,
    destination: &mut D,
    after_authority_handoff: F,
) -> Result<TransferOperatorResultV1, TransferOperatorError>
where
    D: worldstream_transfer::TransferDestinationV1,
    F: FnOnce() -> Result<(), TransferOperatorError>,
{
    loaded.generation = persist_generation(
        &loaded.state_dir,
        Some(loaded.generation.sequence),
        OperatorPhaseV1::Finalizing,
        &loaded.binding,
        &loaded.bundle,
        &loaded.session,
    )?;
    loaded.retained_source.revalidate()?;
    finalize_whole_deployment(
        &mut loaded.session,
        destination,
        &loaded.source,
        &loaded.bundle,
    )
    .map_err(|_| TransferOperatorError::Destination("whole-deployment finalization"))?;
    loaded.retained_source.revalidate()?;
    after_authority_handoff()?;
    loaded.retained_source.revalidate()?;
    loaded.generation = persist_generation(
        &loaded.state_dir,
        Some(loaded.generation.sequence),
        OperatorPhaseV1::TargetAuthoritative,
        &loaded.binding,
        &loaded.bundle,
        &loaded.session,
    )?;
    loaded.retained_source.revalidate()?;
    operator_result(
        "finalize",
        loaded.generation.phase,
        &loaded.bundle,
        &loaded.target,
        &loaded.session,
    )
}

/// Tombstones/discards the exact `PostgreSQL` import before restoring `SQLite`
/// authority. The pre-abort session is retained for idempotent replay because
/// source-authoritative import sessions are intentionally not deserializable.
///
/// # Errors
///
/// Returns a closed error if the exact incomplete provider import cannot be
/// discarded and reconfirmed before the matching source authority is restored.
pub fn abort_transfer(
    sqlite: &Path,
    bundle_path: &Path,
    state_dir: &Path,
    admin: &PostgresAdmin,
) -> Result<TransferOperatorResultV1, TransferOperatorError> {
    let prepared_state_dir = prepare_data_directory(state_dir)
        .map_err(|_| TransferOperatorError::State("owner-only state directory"))?;
    let operation_lock = OperationLockV1::acquire(&prepared_state_dir)?;
    admin
        .verify_schema()
        .map_err(|_| TransferOperatorError::Destination("schema verification"))?;
    let mut loaded = load_transfer(sqlite, bundle_path, state_dir)?;
    if loaded.generation.phase == OperatorPhaseV1::TargetAuthoritative
        || loaded.session.state() == TransferStateV1::TargetAuthoritative
    {
        return Err(TransferOperatorError::State("target is authoritative"));
    }
    revalidate_abort_source(&loaded)?;
    operation_lock.revalidate()?;
    let mut destination =
        PostgresTransferDestination::new(admin, &loaded.bundle, loaded.target.clone())
            .map_err(|_| TransferOperatorError::Destination("bind exact target"))?;
    abort_loaded(&mut loaded, &mut destination, || Ok(()))
}

fn abort_loaded<D, F>(
    loaded: &mut LoadedTransferV1,
    destination: &mut D,
    after_source_restored: F,
) -> Result<TransferOperatorResultV1, TransferOperatorError>
where
    D: worldstream_transfer::TransferDestinationV1,
    F: FnOnce() -> Result<(), TransferOperatorError>,
{
    loaded.generation = persist_generation(
        &loaded.state_dir,
        Some(loaded.generation.sequence),
        OperatorPhaseV1::Aborting,
        &loaded.binding,
        &loaded.bundle,
        &loaded.session,
    )?;
    let pre_abort_bytes = loaded
        .session
        .to_bytes()
        .map_err(|_| TransferOperatorError::State("serialize pre-abort session"))?;
    loaded.retained_source.revalidate()?;
    abort_whole_deployment(
        &mut loaded.session,
        destination,
        &loaded.source,
        &loaded.bundle,
    )
    .map_err(|_| TransferOperatorError::Destination("whole-deployment abort"))?;
    loaded.retained_source.revalidate()?;
    after_source_restored()?;
    loaded.retained_source.revalidate()?;
    let retry_session = TransferImportSessionV1::from_bytes(&loaded.bundle, &pre_abort_bytes)
        .map_err(|_| TransferOperatorError::State("restore pre-abort session"))?;
    loaded.generation = persist_generation(
        &loaded.state_dir,
        Some(loaded.generation.sequence),
        OperatorPhaseV1::SourceAuthoritative,
        &loaded.binding,
        &loaded.bundle,
        &retry_session,
    )?;
    loaded.retained_source.revalidate()?;
    operator_result(
        "abort",
        loaded.generation.phase,
        &loaded.bundle,
        &loaded.target,
        &retry_session,
    )
}

fn load_transfer(
    sqlite: &Path,
    bundle_path: &Path,
    state_dir: &Path,
) -> Result<LoadedTransferV1, TransferOperatorError> {
    let state_dir = prepare_data_directory(state_dir)
        .map_err(|_| TransferOperatorError::State("owner-only state directory"))?;
    let bundle_path = artifact_path_in_state_dir(bundle_path, &state_dir)?;
    let intent = load_begin_intent(&state_dir)?;
    validate_state_directory_inventory(&state_dir, &intent)?;
    validate_owner_only_file(&bundle_path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only bundle"))?;
    let bundle = read_canonical_bundle(&bundle_path)?;
    NativeSqliteTransferAdapterV1::validate_bundle(&bundle)
        .map_err(|_| TransferOperatorError::Bundle("native operational evidence"))?;
    let target = TargetFingerprintV1::for_bundle(&bundle)
        .map_err(|_| TransferOperatorError::Bundle("target fingerprint"))?;
    let source_path = canonical_regular_file(sqlite, "SQLite source")?;
    let retained_source = RetainedSourceV1::open(&source_path)?;
    let source = retained_source.open_store()?;
    retained_source.revalidate()?;
    let source_path_digest = path_digest(&source_path);
    if intent.source_path_digest != source_path_digest.to_string()
        || intent.source_file_identity_digest != retained_source.identity_digest().to_string()
        || intent.bundle_name_digest != artifact_name_digest(&bundle_path)?.to_string()
        || intent.transfer_id_digest != DigestV1::hash(bundle.bundle_id().as_bytes()).to_string()
    {
        return Err(TransferOperatorError::State("begin intent binding"));
    }
    let status = source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("read authority fence"))?;
    retained_source.revalidate()?;
    let backup_path = status
        .backup_path()
        .map(|path| {
            let resolved = artifact_path_in_state_dir(path, &state_dir)
                .map_err(|_| TransferOperatorError::State("begin backup intent binding"))?;
            if path != resolved
                || artifact_name_digest(&resolved)?.to_string() != intent.backup_name_digest
            {
                return Err(TransferOperatorError::State("begin backup intent binding"));
            }
            Ok(resolved)
        })
        .transpose()?;
    let bundle_hash = bundle
        .bundle_hash()
        .map_err(|_| TransferOperatorError::Bundle("bundle digest"))?;
    let target_fingerprint = target
        .fingerprint_digest()
        .map_err(|_| TransferOperatorError::Bundle("target digest"))?;
    let (generation, session) = load_latest_generation_without_backup(
        &state_dir,
        &bundle,
        &target,
        source_path_digest,
        bundle_hash,
        target_fingerprint,
    )?
    .ok_or(TransferOperatorError::State("checkpoint generation absent"))?;
    let backup_digest = decode_digest(&generation.backup_digest)?;
    let binding = TransferBindingV1 {
        source_path_digest,
        backup_digest,
        bundle_hash,
        target_fingerprint,
    };
    Ok(LoadedTransferV1 {
        source,
        retained_source,
        backup_path,
        bundle,
        target,
        binding,
        generation,
        session,
        state_dir,
    })
}

fn revalidate_pending_source(loaded: &LoadedTransferV1) -> Result<(), TransferOperatorError> {
    loaded.retained_source.revalidate()?;
    let status = loaded
        .source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("read authority fence"))?;
    if status.state() != SqliteSourceTransferStateV1::TransferPending
        || status.source_epoch() != Some(loaded.bundle.source_epoch())
        || status.target_epoch() != Some(loaded.target.storage_epoch())
        || status.backup_digest() != Some(loaded.binding.backup_digest)
    {
        return Err(TransferOperatorError::Source("pending authority binding"));
    }
    let backup = revalidate_bound_backup_path(loaded, &status, "pending backup path")?;
    validate_owner_only_file(&backup)
        .map_err(|_| TransferOperatorError::Source("owner-only pending backup"))?;
    let pack_store_root = loaded
        .retained_source
        .path
        .parent()
        .ok_or(TransferOperatorError::Source("SQLite source parent"))?
        .join("activity-packs");
    let rebuilt = build_frozen_bundle(
        &loaded.source,
        &backup,
        &status,
        loaded.bundle.bundle_id(),
        &pack_store_root,
    )?;
    if rebuilt
        .bundle_hash()
        .map_err(|_| TransferOperatorError::Bundle("rebuilt digest"))?
        != loaded.binding.bundle_hash
    {
        return Err(TransferOperatorError::Bundle("frozen source mismatch"));
    }
    loaded.retained_source.revalidate()
}

fn revalidate_abort_source(loaded: &LoadedTransferV1) -> Result<(), TransferOperatorError> {
    loaded.retained_source.revalidate()?;
    let status = loaded
        .source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("read authority fence"))?;
    let result = match status.state() {
        SqliteSourceTransferStateV1::TransferPending => revalidate_pending_source(loaded),
        SqliteSourceTransferStateV1::SourceAuthoritative => {
            if status.last_aborted_bundle_hash() != Some(loaded.binding.bundle_hash)
                || status.last_aborted_target_fingerprint()
                    != Some(loaded.binding.target_fingerprint)
            {
                return Err(TransferOperatorError::Source("aborted authority binding"));
            }
            Ok(())
        }
        SqliteSourceTransferStateV1::SourceRetired => {
            Err(TransferOperatorError::Source("source is retired"))
        }
    };
    result?;
    loaded.retained_source.revalidate()
}

fn revalidate_finalization_source(loaded: &LoadedTransferV1) -> Result<(), TransferOperatorError> {
    loaded.retained_source.revalidate()?;
    let status = loaded
        .source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("read authority fence"))?;
    let result = match status.state() {
        SqliteSourceTransferStateV1::TransferPending => revalidate_pending_source(loaded),
        SqliteSourceTransferStateV1::SourceRetired => {
            if status.source_epoch() != Some(loaded.bundle.source_epoch())
                || status.target_epoch() != Some(loaded.target.storage_epoch())
                || status.backup_digest() != Some(loaded.binding.backup_digest)
                || status.bundle_hash() != Some(loaded.binding.bundle_hash)
                || status.target_fingerprint() != Some(loaded.binding.target_fingerprint)
            {
                return Err(TransferOperatorError::Source("retired authority binding"));
            }
            let backup = revalidate_bound_backup_path(loaded, &status, "retired backup path")?;
            let retained = RetainedBackupV1::open(&backup, &status)?;
            let report = verify_retained_file(
                &retained.path,
                &retained.file,
                NativeSqliteLimits::default(),
            )
            .map_err(|_| TransferOperatorError::Source("retired backup verifier"))?;
            retained.revalidate()?;
            if !report.canonical_ready || report.diagnostics.iter().any(|item| item.blocking) {
                return Err(TransferOperatorError::Source("retired backup verifier"));
            }
            Ok(())
        }
        SqliteSourceTransferStateV1::SourceAuthoritative => Err(TransferOperatorError::Source(
            "finalization source authority",
        )),
    };
    result?;
    loaded.retained_source.revalidate()
}

fn revalidate_bound_backup_path(
    loaded: &LoadedTransferV1,
    status: &worldstream_sqlite::SqliteSourceTransferStatusV1,
    label: &'static str,
) -> Result<PathBuf, TransferOperatorError> {
    let path = status
        .backup_path()
        .ok_or(TransferOperatorError::Source(label))?;
    let resolved = artifact_path_in_state_dir(path, &loaded.state_dir)
        .map_err(|_| TransferOperatorError::Source(label))?;
    if path != resolved || loaded.backup_path.as_deref() != Some(resolved.as_path()) {
        return Err(TransferOperatorError::Source(label));
    }
    Ok(resolved)
}

fn build_frozen_bundle(
    source: &SqliteRoomStore,
    backup: &Path,
    status: &SqliteSourceTransferStatusV1,
    transfer_id: &str,
    pack_store_root: &Path,
) -> Result<TransferBundleV1, TransferOperatorError> {
    let retained = RetainedBackupV1::open(backup, status)?;
    let report = verify_retained_file(
        &retained.path,
        &retained.file,
        NativeSqliteLimits::default(),
    )
    .map_err(|_| TransferOperatorError::Source("complete backup verifier"))?;
    retained.revalidate()?;
    if !report.canonical_ready || report.diagnostics.iter().any(|item| item.blocking) {
        return Err(TransferOperatorError::Source("complete backup verifier"));
    }
    let recomputed_digest = durable_transfer_point_digest_retained(
        &retained.path,
        &retained.file,
        NativeSqliteLimits::default(),
    )
    .map_err(|_| TransferOperatorError::Source("standalone backup digest"))?;
    retained.revalidate()?;
    let recomputed_digest = DigestV1::from_bytes(&recomputed_digest)
        .map_err(|_| TransferOperatorError::Source("standalone backup digest"))?;
    if status.backup_digest() != Some(recomputed_digest) {
        return Err(TransferOperatorError::Source(
            "persisted backup digest mismatch",
        ));
    }
    let rows = extract_operational_rows_retained(
        &retained.path,
        &retained.file,
        NativeSqliteLimits::default(),
    )
    .map_err(|_| TransferOperatorError::Source("bounded operational export"))?;
    retained.revalidate()?;
    let export = source
        .export_canonical_evidence()
        .map_err(|_| TransferOperatorError::Source("verified frozen canonical export"))?;
    let canonical_records = canonical_records(&export)?;
    let target_backend = postgres_backend_fingerprint()
        .map_err(|_| TransferOperatorError::Bundle("PostgreSQL backend fingerprint"))?;
    let source_backend = BackendFingerprintV1::new(
        BundleProfileV1::SqliteBundled,
        "sqlite-bundled",
        target_backend.schema().clone(),
    )
    .map_err(|_| TransferOperatorError::Bundle("SQLite backend fingerprint"))?;
    let portable_pack_bundles = retained_portable_pack_bundles(&export, pack_store_root)?;
    let spec = NativeSqliteTransferSpecV1::new_with_deployment_resources(
        transfer_id,
        export.deployment_lineage(),
        export.storage_epoch(),
        source_backend,
        target_backend,
        export.deployment_identity().clone(),
        export.resource_payloads().to_vec(),
        SessionStatePolicyV1::InvalidateAndRebuild,
    )
    .with_portable_pack_bundles(portable_pack_bundles);
    NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
        &rows,
        &spec,
        &canonical_records,
    )
    .map_err(|_| TransferOperatorError::Bundle("construct native transfer bundle"))
}

fn retained_portable_pack_bundles(
    export: &SqliteCanonicalExportV1,
    store_root: &Path,
) -> Result<Vec<RetainedPackBundleArtifactV1>, TransferOperatorError> {
    let base_revisions = export
        .deployment_identity()
        .packs()
        .iter()
        .map(|pack| format!("blake3:{}", pack.digest()))
        .collect::<std::collections::BTreeSet<_>>();
    let mut referenced = std::collections::BTreeSet::<PackDigestV1>::new();
    for record in export
        .records()
        .iter()
        .filter(|record| record.kind() == SqliteCanonicalRecordKindV1::PackRevisionLock)
    {
        let lock: PackRevisionLockV1 = CanonicalJsonV1::decode_canonical(record.bytes())
            .map_err(|_| TransferOperatorError::Bundle("retained Pack revision lock"))?;
        let digest = lock
            .revision_digest()
            .map_err(|_| TransferOperatorError::Bundle("retained Pack revision digest"))?;
        if !base_revisions.contains(&digest.to_string()) {
            referenced.insert(digest);
        }
    }
    if referenced.is_empty() {
        return Ok(Vec::new());
    }
    if !store_root.is_dir() {
        return Err(TransferOperatorError::Bundle(
            "referenced portable Pack store",
        ));
    }
    let store = PackBundleStoreV1::open(store_root.to_owned())
        .map_err(|_| TransferOperatorError::Bundle("portable Pack store"))?;
    let inventory = store
        .load_startup_inventory()
        .map_err(|_| TransferOperatorError::Bundle("portable Pack inventory"))?;
    let mut artifacts = Vec::with_capacity(referenced.len());
    for revision in referenced {
        let entry = inventory
            .entries()
            .iter()
            .find(|entry| entry.bundle().revision_digest() == &revision)
            .ok_or(TransferOperatorError::Bundle(
                "missing referenced portable Pack revision",
            ))?;
        artifacts.push(
            RetainedPackBundleArtifactV1::from_archive_bytes(
                entry.bundle().archive_bytes().to_vec(),
            )
            .map_err(|_| TransferOperatorError::Bundle("portable Pack archive"))?,
        );
    }
    artifacts.sort_by(|left, right| {
        (
            left.revision_digest.to_string(),
            left.bundle_digest.to_string(),
        )
            .cmp(&(
                right.revision_digest.to_string(),
                right.bundle_digest.to_string(),
            ))
    });
    Ok(artifacts)
}

fn canonical_records(
    export: &SqliteCanonicalExportV1,
) -> Result<Vec<LogicalRecordV1>, TransferOperatorError> {
    let mut records = Vec::with_capacity(export.records().len().saturating_add(2));
    push_record(
        &mut records,
        CanonicalRecordKindV1::DeploymentLineage,
        "deployment/lineage",
        export.deployment_lineage().as_bytes(),
    )?;
    let epoch = export.storage_epoch().to_string();
    push_record(
        &mut records,
        CanonicalRecordKindV1::StorageEpoch,
        "deployment/epoch",
        epoch.as_bytes(),
    )?;
    for record in export.records() {
        let kind = match record.kind() {
            SqliteCanonicalRecordKindV1::RoomGenesis => CanonicalRecordKindV1::RoomGenesis,
            SqliteCanonicalRecordKindV1::RoomTransition => CanonicalRecordKindV1::RoomTransition,
            SqliteCanonicalRecordKindV1::RoomHead => CanonicalRecordKindV1::RoomHead,
            SqliteCanonicalRecordKindV1::CoreMaterialization => {
                CanonicalRecordKindV1::CoreMaterialization
            }
            SqliteCanonicalRecordKindV1::ActivityMaterialization => {
                CanonicalRecordKindV1::ActivityMaterialization
            }
            SqliteCanonicalRecordKindV1::PackRevisionLock => {
                CanonicalRecordKindV1::ArtifactMetadata
            }
            SqliteCanonicalRecordKindV1::ExternalInputPreparation => {
                CanonicalRecordKindV1::ExternalInputPreparation
            }
        };
        push_record(&mut records, kind, record.identity(), record.bytes())?;
    }
    Ok(records)
}

fn push_record(
    records: &mut Vec<LogicalRecordV1>,
    kind: CanonicalRecordKindV1,
    identity: &str,
    bytes: &[u8],
) -> Result<(), TransferOperatorError> {
    let ordinal = u64::try_from(records.len())
        .map_err(|_| TransferOperatorError::Bundle("canonical record count"))?;
    records.push(
        LogicalRecordV1::canonical(ordinal, kind, identity, bytes)
            .map_err(|_| TransferOperatorError::Bundle("canonical record"))?,
    );
    Ok(())
}

fn publish_or_confirm_bundle(
    path: &Path,
    bundle: &TransferBundleV1,
) -> Result<(), TransferOperatorError> {
    if fs::symlink_metadata(path).is_ok() {
        validate_owner_only_file(path)
            .map_err(|_| TransferOperatorError::Bundle("existing owner-only bundle"))?;
        let existing = read_canonical_bundle(path)?;
        let existing_bytes = existing
            .to_bytes()
            .map_err(|_| TransferOperatorError::Bundle("existing bundle encoding"))?;
        let expected_bytes = bundle
            .to_bytes()
            .map_err(|_| TransferOperatorError::Bundle("bundle encoding"))?;
        if existing_bytes != expected_bytes {
            return Err(TransferOperatorError::Bundle("existing bundle mismatch"));
        }
        return Ok(());
    }
    let bytes = bundle
        .to_bytes()
        .map_err(|_| TransferOperatorError::Bundle("encode bundle"))?;
    publish_owner_only(path, &bytes, "bundle publication")
}

fn read_canonical_bundle(path: &Path) -> Result<TransferBundleV1, TransferOperatorError> {
    read_canonical_bundle_with_hook(path, || Ok(()))
}

/// Imports and semantically finalizes one manifest-bearing v2 stream through
/// the durable non-serving PostgreSQL fence without reading the whole file
/// into memory. It never publishes target authority; source retirement and the
/// existing final publication transition remain separate operator steps.
pub fn import_stream_chunks_v2(
    path: &Path,
    admin: &PostgresAdmin,
    target: TargetFingerprintV1,
    limits: TransferStreamLimitsV2,
) -> Result<TransferStreamCheckpointV2, TransferOperatorError> {
    let (mut reader, mut destination, artifact_identity) =
        open_manifest_stream_destination_v2(path, admin, target, limits)?;
    let checkpoint = import_and_finalize_stream_v2(&mut reader, &mut destination)
        .map_err(|_| TransferOperatorError::Destination("stream import and verification"))?;
    revalidate_stream_artifact_v2(path, artifact_identity)?;
    Ok(checkpoint)
}

/// Imports, semantically finalizes, retires the matching SQLite source, and
/// only then publishes the matching PostgreSQL target. Every phase is bound to
/// the stream header, complete manifest, verified footer, frozen backup, and
/// source/target Storage Epochs; no `TransferBundleV1` is constructed.
pub fn finalize_stream_authority_v2(
    source: &SqliteRoomStore,
    path: &Path,
    admin: &PostgresAdmin,
    target: TargetFingerprintV1,
    limits: TransferStreamLimitsV2,
) -> Result<TransferStreamAuthorityResultV2, TransferOperatorError> {
    let (mut reader, mut destination, artifact_identity) =
        open_manifest_stream_destination_v2(path, admin, target, limits)?;
    import_and_finalize_stream_v2(&mut reader, &mut destination)
        .map_err(|_| TransferOperatorError::Destination("stream import and verification"))?;
    let manifest = reader
        .manifest()
        .cloned()
        .ok_or(TransferOperatorError::Bundle("stream manifest required"))?;
    let footer = reader.footer().ok_or(TransferOperatorError::Bundle(
        "verified stream footer required",
    ))?;
    let identity = reader.identity().clone();
    let target = destination.target().clone();
    revalidate_stream_artifact_v2(path, artifact_identity)?;
    finalize_stream_whole_deployment_v2(
        source,
        &mut destination,
        &identity,
        &manifest,
        footer,
        &target,
    )
    .map_err(|_| TransferOperatorError::Destination("stream authority handoff"))?;
    revalidate_stream_artifact_v2(path, artifact_identity)?;
    stream_authority_result_v2("finalize_stream_authority", &identity, &manifest, &target)
}

/// Safely abandons a v2 stream before SQLite source retirement. The whole
/// artifact is read in bounded chunks so its authenticated footer binds the
/// target tombstone to exactly the source transfer that is restored.
pub fn abort_stream_authority_v2(
    source: &SqliteRoomStore,
    path: &Path,
    admin: &PostgresAdmin,
    target: TargetFingerprintV1,
    limits: TransferStreamLimitsV2,
) -> Result<TransferStreamAuthorityResultV2, TransferOperatorError> {
    let (mut reader, mut destination, artifact_identity) =
        open_manifest_stream_destination_v2(path, admin, target, limits)?;
    while reader
        .next_chunk()
        .map_err(|_| TransferOperatorError::Bundle("verify stream before abort"))?
        .is_some()
    {}
    let manifest = reader
        .manifest()
        .cloned()
        .ok_or(TransferOperatorError::Bundle("stream manifest required"))?;
    let footer = reader.footer().ok_or(TransferOperatorError::Bundle(
        "verified stream footer required",
    ))?;
    let identity = reader.identity().clone();
    let target = destination.target().clone();
    revalidate_stream_artifact_v2(path, artifact_identity)?;
    abort_stream_whole_deployment_v2(
        source,
        &mut destination,
        &identity,
        &manifest,
        footer,
        &target,
    )
    .map_err(|_| TransferOperatorError::Destination("stream authority abort"))?;
    revalidate_stream_artifact_v2(path, artifact_identity)?;
    stream_authority_result_v2("abort_stream_authority", &identity, &manifest, &target)
}

fn open_manifest_stream_destination_v2<'a>(
    path: &Path,
    admin: &'a PostgresAdmin,
    target: TargetFingerprintV1,
    limits: TransferStreamLimitsV2,
) -> Result<
    (
        TransferStreamReaderV2<File>,
        PostgresStreamDestinationV2<'a>,
        OperatorFileIdentity,
    ),
    TransferOperatorError,
> {
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only stream bundle"))?;
    let file = File::open(path).map_err(|_| TransferOperatorError::Bundle("open stream bundle"))?;
    let artifact_identity = operator_file_identity(&file)
        .map_err(|_| TransferOperatorError::Bundle("stream bundle identity"))?;
    require_named_file_identity(path, artifact_identity)
        .map_err(|_| TransferOperatorError::Bundle("stream bundle identity"))?;
    let reader = TransferStreamReaderV2::new(file, limits)
        .map_err(|_| TransferOperatorError::Bundle("decode stream header"))?;
    let manifest = reader
        .manifest()
        .cloned()
        .ok_or(TransferOperatorError::Bundle("stream manifest required"))?;
    let destination = PostgresStreamDestinationV2::new_with_manifest(
        admin,
        reader.identity().clone(),
        manifest,
        reader.stream_header_digest(),
        target,
    )
    .map_err(|_| TransferOperatorError::Destination("open stream target"))?;
    Ok((reader, destination, artifact_identity))
}

fn revalidate_stream_artifact_v2(
    path: &Path,
    artifact_identity: OperatorFileIdentity,
) -> Result<(), TransferOperatorError> {
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only stream bundle"))?;
    require_named_file_identity(path, artifact_identity)
        .map_err(|_| TransferOperatorError::Bundle("stream bundle identity changed"))
}

fn stream_authority_result_v2(
    operation: &'static str,
    identity: &TransferStreamIdentityV2,
    manifest: &worldstream_transfer::TransferStreamManifestV2,
    target: &TargetFingerprintV1,
) -> Result<TransferStreamAuthorityResultV2, TransferOperatorError> {
    Ok(TransferStreamAuthorityResultV2 {
        schema: "worldstream/transfer-stream-authority-result/v2",
        status: "ok",
        operation,
        stream_header_digest: identity
            .stream_header_digest(Some(manifest))
            .map_err(|_| TransferOperatorError::Bundle("stream header digest"))?
            .to_string(),
        manifest_digest: manifest
            .digest()
            .map_err(|_| TransferOperatorError::Bundle("stream manifest digest"))?
            .to_string(),
        source_epoch: identity.source_epoch(),
        target_epoch: target.storage_epoch(),
    })
}

/// Exports the exact backup retained by a transfer-pending SQLite source as a
/// manifest-bearing v2 stream.
///
/// The caller first freezes the source with [`SqliteRoomStore::begin_source_transfer`].
/// This function then opens only that retained backup, verifies it incrementally,
/// emits keyset-backed records into an owner-only temporary artifact, and
/// atomically publishes the final stream only after its authenticated footer
/// has been written and synced. It does not construct a [`TransferBundleV1`]
/// or collect the source records in memory.
pub fn export_pending_stream_v2(
    source: &SqliteRoomStore,
    stream_path: &Path,
    stream_id: &str,
    limits: TransferStreamLimitsV2,
) -> Result<TransferStreamExportResultV2, TransferOperatorError> {
    let status = source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("read stream source transfer fence"))?;
    if status.state() != SqliteSourceTransferStateV1::TransferPending {
        return Err(TransferOperatorError::Source(
            "stream source is not transfer-pending",
        ));
    }
    let source_epoch = status
        .source_epoch()
        .ok_or(TransferOperatorError::Source("stream source epoch"))?;
    let backup_path = status
        .backup_path()
        .ok_or(TransferOperatorError::Source("stream retained backup path"))?;
    let backup_digest = status.backup_digest().ok_or(TransferOperatorError::Source(
        "stream retained backup digest",
    ))?;
    validate_owner_only_file(backup_path)
        .map_err(|_| TransferOperatorError::Source("owner-only stream retained backup"))?;

    let target_backend = postgres_backend_fingerprint()
        .map_err(|_| TransferOperatorError::Bundle("PostgreSQL stream backend fingerprint"))?;
    let source_backend = BackendFingerprintV1::new(
        BundleProfileV1::SqliteBundled,
        "sqlite-bundled",
        target_backend.schema().clone(),
    )
    .map_err(|_| TransferOperatorError::Bundle("SQLite stream backend fingerprint"))?;
    let identity = TransferStreamIdentityV2::new(
        stream_id,
        source
            .deployment_lineage()
            .map_err(|_| TransferOperatorError::Source("stream source lineage"))?,
        source_epoch,
        BundleProfileV1::SqliteBundled,
        BundleProfileV1::PostgresPrimary17,
    )
    .map_err(|_| TransferOperatorError::Source("stream identity"))?;
    let mut stream_source = SqliteTransferStreamSourceV2::open(
        backup_path,
        backup_digest,
        &identity,
        source_backend,
        target_backend,
    )
    .map_err(|_| TransferOperatorError::Source("verified stream backup source"))?;
    let manifest = stream_source.manifest().clone();
    let stream_header_digest = identity
        .stream_header_digest(Some(&manifest))
        .map_err(|_| TransferOperatorError::Bundle("stream header digest"))?;
    let manifest_digest = manifest
        .digest()
        .map_err(|_| TransferOperatorError::Bundle("stream manifest digest"))?;

    publish_stream_owner_only(stream_path, "stream publication", |file| {
        export_stream_v2(file, identity.clone(), &mut stream_source, limits)
            .map(|_| ())
            .map_err(|_| TransferOperatorError::Bundle("stream export"))
    })?;

    let after = source
        .source_transfer_status()
        .map_err(|_| TransferOperatorError::Source("recheck stream source transfer fence"))?;
    if after.state() != SqliteSourceTransferStateV1::TransferPending
        || after.source_epoch() != Some(source_epoch)
        || after.backup_digest() != Some(backup_digest)
        || after.backup_path() != Some(backup_path)
    {
        return Err(TransferOperatorError::Source(
            "stream source transfer fence changed",
        ));
    }
    validate_owner_only_file(stream_path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only published stream"))?;
    Ok(TransferStreamExportResultV2 {
        schema: "worldstream/transfer-stream-export-result/v2",
        status: "ok",
        operation: "export_stream",
        stream_header_digest: stream_header_digest.to_string(),
        manifest_digest: manifest_digest.to_string(),
        backup_digest: backup_digest.to_string(),
        source_epoch,
        record_count: manifest.expected_record_count(),
        record_bytes: manifest.expected_record_bytes(),
    })
}

fn read_canonical_bundle_with_hook<F>(
    path: &Path,
    after_open: F,
) -> Result<TransferBundleV1, TransferOperatorError>
where
    F: FnOnce() -> Result<(), TransferOperatorError>,
{
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only bounded bundle"))?;
    let mut file =
        File::open(path).map_err(|_| TransferOperatorError::Bundle("open bounded bundle"))?;
    let identity = operator_file_identity(&file)
        .map_err(|_| TransferOperatorError::Bundle("bounded bundle identity"))?;
    require_named_file_identity(path, identity)
        .map_err(|_| TransferOperatorError::Bundle("bounded bundle identity"))?;
    after_open()?;
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only bounded bundle"))?;
    require_named_file_identity(path, identity)
        .map_err(|_| TransferOperatorError::Bundle("bounded bundle identity changed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| TransferOperatorError::Bundle("stat bounded bundle"))?;
    if !metadata.is_file() || metadata.len() > MAX_BUNDLE_BYTES as u64 {
        return Err(TransferOperatorError::Bundle("bounded bundle size"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(
            u64::try_from(MAX_BUNDLE_BYTES)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .map_err(|_| TransferOperatorError::Bundle("read bounded bundle"))?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(TransferOperatorError::Bundle("bounded bundle size"));
    }
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::Bundle("owner-only bounded bundle"))?;
    require_named_file_identity(path, identity)
        .map_err(|_| TransferOperatorError::Bundle("bounded bundle identity changed"))?;
    if operator_file_identity(&file).ok() != Some(identity) {
        return Err(TransferOperatorError::Bundle(
            "bounded bundle identity changed",
        ));
    }
    let bundle = TransferBundleV1::from_bytes(&bytes)
        .map_err(|_| TransferOperatorError::Bundle("decode exact bundle"))?;
    let canonical = bundle
        .to_bytes()
        .map_err(|_| TransferOperatorError::Bundle("encode exact bundle"))?;
    if canonical != bytes {
        return Err(TransferOperatorError::Bundle(
            "noncanonical bundle encoding",
        ));
    }
    Ok(bundle)
}

fn publish_or_confirm_begin_intent(
    state_dir: &Path,
    intent: &BeginIntentV1,
) -> Result<bool, TransferOperatorError> {
    let path = state_dir.join(BEGIN_INTENT_FILE);
    let bytes = serde_json::to_vec(intent)
        .map_err(|_| TransferOperatorError::State("encode begin intent"))?;
    if fs::symlink_metadata(&path).is_ok() {
        validate_owner_only_file(&path)
            .map_err(|_| TransferOperatorError::State("owner-only begin intent"))?;
        let existing = read_bounded(&path, MAX_STATE_BYTES)?;
        let decoded: BeginIntentV1 = serde_json::from_slice(&existing)
            .map_err(|_| TransferOperatorError::State("decode begin intent"))?;
        let canonical = serde_json::to_vec(&decoded)
            .map_err(|_| TransferOperatorError::State("encode begin intent"))?;
        if existing != canonical || decoded != *intent {
            return Err(TransferOperatorError::State("begin intent mismatch"));
        }
        return Ok(false);
    }
    publish_owner_only(&path, &bytes, "begin intent publication")?;
    let published = read_bounded(&path, MAX_STATE_BYTES)?;
    if published != bytes {
        return Err(TransferOperatorError::State(
            "published begin intent mismatch",
        ));
    }
    Ok(true)
}

fn load_begin_intent(state_dir: &Path) -> Result<BeginIntentV1, TransferOperatorError> {
    let path = state_dir.join(BEGIN_INTENT_FILE);
    validate_owner_only_file(&path)
        .map_err(|_| TransferOperatorError::State("owner-only begin intent"))?;
    let bytes = read_bounded(&path, MAX_STATE_BYTES)?;
    let intent: BeginIntentV1 = serde_json::from_slice(&bytes)
        .map_err(|_| TransferOperatorError::State("decode begin intent"))?;
    let canonical = serde_json::to_vec(&intent)
        .map_err(|_| TransferOperatorError::State("encode begin intent"))?;
    if bytes != canonical || intent.schema != BEGIN_INTENT_SCHEMA {
        return Err(TransferOperatorError::State("begin intent encoding"));
    }
    Ok(intent)
}

fn validate_state_directory_inventory(
    state_dir: &Path,
    intent: &BeginIntentV1,
) -> Result<(), TransferOperatorError> {
    let entries = fs::read_dir(state_dir)
        .map_err(|_| TransferOperatorError::State("list state directory"))?;
    let mut count = 0_usize;
    for entry in entries {
        count = count.saturating_add(1);
        if count > MAX_STATE_DIRECTORY_ENTRIES {
            return Err(TransferOperatorError::State("state directory entry bound"));
        }
        let entry = entry.map_err(|_| TransferOperatorError::State("list state directory"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| TransferOperatorError::State("state artifact file name"))?;
        let known = name == OPERATION_LOCK_FILE
            || name == BEGIN_INTENT_FILE
            || artifact_name_digest(Path::new(&name))
                .is_ok_and(|digest| digest.to_string() == intent.backup_name_digest)
            || artifact_name_digest(Path::new(&name))
                .is_ok_and(|digest| digest.to_string() == intent.bundle_name_digest)
            || (name.starts_with(STATE_PREFIX) && parse_generation_name(&name).is_ok())
            || known_publication_partial(&name, intent)
            || known_native_backup_partial(&name, intent)
            || known_backup_sidecar(&name, intent);
        if !known {
            return Err(TransferOperatorError::State("unknown state artifact"));
        }
        validate_owner_only_file(&entry.path())
            .map_err(|_| TransferOperatorError::State("owner-only state artifact"))?;
    }
    Ok(())
}

fn known_publication_partial(name: &str, intent: &BeginIntentV1) -> bool {
    let Some(value) = name
        .strip_prefix('.')
        .and_then(|value| value.strip_suffix(".partial"))
    else {
        return false;
    };
    let Some((target, nonce)) = value.rsplit_once('.') else {
        return false;
    };
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    target == BEGIN_INTENT_FILE
        || artifact_name_digest(Path::new(target))
            .is_ok_and(|digest| digest.to_string() == intent.bundle_name_digest)
        || (target.starts_with(STATE_PREFIX) && parse_generation_name(target).is_ok())
}

fn known_native_backup_partial(name: &str, intent: &BeginIntentV1) -> bool {
    let value = name
        .strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-shm"))
        .unwrap_or(name);
    let Some(value) = value
        .strip_prefix('.')
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    let Some((backup_name, suffix)) = value.split_once(".worldstream-transfer-") else {
        return false;
    };
    suffix.len() == 32
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && artifact_name_digest(Path::new(backup_name))
            .is_ok_and(|digest| digest.to_string() == intent.backup_name_digest)
}

fn known_backup_sidecar(name: &str, intent: &BeginIntentV1) -> bool {
    let Some(backup_name) = name
        .strip_suffix("-wal")
        .or_else(|| name.strip_suffix("-shm"))
    else {
        return false;
    };
    artifact_name_digest(Path::new(backup_name))
        .is_ok_and(|digest| digest.to_string() == intent.backup_name_digest)
}

fn artifact_name_digest(path: &Path) -> Result<DigestV1, TransferOperatorError> {
    let name = path
        .file_name()
        .ok_or(TransferOperatorError::InvalidArgument("artifact file name"))?;
    Ok(path_digest(Path::new(name)))
}

fn persist_generation(
    state_dir: &Path,
    previous: Option<u64>,
    phase: OperatorPhaseV1,
    binding: &TransferBindingV1,
    bundle: &TransferBundleV1,
    session: &TransferImportSessionV1,
) -> Result<StateGenerationV1, TransferOperatorError> {
    let sequence = previous
        .map_or(Ok(0), |value| value.checked_add(1).ok_or(()))
        .map_err(|()| TransferOperatorError::State("generation sequence"))?;
    let session_bytes = session
        .to_bytes()
        .map_err(|_| TransferOperatorError::State("serialize session"))?;
    let generation = StateGenerationV1 {
        schema: STATE_SCHEMA.to_owned(),
        sequence,
        phase,
        source_path_digest: binding.source_path_digest.to_string(),
        backup_digest: binding.backup_digest.to_string(),
        bundle_hash: binding.bundle_hash.to_string(),
        target_fingerprint: binding.target_fingerprint.to_string(),
        record_count: bundle.records().len(),
        next_ordinal: session.next_ordinal(),
        session_hex: encode_hex(&session_bytes),
    };
    let bytes = serde_json::to_vec(&generation)
        .map_err(|_| TransferOperatorError::State("encode generation"))?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(TransferOperatorError::State("generation size"));
    }
    let nonce = random_nonce()?;
    let path = state_dir.join(format!("{STATE_PREFIX}{sequence:020}-{nonce}.json"));
    publish_owner_only(&path, &bytes, "generation publication")?;
    Ok(generation)
}

fn load_latest_generation(
    state_dir: &Path,
    bundle: &TransferBundleV1,
    target: &TargetFingerprintV1,
    binding: &TransferBindingV1,
) -> Result<Option<(StateGenerationV1, TransferImportSessionV1)>, TransferOperatorError> {
    load_latest_generation_without_backup(
        state_dir,
        bundle,
        target,
        binding.source_path_digest,
        binding.bundle_hash,
        binding.target_fingerprint,
    )
    .and_then(|value| {
        if value.as_ref().is_some_and(|(generation, _)| {
            generation.backup_digest != binding.backup_digest.to_string()
        }) {
            Err(TransferOperatorError::State("backup binding mismatch"))
        } else {
            Ok(value)
        }
    })
}

fn load_latest_generation_without_backup(
    state_dir: &Path,
    bundle: &TransferBundleV1,
    target: &TargetFingerprintV1,
    source_path_digest: DigestV1,
    bundle_hash: DigestV1,
    target_fingerprint: DigestV1,
) -> Result<Option<(StateGenerationV1, TransferImportSessionV1)>, TransferOperatorError> {
    let mut generations = Vec::new();
    let entries =
        fs::read_dir(state_dir).map_err(|_| TransferOperatorError::State("list generations"))?;
    for entry in entries {
        let entry = entry.map_err(|_| TransferOperatorError::State("list generation"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| TransferOperatorError::State("generation file name"))?;
        if name.ends_with(".partial") || !name.starts_with(STATE_PREFIX) {
            continue;
        }
        let sequence = parse_generation_name(&name)?;
        let path = entry.path();
        validate_owner_only_file(&path)
            .map_err(|_| TransferOperatorError::State("owner-only generation"))?;
        let bytes = read_bounded(&path, MAX_STATE_BYTES)?;
        let generation: StateGenerationV1 = serde_json::from_slice(&bytes)
            .map_err(|_| TransferOperatorError::State("decode generation"))?;
        let canonical = serde_json::to_vec(&generation)
            .map_err(|_| TransferOperatorError::State("encode generation"))?;
        if canonical != bytes {
            return Err(TransferOperatorError::State(
                "noncanonical generation encoding",
            ));
        }
        if generation.schema != STATE_SCHEMA
            || generation.sequence != sequence
            || generation.source_path_digest != source_path_digest.to_string()
            || generation.bundle_hash != bundle_hash.to_string()
            || generation.target_fingerprint != target_fingerprint.to_string()
            || generation.record_count != bundle.records().len()
        {
            return Err(TransferOperatorError::State("generation binding"));
        }
        decode_digest(&generation.backup_digest)?;
        let session_bytes = decode_hex(&generation.session_hex)?;
        let session = TransferImportSessionV1::from_bytes(bundle, &session_bytes)
            .map_err(|_| TransferOperatorError::State("session checkpoint"))?;
        if session.target() != target
            || session.next_ordinal() != generation.next_ordinal
            || !phase_matches_session(generation.phase, session.state())
        {
            return Err(TransferOperatorError::State("session phase binding"));
        }
        generations.push((generation, session));
    }
    generations.sort_by_key(|(generation, _)| generation.sequence);
    if generations
        .windows(2)
        .any(|pair| pair[0].0.sequence == pair[1].0.sequence)
    {
        return Err(TransferOperatorError::State(
            "duplicate generation sequence",
        ));
    }
    Ok(generations.pop())
}

fn phase_matches_session(phase: OperatorPhaseV1, state: TransferStateV1) -> bool {
    match phase {
        OperatorPhaseV1::TargetAuthoritative => state == TransferStateV1::TargetAuthoritative,
        OperatorPhaseV1::Finalizing => matches!(
            state,
            TransferStateV1::TargetVerified | TransferStateV1::TargetAuthoritative
        ),
        OperatorPhaseV1::Begun
        | OperatorPhaseV1::Importing
        | OperatorPhaseV1::ChunksComplete
        | OperatorPhaseV1::Aborting
        | OperatorPhaseV1::SourceAuthoritative => state == TransferStateV1::TargetVerified,
    }
}

fn operator_result(
    operation: &'static str,
    phase: OperatorPhaseV1,
    bundle: &TransferBundleV1,
    target: &TargetFingerprintV1,
    session: &TransferImportSessionV1,
) -> Result<TransferOperatorResultV1, TransferOperatorError> {
    let target_fingerprint = target
        .fingerprint_digest()
        .map_err(|_| TransferOperatorError::Bundle("result target fingerprint"))?;
    Ok(TransferOperatorResultV1 {
        schema: RESULT_SCHEMA,
        status: "ok",
        operation,
        phase: phase.label(),
        bundle_hash: target.bundle_hash().to_string(),
        target_fingerprint: target_fingerprint.to_string(),
        source_epoch: bundle.source_epoch(),
        target_epoch: target.storage_epoch(),
        record_count: bundle.records().len(),
        next_ordinal: session.next_ordinal(),
        chunks_complete: session.is_complete(),
    })
}

fn artifact_path_in_state_dir(
    path: &Path,
    state_dir: &Path,
) -> Result<PathBuf, TransferOperatorError> {
    let name = path
        .file_name()
        .ok_or(TransferOperatorError::InvalidArgument("artifact file name"))?;
    let parent = path.parent().filter(|value| !value.as_os_str().is_empty());
    let resolved_parent = fs::canonicalize(parent.unwrap_or_else(|| Path::new(".")))
        .map_err(|_| TransferOperatorError::InvalidArgument("artifact parent"))?;
    if resolved_parent != state_dir {
        return Err(TransferOperatorError::InvalidArgument(
            "artifacts must be direct children of the state directory",
        ));
    }
    Ok(state_dir.join(name))
}

fn canonical_regular_file(
    path: &Path,
    label: &'static str,
) -> Result<PathBuf, TransferOperatorError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| TransferOperatorError::InvalidArgument(label))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(TransferOperatorError::InvalidArgument(label));
    }
    fs::canonicalize(path).map_err(|_| TransferOperatorError::InvalidArgument(label))
}

fn publish_owner_only(
    path: &Path,
    bytes: &[u8],
    label: &'static str,
) -> Result<(), TransferOperatorError> {
    publish_owner_only_with_hook(path, bytes, label, |_| Ok(()))
}

/// Publishes a large transfer stream without first collecting it into a
/// memory buffer. The callback writes only to an unlinked/partial owner-only
/// file; the final name becomes visible after the callback returns, the file
/// is synced, and the completed stream is authenticated by its writer.
fn publish_stream_owner_only<F>(
    path: &Path,
    label: &'static str,
    write_stream: F,
) -> Result<(), TransferOperatorError>
where
    F: FnOnce(&mut File) -> Result<(), TransferOperatorError>,
{
    let parent = PublicationParent::open(path).map_err(|_| TransferOperatorError::State(label))?;
    parent
        .require_relative_absent(parent.name())
        .map_err(|_| TransferOperatorError::State("stream publication destination exists"))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(TransferOperatorError::State(label))?;
    let nonce = random_nonce()?;
    let partial = path.with_file_name(format!(".{name}.{nonce}.partial"));
    let partial_name = partial
        .file_name()
        .ok_or(TransferOperatorError::State(label))?
        .to_owned();
    let (mut source, source_is_named) =
        create_transfer_publication_source(&parent, &partial_name, label)?;
    let source_identity =
        operator_file_identity(&source).map_err(|_| TransferOperatorError::State(label))?;
    let mut published: Option<(File, OperatorFileIdentity)> = None;
    let result = (|| {
        write_stream(&mut source)?;
        source
            .sync_all()
            .map_err(|_| TransferOperatorError::State(label))?;
        if source
            .metadata()
            .map_err(|_| TransferOperatorError::State(label))?
            .len()
            == 0
        {
            return Err(TransferOperatorError::Bundle("empty stream publication"));
        }
        if operator_file_identity(&source).ok() != Some(source_identity) {
            return Err(TransferOperatorError::State(label));
        }
        if source_is_named {
            parent
                .require_relative_identity(&partial_name, source_identity)
                .map_err(|_| TransferOperatorError::State(label))?;
        } else {
            parent
                .require_relative_absent(&partial_name)
                .map_err(|_| TransferOperatorError::State(label))?;
        }
        parent
            .require_named()
            .map_err(|_| TransferOperatorError::State(label))?;
        let final_file = match publish_open_file_noreplace(&source, &parent) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(TransferOperatorError::State(
                    "stream publication destination exists",
                ));
            }
            Err(_) => return Err(TransferOperatorError::State(label)),
        };
        let final_identity =
            operator_file_identity(&final_file).map_err(|_| TransferOperatorError::State(label))?;
        parent
            .require_relative_identity(parent.name(), final_identity)
            .map_err(|_| TransferOperatorError::State(label))?;
        parent
            .sync()
            .map_err(|_| TransferOperatorError::State(label))?;
        validate_owner_only_file(path).map_err(|_| TransferOperatorError::State(label))?;
        if final_file
            .metadata()
            .map_err(|_| TransferOperatorError::State(label))?
            .len()
            == 0
        {
            return Err(TransferOperatorError::Bundle("empty published stream"));
        }
        published = Some((final_file, final_identity));

        if source_is_named {
            #[cfg(windows)]
            {
                parent
                    .require_relative_absent(&partial_name)
                    .map_err(|_| TransferOperatorError::State(label))?;
            }
            #[cfg(not(windows))]
            if !scrub_exact_operator_file(&source, source_identity) {
                return Err(TransferOperatorError::State(
                    "stream publication source cleanup incomplete",
                ));
            }
        } else {
            parent
                .require_relative_absent(&partial_name)
                .map_err(|_| TransferOperatorError::State(label))?;
        }
        parent
            .sync()
            .map_err(|_| TransferOperatorError::State(label))?;
        parent
            .require_named()
            .map_err(|_| TransferOperatorError::State(label))?;
        if let Some((final_file, final_identity)) = published.as_ref() {
            if operator_file_identity(final_file).ok() != Some(*final_identity) {
                return Err(TransferOperatorError::State(label));
            }
            parent
                .require_relative_identity(parent.name(), *final_identity)
                .map_err(|_| TransferOperatorError::State(label))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let mut cleanup_complete = true;
        if let Some((final_file, final_identity)) = published.as_ref() {
            cleanup_complete &= scrub_exact_operator_file(final_file, *final_identity);
        }
        cleanup_complete &= if source_is_named {
            scrub_exact_operator_file(&source, source_identity)
        } else {
            scrub_unlinked_stream_source(&source, source_identity)
        };
        cleanup_complete &= parent.sync().is_ok();
        if !cleanup_complete {
            return Err(TransferOperatorError::State(
                "stream publication cleanup incomplete",
            ));
        }
    }
    result
}

fn scrub_unlinked_stream_source(file: &File, expected: OperatorFileIdentity) -> bool {
    if operator_file_identity(file).ok() != Some(expected) {
        return false;
    }
    if file.set_len(0).and_then(|()| file.sync_all()).is_err() {
        return false;
    }
    operator_file_identity(file).ok() == Some(expected)
        && file.metadata().is_ok_and(|metadata| metadata.len() == 0)
}

#[allow(clippy::too_many_lines)]
fn publish_owner_only_with_hook<F>(
    path: &Path,
    bytes: &[u8],
    label: &'static str,
    after_sync: F,
) -> Result<(), TransferOperatorError>
where
    F: FnOnce(&Path) -> Result<(), TransferOperatorError>,
{
    let parent = PublicationParent::open(path).map_err(|_| TransferOperatorError::State(label))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(TransferOperatorError::State(label))?;
    let nonce = random_nonce()?;
    let partial = path.with_file_name(format!(".{name}.{nonce}.partial"));
    let partial_name = partial
        .file_name()
        .ok_or(TransferOperatorError::State(label))?
        .to_owned();
    let (source, source_is_named) =
        create_transfer_publication_source(&parent, &partial_name, label)?;
    let mut file = Some(source);
    let identity =
        operator_file_identity(file.as_ref().ok_or(TransferOperatorError::State(label))?)
            .map_err(|_| TransferOperatorError::State(label))?;
    let mut published: Option<(File, OperatorFileIdentity)> = None;
    let result = (|| {
        let handle = file.as_mut().ok_or(TransferOperatorError::State(label))?;
        handle
            .write_all(bytes)
            .map_err(|_| TransferOperatorError::State(label))?;
        handle
            .sync_all()
            .map_err(|_| TransferOperatorError::State(label))?;
        require_exact_open_handle_bytes(handle, identity, bytes, label)?;
        after_sync(&partial)?;
        if source_is_named {
            parent
                .require_relative_identity(&partial_name, identity)
                .map_err(|_| TransferOperatorError::State(label))?;
        } else {
            parent
                .require_relative_absent(&partial_name)
                .map_err(|_| TransferOperatorError::State(label))?;
        }
        parent
            .require_named()
            .map_err(|_| TransferOperatorError::State(label))?;
        let mut final_file = match publish_open_file_noreplace(handle, &parent) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(TransferOperatorError::State(
                    "publication destination exists",
                ));
            }
            Err(_) => return Err(TransferOperatorError::State(label)),
        };
        parent
            .sync()
            .map_err(|_| TransferOperatorError::State(label))?;
        require_exact_open_handle_bytes(handle, identity, bytes, label)?;

        let final_identity =
            operator_file_identity(&final_file).map_err(|_| TransferOperatorError::State(label))?;
        parent
            .require_relative_identity(parent.name(), final_identity)
            .map_err(|_| TransferOperatorError::State(label))?;
        parent
            .require_named()
            .map_err(|_| TransferOperatorError::State(label))?;
        validate_owner_only_file(path).map_err(|_| TransferOperatorError::State(label))?;
        require_exact_open_handle_bytes(&mut final_file, final_identity, bytes, label)?;
        published = Some((final_file, final_identity));

        if source_is_named {
            #[cfg(windows)]
            {
                parent
                    .require_relative_absent(&partial_name)
                    .map_err(|_| TransferOperatorError::State(label))?;
            }
            #[cfg(not(windows))]
            if !scrub_exact_operator_file(handle, identity) {
                return Err(TransferOperatorError::State(
                    "publication source cleanup incomplete",
                ));
            }
        } else {
            parent
                .require_relative_absent(&partial_name)
                .map_err(|_| TransferOperatorError::State(label))?;
        }
        parent
            .sync()
            .map_err(|_| TransferOperatorError::State(label))?;
        validate_owner_only_file(path).map_err(|_| TransferOperatorError::State(label))?;
        if let Some((final_file, final_identity)) = published.as_mut() {
            parent
                .require_relative_identity(parent.name(), *final_identity)
                .map_err(|_| TransferOperatorError::State(label))?;
            require_exact_open_handle_bytes(final_file, *final_identity, bytes, label)?;
        }
        parent
            .require_named()
            .map_err(|_| TransferOperatorError::State(label))?;
        Ok(())
    })();
    if result.is_err() {
        let mut cleanup_complete = true;
        if let Some((final_file, final_identity)) = published.as_ref() {
            cleanup_complete &= scrub_exact_operator_file(final_file, *final_identity);
        }
        if let Some(handle) = file.as_ref() {
            cleanup_complete &= scrub_exact_operator_file(handle, identity);
        }
        cleanup_complete &= parent.sync().is_ok();
        if !cleanup_complete {
            return Err(TransferOperatorError::State(
                "publication cleanup incomplete",
            ));
        }
    }
    result
}

fn create_transfer_publication_source(
    parent: &PublicationParent,
    partial_name: &OsStr,
    label: &'static str,
) -> Result<(File, bool), TransferOperatorError> {
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{Mode, OFlags};

        let descriptor = rustix::fs::openat(
            parent.directory(),
            ".",
            OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        );
        return descriptor
            .map(|descriptor| (File::from(descriptor), false))
            .map_err(|_| TransferOperatorError::State(label));
    }
    #[cfg(target_os = "macos")]
    {
        use rustix::fs::{Mode, OFlags};

        let descriptor = rustix::fs::openat(
            parent.directory(),
            partial_name,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| TransferOperatorError::State(label))?;
        return Ok((File::from(descriptor), true));
    }
    #[cfg(windows)]
    {
        return create_owner_only_renameable_file(&parent.path_for(partial_name))
            .map(|file| (file, true))
            .map_err(|_| TransferOperatorError::State(label));
    }
    #[allow(unreachable_code)]
    Err(TransferOperatorError::State(label))
}

fn require_exact_open_handle_bytes(
    file: &mut File,
    identity: OperatorFileIdentity,
    expected: &[u8],
    label: &'static str,
) -> Result<(), TransferOperatorError> {
    if operator_file_identity(file).ok() != Some(identity) {
        return Err(TransferOperatorError::State(label));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| TransferOperatorError::State(label))?;
    let mut observed = Vec::new();
    Read::by_ref(file)
        .take(
            u64::try_from(expected.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut observed)
        .map_err(|_| TransferOperatorError::State(label))?;
    if observed != expected || operator_file_identity(file).ok() != Some(identity) {
        return Err(TransferOperatorError::State(label));
    }
    Ok(())
}

#[cfg(test)]
fn remove_named_identity_with_hook<F>(
    path: &Path,
    identity: OperatorFileIdentity,
    after_identity_check: F,
) -> io::Result<()>
where
    F: FnOnce() -> io::Result<()>,
{
    require_named_file_identity(path, identity)?;
    after_identity_check()?;
    if operator_path_identity(path)? != identity {
        return Err(io::Error::other("private publication name changed"));
    }
    Err(io::Error::other("pathname cleanup is forbidden"))
}

fn sync_directory(path: &Path) -> Result<(), TransferOperatorError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| TransferOperatorError::State("sync state directory"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, TransferOperatorError> {
    read_bounded_with_hook(path, limit, || Ok(()))
}

fn read_bounded_with_hook<F>(
    path: &Path,
    limit: usize,
    after_open: F,
) -> Result<Vec<u8>, TransferOperatorError>
where
    F: FnOnce() -> Result<(), TransferOperatorError>,
{
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::State("owner-only state input"))?;
    let mut file = File::open(path).map_err(|_| TransferOperatorError::State("read generation"))?;
    let identity = operator_file_identity(&file)
        .map_err(|_| TransferOperatorError::State("state input identity"))?;
    require_named_file_identity(path, identity)
        .map_err(|_| TransferOperatorError::State("state input identity"))?;
    after_open()?;
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::State("owner-only state input"))?;
    require_named_file_identity(path, identity)
        .map_err(|_| TransferOperatorError::State("state input identity changed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| TransferOperatorError::State("state input metadata"))?;
    if !metadata.is_file() || metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(TransferOperatorError::State("generation size"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| TransferOperatorError::State("read generation"))?;
    if bytes.len() > limit {
        return Err(TransferOperatorError::State("generation size"));
    }
    validate_owner_only_file(path)
        .map_err(|_| TransferOperatorError::State("owner-only state input"))?;
    require_named_file_identity(path, identity)
        .map_err(|_| TransferOperatorError::State("state input identity changed"))?;
    if operator_file_identity(&file).ok() != Some(identity) {
        return Err(TransferOperatorError::State("state input identity changed"));
    }
    Ok(bytes)
}

fn parse_generation_name(name: &str) -> Result<u64, TransferOperatorError> {
    let value = name
        .strip_prefix(STATE_PREFIX)
        .and_then(|value| value.strip_suffix(".json"))
        .ok_or(TransferOperatorError::State("generation file name"))?;
    let (sequence, nonce) = value
        .split_once('-')
        .ok_or(TransferOperatorError::State("generation file name"))?;
    if sequence.len() != 20
        || nonce.len() != 32
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(TransferOperatorError::State("generation file name"));
    }
    sequence
        .parse()
        .map_err(|_| TransferOperatorError::State("generation sequence"))
}

fn random_nonce() -> Result<String, TransferOperatorError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| TransferOperatorError::State("random generation"))?;
    Ok(encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, TransferOperatorError> {
    if value.len() > MAX_STATE_BYTES.saturating_mul(2) || !value.len().is_multiple_of(2) {
        return Err(TransferOperatorError::State("session encoding"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_digest(value: &str) -> Result<DigestV1, TransferOperatorError> {
    let bytes = decode_hex(value)?;
    DigestV1::from_bytes(&bytes).map_err(|_| TransferOperatorError::State("digest encoding"))
}

fn hex_digit(value: u8) -> Result<u8, TransferOperatorError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(TransferOperatorError::State("session encoding")),
    }
}

fn path_digest(path: &Path) -> DigestV1 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        return DigestV1::hash(path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let bytes = path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        return DigestV1::hash(&bytes);
    }
    #[allow(unreachable_code)]
    DigestV1::hash(path.to_string_lossy().as_bytes())
}

#[cfg(unix)]
fn operator_file_identity(file: &File) -> io::Result<OperatorFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

#[cfg(unix)]
fn operator_file_has_single_link(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata().is_ok_and(|metadata| metadata.nlink() == 1)
}

#[cfg(windows)]
fn operator_file_has_single_link(file: &File) -> bool {
    worldstream_windows_handle::hard_link_count(file).is_ok_and(|count| count == 1)
}

fn scrub_exact_operator_file(file: &File, expected: OperatorFileIdentity) -> bool {
    if operator_file_identity(file).ok() != Some(expected) || !operator_file_has_single_link(file) {
        return false;
    }
    if file.set_len(0).and_then(|()| file.sync_all()).is_err() {
        return false;
    }
    operator_file_identity(file).ok() == Some(expected)
        && operator_file_has_single_link(file)
        && file.metadata().is_ok_and(|metadata| metadata.len() == 0)
}

#[cfg(windows)]
fn operator_file_identity(file: &File) -> io::Result<OperatorFileIdentity> {
    fs_id::FileID::new(file)
}

#[cfg(unix)]
fn operator_path_identity(path: &Path) -> io::Result<OperatorFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(io::Error::other("operator artifact is not a regular file"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn operator_path_identity(path: &Path) -> io::Result<OperatorFileIdentity> {
    fs_id::FileID::new(path)
}

#[cfg(unix)]
fn operator_directory_path_identity(path: &Path) -> io::Result<OperatorFileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(io::Error::other("operator state is not a directory"));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn operator_directory_path_identity(path: &Path) -> io::Result<OperatorFileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(io::Error::other("operator state is not a directory"));
    }
    fs_id::FileID::new(path)
}

fn require_named_file_identity(path: &Path, identity: OperatorFileIdentity) -> io::Result<()> {
    if operator_path_identity(path)? != identity {
        return Err(io::Error::other("operator artifact identity changed"));
    }
    Ok(())
}

#[cfg(unix)]
fn operator_identity_digest(identity: OperatorFileIdentity) -> DigestV1 {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&identity.0.to_le_bytes());
    bytes.extend_from_slice(&identity.1.to_le_bytes());
    DigestV1::hash(&bytes)
}

#[cfg(unix)]
fn operator_identity_matches_parts(
    identity: OperatorFileIdentity,
    storage_id: Option<u64>,
    file_id: Option<u128>,
) -> bool {
    storage_id == Some(identity.0) && file_id == Some(u128::from(identity.1))
}

#[cfg(windows)]
fn operator_identity_matches_parts(
    identity: OperatorFileIdentity,
    storage_id: Option<u64>,
    file_id: Option<u128>,
) -> bool {
    storage_id == Some(identity.storage_id()) && file_id == Some(identity.internal_file_id())
}

#[cfg(windows)]
fn operator_identity_digest(identity: OperatorFileIdentity) -> DigestV1 {
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&identity.storage_id().to_le_bytes());
    bytes.extend_from_slice(&identity.internal_file_id().to_le_bytes());
    DigestV1::hash(&bytes)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write as _};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use tempfile::tempdir;
    use worldstream_postgres::postgres_backend_fingerprint;
    use worldstream_runtime::{
        create_owner_only_file, prepare_data_directory, validate_owner_only_file,
    };
    use worldstream_sqlite::{
        SqliteRoomStore, SqliteSourceTransferStateV1, SqliteTransferStreamSourceV2,
    };
    use worldstream_transfer::{
        BackendFingerprintV1, BundleProfileV1, DeploymentIdentityV1, DigestV1, LogicalRecordV1,
        PackIdentityV1, RecordKindV1, ResourceKindV1, TargetFingerprintV1, TransferBundleV1,
        TransferChunkDispositionV1, TransferChunkV1, TransferDestinationV1,
        TransferStreamIdentityV2, TransferStreamLimitsV2, TransferStreamReaderV2,
        TransferStreamSourceV2,
    };

    use super::{
        MAX_STATE_BYTES, OperationLockV1, OperatorPhaseV1, TransferOperatorError, abort_loaded,
        begin_transfer, begin_transfer_with_hook, begin_transfer_with_hooks,
        export_pending_stream_v2, finalize_loaded, load_transfer, operator_file_identity,
        persist_generation, publish_or_confirm_bundle, publish_owner_only,
        publish_owner_only_with_hook, publish_stream_owner_only, read_bounded_with_hook,
        read_canonical_bundle_with_hook, remove_named_identity_with_hook, require_resumable_phase,
        revalidate_abort_source, revalidate_finalization_source, revalidate_pending_source,
    };

    #[allow(clippy::struct_excessive_bools)]
    #[derive(Default)]
    struct MemoryDestination {
        aborted: bool,
        verified: bool,
        finalized: bool,
        authoritative: bool,
    }

    impl TransferDestinationV1 for MemoryDestination {
        type Error = &'static str;

        fn apply_chunk(
            &mut self,
            _chunk: &TransferChunkV1,
        ) -> Result<TransferChunkDispositionV1, Self::Error> {
            Ok(TransferChunkDispositionV1::Applied)
        }

        fn verify_complete(
            &mut self,
            _bundle: &worldstream_transfer::TransferBundleV1,
            _target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            self.verified = true;
            Ok(())
        }

        fn record_finalization(
            &mut self,
            _target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            if !self.verified {
                return Err("target not verified");
            }
            self.finalized = true;
            Ok(())
        }

        fn reconcile_finalization_after_definite_source_failure(
            &mut self,
            _target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            if self.authoritative {
                return Err("target finalization cannot be reconciled");
            }
            if self.finalized {
                self.finalized = false;
            }
            Ok(())
        }

        fn accept_target_write(
            &mut self,
            _target: &TargetFingerprintV1,
        ) -> Result<(), Self::Error> {
            if !self.finalized {
                return Err("target not finalized");
            }
            self.authoritative = true;
            Ok(())
        }

        fn abort_import(&mut self, _target: &TargetFingerprintV1) -> Result<(), Self::Error> {
            self.aborted = true;
            Ok(())
        }
    }

    #[test]
    fn begin_intent_recovers_pending_crash_rejects_substitution_and_aborts_without_postgres()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let state_dir = temporary.path().join("transfer-state");
        prepare_data_directory(&state_dir)?;
        let backup = state_dir.join("source.backup.sqlite3");
        let bundle = state_dir.join("transfer.bundle");

        drop(initialized_source(&sqlite)?);
        let crashed = begin_transfer_with_hook(
            &sqlite,
            &backup,
            &bundle,
            &state_dir,
            "operator-crash-window",
            || Err(TransferOperatorError::State("test begin crash")),
        );
        assert_eq!(
            crashed,
            Err(TransferOperatorError::State("test begin crash"))
        );
        let store = SqliteRoomStore::open(&sqlite)?;
        assert_eq!(
            store.source_transfer_state(),
            SqliteSourceTransferStateV1::TransferPending
        );
        drop(store);
        assert!(!bundle.exists());
        assert!(generation_files(&state_dir)?.is_empty());

        assert!(
            begin_transfer(
                &sqlite,
                &backup,
                &bundle,
                &state_dir,
                "substituted-transfer-id",
            )
            .is_err()
        );
        assert!(
            begin_transfer(
                &sqlite,
                &backup,
                &state_dir.join("substituted.bundle"),
                &state_dir,
                "operator-crash-window",
            )
            .is_err()
        );

        let result = begin_transfer(
            &sqlite,
            &backup,
            &bundle,
            &state_dir,
            "operator-crash-window",
        )?;
        assert_eq!(result.phase, "begun");
        validate_owner_only_file(&bundle)?;
        assert_eq!(generation_files(&state_dir)?.len(), 1);

        let mut loaded = load_transfer(&sqlite, &bundle, &state_dir)?;
        revalidate_abort_source(&loaded)?;
        let mut destination = MemoryDestination::default();
        let aborted = abort_loaded(&mut loaded, &mut destination, || Ok(()))?;
        assert_eq!(aborted.phase, "source_authoritative");
        assert!(destination.aborted);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn begin_rejects_source_substitution_between_identity_admission_and_store_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let held = temporary.path().join("admitted-source.sqlite3");
        let alternate = temporary.path().join("alternate.sqlite3");
        let state_dir = temporary.path().join("transfer-state");
        prepare_data_directory(&state_dir)?;
        let backup = state_dir.join("source.backup.sqlite3");
        let bundle = state_dir.join("transfer.bundle");
        drop(initialized_source(&sqlite)?);
        drop(initialized_source(&alternate)?);

        let result = begin_transfer_with_hooks(
            &sqlite,
            &backup,
            &bundle,
            &state_dir,
            "source-open-substitution",
            || {
                fs::rename(&sqlite, &held)
                    .map_err(|_| TransferOperatorError::State("test source swap"))?;
                fs::rename(&alternate, &sqlite)
                    .map_err(|_| TransferOperatorError::State("test source swap"))?;
                Ok(())
            },
            || Ok(()),
        );

        assert_eq!(
            result,
            Err(TransferOperatorError::Source(
                "SQLite source identity changed"
            ))
        );
        assert!(!backup.exists());
        assert!(!bundle.exists());
        assert_eq!(
            SqliteRoomStore::open(&held)?.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceAuthoritative
        );
        assert_eq!(
            SqliteRoomStore::open(&sqlite)?.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceAuthoritative
        );
        Ok(())
    }

    #[test]
    fn abort_replays_after_source_restoration_before_terminal_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, sqlite, state_dir, _backup, bundle) = begun_fixture("abort-replay")?;
        let _keep_temporary = temporary;
        let mut loaded = load_transfer(&sqlite, &bundle, &state_dir)?;
        revalidate_abort_source(&loaded)?;
        let expected_bundle = loaded.binding.bundle_hash;
        let expected_target = loaded.binding.target_fingerprint;
        let mut destination = MemoryDestination::default();
        let crashed = abort_loaded(&mut loaded, &mut destination, || {
            Err(TransferOperatorError::State("test abort crash"))
        });
        assert_eq!(
            crashed,
            Err(TransferOperatorError::State("test abort crash"))
        );
        assert_eq!(
            loaded.source.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceAuthoritative
        );
        drop(loaded);

        let mut replay = load_transfer(&sqlite, &bundle, &state_dir)?;
        assert_eq!(replay.generation.phase, OperatorPhaseV1::Aborting);
        revalidate_abort_source(&replay)?;
        let status = replay.source.source_transfer_status()?;
        assert_eq!(status.last_aborted_bundle_hash(), Some(expected_bundle));
        assert_eq!(
            status.last_aborted_target_fingerprint(),
            Some(expected_target)
        );
        let result = abort_loaded(&mut replay, &mut destination, || Ok(()))?;
        assert_eq!(result.phase, "source_authoritative");
        Ok(())
    }

    #[test]
    fn finalize_replays_after_authority_handoff_before_terminal_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, sqlite, state_dir, _backup, bundle) = begun_fixture("finalize-replay")?;
        let _keep_temporary = temporary;
        let mut loaded = load_transfer(&sqlite, &bundle, &state_dir)?;
        let mut destination = MemoryDestination::default();
        while loaded.session.next_ordinal() < loaded.bundle.records().len() {
            let start = loaded.session.next_ordinal();
            let chunk = loaded.bundle.chunk(start, loaded.bundle.records().len())?;
            loaded.session.apply_chunk(&mut destination, &chunk)?;
        }
        loaded.generation = persist_generation(
            &loaded.state_dir,
            Some(loaded.generation.sequence),
            OperatorPhaseV1::ChunksComplete,
            &loaded.binding,
            &loaded.bundle,
            &loaded.session,
        )?;
        revalidate_finalization_source(&loaded)?;
        let crashed = finalize_loaded(&mut loaded, &mut destination, || {
            Err(TransferOperatorError::State("test finalize crash"))
        });
        assert_eq!(
            crashed,
            Err(TransferOperatorError::State("test finalize crash"))
        );
        assert_eq!(
            loaded.source.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceRetired
        );
        drop(loaded);

        let mut replay = load_transfer(&sqlite, &bundle, &state_dir)?;
        assert_eq!(replay.generation.phase, OperatorPhaseV1::Finalizing);
        revalidate_finalization_source(&replay)?;
        let result = finalize_loaded(&mut replay, &mut destination, || Ok(()))?;
        assert_eq!(result.phase, "target_authoritative");
        assert!(destination.authoritative);
        Ok(())
    }

    #[test]
    fn operation_lock_fails_fast_under_contention_and_recovers_after_drop()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state_dir = prepare_data_directory(&temporary.path().join("transfer-state"))?;
        let first = OperationLockV1::acquire(&state_dir)?;
        assert!(matches!(
            OperationLockV1::acquire(&state_dir),
            Err(TransferOperatorError::State(
                "operation already in progress"
            ))
        ));
        let lock = state_dir.join(super::OPERATION_LOCK_FILE);
        let held = state_dir.join("held-operation.lock");
        fs::rename(&lock, &held)?;
        let replacement = create_owner_only_file(&lock)?;
        replacement.sync_all()?;
        drop(replacement);
        assert_eq!(
            first.revalidate(),
            Err(TransferOperatorError::State("operation lock identity"))
        );
        assert!(matches!(
            OperationLockV1::acquire(&state_dir),
            Err(TransferOperatorError::State(
                "operation already in progress"
            ))
        ));
        drop(first);
        let _recovered = OperationLockV1::acquire(&state_dir)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn begin_rejects_a_logically_modified_verified_backup_digest()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let state_dir = prepare_data_directory(&temporary.path().join("transfer-state"))?;
        let backup = state_dir.join("source.backup.sqlite3");
        let bundle = state_dir.join("transfer.bundle");
        drop(initialized_source(&sqlite)?);

        let result = begin_transfer_with_hooks(
            &sqlite,
            &backup,
            &bundle,
            &state_dir,
            "backup-digest-mutation",
            || Ok(()),
            || {
                fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))
                    .map_err(|_| TransferOperatorError::State("test backup mutation"))?;
                let connection = rusqlite::Connection::open(&backup)
                    .map_err(|_| TransferOperatorError::State("test backup mutation"))?;
                connection
                    .execute_batch(
                        "CREATE TABLE digest_only_injection(value TEXT) STRICT;\
                         INSERT INTO digest_only_injection VALUES ('not in persisted digest');",
                    )
                    .map_err(|_| TransferOperatorError::State("test backup mutation"))
            },
        );

        assert_eq!(
            result,
            Err(TransferOperatorError::Source(
                "persisted backup digest mismatch"
            ))
        );
        assert!(!bundle.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn begin_rejects_injected_backup_wal_sidecars_without_removing_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let state_dir = prepare_data_directory(&temporary.path().join("transfer-state"))?;
        let backup = state_dir.join("source.backup.sqlite3");
        let bundle = state_dir.join("transfer.bundle");
        let wal = backup.with_file_name(format!(
            "{}-wal",
            backup
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("backup name")?
        ));
        let shm = backup.with_file_name(format!(
            "{}-shm",
            backup
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("backup name")?
        ));
        drop(initialized_source(&sqlite)?);
        let mut injected_writer = None;

        let result = begin_transfer_with_hooks(
            &sqlite,
            &backup,
            &bundle,
            &state_dir,
            "backup-wal-injection",
            || Ok(()),
            || {
                fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))
                    .map_err(|_| TransferOperatorError::State("test backup WAL injection"))?;
                let connection = rusqlite::Connection::open(&backup)
                    .map_err(|_| TransferOperatorError::State("test backup WAL injection"))?;
                connection
                    .execute_batch(
                        "PRAGMA journal_mode=WAL;\
                         CREATE TABLE wal_only_injection(value TEXT) STRICT;\
                         INSERT INTO wal_only_injection VALUES ('unadmitted WAL bytes');\
                         BEGIN IMMEDIATE;\
                         INSERT INTO wal_only_injection VALUES ('uncommitted WAL bytes');",
                    )
                    .map_err(|_| TransferOperatorError::State("test backup WAL injection"))?;
                injected_writer = Some(connection);
                Ok(())
            },
        );

        assert!(matches!(result, Err(TransferOperatorError::Source(_))));
        assert!(wal.is_file());
        assert!(shm.is_file());
        assert!(!bundle.exists());
        drop(injected_writer);
        Ok(())
    }

    #[test]
    fn owner_only_publication_never_clobbers_an_existing_destination()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state_dir = prepare_data_directory(&temporary.path().join("transfer-state"))?;
        let destination = state_dir.join("generation.json");
        let mut existing = create_owner_only_file(&destination)?;
        existing.write_all(b"existing")?;
        existing.sync_all()?;
        drop(existing);

        assert!(publish_owner_only(&destination, b"replacement", "test publication").is_err());
        assert_eq!(fs::read(&destination)?, b"existing");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_publication_rejects_a_swapped_private_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state_dir = prepare_data_directory(&temporary.path().join("transfer-state"))?;
        let destination = state_dir.join("generation.json");
        let held = state_dir.join("held-original.partial");
        let result = publish_owner_only_with_hook(
            &destination,
            b"admitted generation",
            "test publication",
            |partial| {
                if partial.exists() {
                    fs::rename(partial, &held)
                        .map_err(|_| TransferOperatorError::State("test staging swap"))?;
                }
                let mut replacement = create_owner_only_file(partial)
                    .map_err(|_| TransferOperatorError::State("test staging swap"))?;
                replacement
                    .write_all(b"substituted generation")
                    .and_then(|()| replacement.sync_all())
                    .map_err(|_| TransferOperatorError::State("test staging swap"))?;
                Ok(())
            },
        );

        assert_eq!(
            result,
            Err(TransferOperatorError::State("test publication"))
        );
        assert!(
            !destination.exists() || fs::metadata(&destination)?.len() == 0,
            "failed publication left attacker-controlled bytes at the final path"
        );
        if held.exists() {
            assert_eq!(fs::metadata(&held)?.len(), 0);
        }
        assert_eq!(
            fs::read(
                state_dir.join(
                    fs::read_dir(&state_dir)?
                        .filter_map(Result::ok)
                        .map(|entry| entry.file_name())
                        .find(|name| name.to_string_lossy().contains(".generation.json."))
                        .ok_or("replacement private name")?
                )
            )?,
            b"substituted generation"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_publication_rejects_destination_parent_substitution()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let admitted_parent = prepare_data_directory(&temporary.path().join("admitted"))?;
        let held_parent = temporary.path().join("held-admitted");
        let destination = admitted_parent.join("generation.json");

        let result = publish_owner_only_with_hook(
            &destination,
            b"admitted generation",
            "test publication",
            |_| {
                fs::rename(&admitted_parent, &held_parent)
                    .map_err(|_| TransferOperatorError::State("test parent substitution"))?;
                prepare_data_directory(&admitted_parent)
                    .map_err(|_| TransferOperatorError::State("test parent substitution"))?;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(
            !destination.exists() || fs::metadata(&destination)?.len() == 0,
            "publication reached the replacement parent"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_cleanup_never_unlinks_a_substituted_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let state_dir = prepare_data_directory(&temporary.path().join("transfer-state"))?;
        let partial = state_dir.join("partial");
        let held = state_dir.join("held-original");
        let replacement = b"same-owner replacement";
        let mut original = create_owner_only_file(&partial)?;
        original.write_all(b"admitted")?;
        original.sync_all()?;
        let identity = operator_file_identity(&original)?;

        let result = remove_named_identity_with_hook(&partial, identity, || {
            fs::rename(&partial, &held)?;
            let mut installed = create_owner_only_file(&partial).map_err(std::io::Error::other)?;
            installed.write_all(replacement)?;
            installed.sync_all()
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&partial)?, replacement);
        Ok(())
    }

    #[test]
    fn terminal_resume_hints_never_return_local_success() {
        for phase in [
            OperatorPhaseV1::TargetAuthoritative,
            OperatorPhaseV1::SourceAuthoritative,
        ] {
            assert!(require_resumable_phase(phase).is_err());
        }
    }

    #[test]
    fn unknown_state_artifact_is_rejected_before_resume() -> Result<(), Box<dyn std::error::Error>>
    {
        let (temporary, sqlite, state_dir, _backup, bundle) = begun_fixture("unknown-artifact")?;
        let _keep_temporary = temporary;
        let unexpected = state_dir.join("unexpected-state.json");
        let mut file = create_owner_only_file(&unexpected)?;
        file.write_all(b"{}")?;
        file.sync_all()?;
        drop(file);

        assert!(matches!(
            load_transfer(&sqlite, &bundle, &state_dir),
            Err(TransferOperatorError::State("unknown state artifact"))
        ));
        Ok(())
    }

    #[test]
    fn persisted_backup_with_same_name_outside_state_directory_is_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, sqlite, state_dir, backup, bundle) = begun_fixture("backup-path-swap")?;
        let _keep_temporary = temporary;
        let outside = state_dir
            .parent()
            .ok_or("state directory parent")?
            .join("outside");
        prepare_data_directory(&outside)?;
        let substituted = outside.join(backup.file_name().ok_or("backup filename")?);
        fs::copy(&backup, &substituted)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&substituted, fs::Permissions::from_mode(0o600))?;
        }
        let connection = rusqlite::Connection::open(&sqlite)?;
        connection.execute(
            "UPDATE source_transfer_lifecycle SET backup_path = ?1 WHERE lifecycle_id = 1",
            [substituted.to_str().ok_or("substituted path")?],
        )?;
        drop(connection);

        assert!(load_transfer(&sqlite, &bundle, &state_dir).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn persisted_backup_identity_rejects_a_same_byte_replacement_before_retention()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, sqlite, state_dir, backup, bundle) =
            begun_fixture("backup-identity-replacement")?;
        let held = temporary.path().join("held-genuine-backup.sqlite3");
        let victim = temporary.path().join("replacement-victim.sqlite3");
        let _keep_temporary = temporary;
        fs::copy(&backup, &victim)?;
        let victim_bytes = fs::read(&victim)?;
        fs::rename(&backup, &held)?;
        fs::rename(&victim, &backup)?;

        let loaded = load_transfer(&sqlite, &bundle, &state_dir)?;
        assert_eq!(
            revalidate_pending_source(&loaded),
            Err(TransferOperatorError::Source("persisted backup identity"))
        );
        assert_eq!(fs::read(&backup)?, victim_bytes);
        assert_eq!(fs::read(&held)?, victim_bytes);
        Ok(())
    }

    #[test]
    fn noncanonical_generation_encoding_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, sqlite, state_dir, _backup, bundle) = begun_fixture("noncanonical-state")?;
        let _keep_temporary = temporary;
        let generation_name = generation_files(&state_dir)?
            .into_iter()
            .next()
            .ok_or("generation file")?;
        let generation = state_dir.join(generation_name);
        let mut bytes = fs::read(&generation)?;
        bytes.push(b'\n');
        fs::write(&generation, bytes)?;

        assert_eq!(
            load_transfer(&sqlite, &bundle, &state_dir).map(|_| ()),
            Err(TransferOperatorError::State(
                "noncanonical generation encoding"
            ))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn generation_reader_rejects_a_named_path_substitution_after_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, _sqlite, state_dir, _backup, _bundle) =
            begun_fixture("generation-path-race")?;
        let _keep_temporary = temporary;
        let generation_name = generation_files(&state_dir)?
            .into_iter()
            .next()
            .ok_or("generation file")?;
        let generation = state_dir.join(generation_name);
        let admitted = fs::read(&generation)?;
        let held = state_dir.join("held-generation");
        let result = read_bounded_with_hook(&generation, MAX_STATE_BYTES, || {
            fs::rename(&generation, &held)
                .map_err(|_| TransferOperatorError::State("test generation swap"))?;
            let mut replacement = create_owner_only_file(&generation)
                .map_err(|_| TransferOperatorError::State("test generation swap"))?;
            replacement
                .write_all(&admitted)
                .and_then(|()| replacement.sync_all())
                .map_err(|_| TransferOperatorError::State("test generation swap"))?;
            Ok(())
        });

        assert_eq!(
            result,
            Err(TransferOperatorError::State("state input identity changed"))
        );
        Ok(())
    }

    #[test]
    fn legacy_bundle_encoding_is_rejected_in_confirm_and_load()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, sqlite, state_dir, _backup, bundle_path) = begun_fixture("legacy-bundle")?;
        let _keep_temporary = temporary;
        let canonical = TransferBundleV1::from_bytes(&fs::read(&bundle_path)?)?;
        let legacy = legacy_bundle_bytes(&canonical)?;
        assert_eq!(TransferBundleV1::from_bytes(&legacy)?, canonical);
        assert_ne!(legacy, canonical.to_bytes()?);
        fs::write(&bundle_path, legacy)?;

        assert_eq!(
            publish_or_confirm_bundle(&bundle_path, &canonical),
            Err(TransferOperatorError::Bundle(
                "noncanonical bundle encoding"
            ))
        );
        assert_eq!(
            load_transfer(&sqlite, &bundle_path, &state_dir).map(|_| ()),
            Err(TransferOperatorError::Bundle(
                "noncanonical bundle encoding"
            ))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bundle_reader_rejects_a_named_path_substitution_after_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let (temporary, _sqlite, state_dir, _backup, bundle_path) =
            begun_fixture("bundle-path-race")?;
        let _keep_temporary = temporary;
        let admitted = fs::read(&bundle_path)?;
        let held = state_dir.join("held-bundle");
        let result = read_canonical_bundle_with_hook(&bundle_path, || {
            fs::rename(&bundle_path, &held)
                .map_err(|_| TransferOperatorError::Bundle("test bundle swap"))?;
            let mut replacement = create_owner_only_file(&bundle_path)
                .map_err(|_| TransferOperatorError::Bundle("test bundle swap"))?;
            replacement
                .write_all(&admitted)
                .and_then(|()| replacement.sync_all())
                .map_err(|_| TransferOperatorError::Bundle("test bundle swap"))?;
            Ok(())
        });

        assert_eq!(
            result,
            Err(TransferOperatorError::Bundle(
                "bounded bundle identity changed"
            ))
        );
        Ok(())
    }

    fn legacy_bundle_bytes(
        bundle: &TransferBundleV1,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut output = b"WSTRANS1".to_vec();
        output.extend_from_slice(&2_u16.to_le_bytes());
        test_write_string(&mut output, bundle.bundle_id())?;
        test_write_string(&mut output, bundle.lineage_id())?;
        output.extend_from_slice(&bundle.source_epoch().to_le_bytes());
        output.push(test_profile_tag(bundle.source_profile()));
        output.push(test_profile_tag(bundle.target_profile()));
        test_write_backend(
            &mut output,
            bundle.source_backend().ok_or("source backend")?,
        )?;
        test_write_backend(&mut output, bundle.target_backend())?;
        let pack = bundle.pack().ok_or("pack identity")?;
        test_write_string(&mut output, pack.pack_id())?;
        test_write_string(&mut output, pack.revision())?;
        output.extend_from_slice(&pack.digest().as_bytes());
        test_write_count(&mut output, bundle.resources().len())?;
        for resource in bundle.resources() {
            output.push(test_resource_tag(resource.kind()));
            test_write_string(&mut output, resource.identity())?;
            output.extend_from_slice(&resource.size_bytes().to_le_bytes());
            output.extend_from_slice(&resource.digest().as_bytes());
        }
        output.push(1);
        test_write_count(&mut output, bundle.records().len())?;
        for record in bundle.records() {
            test_write_record(&mut output, record)?;
        }
        Ok(output)
    }

    fn test_write_backend(
        output: &mut Vec<u8>,
        backend: &BackendFingerprintV1,
    ) -> Result<(), Box<dyn std::error::Error>> {
        output.push(test_profile_tag(backend.profile()));
        test_write_string(output, backend.engine_identity())?;
        let schema = backend.schema();
        test_write_string(output, schema.logical_history_id())?;
        output.extend_from_slice(&schema.schema_contract_fingerprint().as_bytes());
        test_write_count(output, schema.migrations().len())?;
        for migration in schema.migrations() {
            output.extend_from_slice(&migration.version().to_le_bytes());
            test_write_string(output, migration.migration_id())?;
            output.extend_from_slice(&migration.checksum().as_bytes());
        }
        Ok(())
    }

    fn test_write_record(
        output: &mut Vec<u8>,
        record: &LogicalRecordV1,
    ) -> Result<(), Box<dyn std::error::Error>> {
        output.extend_from_slice(&record.ordinal().to_le_bytes());
        let (class, kind) = match record.kind() {
            RecordKindV1::Canonical(kind) => (1, kind as u8 + 1),
            RecordKindV1::Derived(kind) => (2, kind as u8 + 1),
        };
        output.extend_from_slice(&[class, kind]);
        test_write_string(output, record.identity())?;
        output.extend_from_slice(&u64::try_from(record.bytes().len())?.to_le_bytes());
        output.extend_from_slice(record.bytes());
        output.extend_from_slice(&record.digest().as_bytes());
        Ok(())
    }

    fn test_write_string(
        output: &mut Vec<u8>,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        output.extend_from_slice(&u32::try_from(value.len())?.to_le_bytes());
        output.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn test_write_count(
        output: &mut Vec<u8>,
        value: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        output.extend_from_slice(&u32::try_from(value)?.to_le_bytes());
        Ok(())
    }

    const fn test_profile_tag(profile: BundleProfileV1) -> u8 {
        match profile {
            BundleProfileV1::SqliteBundled => 1,
            BundleProfileV1::PostgresPrimary17 => 2,
        }
    }

    const fn test_resource_tag(kind: ResourceKindV1) -> u8 {
        match kind {
            ResourceKindV1::Artifact => 1,
            ResourceKindV1::Codec => 2,
            ResourceKindV1::Schema => 3,
        }
    }

    fn initialized_source(
        path: &std::path::Path,
    ) -> Result<SqliteRoomStore, Box<dyn std::error::Error>> {
        let store = SqliteRoomStore::open(path)?;
        store.initialize_canonical_metadata("deployment/operator-test", 9)?;
        let pack = PackIdentityV1::new(
            "operator-test-pack",
            "v1",
            DigestV1::hash(b"operator-test-pack-v1"),
        )?;
        store.initialize_deployment_identity(DeploymentIdentityV1::new(vec![pack], vec![])?)?;
        Ok(store)
    }

    type BegunFixture = (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    );

    fn begun_fixture(transfer_id: &str) -> Result<BegunFixture, Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let state_dir = temporary.path().join("transfer-state");
        prepare_data_directory(&state_dir)?;
        let backup = state_dir.join("source.backup.sqlite3");
        let bundle = state_dir.join("transfer.bundle");
        drop(initialized_source(&sqlite)?);
        begin_transfer(&sqlite, &backup, &bundle, &state_dir, transfer_id)?;
        Ok((temporary, sqlite, state_dir, backup, bundle))
    }

    #[test]
    fn pending_source_exports_a_manifest_stream_without_a_bundle_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let state_dir = temporary.path().join("transfer-state");
        prepare_data_directory(&state_dir)?;
        let backup = state_dir.join("source.backup.sqlite3");
        let stream = state_dir.join("source.stream");
        let store = initialized_source(&sqlite)?;
        let pending = store.begin_source_transfer(&backup)?;
        assert_eq!(
            pending.state(),
            SqliteSourceTransferStateV1::TransferPending
        );

        let result = export_pending_stream_v2(
            &store,
            &stream,
            "stream/operator-export",
            TransferStreamLimitsV2 {
                max_records_per_chunk: 2,
                max_chunk_bytes: 4096,
                max_record_bytes: 2048,
                ..TransferStreamLimitsV2::default()
            },
        )?;
        assert_eq!(
            result.schema,
            "worldstream/transfer-stream-export-result/v2"
        );
        assert!(stream.exists());
        validate_owner_only_file(&stream)?;
        assert_eq!(
            store.source_transfer_status()?.state(),
            SqliteSourceTransferStateV1::TransferPending
        );
        let file = fs::File::open(&stream)?;
        let mut reader = TransferStreamReaderV2::new(
            file,
            TransferStreamLimitsV2 {
                max_records_per_chunk: 2,
                max_chunk_bytes: 4096,
                max_record_bytes: 2048,
                ..TransferStreamLimitsV2::default()
            },
        )?;
        let manifest = reader.manifest().ok_or("stream manifest")?;
        assert_eq!(
            manifest.source_backup_digest().to_string(),
            result.backup_digest
        );
        let mut count = 0_u64;
        while let Some(chunk) = reader.next_chunk()? {
            count += u64::try_from(chunk.records().len())?;
        }
        assert_eq!(count, result.record_count);
        assert_eq!(
            reader.footer().ok_or("stream footer")?.record_bytes(),
            result.record_bytes
        );
        Ok(())
    }

    #[test]
    fn verified_stream_source_cursor_reopens_at_the_next_exact_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let sqlite = temporary.path().join("source.sqlite3");
        let state_dir = temporary.path().join("transfer-state");
        prepare_data_directory(&state_dir)?;
        let backup = state_dir.join("source.backup.sqlite3");
        let store = initialized_source(&sqlite)?;
        let pending = store.begin_source_transfer(&backup)?;
        let backup_digest = pending.backup_digest().ok_or("source backup digest")?;
        let source_epoch = pending.source_epoch().ok_or("source epoch")?;
        let target_backend = postgres_backend_fingerprint()?;
        let source_backend = BackendFingerprintV1::new(
            BundleProfileV1::SqliteBundled,
            "sqlite-bundled",
            target_backend.schema().clone(),
        )?;
        let identity = TransferStreamIdentityV2::new(
            "stream/operator-cursor",
            store.deployment_lineage()?,
            source_epoch,
            BundleProfileV1::SqliteBundled,
            BundleProfileV1::PostgresPrimary17,
        )?;
        let mut first = SqliteTransferStreamSourceV2::open(
            &backup,
            backup_digest,
            &identity,
            source_backend.clone(),
            target_backend.clone(),
        )?;
        let manifest = first.manifest().clone();
        let mut prefix = Vec::new();
        for _ in 0..3 {
            prefix.push(first.next_stream_record()?.ok_or("stream prefix record")?);
        }
        let cursor = first.cursor().clone();
        assert_eq!(cursor.next_ordinal(), 3);

        let mut resumed = SqliteTransferStreamSourceV2::open_at_cursor(
            &backup,
            backup_digest,
            &identity,
            source_backend,
            target_backend,
            cursor,
        )?;
        assert_eq!(resumed.manifest(), &manifest);
        let mut expected_ordinal = 3_u64;
        while let Some(record) = resumed.next_stream_record()? {
            assert_eq!(record.ordinal(), expected_ordinal);
            expected_ordinal += 1;
        }
        assert_eq!(
            expected_ordinal,
            manifest.expected_record_count(),
            "a resumed cursor must cover the exact remaining manifest suffix"
        );
        assert_eq!(prefix.len(), 3);
        Ok(())
    }

    #[test]
    fn failed_stream_publication_never_exposes_the_final_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempdir()?;
        let destination = temporary.path().join("failed.stream");
        let result = publish_stream_owner_only(&destination, "test stream publication", |file| {
            file.write_all(b"partial stream bytes")
                .map_err(|_| TransferOperatorError::State("test stream write"))?;
            Err(TransferOperatorError::Bundle(
                "injected stream disk failure",
            ))
        });
        assert_eq!(
            result,
            Err(TransferOperatorError::Bundle(
                "injected stream disk failure"
            ))
        );
        assert!(!destination.exists());
        Ok(())
    }

    fn generation_files(path: &std::path::Path) -> std::io::Result<Vec<String>> {
        Ok(fs::read_dir(path)?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| {
                name.starts_with("transfer-state-v1-") && name.strip_suffix(".json").is_some()
            })
            .collect())
    }
}
