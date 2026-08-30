#![cfg_attr(test, allow(clippy::panic, clippy::unwrap_used))]

use std::{fs, path::Path, sync::Arc};

use serde::Serialize;
use thiserror::Error;
use worldstream_component_host::{ComponentHostErrorV1, ComponentPackHostV1};
use worldstream_core::{
    CanonicalJsonError, CanonicalJsonV1, PackRegistryErrorV1, PackRegistryStatusV1, PackRegistryV1,
    builtin_worldstream_registry,
};
use worldstream_pack_bundle::{
    MAX_INSTALLED_BUNDLE_COUNT, PackBundleErrorV1, PackBundleStartupInventoryV1, PackBundleStoreV1,
    PackInstallStateV1,
};
use worldstream_runtime::StorageProfile;
use worldstream_transfer::{DeploymentIdentityV1, DigestV1, PackIdentityV1, TransferError};

/// Bounded facts about the immutable Activity Pack registry assembled at startup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartupPackRegistryDiagnosticsV1 {
    pub embedded_revisions: usize,
    pub installed_bundles: usize,
    pub installed_selectable: usize,
    pub installed_retained_only: usize,
    pub total_revisions: usize,
    pub installed_bundle_limit: usize,
}

/// One process-owned immutable Activity Pack registry and its startup facts.
pub struct StartupPackRegistryV1 {
    registry: Arc<PackRegistryV1>,
    base_distribution_identity: DeploymentIdentityV1,
    inventory_digest: String,
    diagnostics: StartupPackRegistryDiagnosticsV1,
}

impl StartupPackRegistryV1 {
    #[must_use]
    pub fn registry(&self) -> &Arc<PackRegistryV1> {
        &self.registry
    }

    /// Returns the immutable base Runtime Distribution identity. Mutable local
    /// bundle inventory is deliberately carried separately by backup/transfer.
    #[must_use]
    pub fn base_distribution_identity(&self) -> &DeploymentIdentityV1 {
        &self.base_distribution_identity
    }

    /// Returns the digest of the exact, digest-sorted installed inventory
    /// snapshot admitted by this registry. The identity covers Pack/version,
    /// physical bundle, semantic revision, and selectability state.
    #[must_use]
    pub fn inventory_digest(&self) -> &str {
        &self.inventory_digest
    }

    #[must_use]
    pub const fn diagnostics(&self) -> StartupPackRegistryDiagnosticsV1 {
        self.diagnostics
    }
}

