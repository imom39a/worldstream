//! Host-local Activity Client deployments, bindings, and deterministic selection.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use worldstream_activity_client::{
    ActivityClientReleaseV1, ExactPackReferenceV1, read_activity_client_release,
};
use worldstream_protocol::AccessMode;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

const BOOTSTRAP_SCHEMA_V1: &str = "worldstream/client-binding-bootstrap/v1";
const DEPLOYMENT_SCHEMA_V1: &str = "worldstream/client-deployment/v1";
const BINDING_SCHEMA_V1: &str = "worldstream/client-binding/v1";
const INSPECTOR_FALLBACK_SCHEMA_V1: &str = "worldstream/inspector-fallback/v1";
const DEPLOYMENT_STATUS_SCHEMA_V1: &str = "worldstream/client-deployment-status/v1";
const BINDING_STATUS_SCHEMA_V1: &str = "worldstream/client-binding-status/v1";
const MAX_RECORD_BYTES: usize = 256 * 1024;
const MAX_RECORDS: usize = 1_024;
const MAX_SURFACES: usize = 32;
const MAX_ROLES: usize = 64;
const MAX_TEXT_BYTES: usize = 128;
const MAX_LAUNCH_URL_BYTES: usize = 2_048;

/// Host approval provenance for one already-running deployment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTrustLevelV1 {
    Verified,
    ExternallyTrusted,
}

/// Host-local admission policy for client Deployment trust classifications.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientDeploymentTrustPolicyV1 {
    VerifiedOnly,
    AllowExternallyTrusted,
}

impl ClientDeploymentTrustPolicyV1 {
    const fn permits(self, trust_level: DeploymentTrustLevelV1) -> bool {
        matches!(trust_level, DeploymentTrustLevelV1::Verified)
            || matches!(self, Self::AllowExternallyTrusted)
    }
}

/// One release-declared surface made available at a Host-local launch URL.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeployedClientSurfaceV1 {
    pub surface_id: String,
    pub launch_url: String,
}

/// Immutable Host record for one already-running Activity Client release.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDeploymentV1 {
    pub schema: String,
    pub deployment_id: String,
    pub client_id: String,
    pub release_digest: String,
    pub trust_level: DeploymentTrustLevelV1,
    pub verification_evidence_digest: Option<String>,
    pub surfaces: Vec<DeployedClientSurfaceV1>,
}

/// Operator preference applied only after every exact binding predicate matches.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientBindingPreferenceV1 {
    Eligible,
    Default,
}

/// Immutable Host approval for one exact Pack/Membership/client surface tuple.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientBindingV1 {
    pub schema: String,
    pub binding_id: String,
    pub pack: ExactPackReferenceV1,
    pub client_contract: String,
    pub access_mode: AccessMode,
    pub roles: Vec<String>,
    pub deployment_id: String,
    pub surface_id: String,
    pub preference: ClientBindingPreferenceV1,
}

/// Host-approved generic Inspector used only when no specialized binding is viable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InspectorFallbackV1 {
    pub schema: String,
    pub fallback_id: String,
    pub deployment_id: String,
    pub surface_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientBindingBootstrapV1 {
    schema: String,
    deployment_trust_policy: ClientDeploymentTrustPolicyV1,
    deployments: Vec<ClientDeploymentV1>,
    bindings: Vec<ClientBindingV1>,
    inspector_fallback: InspectorFallbackV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClientDeploymentStatusV1 {
    Ready,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientDeploymentStatusRecordV1 {
    schema: String,
    deployment_id: String,
    status: ClientDeploymentStatusV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClientBindingStatusV1 {
    Approved,
    Disabled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientBindingStatusRecordV1 {
    schema: String,
    binding_id: String,
    status: ClientBindingStatusV1,
}

/// Current authoritative facts used for one selection attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientSelectionRequestV1 {
    pub pack: ExactPackReferenceV1,
    pub client_contract: String,
    pub access_mode: AccessMode,
    pub role: Option<String>,
}

/// One browser-safe approved candidate. The launch URL stays inside the Host
/// broker until this candidate is selected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCandidateV1 {
    pub candidate_id: String,
    pub deployment_id: String,
    pub client_id: String,
    pub release_digest: String,
    pub surface_id: String,
    pub trust_level: DeploymentTrustLevelV1,
    pub launch_url: String,
}

/// Immutable identity class retained by an already-issued browser handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientCandidateClassV1 {
    Binding,
    InspectorFallback,
}

/// Complete deterministic result of evaluating current Host-local trust.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientSelectionV1 {
    Selected { candidate: ClientCandidateV1 },
    SelectionRequired { candidates: Vec<ClientCandidateV1> },
    InspectorFallback { candidate: ClientCandidateV1 },
}

