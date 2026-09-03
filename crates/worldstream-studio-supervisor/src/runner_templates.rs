//! Immutable owner-installed Runner Template revisions and bounded instances.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_protocol::{BearerWireV1, PackReference, UlidString};
use worldstream_runtime::{create_owner_only_file, prepare_data_directory};
use zeroize::Zeroize as _;

use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const MANIFEST_SCHEMA_V1: &str = "worldstream/runner-template/v1";
const CATALOG_SCHEMA_V1: &str = "worldstream/studio-runner-template-catalog/v1";
const STATUS_SCHEMA_V1: &str = "worldstream/studio-runner-instance-status/v1";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 256;
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 4096;
const MAX_HTTP_RESPONSE_BYTES: u64 = 16 * 1024;

/// Exact compatibility decision for an Activity Pack assignment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityV1 {
    /// The exact Activity Pack revision is declared by the template.
    Compatible,
    /// The exact Activity Pack revision is not declared by the template.
    Incompatible,
}

/// Closed result of binding an exact Task Runner authority to one managed instance.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ManagedRunnerBindingErrorV1 {
    #[error("managed Runner instance was not found")]
    NotFound,
    #[error("managed Runner instance is already bound")]
    Conflict,
    #[error("managed Runner binding is unavailable")]
    Unavailable,
}

/// Approved executable identity retained only in the local registry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerExecutableV1 {
    /// Owner-installed executable path. This never crosses the browser API.
    pub path: PathBuf,
    /// Exact BLAKE3 digest of the approved executable bytes.
    pub blake3: String,
}

/// Exact supported Activity Pack revisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerCompatibilityRuleV1 {
    /// Stable Activity Pack identity.
    pub activity_pack_id: String,
    /// Closed set of exact supported semantic revisions.
    pub exact_revisions: Vec<String>,
}

/// Bounded Runner capacity declared by the template revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerCapacityV1 {
    /// Maximum concurrent Invocations served by one instance.
    pub maximum_concurrent_invocations: u32,
}

/// Local HTTP health contract shared by the template's instances.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerHealthContractV1 {
    /// Fixed health path served by every instance.
    pub path: String,
    /// Per-probe timeout.
    pub timeout_ms: u64,
    /// Age after which the last successful observation is stale.
    pub stale_after_ms: u64,
}

/// One secret-bearing environment setting represented only by an opaque ref.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerSecretSettingV1 {
    /// Fixed environment key supplied to the approved process.
    pub key: String,
    /// Credential authority boundary.
    pub kind: SecretKindV1,
    /// Opaque Supervisor secret reference, never returned by Runner APIs.
    pub reference: SecretReferenceV1,
}

/// One owner-installed instance identity and local health listener.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerInstanceInstallV1 {
    /// Stable installed instance identity used by typed lifecycle routes.
    pub instance_id: String,
    /// Loopback address declared by the owner-installed manifest.
    pub health_address: SocketAddr,
}

/// Immutable owner-installed Runner Template revision manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerTemplateManifestV1 {
    /// Stable manifest schema.
    pub schema: String,
    /// Stable Runner Template identity.
    pub template_id: String,
    /// Immutable exact template revision.
    pub revision: String,
    /// Operator-facing name.
    pub display_name: String,
    /// Approved executable identity retained locally.
    pub executable: RunnerExecutableV1,
    /// Exact supported Activity Pack revisions.
    pub compatibility: Vec<RunnerCompatibilityRuleV1>,
    /// Instance capacity.
    pub capacity: RunnerCapacityV1,
    /// Fixed local health contract.
    pub health: RunnerHealthContractV1,
    /// Explicit non-secret settings.
    pub non_secret_environment: BTreeMap<String, String>,
    /// Secret-bearing settings represented only by Supervisor references.
    pub secret_environment: Vec<RunnerSecretSettingV1>,
    /// Owner-installed instance identities.
    pub instances: Vec<RunnerInstanceInstallV1>,
}

/// Closed owner-install and registry failure vocabulary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunnerTemplateErrorV1 {
    /// A manifest is malformed, unsupported, or outside bounded limits.
    #[error("Runner Template manifest is invalid")]
    InvalidManifest,
    /// An installed exact revision was changed in place.
    #[error("installed Runner Template revision is immutable")]
    ImmutableRevisionConflict,
    /// Owner-controlled local registry storage is unavailable.
    #[error("Runner Template registry is unavailable")]
    RegistryUnavailable,
}

/// Durable immutable registry populated only from owner-controlled files.
#[derive(Clone)]
pub struct RunnerTemplateRegistryV1 {
    templates: Arc<BTreeMap<(String, String), RunnerTemplateManifestV1>>,
    instances: Arc<BTreeMap<String, (String, String, RunnerInstanceInstallV1)>>,
}