/// Closed startup failure before either storage profile can serve Rooms.
#[derive(Debug, Error)]
pub enum StartupPackRegistryErrorV1 {
    #[error("local Activity Pack Bundle inventory failed closed")]
    Bundle(#[from] PackBundleErrorV1),
    #[error("portable Activity Pack Component admission failed closed")]
    Component(#[from] ComponentHostErrorV1),
    #[error("combined Activity Pack registry validation failed closed")]
    Registry(#[from] PackRegistryErrorV1),
    #[error("base Runtime Distribution identity could not be encoded")]
    Canonical(#[from] CanonicalJsonError),
    #[error("base Runtime Distribution identity was invalid")]
    DistributionIdentity(#[from] TransferError),
    #[error("Activity Pack deployment target identity failed closed")]
    DeploymentTarget,
}

/// Builds exactly one embedded-plus-installed immutable registry from the
/// startup snapshot at `<data_dir>/activity-packs`.
///
/// Install, approval, and selectability changes made after this call do not
/// affect the returned registry. The daemon must restart to take a new snapshot.
///
/// # Errors
///
/// Returns a fail-closed inventory, Component admission, semantic validation,
/// collision, or golden-corpus error before any backend is constructed.
pub fn assemble_startup_pack_registry(
    data_directory: &Path,
) -> Result<StartupPackRegistryV1, StartupPackRegistryErrorV1> {
    let store = PackBundleStoreV1::open(data_directory.join("activity-packs"))?;
    let inventory = store.load_startup_inventory()?;
    let inventory_counts = inventory.counts();
    let inventory_digest = startup_pack_inventory_digest(&inventory)?;
    let embedded = builtin_worldstream_registry()?;
    let embedded_revisions = embedded.len();
    let base_distribution_identity = distribution_identity(&embedded)?;
    let host = ComponentPackHostV1::new()?;
    let mut portable = Vec::with_capacity(inventory_counts.installed);

    for entry in inventory.into_entries() {
        let (installed, bundle) = entry.into_parts();
        let status = match installed.install_state {
            PackInstallStateV1::Selectable => PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
            },
            PackInstallStateV1::RetainedOnly => PackRegistryStatusV1 {
                selectable_for_new_rooms: false,
                runnable_for_retained_rooms: true,
            },
        };
        portable.push(host.admit(bundle, status)?);
    }

    let registry = if portable.is_empty() {
        embedded
    } else {
        embedded.admit_portable(portable)?
    };
    let total_revisions = registry.len();
    Ok(StartupPackRegistryV1 {
        registry: Arc::new(registry),
        base_distribution_identity,
        inventory_digest,
        diagnostics: StartupPackRegistryDiagnosticsV1 {
            embedded_revisions,
            installed_bundles: inventory_counts.installed,
            installed_selectable: inventory_counts.selectable,
            installed_retained_only: inventory_counts.retained_only,
            total_revisions,
            installed_bundle_limit: MAX_INSTALLED_BUNDLE_COUNT,
        },
    })
}

const STARTUP_PACK_INVENTORY_ID: &str = "worldstream/startup-pack-inventory/v1";
const PACK_DEPLOYMENT_BINDING_ID: &str = "worldstream/pack-deployment-binding/v1";

#[derive(Serialize)]
struct StartupPackInventoryIdentityV1 {
    schema: &'static str,
    entries: Vec<StartupPackInventoryEntryIdentityV1>,
}

#[derive(Serialize)]
struct StartupPackInventoryEntryIdentityV1 {
    pack_id: String,
    explanatory_version: String,
    bundle_digest: String,
    revision_digest: String,
    install_state: PackInstallStateV1,
}

#[derive(Serialize)]
struct PackDeploymentBindingIdentityV1 {
    schema: &'static str,
    storage_profile: &'static str,
    data_directory_digest: String,
    deployment_lineage_digest: Option<String>,
    storage_epoch_digest: Option<String>,
    storage_epoch: Option<u64>,
}

/// Computes the portable identity shared by inventory and restart-readiness
/// receipts. Installation timestamps and local paths are deliberately absent;
/// every field that can change the runnable startup registry is included.
pub(crate) fn startup_pack_inventory_digest(
    inventory: &PackBundleStartupInventoryV1,
) -> Result<String, CanonicalJsonError> {
    let identity = StartupPackInventoryIdentityV1 {
        schema: STARTUP_PACK_INVENTORY_ID,
        entries: inventory
            .entries()
            .iter()
            .map(|entry| StartupPackInventoryEntryIdentityV1 {
                pack_id: entry.bundle().descriptor().pack_id.clone(),
                explanatory_version: entry.bundle().descriptor().explanatory_version.clone(),
                bundle_digest: entry.installed().bundle_digest.to_string(),
                revision_digest: entry.installed().revision_digest.to_string(),
                install_state: entry.installed().install_state,
            })
            .collect(),
    };
    let serialized = serde_json::to_vec(&identity)
        .map_err(|error| CanonicalJsonError::Serialization(error.to_string()))?;
    let canonical = CanonicalJsonV1::parse(&serialized)?.to_bytes()?;
    Ok(format!("blake3:{}", blake3::hash(&canonical).to_hex()))
}

/// Creates a pathless identity for the exact local deployment target used by
/// restart-readiness and daemon startup. Provider metadata is hashed before it
/// enters the seal; the canonical data-directory identity prevents receipts
/// from another local deployment from being substituted when metadata is
/// intentionally absent.
///
/// # Errors
///
/// Returns a closed target-identity error when the directory is unavailable,
/// provider metadata is incomplete, or canonical encoding fails.
pub fn pack_deployment_binding(
    data_directory: &Path,
    storage_profile: StorageProfile,
    deployment_lineage_bytes: Option<&[u8]>,
    storage_epoch_bytes: Option<&[u8]>,
    storage_epoch: Option<u64>,
) -> Result<String, StartupPackRegistryErrorV1> {
    if deployment_lineage_bytes.is_some() != storage_epoch_bytes.is_some()
        || deployment_lineage_bytes.is_some() != storage_epoch.is_some()
    {
        return Err(StartupPackRegistryErrorV1::DeploymentTarget);
    }
    let canonical_directory = fs::canonicalize(data_directory)
        .map_err(|_| StartupPackRegistryErrorV1::DeploymentTarget)?;
    let identity = PackDeploymentBindingIdentityV1 {
        schema: PACK_DEPLOYMENT_BINDING_ID,
        storage_profile: storage_profile.as_str(),
        data_directory_digest: format!(
            "blake3:{}",
            blake3::hash(canonical_directory.as_os_str().as_encoded_bytes()).to_hex()
        ),
        deployment_lineage_digest: deployment_lineage_bytes
            .map(|bytes| format!("blake3:{}", blake3::hash(bytes).to_hex())),
        storage_epoch_digest: storage_epoch_bytes
            .map(|bytes| format!("blake3:{}", blake3::hash(bytes).to_hex())),
        storage_epoch,
    };
    let serialized =
        serde_json::to_vec(&identity).map_err(|_| StartupPackRegistryErrorV1::DeploymentTarget)?;
    let canonical = CanonicalJsonV1::parse(&serialized)
        .and_then(|value| value.to_bytes())
        .map_err(|_| StartupPackRegistryErrorV1::DeploymentTarget)?;
    Ok(format!("blake3:{}", blake3::hash(&canonical).to_hex()))
}

/// Requires an installed portable inventory to carry the exact target-bound
/// readiness seal produced after executable Replay. Embedded-only startup does
/// not need an offline Pack seal.
///
/// # Errors
///
/// Returns a closed bundle error when the seal is missing, malformed, or does
/// not match the frozen inventory, storage profile, and deployment binding.
pub fn verify_startup_pack_readiness_seal(
    data_directory: &Path,
    startup: &StartupPackRegistryV1,
    storage_profile: StorageProfile,
    deployment_binding: &str,
) -> Result<(), StartupPackRegistryErrorV1> {
    if startup.diagnostics().installed_bundles == 0 {
        return Ok(());
    }
    let store = PackBundleStoreV1::open(data_directory.join("activity-packs"))?;
    let seal = store
        .startup_readiness()?
        .ok_or(PackBundleErrorV1::StartupReadinessMissing)?;
    if seal.inventory_digest != startup.inventory_digest()
        || seal.storage_profile != storage_profile.as_str()
        || seal.deployment_binding != deployment_binding
    {
        return Err(PackBundleErrorV1::StartupReadinessMismatch.into());
    }
    Ok(())
}

fn distribution_identity(
    registry: &PackRegistryV1,
) -> Result<DeploymentIdentityV1, StartupPackRegistryErrorV1> {
    let packs = registry
        .retained_revision_locks()
        .map(|revision_lock| {
            let semantic_digest = revision_lock.revision_digest()?;
            let digest = DigestV1::from_bytes(semantic_digest.digest().as_bytes())?;
            Ok(PackIdentityV1::new(
                revision_lock.pack_id.clone(),
                revision_lock.explanatory_version.clone(),
                digest,
            )?)
        })
        .collect::<Result<Vec<_>, StartupPackRegistryErrorV1>>()?;
    Ok(DeploymentIdentityV1::new(packs, Vec::new())?)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use worldstream_core::builtin_worldstream_registry;
    use worldstream_pack_bundle::MAX_INSTALLED_BUNDLE_COUNT;
    use worldstream_runtime::StorageProfile;

    use super::{
        StartupPackRegistryErrorV1, assemble_startup_pack_registry, pack_deployment_binding,
    };

    #[test]
    fn empty_inventory_assembles_the_complete_embedded_registry_once() {
        let directory = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let startup = assemble_startup_pack_registry(directory.path())
            .unwrap_or_else(|error| panic!("startup registry: {error}"));
        let expected = builtin_worldstream_registry()
            .unwrap_or_else(|error| panic!("embedded registry: {error}"));

        assert_eq!(startup.registry().len(), expected.len());
        assert_eq!(startup.diagnostics().embedded_revisions, expected.len());
        assert_eq!(startup.diagnostics().installed_bundles, 0);
        assert_eq!(startup.diagnostics().total_revisions, expected.len());
        assert_eq!(
            startup.diagnostics().installed_bundle_limit,
            MAX_INSTALLED_BUNDLE_COUNT
        );
        assert_eq!(
            startup.base_distribution_identity().packs().len(),
            expected.len()
        );
        assert!(directory.path().join("activity-packs/inventory").is_dir());
    }

    #[test]
    fn startup_snapshot_is_immutable_and_next_restart_rejects_corrupt_inventory() {
        let directory = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let startup = assemble_startup_pack_registry(directory.path())
            .unwrap_or_else(|error| panic!("startup registry: {error}"));
        let original_count = startup.registry().len();
        let inventory = directory.path().join("activity-packs/inventory");
        fs::write(inventory.join(format!("{}.json", "0".repeat(64))), b"{}")
            .unwrap_or_else(|error| panic!("corrupt inventory fixture: {error}"));

        assert_eq!(startup.registry().len(), original_count);
        assert!(matches!(
            assemble_startup_pack_registry(directory.path()),
            Err(StartupPackRegistryErrorV1::Bundle(_))
        ));
    }

    #[test]
    fn deployment_binding_changes_with_profile_directory_and_provider_metadata() {
        let first = tempdir().unwrap_or_else(|error| panic!("first directory: {error}"));
        let second = tempdir().unwrap_or_else(|error| panic!("second directory: {error}"));
        let sqlite = pack_deployment_binding(
            first.path(),
            StorageProfile::SqliteBundled,
            Some(b"deployment/one"),
            Some(b"7"),
            Some(7),
        )
        .unwrap_or_else(|error| panic!("SQLite binding: {error}"));
        let postgres = pack_deployment_binding(
            first.path(),
            StorageProfile::PostgresPrimary,
            Some(b"deployment/one"),
            Some(b"7"),
            Some(7),
        )
        .unwrap_or_else(|error| panic!("PostgreSQL binding: {error}"));
        let other_directory = pack_deployment_binding(
            second.path(),
            StorageProfile::SqliteBundled,
            Some(b"deployment/one"),
            Some(b"7"),
            Some(7),
        )
        .unwrap_or_else(|error| panic!("other-directory binding: {error}"));
        let other_epoch = pack_deployment_binding(
            first.path(),
            StorageProfile::SqliteBundled,
            Some(b"deployment/one"),
            Some(b"8"),
            Some(8),
        )
        .unwrap_or_else(|error| panic!("other-epoch binding: {error}"));

        assert_ne!(sqlite, postgres);
        assert_ne!(sqlite, other_directory);
        assert_ne!(sqlite, other_epoch);
        assert!(matches!(
            pack_deployment_binding(
                first.path(),
                StorageProfile::SqliteBundled,
                Some(b"deployment/one"),
                None,
                Some(7),
            ),
            Err(StartupPackRegistryErrorV1::DeploymentTarget)
        ));
    }
}
