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
        Arc, Mutex, PoisonError,
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
    task_setup::{TaskSetupStateV1, TaskSetupSupervisorV1},
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
            || !bounded(model)
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
    }
}

#[derive(Clone)]
pub struct ManagedAgentHostOperationsV1 {
    root: Arc<PathBuf>,
    profiles: ManagedAgentHostStoreV1,
    source: Arc<dyn ManagedAgentHostStartSourceV1>,
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
        let _admission = {
            let admission = self
                .starts_paused
                .try_read()
                .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
            if *admission {
                return Err(ManagedAgentHostErrorV1::Unavailable);
            }
            admission
        };
        self.start_permitted(assignment_id)
    }

    // The lifecycle coordinator holds the exclusive start gate while restoring.
    // All HTTP/public start paths must use `start`, including retry aliases.
    pub(crate) fn start_permitted(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostStatusV1, ManagedAgentHostErrorV1> {
        if assignment_id.parse::<UlidString>().is_err() {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        if self
            .processes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(assignment_id)
        {
            return self.status_locked(assignment_id);
        }
        let prepared = self.source.prepare(assignment_id)?;
        self.profiles.publish_profile(&prepared.profile)?;
        if self.active_for_host(&prepared.profile.host_id)? >= prepared.profile.capacity {
            return Err(ManagedAgentHostErrorV1::AtCapacity);
        }
        let binding_hash = launch_binding_hash(&prepared)?;
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
                self.processes
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(assignment_id.to_owned(), process);
                operation.state = ManagedAgentHostStateV1::Running;
                operation.observed_at_ms = Some(now_ms()?);
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
        if model_credential.is_empty() || model_credential.len() > 16 * 1024 {
            return Err(ManagedAgentHostErrorV1::InvalidInput);
        }
        let mut helper = Command::new(plan.helper_program())
            .args(plan.helper_arguments())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ManagedAgentHostErrorV1::Unavailable)?;
        let Ok(mut host) = Command::new(plan.host_program())
            .args(plan.host_arguments())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
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
        let mut credential_frame = Zeroizing::new(String::from("WSMCRED1 "));
        for byte in model_credential.iter() {
            use fmt::Write as _;
            if write!(credential_frame, "{byte:02x}").is_err() {
                kill_and_wait(&mut helper);
                kill_and_wait(&mut host);
                return Err(ManagedAgentHostErrorV1::Unavailable);
            }
        }
        credential_frame.push('\n');
        if host_in
            .write_all(credential_frame.as_bytes())
            .and_then(|()| host_in.flush())
            .is_err()
        {
            kill_and_wait(&mut helper);
            kill_and_wait(&mut host);
            return Err(ManagedAgentHostErrorV1::Unavailable);
        }
        let helper_to_host = std::thread::spawn(move || {
            let _ = std::io::copy(&mut helper_out, &mut host_in);
        });
        let activity_at_ms = Arc::new(AtomicU64::new(now_ms().unwrap_or(0)));
        let activity_for_bridge = Arc::clone(&activity_at_ms);
        let host_to_helper = std::thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            while let Ok(read) = host_out.read(&mut buffer) {
                if read == 0 || helper_in.write_all(&buffer[..read]).is_err() {
                    break;
                }
                if buffer[..read].contains(&b'\n') {
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
        || !bounded(&value.model)
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
    use std::path::Path;

    use super::{ManagedAgentHostLaunchPlanV1, ManagedReferenceProviderV1};

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
}
