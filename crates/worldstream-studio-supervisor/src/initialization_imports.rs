//! Bounded explicit prerequisite review, distinct from Room setup and startup.

use crate::{
    agent_profiles::{AgentHostContractV1, AgentProfileStoreV1, ManagedReferenceProviderV1},
    client_bindings::{ClientBindingStoreV1, ClientImportReviewV1},
    initialization_inputs::{
        AgentProfilePublishInputV2, ProviderCredentialImportV1, parse_agent_profile,
        parse_client_declaration, parse_provider_declaration, parse_runner_template,
    },
    local_initialization::{
        ImportInstallation, InitializationRequest, import_installation, lock_import_installation,
        validate_import_authority,
    },
    model_provider_credentials::{ModelProviderCredentialRegistryV1, RetainedProviderDependencyV1},
    runner_templates::{RunnerExecutableV1, RunnerTemplateManifestV1, RunnerTemplateRegistryV1},
    secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
};
use serde::Serialize;
use std::{collections::BTreeMap, fs, io::Read as _, path::PathBuf};
use thiserror::Error;

/// Captured explicit files. No discovery, implicit approval, or process startup.
pub struct InitializationImportRequest {
    pub installation: InitializationRequest,
    pub runner_templates: Vec<PathBuf>,
    pub provider_declarations: Vec<PathBuf>,
    pub agent_profiles: Vec<PathBuf>,
    pub client_declarations: Vec<PathBuf>,
    pub approval: Option<String>,
}

/// Closed failures never carry input contents or private vault references.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("initialize the protected installation before reviewing imports")]
    InitializationRequired,
    #[error("initialization import declarations are invalid or unavailable")]
    Invalid,
    #[error("exact reviewed import approval is required")]
    ApprovalRequired,
    #[error("import publication may be incomplete; inspect and retry the exact reviewed inputs")]
    PublicationUncertain,
}

/// Public immutable named profile identity; not a private database key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportedProfileIdentityV1 {
    pub profile_id: String,
    pub revision: String,
}

/// Reviewed existing execution dependency, excluding retained private vault refs.
#[derive(Clone, Debug, Serialize)]
pub struct RunnerDependencyReviewV1 {
    pub template_id: String,
    pub revision: String,
    pub executable: RunnerExecutableV1,
}

/// Named retained provider metadata, excluding its private vault reference.
#[derive(Clone, Debug, Serialize)]
pub struct ProviderDependencyReviewV1 {
    pub credential_id: String,
    pub display_name: String,
    pub provider: ManagedReferenceProviderV1,
    pub kind: SecretKindV1,
}

/// Non-secret review output. Its digest grants nothing until explicitly supplied.
#[derive(Debug, Serialize)]
pub struct ImportReviewV1 {
    pub schema: String,
    pub digest: String,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    pub agent_profiles: Vec<ImportedProfileIdentityV1>,
    pub runner_templates: Vec<RunnerTemplateManifestV1>,
    pub provider_credentials: Vec<ProviderCredentialImportV1>,
    pub runner_dependencies: Vec<RunnerDependencyReviewV1>,
    pub provider_dependencies: Vec<ProviderDependencyReviewV1>,
    pub client_declarations: Vec<ClientImportReviewV1>,
    pub services_started: bool,
}

/// Exact named publication result; no service or participant authority is created.
#[derive(Debug, Serialize)]
pub struct ImportApplyV1 {
    pub schema: String,
    pub digest: String,
    pub config_path: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    pub created_agent_profiles: Vec<ImportedProfileIdentityV1>,
    pub reused_agent_profiles: Vec<ImportedProfileIdentityV1>,
    pub created_runner_templates: Vec<RunnerTemplateManifestV1>,
    pub reused_runner_templates: Vec<RunnerTemplateManifestV1>,
    pub created_provider_credentials: Vec<String>,
    pub reused_provider_credentials: Vec<String>,
    pub created_client_declarations: Vec<PathBuf>,
    pub reused_client_declarations: Vec<PathBuf>,
    pub services_started: bool,
}

struct PreparedImports {
    review: ImportReviewV1,
    profiles: Vec<AgentProfilePublishInputV2>,
    runners: Vec<RunnerTemplateManifestV1>,
    providers: Vec<ProviderCredentialImportV1>,
    clients: Vec<ClientImportReviewV1>,
    client_paths: Vec<PathBuf>,
}

