//! Offline Host Operator surface for exact Activity Pack Bundle lifecycle.

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr as _,
};

use serde::Serialize;
use thiserror::Error;
use worldstream_backup::native_sqlite::{NativeSqliteLimits, extract_restore_evidence};
use worldstream_core::{
    CanonicalJsonV1, CompleteHeadV1, CoreTraceV1, PackDigestV1, PackRegistryV1,
};
use worldstream_pack_bundle::{
    ApprovalDecisionV1, OperatorApprovalV1, PackBundleDigestV1, PackBundleErrorV1,
    PackBundleInspectionV1, PackBundleStoreV1, PackInstallStateV1, PackRemovalV1,
    PackStartupReadinessSealV1, RetainedRevisionSourceV1,
};
use worldstream_postgres::PostgresAdmin;
use worldstream_runtime::{
    StorageConfig, StorageProfile, create_owner_only_file, prepare_data_directory,
    validate_data_directory,
};
use worldstream_sqlite::{
    SqliteCanonicalMetadataStatusV1, SqliteRoomStore, SqliteSourceTransferStateV1,
};

use crate::{
    StartupPackRegistryV1, assemble_startup_pack_registry,
    pack_startup::{pack_deployment_binding, startup_pack_inventory_digest},
};

const RECEIPT_SCHEMA: &str = "worldstream/pack-operator-receipt/v1";
const READINESS_SNAPSHOT_PREFIX: &str = ".pack-restart-readiness-";

/// Closed operator failures without provider text, source bytes, or secrets.
#[derive(Debug, Error)]
pub enum PackOperatorErrorV1 {
    #[error("Activity Pack operator data directory was rejected")]
    DataDirectory,
    #[error("Activity Pack operator lifecycle failed closed")]
    Bundle(#[from] PackBundleErrorV1),
    #[error("Activity Pack startup readiness failed closed")]
    Readiness,
    #[error("Activity Pack retained Room reference proof failed closed")]
    References,
    #[error("Activity Pack export target was rejected")]
    Export,
    #[error("Activity Pack bundle digest was invalid")]
    Digest,
}

/// Exact bundle and semantic identity emitted by read-only inspection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackInspectionReceiptV1 {
    pub schema: &'static str,
    pub status: &'static str,
    pub operation: &'static str,
    pub pack_id: String,
    pub explanatory_version: String,
    pub bundle_digest: String,
    pub revision_digest: String,
    pub member_count: usize,
    pub byte_count: usize,
    pub approval_imported: bool,
}

/// Typed receipt for one local approval, installation, selection, or removal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackMutationReceiptV1 {
    pub schema: &'static str,
    pub status: &'static str,
    pub operation: &'static str,
    pub bundle_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_state: Option<PackInstallStateV1>,
    pub restart_required: bool,
    pub approval_transferred: bool,
}

/// One deterministic inventory row after complete original-byte verification.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, Serialize)]
pub struct PackInventoryEntryV1 {
    pub pack_id: String,
    pub explanatory_version: String,
    pub bundle_digest: String,
    pub revision_digest: String,
    pub install_state: PackInstallStateV1,
}

/// Complete bounded startup inventory receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackInventoryReceiptV1 {
    pub schema: &'static str,
    pub status: &'static str,
    pub operation: &'static str,
    pub installed: usize,
    pub selectable: usize,
    pub retained_only: usize,
    pub storage_profile: String,
    pub inventory_digest: String,
    pub entries: Vec<PackInventoryEntryV1>,
    pub restart_required_for_pending_changes: bool,
}

/// Production-host startup admission result over the exact frozen inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackReadinessReceiptV1 {
    pub schema: &'static str,
    pub status: &'static str,
    pub operation: &'static str,
    pub embedded_revisions: usize,
    pub installed_bundles: usize,
    pub installed_selectable: usize,
    pub installed_retained_only: usize,
    pub storage_profile: String,
    pub deployment_binding: String,
    pub inventory_digest: String,
    pub total_revisions: usize,
    pub original_bytes_reverified: bool,
    pub production_component_host_admission: bool,
    pub room_replay_checked: bool,
    pub rooms_replayed: usize,
    pub isolated_rooms_skipped: usize,
}

/// Byte-identical export receipt. Approval never enters the output bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackExportReceiptV1 {
    pub schema: &'static str,
    pub status: &'static str,
    pub operation: &'static str,
    pub bundle_digest: String,
    pub revision_digest: String,
    pub byte_count: usize,
    pub approval_exported: bool,
}

fn store(data_directory: &Path) -> Result<PackBundleStoreV1, PackOperatorErrorV1> {
    let directory =
        prepare_data_directory(data_directory).map_err(|_| PackOperatorErrorV1::DataDirectory)?;
    PackBundleStoreV1::open(directory.join("activity-packs")).map_err(Into::into)
}

fn inspection_receipt(
    operation: &'static str,
    inspection: PackBundleInspectionV1,
) -> PackInspectionReceiptV1 {
    PackInspectionReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation,
        pack_id: inspection.pack_id,
        explanatory_version: inspection.explanatory_version,
        bundle_digest: inspection.bundle_digest.to_string(),
        revision_digest: inspection.revision_digest.to_string(),
        member_count: inspection.member_count,
        byte_count: inspection.byte_count,
        approval_imported: false,
    }
}

/// Completely verifies an untrusted local candidate without installing it.
///
/// # Errors
///
/// Returns a closed directory, bounded-read, archive, or identity error.
pub fn inspect_pack(
    data_directory: &Path,
    candidate: &Path,
) -> Result<PackInspectionReceiptV1, PackOperatorErrorV1> {
    let inspection = store(data_directory)?.inspect_path(candidate)?;
    Ok(inspection_receipt("inspect", inspection))
}

/// Records an explicit local approval for the candidate's exact physical
/// digest. The candidate is fully reverified before the decision is stored.
///
/// # Errors
///
/// Returns a closed verification, operator-field, or durable-write error.
pub fn approve_pack(
    data_directory: &Path,
    candidate: &Path,
    operator_id: String,
    decided_at: String,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    let store = store(data_directory)?;
    let digest = store.approve_path(
        candidate,
        OperatorApprovalV1 {
            operator_id,
            decided_at,
            decision: ApprovalDecisionV1::Approved,
        },
    )?;
    Ok(PackMutationReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "approve",
        bundle_digest: digest.to_string(),
        revision_digest: None,
        install_state: None,
        restart_required: false,
        approval_transferred: false,
    })
}