impl RunnerTemplateRegistryV1 {
    #[cfg(feature = "cli-operator-preview")]
    pub(crate) fn review_installed(
        root: &Path,
        template_id: &str,
        revision: &str,
    ) -> Result<RunnerTemplateManifestV1, RunnerTemplateErrorV1> {
        if !valid_id(template_id) || !valid_revision(revision) {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
        worldstream_runtime::validate_data_directory(root)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let path = root.join(format!("{template_id}--{revision}.json"));
        worldstream_runtime::validate_owner_only_file(&path)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let manifest = read_manifest(&path, false)?;
        if manifest.template_id != template_id || manifest.revision != revision {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
        validate_manifest(&manifest, true)?;
        Ok(manifest)
    }
    /// Validates selected exact imports and global instance uniqueness without writes.
    #[cfg(feature = "cli-operator-preview")]
    pub(crate) fn check_imports(
        root: &Path,
        selected: &[RunnerTemplateManifestV1],
    ) -> Result<Vec<bool>, RunnerTemplateErrorV1> {
        let mut manifests = BTreeMap::new();
        match fs::symlink_metadata(root) {
            Ok(_) => {
                worldstream_runtime::validate_data_directory(root)
                    .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
                for path in json_files(root)? {
                    worldstream_runtime::validate_owner_only_file(&path)
                        .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
                    let manifest = read_manifest(&path, false)?;
                    let key = (manifest.template_id.clone(), manifest.revision.clone());
                    if manifests.insert(key, manifest).is_some() {
                        return Err(RunnerTemplateErrorV1::InvalidManifest);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(RunnerTemplateErrorV1::RegistryUnavailable),
        }
        let mut reused = Vec::new();
        for manifest in selected {
            validate_manifest(manifest, true)?;
            let key = (manifest.template_id.clone(), manifest.revision.clone());
            match manifests.get(&key) {
                Some(existing) if existing == manifest => reused.push(true),
                Some(_) => return Err(RunnerTemplateErrorV1::ImmutableRevisionConflict),
                None => {
                    manifests.insert(key, manifest.clone());
                    reused.push(false);
                }
            }
        }
        let mut filenames = BTreeSet::new();
        let mut instances = BTreeSet::new();
        for manifest in manifests.values() {
            let filename = format!("{}--{}.json", manifest.template_id, manifest.revision);
            // Retain the installed format while rejecting identities that would
            // publish to the same file, including ASCII case aliases on the
            // supported case-insensitive desktop filesystems.
            #[cfg(any(target_os = "macos", windows))]
            let filename = filename.to_ascii_lowercase();
            if !filenames.insert(filename) {
                return Err(RunnerTemplateErrorV1::ImmutableRevisionConflict);
            }
            for instance in &manifest.instances {
                if !instances.insert(&instance.instance_id) {
                    return Err(RunnerTemplateErrorV1::InvalidManifest);
                }
            }
        }
        Ok(reused)
    }

    /// Publishes only one explicitly reviewed manifest, never a source directory.
    #[cfg(feature = "cli-operator-preview")]
    pub(crate) fn import_exact(
        root: &Path,
        manifest: &RunnerTemplateManifestV1,
    ) -> Result<bool, RunnerTemplateErrorV1> {
        if Self::check_imports(root, std::slice::from_ref(manifest))? == [true] {
            return Ok(true);
        }
        let root =
            prepare_data_directory(root).map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let target = root.join(format!(
            "{}--{}.json",
            manifest.template_id, manifest.revision
        ));
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let temporary = root.join(format!(
            ".runner-import-{}.tmp",
            blake3::hash(&nonce).to_hex()
        ));
        let result = (|| {
            let bytes = serde_json::to_vec_pretty(manifest)
                .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
            let mut file = worldstream_runtime::create_owner_only_renameable_file(&temporary)
                .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
            file.write_all(&bytes)
                .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
            match crate::protected_publication::publish(
                file,
                &temporary,
                &target,
                crate::protected_publication::PublicationMode::CreateNew,
            ) {
                Ok(()) => Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    worldstream_runtime::validate_owner_only_file(&target)
                        .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
                    if read_manifest(&target, false)? == *manifest {
                        Ok(true)
                    } else {
                        Err(RunnerTemplateErrorV1::ImmutableRevisionConflict)
                    }
                }
                Err(_) => Err(RunnerTemplateErrorV1::RegistryUnavailable),
            }
        })();
        let _ = fs::remove_file(temporary);
        result
    }
    /// Opens installed records and installs new owner-provided manifest files.
    ///
    /// # Errors
    ///
    /// Fails closed for invalid manifests, changed exact revisions, unsafe
    /// executable identities, or protected local storage failures.
    pub fn open(
        installed_root: &Path,
        owner_manifests: &Path,
    ) -> Result<Self, RunnerTemplateErrorV1> {
        let installed_root = prepare_data_directory(installed_root)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        for source in json_files(owner_manifests)? {
            let manifest = read_manifest(&source, true)?;
            let target = installed_root.join(format!(
                "{}--{}.json",
                manifest.template_id, manifest.revision
            ));
            if target.exists() {
                if read_manifest(&target, false)? != manifest {
                    return Err(RunnerTemplateErrorV1::ImmutableRevisionConflict);
                }
            } else {
                let encoded = serde_json::to_vec_pretty(&manifest)
                    .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
                let mut file = create_owner_only_file(&target)
                    .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
                file.write_all(&encoded)
                    .and_then(|()| file.sync_all())
                    .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
            }
        }

        Self::open_installed(&installed_root)
    }

    /// Opens only retained exact revisions, without importing owner manifests.
    /// A fresh empty registry does not imply any Runner approval.
    ///
    /// # Errors
    /// Rejects invalid retained records or unavailable protected storage.
    pub fn open_installed(installed_root: &Path) -> Result<Self, RunnerTemplateErrorV1> {
        let installed_root = prepare_data_directory(installed_root)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let mut templates = BTreeMap::new();
        let mut instances = BTreeMap::new();
        for record in json_files(&installed_root)? {
            let manifest = read_manifest(&record, false)?;
            let key = (manifest.template_id.clone(), manifest.revision.clone());
            if templates.insert(key.clone(), manifest.clone()).is_some() {
                return Err(RunnerTemplateErrorV1::InvalidManifest);
            }
            for instance in &manifest.instances {
                if instances
                    .insert(
                        instance.instance_id.clone(),
                        (key.0.clone(), key.1.clone(), instance.clone()),
                    )
                    .is_some()
                {
                    return Err(RunnerTemplateErrorV1::InvalidManifest);
                }
            }
        }
        Ok(Self {
            templates: Arc::new(templates),
            instances: Arc::new(instances),
        })
    }

    /// Returns installed exact revisions in stable identity order.
    #[must_use]
    pub fn templates(&self) -> Vec<RunnerTemplateManifestV1> {
        self.templates.values().cloned().collect()
    }

    /// Checks one exact Activity Pack revision against an installed revision.
    #[must_use]
    pub fn compatibility(
        &self,
        template_id: &str,
        revision: &str,
        activity_pack_id: &str,
        activity_pack_revision: &str,
    ) -> CompatibilityV1 {
        let compatible = self
            .templates
            .get(&(template_id.to_owned(), revision.to_owned()))
            .and_then(|manifest| {
                manifest
                    .compatibility
                    .iter()
                    .find(|rule| rule.activity_pack_id == activity_pack_id)
            })
            .is_some_and(|rule| {
                rule.exact_revisions
                    .iter()
                    .any(|exact| exact == activity_pack_revision)
            });
        if compatible {
            CompatibilityV1::Compatible
        } else {
            CompatibilityV1::Incompatible
        }
    }

    fn template_for_instance(
        &self,
        instance_id: &str,
    ) -> Option<(&RunnerTemplateManifestV1, &RunnerInstanceInstallV1)> {
        let (template_id, revision, instance) = self.instances.get(instance_id)?;
        self.templates
            .get(&(template_id.clone(), revision.clone()))
            .map(|manifest| (manifest, instance))
    }
}

fn json_files(root: &Path) -> Result<Vec<PathBuf>, RunnerTemplateErrorV1> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(root)
        .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn read_manifest(
    path: &Path,
    verify_executable: bool,
) -> Result<RunnerTemplateManifestV1, RunnerTemplateErrorV1> {
    let metadata = fs::metadata(path).map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?;
    if metadata.len() == 0 || metadata.len() > MAX_MANIFEST_BYTES {
        return Err(RunnerTemplateErrorV1::InvalidManifest);
    }
    let mut manifest: RunnerTemplateManifestV1 = serde_json::from_slice(
        &fs::read(path).map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?,
    )
    .map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?;
    if verify_executable {
        let executable = if manifest.executable.path.is_absolute() {
            manifest.executable.path.clone()
        } else {
            path.parent()
                .ok_or(RunnerTemplateErrorV1::InvalidManifest)?
                .join(&manifest.executable.path)
        };
        manifest.executable.path = executable
            .canonicalize()
            .map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?;
    }
    validate_manifest(&manifest, verify_executable)?;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &RunnerTemplateManifestV1,
    verify_executable: bool,
) -> Result<(), RunnerTemplateErrorV1> {
    if manifest.schema != MANIFEST_SCHEMA_V1
        || !valid_id(&manifest.template_id)
        || !valid_revision(&manifest.revision)
        || !bounded(&manifest.display_name, MAX_TEXT_BYTES)
        || !manifest.executable.path.is_absolute()
        || !valid_digest(&manifest.executable.blake3)
        || manifest.compatibility.is_empty()
        || manifest.capacity.maximum_concurrent_invocations == 0
        || manifest.capacity.maximum_concurrent_invocations > 10_000
        || !valid_health(&manifest.health)
        || manifest.instances.is_empty()
        || manifest.instances.len() > 128
    {
        return Err(RunnerTemplateErrorV1::InvalidManifest);
    }
    if verify_executable
        && executable_digest(&manifest.executable.path)? != manifest.executable.blake3
    {
        return Err(RunnerTemplateErrorV1::InvalidManifest);
    }
    let mut pack_ids = BTreeSet::new();
    for rule in &manifest.compatibility {
        if !valid_activity_pack_id(&rule.activity_pack_id)
            || rule.exact_revisions.is_empty()
            || !pack_ids.insert(&rule.activity_pack_id)
            || rule
                .exact_revisions
                .iter()
                .any(|revision| !valid_revision(revision))
            || rule.exact_revisions.iter().collect::<BTreeSet<_>>().len()
                != rule.exact_revisions.len()
        {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
    }
    let mut environment_keys = BTreeSet::new();
    for (key, value) in &manifest.non_secret_environment {
        if !valid_environment_key(key)
            || sensitive_key(key)
            || !bounded(value, MAX_ENVIRONMENT_VALUE_BYTES)
            || !environment_keys.insert(key)
        {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
    }
    for setting in &manifest.secret_environment {
        if !valid_environment_key(&setting.key)
            || !valid_secret_reference(setting.reference.as_str())
            || !environment_keys.insert(&setting.key)
        {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
    }
    let mut instance_ids = BTreeSet::new();
    for instance in &manifest.instances {
        if !valid_id(&instance.instance_id)
            || !instance.health_address.ip().is_loopback()
            || instance.health_address.port() == 0
            || !instance_ids.insert(&instance.instance_id)
        {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
    }
    Ok(())
}

fn executable_digest(path: &Path) -> Result<String, RunnerTemplateErrorV1> {
    let metadata = fs::metadata(path).map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_EXECUTABLE_BYTES {
        return Err(RunnerTemplateErrorV1::InvalidManifest);
    }
    let mut hasher = blake3::Hasher::new();
    let mut file = fs::File::open(path).map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?;
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).map_err(|_| RunnerTemplateErrorV1::InvalidManifest)?)
            .ok_or(RunnerTemplateErrorV1::InvalidManifest)?;
        if total > MAX_EXECUTABLE_BYTES {
            return Err(RunnerTemplateErrorV1::InvalidManifest);
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

/// Activity Pack identities may be namespaced (for example
/// `worldstream.counter`) while Runner and instance identities remain local
/// identifiers.  A dotted value must have nonempty local-name segments.
fn valid_activity_pack_id(value: &str) -> bool {
    valid_id(value)
        || (value.len() <= 64
            && value.contains('.')
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_')
                    })
            }))
}

fn valid_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_environment_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn sensitive_key(value: &str) -> bool {
    ["SECRET", "TOKEN", "PASSWORD", "KEY", "CREDENTIAL"]
        .iter()
        .any(|marker| value.contains(marker))
}

fn valid_secret_reference(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_digest(value: &str) -> bool {
    valid_secret_reference(value)
}

fn valid_health(health: &RunnerHealthContractV1) -> bool {
    health.path.starts_with('/')
        && !health.path.contains(['\r', '\n', ' ', '?', '#'])
        && health.path.len() <= 128
        && (10..=10_000).contains(&health.timeout_ms)
        && health.stale_after_ms >= health.timeout_ms
        && health.stale_after_ms <= 300_000
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.contains(['\0', '\r', '\n'])
}

/// Browser-safe installed template catalog.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerTemplateCatalogV1 {
    /// Stable response schema.
    pub schema: String,
    /// Installed immutable revisions.
    pub templates: Vec<RunnerTemplateViewV1>,
}

/// Browser-safe installed template revision metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerTemplateViewV1 {
    /// Stable template identity.
    pub template_id: String,
    /// Immutable exact revision.
    pub revision: String,
    /// Operator-facing name.
    pub display_name: String,
    /// Approved executable digest, without its local path.
    pub executable_blake3: String,
    /// Exact compatibility rules.
    pub compatibility: Vec<RunnerCompatibilityRuleV1>,
    /// Declared capacity per instance.
    pub capacity: RunnerCapacityV1,
    /// Health freshness bound.
    pub health_stale_after_ms: u64,
    /// Explicit non-secret setting names, without values.
    pub non_secret_settings: Vec<String>,
    /// Secret setting kinds and availability, without retained references.
    pub secret_settings: Vec<RunnerSecretSettingViewV1>,
    /// Owner-installed instance identities.
    pub instances: Vec<String>,
}

/// Browser-safe secret requirement for a template.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerSecretSettingViewV1 {
    /// Fixed setting name.
    pub key: String,
    /// Credential authority boundary.
    pub kind: SecretKindV1,
    /// Whether the exact retained reference resolves safely.
    pub configured: bool,
}

/// Closed Runner instance lifecycle.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerInstanceStateV1 {
    /// Installed instance is not running.
    Stopped,
    /// Configured launch is in progress.
    Starting,
    /// Process is live or health-reconciled.
    Running,
    /// Graceful stop is in progress.
    Stopping,
    /// Lifecycle or reconciliation needs operator action.
    Failed,
    /// Required installed material is unavailable.
    Unavailable,
}

/// Closed health assessment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerInstanceHealthV1 {
    /// Health contract returned a valid live observation.
    Healthy,
    /// Process is stopped.
    Stopped,
    /// Health could not be established.
    Unavailable,
}

