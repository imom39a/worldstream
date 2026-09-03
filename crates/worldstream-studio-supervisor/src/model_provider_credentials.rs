//! Owner-installed named model-provider credentials with browser-safe views.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    agent_profiles::ManagedReferenceProviderV1,
    secrets::{FileSecretVaultV1, SecretAvailabilityV1, SecretKindV1, SecretReferenceV1},
};

const SCHEMA: &str = "worldstream/model-provider-credential/v1";
const CATALOG_SCHEMA: &str = "worldstream/studio-model-provider-credential-catalog/v1";
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_ROWS: usize = 256;
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestV1 {
    schema: String,
    credential_id: String,
    display_name: String,
    provider: ManagedReferenceProviderV1,
    secret: SecretV1,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SecretV1 {
    kind: SecretKindV1,
    reference: SecretReferenceV1,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProviderCredentialViewV1 {
    pub credential_id: String,
    pub display_name: String,
    pub provider: ManagedReferenceProviderV1,
    pub availability: CredentialAvailabilityV1,
}
#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAvailabilityV1 {
    Configured,
    Missing,
    Unavailable,
}
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProviderCredentialCatalogV1 {
    pub schema: String,
    pub credentials: Vec<ModelProviderCredentialViewV1>,
}
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ModelProviderCredentialErrorV1 {
    #[error("model-provider credential is invalid")]
    Invalid,
    #[error("model-provider credential revision is immutable")]
    Immutable,
    #[error("model-provider credential was not found")]
    NotFound,
    #[error("model-provider credential is unavailable")]
    Unavailable,
}

#[derive(Clone)]
pub struct ModelProviderCredentialRegistryV1 {
    entries: Arc<BTreeMap<String, ManifestV1>>,
    vault: FileSecretVaultV1,
}
impl ModelProviderCredentialRegistryV1 {
    /// Opens the immutable installed credential catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the owner manifests or installed copies are
    /// malformed, violate immutable publication rules, or cannot be read.
    pub fn open(
        installed_root: &Path,
        owner_manifests: &Path,
        vault: FileSecretVaultV1,
    ) -> Result<Self, ModelProviderCredentialErrorV1> {
        let installed = prepare_data_directory(installed_root)
            .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
        recover_interrupted_publications(&installed)?;
        let mut supplied = BTreeSet::new();
        for source in json_files(owner_manifests, false)? {
            let manifest = read_manifest(&source, false)?;
            let expected_name = format!("{}.json", manifest.credential_id);
            if source.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
                || !supplied.insert(manifest.credential_id.clone())
            {
                return Err(ModelProviderCredentialErrorV1::Invalid);
            }
            let target = installed.join(format!("{}.json", manifest.credential_id));
            if target.exists() {
                if read_manifest(&target, true)? != manifest {
                    return Err(ModelProviderCredentialErrorV1::Immutable);
                }
            } else {
                publish_manifest(&target, &manifest)?;
            }
        }
        Self::open_installed(&installed, vault)
    }

    /// Opens only retained credential declarations, without importing sources.
    /// A fresh empty catalog does not create credentials or confer approval.
    ///
    /// # Errors
    /// Rejects malformed retained records or unavailable protected storage.
    pub fn open_installed(
        installed_root: &Path,
        vault: FileSecretVaultV1,
    ) -> Result<Self, ModelProviderCredentialErrorV1> {
        let installed = prepare_data_directory(installed_root)
            .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
        recover_interrupted_publications(&installed)?;
        let mut entries = BTreeMap::new();
        for path in json_files(&installed, true)? {
            let value = read_manifest(&path, true)?;
            if path != installed.join(format!("{}.json", value.credential_id))
                || entries.insert(value.credential_id.clone(), value).is_some()
            {
                return Err(ModelProviderCredentialErrorV1::Invalid);
            }
        }
        Ok(Self {
            entries: Arc::new(entries),
            vault,
        })
    }
    #[must_use]
    pub fn catalog(&self) -> ModelProviderCredentialCatalogV1 {
        ModelProviderCredentialCatalogV1 {
            schema: CATALOG_SCHEMA.to_owned(),
            credentials: self
                .entries
                .values()
                .map(|entry| ModelProviderCredentialViewV1 {
                    credential_id: entry.credential_id.clone(),
                    display_name: entry.display_name.clone(),
                    provider: entry.provider,
                    availability: match self
                        .vault
                        .inspect(entry.secret.kind, &entry.secret.reference)
                        .availability
                    {
                        SecretAvailabilityV1::Configured => CredentialAvailabilityV1::Configured,
                        SecretAvailabilityV1::Missing => CredentialAvailabilityV1::Missing,
                        SecretAvailabilityV1::Unavailable => CredentialAvailabilityV1::Unavailable,
                    },
                })
                .collect(),
        }
    }
    /// Resolves a named credential to its opaque secret reference.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential does not exist, is incompatible
    /// with the requested provider, or its secret cannot be used.
    pub fn resolve(
        &self,
        credential_id: &str,
        provider: ManagedReferenceProviderV1,
    ) -> Result<SecretReferenceV1, ModelProviderCredentialErrorV1> {
        let entry = self
            .entries
            .get(credential_id)
            .ok_or(ModelProviderCredentialErrorV1::NotFound)?;
        if entry.provider != provider || entry.secret.kind != SecretKindV1::ModelProvider {
            return Err(ModelProviderCredentialErrorV1::Invalid);
        }
        match self
            .vault
            .inspect(entry.secret.kind, &entry.secret.reference)
            .availability
        {
            SecretAvailabilityV1::Configured => Ok(entry.secret.reference.clone()),
            SecretAvailabilityV1::Missing => Err(ModelProviderCredentialErrorV1::NotFound),
            SecretAvailabilityV1::Unavailable => Err(ModelProviderCredentialErrorV1::Unavailable),
        }
    }
}
pub fn model_provider_credential_router(registry: ModelProviderCredentialRegistryV1) -> Router {
    Router::new()
        .route(
            "/api/v1/model-provider-credentials",
            get(
                |State(registry): State<ModelProviderCredentialRegistryV1>| async move {
                    Json(registry.catalog())
                },
            ),
        )
        .with_state(registry)
}
fn json_files(
    root: &Path,
    protected: bool,
) -> Result<Vec<PathBuf>, ModelProviderCredentialErrorV1> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(root)
        .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
    if entries.len() > MAX_ROWS {
        return Err(ModelProviderCredentialErrorV1::Unavailable);
    }
    let mut paths = Vec::with_capacity(entries.len());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
        let path = entry.path();
        if file_type.is_file() && path.extension().and_then(|v| v.to_str()) == Some("json") {
            paths.push(path);
        } else if protected && file_type.is_file() && is_reserved_temporary(&path) {
            validate_owner_only_file(&path).map_err(|_| ModelProviderCredentialErrorV1::Invalid)?;
        } else {
            return Err(ModelProviderCredentialErrorV1::Invalid);
        }
    }
    paths.sort();
    Ok(paths)
}