/// Selection seam consumed by the handoff broker. Tests can supply a bounded
/// in-memory adapter without learning the durable store's file layout.
pub trait ClientSelectionSourceV1: Send + Sync + 'static {
    /// Evaluates one current Membership request and optional prior candidate.
    ///
    /// # Errors
    ///
    /// Returns the same closed store errors as the durable implementation.
    fn select_client(
        &self,
        request: &ClientSelectionRequestV1,
        preferred_candidate_id: Option<&str>,
    ) -> Result<ClientSelectionV1, ClientBindingStoreErrorV1>;

    /// Revalidates an already-issued exact client without applying new-handoff
    /// preference or Binding-disable policy.
    ///
    /// # Errors
    ///
    /// Returns `InvalidChoice` when the Membership tuple no longer matches or
    /// the retained Deployment was revoked, and `Unavailable` when current
    /// Host trust state cannot be read safely.
    fn resolve_active_client(
        &self,
        request: &ClientSelectionRequestV1,
        class: ClientCandidateClassV1,
        candidate_id: &str,
    ) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1>;
}

/// Closed storage and selection failures. No filesystem details cross the seam.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ClientBindingStoreErrorV1 {
    #[error("client binding input is invalid")]
    Invalid,
    #[error("client binding choice is not approved")]
    InvalidChoice,
    #[error("client binding state is unavailable")]
    Unavailable,
}

#[derive(Clone)]
struct StorePathsV1 {
    releases: PathBuf,
    deployments: PathBuf,
    bindings: PathBuf,
    inspector_fallback: PathBuf,
    deployment_status: PathBuf,
    binding_status: PathBuf,
}

#[derive(Clone)]
pub struct ClientBindingStoreV1 {
    paths: Arc<StorePathsV1>,
    deployment_trust_policy: ClientDeploymentTrustPolicyV1,
    mutation: Arc<Mutex<()>>,
}

impl ClientBindingStoreV1 {
    /// Opens Host-local state from an operator-controlled release directory
    /// and one reviewed deployment/binding bootstrap document.
    ///
    /// # Errors
    ///
    /// Rejects missing, non-regular, oversized, or malformed configuration,
    /// then applies the same validation and persistence rules as [`Self::open`].
    pub fn open_configured(
        root: &Path,
        releases_directory: &Path,
        bootstrap_path: &Path,
    ) -> Result<Self, ClientBindingStoreErrorV1> {
        let mut release_paths = read_configuration_paths(releases_directory)?;
        release_paths.sort();
        let release_documents = release_paths
            .iter()
            .map(|path| read_configuration_document(path))
            .collect::<Result<Vec<_>, _>>()?;
        let release_slices: Vec<&[u8]> = release_documents.iter().map(Vec::as_slice).collect();
        let bootstrap = read_configuration_document(bootstrap_path)?;
        Self::open(root, &release_slices, &bootstrap)
    }

    /// Opens durable Host-local trust state and idempotently imports an
    /// operator-reviewed bootstrap. Existing immutable identities must match
    /// byte-for-byte semantics; disable and revoke state is never reset.
    ///
    /// # Errors
    ///
    /// Returns a closed invalid or unavailable result for malformed input,
    /// conflicting identities, unsafe launch URLs, or protected-state failure.
    pub fn open(
        root: &Path,
        release_documents: &[&[u8]],
        bootstrap_document: &[u8],
    ) -> Result<Self, ClientBindingStoreErrorV1> {
        let bootstrap = read_bootstrap(bootstrap_document)?;
        let store = Self::open_layout(root, bootstrap.deployment_trust_policy)?;
        store.import(release_documents, bootstrap_document)?;
        Ok(store)
    }

    /// Opens retained client trust state without importing declarations or approval.
    /// The caller supplies selection policy; opening never rewrites retained trust,
    /// disable, or revoke records. A fully empty inventory is valid but cannot launch.
    ///
    /// # Errors
    /// Rejects inconsistent retained inventory or unavailable protected storage.
    pub fn open_installed(
        root: &Path,
        deployment_trust_policy: ClientDeploymentTrustPolicyV1,
    ) -> Result<Self, ClientBindingStoreErrorV1> {
        let store = Self::open_layout(root, deployment_trust_policy)?;
        store.load_inventory()?;
        Ok(store)
    }

    fn open_layout(
        root: &Path,
        deployment_trust_policy: ClientDeploymentTrustPolicyV1,
    ) -> Result<Self, ClientBindingStoreErrorV1> {
        let root =
            prepare_data_directory(root).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        let paths = StorePathsV1 {
            releases: prepare_store_directory(&root, "releases")?,
            deployments: prepare_store_directory(&root, "deployments")?,
            bindings: prepare_store_directory(&root, "bindings")?,
            inspector_fallback: prepare_store_directory(&root, "inspector-fallback")?,
            deployment_status: prepare_store_directory(&root, "deployment-status")?,
            binding_status: prepare_store_directory(&root, "binding-status")?,
        };
        Ok(Self {
            paths: Arc::new(paths),
            deployment_trust_policy,
            mutation: Arc::new(Mutex::new(())),
        })
    }

