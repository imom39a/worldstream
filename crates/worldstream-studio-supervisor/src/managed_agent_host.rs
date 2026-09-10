//! Post-MVP reference managed Agent Host.
//!
//! The Supervisor launches a fixed assignment-MCP helper and a separate
//! owner-approved model host, then bridges only their stdio. The model host is
//! never given Supervisor storage, launch references, or participant/Runner
//! authority.

use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{Read as _, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{RwLock, RwLockWriteGuard};
use thiserror::Error;
use worldstream_hosted_contract::HouseAgentRevision;
use worldstream_protocol::UlidString;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::{
    agent_profiles::{
        AgentExecutionBindingV1, AgentHostContractV1, AgentProfileStoreV1,
        ManagedReferenceProviderV1,
    },
    assignment_mcp::{AssignedMembershipLaunchSourceV1, AssignmentMcpLaunchRegistryV1},
    managed_activation_status::{ManagedActivationStatusErrorV1, ManagedActivationStatusStoreV1},
    runner_templates::RunnerSupervisorV1,
    secrets::{FileSecretVaultV1, SecretKindV1},
    task_setup::{
        ManagedReferenceHostReadinessSourceV1, TaskSeatReadinessReasonV1, TaskSetupStateV1,
        TaskSetupSupervisorV1,
    },
};

const PROFILE_SCHEMA: &str = "worldstream/managed-agent-host-profile/v1";
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const MAX_TEXT_BYTES: usize = 256;

/// Exact private host/Profile revision retained owner-only.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAgentHostProfileV1 {
    schema: String,
    assignment_id: String,
    host_id: String,
    host_revision: String,
    provider: String,
    provider_address: SocketAddr,
    model: String,
    credential_reference: String,
    capacity: u32,
    stale_after_ms: u64,
}

impl ManagedAgentHostProfileV1 {
    /// Constructs one exact immutable host/Profile revision.
    ///
    /// # Errors
    ///
    /// Rejects malformed identities, labels, capacity, or freshness.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        assignment_id: &str,
        host_id: &str,
        host_revision: &str,
        provider: &str,
        provider_address: SocketAddr,
        model: &str,
        credential_reference: &str,
        capacity: u32,
        stale_after: Duration,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        let profile = Self {
            schema: PROFILE_SCHEMA.to_owned(),
            assignment_id: assignment_id.to_owned(),
            host_id: host_id.to_owned(),
            host_revision: host_revision.to_owned(),
            provider: provider.to_owned(),
            provider_address,
            model: model.to_owned(),
            credential_reference: credential_reference.to_owned(),
            capacity,
            stale_after_ms: u64::try_from(stale_after.as_millis()).unwrap_or(u64::MAX),
        };
        validate_profile(&profile)?;
        Ok(profile)
    }
}

impl fmt::Debug for ManagedAgentHostProfileV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedAgentHostProfileV1")
            .field("schema", &self.schema)
            .field("assignment_id", &self.assignment_id)
            .field("host_id", &self.host_id)
            .field("host_revision", &self.host_revision)
            .field("provider", &self.provider)
            .field("provider_address", &self.provider_address)
            .field("model", &self.model)
            .field("credential_reference", &"[REDACTED]")
            .field("capacity", &self.capacity)
            .field("stale_after_ms", &self.stale_after_ms)
            .finish()
    }
}

/// Browser/log-safe Operations projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAgentHostStatusV1 {
    pub schema: String,
    pub assignment_id: String,
    pub host_id: String,
    pub host_revision: String,
    pub state: ManagedAgentHostStateV1,
    pub ready: bool,
    pub capacity: u32,
    pub active_invocations: u32,
    pub freshness: ManagedAgentHostFreshnessV1,
    #[serde(default)]
    pub activation: ManagedAgentActivationStatusV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ManagedAgentHostFailureV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAgentActivationStatusV1 {
    pub state: ManagedAgentActivationStateV1,
    pub last_confirmed_disposition: Option<ManagedAgentActivationDispositionV1>,
}