fn recover_interrupted_publications(root: &Path) -> Result<(), ModelProviderCredentialErrorV1> {
    let mut removed = false;
    for entry in fs::read_dir(root).map_err(|_| ModelProviderCredentialErrorV1::Unavailable)? {
        let entry = entry.map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?
            .is_file()
            || !is_reserved_temporary(&path)
        {
            continue;
        }
        validate_owner_only_file(&path).map_err(|_| ModelProviderCredentialErrorV1::Invalid)?;
        fs::remove_file(path).map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
        removed = true;
    }
    if removed {
        sync_directory(root).map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
    }
    Ok(())
}

fn is_reserved_temporary(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let mut parts = name.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(""), Some("credential"), Some(pid), Some(counter), Some("tmp"))
            if !pid.is_empty()
                && !counter.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
                && counter.bytes().all(|byte| byte.is_ascii_digit())
    ) && parts.next().is_none()
}
fn read_manifest(
    path: &Path,
    protected: bool,
) -> Result<ManifestV1, ModelProviderCredentialErrorV1> {
    if protected {
        validate_owner_only_file(path).map_err(|_| ModelProviderCredentialErrorV1::Invalid)?;
    }
    let metadata = fs::metadata(path).map_err(|_| ModelProviderCredentialErrorV1::Invalid)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_RECORD_BYTES {
        return Err(ModelProviderCredentialErrorV1::Invalid);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    fs::File::open(path)
        .map_err(|_| ModelProviderCredentialErrorV1::Invalid)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ModelProviderCredentialErrorV1::Invalid)?;
    if bytes.is_empty() || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
        return Err(ModelProviderCredentialErrorV1::Invalid);
    }
    let value: ManifestV1 =
        serde_json::from_slice(&bytes).map_err(|_| ModelProviderCredentialErrorV1::Invalid)?;
    if value.schema != SCHEMA
        || !valid_id(&value.credential_id)
        || !valid_text(&value.display_name)
        || value.secret.kind != SecretKindV1::ModelProvider
    {
        return Err(ModelProviderCredentialErrorV1::Invalid);
    }
    Ok(value)
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && matches!(byte, b'_' | b'-'))
        })
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.contains(['\0', '\r', '\n'])
}