    /// Selects from current immutable identities and current disable/revoke
    /// state. `preferred_candidate_id` is an opaque candidate previously
    /// returned by this exact selection interface.
    ///
    /// # Errors
    ///
    /// Returns `InvalidChoice` when an explicit choice is no longer viable,
    /// and `Unavailable` for protected-state failure.
    pub fn select(
        &self,
        request: &ClientSelectionRequestV1,
        preferred_candidate_id: Option<&str>,
    ) -> Result<ClientSelectionV1, ClientBindingStoreErrorV1> {
        validate_selection_request(request)?;
        let _guard = self.lock();
        let inventory = self.load_inventory()?;
        let mut candidates: BTreeMap<(String, String), (ClientCandidateV1, bool)> = BTreeMap::new();
        for binding in inventory.bindings.values() {
            if inventory.binding_status.get(&binding.binding_id)
                != Some(&ClientBindingStatusV1::Approved)
                || binding.pack.digest != request.pack.digest
                || binding.client_contract != request.client_contract
                || binding.access_mode != request.access_mode
                || !role_matches(&binding.roles, request.role.as_deref())
            {
                continue;
            }
            let Some(deployment) = inventory.deployments.get(&binding.deployment_id) else {
                return Err(ClientBindingStoreErrorV1::Unavailable);
            };
            if inventory.deployment_status.get(&deployment.deployment_id)
                != Some(&ClientDeploymentStatusV1::Ready)
                || !self.deployment_trust_policy.permits(deployment.trust_level)
            {
                continue;
            }
            let Some(surface) = deployment
                .surfaces
                .iter()
                .find(|surface| surface.surface_id == binding.surface_id)
            else {
                return Err(ClientBindingStoreErrorV1::Unavailable);
            };
            let candidate = ClientCandidateV1 {
                candidate_id: binding.binding_id.clone(),
                deployment_id: deployment.deployment_id.clone(),
                client_id: deployment.client_id.clone(),
                release_digest: deployment.release_digest.clone(),
                surface_id: surface.surface_id.clone(),
                trust_level: deployment.trust_level,
                launch_url: surface.launch_url.clone(),
            };
            let key = (
                candidate.deployment_id.clone(),
                candidate.surface_id.clone(),
            );
            let is_default = binding.preference == ClientBindingPreferenceV1::Default;
            match candidates.get_mut(&key) {
                Some((retained, retained_default)) => {
                    if candidate.candidate_id < retained.candidate_id {
                        *retained = candidate;
                    }
                    *retained_default |= is_default;
                }
                None => {
                    candidates.insert(key, (candidate, is_default));
                }
            }
        }

        let candidates: Vec<(ClientCandidateV1, bool)> = candidates.into_values().collect();
        if let Some(preferred) = preferred_candidate_id {
            return candidates
                .into_iter()
                .find_map(|(candidate, _)| {
                    (candidate.candidate_id == preferred).then_some(candidate)
                })
                .map(|candidate| ClientSelectionV1::Selected { candidate })
                .ok_or(ClientBindingStoreErrorV1::InvalidChoice);
        }
        match candidates.as_slice() {
            [] => inspector_candidate(&inventory, self.deployment_trust_policy)
                .map(|candidate| ClientSelectionV1::InspectorFallback { candidate }),
            [(candidate, _)] => Ok(ClientSelectionV1::Selected {
                candidate: candidate.clone(),
            }),
            many => {
                let defaults: Vec<&ClientCandidateV1> = many
                    .iter()
                    .filter_map(|(candidate, is_default)| is_default.then_some(candidate))
                    .collect();
                if let [candidate] = defaults.as_slice() {
                    return Ok(ClientSelectionV1::Selected {
                        candidate: (*candidate).clone(),
                    });
                }
                Ok(ClientSelectionV1::SelectionRequired {
                    candidates: many
                        .iter()
                        .map(|(candidate, _)| candidate.clone())
                        .collect(),
                })
            }
        }
    }

    /// Resolves one exact client retained by an active browser session.
    ///
    /// A disabled Binding remains valid for a session that already retained
    /// it, while a revoked Deployment is terminal. Membership compatibility,
    /// exact Release identity, surface identity, and current Host trust policy
    /// are checked on every call.
    ///
    /// # Errors
    ///
    /// Returns `InvalidChoice` when the retained client is no longer valid for
    /// the current Membership or its Deployment was revoked, and `Unavailable`
    /// for corrupt or unreadable protected state.
    pub fn resolve_active_client(
        &self,
        request: &ClientSelectionRequestV1,
        class: ClientCandidateClassV1,
        candidate_id: &str,
    ) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1> {
        validate_selection_request(request)?;
        if !is_identifier(candidate_id) {
            return Err(ClientBindingStoreErrorV1::InvalidChoice);
        }
        let _guard = self.lock();
        let inventory = self.load_inventory()?;
        match class {
            ClientCandidateClassV1::Binding => {
                let binding = inventory
                    .bindings
                    .get(candidate_id)
                    .ok_or(ClientBindingStoreErrorV1::InvalidChoice)?;
                if binding.pack.digest != request.pack.digest
                    || binding.client_contract != request.client_contract
                    || binding.access_mode != request.access_mode
                    || !role_matches(&binding.roles, request.role.as_deref())
                {
                    return Err(ClientBindingStoreErrorV1::InvalidChoice);
                }
                let deployment = inventory
                    .deployments
                    .get(&binding.deployment_id)
                    .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
                active_candidate(
                    &inventory,
                    self.deployment_trust_policy,
                    candidate_id,
                    deployment,
                    &binding.surface_id,
                )
            }
            ClientCandidateClassV1::InspectorFallback => {
                let fallback = inventory
                    .inspector_fallback
                    .as_ref()
                    .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
                if fallback.fallback_id != candidate_id {
                    return Err(ClientBindingStoreErrorV1::InvalidChoice);
                }
                let deployment = inventory
                    .deployments
                    .get(&fallback.deployment_id)
                    .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
                active_candidate(
                    &inventory,
                    self.deployment_trust_policy,
                    candidate_id,
                    deployment,
                    &fallback.surface_id,
                )
            }
        }
    }

