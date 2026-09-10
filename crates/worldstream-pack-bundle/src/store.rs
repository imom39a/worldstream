use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;
use worldstream_core::{CanonicalJsonV1, PackDigestV1};

use crate::{
    MAX_BUNDLE_BYTES, PackBundleDigestV1, PackBundleErrorV1, PackBundleInspectionV1,
    PackBundleVerifierV1, RetainedPackBundleArtifactV1, VerifiedPackBundleV1,
};

const APPROVAL_RECORD_ID: &str = "worldstream/pack-bundle-approval/v1";
const INVENTORY_RECORD_ID: &str = "worldstream/installed-pack-bundle/v1";
const STARTUP_READINESS_RECORD_ID: &str = "worldstream/pack-startup-readiness/v1";
const TOMBSTONE_RECORD_ID: &str = "worldstream/pack-bundle-tombstone/v1";

/// Maximum installed Activity Pack Bundles admitted in one startup snapshot.
pub const MAX_INSTALLED_BUNDLE_COUNT: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionV1 {
    Approved,
    Revoked,
}

/// Host Operator decision supplied without consulting an ambient clock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorApprovalV1 {
    pub operator_id: String,
    pub decided_at: String,
    pub decision: ApprovalDecisionV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackInstallStateV1 {
    Selectable,
    RetainedOnly,
}

/// Durable local inventory record for one installed exact bundle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPackBundleV1 {
    pub bundle_digest: PackBundleDigestV1,
    pub installed_at: String,
    pub install_state: PackInstallStateV1,
    pub inventory_record_id: String,
    pub revision_digest: PackDigestV1,
}

/// Bounded counts emitted with one immutable startup inventory snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackBundleInventoryCountsV1 {
    pub installed: usize,
    pub selectable: usize,
    pub retained_only: usize,
}

/// One installed inventory row joined to its fully re-verified exact bundle.
#[derive(Clone)]
pub struct PackBundleStartupEntryV1 {
    installed: InstalledPackBundleV1,
    bundle: VerifiedPackBundleV1,
    approved_for_activity_start: bool,
}

impl PackBundleStartupEntryV1 {
    #[must_use]
    pub fn installed(&self) -> &InstalledPackBundleV1 {
        &self.installed
    }

    #[must_use]
    pub fn bundle(&self) -> &VerifiedPackBundleV1 {
        &self.bundle
    }

    /// Reports whether this exact Bundle still has a current matching Host
    /// approval in the immutable startup snapshot.
    #[must_use]
    pub const fn approved_for_activity_start(&self) -> bool {
        self.approved_for_activity_start
    }

    #[must_use]
    pub fn into_parts(self) -> (InstalledPackBundleV1, VerifiedPackBundleV1) {
        (self.installed, self.bundle)
    }
}

/// Immutable, exact-digest-sorted startup view of the installed local CAS.
pub struct PackBundleStartupInventoryV1 {
    entries: Vec<PackBundleStartupEntryV1>,
    counts: PackBundleInventoryCountsV1,
}

/// Durable, target-bound proof that the exact installed inventory completed
/// production Component admission and configured-store executable Replay.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackStartupReadinessSealV1 {
    pub readiness_record_id: String,
    pub inventory_digest: String,
    pub storage_profile: String,
    pub deployment_binding: String,
}

impl PackStartupReadinessSealV1 {
    /// Constructs one closed seal from already verified digest identities.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed digests or an unsupported profile.
    pub fn new(
        inventory_digest: String,
        storage_profile: String,
        deployment_binding: String,
    ) -> Result<Self, PackBundleErrorV1> {
        validate_readiness_identity(&inventory_digest, &storage_profile, &deployment_binding)?;
        Ok(Self {
            readiness_record_id: STARTUP_READINESS_RECORD_ID.to_owned(),
            inventory_digest,
            storage_profile,
            deployment_binding,
        })
    }
}

impl PackBundleStartupInventoryV1 {
    #[must_use]
    pub fn entries(&self) -> &[PackBundleStartupEntryV1] {
        &self.entries
    }

    #[must_use]
    pub const fn counts(&self) -> PackBundleInventoryCountsV1 {
        self.counts
    }

    #[must_use]
    pub fn into_entries(self) -> impl ExactSizeIterator<Item = PackBundleStartupEntryV1> {
        self.entries.into_iter()
    }
}

