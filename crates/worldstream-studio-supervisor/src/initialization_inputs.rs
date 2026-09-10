//! Pure, bounded declarations for explicit operator initialization imports.
//!
//! Parsing grants no approval and performs no filesystem, vault, or process I/O.
//! The importer resolves relative paths against the declaring file and checks
//! protected files, exact identities, and compatibility before installing.

use crate::{
    agent_profiles::{AgentHostContractV1, ManagedReferenceProviderV1},
    runner_templates::RunnerTemplateManifestV1,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use std::{collections::BTreeMap, net::SocketAddr};
use thiserror::Error;

const SMALL_DOCUMENT_BYTES: usize = 64 * 1024;
// Explicit imports retain older exact Releases alongside current clients.
const MAX_CLIENT_RELEASE_FILES: usize = 64;

/// Closed error: never contains declaration contents, paths, or secret values.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("initialization declaration is invalid")]
pub struct InitializationInputError;

/// Explicit source-file selection; this is not the retained vault-reference record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredentialImportV1 {
    pub schema: String,
    pub credential_id: String,
    pub display_name: String,
    pub provider: ManagedReferenceProviderV1,
    pub secret_file: PathBuf,
}

/// Named publication input; retained Profiles instead contain private vault references.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProfilePublishInputV2 {
    pub schema: String,
    pub profile_id: String,
    pub revision: String,
    pub display_name: String,
    pub non_secret_configuration: BTreeMap<String, String>,
    #[serde(deserialize_with = "strict_host_contract")]
    pub host_contract: AgentHostContractV1,
    pub managed_provider_credential_id: Option<String>,
}

/// Explicit transitive file inventory; no implicit directory scanning is allowed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDeclarationImportV1 {
    pub schema: String,
    pub release_files: Vec<PathBuf>,
    pub bindings_file: PathBuf,
}

// An empty struct variant, unlike a tagged unit variant, rejects extra fields.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HostContractInput {
    GenericMcp {},
    ManagedReference {
        host_contract_revision: String,
        runner_template: crate::room_drafts::RunnerTemplateRevisionReferenceV1,
        provider: ManagedReferenceProviderV1,
        provider_address: SocketAddr,
        model_id: String,
    },
    ManagedHouseOpenrouter {
        host_contract_revision: String,
        runner_template: crate::room_drafts::RunnerTemplateRevisionReferenceV1,
    },
}

fn strict_host_contract<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<AgentHostContractV1, D::Error> {
    Ok(match HostContractInput::deserialize(deserializer)? {
        HostContractInput::GenericMcp {} => AgentHostContractV1::GenericMcp,
        HostContractInput::ManagedReference {
            host_contract_revision,
            runner_template,
            provider,
            provider_address,
            model_id,
        } => AgentHostContractV1::ManagedReference {
            host_contract_revision,
            runner_template,
            provider,
            provider_address,
            model_id,
        },
        HostContractInput::ManagedHouseOpenrouter {
            host_contract_revision,
            runner_template,
        } => AgentHostContractV1::ManagedHouseOpenrouter {
            host_contract_revision,
            runner_template,
        },
    })
}