    /// Monotonically revokes a deployment without changing its identity.
    ///
    /// # Errors
    ///
    /// Returns `Invalid` for an unknown deployment and `Unavailable` for a
    /// protected-state failure.
    pub fn revoke_deployment(&self, deployment_id: &str) -> Result<(), ClientBindingStoreErrorV1> {
        let _guard = self.lock();
        if !self
            .load_inventory()?
            .deployments
            .contains_key(deployment_id)
        {
            return Err(ClientBindingStoreErrorV1::Invalid);
        }
        write_replace(
            &self.paths.deployment_status,
            deployment_id,
            &ClientDeploymentStatusRecordV1 {
                schema: DEPLOYMENT_STATUS_SCHEMA_V1.to_owned(),
                deployment_id: deployment_id.to_owned(),
                status: ClientDeploymentStatusV1::Revoked,
            },
        )
    }

    /// Monotonically disables a binding without changing its approved tuple.
    ///
    /// # Errors
    ///
    /// Returns `Invalid` for an unknown binding and `Unavailable` for a
    /// protected-state failure.
    pub fn disable_binding(&self, binding_id: &str) -> Result<(), ClientBindingStoreErrorV1> {
        let _guard = self.lock();
        if !self.load_inventory()?.bindings.contains_key(binding_id) {
            return Err(ClientBindingStoreErrorV1::Invalid);
        }
        write_replace(
            &self.paths.binding_status,
            binding_id,
            &ClientBindingStatusRecordV1 {
                schema: BINDING_STATUS_SCHEMA_V1.to_owned(),
                binding_id: binding_id.to_owned(),
                status: ClientBindingStatusV1::Disabled,
            },
        )
    }

    fn import(
        &self,
        release_documents: &[&[u8]],
        bootstrap_document: &[u8],
    ) -> Result<(), ClientBindingStoreErrorV1> {
        let _guard = self.lock();
        let mut releases = load_releases(&self.paths.releases)?;
        let mut imported_releases = Vec::with_capacity(release_documents.len());
        for document in release_documents {
            let release = read_activity_client_release(document)
                .map_err(|_| ClientBindingStoreErrorV1::Invalid)?;
            insert_exact_release(&mut releases, &release)?;
            imported_releases.push(release);
        }
        let bootstrap = read_bootstrap(bootstrap_document)?;
        validate_bootstrap(&bootstrap, &releases)?;

        let existing = self.load_inventory()?;
        for release in &imported_releases {
            write_immutable(&self.paths.releases, &release_record_id(release), release)?;
        }
        for deployment in &bootstrap.deployments {
            if let Some(current) = existing.deployments.get(&deployment.deployment_id)
                && current != deployment
            {
                return Err(ClientBindingStoreErrorV1::Invalid);
            }
            write_immutable(
                &self.paths.deployments,
                &deployment.deployment_id,
                deployment,
            )?;
            write_status_if_absent(
                &self.paths.deployment_status,
                &deployment.deployment_id,
                &ClientDeploymentStatusRecordV1 {
                    schema: DEPLOYMENT_STATUS_SCHEMA_V1.to_owned(),
                    deployment_id: deployment.deployment_id.clone(),
                    status: ClientDeploymentStatusV1::Ready,
                },
            )?;
        }
        for binding in &bootstrap.bindings {
            if let Some(current) = existing.bindings.get(&binding.binding_id)
                && current != binding
            {
                return Err(ClientBindingStoreErrorV1::Invalid);
            }
            write_immutable(&self.paths.bindings, &binding.binding_id, binding)?;
            write_status_if_absent(
                &self.paths.binding_status,
                &binding.binding_id,
                &ClientBindingStatusRecordV1 {
                    schema: BINDING_STATUS_SCHEMA_V1.to_owned(),
                    binding_id: binding.binding_id.clone(),
                    status: ClientBindingStatusV1::Approved,
                },
            )?;
        }
        if let Some(current) = existing.inspector_fallback.as_ref()
            && current != &bootstrap.inspector_fallback
        {
            return Err(ClientBindingStoreErrorV1::Invalid);
        }
        write_immutable(
            &self.paths.inspector_fallback,
            &bootstrap.inspector_fallback.fallback_id,
            &bootstrap.inspector_fallback,
        )?;
        self.load_inventory().map(|_| ())
    }