/// Revokes local approval for one installed exact bundle and makes it
/// retained-only. The change affects new-Room selection after restart.
///
/// # Errors
///
/// Returns a closed digest, installed-object, or durable-write error.
pub fn revoke_pack(
    data_directory: &Path,
    digest: &str,
    operator_id: String,
    decided_at: String,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    let digest = parse_digest(digest)?;
    let store = store(data_directory)?;
    let bundle = store.load_installed(&digest)?;
    let revision_digest = bundle.revision_digest().to_string();
    store.record_approval(
        &bundle,
        OperatorApprovalV1 {
            operator_id,
            decided_at,
            decision: ApprovalDecisionV1::Revoked,
        },
    )?;
    Ok(PackMutationReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "revoke",
        bundle_digest: digest.to_string(),
        revision_digest: Some(revision_digest),
        install_state: Some(PackInstallStateV1::RetainedOnly),
        restart_required: true,
        approval_transferred: false,
    })
}

/// Installs an already-approved candidate as retained-only inventory.
///
/// # Errors
///
/// Returns a closed verification, approval, substitution, or write error.
pub fn install_pack(
    data_directory: &Path,
    candidate: &Path,
    installed_at: String,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    let installed = store(data_directory)?.install_approved(candidate, installed_at)?;
    Ok(PackMutationReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "install",
        bundle_digest: installed.bundle_digest.to_string(),
        revision_digest: Some(installed.revision_digest.to_string()),
        install_state: Some(installed.install_state),
        restart_required: true,
        approval_transferred: false,
    })
}

/// Lists the complete bounded, digest-sorted, fully reverified inventory.
///
/// # Errors
///
/// Returns a closed inventory, approval, original-byte, or directory error.
pub fn inventory(
    data_directory: &Path,
    storage: &StorageConfig,
) -> Result<PackInventoryReceiptV1, PackOperatorErrorV1> {
    let inventory = store(data_directory)?.load_startup_inventory()?;
    let counts = inventory.counts();
    let inventory_digest =
        startup_pack_inventory_digest(&inventory).map_err(|_| PackOperatorErrorV1::Readiness)?;
    let entries = inventory
        .entries()
        .iter()
        .map(|entry| PackInventoryEntryV1 {
            pack_id: entry.bundle().descriptor().pack_id.clone(),
            explanatory_version: entry.bundle().descriptor().explanatory_version.clone(),
            bundle_digest: entry.installed().bundle_digest.to_string(),
            revision_digest: entry.installed().revision_digest.to_string(),
            install_state: entry.installed().install_state,
        })
        .collect();
    Ok(PackInventoryReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "inventory",
        installed: counts.installed,
        selectable: counts.selectable,
        retained_only: counts.retained_only,
        storage_profile: storage.profile.as_str().to_owned(),
        inventory_digest,
        entries,
        restart_required_for_pending_changes: true,
    })
}

/// Changes only new-Room selectability for the next daemon startup.
///
/// # Errors
///
/// Returns a closed digest, approval, installed-object, or write error.
pub fn set_pack_selectable(
    data_directory: &Path,
    digest: &str,
    selectable: bool,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    let digest = parse_digest(digest)?;
    let installed = store(data_directory)?.set_selectable(&digest, selectable)?;
    Ok(PackMutationReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "set_selectable",
        bundle_digest: installed.bundle_digest.to_string(),
        revision_digest: Some(installed.revision_digest.to_string()),
        install_state: Some(installed.install_state),
        restart_required: true,
        approval_transferred: false,
    })
}