fn publish_manifest(
    target: &Path,
    manifest: &ManifestV1,
) -> Result<(), ModelProviderCredentialErrorV1> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
    let parent = target
        .parent()
        .ok_or(ModelProviderCredentialErrorV1::Unavailable)?;
    let temporary = parent.join(format!(
        ".credential.{}.{}.tmp",
        std::process::id(),
        TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = create_owner_only_file(&temporary)
        .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)?;
    let written = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(ModelProviderCredentialErrorV1::Unavailable);
    }
    match fs::hard_link(&temporary, target) {
        Ok(()) => {
            let publication = sync_directory(parent);
            let _ = fs::remove_file(&temporary);
            let cleanup = sync_directory(parent);
            publication
                .and(cleanup)
                .map_err(|_| ModelProviderCredentialErrorV1::Unavailable)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temporary);
            Err(ModelProviderCredentialErrorV1::Immutable)
        }
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            Err(ModelProviderCredentialErrorV1::Unavailable)
        }
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};
    OpenOptions::new()
        .read(true)
        .custom_flags(0x0200_0000)
        .open(path)?
        .sync_all()
}

#[cfg(test)]
mod tests {
    use super::{
        ModelProviderCredentialErrorV1, ModelProviderCredentialRegistryV1, read_manifest, valid_id,
    };
    use crate::secrets::{FileSecretVaultV1, SecretKindV1};
    use std::io::Write as _;

    #[test]
    fn credential_identity_is_nonempty_and_bounded() {
        assert!(!valid_id(""));
        assert!(valid_id("local-openai"));
        assert!(!valid_id(&"a".repeat(65)));
    }

    #[test]
    fn oversized_manifest_is_rejected_before_json_parsing() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let path = directory.path().join("oversized.json");
        let mut file =
            std::fs::File::create(&path).unwrap_or_else(|error| unreachable!("manifest: {error}"));
        file.write_all(&vec![b'x'; 64 * 1024 + 1])
            .unwrap_or_else(|error| unreachable!("manifest bytes: {error}"));
        assert!(matches!(
            read_manifest(&path, false),
            Err(ModelProviderCredentialErrorV1::Invalid)
        ));
    }

    #[test]
    fn interrupted_publication_recovers_before_and_after_target_without_accepting_changes() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
            .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
        let reference = vault
            .store(SecretKindV1::ModelProvider, b"provider")
            .unwrap_or_else(|error| unreachable!("secret: {error:?}"));
        let source = directory.path().join("config");
        std::fs::create_dir_all(&source).unwrap_or_else(|error| unreachable!("source: {error}"));
        let manifest = serde_json::json!({
            "schema": "worldstream/model-provider-credential/v1",
            "credential_id": "local-openai",
            "display_name": "Local OpenAI",
            "provider": "open_ai_compatible",
            "secret": { "kind": "model_provider", "reference": reference.as_str() }
        });
        std::fs::write(
            source.join("local-openai.json"),
            serde_json::to_vec(&manifest).unwrap_or_else(|error| unreachable!("manifest: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("write: {error}"));
        let installed =
            worldstream_runtime::prepare_data_directory(&directory.path().join("installed"))
                .unwrap_or_else(|error| unreachable!("installed: {error}"));
        let temporary = installed.join(".credential.1.1.tmp");
        let _ = worldstream_runtime::create_owner_only_file(&temporary)
            .unwrap_or_else(|error| unreachable!("temporary: {error}"));
        let first = ModelProviderCredentialRegistryV1::open(&installed, &source, vault.clone())
            .unwrap_or_else(|error| unreachable!("recover before target: {error:?}"));
        assert_eq!(first.catalog().credentials.len(), 1);
        assert!(!temporary.exists());
        let temporary_after = installed.join(".credential.1.2.tmp");
        let _ = worldstream_runtime::create_owner_only_file(&temporary_after)
            .unwrap_or_else(|error| unreachable!("temporary after: {error}"));
        let restarted = ModelProviderCredentialRegistryV1::open(&installed, &source, vault.clone())
            .unwrap_or_else(|error| unreachable!("recover after target: {error:?}"));
        assert_eq!(
            restarted.resolve(
                "local-openai",
                crate::agent_profiles::ManagedReferenceProviderV1::OpenAiCompatible
            ),
            Ok(reference)
        );
        assert!(!temporary_after.exists());
        let changed = serde_json::json!({
            "schema": "worldstream/model-provider-credential/v1",
            "credential_id": "local-openai",
            "display_name": "Changed identity",
            "provider": "open_ai_compatible",
            "secret": { "kind": "model_provider", "reference": restarted.resolve("local-openai", crate::agent_profiles::ManagedReferenceProviderV1::OpenAiCompatible).unwrap_or_else(|error| unreachable!("reference: {error:?}")).as_str() }
        });
        std::fs::write(
            source.join("local-openai.json"),
            serde_json::to_vec(&changed)
                .unwrap_or_else(|error| unreachable!("changed manifest: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("changed write: {error}"));
        assert!(matches!(
            ModelProviderCredentialRegistryV1::open(&installed, &source, vault),
            Err(ModelProviderCredentialErrorV1::Immutable)
        ));
    }
}