    fn load_inventory(&self) -> Result<ClientBindingInventoryV1, ClientBindingStoreErrorV1> {
        let releases = load_releases(&self.paths.releases)?;
        let deployments =
            load_keyed_records::<ClientDeploymentV1>(&self.paths.deployments, |record| {
                &record.deployment_id
            })?;
        let bindings = load_keyed_records::<ClientBindingV1>(&self.paths.bindings, |record| {
            &record.binding_id
        })?;
        let mut fallback_records =
            load_keyed_records::<InspectorFallbackV1>(&self.paths.inspector_fallback, |record| {
                &record.fallback_id
            })?;
        let inspector_fallback = if fallback_records.len() == 1 {
            fallback_records.pop_first().map(|(_, record)| record)
        } else {
            None
        };
        let deployment_status_records = load_keyed_records::<ClientDeploymentStatusRecordV1>(
            &self.paths.deployment_status,
            |record| &record.deployment_id,
        )?;
        let binding_status_records = load_keyed_records::<ClientBindingStatusRecordV1>(
            &self.paths.binding_status,
            |record| &record.binding_id,
        )?;
        if deployment_status_records.values().any(|record| {
            record.schema != DEPLOYMENT_STATUS_SCHEMA_V1 || !is_identifier(&record.deployment_id)
        }) || binding_status_records.values().any(|record| {
            record.schema != BINDING_STATUS_SCHEMA_V1 || !is_identifier(&record.binding_id)
        }) {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        }
        let deployment_status = deployment_status_records
            .into_iter()
            .map(|(id, record)| (id, record.status))
            .collect();
        let binding_status = binding_status_records
            .into_iter()
            .map(|(id, record)| (id, record.status))
            .collect();
        let inventory = ClientBindingInventoryV1 {
            releases,
            deployments,
            bindings,
            inspector_fallback,
            deployment_status,
            binding_status,
        };
        validate_inventory(&inventory)?;
        Ok(inventory)
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ClientSelectionSourceV1 for ClientBindingStoreV1 {
    fn select_client(
        &self,
        request: &ClientSelectionRequestV1,
        preferred_candidate_id: Option<&str>,
    ) -> Result<ClientSelectionV1, ClientBindingStoreErrorV1> {
        self.select(request, preferred_candidate_id)
    }

    fn resolve_active_client(
        &self,
        request: &ClientSelectionRequestV1,
        class: ClientCandidateClassV1,
        candidate_id: &str,
    ) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1> {
        self.resolve_active_client(request, class, candidate_id)
    }
}

struct ClientBindingInventoryV1 {
    releases: BTreeMap<(String, String), ActivityClientReleaseV1>,
    deployments: BTreeMap<String, ClientDeploymentV1>,
    bindings: BTreeMap<String, ClientBindingV1>,
    inspector_fallback: Option<InspectorFallbackV1>,
    deployment_status: BTreeMap<String, ClientDeploymentStatusV1>,
    binding_status: BTreeMap<String, ClientBindingStatusV1>,
}

fn prepare_store_directory(root: &Path, name: &str) -> Result<PathBuf, ClientBindingStoreErrorV1> {
    prepare_data_directory(&root.join(name)).map_err(|_| ClientBindingStoreErrorV1::Unavailable)
}

fn read_configuration_paths(directory: &Path) -> Result<Vec<PathBuf>, ClientBindingStoreErrorV1> {
    let metadata =
        fs::symlink_metadata(directory).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let paths = fs::read_dir(directory)
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|_| ClientBindingStoreErrorV1::Unavailable)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if paths.is_empty() || paths.len() > MAX_RECORDS {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    if paths
        .iter()
        .any(|path| path.extension().and_then(|extension| extension.to_str()) != Some("json"))
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    Ok(paths)
}

fn read_configuration_document(path: &Path) -> Result<Vec<u8>, ClientBindingStoreErrorV1> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RECORD_BYTES as u64
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    fs::read(path).map_err(|_| ClientBindingStoreErrorV1::Unavailable)
}

fn read_bootstrap(document: &[u8]) -> Result<ClientBindingBootstrapV1, ClientBindingStoreErrorV1> {
    if document.is_empty() || document.len() > MAX_RECORD_BYTES {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    serde_json::from_slice(document).map_err(|_| ClientBindingStoreErrorV1::Invalid)
}

fn validate_bootstrap(
    bootstrap: &ClientBindingBootstrapV1,
    releases: &BTreeMap<(String, String), ActivityClientReleaseV1>,
) -> Result<(), ClientBindingStoreErrorV1> {
    if bootstrap.schema != BOOTSTRAP_SCHEMA_V1
        || bootstrap.deployments.len() > MAX_RECORDS
        || bootstrap.bindings.len() > MAX_RECORDS
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let mut deployments = BTreeMap::new();
    for deployment in &bootstrap.deployments {
        validate_deployment(deployment, releases)?;
        if deployments
            .insert(deployment.deployment_id.clone(), deployment.clone())
            .is_some()
        {
            return Err(ClientBindingStoreErrorV1::Invalid);
        }
    }
    let mut bindings = BTreeSet::new();
    for binding in &bootstrap.bindings {
        validate_binding(binding, &deployments, releases)?;
        if !bindings.insert(binding.binding_id.as_str()) {
            return Err(ClientBindingStoreErrorV1::Invalid);
        }
    }
    validate_inspector_fallback(&bootstrap.inspector_fallback, &deployments, releases)?;
    Ok(())
}

fn validate_inventory(
    inventory: &ClientBindingInventoryV1,
) -> Result<(), ClientBindingStoreErrorV1> {
    if inventory.releases.is_empty()
        && inventory.deployments.is_empty()
        && inventory.bindings.is_empty()
        && inventory.deployment_status.is_empty()
        && inventory.binding_status.is_empty()
        && inventory.inspector_fallback.is_none()
    {
        return Ok(());
    }
    if inventory.releases.len() > MAX_RECORDS
        || inventory.deployments.len() > MAX_RECORDS
        || inventory.bindings.len() > MAX_RECORDS
        || inventory.inspector_fallback.is_none()
        || inventory.deployment_status.len() != inventory.deployments.len()
        || inventory.binding_status.len() != inventory.bindings.len()
    {
        return Err(ClientBindingStoreErrorV1::Unavailable);
    }
    for deployment in inventory.deployments.values() {
        validate_deployment(deployment, &inventory.releases)
            .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        if !inventory
            .deployment_status
            .contains_key(&deployment.deployment_id)
        {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        }
    }
    for binding in inventory.bindings.values() {
        validate_binding(binding, &inventory.deployments, &inventory.releases)
            .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        if !inventory.binding_status.contains_key(&binding.binding_id) {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        }
    }
    validate_inspector_fallback(
        inventory
            .inspector_fallback
            .as_ref()
            .ok_or(ClientBindingStoreErrorV1::Unavailable)?,
        &inventory.deployments,
        &inventory.releases,
    )
    .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    Ok(())
}

fn validate_inspector_fallback(
    fallback: &InspectorFallbackV1,
    deployments: &BTreeMap<String, ClientDeploymentV1>,
    releases: &BTreeMap<(String, String), ActivityClientReleaseV1>,
) -> Result<(), ClientBindingStoreErrorV1> {
    if fallback.schema != INSPECTOR_FALLBACK_SCHEMA_V1
        || !is_identifier(&fallback.fallback_id)
        || !is_identifier(&fallback.deployment_id)
        || !is_identifier(&fallback.surface_id)
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let deployment = deployments
        .get(&fallback.deployment_id)
        .ok_or(ClientBindingStoreErrorV1::Invalid)?;
    let release = releases
        .get(&(
            deployment.client_id.clone(),
            deployment.release_digest.clone(),
        ))
        .ok_or(ClientBindingStoreErrorV1::Invalid)?;
    if !deployment
        .surfaces
        .iter()
        .any(|surface| surface.surface_id == fallback.surface_id)
        || !release
            .surfaces
            .iter()
            .any(|surface| surface.surface_id == fallback.surface_id)
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    Ok(())
}

fn validate_deployment(
    deployment: &ClientDeploymentV1,
    releases: &BTreeMap<(String, String), ActivityClientReleaseV1>,
) -> Result<(), ClientBindingStoreErrorV1> {
    if deployment.schema != DEPLOYMENT_SCHEMA_V1
        || !is_identifier(&deployment.deployment_id)
        || !is_identifier(&deployment.client_id)
        || !is_digest(&deployment.release_digest)
        || deployment.surfaces.is_empty()
        || deployment.surfaces.len() > MAX_SURFACES
        || matches!(deployment.trust_level, DeploymentTrustLevelV1::Verified)
            != deployment.verification_evidence_digest.is_some()
        || deployment
            .verification_evidence_digest
            .as_deref()
            .is_some_and(|digest| !is_digest(digest))
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let release = releases
        .get(&(
            deployment.client_id.clone(),
            deployment.release_digest.clone(),
        ))
        .ok_or(ClientBindingStoreErrorV1::Invalid)?;
    let mut surface_ids = BTreeSet::new();
    for surface in &deployment.surfaces {
        let declared = release
            .surfaces
            .iter()
            .find(|declared| declared.surface_id == surface.surface_id);
        if !is_identifier(&surface.surface_id)
            || !is_loopback_launch_url(&surface.launch_url)
            || !surface_ids.insert(surface.surface_id.as_str())
            || declared.is_none_or(|declared| {
                !launch_url_matches_entrypoint(&surface.launch_url, &declared.entrypoint)
            })
        {
            return Err(ClientBindingStoreErrorV1::Invalid);
        }
    }
    Ok(())
}

fn validate_binding(
    binding: &ClientBindingV1,
    deployments: &BTreeMap<String, ClientDeploymentV1>,
    releases: &BTreeMap<(String, String), ActivityClientReleaseV1>,
) -> Result<(), ClientBindingStoreErrorV1> {
    if binding.schema != BINDING_SCHEMA_V1
        || !is_identifier(&binding.binding_id)
        || !is_identifier(&binding.pack.id)
        || binding.pack.version.is_empty()
        || binding.pack.version.len() > MAX_TEXT_BYTES
        || !is_digest(&binding.pack.digest)
        || !is_contract(&binding.client_contract)
        || binding.roles.len() > MAX_ROLES
        || !is_identifier(&binding.deployment_id)
        || !is_identifier(&binding.surface_id)
        || !unique_roles(&binding.roles)
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let deployment = deployments
        .get(&binding.deployment_id)
        .ok_or(ClientBindingStoreErrorV1::Invalid)?;
    let release = releases
        .get(&(
            deployment.client_id.clone(),
            deployment.release_digest.clone(),
        ))
        .ok_or(ClientBindingStoreErrorV1::Invalid)?;
    if release.client_contract != binding.client_contract
        || !deployment
            .surfaces
            .iter()
            .any(|surface| surface.surface_id == binding.surface_id)
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    Ok(())
}

fn validate_selection_request(
    request: &ClientSelectionRequestV1,
) -> Result<(), ClientBindingStoreErrorV1> {
    if !is_identifier(&request.pack.id)
        || request.pack.version.is_empty()
        || request.pack.version.len() > MAX_TEXT_BYTES
        || !is_digest(&request.pack.digest)
        || !is_contract(&request.client_contract)
        || request.role.as_deref().is_some_and(|role| !is_role(role))
    {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    Ok(())
}

fn load_releases(
    directory: &Path,
) -> Result<BTreeMap<(String, String), ActivityClientReleaseV1>, ClientBindingStoreErrorV1> {
    let mut releases = BTreeMap::new();
    for (_, bytes) in load_record_bytes(directory)? {
        let release = read_activity_client_release(&bytes)
            .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        insert_exact_release(&mut releases, &release)
            .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    }
    Ok(releases)
}

fn insert_exact_release(
    releases: &mut BTreeMap<(String, String), ActivityClientReleaseV1>,
    release: &ActivityClientReleaseV1,
) -> Result<(), ClientBindingStoreErrorV1> {
    let key = (release.client_id.clone(), release.release_digest.clone());
    if let Some(existing) = releases.get(&key) {
        return (existing == release)
            .then_some(())
            .ok_or(ClientBindingStoreErrorV1::Invalid);
    }
    releases.insert(key, release.clone());
    Ok(())
}

fn load_keyed_records<T>(
    directory: &Path,
    key: impl Fn(&T) -> &str,
) -> Result<BTreeMap<String, T>, ClientBindingStoreErrorV1>
where
    T: DeserializeOwned,
{
    let mut records = BTreeMap::new();
    for (file_stem, bytes) in load_record_bytes(directory)? {
        let record: T =
            serde_json::from_slice(&bytes).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        let record_key = key(&record);
        if record_key != file_stem || records.insert(file_stem, record).is_some() {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        }
    }
    Ok(records)
}

fn load_record_bytes(
    directory: &Path,
) -> Result<Vec<(String, Vec<u8>)>, ClientBindingStoreErrorV1> {
    let mut paths = fs::read_dir(directory)
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    paths.sort_by_key(fs::DirEntry::file_name);
    if paths.len() > MAX_RECORDS {
        return Err(ClientBindingStoreErrorV1::Unavailable);
    }
    let mut records = Vec::with_capacity(paths.len());
    for entry in paths {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        };
        if name.starts_with('.') {
            continue;
        }
        let Some(file_stem) = name.strip_suffix(".json") else {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        };
        if !is_identifier(file_stem) {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        }
        validate_owner_only_file(&path).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        if metadata.len() > MAX_RECORD_BYTES as u64 {
            return Err(ClientBindingStoreErrorV1::Unavailable);
        }
        records.push((
            file_stem.to_owned(),
            fs::read(path).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?,
        ));
    }
    Ok(records)
}

fn write_immutable<T: Serialize + DeserializeOwned + Eq>(
    directory: &Path,
    id: &str,
    value: &T,
) -> Result<(), ClientBindingStoreErrorV1> {
    if !is_identifier(id) {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let target = directory.join(format!("{id}.json"));
    if target.exists() {
        validate_owner_only_file(&target).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        let existing: T = serde_json::from_slice(
            &fs::read(&target).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?,
        )
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        return (existing == *value)
            .then_some(())
            .ok_or(ClientBindingStoreErrorV1::Invalid);
    }
    let bytes = serialize_record(value)?;
    let temporary = write_temporary(directory, id, &bytes)?;
    let result = fs::hard_link(&temporary, &target)
        .and_then(|()| sync_directory(directory))
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable);
    let _ = fs::remove_file(temporary);
    result
}

fn write_status_if_absent<T: Serialize>(
    directory: &Path,
    id: &str,
    value: &T,
) -> Result<(), ClientBindingStoreErrorV1> {
    let target = directory.join(format!("{id}.json"));
    if target.exists() {
        validate_owner_only_file(&target).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
        return Ok(());
    }
    let bytes = serialize_record(value)?;
    let temporary = write_temporary(directory, id, &bytes)?;
    let result = fs::hard_link(&temporary, &target)
        .and_then(|()| sync_directory(directory))
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable);
    let _ = fs::remove_file(temporary);
    result
}

fn write_replace<T: Serialize>(
    directory: &Path,
    id: &str,
    value: &T,
) -> Result<(), ClientBindingStoreErrorV1> {
    if !is_identifier(id) {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    let bytes = serialize_record(value)?;
    let temporary = write_temporary(directory, id, &bytes)?;
    let target = directory.join(format!("{id}.json"));
    let result = fs::rename(&temporary, target)
        .and_then(|()| sync_directory(directory))
        .map_err(|_| ClientBindingStoreErrorV1::Unavailable);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn serialize_record<T: Serialize>(value: &T) -> Result<Vec<u8>, ClientBindingStoreErrorV1> {
    let bytes = serde_json::to_vec(value).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(ClientBindingStoreErrorV1::Invalid);
    }
    Ok(bytes)
}

fn write_temporary(
    directory: &Path,
    id: &str,
    bytes: &[u8],
) -> Result<PathBuf, ClientBindingStoreErrorV1> {
    let temporary = directory.join(format!(".{id}.{}.tmp", random_suffix()?));
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    if file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        return Err(ClientBindingStoreErrorV1::Unavailable);
    }
    Ok(temporary)
}

fn release_record_id(release: &ActivityClientReleaseV1) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"worldstream-client-release-record-v1\0");
    hash.update(release.client_id.as_bytes());
    hash.update(b"\0");
    hash.update(release.release_digest.as_bytes());
    format!("release-{}", hash.finalize().to_hex())
}

