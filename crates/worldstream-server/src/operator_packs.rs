//! Offline Host Operator surface for exact Activity Pack Bundle lifecycle.

use std::{fs, path::Path, str::FromStr as _};

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
};

use crate::{
    StartupPackRegistryV1, assemble_startup_pack_registry,
    pack_startup::{pack_deployment_binding, startup_pack_inventory_digest},
};

const RECEIPT_SCHEMA: &str = "worldstream/pack-operator-receipt/v1";

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
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
    let replay = verify_sqlite_pack_replays(&database, startup.registry())?;
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
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;
    use worldstream_runtime::EffectiveConfig;

    use super::*;

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
            .join("../../packs/negotiate/releases/0.1.0/worldstream-negotiate-candidate.wspack")
    }

    fn sqlite_storage(data_directory: &Path) -> StorageConfig {
        let mut config = EffectiveConfig::default();
        config.storage.data_dir = data_directory.to_owned();
        config.storage
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
}