/// Closed freshness assessment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerFreshnessV1 {
    /// Last health observation is within the manifest contract.
    Fresh,
    /// Last successful observation is outside the manifest contract.
    Stale,
    /// No successful observation is available.
    Unavailable,
}

/// Bounded Runner failure suitable for Studio.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerInstanceFailureV1 {
    /// Stable failure code.
    pub code: String,
    /// Credential-free explanation.
    pub explanation: String,
    /// Safe next action.
    pub next_action: String,
}

/// Live capacity observation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerCapacityStatusV1 {
    /// Template-declared maximum.
    pub maximum: u32,
    /// Health-reported active Invocations.
    pub in_use: u32,
    /// Remaining capacity.
    pub available: u32,
}

/// Complete browser-safe Runner instance snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerInstanceStatusV1 {
    /// Stable installed instance identity.
    pub instance_id: String,
    /// Exact template identity.
    pub template_id: String,
    /// Immutable exact template revision.
    pub template_revision: String,
    /// Lifecycle state.
    pub state: RunnerInstanceStateV1,
    /// Identity of the most recent accepted operation.
    pub operation_id: u64,
    /// Whether this Supervisor owns the process handle.
    pub managed_by_supervisor: bool,
    /// Exact supported Activity Pack revisions.
    pub compatibility: Vec<RunnerCompatibilityRuleV1>,
    /// Declared and observed capacity.
    pub capacity: RunnerCapacityStatusV1,
    /// Current health assessment.
    pub health: RunnerInstanceHealthV1,
    /// Current freshness assessment.
    pub freshness: RunnerFreshnessV1,
    /// Last successful observation time, if any.
    pub observed_at_unix_ms: Option<u64>,
    /// Bounded actionable failure.
    pub failure: Option<RunnerInstanceFailureV1>,
}

/// Stable list response for Runner instance status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerInstanceStatusResponseV1 {
    /// Stable response schema.
    pub schema: String,
    /// Installed instances in stable identity order.
    pub instances: Vec<RunnerInstanceStatusV1>,
}

/// Successful health observation from an approved Runner.
#[derive(Clone, Copy, Debug)]
struct RunnerHealthObservationV1 {
    capacity_in_use: u32,
}

trait RunnerRuntimeBackend: Send + Sync + 'static {
    fn launch(
        &self,
        executable: &Path,
        environment: &[(String, String)],
    ) -> Result<Box<dyn ManagedRunner>, ()>;
    fn probe(
        &self,
        instance_id: &str,
        address: SocketAddr,
        path: &str,
        timeout: Duration,
    ) -> Result<RunnerHealthObservationV1, ()>;
}

trait ManagedRunner: Send + 'static {
    fn try_wait(&mut self) -> Result<Option<ProcessExit>, ()>;
    fn graceful_stop(&mut self, timeout: Duration) -> Result<bool, ()>;
}

#[derive(Clone, Copy)]
struct ProcessExit;

#[derive(Clone)]
pub struct RunnerSupervisorV1 {
    registry: RunnerTemplateRegistryV1,
    vault: FileSecretVaultV1,
    runtime_root: Arc<PathBuf>,
    graceful_timeout: Duration,
    backend: Arc<dyn RunnerRuntimeBackend>,
    instances: Arc<Mutex<BTreeMap<String, RunnerRuntime>>>,
}

struct RunnerRuntime {
    template_id: String,
    template_revision: String,
    install: RunnerInstanceInstallV1,
    state: RunnerInstanceStateV1,
    operation_id: u64,
    managed: bool,
    expected_running: bool,
    process: Option<Box<dyn ManagedRunner>>,
    health: RunnerInstanceHealthV1,
    capacity_in_use: u32,
    observed_at_unix_ms: Option<u64>,
    failure: Option<RunnerInstanceFailureV1>,
    task_runner_id: Option<String>,
    task_runner_authority: Option<SecretReferenceV1>,
}

#[derive(Deserialize, Serialize)]
struct RunnerRuntimeRecordV1 {
    schema: String,
    instance_id: String,
    operation_id: u64,
    expected_running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_runner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_runner_authority: Option<SecretReferenceV1>,
}

impl RunnerSupervisorV1 {
    /// Opens a persistent instance supervisor for installed template revisions.
    ///
    /// # Errors
    ///
    /// Returns a pathless registry error when owner-only runtime state cannot
    /// be opened or validated.
    pub fn open(
        registry: RunnerTemplateRegistryV1,
        runtime_root: &Path,
        vault: FileSecretVaultV1,
        graceful_timeout: Duration,
    ) -> Result<Self, RunnerTemplateErrorV1> {
        Self::with_backend(
            registry,
            runtime_root,
            vault,
            graceful_timeout,
            OsRunnerRuntimeBackend,
        )
    }