fn inspector_candidate(
    inventory: &ClientBindingInventoryV1,
    trust_policy: ClientDeploymentTrustPolicyV1,
) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1> {
    let fallback = inventory
        .inspector_fallback
        .as_ref()
        .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
    let deployment = inventory
        .deployments
        .get(&fallback.deployment_id)
        .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
    if inventory.deployment_status.get(&deployment.deployment_id)
        != Some(&ClientDeploymentStatusV1::Ready)
        || !trust_policy.permits(deployment.trust_level)
    {
        return Err(ClientBindingStoreErrorV1::Unavailable);
    }
    let surface = deployment
        .surfaces
        .iter()
        .find(|surface| surface.surface_id == fallback.surface_id)
        .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
    Ok(ClientCandidateV1 {
        candidate_id: fallback.fallback_id.clone(),
        deployment_id: deployment.deployment_id.clone(),
        client_id: deployment.client_id.clone(),
        release_digest: deployment.release_digest.clone(),
        surface_id: surface.surface_id.clone(),
        trust_level: deployment.trust_level,
        launch_url: surface.launch_url.clone(),
    })
}

fn active_candidate(
    inventory: &ClientBindingInventoryV1,
    trust_policy: ClientDeploymentTrustPolicyV1,
    candidate_id: &str,
    deployment: &ClientDeploymentV1,
    surface_id: &str,
) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1> {
    if inventory.deployment_status.get(&deployment.deployment_id)
        != Some(&ClientDeploymentStatusV1::Ready)
        || !trust_policy.permits(deployment.trust_level)
    {
        return Err(ClientBindingStoreErrorV1::InvalidChoice);
    }
    let surface = deployment
        .surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .ok_or(ClientBindingStoreErrorV1::Unavailable)?;
    Ok(ClientCandidateV1 {
        candidate_id: candidate_id.to_owned(),
        deployment_id: deployment.deployment_id.clone(),
        client_id: deployment.client_id.clone(),
        release_digest: deployment.release_digest.clone(),
        surface_id: surface.surface_id.clone(),
        trust_level: deployment.trust_level,
        launch_url: surface.launch_url.clone(),
    })
}