/// Rebuilds the production startup registry and replays every healthy Room in
/// the default `SQLite` database from exact Genesis/Transition bytes. Existing
/// faulted or quarantined Rooms remain isolated and are reported, never
/// promoted. An absent database is a verified empty deployment.
///
/// This is an offline mutating preflight: it opens the configured database
/// through the production [`SqliteRoomStore`] path, which may apply a supported
/// schema migration before the baseline is captured. Replay itself reads only
/// a verified standalone snapshot. From that baseline onward, readiness must
/// preserve the source's authority, native identity, and canonical semantics.
/// Secure snapshot cleanup deliberately retains one private directory and a
/// bounded set of zero-byte identity-bound markers; pathname deletion would
/// reintroduce a same-owner substitution race. Successful completion
/// separately writes the exact startup-readiness seal under Activity Pack
/// operator state.
///
/// # Errors
///
/// Returns a closed inventory, Component admission, native evidence, retained
/// executor, canonical materialization, or executable Replay error.
pub fn verify_pack_restart_readiness_sqlite(
    data_directory: &Path,
    database: &Path,
    storage: &StorageConfig,
) -> Result<PackReadinessReceiptV1, PackOperatorErrorV1> {
    if storage.profile != StorageProfile::SqliteBundled {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let database = exact_sqlite_database_target(data_directory, database)?;
    let startup = startup_registry(data_directory)?;
    let replay = verify_sqlite_pack_replays_from_live_database(
        data_directory,
        &database,
        startup.registry(),
    )?;
    let deployment_binding = sqlite_deployment_binding(data_directory, storage, &replay)?;
    readiness_receipt(
        data_directory,
        &startup,
        storage.profile,
        deployment_binding,
        replay.rooms_replayed,
        replay.isolated_rooms_skipped,
    )
}

fn exact_sqlite_database_target(
    data_directory: &Path,
    database: &Path,
) -> Result<std::path::PathBuf, PackOperatorErrorV1> {
    let directory =
        prepare_data_directory(data_directory).map_err(|_| PackOperatorErrorV1::DataDirectory)?;
    if database.file_name().and_then(|name| name.to_str()) != Some("worldstream.sqlite3") {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let parent = database.parent().ok_or(PackOperatorErrorV1::Readiness)?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| PackOperatorErrorV1::Readiness)?;
    if canonical_parent != directory {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let target = directory.join("worldstream.sqlite3");
    if target.exists()
        && fs::symlink_metadata(&target)
            .map_err(|_| PackOperatorErrorV1::Readiness)?
            .file_type()
            .is_symlink()
    {
        return Err(PackOperatorErrorV1::Readiness);
    }
    Ok(target)
}

/// Rebuilds the production startup registry and verifies all healthy Rooms
/// through `PostgreSQL`'s repeatable-read executable Replay preflight.
///
/// # Errors
///
/// Returns a closed inventory, Component admission, provider, bound,
/// canonical-data, or executable Replay error.
pub fn verify_pack_restart_readiness_postgres(
    data_directory: &Path,
    admin: &PostgresAdmin,
    storage: &StorageConfig,
) -> Result<PackReadinessReceiptV1, PackOperatorErrorV1> {
    if storage.profile != StorageProfile::PostgresPrimary {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let startup = startup_registry(data_directory)?;
    let summary = admin
        .verify_all_executable_pack_replays(startup.registry())
        .map_err(|_| PackOperatorErrorV1::Readiness)?;
    let metadata = admin
        .deployment_metadata_status()
        .map_err(|_| PackOperatorErrorV1::Readiness)?;
    let lineage = metadata
        .deployment_lineage_bytes
        .as_deref()
        .ok_or(PackOperatorErrorV1::Readiness)?;
    let epoch_bytes = metadata
        .storage_epoch_bytes
        .as_deref()
        .ok_or(PackOperatorErrorV1::Readiness)?;
    let epoch = u64::try_from(
        metadata
            .storage_epoch
            .ok_or(PackOperatorErrorV1::Readiness)?,
    )
    .ok()
    .filter(|value| *value > 0)
    .ok_or(PackOperatorErrorV1::Readiness)?;
    let deployment_binding = pack_deployment_binding(
        data_directory,
        storage.profile,
        Some(lineage),
        Some(epoch_bytes),
        Some(epoch),
    )
    .map_err(|_| PackOperatorErrorV1::Readiness)?;
    readiness_receipt(
        data_directory,
        &startup,
        storage.profile,
        deployment_binding,
        summary.rooms_replayed,
        summary.isolated_rooms_skipped,
    )
}

fn startup_registry(data_directory: &Path) -> Result<StartupPackRegistryV1, PackOperatorErrorV1> {
    let directory =
        prepare_data_directory(data_directory).map_err(|_| PackOperatorErrorV1::DataDirectory)?;
    assemble_startup_pack_registry(&directory).map_err(|_| PackOperatorErrorV1::Readiness)
}

fn readiness_receipt(
    data_directory: &Path,
    startup: &StartupPackRegistryV1,
    storage_profile: StorageProfile,
    deployment_binding: String,
    rooms_replayed: usize,
    isolated_rooms_skipped: usize,
) -> Result<PackReadinessReceiptV1, PackOperatorErrorV1> {
    let inventory = store(data_directory)?.load_startup_inventory()?;
    let observed_inventory_digest =
        startup_pack_inventory_digest(&inventory).map_err(|_| PackOperatorErrorV1::Readiness)?;
    if observed_inventory_digest != startup.inventory_digest() {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let seal = PackStartupReadinessSealV1::new(
        observed_inventory_digest.clone(),
        storage_profile.as_str().to_owned(),
        deployment_binding.clone(),
    )?;
    store(data_directory)?.record_startup_readiness(&seal)?;
    let diagnostics = startup.diagnostics();
    Ok(PackReadinessReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "ready",
        operation: "restart_readiness",
        embedded_revisions: diagnostics.embedded_revisions,
        installed_bundles: diagnostics.installed_bundles,
        installed_selectable: diagnostics.installed_selectable,
        installed_retained_only: diagnostics.installed_retained_only,
        storage_profile: storage_profile.as_str().to_owned(),
        deployment_binding,
        inventory_digest: observed_inventory_digest,
        total_revisions: diagnostics.total_revisions,
        original_bytes_reverified: true,
        production_component_host_admission: true,
        room_replay_checked: true,
        rooms_replayed,
        isolated_rooms_skipped,
    })
}

struct SqlitePackReplaySummaryV1 {
    rooms_replayed: usize,
    isolated_rooms_skipped: usize,
    deployment_lineage: Option<String>,
    storage_epoch: Option<u64>,
}

fn verify_sqlite_pack_replays_from_live_database(
    data_directory: &Path,
    database: &Path,
    registry: &PackRegistryV1,
) -> Result<SqlitePackReplaySummaryV1, PackOperatorErrorV1> {
    verify_sqlite_pack_replays_from_live_database_with(
        data_directory,
        database,
        registry,
        verify_sqlite_pack_replays,
    )
}

fn verify_sqlite_pack_replays_from_live_database_with<F>(
    data_directory: &Path,
    database: &Path,
    registry: &PackRegistryV1,
    verifier: F,
) -> Result<SqlitePackReplaySummaryV1, PackOperatorErrorV1>
where
    F: FnOnce(&Path, &PackRegistryV1) -> Result<SqlitePackReplaySummaryV1, PackOperatorErrorV1>,
{
    if !database.exists() {
        return verifier(database, registry);
    }
    let source = SqliteRoomStore::open(database).map_err(|_| PackOperatorErrorV1::Readiness)?;
    if source.source_transfer_state() != SqliteSourceTransferStateV1::SourceAuthoritative {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let source_identity = source.database_identity();
    let source_metadata = source
        .canonical_metadata_status()
        .map_err(|_| PackOperatorErrorV1::Readiness)?;
    let snapshot = ReadinessSnapshotV1::create(data_directory)?;
    let receipt = source
        .create_restart_readiness_snapshot(snapshot.path())
        .map_err(|_| PackOperatorErrorV1::Readiness)?;
    let replay = verifier(snapshot.path(), registry);
    if source
        .scrub_restart_readiness_snapshot(snapshot.path(), &receipt)
        .is_err()
    {
        return Err(PackOperatorErrorV1::Readiness);
    }
    if source.database_identity() != source_identity
        || source.source_transfer_state() != SqliteSourceTransferStateV1::SourceAuthoritative
        || source
            .canonical_metadata_status()
            .map_err(|_| PackOperatorErrorV1::Readiness)?
            != source_metadata
    {
        snapshot.verify_scrubbed_marker()?;
        return Err(PackOperatorErrorV1::Readiness);
    }
    snapshot.verify_scrubbed_marker()?;
    let replay = replay?;
    if !replay_matches_live_metadata(&replay, source_metadata.as_ref()) {
        return Err(PackOperatorErrorV1::Readiness);
    }
    Ok(replay)
}

struct ReadinessSnapshotV1 {
    directory: PathBuf,
    path: PathBuf,
}

impl ReadinessSnapshotV1 {
    fn create(data_directory: &Path) -> Result<Self, PackOperatorErrorV1> {
        const HEX: &[u8; 16] = b"0123456789abcdef";

        let parent = prepare_data_directory(data_directory)
            .map_err(|_| PackOperatorErrorV1::DataDirectory)?;
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| PackOperatorErrorV1::Readiness)?;
        let mut nonce = String::with_capacity(random.len().saturating_mul(2));
        for byte in random {
            nonce.push(char::from(HEX[usize::from(byte >> 4)]));
            nonce.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        let directory =
            prepare_data_directory(&parent.join(format!("{READINESS_SNAPSHOT_PREFIX}{nonce}")))
                .map_err(|_| PackOperatorErrorV1::DataDirectory)?;
        let path = directory.join("snapshot.sqlite3");
        Ok(Self { directory, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn verify_scrubbed_marker(&self) -> Result<(), PackOperatorErrorV1> {
        if validate_data_directory(&self.directory).map_err(|_| PackOperatorErrorV1::Readiness)?
            != self.directory
        {
            return Err(PackOperatorErrorV1::Readiness);
        }
        let entries = fs::read_dir(&self.directory)
            .map_err(|_| PackOperatorErrorV1::Readiness)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PackOperatorErrorV1::Readiness)?;
        if entries.is_empty() || entries.len() > 2 {
            return Err(PackOperatorErrorV1::Readiness);
        }
        let mut found_snapshot = false;
        for entry in entries {
            let path = entry.path();
            let name = entry
                .file_name()
                .to_str()
                .ok_or(PackOperatorErrorV1::Readiness)?
                .to_owned();
            if path == self.path {
                found_snapshot = true;
            } else if !is_scrubbed_snapshot_staging_name(&name) {
                return Err(PackOperatorErrorV1::Readiness);
            }
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| PackOperatorErrorV1::Readiness)?;
            if !metadata.file_type().is_file() || metadata.len() != 0 {
                return Err(PackOperatorErrorV1::Readiness);
            }
        }
        if !found_snapshot {
            return Err(PackOperatorErrorV1::Readiness);
        }
        Ok(())
    }
}

fn is_scrubbed_snapshot_staging_name(name: &str) -> bool {
    let Some(nonce) = name
        .strip_prefix(".snapshot.sqlite3.worldstream-transfer-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    nonce.len() == 32
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn replay_matches_live_metadata(
    replay: &SqlitePackReplaySummaryV1,
    metadata: Option<&SqliteCanonicalMetadataStatusV1>,
) -> bool {
    match metadata {
        None => replay.deployment_lineage.is_none() && replay.storage_epoch.is_none(),
        Some(metadata) => {
            replay.deployment_lineage.as_deref() == Some(metadata.deployment_lineage.as_str())
                && replay.storage_epoch == Some(metadata.storage_epoch)
        }
    }
}

fn verify_sqlite_pack_replays(
    database: &Path,
    registry: &PackRegistryV1,
) -> Result<SqlitePackReplaySummaryV1, PackOperatorErrorV1> {
    if !database.exists() {
        return Ok(SqlitePackReplaySummaryV1 {
            rooms_replayed: 0,
            isolated_rooms_skipped: 0,
            deployment_lineage: None,
            storage_epoch: None,
        });
    }
    let evidence = extract_restore_evidence(database, NativeSqliteLimits::default())
        .map_err(|_| PackOperatorErrorV1::Readiness)?;
    let mut rooms_replayed = 0_usize;
    let mut isolated_rooms_skipped = 0_usize;
    for (room_id, records) in &evidence.canonical_records {
        let (integrity, _) = evidence
            .integrity
            .get(room_id)
            .ok_or(PackOperatorErrorV1::Readiness)?;
        match integrity.as_str() {
            "faulted" | "quarantined" => {
                isolated_rooms_skipped = isolated_rooms_skipped
                    .checked_add(1)
                    .ok_or(PackOperatorErrorV1::Readiness)?;
                continue;
            }
            "healthy" => {}
            _ => return Err(PackOperatorErrorV1::Readiness),
        }
        let genesis = records
            .first()
            .filter(|record| record.room_seq == 0)
            .ok_or(PackOperatorErrorV1::Readiness)?;
        let transitions = records
            .iter()
            .skip(1)
            .map(|record| record.bytes.clone())
            .collect::<Vec<_>>();
        let stored_head = evidence
            .room_heads
            .get(room_id)
            .ok_or(PackOperatorErrorV1::Readiness)?;
        let expected_head: CompleteHeadV1 =
            CanonicalJsonV1::decode_canonical(&stored_head.complete_head_bytes)
                .map_err(|_| PackOperatorErrorV1::Readiness)?;
        let (core_state, activity_state) = evidence
            .materializations
            .get(room_id)
            .ok_or(PackOperatorErrorV1::Readiness)?;
        CoreTraceV1::verify_executable_history_for_storage(
            registry,
            &expected_head,
            &genesis.bytes,
            &transitions,
            core_state,
            activity_state,
        )
        .map_err(|_| PackOperatorErrorV1::Readiness)?;
        rooms_replayed = rooms_replayed
            .checked_add(1)
            .ok_or(PackOperatorErrorV1::Readiness)?;
    }
    Ok(SqlitePackReplaySummaryV1 {
        rooms_replayed,
        isolated_rooms_skipped,
        deployment_lineage: evidence.deployment_lineage,
        storage_epoch: evidence.storage_epoch,
    })
}

fn sqlite_deployment_binding(
    data_directory: &Path,
    storage: &StorageConfig,
    replay: &SqlitePackReplaySummaryV1,
) -> Result<String, PackOperatorErrorV1> {
    if replay.deployment_lineage.is_some() != replay.storage_epoch.is_some() {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let configured = match (&storage.deployment_lineage, storage.storage_epoch) {
        (Some(lineage), Some(epoch)) => Some((lineage.as_str().to_owned(), epoch.get())),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => return Err(PackOperatorErrorV1::Readiness),
    };
    let observed = replay
        .deployment_lineage
        .as_ref()
        .zip(replay.storage_epoch)
        .map(|(lineage, epoch)| (lineage.clone(), epoch));
    if configured.is_some() && observed.is_some() && configured != observed {
        return Err(PackOperatorErrorV1::Readiness);
    }
    let effective = observed.or(configured);
    let lineage = effective.as_ref().map(|(lineage, _)| lineage.as_bytes());
    let epoch = effective.as_ref().map(|(_, epoch)| *epoch);
    let epoch_bytes = epoch.map(|value| value.to_string().into_bytes());
    pack_deployment_binding(
        data_directory,
        storage.profile,
        lineage,
        epoch_bytes.as_deref(),
        epoch,
    )
    .map_err(|_| PackOperatorErrorV1::Readiness)
}

/// Exports the byte-identical installed `.wspack` to one new owner-only file.
///
/// # Errors
///
/// Returns a closed digest, installed-object, output-policy, or write error.
pub fn export_pack(
    data_directory: &Path,
    digest: &str,
    output: &Path,
) -> Result<PackExportReceiptV1, PackOperatorErrorV1> {
    let digest = parse_digest(digest)?;
    let store = store(data_directory)?;
    let bundle = store.load_installed(&digest)?;
    let revision_digest = bundle.revision_digest().to_string();
    let byte_count = bundle.archive_bytes().len();
    drop(bundle);
    let mut file = create_owner_only_file(output).map_err(|_| PackOperatorErrorV1::Export)?;
    let result = store
        .export_exact(&digest, &mut file)
        .and_then(|()| file.sync_all().map_err(PackBundleErrorV1::from));
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(output);
        return Err(PackOperatorErrorV1::Bundle(error));
    }
    Ok(PackExportReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "export",
        bundle_digest: digest.to_string(),
        revision_digest,
        byte_count,
        approval_exported: false,
    })
}

struct SqliteReferences {
    revisions: Vec<String>,
}

impl RetainedRevisionSourceV1 for SqliteReferences {
    fn is_revision_referenced(
        &self,
        revision_digest: &PackDigestV1,
    ) -> Result<bool, PackBundleErrorV1> {
        Ok(self
            .revisions
            .iter()
            .any(|candidate| candidate == &revision_digest.to_string()))
    }
}

struct PostgresReferences<'a>(&'a PostgresAdmin);

impl RetainedRevisionSourceV1 for PostgresReferences<'_> {
    fn is_revision_referenced(
        &self,
        revision_digest: &PackDigestV1,
    ) -> Result<bool, PackBundleErrorV1> {
        self.0
            .is_pack_revision_referenced(revision_digest)
            .map_err(|_| PackBundleErrorV1::ReferenceQuery)
    }
}

/// Safely removes an unreferenced bundle using complete bounded read-only
/// `SQLite` evidence. No force-removal path exists.
///
/// # Errors
///
/// Returns a closed evidence, reference, digest, or lifecycle error.
pub fn remove_pack_sqlite(
    data_directory: &Path,
    database: &Path,
    digest: &str,
    operator_id: String,
    removed_at: String,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    let evidence = extract_restore_evidence(database, NativeSqliteLimits::default())
        .map_err(|_| PackOperatorErrorV1::References)?;
    let references = SqliteReferences {
        revisions: evidence
            .room_heads
            .values()
            .map(|head| format!("blake3:{}", head.pack_digest.as_str()))
            .collect(),
    };
    remove_pack_with_references(data_directory, digest, &references, operator_id, removed_at)
}

/// Safely removes an unreferenced bundle using a bounded direct-admin
/// `PostgreSQL` reference proof. No force-removal path exists.
///
/// # Errors
///
/// Returns a closed provider, reference, digest, or lifecycle error.
pub fn remove_pack_postgres(
    data_directory: &Path,
    admin: &PostgresAdmin,
    digest: &str,
    operator_id: String,
    removed_at: String,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    remove_pack_with_references(
        data_directory,
        digest,
        &PostgresReferences(admin),
        operator_id,
        removed_at,
    )
}

fn remove_pack_with_references(
    data_directory: &Path,
    digest: &str,
    references: &dyn RetainedRevisionSourceV1,
    operator_id: String,
    removed_at: String,
) -> Result<PackMutationReceiptV1, PackOperatorErrorV1> {
    let digest = parse_digest(digest)?;
    let store = store(data_directory)?;
    let revision = store.load_installed(&digest)?.revision_digest().to_string();
    store.remove_if_unreferenced(
        &digest,
        references,
        PackRemovalV1 {
            operator_id,
            removed_at,
        },
    )?;
    Ok(PackMutationReceiptV1 {
        schema: RECEIPT_SCHEMA,
        status: "complete",
        operation: "remove",
        bundle_digest: digest.to_string(),
        revision_digest: Some(revision),
        install_state: None,
        restart_required: true,
        approval_transferred: false,
    })
}

fn parse_digest(value: &str) -> Result<PackBundleDigestV1, PackOperatorErrorV1> {
    PackBundleDigestV1::from_str(value).map_err(|_| PackOperatorErrorV1::Digest)
}

#[cfg(test)]
#[allow(clippy::panic, clippy::too_many_lines)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use tempfile::tempdir;
    use worldstream_core::{
        AuthorityBootstrapV1, AuthorityCheckedAt, AuthorityV1, CapabilityBearerV1, PrincipalKindV1,
        builtin_counter_registry, counter_v2_digest,
    };
    use worldstream_protocol::{
        AccessMode, BearerWireV1, CreateMember, CreateRoomRequest, PackReference, PrincipalKind,
    };
    use worldstream_runtime::EffectiveConfig;
    use worldstream_sqlite::{SqliteRoomStore, SqliteSourceTransferStateV1};

    use super::*;
    use crate::{GatewayBackend, GatewaySession, SqliteGatewayBackend};

    struct References(bool);

    impl RetainedRevisionSourceV1 for References {
        fn is_revision_referenced(
            &self,
            _revision_digest: &PackDigestV1,
        ) -> Result<bool, PackBundleErrorV1> {
            Ok(self.0)
        }
    }

    fn official_negotiate_candidate() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/packs/negotiate/releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack")
    }

    fn sqlite_storage(data_directory: &Path) -> StorageConfig {
        let mut config = EffectiveConfig::default();
        config.storage.data_dir = data_directory.to_owned();
        config.storage
    }

    fn seed_wal_database(data_directory: &Path) -> SqliteLiveBackupFixtureV1 {
        prepare_data_directory(data_directory)
            .unwrap_or_else(|error| panic!("prepare data directory: {error}"));
        let database = data_directory.join("worldstream.sqlite3");
        let startup = startup_registry(data_directory)
            .unwrap_or_else(|error| panic!("startup registry: {error}"));
        let store = SqliteRoomStore::open(&database)
            .unwrap_or_else(|error| panic!("open WAL database: {error}"));
        store
            .initialize_canonical_metadata("deployment/pack-readiness", 9)
            .unwrap_or_else(|error| panic!("initialize canonical metadata: {error}"));
        store
            .initialize_deployment_identity(startup.base_distribution_identity().clone())
            .unwrap_or_else(|error| panic!("initialize deployment identity: {error}"));
        assert_eq!(
            store.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceAuthoritative
        );
        let source_identity = store.database_identity();
        let witness = data_directory.join("pre-readiness-witness.sqlite3");
        let receipt = store
            .create_live_backup(&witness)
            .unwrap_or_else(|error| panic!("create source witness: {error}"));
        let semantic_digest = receipt.semantic_digest().clone();
        store
            .scrub_live_backup(&witness, &receipt)
            .unwrap_or_else(|error| panic!("scrub source witness: {error}"));
        drop(store);
        let connection = rusqlite::Connection::open(&database)
            .unwrap_or_else(|error| panic!("inspect journal mode: {error}"));
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", (), |row| row.get(0))
            .unwrap_or_else(|error| panic!("read journal mode: {error}"));
        assert_eq!(journal_mode, "wal");
        drop(connection);
        SqliteLiveBackupFixtureV1 {
            database,
            source_identity,
            semantic_digest,
        }
    }

    fn seed_real_room_wal_database_without_canonical_metadata(
        data_directory: &Path,
    ) -> SqliteLiveBackupFixtureV1 {
        prepare_data_directory(data_directory)
            .unwrap_or_else(|error| panic!("prepare data directory: {error}"));
        let database = data_directory.join("worldstream.sqlite3");
        let startup = startup_registry(data_directory)
            .unwrap_or_else(|error| panic!("startup registry: {error}"));
        let store = SqliteRoomStore::open(&database)
            .unwrap_or_else(|error| panic!("open WAL database: {error}"));
        store
            .initialize_deployment_identity(startup.base_distribution_identity().clone())
            .unwrap_or_else(|error| panic!("initialize deployment identity: {error}"));

        let bearer = CapabilityBearerV1::from_bytes([0xa9; 32]);
        let principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse()
            .unwrap_or_else(|error| panic!("principal: {error}"));
        AuthorityV1::new(Arc::new(store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|error| panic!("capability id: {error}")),
                    principal,
                    PrincipalKindV1::Human,
                    "01ARZ3NDEKTSV4RRFFQ69G5FC3"
                        .parse()
                        .unwrap_or_else(|error| panic!("authority change id: {error}")),
                    bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| panic!("authority bootstrap: {error}")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|error| panic!("authority checked at: {error}")),
            )
            .unwrap_or_else(|error| panic!("bootstrap authority: {error}"));

        let registry = Arc::new(
            builtin_counter_registry().unwrap_or_else(|error| panic!("counter registry: {error}")),
        );
        let retained_pack = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| panic!("counter revision: {error}"));
        let descriptor = retained_pack.descriptor();
        let wire = BearerWireV1::from_bytes([0xa9; 32]);
        let session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FC5"
                .parse()
                .unwrap_or_else(|error| panic!("session id: {error}")),
            CapabilityBearerV1::from_bytes(
                BearerWireV1::parse(&wire.to_wire())
                    .unwrap_or_else(|error| panic!("bearer wire: {error}"))
                    .into_bytes(),
            ),
            wire,
        );
        let backend = SqliteGatewayBackend::new(store.clone(), Arc::clone(&registry));
        let created = backend
            .create_room(
                &session,
                CreateRoomRequest {
                    pack: PackReference {
                        id: descriptor.pack_id.clone(),
                        version: descriptor.explanatory_version.clone(),
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: serde_json::json!({
                        "initial_value": 0,
                        "maximum_value": 4
                    }),
                    members: vec![CreateMember {
                        principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
                        principal_kind: PrincipalKind::Human,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    }],
                    idempotency_key: "pack-readiness-real-wal-room".to_owned(),
                },
            )
            .unwrap_or_else(|error| panic!("create real Room: {error}"));
        assert_eq!(created.room_head.room_seq, 0);
        drop(backend);

        assert_eq!(
            store
                .canonical_metadata_status()
                .unwrap_or_else(|error| panic!("canonical metadata status: {error}")),
            None
        );
        assert_eq!(
            store.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceAuthoritative
        );
        let source_identity = store.database_identity();
        let witness = data_directory.join("pre-readiness-real-room-witness.sqlite3");
        let receipt = store
            .create_restart_readiness_snapshot(&witness)
            .unwrap_or_else(|error| panic!("create source witness: {error}"));
        let semantic_digest = receipt.semantic_digest().clone();
        store
            .scrub_restart_readiness_snapshot(&witness, &receipt)
            .unwrap_or_else(|error| panic!("scrub source witness: {error}"));
        drop(store);

        let connection = rusqlite::Connection::open(&database)
            .unwrap_or_else(|error| panic!("inspect journal mode: {error}"));
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode", (), |row| row.get(0))
            .unwrap_or_else(|error| panic!("read journal mode: {error}"));
        assert_eq!(journal_mode, "wal");
        drop(connection);
        SqliteLiveBackupFixtureV1 {
            database,
            source_identity,
            semantic_digest,
        }
    }

    struct SqliteLiveBackupFixtureV1 {
        database: PathBuf,
        source_identity: worldstream_sqlite::SqliteFileIdentityV1,
        semantic_digest: worldstream_transfer::DigestV1,
    }

    fn readiness_snapshot_directories(data_directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(data_directory)
            .unwrap_or_else(|error| panic!("read data directory: {error}"))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|error| panic!("read data directory entry: {error}"))
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(READINESS_SNAPSHOT_PREFIX))
            })
            .map(|entry| entry.path())
            .collect()
    }

    fn assert_only_scrubbed_readiness_snapshots(data_directory: &Path, expected: usize) {
        let directories = readiness_snapshot_directories(data_directory);
        assert_eq!(directories.len(), expected);
        for directory in directories {
            let directory_metadata = fs::symlink_metadata(&directory)
                .unwrap_or_else(|error| panic!("read readiness directory metadata: {error}"));
            assert!(directory_metadata.file_type().is_dir());
            assert!(!directory_metadata.file_type().is_symlink());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(directory_metadata.permissions().mode() & 0o777, 0o700);
            }
            let entries = fs::read_dir(&directory)
                .unwrap_or_else(|error| panic!("read readiness directory: {error}"))
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|error| panic!("read readiness entry: {error}"));
            assert!((1..=2).contains(&entries.len()));
            let snapshot = directory.join("snapshot.sqlite3");
            assert!(entries.iter().any(|entry| entry.path() == snapshot));
            for entry in entries {
                let name = entry
                    .file_name()
                    .to_str()
                    .unwrap_or_else(|| panic!("readiness marker name"))
                    .to_owned();
                assert!(entry.path() == snapshot || is_scrubbed_snapshot_staging_name(&name));
                let metadata = fs::symlink_metadata(entry.path())
                    .unwrap_or_else(|error| panic!("read scrubbed marker: {error}"));
                assert!(metadata.file_type().is_file());
                assert!(!metadata.file_type().is_symlink());
                assert_eq!(metadata.len(), 0);
            }
            assert!(!PathBuf::from(format!("{}-wal", snapshot.display())).exists());
            assert!(!PathBuf::from(format!("{}-shm", snapshot.display())).exists());
        }
    }

    fn assert_source_semantics_unchanged_after_preflight(
        data_directory: &Path,
        fixture: &SqliteLiveBackupFixtureV1,
    ) {
        let store = SqliteRoomStore::open(&fixture.database)
            .unwrap_or_else(|error| panic!("reopen source: {error}"));
        assert_eq!(store.database_identity(), fixture.source_identity);
        assert_eq!(
            store.source_transfer_state(),
            SqliteSourceTransferStateV1::SourceAuthoritative
        );
        let witness = data_directory.join("post-readiness-witness.sqlite3");
        let receipt = store
            .create_restart_readiness_snapshot(&witness)
            .unwrap_or_else(|error| panic!("create post-readiness witness: {error}"));
        assert_eq!(receipt.semantic_digest(), &fixture.semantic_digest);
        store
            .scrub_restart_readiness_snapshot(&witness, &receipt)
            .unwrap_or_else(|error| panic!("scrub post-readiness witness: {error}"));
    }

    #[test]
    fn official_bundle_completes_the_offline_operator_lifecycle_through_production_host() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let candidate = official_negotiate_candidate();
        let storage = sqlite_storage(&data_directory);
        let original = fs::read(&candidate)
            .unwrap_or_else(|error| panic!("official Negotiate candidate: {error}"));

        let inspected = inspect_pack(&data_directory, &candidate)
            .unwrap_or_else(|error| panic!("inspect candidate: {error}"));
        assert_eq!(inspected.pack_id, "worldstream.negotiate");
        assert!(!inspected.bundle_digest.is_empty());
        assert!(!inspected.revision_digest.is_empty());

        approve_pack(
            &data_directory,
            &candidate,
            "test-operator".to_owned(),
            "2026-08-30T12:00:00Z".to_owned(),
        )
        .unwrap_or_else(|error| panic!("approve candidate: {error}"));
        let installed = install_pack(
            &data_directory,
            &candidate,
            "2026-08-30T12:00:01Z".to_owned(),
        )
        .unwrap_or_else(|error| panic!("install candidate: {error}"));
        assert_eq!(
            installed.install_state,
            Some(PackInstallStateV1::RetainedOnly)
        );

        let retained_inventory = inventory(&data_directory, &storage)
            .unwrap_or_else(|error| panic!("retained inventory: {error}"));
        assert_eq!(retained_inventory.installed, 1);
        assert_eq!(retained_inventory.selectable, 0);
        assert_eq!(retained_inventory.retained_only, 1);
        assert!(retained_inventory.inventory_digest.starts_with("blake3:"));

        set_pack_selectable(&data_directory, &inspected.bundle_digest, true)
            .unwrap_or_else(|error| panic!("select candidate: {error}"));
        let selectable_inventory = inventory(&data_directory, &storage)
            .unwrap_or_else(|error| panic!("selectable inventory: {error}"));
        assert_ne!(
            retained_inventory.inventory_digest,
            selectable_inventory.inventory_digest
        );
        let readiness = verify_pack_restart_readiness_sqlite(
            &data_directory,
            &data_directory.join("worldstream.sqlite3"),
            &storage,
        )
        .unwrap_or_else(|error| panic!("production restart readiness: {error}"));
        assert!(readiness.original_bytes_reverified);
        assert!(readiness.production_component_host_admission);
        assert!(readiness.room_replay_checked);
        assert_eq!(readiness.rooms_replayed, 0);
        assert_eq!(readiness.installed_bundles, 1);
        assert_eq!(readiness.installed_selectable, 1);
        assert_eq!(
            readiness.inventory_digest,
            selectable_inventory.inventory_digest
        );
        assert_eq!(readiness.storage_profile, "sqlite-bundled");
        assert!(readiness.deployment_binding.starts_with("blake3:"));
        let admitted = assemble_startup_pack_registry(&data_directory)
            .unwrap_or_else(|error| panic!("sealed startup registry: {error}"));
        crate::verify_startup_pack_readiness_seal(
            &data_directory,
            &admitted,
            StorageProfile::SqliteBundled,
            &readiness.deployment_binding,
        )
        .unwrap_or_else(|error| panic!("consume exact readiness seal: {error}"));

        let export_directory = prepare_data_directory(&temporary.path().join("exports"))
            .unwrap_or_else(|error| panic!("prepare owner-only export directory: {error}"));
        let exported = export_directory.join("exported.wspack");
        let export = export_pack(&data_directory, &inspected.bundle_digest, &exported)
            .unwrap_or_else(|error| panic!("export candidate: {error}"));
        assert!(!export.approval_exported);
        assert_eq!(
            fs::read(&exported).unwrap_or_else(|error| panic!("read export: {error}")),
            original
        );

        let referenced = remove_pack_with_references(
            &data_directory,
            &inspected.bundle_digest,
            &References(true),
            "test-operator".to_owned(),
            "2026-08-30T12:00:02Z".to_owned(),
        );
        assert!(matches!(
            referenced,
            Err(PackOperatorErrorV1::Bundle(
                PackBundleErrorV1::RevisionReferenced
            ))
        ));

        revoke_pack(
            &data_directory,
            &inspected.bundle_digest,
            "test-operator".to_owned(),
            "2026-08-30T12:00:03Z".to_owned(),
        )
        .unwrap_or_else(|error| panic!("revoke candidate: {error}"));
        assert!(matches!(
            crate::verify_startup_pack_readiness_seal(
                &data_directory,
                &admitted,
                StorageProfile::SqliteBundled,
                &readiness.deployment_binding,
            ),
            Err(crate::StartupPackRegistryErrorV1::Bundle(
                PackBundleErrorV1::StartupReadinessMissing
            ))
        ));
        remove_pack_with_references(
            &data_directory,
            &inspected.bundle_digest,
            &References(false),
            "test-operator".to_owned(),
            "2026-08-30T12:00:04Z".to_owned(),
        )
        .unwrap_or_else(|error| panic!("remove candidate: {error}"));
        assert_eq!(
            inventory(&data_directory, &storage)
                .unwrap_or_else(|error| panic!("empty inventory: {error}"))
                .installed,
            0
        );
    }

    #[test]
    fn sqlite_readiness_rejects_a_database_other_than_the_daemon_target() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let other_directory = temporary.path().join("other");
        prepare_data_directory(&data_directory)
            .unwrap_or_else(|error| panic!("prepare data directory: {error}"));
        prepare_data_directory(&other_directory)
            .unwrap_or_else(|error| panic!("prepare other directory: {error}"));
        let storage = sqlite_storage(&data_directory);

        assert!(matches!(
            verify_pack_restart_readiness_sqlite(
                &data_directory,
                &data_directory.join("different.sqlite3"),
                &storage,
            ),
            Err(PackOperatorErrorV1::Readiness)
        ));
        assert!(matches!(
            verify_pack_restart_readiness_sqlite(
                &data_directory,
                &other_directory.join("worldstream.sqlite3"),
                &storage,
            ),
            Err(PackOperatorErrorV1::Readiness)
        ));
    }

    #[test]
    fn sqlite_readiness_verifies_a_standalone_snapshot_of_real_wal_metadata() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let fixture = seed_wal_database(&data_directory);
        let storage = sqlite_storage(&data_directory);

        let readiness =
            verify_pack_restart_readiness_sqlite(&data_directory, &fixture.database, &storage)
                .unwrap_or_else(|error| panic!("verify WAL readiness: {error}"));

        assert!(readiness.room_replay_checked);
        assert_eq!(readiness.rooms_replayed, 0);
        assert_eq!(readiness.isolated_rooms_skipped, 0);
        assert_only_scrubbed_readiness_snapshots(&data_directory, 1);
        assert_source_semantics_unchanged_after_preflight(&data_directory, &fixture);
    }

    #[test]
    fn sqlite_readiness_replays_a_real_wal_room_without_canonical_metadata() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let fixture = seed_real_room_wal_database_without_canonical_metadata(&data_directory);
        let storage = sqlite_storage(&data_directory);

        let readiness =
            verify_pack_restart_readiness_sqlite(&data_directory, &fixture.database, &storage)
                .unwrap_or_else(|error| panic!("verify real Room WAL readiness: {error}"));

        assert!(readiness.room_replay_checked);
        assert_eq!(readiness.rooms_replayed, 1);
        assert_eq!(readiness.isolated_rooms_skipped, 0);
        assert_only_scrubbed_readiness_snapshots(&data_directory, 1);
        assert_source_semantics_unchanged_after_preflight(&data_directory, &fixture);
    }

    #[test]
    fn sqlite_readiness_scrubs_its_verified_snapshot_when_replay_fails() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let fixture = seed_wal_database(&data_directory);
        let startup = startup_registry(&data_directory)
            .unwrap_or_else(|error| panic!("startup registry: {error}"));

        let result = verify_sqlite_pack_replays_from_live_database_with(
            &data_directory,
            &fixture.database,
            startup.registry(),
            |snapshot, _| {
                assert_ne!(snapshot, fixture.database);
                assert!(snapshot.exists());
                let snapshot_metadata = fs::symlink_metadata(snapshot)
                    .unwrap_or_else(|error| panic!("read snapshot metadata: {error}"));
                assert!(snapshot_metadata.file_type().is_file());
                assert!(!snapshot_metadata.file_type().is_symlink());
                let directory = snapshot
                    .parent()
                    .unwrap_or_else(|| panic!("snapshot directory"));
                let directory_metadata = fs::symlink_metadata(directory)
                    .unwrap_or_else(|error| panic!("read snapshot directory metadata: {error}"));
                assert!(directory_metadata.file_type().is_dir());
                assert!(!directory_metadata.file_type().is_symlink());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    assert_eq!(directory_metadata.permissions().mode() & 0o777, 0o700);
                }
                let connection = rusqlite::Connection::open_with_flags(
                    snapshot,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap_or_else(|error| panic!("open standalone snapshot: {error}"));
                let journal_mode: String = connection
                    .query_row("PRAGMA journal_mode", (), |row| row.get(0))
                    .unwrap_or_else(|error| panic!("read snapshot journal mode: {error}"));
                assert_eq!(journal_mode, "delete");
                Err(PackOperatorErrorV1::Readiness)
            },
        );

        assert!(matches!(result, Err(PackOperatorErrorV1::Readiness)));
        assert_only_scrubbed_readiness_snapshots(&data_directory, 1);
        assert_source_semantics_unchanged_after_preflight(&data_directory, &fixture);
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_readiness_never_deletes_a_directory_substitution_victim() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let fixture = seed_wal_database(&data_directory);
        let startup = startup_registry(&data_directory)
            .unwrap_or_else(|error| panic!("startup registry: {error}"));
        let victim = temporary.path().join("substitution-victim");
        prepare_data_directory(&victim)
            .unwrap_or_else(|error| panic!("prepare substitution victim: {error}"));
        let victim_snapshot = victim.join("snapshot.sqlite3");
        let victim_bytes = b"same-owner directory substitution victim";
        fs::write(&victim_snapshot, victim_bytes)
            .unwrap_or_else(|error| panic!("write substitution victim: {error}"));
        let held_directory = data_directory.join("held-readiness-snapshot");
        let mut original_bytes = None;

        let result = verify_sqlite_pack_replays_from_live_database_with(
            &data_directory,
            &fixture.database,
            startup.registry(),
            |snapshot, registry| {
                let replay = verify_sqlite_pack_replays(snapshot, registry)?;
                let directory = snapshot
                    .parent()
                    .unwrap_or_else(|| panic!("snapshot directory"))
                    .to_owned();
                original_bytes = Some(
                    fs::read(snapshot)
                        .unwrap_or_else(|error| panic!("read admitted snapshot: {error}")),
                );
                fs::rename(&directory, &held_directory)
                    .unwrap_or_else(|error| panic!("hold admitted directory: {error}"));
                std::os::unix::fs::symlink(&victim, &directory)
                    .unwrap_or_else(|error| panic!("substitute readiness directory: {error}"));
                Ok(replay)
            },
        );

        assert!(matches!(result, Err(PackOperatorErrorV1::Readiness)));
        assert_eq!(
            fs::read(&victim_snapshot)
                .unwrap_or_else(|error| panic!("read substitution victim: {error}")),
            victim_bytes
        );
        assert_eq!(
            fs::read(held_directory.join("snapshot.sqlite3"))
                .unwrap_or_else(|error| panic!("read retained admitted snapshot: {error}")),
            original_bytes.unwrap_or_else(|| panic!("original snapshot bytes"))
        );
        assert_source_semantics_unchanged_after_preflight(&data_directory, &fixture);
    }

    #[cfg(unix)]
    #[test]
    fn post_scrub_validation_never_deletes_a_directory_substitution_victim() {
        let temporary = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let data_directory = temporary.path().join("data");
        let snapshot = ReadinessSnapshotV1::create(&data_directory)
            .unwrap_or_else(|error| panic!("create readiness snapshot path: {error}"));
        create_owner_only_file(snapshot.path())
            .unwrap_or_else(|error| panic!("create scrubbed marker: {error}"));
        let admitted_directory = snapshot.directory.clone();
        let held_directory = data_directory.join("held-scrubbed-readiness");
        fs::rename(&admitted_directory, &held_directory)
            .unwrap_or_else(|error| panic!("hold scrubbed directory: {error}"));
        let victim = temporary.path().join("post-scrub-victim");
        prepare_data_directory(&victim)
            .unwrap_or_else(|error| panic!("prepare post-scrub victim: {error}"));
        let victim_file = victim.join("snapshot.sqlite3");
        let victim_bytes = b"post-scrub directory substitution victim";
        fs::write(&victim_file, victim_bytes)
            .unwrap_or_else(|error| panic!("write post-scrub victim: {error}"));
        std::os::unix::fs::symlink(&victim, &admitted_directory)
            .unwrap_or_else(|error| panic!("substitute post-scrub directory: {error}"));

        assert!(matches!(
            snapshot.verify_scrubbed_marker(),
            Err(PackOperatorErrorV1::Readiness)
        ));
        assert_eq!(
            fs::read(&victim_file)
                .unwrap_or_else(|error| panic!("read post-scrub victim: {error}")),
            victim_bytes
        );
        assert_eq!(
            fs::metadata(held_directory.join("snapshot.sqlite3"))
                .unwrap_or_else(|error| panic!("read held scrubbed marker: {error}"))
                .len(),
            0
        );
    }
}