    fn with_backend(
        registry: RunnerTemplateRegistryV1,
        runtime_root: &Path,
        vault: FileSecretVaultV1,
        graceful_timeout: Duration,
        backend: impl RunnerRuntimeBackend,
    ) -> Result<Self, RunnerTemplateErrorV1> {
        let runtime_root = prepare_data_directory(runtime_root)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let mut instances = BTreeMap::new();
        for (instance_id, (template_id, revision, install)) in registry.instances.iter() {
            let record = read_runtime_record(&runtime_root, instance_id)?;
            let expected_running = record
                .as_ref()
                .is_some_and(|record| record.expected_running);
            let operation_id = record.as_ref().map_or(0, |record| record.operation_id);
            let task_runner_id = record
                .as_ref()
                .and_then(|record| record.task_runner_id.clone());
            let task_runner_authority = record
                .as_ref()
                .and_then(|record| record.task_runner_authority.clone());
            instances.insert(
                instance_id.clone(),
                RunnerRuntime {
                    template_id: template_id.clone(),
                    template_revision: revision.clone(),
                    install: install.clone(),
                    state: if expected_running {
                        RunnerInstanceStateV1::Failed
                    } else {
                        RunnerInstanceStateV1::Stopped
                    },
                    operation_id,
                    managed: false,
                    expected_running,
                    process: None,
                    health: if expected_running {
                        RunnerInstanceHealthV1::Unavailable
                    } else {
                        RunnerInstanceHealthV1::Stopped
                    },
                    capacity_in_use: 0,
                    observed_at_unix_ms: None,
                    failure: expected_running.then(reconciliation_failed),
                    task_runner_id,
                    task_runner_authority,
                },
            );
        }
        Ok(Self {
            registry,
            vault,
            runtime_root: Arc::new(runtime_root),
            graceful_timeout,
            backend: Arc::new(backend),
            instances: Arc::new(Mutex::new(instances)),
        })
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, RunnerRuntime>> {
        self.instances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns the browser-safe immutable template catalog.
    #[must_use]
    pub fn catalog(&self) -> RunnerTemplateCatalogV1 {
        RunnerTemplateCatalogV1 {
            schema: CATALOG_SCHEMA_V1.to_owned(),
            templates: self
                .registry
                .templates
                .values()
                .map(|manifest| RunnerTemplateViewV1 {
                    template_id: manifest.template_id.clone(),
                    revision: manifest.revision.clone(),
                    display_name: manifest.display_name.clone(),
                    executable_blake3: manifest.executable.blake3.clone(),
                    compatibility: manifest.compatibility.clone(),
                    capacity: manifest.capacity,
                    health_stale_after_ms: manifest.health.stale_after_ms,
                    non_secret_settings: manifest.non_secret_environment.keys().cloned().collect(),
                    secret_settings: manifest
                        .secret_environment
                        .iter()
                        .map(|setting| RunnerSecretSettingViewV1 {
                            key: setting.key.clone(),
                            kind: setting.kind,
                            configured: self
                                .vault
                                .resolve(setting.kind, &setting.reference)
                                .is_ok(),
                        })
                        .collect(),
                    instances: manifest
                        .instances
                        .iter()
                        .map(|instance| instance.instance_id.clone())
                        .collect(),
                })
                .collect(),
        }
    }

    /// Reconciles and returns every installed instance.
    #[must_use]
    pub fn statuses(&self) -> RunnerInstanceStatusResponseV1 {
        let ids = self.lock().keys().cloned().collect::<Vec<_>>();
        for instance_id in ids {
            self.reconcile(&instance_id);
        }
        self.status_response()
    }

    /// Idempotently starts one owner-installed instance.
    #[must_use]
    pub fn start(&self, instance_id: &str) -> Option<RunnerInstanceStatusResponseV1> {
        self.reconcile(instance_id);
        let (manifest, _) = self.registry.template_for_instance(instance_id)?;
        {
            let instances = self.lock();
            let runtime = instances.get(instance_id)?;
            if matches!(
                runtime.state,
                RunnerInstanceStateV1::Starting
                    | RunnerInstanceStateV1::Running
                    | RunnerInstanceStateV1::Stopping
                    | RunnerInstanceStateV1::Unavailable
            ) || runtime.process.is_some()
                || (runtime.state == RunnerInstanceStateV1::Failed && runtime.expected_running)
            {
                return Some(self.status_response_from(&instances));
            }
        }

        let mut environment = match self.launch_environment(manifest, instance_id) {
            Ok(environment) => environment,
            Err(failure) => {
                let mut instances = self.lock();
                let runtime = instances.get_mut(instance_id)?;
                runtime.state = RunnerInstanceStateV1::Failed;
                runtime.expected_running = false;
                runtime.failure = Some(failure);
                let _ = self.persist(runtime);
                return Some(self.status_response_from(&instances));
            }
        };
        let executable_matches = executable_digest(&manifest.executable.path)
            .is_ok_and(|digest| digest == manifest.executable.blake3);
        if !executable_matches {
            let mut instances = self.lock();
            let runtime = instances.get_mut(instance_id)?;
            runtime.state = RunnerInstanceStateV1::Unavailable;
            runtime.expected_running = false;
            runtime.failure = Some(executable_unavailable());
            let _ = self.persist(runtime);
            return Some(self.status_response_from(&instances));
        }

        {
            let mut instances = self.lock();
            let runtime = instances.get_mut(instance_id)?;
            runtime.operation_id = runtime.operation_id.saturating_add(1);
            runtime.state = RunnerInstanceStateV1::Starting;
            runtime.expected_running = true;
            runtime.managed = true;
            runtime.failure = None;
            if self.persist(runtime).is_err() {
                runtime.state = RunnerInstanceStateV1::Failed;
                runtime.expected_running = false;
                runtime.managed = false;
                runtime.failure = Some(state_failed());
                return Some(self.status_response_from(&instances));
            }
        }

        let launched = self.backend.launch(&manifest.executable.path, &environment);
        for (_, value) in &mut environment {
            value.zeroize();
        }
        let mut instances = self.lock();
        let runtime = instances.get_mut(instance_id)?;
        if let Ok(process) = launched {
            runtime.process = Some(process);
            runtime.state = RunnerInstanceStateV1::Running;
            runtime.health = RunnerInstanceHealthV1::Unavailable;
            runtime.failure = None;
        } else {
            runtime.state = RunnerInstanceStateV1::Failed;
            runtime.expected_running = false;
            runtime.managed = false;
            runtime.failure = Some(start_failed());
            let _ = self.persist(runtime);
        }
        Some(self.status_response_from(&instances))
    }

    /// Gracefully stops one managed installed instance.
    #[must_use]
    pub fn stop(&self, instance_id: &str) -> Option<RunnerInstanceStatusResponseV1> {
        self.reconcile(instance_id);
        let mut instances = self.lock();
        let runtime = instances.get_mut(instance_id)?;
        if runtime.state == RunnerInstanceStateV1::Stopped {
            return Some(self.status_response_from(&instances));
        }
        if runtime.state == RunnerInstanceStateV1::Running && !runtime.managed {
            runtime.state = RunnerInstanceStateV1::Failed;
            runtime.failure = Some(not_managed());
            return Some(self.status_response_from(&instances));
        }
        let Some(mut process) = runtime.process.take() else {
            return Some(self.status_response_from(&instances));
        };
        runtime.operation_id = runtime.operation_id.saturating_add(1);
        runtime.state = RunnerInstanceStateV1::Stopping;
        runtime.expected_running = false;
        let _ = self.persist(runtime);
        match process.graceful_stop(self.graceful_timeout) {
            Ok(true) => {
                runtime.state = RunnerInstanceStateV1::Stopped;
                runtime.managed = false;
                runtime.health = RunnerInstanceHealthV1::Stopped;
                runtime.capacity_in_use = 0;
                runtime.failure = None;
            }
            Ok(false) => {
                runtime.process = Some(process);
                runtime.state = RunnerInstanceStateV1::Failed;
                runtime.managed = true;
                runtime.failure = Some(stop_timeout());
            }
            Err(()) => {
                runtime.process = Some(process);
                runtime.state = RunnerInstanceStateV1::Failed;
                runtime.managed = true;
                runtime.failure = Some(stop_failed());
            }
        }
        Some(self.status_response_from(&instances))
    }

    /// Gracefully restarts one managed installed instance.
    #[must_use]
    pub fn restart(&self, instance_id: &str) -> Option<RunnerInstanceStatusResponseV1> {
        let stopped = self.stop(instance_id)?;
        let instance = stopped
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)?;
        if instance.state != RunnerInstanceStateV1::Stopped {
            return Some(stopped);
        }
        self.start(instance_id)
    }

    /// Returns whether an instance can accept its first immutable Task Runner binding.
    #[must_use]
    pub fn task_runner_binding_available(&self, instance_id: &str) -> bool {
        self.lock().get(instance_id).is_some_and(|runtime| {
            runtime.task_runner_id.is_none() && runtime.task_runner_authority.is_none()
        })
    }

    /// Resolves one exact approved reference-host launch target without
    /// requiring it to be running or binding Runner authority into its process.
    #[must_use]
    pub(crate) fn managed_reference_launch_target(
        &self,
        template_id: &str,
        template_revision: &str,
        pack: &PackReference,
    ) -> Option<RunnerInstanceStatusV1> {
        self.statuses().instances.into_iter().find(|instance| {
            instance.template_id == template_id
                && instance.template_revision == template_revision
                && matches!(
                    instance.state,
                    RunnerInstanceStateV1::Stopped | RunnerInstanceStateV1::Running
                )
                && instance.compatibility.iter().any(|rule| {
                    rule.activity_pack_id == pack.id && rule.exact_revisions.contains(&pack.version)
                })
                && self.task_runner_binding_available(&instance.instance_id)
        })
    }

    pub(crate) fn managed_reference_executable(
        &self,
        instance_id: &str,
        template_id: &str,
        template_revision: &str,
    ) -> Option<PathBuf> {
        let (manifest, _) = self.registry.template_for_instance(instance_id)?;
        if manifest.template_id != template_id
            || manifest.revision != template_revision
            || executable_digest(&manifest.executable.path).ok()? != manifest.executable.blake3
        {
            return None;
        }
        Some(manifest.executable.path.clone())
    }

    /// Durably binds and delivers one exact provisioned Runner identity and authority.
    ///
    /// An already-bound instance accepts only the identical identity/reference pair.
    /// A new binding restarts the owner-managed process so it receives the exact
    /// identity and credential through its private launch environment.
    ///
    /// # Errors
    ///
    /// Returns a closed not-found, immutable-conflict, or local-unavailable result.
    pub fn bind_task_runner_authority(
        &self,
        instance_id: &str,
        runner_id: &str,
        authority: &SecretReferenceV1,
    ) -> Result<(), ManagedRunnerBindingErrorV1> {
        if runner_id.parse::<UlidString>().is_err()
            || !self
                .vault
                .resolve(SecretKindV1::RunnerAuthority, authority)
                .is_ok_and(|secret| secret.as_bytes().len() == 32)
        {
            return Err(ManagedRunnerBindingErrorV1::Unavailable);
        }
        {
            let mut instances = self.lock();
            let runtime = instances
                .get_mut(instance_id)
                .ok_or(ManagedRunnerBindingErrorV1::NotFound)?;
            match (&runtime.task_runner_id, &runtime.task_runner_authority) {
                (Some(existing_id), Some(existing_authority))
                    if existing_id == runner_id && existing_authority == authority =>
                {
                    return Ok(());
                }
                (Some(_), _) | (_, Some(_)) => {
                    return Err(ManagedRunnerBindingErrorV1::Conflict);
                }
                (None, None) => {}
            }
            if runtime.state != RunnerInstanceStateV1::Running
                || !runtime.managed
                || runtime.health != RunnerInstanceHealthV1::Healthy
            {
                return Err(ManagedRunnerBindingErrorV1::Unavailable);
            }
            runtime.task_runner_id = Some(runner_id.to_owned());
            runtime.task_runner_authority = Some(authority.clone());
            if self.persist(runtime).is_err() {
                runtime.task_runner_id = None;
                runtime.task_runner_authority = None;
                return Err(ManagedRunnerBindingErrorV1::Unavailable);
            }
        }
        let status = self
            .restart(instance_id)
            .ok_or(ManagedRunnerBindingErrorV1::Unavailable)?;
        let instance = status
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .ok_or(ManagedRunnerBindingErrorV1::Unavailable)?;
        if instance.state == RunnerInstanceStateV1::Running {
            Ok(())
        } else {
            Err(ManagedRunnerBindingErrorV1::Unavailable)
        }
    }

    fn launch_environment(
        &self,
        manifest: &RunnerTemplateManifestV1,
        instance_id: &str,
    ) -> Result<Vec<(String, String)>, RunnerInstanceFailureV1> {
        let mut environment = manifest
            .non_secret_environment
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        environment.push((
            "WORLDSTREAM_RUNNER_INSTANCE_ID".to_owned(),
            instance_id.to_owned(),
        ));
        let task_binding = self.lock().get(instance_id).and_then(|runtime| {
            runtime
                .task_runner_id
                .clone()
                .zip(runtime.task_runner_authority.clone())
        });
        if let Some((runner_id, authority)) = task_binding {
            let resolved = self
                .vault
                .resolve(SecretKindV1::RunnerAuthority, &authority)
                .map_err(|_| missing_secret())?;
            let bytes: [u8; 32] = resolved
                .as_bytes()
                .try_into()
                .map_err(|_| unusable_secret())?;
            let bearer = BearerWireV1::from_bytes(bytes).to_wire();
            environment.push(("WORLDSTREAM_RUNNER_ID".to_owned(), runner_id));
            environment.push(("WORLDSTREAM_RUNNER_BEARER".to_owned(), bearer));
        }
        for setting in &manifest.secret_environment {
            let resolved = self
                .vault
                .resolve(setting.kind, &setting.reference)
                .map_err(|_| missing_secret())?;
            let value = std::str::from_utf8(resolved.as_bytes())
                .map_err(|_| unusable_secret())?
                .to_owned();
            environment.push((setting.key.clone(), value));
        }
        Ok(environment)
    }

    fn reconcile(&self, instance_id: &str) {
        let Some((manifest, install)) = self.registry.template_for_instance(instance_id) else {
            return;
        };
        {
            let mut instances = self.lock();
            let Some(runtime) = instances.get_mut(instance_id) else {
                return;
            };
            if let Some(process) = runtime.process.as_mut() {
                match process.try_wait() {
                    Ok(Some(_)) => {
                        runtime.process = None;
                        runtime.state = RunnerInstanceStateV1::Failed;
                        runtime.managed = false;
                        runtime.expected_running = false;
                        runtime.health = RunnerInstanceHealthV1::Unavailable;
                        runtime.failure = Some(unexpected_exit());
                        let _ = self.persist(runtime);
                        return;
                    }
                    Ok(None) => {}
                    Err(()) => {
                        runtime.state = RunnerInstanceStateV1::Failed;
                        runtime.failure = Some(process_state_failed());
                        return;
                    }
                }
            }
        }

        let observation = self.backend.probe(
            instance_id,
            install.health_address,
            &manifest.health.path,
            Duration::from_millis(manifest.health.timeout_ms),
        );
        let mut instances = self.lock();
        let Some(runtime) = instances.get_mut(instance_id) else {
            return;
        };
        match observation {
            Ok(observation)
                if observation.capacity_in_use
                    <= manifest.capacity.maximum_concurrent_invocations =>
            {
                runtime.state = RunnerInstanceStateV1::Running;
                runtime.health = RunnerInstanceHealthV1::Healthy;
                runtime.capacity_in_use = observation.capacity_in_use;
                runtime.observed_at_unix_ms = Some(now_unix_ms());
                if runtime.process.is_none() {
                    runtime.managed = false;
                }
                runtime.expected_running = true;
                runtime.failure = None;
                let _ = self.persist(runtime);
            }
            _ if runtime.process.is_some() => {
                runtime.state = RunnerInstanceStateV1::Running;
                runtime.health = RunnerInstanceHealthV1::Unavailable;
                runtime.failure = Some(health_check_failed());
            }
            _ if runtime.expected_running => {
                runtime.state = RunnerInstanceStateV1::Failed;
                runtime.health = RunnerInstanceHealthV1::Unavailable;
                runtime.failure = Some(reconciliation_failed());
            }
            _ => {
                runtime.state = RunnerInstanceStateV1::Stopped;
                runtime.health = RunnerInstanceHealthV1::Stopped;
                runtime.capacity_in_use = 0;
                runtime.failure = None;
            }
        }
    }

    fn status_response(&self) -> RunnerInstanceStatusResponseV1 {
        self.status_response_from(&self.lock())
    }

    fn status_response_from(
        &self,
        instances: &BTreeMap<String, RunnerRuntime>,
    ) -> RunnerInstanceStatusResponseV1 {
        RunnerInstanceStatusResponseV1 {
            schema: STATUS_SCHEMA_V1.to_owned(),
            instances: instances
                .values()
                .filter_map(|runtime| self.status(runtime))
                .collect(),
        }
    }

    fn status(&self, runtime: &RunnerRuntime) -> Option<RunnerInstanceStatusV1> {
        let manifest = self.registry.templates.get(&(
            runtime.template_id.clone(),
            runtime.template_revision.clone(),
        ))?;
        let maximum = manifest.capacity.maximum_concurrent_invocations;
        let freshness = match runtime.observed_at_unix_ms {
            Some(observed)
                if now_unix_ms().saturating_sub(observed) <= manifest.health.stale_after_ms =>
            {
                RunnerFreshnessV1::Fresh
            }
            Some(_) => RunnerFreshnessV1::Stale,
            None => RunnerFreshnessV1::Unavailable,
        };
        Some(RunnerInstanceStatusV1 {
            instance_id: runtime.install.instance_id.clone(),
            template_id: runtime.template_id.clone(),
            template_revision: runtime.template_revision.clone(),
            state: runtime.state,
            operation_id: runtime.operation_id,
            managed_by_supervisor: runtime.managed,
            compatibility: manifest.compatibility.clone(),
            capacity: RunnerCapacityStatusV1 {
                maximum,
                in_use: runtime.capacity_in_use.min(maximum),
                available: maximum.saturating_sub(runtime.capacity_in_use),
            },
            health: runtime.health,
            freshness,
            observed_at_unix_ms: runtime.observed_at_unix_ms,
            failure: runtime.failure.clone(),
        })
    }

    fn persist(&self, runtime: &RunnerRuntime) -> Result<(), RunnerTemplateErrorV1> {
        let record = RunnerRuntimeRecordV1 {
            schema: "worldstream/runner-instance-runtime/v1".to_owned(),
            instance_id: runtime.install.instance_id.clone(),
            operation_id: runtime.operation_id,
            expected_running: runtime.expected_running,
            task_runner_id: runtime.task_runner_id.clone(),
            task_runner_authority: runtime.task_runner_authority.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&record)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        let target = runtime_record_path(&self.runtime_root, &runtime.install.instance_id);
        let temporary = self.runtime_root.join(format!(
            ".{}-{}.tmp",
            runtime.install.instance_id, runtime.operation_id
        ));
        if temporary.exists() {
            fs::remove_file(&temporary).map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        }
        let mut file = create_owner_only_file(&temporary)
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        file.write_all(&encoded)
            .and_then(|()| file.sync_all())
            .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
        fs::rename(&temporary, target).map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)
    }
}