fn role_matches(roles: &[String], role: Option<&str>) -> bool {
    match role {
        Some(role) => roles.iter().any(|candidate| candidate == role),
        None => roles.is_empty(),
    }
}

fn unique_roles(roles: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    roles
        .iter()
        .all(|role| is_role(role) && unique.insert(role.as_str()))
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn is_role(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_contract(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'/')
        })
}

fn is_digest(value: &str) -> bool {
    value
        .strip_prefix("blake3:")
        .or_else(|| value.strip_prefix("sha256:"))
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

fn is_loopback_launch_url(value: &str) -> bool {
    if value.len() > MAX_LAUNCH_URL_BYTES || value.contains(['?', '#', '\\', '\r', '\n']) {
        return false;
    }
    let Some(rest) = value.strip_prefix("http://") else {
        return false;
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return false;
    };
    let host_allowed = authority == "127.0.0.1"
        || authority == "localhost"
        || authority == "[::1]"
        || authority.strip_prefix("127.0.0.1:").is_some_and(valid_port)
        || authority.strip_prefix("localhost:").is_some_and(valid_port)
        || authority.strip_prefix("[::1]:").is_some_and(valid_port);
    host_allowed
        && !path.is_empty()
        && path.ends_with('/')
        && !path.split('/').any(|segment| segment == "..")
        && path.bytes().all(|byte| byte.is_ascii_graphic())
}

fn launch_url_matches_entrypoint(launch_url: &str, entrypoint: &str) -> bool {
    let Some(rest) = launch_url.strip_prefix("http://") else {
        return false;
    };
    let Some((_, path)) = rest.split_once('/') else {
        return false;
    };
    entrypoint
        .strip_prefix('/')
        .is_some_and(|entrypoint| path == entrypoint)
}

fn valid_port(value: &str) -> bool {
    value
        .parse::<u16>()
        .is_ok_and(|port| port != 0 && !value.starts_with('0'))
}

fn random_suffix() -> Result<String, ClientBindingStoreErrorV1> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| ClientBindingStoreErrorV1::Unavailable)?;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(output)
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}