impl Default for ManagedAgentActivationStatusV1 {
    fn default() -> Self {
        Self {
            state: ManagedAgentActivationStateV1::Unavailable,
            last_confirmed_disposition: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAgentActivationStateV1 {
    Idle,
    Waiting,
    Leased,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAgentActivationDispositionV1 {
    Handled,
    Declined,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAgentHostStateV1 {
    Stopped,
    Starting,
    Running,
    NeedsAttention,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAgentHostFreshnessV1 {
    Fresh,
    Stale,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAgentHostFailureV1 {
    pub code: String,
    pub message: String,
    pub safe_action: String,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ManagedAgentHostErrorV1 {
    #[error("managed Agent Host input is invalid")]
    InvalidInput,
    #[error("managed Agent Host profile identity is immutable")]
    ImmutableProfileConflict,
    #[error("managed Agent Host is at capacity")]
    AtCapacity,
    #[error("managed Agent Host dependency is unavailable")]
    Unavailable,
    #[error("managed Agent Host operation is ambiguous")]
    Ambiguous,
    #[error("managed Agent Host retained state is corrupt")]
    Corrupt,
}

/// Private two-child launch plan. It deliberately has no `Serialize` or `Debug`.
pub struct ManagedAgentHostLaunchPlanV1 {
    helper_executable: PathBuf,
    helper_state_dir: PathBuf,
    launch_reference: String,
    host_executable: PathBuf,
    provider: String,
    provider_address: SocketAddr,
    model: String,
    house: Option<HouseAgentHostLaunchV1>,
}

struct HouseAgentHostLaunchV1 {
    working_directory: PathBuf,
    runner_unit_id: String,
    revision_digest: String,
    revision_bytes: Vec<u8>,
    development_provider_address: Option<SocketAddr>,
}

impl ManagedAgentHostLaunchPlanV1 {
    /// Constructs the fixed topology for a typed managed-reference provider.
    ///
    /// # Errors
    ///
    /// Rejects the same unsafe topology or bounded configuration as [`Self::new`].
    pub fn new_managed_reference(
        helper_executable: &Path,
        state_dir: &Path,
        launch_reference: &str,
        host_executable: &Path,
        provider: ManagedReferenceProviderV1,
        provider_address: SocketAddr,
        model: &str,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        if provider != ManagedReferenceProviderV1::OpenAiCompatible {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        Self::new(
            helper_executable,
            state_dir,
            launch_reference,
            host_executable,
            managed_reference_provider_cli_argument(provider),
            provider_address,
            model,
        )
    }

    /// Constructs the fixed-helper/two-child topology.
    ///
    /// # Errors
    ///
    /// Rejects non-absolute executables/state, unsafe reference, or labels.
    pub fn new(
        helper_executable: &Path,
        state_dir: &Path,
        launch_reference: &str,
        host_executable: &Path,
        provider: &str,
        provider_address: SocketAddr,
        model: &str,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        let helper_name = helper_executable
            .file_stem()
            .and_then(|value| value.to_str());
        if helper_name != Some("worldstream-assignment-mcp")
            || !helper_executable.is_absolute()
            || !state_dir.is_absolute()
            || !host_executable.is_absolute()
            || !is_secret_reference(launch_reference)
            || !bounded(provider)
            || !provider_address.ip().is_loopback()
            || !bounded_model(model)
        {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        Ok(Self {
            helper_executable: helper_executable.to_owned(),
            helper_state_dir: state_dir.to_owned(),
            launch_reference: launch_reference.to_owned(),
            host_executable: host_executable.to_owned(),
            provider: provider.to_owned(),
            provider_address,
            model: model.to_owned(),
            house: None,
        })
    }

    /// Constructs the fixed, authority-free `OpenRouter` House host topology.
    ///
    /// The model host receives no Controller path or assignment launch
    /// reference. Its private working directory is selected by the Supervisor,
    /// and the exact House revision is delivered through private stdin.
    pub(crate) fn new_house(
        helper_executable: &Path,
        state_dir: &Path,
        launch_reference: &str,
        host_executable: &Path,
        working_directory: &Path,
        runner_unit_id: &str,
        revision: &HouseAgentRevision,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        Self::new_house_with_provider(
            helper_executable,
            state_dir,
            launch_reference,
            host_executable,
            working_directory,
            runner_unit_id,
            revision,
            None,
        )
    }

    /// Constructs a House host that can reach only one explicit loopback
    /// development substitute. This path is never selected by production setup.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_house_development(
        helper_executable: &Path,
        state_dir: &Path,
        launch_reference: &str,
        host_executable: &Path,
        working_directory: &Path,
        runner_unit_id: &str,
        revision: &HouseAgentRevision,
        provider_address: SocketAddr,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        if !provider_address.ip().is_loopback() || provider_address.port() == 0 {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        Self::new_house_with_provider(
            helper_executable,
            state_dir,
            launch_reference,
            host_executable,
            working_directory,
            runner_unit_id,
            revision,
            Some(provider_address),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_house_with_provider(
        helper_executable: &Path,
        state_dir: &Path,
        launch_reference: &str,
        host_executable: &Path,
        working_directory: &Path,
        runner_unit_id: &str,
        revision: &HouseAgentRevision,
        development_provider_address: Option<SocketAddr>,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        let helper_name = helper_executable
            .file_stem()
            .and_then(|value| value.to_str());
        if helper_name != Some("worldstream-assignment-mcp")
            || !helper_executable.is_absolute()
            || !state_dir.is_absolute()
            || !host_executable.is_absolute()
            || !working_directory.is_absolute()
            || !is_secret_reference(launch_reference)
            || !bounded_runner_unit(runner_unit_id)
        {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        Ok(Self {
            helper_executable: helper_executable.to_owned(),
            helper_state_dir: state_dir.to_owned(),
            launch_reference: launch_reference.to_owned(),
            host_executable: host_executable.to_owned(),
            provider: "openrouter-house".to_owned(),
            provider_address: SocketAddr::from(([127, 0, 0, 1], 1)),
            model: revision.model_slug().to_owned(),
            house: Some(HouseAgentHostLaunchV1 {
                working_directory: working_directory.to_owned(),
                runner_unit_id: runner_unit_id.to_owned(),
                revision_digest: revision.digest().to_owned(),
                revision_bytes: revision.canonical_bytes().to_vec(),
                development_provider_address,
            }),
        })
    }

    #[must_use]
    pub fn helper_program(&self) -> &Path {
        &self.helper_executable
    }
    #[must_use]
    pub fn helper_arguments(&self) -> [&str; 4] {
        [
            "--state-dir",
            self.helper_state_dir.to_str().unwrap_or(""),
            "--launch-reference",
            &self.launch_reference,
        ]
    }
    #[must_use]
    pub fn host_program(&self) -> &Path {
        &self.host_executable
    }
    #[must_use]
    pub fn host_arguments(&self) -> Vec<String> {
        if let Some(house) = &self.house {
            let mut arguments = vec![
                "--transport".to_owned(),
                "stdio".to_owned(),
                "--provider".to_owned(),
                "openrouter-house".to_owned(),
            ];
            if let Some(address) = house.development_provider_address {
                arguments.push("--provider-address".to_owned());
                arguments.push(address.to_string());
            }
            return arguments;
        }
        vec![
            "--transport".to_owned(),
            "stdio".to_owned(),
            "--provider".to_owned(),
            self.provider.clone(),
            "--provider-address".to_owned(),
            self.provider_address.to_string(),
            "--model".to_owned(),
            self.model.clone(),
        ]
    }
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    fn host_working_directory(&self) -> Option<&Path> {
        self.house
            .as_ref()
            .map(|house| house.working_directory.as_path())
    }

    fn startup_frame(
        &self,
        model_credential: &Zeroizing<Vec<u8>>,
    ) -> Result<Zeroizing<Vec<u8>>, ManagedAgentHostErrorV1> {
        if let Some(house) = &self.house {
            if !(32..=512).contains(&model_credential.len())
                || house.revision_bytes.is_empty()
                || house.revision_bytes.len() > 262_144
            {
                return Err(ManagedAgentHostErrorV1::InvalidInput);
            }
            let header = format!(
                "WSMHOUSE1 {} {} {}\n",
                house.runner_unit_id,
                house.revision_bytes.len(),
                model_credential.len()
            );
            let mut frame = Zeroizing::new(Vec::with_capacity(
                header.len() + house.revision_bytes.len() + model_credential.len(),
            ));
            frame.extend_from_slice(header.as_bytes());
            frame.extend_from_slice(&house.revision_bytes);
            frame.extend_from_slice(model_credential);
            return Ok(frame);
        }
        if model_credential.is_empty() || model_credential.len() > 16 * 1024 {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        let mut frame = Zeroizing::new(Vec::with_capacity(10 + model_credential.len() * 2));
        frame.extend_from_slice(b"WSMCRED1 ");
        for byte in model_credential.iter() {
            frame.push(b"0123456789abcdef"[usize::from(byte >> 4)]);
            frame.push(b"0123456789abcdef"[usize::from(byte & 0x0f)]);
        }
        frame.push(b'\n');
        Ok(frame)
    }
}

/// Private prepared launch passed only between Supervisor-owned adapters.
pub struct ManagedAgentHostPreparedLaunchV1 {
    profile: ManagedAgentHostProfileV1,
    plan: ManagedAgentHostLaunchPlanV1,
    credential: Zeroizing<Vec<u8>>,
}

impl ManagedAgentHostPreparedLaunchV1 {
    /// Constructs one exact prepared launch without exposing its fields.
    #[must_use]
    pub fn new(
        profile: ManagedAgentHostProfileV1,
        plan: ManagedAgentHostLaunchPlanV1,
        credential: Zeroizing<Vec<u8>>,
    ) -> Self {
        Self {
            profile,
            plan,
            credential,
        }
    }

    pub(crate) fn new_house(
        assignment_id: &str,
        host_contract_revision: &str,
        credential_reference: &str,
        plan: ManagedAgentHostLaunchPlanV1,
        credential: Zeroizing<Vec<u8>>,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        let profile = ManagedAgentHostProfileV1::new(
            assignment_id,
            "openrouter-house",
            host_contract_revision,
            "openrouter_house",
            SocketAddr::from(([127, 0, 0, 1], 1)),
            plan.model(),
            credential_reference,
            4,
            Duration::from_secs(30),
        )?;
        Ok(Self {
            profile,
            plan,
            credential,
        })
    }
}

/// Exact assignment/Profile/template resolution boundary for post-setup start.
pub trait ManagedAgentHostStartSourceV1: Send + Sync {
    /// Resolves one immutable launch or returns a closed failure.
    ///
    /// # Errors
    ///
    /// Rejects missing, incompatible, changed, or unavailable dependencies.
    fn prepare(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1>;
}

#[derive(Clone)]
struct ProductionManagedAgentHostStartSourceV1<S> {
    state_dir: PathBuf,
    helper_executable: PathBuf,
    profiles: AgentProfileStoreV1,
    runners: RunnerSupervisorV1,
    launches: AssignmentMcpLaunchRegistryV1<S>,
    vault: FileSecretVaultV1,
    task_setup: TaskSetupSupervisorV1,
}

impl<S> ProductionManagedAgentHostStartSourceV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    fn new(
        state_dir: &Path,
        helper_executable: &Path,
        profiles: AgentProfileStoreV1,
        runners: RunnerSupervisorV1,
        launches: AssignmentMcpLaunchRegistryV1<S>,
        vault: FileSecretVaultV1,
        task_setup: TaskSetupSupervisorV1,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        if !state_dir.is_absolute() || !helper_executable.is_absolute() {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        Ok(Self {
            state_dir: state_dir.to_owned(),
            helper_executable: helper_executable.to_owned(),
            profiles,
            runners,
            launches,
            vault,
            task_setup,
        })
    }
}

impl<S> ManagedAgentHostStartSourceV1 for ProductionManagedAgentHostStartSourceV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    #[allow(clippy::too_many_lines)]
    fn prepare(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1> {
        let assignment = self
            .profiles
            .assignment(assignment_id)
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let revision = self
            .profiles
            .retained_revision(&assignment.profile.profile_id, &assignment.profile.revision)
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let setup = self
            .task_setup
            .status(&assignment.draft_id)
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let setup_seat = setup
            .seats
            .iter()
            .find(|seat| seat.seat_id == assignment.seat_id)
            .ok_or(ManagedAgentHostErrorV1::Corrupt)?;
        if setup.state != TaskSetupStateV1::Ready
            || setup.room_id != assignment.membership.room_id
            || setup_seat.member_id.as_deref() != Some(assignment.membership.member_id.as_str())
            || setup_seat.runner_authority != "provisioned"
        {
            return Err(ManagedAgentHostErrorV1::Unavailable);
        }
        let (
            AgentHostContractV1::ManagedReference {
                host_contract_revision,
                runner_template,
                provider,
                provider_address,
                model_id,
            },
            AgentExecutionBindingV1::ManagedReference {
                runner_id,
                instance_id,
                template_id,
                template_revision,
            },
        ) = (revision.host_contract, assignment.execution)
        else {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        };
        if runner_template.template_id != template_id
            || runner_template.revision != template_revision
            || runner_id.parse::<UlidString>().is_err()
        {
            return Err(ManagedAgentHostErrorV1::Corrupt);
        }
        let runner_status = self.runners.statuses();
        let exact_instance = runner_status
            .instances
            .iter()
            .find(|instance| {
                instance.instance_id == instance_id
                    && instance.template_id == template_id
                    && instance.template_revision == template_revision
            })
            .ok_or(ManagedAgentHostErrorV1::Unavailable)?;
        let host_executable = self
            .runners
            .managed_reference_executable(&instance_id, &template_id, &template_revision)
            .ok_or(ManagedAgentHostErrorV1::Unavailable)?;
        let launch_reference = self
            .launches
            .issue(assignment_id)
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let mut model_provider_settings = revision.secret_settings.iter().filter(|setting| {
            setting.kind == SecretKindV1::ModelProvider && setting.key == "MODEL_PROVIDER_TOKEN"
        });
        let secret = model_provider_settings
            .next()
            .filter(|_| model_provider_settings.next().is_none())
            .ok_or(ManagedAgentHostErrorV1::Corrupt)?;
        let credential = self
            .vault
            .resolve(SecretKindV1::ModelProvider, &secret.reference)
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let profile = ManagedAgentHostProfileV1::new(
            assignment_id,
            &instance_id,
            &host_contract_revision,
            match provider {
                ManagedReferenceProviderV1::OpenAiCompatible => "openai_compatible",
                ManagedReferenceProviderV1::Openrouter => {
                    return Err(ManagedAgentHostErrorV1::Corrupt);
                }
            },
            provider_address,
            &model_id,
            secret.reference.as_str(),
            exact_instance.capacity.maximum,
            Duration::from_secs(30),
        )?;
        let plan = ManagedAgentHostLaunchPlanV1::new_managed_reference(
            &self.helper_executable,
            &self.state_dir,
            &launch_reference,
            &host_executable,
            provider,
            provider_address,
            &model_id,
        )?;
        Ok(ManagedAgentHostPreparedLaunchV1 {
            profile,
            plan,
            credential: Zeroizing::new(credential.as_bytes().to_vec()),
        })
    }
}

const fn managed_reference_provider_cli_argument(
    provider: ManagedReferenceProviderV1,
) -> &'static str {
    match provider {
        ManagedReferenceProviderV1::OpenAiCompatible => "openai-compatible",
        ManagedReferenceProviderV1::Openrouter => "unsupported-openrouter",
    }
}

#[derive(Clone)]
pub struct ManagedAgentHostOperationsV1 {
    root: Arc<PathBuf>,
    profiles: ManagedAgentHostStoreV1,
    source: Arc<dyn ManagedAgentHostStartSourceV1>,
    house_source: Arc<Mutex<Option<Weak<dyn ManagedAgentHostStartSourceV1>>>>,
    launcher: Arc<dyn ManagedAgentHostProcessLauncherV1>,
    processes: Arc<Mutex<BTreeMap<String, Box<dyn ManagedAgentHostProcessV1>>>>,
    mutation: Arc<Mutex<()>>,
    starts_paused: Arc<RwLock<bool>>,
    activation_status: Option<ManagedActivationStatusStoreV1>,
}

impl ManagedAgentHostOperationsV1 {
    /// Opens the durable post-setup host operation with production exact stores.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe state, helper topology, or corrupt operations.
    #[allow(clippy::too_many_arguments)]
    pub fn open_production<S>(
        root: &Path,
        state_dir: &Path,
        helper_executable: &Path,
        profiles: AgentProfileStoreV1,
        runners: RunnerSupervisorV1,
        launches: AssignmentMcpLaunchRegistryV1<S>,
        vault: FileSecretVaultV1,
        task_setup: TaskSetupSupervisorV1,
    ) -> Result<Self, ManagedAgentHostErrorV1>
    where
        S: AssignedMembershipLaunchSourceV1,
    {
        let source = ProductionManagedAgentHostStartSourceV1::new(
            state_dir,
            helper_executable,
            profiles,
            runners,
            launches,
            vault,
            task_setup,
        )?;
        let mut operations = Self::open_with(root, source, OsManagedAgentHostProcessLauncherV1)?;
        operations.activation_status = Some(
            ManagedActivationStatusStoreV1::open(state_dir.join("managed-agent-activation-status"))
                .map_err(map_activation_status_error)?,
        );
        Ok(operations)
    }

    /// Opens an operation store with bounded process/source adapters.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe or corrupt retained state.
    pub fn open_with(
        root: &Path,
        source: impl ManagedAgentHostStartSourceV1 + 'static,
        launcher: impl ManagedAgentHostProcessLauncherV1 + 'static,
    ) -> Result<Self, ManagedAgentHostErrorV1> {
        let root =
            prepare_data_directory(root).map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let profiles = ManagedAgentHostStoreV1::open(&root.join("turns"))?;
        let operations = prepare_data_directory(&root.join("operations"))
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        for path in json_files(&operations)? {
            let operation: ManagedAgentHostOperationV1 = read_record(&path)?;
            validate_host_operation(&operation)?;
            if path != operations.join(format!("{}.json", operation.assignment_id)) {
                return Err(ManagedAgentHostErrorV1::Corrupt);
            }
        }
        Ok(Self {
            root: Arc::new(operations),
            profiles,
            source: Arc::new(source),
            house_source: Arc::new(Mutex::new(None)),
            launcher: Arc::new(launcher),
            processes: Arc::new(Mutex::new(BTreeMap::new())),
            mutation: Arc::new(Mutex::new(())),
            starts_paused: Arc::new(RwLock::new(false)),
            activation_status: None,
        })
    }

    /// Starts or safely retries the exact retained managed host operation.
    ///
    /// # Errors
    ///
    /// Rejects altered bindings and returns closed dependency/process failures.
    pub fn start(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        let _admission = self.admit_start()?;
        self.start_permitted(assignment_id)
    }

    /// Starts or resumes one exact House process pair prepared by the hosted
    /// lifecycle coordinator. The retained binding is checked before an
    /// existing live child can be treated as the requested unit.
    pub(crate) fn start_house(
        &self,
        assignment_id: &str,
        prepared: &ManagedAgentHostPreparedLaunchV1,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        let _admission = self.admit_start()?;
        self.start_prepared(assignment_id, prepared)
    }

    fn admit_start(&self) -> Result<std::sync::RwLockReadGuard<'_, bool>, ManagedAgentHostErrorV1> {
        let admission = self
            .starts_paused
            .try_read()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        if *admission {
            return Err(ManagedAgentHostErrorV1::Unavailable);
        }
        Ok(admission)
    }

    // The lifecycle coordinator holds the exclusive start gate while restoring.
    // All HTTP/public start paths must use `start`, including retry aliases.
    pub(crate) fn start_permitted(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        let prepared = match self.source.prepare(assignment_id) {
            Err(ManagedAgentHostErrorV1::InvalidInput) => {
                let source = self
                    .house_source
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .ok_or(ManagedAgentHostErrorV1::InvalidInput)?;
                source.prepare(assignment_id)?
            }
            result => result?,
        };
        self.start_prepared(assignment_id, &prepared)
    }

    // The hosted adapter owns the strong reference. A weak link avoids a cycle
    // through its readiness dependencies and cannot keep a retired adapter alive.
    pub(crate) fn register_house_start_source(
        &self,
        source: &Arc<dyn ManagedAgentHostStartSourceV1>,
    ) -> Result<(), ManagedAgentHostErrorV1> {
        let mut retained = self
            .house_source
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if retained.is_some() {
            return Err(ManagedAgentHostErrorV1::ImmutableProfileConflict);
        }
        *retained = Some(Arc::downgrade(source));
        Ok(())
    }

    fn start_prepared(
        &self,
        assignment_id: &str,
        prepared: &ManagedAgentHostPreparedLaunchV1,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        if assignment_id.parse::<UlidString>().is_err() {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        if prepared.profile.assignment_id != assignment_id {
            return Err(ManagedAgentHostErrorV1::ImmutableProfileConflict);
        }
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        self.profiles.publish_profile(&prepared.profile)?;
        let binding_hash = launch_binding_hash(prepared)?;
        if self
            .processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(assignment_id)
        {
            let operation = self
                .load_operation(assignment_id)?
                .ok_or(ManagedAgentHostErrorV1::Corrupt)?;
            if operation.binding_hash != binding_hash {
                return Err(ManagedAgentHostErrorV1::ImmutableProfileConflict);
            }
            return self.status_locked(assignment_id);
        }
        if self.active_for_host(&prepared.profile.host_id)? >= prepared.profile.capacity {
            return Err(ManagedAgentHostErrorV1::AtCapacity);
        }
        let mut operation =
            self.load_operation(assignment_id)?
                .unwrap_or_else(|| ManagedAgentHostOperationV1 {
                    schema: "worldstream/managed-agent-host-operation/v1".to_owned(),
                    assignment_id: assignment_id.to_owned(),
                    operation_id: stable_id(assignment_id, 1, "host-start"),
                    binding_hash: binding_hash.clone(),
                    state: ManagedAgentHostStateV1::Stopped,
                    expected_running: false,
                    attempts: 0,
                    observed_at_ms: None,
                    failure: None,
                });
        if operation.binding_hash != binding_hash {
            return Err(ManagedAgentHostErrorV1::ImmutableProfileConflict);
        }
        operation.state = ManagedAgentHostStateV1::Starting;
        operation.expected_running = true;
        operation.attempts = operation.attempts.saturating_add(1);
        operation.failure = None;
        self.persist_operation(&operation)?;
        match self
            .launcher
            .launch_bridged(&prepared.plan, &prepared.credential)
        {
            Ok(process) => {
                let observed_at_ms = if prepared.plan.house.is_some() {
                    // A House unit is ready only after the model host has
                    // parsed its startup frame, opened the allowance ledger,
                    // and completed the MCP initialization exchange.
                    process.last_activity_at_ms()
                } else {
                    Some(now_ms()?)
                };
                self.processes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(assignment_id.to_owned(), process);
                operation.state = ManagedAgentHostStateV1::Running;
                operation.observed_at_ms = observed_at_ms;
                self.persist_operation(&operation)?;
                let active = self.active_for_host(&prepared.profile.host_id)?;
                let mut status = operation.status(&prepared.profile, active);
                self.attach_activation_status(&mut status)?;
                Ok(status)
            }
            Err(error) => {
                operation.state = ManagedAgentHostStateV1::NeedsAttention;
                operation.failure = Some(process_failure(
                    "host_start_failed",
                    "Retry the same managed host operation after repairing the approved executable or credential.",
                ));
                self.persist_operation(&operation)?;
                Err(error)
            }
        }
    }

    pub(crate) fn pause_starts(
        &self,
    ) -> Result<(RwLockWriteGuard<'_, bool>, bool), ManagedAgentHostErrorV1> {
        let mut paused = self
            .starts_paused
            .write()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let previous = *paused;
        *paused = true;
        Ok((paused, previous))
    }

    /// Returns and reconciles browser-safe lifecycle/capacity/freshness state.
    ///
    /// # Errors
    ///
    /// Fails closed for unknown or corrupt retained operations.
    pub fn status(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        self.status_locked(assignment_id)
    }

    /// Returns all retained managed reference hosts in stable assignment order.
    ///
    /// # Errors
    ///
    /// Fails closed for corrupt retained state or unavailable process/storage observation.
    pub fn statuses(&self) -> Result<Vec<ManagedAgentHostStatusV1>, ManagedAgentHostErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let mut statuses = Vec::new();
        for path in json_files(&self.root)? {
            if statuses.len() >= 256 {
                return Err(ManagedAgentHostErrorV1::Corrupt);
            }
            let assignment_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or(ManagedAgentHostErrorV1::Corrupt)?;
            statuses.push(self.status_locked(assignment_id)?);
        }
        Ok(statuses)
    }

    pub(crate) fn owned_assignments(&self) -> Result<Vec<String>, ManagedAgentHostErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let records = json_files(&self.root)?;
        if records.len() > 256 {
            return Err(ManagedAgentHostErrorV1::Corrupt);
        }
        for path in records {
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or(ManagedAgentHostErrorV1::Corrupt)?;
            self.status_locked(id)?;
            let operation = self
                .load_operation(id)?
                .ok_or(ManagedAgentHostErrorV1::Corrupt)?;
            if operation.expected_running
                && !self
                    .processes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains_key(id)
            {
                // A recovered Controller cannot silently omit a retained live
                // intent just because its old child handle is unavailable.
                return Err(ManagedAgentHostErrorV1::Unavailable);
            }
        }
        Ok(self
            .processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect())
    }

    fn status_locked(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        let profile = self.profiles.profile(assignment_id)?;
        let mut operation = self
            .load_operation(assignment_id)?
            .ok_or(ManagedAgentHostErrorV1::InvalidInput)?;
        let (exited, last_activity) = {
            let mut processes = self
                .processes
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match processes.get_mut(assignment_id) {
                Some(process) => (process.try_wait()?, process.last_activity_at_ms()),
                None => (None, None),
            }
        };
        if let Some(exit_code) = exited {
            self.processes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(assignment_id);
            // This is an observed exit of our owned child, unlike a lost handle.
            operation.expected_running = false;
            operation.state = ManagedAgentHostStateV1::NeedsAttention;
            operation.failure = Some(process_exit_failure(exit_code));
            self.persist_operation(&operation)?;
        } else if operation.expected_running
            && self
                .processes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(assignment_id)
        {
            operation.state = ManagedAgentHostStateV1::Running;
            if let Some(observed) = last_activity {
                operation.observed_at_ms = Some(
                    operation
                        .observed_at_ms
                        .map_or(observed, |retained| retained.max(observed)),
                );
            }
            operation.failure = None;
            self.persist_operation(&operation)?;
        } else if operation.expected_running
            && operation.failure.is_none()
            && !self
                .processes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(assignment_id)
        {
            operation.state = ManagedAgentHostStateV1::NeedsAttention;
            operation.failure = Some(process_failure(
                "host_restart_required",
                "Retry the same managed host operation after Supervisor restart.",
            ));
            self.persist_operation(&operation)?;
        }
        let active = self.active_for_host(&profile.host_id)?;
        let mut status = operation.status(&profile, active);
        self.attach_activation_status(&mut status)?;
        Ok(status)
    }

    /// Stops only the exact retained two-child operation.
    ///
    /// # Errors
    ///
    /// Returns closed unavailable/corrupt state.
    pub fn stop(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let profile = self.profiles.profile(assignment_id)?;
        let mut operation = self
            .load_operation(assignment_id)?
            .ok_or(ManagedAgentHostErrorV1::InvalidInput)?;
        {
            let mut processes = self
                .processes
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(process) = processes.get_mut(assignment_id) {
                // Keep ownership when a stop fails so the same process can be
                // inspected or stopped again. An error does not prove exit.
                process.stop()?;
            } else if operation.expected_running {
                // A retained running intent without its owned process handle
                // is unresolved, not evidence of a successful shutdown.
                return Err(ManagedAgentHostErrorV1::Unavailable);
            }
            processes.remove(assignment_id);
        }
        operation.state = ManagedAgentHostStateV1::Stopped;
        operation.expected_running = false;
        operation.failure = None;
        self.persist_operation(&operation)?;
        let active = self.active_for_host(&profile.host_id)?;
        let mut status = operation.status(&profile, active);
        self.attach_activation_status(&mut status)?;
        Ok(status)
    }

    fn attach_activation_status(
        &self,
        status: &mut ManagedAgentHostStatusV1,
    ) -> Result<(), ManagedAgentHostErrorV1> {
        if let Some(activation_status) = &self.activation_status {
            status.activation = activation_status
                .status(&status.assignment_id)
                .map_err(map_activation_status_error)?;
        }
        Ok(())
    }

    fn load_operation(
        &self,
        assignment_id: &str,
    ) -> Result<Option<ManagedAgentHostOperationV1>, ManagedAgentHostErrorV1> {
        let path = self.root.join(format!("{assignment_id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let operation = read_record(&path)?;
        validate_host_operation(&operation)?;
        Ok(Some(operation))
    }

    fn persist_operation(
        &self,
        operation: &ManagedAgentHostOperationV1,
    ) -> Result<(), ManagedAgentHostErrorV1> {
        validate_host_operation(operation)?;
        persist_record(
            &self.root,
            &self.root.join(format!("{}.json", operation.assignment_id)),
            operation,
            true,
        )
    }

    fn active_for_host(&self, host_id: &str) -> Result<u32, ManagedAgentHostErrorV1> {
        let assignments = self
            .processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut count = 0_usize;
        for assignment_id in &assignments {
            if self.profiles.profile(assignment_id)?.host_id == host_id {
                count = count.saturating_add(1);
            }
        }
        u32::try_from(count).map_err(|_| ManagedAgentHostErrorV1::Corrupt)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedAgentHostOperationV1 {
    schema: String,
    assignment_id: String,
    operation_id: String,
    binding_hash: String,
    state: ManagedAgentHostStateV1,
    expected_running: bool,
    attempts: u32,
    observed_at_ms: Option<u64>,
    failure: Option<ManagedAgentHostFailureV1>,
}

impl ManagedAgentHostOperationV1 {
    fn status(
        &self,
        profile: &ManagedAgentHostProfileV1,
        active_invocations: u32,
    ) -> ManagedAgentHostStatusV1 {
        let freshness =
            self.observed_at_ms
                .map_or(ManagedAgentHostFreshnessV1::Stale, |observed| {
                    let age = now_ms().unwrap_or(u64::MAX).saturating_sub(observed);
                    if age <= profile.stale_after_ms {
                        ManagedAgentHostFreshnessV1::Fresh
                    } else {
                        ManagedAgentHostFreshnessV1::Stale
                    }
                });
        ManagedAgentHostStatusV1 {
            schema: "worldstream/managed-agent-host-status/v1".to_owned(),
            assignment_id: self.assignment_id.clone(),
            host_id: profile.host_id.clone(),
            host_revision: profile.host_revision.clone(),
            state: self.state,
            ready: self.state == ManagedAgentHostStateV1::Running
                && freshness == ManagedAgentHostFreshnessV1::Fresh
                && active_invocations <= profile.capacity,
            capacity: profile.capacity,
            active_invocations,
            freshness,
            activation: ManagedAgentActivationStatusV1::default(),
            failure: self.failure.clone(),
        }
    }
}

fn launch_binding_hash(
    prepared: &ManagedAgentHostPreparedLaunchV1,
) -> Result<String, ManagedAgentHostErrorV1> {
    let house = prepared.plan.house.as_ref().map(|house| {
        serde_json::json!({
            "runner_unit_id": house.runner_unit_id,
            "revision_digest": house.revision_digest,
            "working_directory": house.working_directory,
        })
    });
    canonical_hash(&serde_json::json!({
        "assignment_id": prepared.profile.assignment_id,
        "host_id": prepared.profile.host_id,
        "host_revision": prepared.profile.host_revision,
        "provider": prepared.profile.provider,
        "provider_address": prepared.profile.provider_address,
        "model": prepared.profile.model,
        "capacity": prepared.profile.capacity,
        "helper": prepared.plan.helper_program(),
        "host": prepared.plan.host_program(),
        "house": house,
        "launch_reference_hash": blake3::hash(prepared.plan.launch_reference.as_bytes()).to_hex().to_string(),
    }))
}

fn validate_host_operation(
    value: &ManagedAgentHostOperationV1,
) -> Result<(), ManagedAgentHostErrorV1> {
    if value.schema != "worldstream/managed-agent-host-operation/v1"
        || value.assignment_id.parse::<UlidString>().is_err()
        || value.operation_id.parse::<UlidString>().is_err()
        || !value.binding_hash.starts_with("blake3:")
        || value.attempts == 0
        || (value.expected_running && value.state == ManagedAgentHostStateV1::Stopped)
    {
        return Err(ManagedAgentHostErrorV1::Corrupt);
    }
    Ok(())
}

fn process_failure(code: &str, safe_action: &str) -> ManagedAgentHostFailureV1 {
    ManagedAgentHostFailureV1 {
        code: code.to_owned(),
        message: "The managed Agent Host is unavailable; no private provider content was retained."
            .to_owned(),
        safe_action: safe_action.to_owned(),
    }
}

fn process_exit_failure(exit_code: i32) -> ManagedAgentHostFailureV1 {
    ManagedAgentHostFailureV1 {
        code: "host_exited".to_owned(),
        message: format!("Managed Agent Host child exited with process status {exit_code}."),
        safe_action: "Retry the same managed host operation after inspecting the approved provider configuration."
            .to_owned(),
    }
}

const fn map_activation_status_error(
    error: ManagedActivationStatusErrorV1,
) -> ManagedAgentHostErrorV1 {
    match error {
        ManagedActivationStatusErrorV1::Invalid => ManagedAgentHostErrorV1::Corrupt,
        ManagedActivationStatusErrorV1::Unavailable => ManagedAgentHostErrorV1::Unavailable,
    }
}

fn now_ms() -> Result<u64, ManagedAgentHostErrorV1> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?
        .as_millis();
    u64::try_from(value).map_err(|_| ManagedAgentHostErrorV1::Unavailable)
}

impl ManagedReferenceHostReadinessSourceV1 for ManagedAgentHostOperationsV1 {
    fn readiness_reason(&self, assignment_id: &str) -> TaskSeatReadinessReasonV1 {
        match self.status(assignment_id) {
            Ok(status) if status.ready => TaskSeatReadinessReasonV1::Ready,
            Ok(status)
                if status.state == ManagedAgentHostStateV1::Running
                    && status.freshness == ManagedAgentHostFreshnessV1::Stale =>
            {
                TaskSeatReadinessReasonV1::RunnerStale
            }
            Ok(status)
                if status.state == ManagedAgentHostStateV1::Running
                    && status.active_invocations > status.capacity =>
            {
                TaskSeatReadinessReasonV1::RunnerOverCapacity
            }
            Ok(_) => TaskSeatReadinessReasonV1::RunnerDisconnected,
            Err(ManagedAgentHostErrorV1::InvalidInput) => TaskSeatReadinessReasonV1::RunnerMissing,
            Err(
                ManagedAgentHostErrorV1::ImmutableProfileConflict
                | ManagedAgentHostErrorV1::Corrupt,
            ) => TaskSeatReadinessReasonV1::RunnerIncompatible,
            Err(ManagedAgentHostErrorV1::AtCapacity) => {
                TaskSeatReadinessReasonV1::RunnerOverCapacity
            }
            Err(ManagedAgentHostErrorV1::Unavailable | ManagedAgentHostErrorV1::Ambiguous) => {
                TaskSeatReadinessReasonV1::RunnerDisconnected
            }
        }
    }
}

/// Adds bounded post-setup managed host lifecycle routes.
pub fn managed_agent_host_router(operations: ManagedAgentHostOperationsV1) -> Router {
    Router::new()
        .route(
            "/api/v1/managed-agent-hosts/{assignment_id}",
            get(host_status),
        )
        .route(
            "/api/v1/managed-agent-hosts/{assignment_id}/start",
            post(host_start),
        )
        .route(
            "/api/v1/managed-agent-hosts/{assignment_id}/retry",
            post(host_start),
        )
        .route(
            "/api/v1/managed-agent-hosts/{assignment_id}/stop",
            post(host_stop),
        )
        .with_state(operations)
}

async fn host_status(
    State(operations): State<ManagedAgentHostOperationsV1>,
    AxumPath(assignment_id): AxumPath<String>,
) -> Result<Json<ManagedAgentHostStatusV1>, StatusCode> {
    tokio::task::spawn_blocking(move || operations.status(&assignment_id))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map(Json)
        .map_err(map_host_status)
}
async fn host_start(
    State(operations): State<ManagedAgentHostOperationsV1>,
    AxumPath(assignment_id): AxumPath<String>,
) -> Result<Json<ManagedAgentHostStatusV1>, StatusCode> {
    tokio::task::spawn_blocking(move || operations.start(&assignment_id))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map(Json)
        .map_err(map_host_status)
}
async fn host_stop(
    State(operations): State<ManagedAgentHostOperationsV1>,
    AxumPath(assignment_id): AxumPath<String>,
) -> Result<Json<ManagedAgentHostStatusV1>, StatusCode> {
    tokio::task::spawn_blocking(move || operations.stop(&assignment_id))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map(Json)
        .map_err(map_host_status)
}
const fn map_host_status(error: ManagedAgentHostErrorV1) -> StatusCode {
    match error {
        ManagedAgentHostErrorV1::InvalidInput => StatusCode::BAD_REQUEST,
        ManagedAgentHostErrorV1::ImmutableProfileConflict => StatusCode::CONFLICT,
        ManagedAgentHostErrorV1::AtCapacity => StatusCode::TOO_MANY_REQUESTS,
        ManagedAgentHostErrorV1::Unavailable | ManagedAgentHostErrorV1::Ambiguous => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ManagedAgentHostErrorV1::Corrupt => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Process boundary that must create two children and bridge only stdio.
pub trait ManagedAgentHostProcessLauncherV1: Send + Sync {
    /// Starts the two fixed children and bridges their stdio.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable failure without child output or secrets.
    fn launch_bridged(
        &self,
        plan: &ManagedAgentHostLaunchPlanV1,
        model_credential: &Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1>;
}

/// Bounded lifecycle handle retained only by the Supervisor.
pub trait ManagedAgentHostProcessV1: Send {
    /// Polls the bridged children without blocking.
    ///
    /// # Errors
    ///
    /// Returns a closed process observation failure.
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1>;
    /// Returns the most recent observed assignment-MCP request time.
    fn last_activity_at_ms(&self) -> Option<u64> {
        None
    }
    /// Stops and reaps both fixed children.
    ///
    /// # Errors
    ///
    /// Returns a closed process cleanup failure.
    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1>;
}

/// Production launcher. Child stdout is protocol-only and stderr is discarded.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsManagedAgentHostProcessLauncherV1;

impl ManagedAgentHostProcessLauncherV1 for OsManagedAgentHostProcessLauncherV1 {
    fn launch_bridged(
        &self,
        plan: &ManagedAgentHostLaunchPlanV1,
        model_credential: &Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        let startup_frame = plan.startup_frame(model_credential)?;
        if let Some(working_directory) = plan.host_working_directory() {
            worldstream_runtime::validate_data_directory(working_directory)
                .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
            // Operator retirement is installed only while the Controller and
            // its children are stopped. Retain this fence across restarts and
            // reject links and inspection errors as well as ordinary files.
            // It is not an Outcome and never refunds provider allowance.
            for marker in ["retiring.json", "retired.json", "operator-retired.json"] {
                match std::fs::symlink_metadata(working_directory.join(marker)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(ManagedAgentHostErrorV1::Unavailable),
                }
            }
        }
        let mut helper = Command::new(plan.helper_program())
            .args(plan.helper_arguments())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let mut host_command = Command::new(plan.host_program());
        host_command
            .args(plan.host_arguments())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(working_directory) = plan.host_working_directory() {
            host_command.current_dir(working_directory);
        }
        let Ok(mut host) = host_command.spawn() else {
            kill_and_wait(&mut helper);
            return Err(ManagedAgentHostErrorV1::Unavailable);
        };
        let pipes = (
            helper.stdin.take(),
            helper.stdout.take(),
            host.stdin.take(),
            host.stdout.take(),
        );
        let (Some(mut helper_in), Some(mut helper_out), Some(mut host_in), Some(mut host_out)) =
            pipes
        else {
            kill_and_wait(&mut helper);
            kill_and_wait(&mut host);
            return Err(ManagedAgentHostErrorV1::Unavailable);
        };
        if host_in
            .write_all(&startup_frame)
            .and_then(|()| host_in.flush())
            .is_err()
        {
            kill_and_wait(&mut helper);
            kill_and_wait(&mut host);
            return Err(ManagedAgentHostErrorV1::Unavailable);
        }
        let house_startup = plan.house.is_some();
        let helper_to_host = std::thread::spawn(move || {
            let _ = std::io::copy(&mut helper_out, &mut host_in);
        });
        let activity_at_ms = Arc::new(AtomicU64::new(if house_startup {
            0
        } else {
            now_ms().unwrap_or(0)
        }));
        let activity_for_bridge = Arc::clone(&activity_at_ms);
        let host_to_helper = std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            let mut protocol_lines = 0_usize;
            while let Ok(read) = host_out.read(&mut buffer) {
                if read == 0 || helper_in.write_all(&buffer[..read]).is_err() {
                    break;
                }
                for byte in &buffer[..read] {
                    if *byte == b'\n' {
                        protocol_lines = protocol_lines.saturating_add(1);
                    }
                    if protocol_lines >= 2 {
                        break;
                    }
                }
                // For House hosts, line one is `initialize` and line two is the
                // initialized notification. The second line proves the helper
                // response was accepted; a mere child spawn cannot open Lobby.
                if (!house_startup && protocol_lines >= 1) || (house_startup && protocol_lines >= 2)
                {
                    activity_for_bridge.store(now_ms().unwrap_or(0), Ordering::Release);
                }
            }
        });
        Ok(Box::new(OsManagedAgentHostProcessV1 {
            helper,
            host,
            bridges: Some([helper_to_host, host_to_helper]),
            activity_at_ms,
        }))
    }
}

struct OsManagedAgentHostProcessV1 {
    helper: Child,
    host: Child,
    bridges: Option<[std::thread::JoinHandle<()>; 2]>,
    activity_at_ms: Arc<AtomicU64>,
}

impl ManagedAgentHostProcessV1 for OsManagedAgentHostProcessV1 {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        let host = self
            .host
            .try_wait()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let helper = self
            .helper
            .try_wait()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        match (host, helper) {
            (None, None) => Ok(None),
            (Some(status), _) | (_, Some(status)) => {
                let code = status.code().unwrap_or(-1);
                self.stop()?;
                Ok(Some(code))
            }
        }
    }

    fn last_activity_at_ms(&self) -> Option<u64> {
        let value = self.activity_at_ms.load(Ordering::Acquire);
        (value != 0).then_some(value)
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        let _ = self.host.kill();
        let _ = self.helper.kill();
        let host = self.host.wait();
        let helper = self.helper.wait();
        if let Some(bridges) = self.bridges.take() {
            for bridge in bridges {
                let _ = bridge.join();
            }
        }
        if host.is_err() || helper.is_err() {
            Err(ManagedAgentHostErrorV1::Unavailable)
        } else {
            Ok(())
        }
    }
}

impl Drop for OsManagedAgentHostProcessV1 {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Owner-only immutable Profile store for the production MCP process contract.
#[derive(Clone)]
pub struct ManagedAgentHostStoreV1 {
    profiles: Arc<PathBuf>,
    mutation: Arc<Mutex<()>>,
}

impl ManagedAgentHostStoreV1 {
    /// Opens and validates owner-only retained state.
    ///
    /// # Errors
    ///
    /// Fails closed for unsafe storage or corrupt records.
    pub fn open(root: &Path) -> Result<Self, ManagedAgentHostErrorV1> {
        let root =
            prepare_data_directory(root).map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let store = Self {
            profiles: Arc::new(
                prepare_data_directory(&root.join("profiles"))
                    .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?,
            ),
            mutation: Arc::new(Mutex::new(())),
        };
        store.validate_retained()?;
        Ok(store)
    }

    /// Retains one immutable exact host/Profile revision.
    ///
    /// # Errors
    ///
    /// Rejects malformed or changed content under an existing assignment.
    pub fn publish_profile(
        &self,
        profile: &ManagedAgentHostProfileV1,
    ) -> Result<(), ManagedAgentHostErrorV1> {
        validate_profile(profile)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.profile_path(&profile.assignment_id);
        if path.exists() {
            let retained: ManagedAgentHostProfileV1 = read_record(&path)?;
            return if retained == *profile {
                Ok(())
            } else {
                Err(ManagedAgentHostErrorV1::ImmutableProfileConflict)
            };
        }
        persist_record(&self.profiles, &path, profile, false)
    }

    fn profile(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostProfileV1, ManagedAgentHostErrorV1> {
        let profile: ManagedAgentHostProfileV1 = read_record(&self.profile_path(assignment_id))?;
        validate_profile(&profile)?;
        if profile.assignment_id != assignment_id {
            return Err(ManagedAgentHostErrorV1::Corrupt);
        }
        Ok(profile)
    }

    fn profile_path(&self, id: &str) -> PathBuf {
        self.profiles.join(format!("{id}.json"))
    }
    fn validate_retained(&self) -> Result<(), ManagedAgentHostErrorV1> {
        for path in json_files(&self.profiles)? {
            let value: ManagedAgentHostProfileV1 = read_record(&path)?;
            validate_profile(&value)?;
            if path != self.profile_path(&value.assignment_id) {
                return Err(ManagedAgentHostErrorV1::Corrupt);
            }
        }
        Ok(())
    }
}

fn validate_profile(value: &ManagedAgentHostProfileV1) -> Result<(), ManagedAgentHostErrorV1> {
    if value.schema != PROFILE_SCHEMA
        || value.assignment_id.parse::<UlidString>().is_err()
        || !bounded(&value.host_id)
        || !bounded(&value.host_revision)
        || !bounded(&value.provider)
        || !value.provider_address.ip().is_loopback()
        || !bounded_model(&value.model)
        || !is_secret_reference(&value.credential_reference)
        || value.capacity == 0
        || value.capacity > 64
        || value.stale_after_ms == 0
        || value.stale_after_ms > 300_000
    {
        return Err(ManagedAgentHostErrorV1::InvalidInput);
    }
    Ok(())
}

fn canonical_hash(value: &Value) -> Result<String, ManagedAgentHostErrorV1> {
    let raw = serde_json::to_vec(value).map_err(|_| ManagedAgentHostErrorV1::Corrupt)?;
    let canonical = worldstream_core::CanonicalJsonV1::parse(&raw)
        .map_err(|_| ManagedAgentHostErrorV1::Corrupt)?;
    Ok(worldstream_core::Blake3DigestV1::hash(
        &canonical
            .to_bytes()
            .map_err(|_| ManagedAgentHostErrorV1::Corrupt)?,
    )
    .to_string())
}
fn bounded(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}
fn bounded_runner_unit(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
fn bounded_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
}
fn is_secret_reference(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn stable_id(binding: &str, generation: u64, purpose: &str) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let digest = blake3::hash(
        format!("worldstream/managed-agent-host/v1:{binding}:{generation}:{purpose}").as_bytes(),
    );
    let mut number = u128::from_be_bytes(digest.as_bytes()[..16].try_into().unwrap_or([0; 16]));
    let mut output = [b'0'; 26];
    for slot in output.iter_mut().rev() {
        *slot = ALPHABET[(number & 31) as usize];
        number >>= 5;
    }
    String::from_utf8(output.to_vec()).unwrap_or_default()
}
fn json_files(root: &Path) -> Result<Vec<PathBuf>, ManagedAgentHostErrorV1> {
    let mut values = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| ManagedAgentHostErrorV1::Unavailable)? {
        let path = entry
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?
            .path();
        if path.extension().and_then(|item| item.to_str()) != Some("json") {
            return Err(ManagedAgentHostErrorV1::Corrupt);
        }
        values.push(path);
    }
    values.sort();
    Ok(values)
}
fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ManagedAgentHostErrorV1> {
    validate_owner_only_file(path).map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
    if fs::metadata(path)
        .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?
        .len()
        > MAX_RECORD_BYTES
    {
        return Err(ManagedAgentHostErrorV1::Corrupt);
    }
    serde_json::from_slice(&fs::read(path).map_err(|_| ManagedAgentHostErrorV1::Unavailable)?)
        .map_err(|_| ManagedAgentHostErrorV1::Corrupt)
}
fn persist_record<T: Serialize>(
    root: &Path,
    target: &Path,
    value: &T,
    replace: bool,
) -> Result<(), ManagedAgentHostErrorV1> {
    let bytes = serde_json::to_vec(value).map_err(|_| ManagedAgentHostErrorV1::Corrupt)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
        return Err(ManagedAgentHostErrorV1::Corrupt);
    }
    let temporary = root.join(format!(
        ".{}.tmp",
        target
            .file_name()
            .and_then(|item| item.to_str())
            .ok_or(ManagedAgentHostErrorV1::Corrupt)?
    ));
    let _ = fs::remove_file(&temporary);
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
    drop(file);
    if replace && target.exists() {
        replace_file(&temporary, target)
    } else {
        fs::rename(&temporary, target)
    }
    .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
    sync_directory(root).map_err(|_| ManagedAgentHostErrorV1::Unavailable)
}
#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}
#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    use winsafe::{MOVEFILE, prelude::kernel_Hpath};
    source
        .MoveFileEx(target, MOVEFILE::REPLACE_EXISTING | MOVEFILE::WRITE_THROUGH)
        .map_err(std::io::Error::other)
}
#[cfg(not(windows))]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}
#[cfg(windows)]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
    };

    use tempfile::tempdir;
    use worldstream_core::CanonicalJsonV1;
    use worldstream_hosted_contract::HouseAgentRevision;
    use zeroize::Zeroizing;

    use super::{
        ManagedAgentHostErrorV1, ManagedAgentHostLaunchPlanV1, ManagedAgentHostOperationsV1,
        ManagedAgentHostPreparedLaunchV1, ManagedAgentHostProcessLauncherV1,
        ManagedAgentHostProcessV1, ManagedAgentHostStartSourceV1, ManagedReferenceProviderV1,
        now_ms,
    };

    const HOUSE_REVISION: &[u8] =
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-1.json");
    const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const SECRET_REFERENCE: &str =
        "abababababababababababababababababababababababababababababababab";

    #[derive(Clone, Copy)]
    struct UnusedStartSource(ManagedAgentHostErrorV1);

    impl ManagedAgentHostStartSourceV1 for UnusedStartSource {
        fn prepare(
            &self,
            _assignment_id: &str,
        ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1> {
            Err(self.0)
        }
    }

    struct PreparedStartSource(Mutex<Option<ManagedAgentHostPreparedLaunchV1>>);

    impl ManagedAgentHostStartSourceV1 for PreparedStartSource {
        fn prepare(
            &self,
            assignment_id: &str,
        ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1> {
            assert_eq!(assignment_id, ASSIGNMENT);
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .ok_or(ManagedAgentHostErrorV1::Unavailable)
        }
    }

    #[derive(Clone)]
    struct HandshakeProcessLauncher {
        activity_at_ms: Arc<AtomicU64>,
        launches: Arc<AtomicUsize>,
    }

    impl ManagedAgentHostProcessLauncherV1 for HandshakeProcessLauncher {
        fn launch_bridged(
            &self,
            _plan: &ManagedAgentHostLaunchPlanV1,
            credential: &Zeroizing<Vec<u8>>,
        ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
            assert_eq!(credential.as_slice(), &[0xab; 32]);
            self.launches.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(HandshakeProcess {
                activity_at_ms: Arc::clone(&self.activity_at_ms),
            }))
        }
    }

    struct HandshakeProcess {
        activity_at_ms: Arc<AtomicU64>,
    }

    impl ManagedAgentHostProcessV1 for HandshakeProcess {
        fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
            Ok(None)
        }

        fn last_activity_at_ms(&self) -> Option<u64> {
            let value = self.activity_at_ms.load(Ordering::Acquire);
            (value != 0).then_some(value)
        }

        fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
            Ok(())
        }
    }

    #[test]
    fn typed_managed_reference_plan_uses_the_host_cli_value() {
        let plan = ManagedAgentHostLaunchPlanV1::new_managed_reference(
            Path::new("/approved/worldstream-assignment-mcp"),
            Path::new("/owner/worldstream-state"),
            "abababababababababababababababababababababababababababababababab",
            Path::new("/approved/reference-agent-host"),
            ManagedReferenceProviderV1::OpenAiCompatible,
            "127.0.0.1:11434"
                .parse()
                .unwrap_or_else(|error| unreachable!("provider address: {error}")),
            "test-model",
        )
        .unwrap_or_else(|error| unreachable!("managed reference plan: {error:?}"));
        assert_eq!(plan.host_arguments()[3], "openai-compatible");
    }

    #[test]
    fn house_plan_seals_exact_revision_and_credential_in_private_stdin() {
        let bytes = CanonicalJsonV1::parse(HOUSE_REVISION)
            .and_then(|value| value.to_bytes())
            .unwrap_or_else(|error| unreachable!("House revision fixture: {error}"));
        let revision = HouseAgentRevision::from_canonical_bytes(&bytes)
            .unwrap_or_else(|error| unreachable!("House revision fixture: {error}"));
        let plan = ManagedAgentHostLaunchPlanV1::new_house(
            Path::new("/approved/worldstream-assignment-mcp"),
            Path::new("/owner/worldstream-state"),
            "abababababababababababababababababababababababababababababababab",
            Path::new("/approved/worldstream-managed-agent-host"),
            Path::new("/owner/house-unit"),
            "house-unit-01",
            &revision,
        )
        .unwrap_or_else(|error| unreachable!("House plan: {error:?}"));
        let credential = Zeroizing::new(vec![0xab; 32]);
        let frame = plan
            .startup_frame(&credential)
            .unwrap_or_else(|error| unreachable!("House startup frame: {error:?}"));
        let header = format!("WSMHOUSE1 house-unit-01 {} 32\n", bytes.len());
        assert!(frame.starts_with(header.as_bytes()));
        assert_eq!(
            &frame[header.len()..header.len() + bytes.len()],
            bytes.as_slice()
        );
        assert_eq!(&frame[header.len() + bytes.len()..], credential.as_slice());
        assert_eq!(
            plan.host_arguments(),
            ["--transport", "stdio", "--provider", "openrouter-house"]
        );
        assert!(!frame.windows(7).any(|window| window == b"room_id"));
        assert!(!frame.windows(9).any(|window| window == b"member_id"));
    }

    #[cfg(unix)]
    #[test]
    fn retired_house_unit_cannot_spawn_either_child() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let helper = root.join("worldstream-assignment-mcp");
        std::fs::write(&helper, b"#!/bin/sh\nexec /bin/cat\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let host = root.join("managed-house-host");
        // Accept the House CLI arguments and keep stdin open for its startup
        // frame. Calling /bin/cat directly rejects those arguments and races
        // the startup write. Consume input without echoing, while keeping both
        // pipe descriptors open until the launcher stops the children.
        std::fs::write(&host, b"#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bytes = CanonicalJsonV1::parse(HOUSE_REVISION)
            .unwrap()
            .to_bytes()
            .unwrap();
        let revision = HouseAgentRevision::from_canonical_bytes(&bytes).unwrap();
        let plan = ManagedAgentHostLaunchPlanV1::new_house(
            &helper,
            &root,
            SECRET_REFERENCE,
            &host,
            &root,
            "house-unit-01",
            &revision,
        )
        .unwrap();
        let marker = root.join("operator-retired.json");
        let mut active = super::OsManagedAgentHostProcessLauncherV1
            .launch_bridged(&plan, &Zeroizing::new(vec![0xab; 32]))
            .expect("an unfenced House unit can start");
        // Catch a child that accepts the startup write but immediately exits.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(active.try_wait().unwrap(), None);
        active.stop().unwrap();
        // Contents are operator evidence, not executable instructions or authority.
        std::fs::write(&marker, b"{}").unwrap();
        let result = super::OsManagedAgentHostProcessLauncherV1
            .launch_bridged(&plan, &Zeroizing::new(vec![0xab; 32]));
        let rejected = result.is_err();
        if let Ok(mut process) = result {
            process.stop().unwrap();
        }
        assert!(
            rejected,
            "a retained retirement marker must prevent restart"
        );

        // The hosted reconciler uses its own signed evidence filename. The
        // process boundary intentionally treats the marker as a fence, not as
        // executable instructions or a reason to revive a retired unit.
        std::fs::remove_file(&marker).unwrap();
        let automatic = root.join("retired.json");
        std::fs::write(&automatic, b"{}").unwrap();
        assert!(
            super::OsManagedAgentHostProcessLauncherV1
                .launch_bridged(&plan, &Zeroizing::new(vec![0xab; 32]))
                .is_err(),
            "an automatic retirement fence must prevent a late provider response from restarting"
        );

        // The retirement coordinator installs this intent before it asks the
        // managed host to drain. A crash during drain must be just as unable
        // to launch a replacement pair.
        std::fs::remove_file(&automatic).unwrap();
        let intent = root.join("retiring.json");
        std::fs::write(&intent, b"{}").unwrap();
        assert!(
            super::OsManagedAgentHostProcessLauncherV1
                .launch_bridged(&plan, &Zeroizing::new(vec![0xab; 32]))
                .is_err(),
            "a durable retirement intent must prevent spawn before final receipt"
        );

        // A broken link must not be treated as an absent fence.
        std::fs::remove_file(&intent).unwrap();
        std::os::unix::fs::symlink(root.join("missing-receipt"), &marker).unwrap();
        assert!(
            super::OsManagedAgentHostProcessLauncherV1
                .launch_bridged(&plan, &Zeroizing::new(vec![0xab; 32]))
                .is_err()
        );
    }

    #[test]
    fn development_house_plan_passes_only_the_reviewed_loopback_address() {
        let bytes = CanonicalJsonV1::parse(HOUSE_REVISION)
            .and_then(|value| value.to_bytes())
            .unwrap_or_else(|error| unreachable!("House revision fixture: {error}"));
        let revision = HouseAgentRevision::from_canonical_bytes(&bytes)
            .unwrap_or_else(|error| unreachable!("House revision fixture: {error}"));
        let plan = ManagedAgentHostLaunchPlanV1::new_house_development(
            Path::new("/approved/worldstream-assignment-mcp"),
            Path::new("/owner/worldstream-state"),
            SECRET_REFERENCE,
            Path::new("/approved/worldstream-managed-agent-host"),
            Path::new("/owner/house-unit"),
            "house-unit-01",
            &revision,
            "127.0.0.1:8787"
                .parse()
                .unwrap_or_else(|error| unreachable!("provider address: {error}")),
        )
        .unwrap_or_else(|error| unreachable!("development House plan: {error:?}"));
        assert_eq!(
            plan.host_arguments(),
            [
                "--transport",
                "stdio",
                "--provider",
                "openrouter-house",
                "--provider-address",
                "127.0.0.1:8787",
            ]
        );
        assert_eq!(
            ManagedAgentHostLaunchPlanV1::new_house_development(
                Path::new("/approved/worldstream-assignment-mcp"),
                Path::new("/owner/worldstream-state"),
                SECRET_REFERENCE,
                Path::new("/approved/worldstream-managed-agent-host"),
                Path::new("/owner/house-unit"),
                "house-unit-01",
                &revision,
                "192.0.2.1:8787"
                    .parse()
                    .unwrap_or_else(|error| unreachable!("provider address: {error}")),
            )
            .err(),
            Some(ManagedAgentHostErrorV1::InvalidInput)
        );
    }

    #[test]
    fn house_start_waits_for_handshake_and_retry_does_not_launch_again() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
        let root = directory
            .path()
            .canonicalize()
            .unwrap_or_else(|error| unreachable!("canonical root: {error}"));
        let bytes = CanonicalJsonV1::parse(HOUSE_REVISION)
            .and_then(|value| value.to_bytes())
            .unwrap_or_else(|error| unreachable!("House revision fixture: {error}"));
        let revision = HouseAgentRevision::from_canonical_bytes(&bytes)
            .unwrap_or_else(|error| unreachable!("House revision fixture: {error}"));
        let plan = ManagedAgentHostLaunchPlanV1::new_house(
            Path::new("/approved/worldstream-assignment-mcp"),
            Path::new("/owner/worldstream-state"),
            SECRET_REFERENCE,
            Path::new("/approved/worldstream-managed-agent-host"),
            &root,
            "house-unit-01",
            &revision,
        )
        .unwrap_or_else(|error| unreachable!("House plan: {error:?}"));
        let prepared = ManagedAgentHostPreparedLaunchV1::new_house(
            ASSIGNMENT,
            revision.digest(),
            SECRET_REFERENCE,
            plan,
            Zeroizing::new(vec![0xab; 32]),
        )
        .unwrap_or_else(|error| unreachable!("prepared House launch: {error:?}"));
        let activity_at_ms = Arc::new(AtomicU64::new(0));
        let launches = Arc::new(AtomicUsize::new(0));
        let operations = ManagedAgentHostOperationsV1::open_with(
            &root.join("operations"),
            UnusedStartSource(ManagedAgentHostErrorV1::InvalidInput),
            HandshakeProcessLauncher {
                activity_at_ms: Arc::clone(&activity_at_ms),
                launches: Arc::clone(&launches),
            },
        )
        .unwrap_or_else(|error| unreachable!("House operations: {error:?}"));

        let spawned = operations
            .start_house(ASSIGNMENT, &prepared)
            .unwrap_or_else(|error| unreachable!("spawn House host: {error:?}"));
        assert!(!spawned.ready);
        assert_eq!(launches.load(Ordering::SeqCst), 1);

        activity_at_ms.store(
            now_ms().unwrap_or_else(|error| unreachable!("current time: {error:?}")),
            Ordering::Release,
        );
        let handshaken = operations
            .start_house(ASSIGNMENT, &prepared)
            .unwrap_or_else(|error| unreachable!("observe House handshake: {error:?}"));
        assert!(handshaken.ready);
        assert_eq!(launches.load(Ordering::SeqCst), 1);
        let source: Arc<dyn ManagedAgentHostStartSourceV1> =
            Arc::new(PreparedStartSource(Mutex::new(Some(prepared))));
        operations
            .register_house_start_source(&source)
            .unwrap_or_else(|error| unreachable!("register House source: {error:?}"));
        assert_eq!(
            operations.register_house_start_source(&source).err(),
            Some(ManagedAgentHostErrorV1::ImmutableProfileConflict)
        );
        operations
            .stop(ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("stop House: {error:?}"));
        assert!(
            operations.start_permitted(ASSIGNMENT).is_ok(),
            "the lifecycle must restore a retained House host"
        );
        assert_eq!(launches.load(Ordering::SeqCst), 2);
        operations
            .stop(ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("stop restored House: {error:?}"));
        drop(source);
        assert_eq!(
            operations.start_permitted(ASSIGNMENT).err(),
            Some(ManagedAgentHostErrorV1::InvalidInput)
        );
        assert_eq!(launches.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn house_restore_does_not_bypass_primary_authority_or_availability_failures() {
        for error in [
            ManagedAgentHostErrorV1::Unavailable,
            ManagedAgentHostErrorV1::Corrupt,
            ManagedAgentHostErrorV1::ImmutableProfileConflict,
        ] {
            let directory =
                tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
            let launches = Arc::new(AtomicUsize::new(0));
            let root = directory
                .path()
                .canonicalize()
                .unwrap_or_else(|error| unreachable!("canonical root: {error}"));
            let operations = ManagedAgentHostOperationsV1::open_with(
                &root.join("operations"),
                UnusedStartSource(error),
                HandshakeProcessLauncher {
                    activity_at_ms: Arc::new(AtomicU64::new(0)),
                    launches: Arc::clone(&launches),
                },
            )
            .unwrap_or_else(|error| unreachable!("operations: {error:?}"));
            let source: Arc<dyn ManagedAgentHostStartSourceV1> =
                Arc::new(UnusedStartSource(ManagedAgentHostErrorV1::Ambiguous));
            operations
                .register_house_start_source(&source)
                .unwrap_or_else(|error| unreachable!("register: {error:?}"));
            assert_eq!(operations.start_permitted(ASSIGNMENT).err(), Some(error));
            assert_eq!(launches.load(Ordering::SeqCst), 0);
        }
    }
}