fn read_runtime_record(
    root: &Path,
    instance_id: &str,
) -> Result<Option<RunnerRuntimeRecordV1>, RunnerTemplateErrorV1> {
    let path = runtime_record_path(root, instance_id);
    if !path.exists() {
        return Ok(None);
    }
    let record: RunnerRuntimeRecordV1 = serde_json::from_slice(
        &fs::read(path).map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?,
    )
    .map_err(|_| RunnerTemplateErrorV1::RegistryUnavailable)?;
    if record.schema != "worldstream/runner-instance-runtime/v1"
        || record.instance_id != instance_id
        || record.task_runner_id.is_some() != record.task_runner_authority.is_some()
        || record
            .task_runner_id
            .as_ref()
            .is_some_and(|runner_id| runner_id.parse::<UlidString>().is_err())
    {
        return Err(RunnerTemplateErrorV1::RegistryUnavailable);
    }
    Ok(Some(record))
}

fn runtime_record_path(root: &Path, instance_id: &str) -> PathBuf {
    root.join(format!("{instance_id}.json"))
}

/// Builds browser-safe catalog, status, and typed instance lifecycle routes.
pub fn runner_router(supervisor: RunnerSupervisorV1) -> Router {
    Router::new()
        .route("/api/v1/runner-templates", get(runner_templates))
        .route("/api/v1/runner-instances", get(runner_instances))
        .route(
            "/api/v1/runner-instances/{instance_id}/start",
            post(start_runner),
        )
        .route(
            "/api/v1/runner-instances/{instance_id}/stop",
            post(stop_runner),
        )
        .route(
            "/api/v1/runner-instances/{instance_id}/restart",
            post(restart_runner),
        )
        .with_state(supervisor)
}