// Legacy Runner DTOs accept unknown fields. These small wire shapes deliberately
// make initialization stricter without changing any existing route or wire DTO.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerInput {
    schema: String,
    template_id: String,
    revision: String,
    display_name: String,
    executable: ExecutableInput,
    compatibility: Vec<CompatibilityInput>,
    capacity: CapacityInput,
    health: HealthInput,
    non_secret_environment: BTreeMap<String, String>,
    secret_environment: Vec<()>,
    instances: Vec<InstanceInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutableInput {
    path: PathBuf,
    blake3: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityInput {
    activity_pack_id: String,
    exact_revisions: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapacityInput {
    maximum_concurrent_invocations: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthInput {
    path: String,
    timeout_ms: u64,
    stale_after_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstanceInput {
    instance_id: String,
    health_address: SocketAddr,
}

/// Checks closed Runner syntax while leaving executable admission to the importer.
///
/// # Errors
/// Rejects unknown fields, secret references, malformed values, or exceeded bounds.
pub fn parse_runner_template(
    bytes: &[u8],
) -> Result<RunnerTemplateManifestV1, InitializationInputError> {
    let input: RunnerInput = decode(bytes, SMALL_DOCUMENT_BYTES)?;
    require(
        input.schema == "worldstream/runner-template/v1"
            && local_id(&input.template_id)
            && revision(&input.revision)
            && text(&input.display_name, 256)
            && local_path(&input.executable.path)
            && input.executable.blake3.len() == 64
            && input
                .executable
                .blake3
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && !input.compatibility.is_empty()
            && input.compatibility.iter().all(|rule| {
                text(&rule.activity_pack_id, 64)
                    && rule.activity_pack_id.split('.').all(local_id)
                    && !rule.exact_revisions.is_empty()
                    && rule.exact_revisions.iter().all(|value| revision(value))
            })
            && (1..=10_000).contains(&input.capacity.maximum_concurrent_invocations)
            && input.health.path.starts_with('/')
            && text(&input.health.path, 128)
            && !input.health.path.contains([' ', '?', '#'])
            && (10..=10_000).contains(&input.health.timeout_ms)
            && input.health.stale_after_ms >= input.health.timeout_ms
            && input.health.stale_after_ms <= 300_000
            && input
                .non_secret_environment
                .iter()
                .all(|(key, value)| environment_key(key) && non_secret(key, value))
            && input.secret_environment.is_empty()
            && !input.instances.is_empty()
            && input.instances.len() <= 128
            && input.instances.iter().all(|instance| {
                local_id(&instance.instance_id)
                    && instance.health_address.ip().is_loopback()
                    && instance.health_address.port() != 0
            }),
    )?;
    // Return the existing typed contract, not a parallel installation model.
    decode(bytes, SMALL_DOCUMENT_BYTES)
}

/// Parses the existing named Profile publication shape without resolving credentials.
///
/// # Errors
/// Rejects unknown fields, inline credentials, malformed values, or wrong schemas.
pub fn parse_agent_profile(
    bytes: &[u8],
) -> Result<AgentProfilePublishInputV2, InitializationInputError> {
    let input: AgentProfilePublishInputV2 = decode(bytes, SMALL_DOCUMENT_BYTES)?;
    require(
        input.schema == "worldstream/studio-agent-profile-publish/v2"
            && identifier(&input.profile_id)
            && revision(&input.revision)
            && text(&input.display_name, 256)
            && input.non_secret_configuration.len() <= 64
            && input
                .non_secret_configuration
                .iter()
                .all(|(key, value)| configuration_key(key) && non_secret(key, value)),
    )?;
    match (&input.host_contract, &input.managed_provider_credential_id) {
        (AgentHostContractV1::GenericMcp, None) => {}
        (
            AgentHostContractV1::ManagedReference {
                host_contract_revision,
                runner_template,
                provider_address,
                model_id,
                ..
            },
            Some(id),
        ) => {
            require(
                identifier(id)
                    && revision(host_contract_revision)
                    && identifier(&runner_template.template_id)
                    && revision(&runner_template.revision)
                    && provider_address.ip().is_loopback()
                    && provider_address.port() != 0
                    && text(model_id, 256),
            )?;
        }
        (
            AgentHostContractV1::ManagedHouseOpenrouter {
                host_contract_revision,
                runner_template,
            },
            Some(id),
        ) => {
            require(
                identifier(id)
                    && revision(host_contract_revision)
                    && identifier(&runner_template.template_id)
                    && revision(&runner_template.revision),
            )?;
        }
        _ => return Err(InitializationInputError),
    }
    Ok(input)
}

/// Parses explicit Release/bootstrap file references without opening any file.
///
/// # Errors
/// Rejects wrong schemas, unbounded paths, unknown fields, or more than 64 Releases.
pub fn parse_client_declaration(
    bytes: &[u8],
) -> Result<ClientDeclarationImportV1, InitializationInputError> {
    let input: ClientDeclarationImportV1 = decode(bytes, 256 * 1024)?;
    require(
        input.schema == "worldstream/client-declaration-import/v1"
            && !input.release_files.is_empty()
            && input.release_files.len() <= MAX_CLIENT_RELEASE_FILES
            && input.release_files.iter().all(|path| local_path(path))
            && local_path(&input.bindings_file),
    )?;
    Ok(input)
}

/// Parses a named provider declaration without opening its selected secret file.
///
/// # Errors
/// Rejects oversized, malformed, wrong-schema, or unbounded input.
pub fn parse_provider_declaration(
    bytes: &[u8],
) -> Result<ProviderCredentialImportV1, InitializationInputError> {
    let input: ProviderCredentialImportV1 = decode(bytes, SMALL_DOCUMENT_BYTES)?;
    require(
        input.schema == "worldstream/model-provider-credential-import/v1"
            && identifier(&input.credential_id)
            && text(&input.display_name, 256)
            && local_path(&input.secret_file),
    )?;
    Ok(input)
}

fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    maximum: usize,
) -> Result<T, InitializationInputError> {
    require(!bytes.is_empty() && bytes.len() <= maximum)?;
    serde_json::from_slice(bytes).map_err(|_| InitializationInputError)
}

fn require(valid: bool) -> Result<(), InitializationInputError> {
    valid.then_some(()).ok_or(InitializationInputError)
}

fn text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn local_path(path: &Path) -> bool {
    path.to_str().is_some_and(|value| text(value, 4096))
}

fn identifier(value: &str) -> bool {
    text(value, 64)
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn local_id(value: &str) -> bool {
    text(value, 64)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn revision(value: &str) -> bool {
    text(value, 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn environment_key(value: &str) -> bool {
    text(value, 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn configuration_key(value: &str) -> bool {
    text(value, 64)
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
}

fn non_secret(key: &str, value: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let value_lower = value.trim().to_ascii_lowercase();
    text(value, 4096)
        && !["secret", "token", "password", "key", "credential"]
            .iter()
            .any(|part| key.contains(part))
        && ![
            "bearer ",
            "bearer=",
            "basic ",
            "sk-",
            "sk_",
            "api_key=",
            "apikey=",
            "token=",
            "secret=",
            "password=",
            "credential=",
        ]
        .iter()
        .any(|prefix| value_lower.starts_with(prefix))
}