/// Host Operator facts for one safe-removal tombstone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackRemovalV1 {
    pub operator_id: String,
    pub removed_at: String,
}

/// Storage-profile adapter used to prove that removal cannot break retained
/// Room lineage. `SQLite`, `PostgreSQL`, and in-memory tests are separate adapters.
pub trait RetainedRevisionSourceV1 {
    /// Reports whether any retained Room lineage names the semantic revision.
    ///
    /// # Errors
    ///
    /// Returns an adapter error when the durable reference query cannot be
    /// completed. Implementations must fail closed.
    fn is_revision_referenced(
        &self,
        revision_digest: &PackDigestV1,
    ) -> Result<bool, PackBundleErrorV1>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ApprovalRecordV1 {
    approval_record_id: String,
    bundle_digest: PackBundleDigestV1,
    decided_at: String,
    decision: ApprovalDecisionV1,
    manifest_digest: worldstream_core::Blake3DigestV1,
    operator_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurrentApprovalV1 {
    Missing,
    Approved,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TombstoneRecordV1 {
    bundle_digest: PackBundleDigestV1,
    operator_id: String,
    removed_at: String,
    revision_digest: PackDigestV1,
    tombstone_record_id: String,
}

/// Local content-addressed lifecycle for verified Activity Pack Bundles.
pub struct PackBundleStoreV1 {
    root: PathBuf,
}

impl PackBundleStoreV1 {
    /// Reads bounded retained installation intent without opening objects,
    /// checking approvals, or creating a storage tree. These are unverified
    /// metadata, never startup admission or proof of a running registry.
    ///
    /// # Errors
    /// Rejects malformed, duplicate, noncanonical or unreadable inventory.
    pub fn read_inventory_metadata(
        root: &Path,
    ) -> Result<Vec<InstalledPackBundleV1>, PackBundleErrorV1> {
        let inventory = root.join("inventory");
        match fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            Ok(_) => {}
        }
        let metadata = fs::symlink_metadata(&inventory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(PackBundleErrorV1::InventoryNotCanonical);
        }
        let mut rows = Vec::new();
        let mut revisions = BTreeSet::new();
        for entry in fs::read_dir(inventory)? {
            if rows.len() == MAX_INSTALLED_BUNDLE_COUNT {
                return Err(PackBundleErrorV1::LimitExceeded);
            }
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            let row: InstalledPackBundleV1 = read_canonical(&entry.path())?;
            if row.inventory_record_id != INVENTORY_RECORD_ID
                || entry.file_name()
                    != std::ffi::OsStr::new(&format!("{}.json", row.bundle_digest.path_component()))
                || !revisions.insert(row.revision_digest.clone())
            {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            rows.push(row);
        }
        rows.sort_by(|left, right| left.bundle_digest.cmp(&right.bundle_digest));
        Ok(rows)
    }

    /// Opens or creates the closed Activity Pack storage tree. The containing
    /// runtime data directory must already have passed the runtime filesystem
    /// and ownership checks.
    ///
    /// # Errors
    ///
    /// Returns a filesystem error if the complete storage tree cannot be
    /// created.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, PackBundleErrorV1> {
        let store = Self { root: root.into() };
        for directory in [
            store.objects_root(),
            store.approvals_root(),
            store.inventory_root(),
            store.tombstones_root(),
            store.staging_root(),
        ] {
            fs::create_dir_all(directory)?;
        }
        Ok(store)
    }

    /// Performs complete read-only inspection of a local candidate.
    ///
    /// # Errors
    ///
    /// Returns a bounded-read or bundle-verification error.
    pub fn inspect_path(&self, path: &Path) -> Result<PackBundleInspectionV1, PackBundleErrorV1> {
        Ok(Self::verify_path(path)?.inspection())
    }

    /// Captures the complete installed registry input once, in exact physical
    /// digest order. Selectable rows must still have current exact approval;
    /// retained-only rows remain loadable after revocation.
    ///
    /// Later install, approval, or selectability changes are deliberately not
    /// reflected in this snapshot and require daemon restart.
    ///
    /// # Errors
    ///
    /// Returns an error when the inventory exceeds its fixed bound, contains
    /// a noncanonical entry, duplicates a semantic revision, lacks approval
    /// for a selectable row, or references missing/corrupt exact bytes.
    pub fn load_startup_inventory(
        &self,
    ) -> Result<PackBundleStartupInventoryV1, PackBundleErrorV1> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(self.inventory_root())? {
            if paths.len() == MAX_INSTALLED_BUNDLE_COUNT {
                return Err(PackBundleErrorV1::LimitExceeded);
            }
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| PackBundleErrorV1::InventoryNotCanonical)?;
            let Some(component) = name.strip_suffix(".json") else {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            };
            let digest = PackBundleDigestV1::from_str(&format!("blake3:{component}"))
                .map_err(|_| PackBundleErrorV1::InventoryNotCanonical)?;
            if name != format!("{}.json", digest.path_component()) {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            paths.push((digest, entry.path()));
        }
        paths.sort_by(|left, right| left.0.cmp(&right.0));

        let mut entries = Vec::with_capacity(paths.len());
        let mut revisions = BTreeSet::new();
        let mut selectable = 0_usize;
        for (expected_digest, path) in paths {
            let installed: InstalledPackBundleV1 = read_canonical(&path)?;
            if installed.inventory_record_id != INVENTORY_RECORD_ID
                || installed.bundle_digest != expected_digest
            {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            let bundle = self.load_installed(&expected_digest)?;
            if !revisions.insert(bundle.revision_digest().clone()) {
                return Err(PackBundleErrorV1::SemanticRevisionAlreadyInstalled);
            }
            let approval = self.current_approval(&bundle)?;
            if installed.install_state == PackInstallStateV1::Selectable {
                match approval {
                    CurrentApprovalV1::Approved => {}
                    CurrentApprovalV1::Missing => {
                        return Err(PackBundleErrorV1::ApprovalMissing);
                    }
                    CurrentApprovalV1::Revoked => {
                        return Err(PackBundleErrorV1::ApprovalDigestMismatch);
                    }
                }
                selectable = selectable
                    .checked_add(1)
                    .ok_or(PackBundleErrorV1::LimitExceeded)?;
            }
            entries.push(PackBundleStartupEntryV1 {
                installed,
                bundle,
                approved_for_activity_start: approval == CurrentApprovalV1::Approved,
            });
        }
        let installed = entries.len();
        let retained_only = installed
            .checked_sub(selectable)
            .ok_or(PackBundleErrorV1::InventoryNotCanonical)?;
        Ok(PackBundleStartupInventoryV1 {
            entries,
            counts: PackBundleInventoryCountsV1 {
                installed,
                selectable,
                retained_only,
            },
        })
    }

    /// Records or revokes Host Operator approval for exact verified bytes.
    ///
    /// # Errors
    ///
    /// Returns a verification, validation, or durable-write error.
    pub fn approve_path(
        &self,
        path: &Path,
        approval: OperatorApprovalV1,
    ) -> Result<PackBundleDigestV1, PackBundleErrorV1> {
        let verified = Self::verify_path(path)?;
        self.record_approval(&verified, approval)?;
        Ok(verified.bundle_digest().clone())
    }

    /// Records or revokes approval without re-reading an already verified
    /// in-memory artifact.
    ///
    /// # Errors
    ///
    /// Returns a validation or durable-write error.
    pub fn record_approval(
        &self,
        bundle: &VerifiedPackBundleV1,
        approval: OperatorApprovalV1,
    ) -> Result<(), PackBundleErrorV1> {
        validate_operator_fields(&approval.operator_id, &approval.decided_at)?;
        self.clear_startup_readiness()?;
        let record = ApprovalRecordV1 {
            approval_record_id: APPROVAL_RECORD_ID.to_owned(),
            bundle_digest: bundle.bundle_digest().clone(),
            decided_at: approval.decided_at,
            decision: approval.decision,
            manifest_digest: bundle.manifest_digest().clone(),
            operator_id: approval.operator_id,
        };
        atomic_write_canonical(
            &self.approvals_root(),
            &self.approval_path(bundle.bundle_digest()),
            &record,
        )?;
        if approval.decision == ApprovalDecisionV1::Revoked {
            let inventory_path = self.inventory_path(bundle.bundle_digest());
            if inventory_path.exists() {
                let mut installed: InstalledPackBundleV1 = read_canonical(&inventory_path)?;
                installed.install_state = PackInstallStateV1::RetainedOnly;
                atomic_write_canonical(&self.inventory_root(), &inventory_path, &installed)?;
            }
        }
        Ok(())
    }

    /// Installs only bytes that currently have an exact approved record.
    /// Installation changes the next startup snapshot; it never mutates a
    /// registry already serving Rooms in the current process.
    ///
    /// # Errors
    ///
    /// Returns an error if verification, approval, collision checks, staged
    /// re-verification, or durable publication fails.
    pub fn install_approved(
        &self,
        path: &Path,
        installed_at: impl Into<String>,
    ) -> Result<InstalledPackBundleV1, PackBundleErrorV1> {
        self.clear_startup_readiness()?;
        let installed_at = installed_at.into();
        if installed_at.is_empty() {
            return Err(PackBundleErrorV1::TypedJson(
                "installed_at must be nonempty".to_owned(),
            ));
        }
        let bundle = Self::verify_path(path)?;
        self.require_approval(&bundle)?;
        self.reject_semantic_substitution(&bundle)?;
        self.install_object(&bundle)?;
        let installed = InstalledPackBundleV1 {
            bundle_digest: bundle.bundle_digest().clone(),
            installed_at,
            install_state: PackInstallStateV1::RetainedOnly,
            inventory_record_id: INVENTORY_RECORD_ID.to_owned(),
            revision_digest: bundle.revision_digest().clone(),
        };
        atomic_write_canonical(
            &self.inventory_root(),
            &self.inventory_path(bundle.bundle_digest()),
            &installed,
        )?;
        Ok(installed)
    }

    /// Restores one byte-identical archive as retained-only inventory without
    /// copying approval or selectability state from the source deployment.
    ///
    /// Complete verification is repeated before staging and again inside the
    /// ordinary atomic object-install path. Existing physical bytes are
    /// accepted only when identical, and a different physical archive for the
    /// same semantic revision fails closed.
    ///
    /// # Errors
    ///
    /// Returns a verification, substitution, or durable-publication error.
    pub fn restore_retained(
        &self,
        artifact: &RetainedPackBundleArtifactV1,
        restored_at: impl Into<String>,
    ) -> Result<InstalledPackBundleV1, PackBundleErrorV1> {
        self.clear_startup_readiness()?;
        let restored_at = restored_at.into();
        if restored_at.is_empty() {
            return Err(PackBundleErrorV1::TypedJson(
                "restored_at must be nonempty".to_owned(),
            ));
        }
        let bundle = artifact.verify()?;
        self.reject_semantic_substitution(&bundle)?;
        self.install_object(&bundle)?;
        let installed = InstalledPackBundleV1 {
            bundle_digest: bundle.bundle_digest().clone(),
            installed_at: restored_at,
            install_state: PackInstallStateV1::RetainedOnly,
            inventory_record_id: INVENTORY_RECORD_ID.to_owned(),
            revision_digest: bundle.revision_digest().clone(),
        };
        atomic_write_canonical(
            &self.inventory_root(),
            &self.inventory_path(bundle.bundle_digest()),
            &installed,
        )?;
        Ok(installed)
    }

    /// Loads and re-verifies original installed bytes. Inventory alone never
    /// grants execution authority.
    ///
    /// # Errors
    ///
    /// Returns an error if inventory is absent or exact installed bytes no
    /// longer pass complete verification.
    pub fn load_installed(
        &self,
        digest: &PackBundleDigestV1,
    ) -> Result<VerifiedPackBundleV1, PackBundleErrorV1> {
        let inventory_path = self.inventory_path(digest);
        if !inventory_path.is_file() {
            return Err(PackBundleErrorV1::NotInstalled);
        }
        let inventory: InstalledPackBundleV1 = read_canonical(&inventory_path)?;
        if inventory.inventory_record_id != INVENTORY_RECORD_ID
            || inventory.bundle_digest != *digest
        {
            return Err(PackBundleErrorV1::CorruptInstalledObject);
        }
        let object_path = self.object_path(digest);
        let bundle = Self::verify_path(&object_path)
            .map_err(|_| PackBundleErrorV1::CorruptInstalledObject)?;
        if bundle.bundle_digest() != digest
            || bundle.revision_digest() != &inventory.revision_digest
        {
            return Err(PackBundleErrorV1::CorruptInstalledObject);
        }
        Ok(bundle)
    }

    /// Changes only new-Room selection state. Retained execution is never
    /// disabled by this operation. The change takes effect after daemon
    /// restart; it never mutates a registry already serving Rooms.
    ///
    /// # Errors
    ///
    /// Returns an error if the bundle is not installed, is corrupt, lacks
    /// current approval when selected, or inventory cannot be persisted.
    pub fn set_selectable(
        &self,
        digest: &PackBundleDigestV1,
        selectable: bool,
    ) -> Result<InstalledPackBundleV1, PackBundleErrorV1> {
        self.clear_startup_readiness()?;
        let bundle = self.load_installed(digest)?;
        if selectable {
            self.require_approval(&bundle)?;
        }
        let path = self.inventory_path(digest);
        let mut inventory: InstalledPackBundleV1 = read_canonical(&path)?;
        inventory.install_state = if selectable {
            PackInstallStateV1::Selectable
        } else {
            PackInstallStateV1::RetainedOnly
        };
        atomic_write_canonical(&self.inventory_root(), &path, &inventory)?;
        Ok(inventory)
    }

    /// Exports byte-identical original archive bytes and no approval state.
    ///
    /// # Errors
    ///
    /// Returns an installed-object verification or output-write error.
    pub fn export_exact(
        &self,
        digest: &PackBundleDigestV1,
        mut writer: impl Write,
    ) -> Result<(), PackBundleErrorV1> {
        let bundle = self.load_installed(digest)?;
        writer.write_all(bundle.archive_bytes())?;
        Ok(())
    }

    /// Removes an exact bundle only after the selected storage adapter proves
    /// that no retained Room lineage references its semantic revision.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference proof fails or reports a reference,
    /// or if installed-object verification or durable removal fails.
    pub fn remove_if_unreferenced(
        &self,
        digest: &PackBundleDigestV1,
        references: &dyn RetainedRevisionSourceV1,
        removal: PackRemovalV1,
    ) -> Result<(), PackBundleErrorV1> {
        validate_operator_fields(&removal.operator_id, &removal.removed_at)?;
        let bundle = self.load_installed(digest)?;
        if references.is_revision_referenced(bundle.revision_digest())? {
            return Err(PackBundleErrorV1::RevisionReferenced);
        }
        self.clear_startup_readiness()?;
        let tombstone = TombstoneRecordV1 {
            bundle_digest: digest.clone(),
            operator_id: removal.operator_id,
            removed_at: removal.removed_at,
            revision_digest: bundle.revision_digest().clone(),
            tombstone_record_id: TOMBSTONE_RECORD_ID.to_owned(),
        };
        let object_path = self.object_path(digest);
        fs::remove_file(&object_path)?;
        let object_directory = object_path
            .parent()
            .ok_or(PackBundleErrorV1::CorruptInstalledObject)?;
        fs::remove_dir(object_directory)?;
        remove_if_present(&self.inventory_path(digest))?;
        sync_directory(&self.objects_root())?;
        sync_directory(&self.inventory_root())?;
        atomic_write_canonical(
            &self.tombstones_root(),
            &self.tombstone_path(digest),
            &tombstone,
        )?;
        Ok(())
    }

    /// Atomically records the exact inventory/target pair admitted by the
    /// offline restart-readiness proof.
    ///
    /// # Errors
    ///
    /// Returns a validation or durable-write failure.
    pub fn record_startup_readiness(
        &self,
        seal: &PackStartupReadinessSealV1,
    ) -> Result<(), PackBundleErrorV1> {
        validate_readiness_seal(seal)?;
        atomic_write_canonical(&self.root, &self.readiness_path(), seal)
    }

    /// Loads the canonical startup seal when one has been produced.
    ///
    /// # Errors
    ///
    /// Returns a canonical or filesystem error for a malformed retained seal.
    pub fn startup_readiness(
        &self,
    ) -> Result<Option<PackStartupReadinessSealV1>, PackBundleErrorV1> {
        Self::read_startup_readiness(&self.root)
    }

    /// Reads a readiness seal without creating or repairing any Pack store
    /// path. This is the read-only operator-inspection entry point.
    ///
    /// # Errors
    ///
    /// Returns a canonical, filesystem, or validation error for a present but
    /// malformed seal.
    pub fn read_startup_readiness(
        root: &Path,
    ) -> Result<Option<PackStartupReadinessSealV1>, PackBundleErrorV1> {
        let path = root.join("restart-readiness-v1.json");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                return Err(PackBundleErrorV1::InventoryNotCanonical);
            }
            Ok(_) => {}
        }
        let seal: PackStartupReadinessSealV1 = read_canonical(&path)?;
        validate_readiness_seal(&seal)?;
        Ok(Some(seal))
    }