async fn runner_templates(
    State(supervisor): State<RunnerSupervisorV1>,
) -> Json<RunnerTemplateCatalogV1> {
    Json(supervisor.catalog())
}

async fn runner_instances(
    State(supervisor): State<RunnerSupervisorV1>,
) -> Json<RunnerInstanceStatusResponseV1> {
    runner_operation(supervisor, None, RunnerOperation::Status)
        .await
        .unwrap_or_else(|_| unreachable!("status has no instance lookup"))
}

async fn start_runner(
    State(supervisor): State<RunnerSupervisorV1>,
    AxumPath(instance_id): AxumPath<String>,
) -> Result<Json<RunnerInstanceStatusResponseV1>, StatusCode> {
    runner_operation(supervisor, Some(instance_id), RunnerOperation::Start).await
}

async fn stop_runner(
    State(supervisor): State<RunnerSupervisorV1>,
    AxumPath(instance_id): AxumPath<String>,
) -> Result<Json<RunnerInstanceStatusResponseV1>, StatusCode> {
    runner_operation(supervisor, Some(instance_id), RunnerOperation::Stop).await
}

async fn restart_runner(
    State(supervisor): State<RunnerSupervisorV1>,
    AxumPath(instance_id): AxumPath<String>,
) -> Result<Json<RunnerInstanceStatusResponseV1>, StatusCode> {
    runner_operation(supervisor, Some(instance_id), RunnerOperation::Restart).await
}

#[derive(Clone, Copy)]
enum RunnerOperation {
    Status,
    Start,
    Stop,
    Restart,
}

async fn runner_operation(
    supervisor: RunnerSupervisorV1,
    instance_id: Option<String>,
    operation: RunnerOperation,
) -> Result<Json<RunnerInstanceStatusResponseV1>, StatusCode> {
    tokio::task::spawn_blocking(move || match operation {
        RunnerOperation::Status => Some(supervisor.statuses()),
        RunnerOperation::Start => supervisor.start(instance_id.as_deref().unwrap_or_default()),
        RunnerOperation::Stop => supervisor.stop(instance_id.as_deref().unwrap_or_default()),
        RunnerOperation::Restart => supervisor.restart(instance_id.as_deref().unwrap_or_default()),
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .map(Json)
    .ok_or(StatusCode::NOT_FOUND)
}

struct OsRunnerRuntimeBackend;

impl RunnerRuntimeBackend for OsRunnerRuntimeBackend {
    fn launch(
        &self,
        executable: &Path,
        environment: &[(String, String)],
    ) -> Result<Box<dyn ManagedRunner>, ()> {
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        for (key, value) in environment {
            command.env(key, value);
        }
        command
            .spawn()
            .map(|child| Box::new(OsManagedRunner { child }) as Box<dyn ManagedRunner>)
            .map_err(|_| ())
    }

    fn probe(
        &self,
        instance_id: &str,
        address: SocketAddr,
        path: &str,
        timeout: Duration,
    ) -> Result<RunnerHealthObservationV1, ()> {
        let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|_| ())?;
        stream.set_read_timeout(Some(timeout)).map_err(|_| ())?;
        stream.set_write_timeout(Some(timeout)).map_err(|_| ())?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        )
        .map_err(|_| ())?;
        let mut bytes = Vec::new();
        stream
            .take(MAX_HTTP_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ())?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_HTTP_RESPONSE_BYTES {
            return Err(());
        }
        let separator = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or(())?;
        let headers = std::str::from_utf8(&bytes[..separator]).map_err(|_| ())?;
        if headers
            .lines()
            .next()
            .and_then(|line| line.split_ascii_whitespace().nth(1))
            != Some("200")
        {
            return Err(());
        }
        let health: RunnerHealthWireV1 =
            serde_json::from_slice(&bytes[(separator + 4)..]).map_err(|_| ())?;
        if health.status != "ok" || health.instance_id != instance_id {
            return Err(());
        }
        Ok(RunnerHealthObservationV1 {
            capacity_in_use: health.capacity_in_use,
        })
    }
}

#[derive(Deserialize)]
struct RunnerHealthWireV1 {
    status: String,
    instance_id: String,
    capacity_in_use: u32,
}

struct OsManagedRunner {
    child: Child,
}

impl ManagedRunner for OsManagedRunner {
    fn try_wait(&mut self) -> Result<Option<ProcessExit>, ()> {
        self.child
            .try_wait()
            .map(|status| status.map(|_| ProcessExit))
            .map_err(|_| ())
    }