#[derive(Serialize)]
struct ReviewedDocument {
    kind: &'static str,
    path: PathBuf,
    byte_length: usize,
    blake3: String,
}

#[derive(Serialize)]
struct ReviewBinding {
    schema: &'static str,
    installation: ImportInstallation,
    documents: Vec<ReviewedDocument>,
    runners: Vec<RunnerTemplateManifestV1>,
    providers: Vec<ProviderCredentialImportV1>,
    runner_dependencies: Vec<RunnerTemplateManifestV1>,
    provider_dependencies: Vec<RetainedProviderDependencyV1>,
    clients: Vec<ClientImportReviewV1>,
}

/// Reviews exactly selected files without mutations or reading secret material.
///
/// # Errors
/// Requires an initialized installation and valid bounded exact declarations.
pub fn preview_imports(
    request: &InitializationImportRequest,
) -> Result<ImportReviewV1, ImportError> {
    prepare(request).map(|prepared| prepared.review)
}

/// Rechecks exact approval and authority before immutable local publication.
///
/// # Errors
/// Rejects missing/stale approval or invalid inputs. Publication errors may
/// leave a subset installed; retry the same reviewed inputs, never infer rollback.
#[allow(
    clippy::too_many_lines,
    reason = "Keep ordered preflight and publication phases visible at the reviewed aggregate boundary"
)]
pub fn apply_imports(request: &InitializationImportRequest) -> Result<ImportApplyV1, ImportError> {
    let approval = request
        .approval
        .as_ref()
        .ok_or(ImportError::ApprovalRequired)?;
    if &prepare(request)?.review.digest != approval {
        return Err(ImportError::ApprovalRequired);
    }
    let _lock = lock_import_installation(&request.installation)
        .map_err(|_| ImportError::InitializationRequired)?;
    let prepared = prepare(request)?;
    if &prepared.review.digest != approval {
        return Err(ImportError::ApprovalRequired);
    }
    validate_import_authority(&request.installation)
        .map_err(|_| ImportError::InitializationRequired)?;
    let runner_root = prepared.review.state_dir.join("runner-templates/installed");
    RunnerTemplateRegistryV1::check_imports(&runner_root, &prepared.runners)
        .map_err(|_| ImportError::Invalid)?;
    let root = prepared.review.state_dir.join("agent-profiles");
    let vault = FileSecretVaultV1::open_existing(&prepared.review.state_dir.join("secrets"))
        .map_err(|_| ImportError::InitializationRequired)?;
    let provider_root = prepared
        .review
        .state_dir
        .join("model-provider-credentials/installed");
    let reused = prepared
        .profiles
        .iter()
        .map(|profile| {
            let reference =
                profile_provider_reference(profile, &provider_root, &prepared.providers, &vault)?;
            AgentProfileStoreV1::check_named_import(&root, profile, reference.as_ref())
                .map_err(|_| ImportError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut provider_material = Vec::new();
    for provider in &prepared.providers {
        let material = zeroize::Zeroizing::new(
            worldstream_runtime::SecretSource::File(provider.secret_file.clone())
                .read_bounded(64 * 1024)
                .map_err(|_| ImportError::Invalid)?,
        );
        if material.is_empty() {
            return Err(ImportError::Invalid);
        }
        if let Some(reference) =
            ModelProviderCredentialRegistryV1::check_import(&provider_root, provider, &vault)
                .map_err(|_| ImportError::Invalid)?
        {
            let existing = vault
                .resolve(SecretKindV1::ModelProvider, &reference)
                .map_err(|_| ImportError::Invalid)?;
            if existing.as_bytes() != material.as_slice() {
                return Err(ImportError::Invalid);
            }
        }
        provider_material.push(material);
    }
    let mut created_runner_templates = Vec::new();
    let mut reused_runner_templates = Vec::new();
    for runner in &prepared.runners {
        if RunnerTemplateRegistryV1::import_exact(&runner_root, runner)
            .map_err(|_| ImportError::PublicationUncertain)?
        {
            reused_runner_templates.push(runner.clone());
        } else {
            created_runner_templates.push(runner.clone());
        }
    }
    let mut created_provider_credentials = Vec::new();
    let mut reused_provider_credentials = Vec::new();
    for (provider, material) in prepared.providers.iter().zip(provider_material) {
        if ModelProviderCredentialRegistryV1::import_exact(
            &provider_root,
            provider,
            &material,
            &vault,
        )
        .map_err(|_| ImportError::PublicationUncertain)?
        {
            reused_provider_credentials.push(provider.credential_id.clone());
        } else {
            created_provider_credentials.push(provider.credential_id.clone());
        }
    }
    let mut created_agent_profiles = Vec::new();
    let mut reused_agent_profiles = Vec::new();
    if !prepared.profiles.is_empty() {
        let store = AgentProfileStoreV1::open(&root, vault.clone())
            .map_err(|_| ImportError::PublicationUncertain)?;
        for (profile, reused) in prepared.profiles.iter().zip(reused) {
            let reference = profile_provider_reference(profile, &provider_root, &[], &vault)
                .map_err(|_| ImportError::PublicationUncertain)?;
            let resolved = AgentProfileStoreV1::resolve_named_import(profile, reference)
                .map_err(|_| ImportError::PublicationUncertain)?;
            store
                .publish(&resolved)
                .map_err(|_| ImportError::PublicationUncertain)?;
            let identity = ImportedProfileIdentityV1 {
                profile_id: profile.profile_id.clone(),
                revision: profile.revision.clone(),
            };
            if reused {
                reused_agent_profiles.push(identity);
            } else {
                created_agent_profiles.push(identity);
            }
        }
    }
    let client_reused = if prepared.clients.is_empty() {
        Vec::new()
    } else {
        ClientBindingStoreV1::import_reviewed(
            &prepared.review.state_dir.join("client-bindings"),
            &prepared.clients,
        )
        .map_err(|_| ImportError::PublicationUncertain)?
    };
    let mut created_client_declarations = Vec::new();
    let mut reused_client_declarations = Vec::new();
    for (path, reused) in prepared.client_paths.into_iter().zip(client_reused) {
        if reused {
            reused_client_declarations.push(path);
        } else {
            created_client_declarations.push(path);
        }
    }
    Ok(ImportApplyV1 {
        schema: "worldstream/initialization-import-apply/v1".to_owned(),
        digest: prepared.review.digest,
        config_path: prepared.review.config_path,
        state_dir: prepared.review.state_dir,
        data_dir: prepared.review.data_dir,
        created_agent_profiles,
        reused_agent_profiles,
        created_runner_templates,
        reused_runner_templates,
        created_provider_credentials,
        reused_provider_credentials,
        created_client_declarations,
        reused_client_declarations,
        services_started: false,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "Keep explicit per-kind review and dependency ordering together without changing the approval preimage"
)]
fn prepare(request: &InitializationImportRequest) -> Result<PreparedImports, ImportError> {
    let installation = import_installation(&request.installation)
        .map_err(|_| ImportError::InitializationRequired)?;
    if (request.agent_profiles.is_empty()
        && request.runner_templates.is_empty()
        && request.provider_declarations.is_empty()
        && request.client_declarations.is_empty())
        || request.agent_profiles.len() > 16
        || request.runner_templates.len() > 16
        || request.provider_declarations.len() > 16
        || request.client_declarations.len() > 16
    {
        return Err(ImportError::Invalid);
    }
    let mut documents = BTreeMap::new();
    let mut profiles = BTreeMap::new();
    let mut revisions = BTreeMap::new();
    let mut runner_dependencies = BTreeMap::new();
    let mut provider_dependencies = BTreeMap::new();
    let mut runners = BTreeMap::new();
    let mut providers = BTreeMap::new();
    let vault = FileSecretVaultV1::open_existing(&installation.state_dir.join("secrets"))
        .map_err(|_| ImportError::InitializationRequired)?;
    for selected in &request.provider_declarations {
        let (path, bytes) = read_declaration(
            request.installation.working_directory.join(selected),
            64 * 1024,
        )?;
        let mut provider = parse_provider_declaration(&bytes).map_err(|_| ImportError::Invalid)?;
        let secret_path = path
            .parent()
            .ok_or(ImportError::Invalid)?
            .join(&provider.secret_file);
        worldstream_runtime::validate_owner_only_file(&secret_path)
            .map_err(|_| ImportError::Invalid)?;
        let length = fs::metadata(&secret_path)
            .map_err(|_| ImportError::Invalid)?
            .len();
        if length == 0 || length > 64 * 1024 {
            return Err(ImportError::Invalid);
        }
        provider.secret_file = fs::canonicalize(secret_path).map_err(|_| ImportError::Invalid)?;
        ModelProviderCredentialRegistryV1::check_import(
            &installation
                .state_dir
                .join("model-provider-credentials/installed"),
            &provider,
            &vault,
        )
        .map_err(|_| ImportError::Invalid)?;
        if providers
            .insert(provider.credential_id.clone(), provider)
            .is_some()
            || documents
                .insert(
                    path.clone(),
                    ReviewedDocument {
                        kind: "provider_declaration",
                        path,
                        byte_length: bytes.len(),
                        blake3: blake3::hash(&bytes).to_hex().to_string(),
                    },
                )
                .is_some()
        {
            return Err(ImportError::Invalid);
        }
    }
    let providers: Vec<_> = providers.into_values().collect();
    if !providers.is_empty() {
        ModelProviderCredentialRegistryV1::check_import_capacity(
            &installation
                .state_dir
                .join("model-provider-credentials/installed"),
            &providers,
        )
        .map_err(|_| ImportError::Invalid)?;
    }
    for selected in &request.runner_templates {
        let (path, bytes) = read_declaration(
            request.installation.working_directory.join(selected),
            64 * 1024,
        )?;
        let mut runner = parse_runner_template(&bytes).map_err(|_| ImportError::Invalid)?;
        runner.executable.path = fs::canonicalize(
            path.parent()
                .ok_or(ImportError::Invalid)?
                .join(&runner.executable.path),
        )
        .map_err(|_| ImportError::Invalid)?;
        if runners
            .insert(
                (runner.template_id.clone(), runner.revision.clone()),
                runner,
            )
            .is_some()
            || documents
                .insert(
                    path.clone(),
                    ReviewedDocument {
                        kind: "runner_template",
                        path,
                        byte_length: bytes.len(),
                        blake3: blake3::hash(&bytes).to_hex().to_string(),
                    },
                )
                .is_some()
        {
            return Err(ImportError::Invalid);
        }
    }
    let runners: Vec<_> = runners.into_values().collect();
    RunnerTemplateRegistryV1::check_imports(
        &installation.state_dir.join("runner-templates/installed"),
        &runners,
    )
    .map_err(|_| ImportError::Invalid)?;
    for selected in &request.agent_profiles {
        let (path, bytes) = read_declaration(
            request.installation.working_directory.join(selected),
            64 * 1024,
        )?;
        let profile = parse_agent_profile(&bytes).map_err(|_| ImportError::Invalid)?;
        let managed_dependency = match &profile.host_contract {
            AgentHostContractV1::GenericMcp => None,
            AgentHostContractV1::ManagedReference {
                runner_template,
                provider,
                ..
            } => Some((runner_template, *provider)),
            AgentHostContractV1::ManagedHouseOpenrouter {
                runner_template, ..
            } => Some((runner_template, ManagedReferenceProviderV1::Openrouter)),
        };
        if let Some((runner_template, provider)) = managed_dependency {
            let key = (
                runner_template.template_id.clone(),
                runner_template.revision.clone(),
            );
            let dependency = if let Some(runner) = runners
                .iter()
                .find(|runner| runner.template_id == key.0 && runner.revision == key.1)
            {
                runner.clone()
            } else {
                RunnerTemplateRegistryV1::review_installed(
                    &installation.state_dir.join("runner-templates/installed"),
                    &key.0,
                    &key.1,
                )
                .map_err(|_| ImportError::Invalid)?
            };
            runner_dependencies.insert(key, dependency);
            let credential_id = profile
                .managed_provider_credential_id
                .as_ref()
                .ok_or(ImportError::Invalid)?;
            // Same-batch provider declarations already bind their explicit source
            // selection and must remain retry-stable before/after publication.
            // Unselected dependencies instead bind the exact retained authority.
            if !providers
                .iter()
                .any(|selected| &selected.credential_id == credential_id)
            {
                let dependency = ModelProviderCredentialRegistryV1::review_import_dependency(
                    &installation
                        .state_dir
                        .join("model-provider-credentials/installed"),
                    credential_id,
                    provider,
                    &vault,
                )
                .map_err(|_| ImportError::Invalid)?;
                if provider_dependencies
                    .get(credential_id)
                    .is_some_and(|existing| existing != &dependency)
                {
                    return Err(ImportError::Invalid);
                }
                provider_dependencies.insert(credential_id.clone(), dependency);
            }
        }
        let identity = ImportedProfileIdentityV1 {
            profile_id: profile.profile_id.clone(),
            revision: profile.revision.clone(),
        };
        let reference = profile_provider_reference(
            &profile,
            &installation
                .state_dir
                .join("model-provider-credentials/installed"),
            &providers,
            &vault,
        )?;
        AgentProfileStoreV1::check_named_import(
            &installation.state_dir.join("agent-profiles"),
            &profile,
            reference.as_ref(),
        )
        .map_err(|_| ImportError::Invalid)?;
        revisions.insert(
            (identity.profile_id.clone(), identity.revision.clone()),
            profile,
        );
        if profiles
            .insert(
                (identity.profile_id.clone(), identity.revision.clone()),
                identity,
            )
            .is_some()
            || documents
                .insert(
                    path.clone(),
                    ReviewedDocument {
                        kind: "agent_profile",
                        path,
                        byte_length: bytes.len(),
                        blake3: blake3::hash(&bytes).to_hex().to_string(),
                    },
                )
                .is_some()
        {
            return Err(ImportError::Invalid);
        }
    }
    let mut clients = BTreeMap::new();
    for selected in &request.client_declarations {
        let (path, bytes) = read_client_document(
            request.installation.working_directory.join(selected),
            "client_declaration",
            &mut documents,
        )?;
        let declaration = parse_client_declaration(&bytes).map_err(|_| ImportError::Invalid)?;
        let parent = path.parent().ok_or(ImportError::Invalid)?;
        let mut releases = BTreeMap::new();
        for release_file in declaration.release_files {
            let (release_path, release) =
                read_client_document(parent.join(release_file), "client_release", &mut documents)?;
            if releases.insert(release_path, release).is_some() {
                return Err(ImportError::Invalid);
            }
        }
        let (_, bootstrap) = read_client_document(
            parent.join(declaration.bindings_file),
            "client_bindings",
            &mut documents,
        )?;
        let review = ClientBindingStoreV1::read_import(
            &releases.into_values().collect::<Vec<_>>(),
            &bootstrap,
        )
        .map_err(|_| ImportError::Invalid)?;
        if clients.insert(path, review).is_some() {
            return Err(ImportError::Invalid);
        }
    }
    let (client_paths, clients): (Vec<_>, Vec<_>) = clients.into_iter().unzip();
    if !clients.is_empty() {
        ClientBindingStoreV1::check_imports(
            &installation.state_dir.join("client-bindings"),
            &clients,
        )
        .map_err(|_| ImportError::Invalid)?;
    }
    let binding = ReviewBinding {
        schema: "worldstream/initialization-import-binding/v1",
        installation,
        documents: documents.into_values().collect(),
        runners: runners.clone(),
        providers: providers.clone(),
        runner_dependencies: runner_dependencies.into_values().collect(),
        provider_dependencies: provider_dependencies.into_values().collect(),
        clients: clients.clone(),
    };
    let bytes = serde_json::to_vec(&binding).map_err(|_| ImportError::Invalid)?;
    let mut digest = blake3::Hasher::new_derive_key("worldstream/initialization-import-review/v1");
    digest.update(&bytes);
    let review = ImportReviewV1 {
        schema: "worldstream/initialization-import-review/v1".to_owned(),
        digest: format!("blake3:{}", digest.finalize().to_hex()),
        config_path: binding.installation.config_path,
        state_dir: binding.installation.state_dir,
        data_dir: binding.installation.data_dir,
        agent_profiles: profiles.into_values().collect(),
        runner_templates: runners.clone(),
        provider_credentials: providers.clone(),
        client_declarations: clients.clone(),
        runner_dependencies: binding
            .runner_dependencies
            .iter()
            .map(|runner| RunnerDependencyReviewV1 {
                template_id: runner.template_id.clone(),
                revision: runner.revision.clone(),
                executable: runner.executable.clone(),
            })
            .collect(),
        provider_dependencies: binding
            .provider_dependencies
            .iter()
            .map(|provider| ProviderDependencyReviewV1 {
                credential_id: provider.credential_id.clone(),
                display_name: provider.display_name.clone(),
                provider: provider.provider,
                kind: provider.kind,
            })
            .collect(),
        services_started: false,
    };
    Ok(PreparedImports {
        review,
        profiles: revisions.into_values().collect(),
        runners,
        providers,
        clients,
        client_paths,
    })
}

fn read_declaration(path: PathBuf, max_bytes: usize) -> Result<(PathBuf, Vec<u8>), ImportError> {
    let metadata = fs::symlink_metadata(&path).map_err(|_| ImportError::Invalid)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ImportError::Invalid);
    }
    let path = fs::canonicalize(path).map_err(|_| ImportError::Invalid)?;
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .map_err(|_| ImportError::Invalid)?
        .take(u64::try_from(max_bytes).map_err(|_| ImportError::Invalid)? + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImportError::Invalid)?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(ImportError::Invalid);
    }
    Ok((path, bytes))
}

fn read_client_document(
    path: PathBuf,
    kind: &'static str,
    documents: &mut BTreeMap<PathBuf, ReviewedDocument>,
) -> Result<(PathBuf, Vec<u8>), ImportError> {
    let (path, bytes) = read_declaration(path, 256 * 1024)?;
    let digest = blake3::hash(&bytes).to_hex().to_string();
    if let Some(existing) = documents.get(&path) {
        if existing.kind != kind || existing.blake3 != digest {
            return Err(ImportError::Invalid);
        }
    } else {
        let total = documents
            .values()
            .try_fold(bytes.len(), |total, document| {
                total.checked_add(document.byte_length)
            })
            .ok_or(ImportError::Invalid)?;
        if total > 16 * 1024 * 1024 {
            return Err(ImportError::Invalid);
        }
        documents.insert(
            path.clone(),
            ReviewedDocument {
                kind,
                path: path.clone(),
                byte_length: bytes.len(),
                blake3: digest,
            },
        );
    }
    Ok((path, bytes))
}

fn profile_provider_reference(
    profile: &AgentProfilePublishInputV2,
    root: &std::path::Path,
    selected: &[ProviderCredentialImportV1],
    vault: &FileSecretVaultV1,
) -> Result<Option<SecretReferenceV1>, ImportError> {
    match (
        &profile.host_contract,
        &profile.managed_provider_credential_id,
    ) {
        (AgentHostContractV1::GenericMcp, None) => Ok(None),
        (AgentHostContractV1::ManagedReference { provider, .. }, Some(id)) => {
            if let Some(declaration) = selected
                .iter()
                .find(|declaration| &declaration.credential_id == id)
            {
                if declaration.provider != *provider {
                    return Err(ImportError::Invalid);
                }
                ModelProviderCredentialRegistryV1::check_import(root, declaration, vault)
                    .map_err(|_| ImportError::Invalid)
            } else {
                ModelProviderCredentialRegistryV1::resolve_import_reference(
                    root, id, *provider, vault,
                )
                .map(Some)
                .map_err(|_| ImportError::Invalid)
            }
        }
        (AgentHostContractV1::ManagedHouseOpenrouter { .. }, Some(id)) => {
            if let Some(declaration) = selected
                .iter()
                .find(|declaration| &declaration.credential_id == id)
            {
                if declaration.provider != ManagedReferenceProviderV1::Openrouter {
                    return Err(ImportError::Invalid);
                }
                ModelProviderCredentialRegistryV1::check_import(root, declaration, vault)
                    .map_err(|_| ImportError::Invalid)
            } else {
                ModelProviderCredentialRegistryV1::resolve_import_reference(
                    root,
                    id,
                    ManagedReferenceProviderV1::Openrouter,
                    vault,
                )
                .map(Some)
                .map_err(|_| ImportError::Invalid)
            }
        }
        _ => Err(ImportError::Invalid),
    }
}