    /// Removes any previous readiness seal before inventory authority changes.
    ///
    /// # Errors
    ///
    /// Returns a durable filesystem failure.
    pub fn clear_startup_readiness(&self) -> Result<(), PackBundleErrorV1> {
        remove_if_present(&self.readiness_path())?;
        sync_directory(&self.root)
    }

    fn verify_path(path: &Path) -> Result<VerifiedPackBundleV1, PackBundleErrorV1> {
        let bytes = read_bounded(path, MAX_BUNDLE_BYTES)?;
        PackBundleVerifierV1.inspect(Arc::from(bytes))
    }

    fn require_approval(&self, bundle: &VerifiedPackBundleV1) -> Result<(), PackBundleErrorV1> {
        match self.current_approval(bundle)? {
            CurrentApprovalV1::Approved => Ok(()),
            CurrentApprovalV1::Missing => Err(PackBundleErrorV1::ApprovalMissing),
            CurrentApprovalV1::Revoked => Err(PackBundleErrorV1::ApprovalDigestMismatch),
        }
    }

    fn current_approval(
        &self,
        bundle: &VerifiedPackBundleV1,
    ) -> Result<CurrentApprovalV1, PackBundleErrorV1> {
        let path = self.approval_path(bundle.bundle_digest());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CurrentApprovalV1::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(PackBundleErrorV1::ApprovalDigestMismatch);
        }
        let approval: ApprovalRecordV1 = read_canonical(&path)?;
        if approval.approval_record_id != APPROVAL_RECORD_ID
            || approval.bundle_digest != *bundle.bundle_digest()
            || approval.manifest_digest != *bundle.manifest_digest()
        {
            return Err(PackBundleErrorV1::ApprovalDigestMismatch);
        }
        Ok(match approval.decision {
            ApprovalDecisionV1::Approved => CurrentApprovalV1::Approved,
            ApprovalDecisionV1::Revoked => CurrentApprovalV1::Revoked,
        })
    }

    fn reject_semantic_substitution(
        &self,
        candidate: &VerifiedPackBundleV1,
    ) -> Result<(), PackBundleErrorV1> {
        for entry in fs::read_dir(self.inventory_root())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                return Err(PackBundleErrorV1::CorruptInstalledObject);
            }
            let installed: InstalledPackBundleV1 = read_canonical(&entry.path())?;
            if installed.revision_digest == *candidate.revision_digest()
                && installed.bundle_digest != *candidate.bundle_digest()
            {
                return Err(PackBundleErrorV1::SemanticRevisionAlreadyInstalled);
            }
        }
        Ok(())
    }

    fn install_object(&self, bundle: &VerifiedPackBundleV1) -> Result<(), PackBundleErrorV1> {
        let target = self.object_path(bundle.bundle_digest());
        if target.exists() {
            let installed = Self::verify_path(&target)
                .map_err(|_| PackBundleErrorV1::CorruptInstalledObject)?;
            if installed.bundle_digest() == bundle.bundle_digest() {
                return Ok(());
            }
            return Err(PackBundleErrorV1::CorruptInstalledObject);
        }

        let stage = tempfile::Builder::new()
            .prefix("install-")
            .tempdir_in(self.staging_root())?;
        let staged_object = stage.path().join("bundle.wspack");
        let mut file = File::create(&staged_object)?;
        file.write_all(bundle.archive_bytes())?;
        file.sync_all()?;
        sync_directory(stage.path())?;
        let staged = Self::verify_path(&staged_object)
            .map_err(|_| PackBundleErrorV1::StagedObjectMismatch)?;
        if staged.bundle_digest() != bundle.bundle_digest()
            || staged.archive_bytes() != bundle.archive_bytes()
        {
            return Err(PackBundleErrorV1::StagedObjectMismatch);
        }

        let target_directory = target
            .parent()
            .ok_or(PackBundleErrorV1::CorruptInstalledObject)?;
        match fs::rename(stage.path(), target_directory) {
            Ok(()) => {
                sync_directory(&self.objects_root())?;
                Ok(())
            }
            Err(_error) if target.is_file() => {
                let installed = Self::verify_path(&target)
                    .map_err(|_| PackBundleErrorV1::CorruptInstalledObject)?;
                if installed.bundle_digest() == bundle.bundle_digest() {
                    Ok(())
                } else {
                    Err(PackBundleErrorV1::CorruptInstalledObject)
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    fn objects_root(&self) -> PathBuf {
        self.root.join("objects").join("blake3")
    }

    fn approvals_root(&self) -> PathBuf {
        self.root.join("approvals")
    }

    fn inventory_root(&self) -> PathBuf {
        self.root.join("inventory")
    }

    fn tombstones_root(&self) -> PathBuf {
        self.root.join("tombstones")
    }

    fn staging_root(&self) -> PathBuf {
        self.root.join(".staging")
    }

    fn object_path(&self, digest: &PackBundleDigestV1) -> PathBuf {
        self.objects_root()
            .join(digest.path_component())
            .join("bundle.wspack")
    }

    fn approval_path(&self, digest: &PackBundleDigestV1) -> PathBuf {
        self.approvals_root()
            .join(format!("{}.json", digest.path_component()))
    }

    fn inventory_path(&self, digest: &PackBundleDigestV1) -> PathBuf {
        self.inventory_root()
            .join(format!("{}.json", digest.path_component()))
    }

    fn readiness_path(&self) -> PathBuf {
        self.root.join("restart-readiness-v1.json")
    }

    fn tombstone_path(&self, digest: &PackBundleDigestV1) -> PathBuf {
        self.tombstones_root()
            .join(format!("{}.json", digest.path_component()))
    }
}

fn validate_operator_fields(operator_id: &str, recorded_at: &str) -> Result<(), PackBundleErrorV1> {
    if operator_id.is_empty() || recorded_at.is_empty() {
        return Err(PackBundleErrorV1::TypedJson(
            "operator identity and recorded time must be nonempty".to_owned(),
        ));
    }
    Ok(())
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, PackBundleErrorV1> {
    let mut file = File::open(path)?;
    if file.metadata()?.len()
        > u64::try_from(maximum).map_err(|_| PackBundleErrorV1::LimitExceeded)?
    {
        return Err(PackBundleErrorV1::LimitExceeded);
    }
    let limit = u64::try_from(maximum)
        .map_err(|_| PackBundleErrorV1::LimitExceeded)?
        .saturating_add(1);
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(PackBundleErrorV1::LimitExceeded);
    }
    Ok(bytes)
}

fn read_canonical<T: DeserializeOwned>(path: &Path) -> Result<T, PackBundleErrorV1> {
    let bytes = read_bounded(path, 64 * 1024)?;
    CanonicalJsonV1::decode_canonical(&bytes).map_err(PackBundleErrorV1::from)
}

fn validate_readiness_seal(seal: &PackStartupReadinessSealV1) -> Result<(), PackBundleErrorV1> {
    if seal.readiness_record_id != STARTUP_READINESS_RECORD_ID {
        return Err(PackBundleErrorV1::StartupReadinessMismatch);
    }
    validate_readiness_identity(
        &seal.inventory_digest,
        &seal.storage_profile,
        &seal.deployment_binding,
    )
}

fn validate_readiness_identity(
    inventory_digest: &str,
    storage_profile: &str,
    deployment_binding: &str,
) -> Result<(), PackBundleErrorV1> {
    PackBundleDigestV1::from_str(inventory_digest)
        .map_err(|_| PackBundleErrorV1::StartupReadinessMismatch)?;
    PackBundleDigestV1::from_str(deployment_binding)
        .map_err(|_| PackBundleErrorV1::StartupReadinessMismatch)?;
    if !matches!(storage_profile, "sqlite-bundled" | "postgres-primary") {
        return Err(PackBundleErrorV1::StartupReadinessMismatch);
    }
    Ok(())
}

fn atomic_write_canonical<T: Serialize>(
    parent: &Path,
    destination: &Path,
    value: &T,
) -> Result<(), PackBundleErrorV1> {
    let serialized = serde_json::to_vec(value)
        .map_err(|error| PackBundleErrorV1::TypedJson(error.to_string()))?;
    let canonical = CanonicalJsonV1::parse(&serialized)?.to_bytes()?;
    let mut staging = NamedTempFile::new_in(parent)?;
    staging.write_all(&canonical)?;
    staging.as_file().sync_all()?;
    staging
        .persist(destination)
        .map_err(|error| PackBundleErrorV1::Io(error.error))?;
    sync_directory(parent)?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), PackBundleErrorV1> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn sync_directory(path: &Path) -> Result<(), PackBundleErrorV1> {
    File::open(path)?.sync_all()?;
    Ok(())
}