    fn graceful_stop(&mut self, timeout: Duration) -> Result<bool, ()> {
        request_runner_stop(&mut self.child)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.try_wait()?.is_some() {
                return Ok(true);
            }
            if std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(unix)]
fn request_runner_stop(child: &mut Child) -> Result<(), ()> {
    let raw_pid = i32::try_from(child.id()).map_err(|_| ())?;
    let pid = rustix::process::Pid::from_raw(raw_pid).ok_or(())?;
    rustix::process::kill_process(pid, rustix::process::Signal::INT).map_err(|_| ())
}

#[cfg(windows)]
fn request_runner_stop(child: &mut Child) -> Result<(), ()> {
    Command::new("taskkill.exe")
        .arg("/PID")
        .arg(child.id().to_string())
        .status()
        .map_err(|_| ())?
        .success()
        .then_some(())
        .ok_or(())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn failure(code: &str, explanation: &str, next_action: &str) -> RunnerInstanceFailureV1 {
    RunnerInstanceFailureV1 {
        code: code.to_owned(),
        explanation: explanation.to_owned(),
        next_action: next_action.to_owned(),
    }
}

fn start_failed() -> RunnerInstanceFailureV1 {
    failure(
        "start_failed",
        "The approved Runner instance could not be started.",
        "Check the installed executable and configuration, then retry.",
    )
}

fn executable_unavailable() -> RunnerInstanceFailureV1 {
    failure(
        "executable_unavailable",
        "The approved Runner executable identity is unavailable or changed.",
        "Restore the exact owner-installed executable before retrying.",
    )
}

fn missing_secret() -> RunnerInstanceFailureV1 {
    failure(
        "secret_unavailable",
        "A required protected Runner setting is unavailable.",
        "Configure the required credential in Supervisor Settings, then retry.",
    )
}

fn unusable_secret() -> RunnerInstanceFailureV1 {
    failure(
        "secret_unusable",
        "A required protected Runner setting cannot be supplied safely.",
        "Replace the credential with a valid text value, then retry.",
    )
}

fn health_check_failed() -> RunnerInstanceFailureV1 {
    failure(
        "health_check_failed",
        "The Runner process is live but its bounded health contract failed.",
        "Inspect Runner diagnostics and retry after health is restored.",
    )
}

fn reconciliation_failed() -> RunnerInstanceFailureV1 {
    failure(
        "reconciliation_failed",
        "The previous Runner instance may still exist, but health cannot confirm it.",
        "Resolve the original process before starting another instance.",
    )
}

fn not_managed() -> RunnerInstanceFailureV1 {
    failure(
        "not_managed",
        "The live Runner was reconciled without an owned process handle.",
        "Stop it from its original process owner before starting it here.",
    )
}

fn stop_timeout() -> RunnerInstanceFailureV1 {
    failure(
        "stop_timeout",
        "The Runner did not exit before the graceful stop timeout.",
        "Allow current Invocations to finish, then retry the stop.",
    )
}

fn stop_failed() -> RunnerInstanceFailureV1 {
    failure(
        "stop_failed",
        "The graceful Runner stop could not be completed.",
        "Inspect the Runner process, then retry the stop.",
    )
}

fn unexpected_exit() -> RunnerInstanceFailureV1 {
    failure(
        "unexpected_exit",
        "The managed Runner exited outside a requested stop.",
        "Inspect Runner diagnostics, then retry the start.",
    )
}

fn process_state_failed() -> RunnerInstanceFailureV1 {
    failure(
        "process_state_failed",
        "The Supervisor could not determine the managed Runner state.",
        "Restart Studio Supervisor before retrying Runner control.",
    )
}

fn state_failed() -> RunnerInstanceFailureV1 {
    failure(
        "state_unavailable",
        "The Supervisor could not persist Runner reconciliation state.",
        "Restore the owner-only Supervisor state directory, then retry.",
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tempfile::TempDir;
    use tower::ServiceExt as _;
    use worldstream_protocol::{BearerWireV1, PackReference};

    use super::{
        CompatibilityV1, ManagedRunner, ManagedRunnerBindingErrorV1, ProcessExit,
        RunnerHealthObservationV1, RunnerInstanceHealthV1, RunnerInstanceStateV1,
        RunnerInstanceStatusResponseV1, RunnerRuntimeBackend, RunnerSupervisorV1,
        RunnerTemplateErrorV1, RunnerTemplateRegistryV1, runner_router,
    };
    use crate::secrets::{FileSecretVaultV1, SecretKindV1};

    #[test]
    fn exact_runner_template_revision_is_immutable_after_owner_install() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.0", "1.1"]);
        let registry = fixture.open_registry();
        assert_eq!(registry.templates().len(), 1);
        assert_eq!(registry.templates()[0].revision, "r2");

        fixture.write_manifest("Changed in place", "r2", &["1.0", "1.1"]);
        assert!(matches!(
            RunnerTemplateRegistryV1::open(&fixture.registry, &fixture.manifests),
            Err(RunnerTemplateErrorV1::ImmutableRevisionConflict)
        ));
    }

    #[test]
    fn invalid_manifest_and_incompatible_activity_assignment_fail_closed() {
        let fixture = Fixture::new();
        fs::write(
            fixture.manifests.join("invalid.json"),
            br#"{"schema":"wrong"}"#,
        )
        .unwrap_or_else(|error| unreachable!("write invalid manifest: {error}"));
        assert!(matches!(
            RunnerTemplateRegistryV1::open(&fixture.registry, &fixture.manifests),
            Err(RunnerTemplateErrorV1::InvalidManifest)
        ));

        fs::remove_file(fixture.manifests.join("invalid.json"))
            .unwrap_or_else(|error| unreachable!("remove invalid manifest: {error}"));
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let registry = fixture.open_registry();
        assert_eq!(
            registry.compatibility("local-mcp-helper", "r2", "agent-heist", "1.0"),
            CompatibilityV1::Incompatible
        );
        assert_eq!(
            registry.compatibility("local-mcp-helper", "r2", "agent-heist", "1.1"),
            CompatibilityV1::Compatible
        );

        let namespaced = Fixture::new();
        namespaced.write_manifest_for_pack(
            "Local MCP Helper",
            "r3",
            "worldstream.counter",
            &["3.0.0"],
        );
        let registry = namespaced.open_registry();
        assert_eq!(
            registry.compatibility("local-mcp-helper", "r3", "worldstream.counter", "3.0.0"),
            CompatibilityV1::Compatible
        );
        assert_eq!(
            registry.compatibility("local-mcp-helper", "r3", "worldstream.counter", "3.0.1"),
            CompatibilityV1::Incompatible
        );
    }

    #[test]
    fn exact_dormant_reference_host_is_selectable_without_runner_bearer_binding() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let supervisor = fixture.supervisor(FakeBackend::healthy());
        let selected = supervisor
            .managed_reference_launch_target(
                "local-mcp-helper",
                "r2",
                &PackReference {
                    id: "agent-heist".to_owned(),
                    version: "1.1".to_owned(),
                    digest:
                        "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_owned(),
                },
            )
            .unwrap_or_else(|| unreachable!("exact stopped reference host"));

        assert_eq!(selected.instance_id, "local-mcp-01");
        assert_eq!(selected.state, RunnerInstanceStateV1::Stopped);
        assert!(supervisor.task_runner_binding_available(&selected.instance_id));
    }

    #[tokio::test]
    async fn duplicate_start_uses_only_the_owner_installed_executable() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let backend = FakeBackend::healthy();
        let router = fixture.router(backend.clone());

        let first = post(&router, "/api/v1/runner-instances/local-mcp-01/start", "").await;
        let second = post(
            &router,
            "/api/v1/runner-instances/local-mcp-01/start",
            r#"{"command":"/bin/sh","path":"/private/hostile"}"#,
        )
        .await;
        assert_eq!(first.instances[0].state, RunnerInstanceStateV1::Running);
        assert_eq!(
            second.instances[0].operation_id,
            first.instances[0].operation_id
        );
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
        let executable = fixture
            .executable
            .canonicalize()
            .unwrap_or_else(|error| unreachable!("canonical executable: {error}"));
        assert_eq!(
            backend
                .last_executable
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref(),
            Some(executable.as_path())
        );
    }

    #[tokio::test]
    async fn failed_health_check_is_actionable_and_does_not_launch_a_duplicate() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let backend = FakeBackend::unhealthy();
        let router = fixture.router(backend.clone());
        post(&router, "/api/v1/runner-instances/local-mcp-01/start", "").await;

        let status = get_instances(&router).await;
        let instance = &status.instances[0];
        assert_eq!(instance.state, RunnerInstanceStateV1::Running);
        assert_eq!(instance.health, RunnerInstanceHealthV1::Unavailable);
        assert_eq!(
            instance
                .failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("health_check_failed")
        );
        post(&router, "/api/v1/runner-instances/local-mcp-01/start", "").await;
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exact_task_runner_binding_is_durable_idempotent_and_delivered_only_to_managed_process() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let backend = FakeBackend::healthy();
        let supervisor = fixture.supervisor(backend.clone());
        let _ = supervisor.start("local-mcp-01");
        let healthy = supervisor.statuses();
        assert_eq!(healthy.instances[0].health, RunnerInstanceHealthV1::Healthy);
        let authority = fixture
            .vault
            .store(SecretKindV1::RunnerAuthority, &[7_u8; 32])
            .unwrap_or_else(|error| unreachable!("store runner authority: {error:?}"));
        let runner_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

        supervisor
            .bind_task_runner_authority("local-mcp-01", runner_id, &authority)
            .unwrap_or_else(|error| unreachable!("bind runner authority: {error:?}"));
        assert_eq!(backend.launches.load(Ordering::SeqCst), 2);
        let environment = backend
            .last_environment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            environment
                .iter()
                .any(|(key, value)| { key == "WORLDSTREAM_RUNNER_ID" && value == runner_id })
        );
        let expected_bearer = BearerWireV1::from_bytes([7_u8; 32]).to_wire();
        assert!(environment.iter().any(|(key, value)| {
            key == "WORLDSTREAM_RUNNER_BEARER" && value == &expected_bearer
        }));
        drop(environment);

        supervisor
            .bind_task_runner_authority("local-mcp-01", runner_id, &authority)
            .unwrap_or_else(|error| unreachable!("repeat exact binding: {error:?}"));
        assert_eq!(backend.launches.load(Ordering::SeqCst), 2);
        assert_eq!(
            supervisor.bind_task_runner_authority(
                "local-mcp-01",
                "01ARZ3NDEKTSV4RRFFQ69G5FAW",
                &authority,
            ),
            Err(ManagedRunnerBindingErrorV1::Conflict)
        );
        drop(supervisor);

        let reopened = fixture.supervisor(backend);
        assert!(!reopened.task_runner_binding_available("local-mcp-01"));
    }

    #[tokio::test]
    async fn supervisor_restart_reconciles_running_and_stopped_instances_without_launching() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let backend = FakeBackend::healthy();
        let first = fixture.supervisor(backend.clone());
        let _ = first.start("local-mcp-01");
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
        drop(first);

        let restarted = fixture.supervisor(backend.clone());
        let running = restarted.statuses();
        assert_eq!(running.instances[0].state, RunnerInstanceStateV1::Running);
        assert!(!running.instances[0].managed_by_supervisor);
        let _ = restarted.start("local-mcp-01");
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);

        backend.set_healthy(false);
        let stopped_fixture = Fixture::new();
        stopped_fixture.write_manifest("Stopped Runner", "r1", &["1.1"]);
        let stopped = stopped_fixture.supervisor(backend.clone()).statuses();
        assert_eq!(stopped.instances[0].state, RunnerInstanceStateV1::Stopped);
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn browser_surface_has_no_registration_upload_marketplace_or_command_routes() {
        let fixture = Fixture::new();
        fixture.write_manifest("Local MCP Helper", "r2", &["1.1"]);
        let router = fixture.router(FakeBackend::healthy());
        for path in [
            "/api/v1/runner-templates/register",
            "/api/v1/runner-templates/upload",
            "/api/v1/runner-templates/marketplace",
            "/api/v1/runner-instances/local-mcp-01/command",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), 404);
        }
    }

    #[derive(Clone)]
    struct FakeBackend {
        launches: Arc<AtomicUsize>,
        healthy: Arc<Mutex<bool>>,
        running: Arc<Mutex<BTreeSet<String>>>,
        last_executable: Arc<Mutex<Option<PathBuf>>>,
        last_environment: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl FakeBackend {
        fn healthy() -> Self {
            Self {
                launches: Arc::new(AtomicUsize::new(0)),
                healthy: Arc::new(Mutex::new(true)),
                running: Arc::new(Mutex::new(BTreeSet::new())),
                last_executable: Arc::new(Mutex::new(None)),
                last_environment: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn unhealthy() -> Self {
            let backend = Self::healthy();
            backend.set_healthy(false);
            backend
        }

        fn set_healthy(&self, healthy: bool) {
            *self
                .healthy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = healthy;
        }
    }

    impl RunnerRuntimeBackend for FakeBackend {
        fn launch(
            &self,
            executable: &Path,
            environment: &[(String, String)],
        ) -> Result<Box<dyn ManagedRunner>, ()> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            *self
                .last_executable
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(executable.to_path_buf());
            *self
                .last_environment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = environment.to_vec();
            let instance_id = environment
                .iter()
                .find(|(key, _)| key == "WORLDSTREAM_RUNNER_INSTANCE_ID")
                .map(|(_, value)| value.clone())
                .ok_or(())?;
            self.running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(instance_id.clone());
            Ok(Box::new(FakeProcess {
                instance_id,
                running: Arc::clone(&self.running),
            }))
        }

        fn probe(
            &self,
            instance_id: &str,
            _address: std::net::SocketAddr,
            _path: &str,
            _timeout: Duration,
        ) -> Result<RunnerHealthObservationV1, ()> {
            if *self
                .healthy
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                && self
                    .running
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(instance_id)
            {
                Ok(RunnerHealthObservationV1 { capacity_in_use: 1 })
            } else {
                Err(())
            }
        }
    }

    struct FakeProcess {
        instance_id: String,
        running: Arc<Mutex<BTreeSet<String>>>,
    }

    impl ManagedRunner for FakeProcess {
        fn try_wait(&mut self) -> Result<Option<ProcessExit>, ()> {
            Ok(None)
        }

        fn graceful_stop(&mut self, _timeout: Duration) -> Result<bool, ()> {
            self.running
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.instance_id);
            Ok(true)
        }
    }

    struct Fixture {
        _root: TempDir,
        manifests: PathBuf,
        registry: PathBuf,
        runtime: PathBuf,
        vault: FileSecretVaultV1,
        executable: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir()
                .unwrap_or_else(|error| unreachable!("temporary runner fixture: {error}"));
            let manifests = root.path().join("owner-manifests");
            fs::create_dir(&manifests)
                .unwrap_or_else(|error| unreachable!("manifest directory: {error}"));
            let executable = root.path().join("approved-runner");
            fs::write(&executable, b"approved runner fixture")
                .unwrap_or_else(|error| unreachable!("runner executable: {error}"));
            let vault = FileSecretVaultV1::open(&root.path().join("secrets"))
                .unwrap_or_else(|error| unreachable!("secret vault: {error}"));
            Self {
                registry: root.path().join("installed"),
                runtime: root.path().join("runtime"),
                _root: root,
                manifests,
                vault,
                executable,
            }
        }

        fn write_manifest(&self, display_name: &str, revision: &str, revisions: &[&str]) {
            self.write_manifest_for_pack(display_name, revision, "agent-heist", revisions);
        }

        fn write_manifest_for_pack(
            &self,
            display_name: &str,
            revision: &str,
            activity_pack_id: &str,
            revisions: &[&str],
        ) {
            let digest = blake3::hash(b"approved runner fixture")
                .to_hex()
                .to_string();
            let manifest = serde_json::json!({
                "schema": "worldstream/runner-template/v1",
                "template_id": if display_name == "Stopped Runner" { "stopped-runner" } else { "local-mcp-helper" },
                "revision": revision,
                "display_name": display_name,
                "executable": { "path": self.executable, "blake3": digest },
                "compatibility": [{ "activity_pack_id": activity_pack_id, "exact_revisions": revisions }],
                "capacity": { "maximum_concurrent_invocations": 4 },
                "health": { "path": "/healthz", "timeout_ms": 250, "stale_after_ms": 5000 },
                "non_secret_environment": { "RUNNER_MODE": "stdio" },
                "secret_environment": [],
                "instances": [{
                    "instance_id": if display_name == "Stopped Runner" { "stopped-01" } else { "local-mcp-01" },
                    "health_address": "127.0.0.1:9501"
                }]
            });
            fs::write(
                self.manifests.join("runner.json"),
                serde_json::to_vec_pretty(&manifest)
                    .unwrap_or_else(|error| unreachable!("manifest JSON: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("write manifest: {error}"));
        }

        fn open_registry(&self) -> RunnerTemplateRegistryV1 {
            RunnerTemplateRegistryV1::open(&self.registry, &self.manifests)
                .unwrap_or_else(|error| unreachable!("open registry: {error}"))
        }

        fn supervisor(&self, backend: FakeBackend) -> RunnerSupervisorV1 {
            RunnerSupervisorV1::with_backend(
                self.open_registry(),
                &self.runtime,
                self.vault.clone(),
                Duration::from_millis(50),
                backend,
            )
            .unwrap_or_else(|error| unreachable!("runner supervisor: {error}"))
        }

        fn router(&self, backend: FakeBackend) -> axum::Router {
            runner_router(self.supervisor(backend))
        }
    }

    async fn post(router: &axum::Router, path: &str, body: &str) -> RunnerInstanceStatusResponseV1 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::from(body.to_owned()))
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        decode(response).await
    }

    async fn get_instances(router: &axum::Router) -> RunnerInstanceStatusResponseV1 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/runner-instances")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        decode(response).await
    }

    async fn decode(response: axum::response::Response) -> RunnerInstanceStatusResponseV1 {
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("response body: {error}"))
            .to_bytes();
        serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("instance status JSON: {error}"))
    }
}
