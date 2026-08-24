//! Durable, resumable post-Genesis participant authority provisioning.

use std::{
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_protocol::{
    AccessMode, BearerWireV1, MAX_MESSAGE_BYTES, MemberCapabilityProvisionRequestV1,
    MemberCapabilityProvisionResponseV1, PrincipalKind, RunnerCapabilityProvisionRequestV1,
    RunnerCapabilityProvisionResponseV1, RunnerMembershipProvisionTargetV1,
    SealedCapabilityBearerV1, SealedCapabilityInputV1, UlidString,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::{
    room_creation::{RoomCreationStateV1, RoomCreationSupervisorV1},
    room_drafts::{AgentAssignmentModeV1, RoomDraftSeatV1},
    secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
};

const SETUP_SCHEMA_V1: &str = "worldstream/studio-task-setup-operation/v1";
const SETUP_STATUS_VERSION_V1: &str = "studio_task_setup.v1";
const MAX_OPERATION_BYTES: usize = 256 * 1024;

/// Operator-visible setup lifecycle, independent from Activity Phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSetupStateV1 {
    Waiting,
    Provisioning,
    Ready,
    NeedsAttention,
}

/// Exact effect currently blocked or being reconciled.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskSetupStageV1 {
    MemberCapability { seat_id: String },
    RunnerCapability { seat_id: String },
}

/// Safe operator guidance for one blocked setup stage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSetupAttentionV1 {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapabilityIntentV1 {
    capability_id: String,
    change_id: String,
    secret_reference: SecretReferenceV1,
    provisioned: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RunnerIntentV1 {
    runner_id: String,
    principal_change_id: String,
    runner_change_id: String,
    capability: CapabilityIntentV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupSeatIntentV1 {
    seat_id: String,
    role: String,
    required: bool,
    display_name: String,
    principal_id: Option<String>,
    principal_kind: Option<PrincipalKind>,
    agent_assignment: Option<AgentAssignmentModeV1>,
    member_id: Option<String>,
    member_capability: Option<CapabilityIntentV1>,
    runner: Option<RunnerIntentV1>,
}

/// Owner-only durable intent. Secret references never enter its browser view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskSetupOperationV1 {
    schema: String,
    draft_id: String,
    operation_id: String,
    room_id: String,
    creation_intent_hash: String,
    setup_intent_hash: String,
    checkpoint_hash: String,
    seats: Vec<SetupSeatIntentV1>,
    state: TaskSetupStateV1,
    attempts: u32,
    active_stage: Option<TaskSetupStageV1>,
    attention: Option<TaskSetupAttentionV1>,
}

/// Browser-safe progress for one stable reviewed seat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSetupSeatStatusV1 {
    pub seat_id: String,
    pub role: String,
    pub required: bool,
    pub display_name: String,
    pub principal_id: Option<String>,
    pub principal_kind: Option<PrincipalKind>,
    pub agent_assignment: Option<AgentAssignmentModeV1>,
    pub member_id: Option<String>,
    pub member_authority: String,
    pub runner_authority: String,
}

/// Complete browser-safe status. It contains no bearer, hash, secret
/// reference, local path, daemon request, or raw daemon receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSetupStatusV1 {
    pub version: String,
    pub draft_id: String,
    pub operation_id: String,
    pub room_id: String,
    pub state: TaskSetupStateV1,
    pub attempts: u32,
    pub completed_stages: u32,
    pub total_stages: u32,
    pub active_stage: Option<TaskSetupStageV1>,
    pub attention: Option<TaskSetupAttentionV1>,
    pub seats: Vec<TaskSetupSeatStatusV1>,
}

impl From<TaskSetupOperationV1> for TaskSetupStatusV1 {
    fn from(operation: TaskSetupOperationV1) -> Self {
        let total_stages = operation
            .seats
            .iter()
            .map(|seat| {
                u32::from(seat.member_capability.is_some()) + u32::from(seat.runner.is_some())
            })
            .sum();
        let completed_stages = operation
            .seats
            .iter()
            .map(|seat| {
                u32::from(
                    seat.member_capability
                        .as_ref()
                        .is_some_and(|value| value.provisioned),
                ) + u32::from(
                    seat.runner
                        .as_ref()
                        .is_some_and(|value| value.capability.provisioned),
                )
            })
            .sum();
        let seats = operation
            .seats
            .into_iter()
            .map(|seat| TaskSetupSeatStatusV1 {
                seat_id: seat.seat_id,
                role: seat.role,
                required: seat.required,
                display_name: seat.display_name,
                principal_id: seat.principal_id,
                principal_kind: seat.principal_kind,
                agent_assignment: seat.agent_assignment,
                member_id: seat.member_id,
                member_authority: authority_status(seat.member_capability.as_ref()),
                runner_authority: seat.runner.as_ref().map_or_else(
                    || "not_applicable".to_owned(),
                    |runner| authority_status(Some(&runner.capability)),
                ),
            })
            .collect();
        Self {
            version: SETUP_STATUS_VERSION_V1.to_owned(),
            draft_id: operation.draft_id,
            operation_id: operation.operation_id,
            room_id: operation.room_id,
            state: operation.state,
            attempts: operation.attempts,
            completed_stages,
            total_stages,
            active_stage: operation.active_stage,
            attention: operation.attention,
            seats,
        }
    }
}

fn authority_status(intent: Option<&CapabilityIntentV1>) -> String {
    match intent {
        None => "unfilled_optional".to_owned(),
        Some(intent) if intent.provisioned => "provisioned".to_owned(),
        Some(_) => "pending".to_owned(),
    }
}

/// Closed daemon-attempt classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskSetupAttemptErrorV1 {
    Ambiguous,
    OperatorFixRequired,
    Rejected,
}

/// Narrow production boundary for sealed authority provisioning.
pub trait DaemonTaskSetupProvisionerV1: Send + Sync + 'static {
    /// Provisions or resolves one exact member Capability.
    ///
    /// # Errors
    ///
    /// Returns a closed ambiguous, operator-fix, or rejected result.
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1>;

    /// Provisions or resolves one exact Runner-control Capability.
    ///
    /// # Errors
    ///
    /// Returns a closed ambiguous, operator-fix, or rejected result.
    fn provision_runner(
        &self,
        request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1>;
}

/// Fixed-address authenticated daemon provisioner.
#[derive(Clone)]
pub struct HttpDaemonTaskSetupProvisionerV1 {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
}

impl HttpDaemonTaskSetupProvisionerV1 {
    #[must_use]
    pub const fn new(
        address: SocketAddr,
        timeout: Duration,
        vault: FileSecretVaultV1,
        host_authority: Option<SecretReferenceV1>,
    ) -> Self {
        Self {
            address,
            timeout,
            vault,
            host_authority,
        }
    }

    fn post<T: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R, TaskSetupAttemptErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(TaskSetupAttemptErrorV1::OperatorFixRequired)?;
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| TaskSetupAttemptErrorV1::OperatorFixRequired)?;
        let host_bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| TaskSetupAttemptErrorV1::OperatorFixRequired)?;
        let host_bearer = Zeroizing::new(BearerWireV1::from_bytes(host_bytes).to_wire());
        let body = Zeroizing::new(
            serde_json::to_string(body).map_err(|_| TaskSetupAttemptErrorV1::Rejected)?,
        );
        let request = Zeroizing::new(format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.address,
            host_bearer.as_str(),
            body.len(),
            body.as_str(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| TaskSetupAttemptErrorV1::Ambiguous)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| TaskSetupAttemptErrorV1::Ambiguous)?;
        stream
            .write_all(request.as_bytes())
            .map_err(|_| TaskSetupAttemptErrorV1::Ambiguous)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut response)
            .map_err(|_| TaskSetupAttemptErrorV1::Ambiguous)?;
        if response.len() > MAX_MESSAGE_BYTES {
            return Err(TaskSetupAttemptErrorV1::Ambiguous);
        }
        let (status, body) = parse_http_response(&response)?;
        match status {
            200 => serde_json::from_slice(body).map_err(|_| TaskSetupAttemptErrorV1::Ambiguous),
            401 | 403 => Err(TaskSetupAttemptErrorV1::OperatorFixRequired),
            400 | 404 | 409 | 422 => Err(TaskSetupAttemptErrorV1::Rejected),
            _ => Err(TaskSetupAttemptErrorV1::Ambiguous),
        }
    }
}

impl DaemonTaskSetupProvisionerV1 for HttpDaemonTaskSetupProvisionerV1 {
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        self.post("/v1/operator/member-capabilities:provision", request)
    }

    fn provision_runner(
        &self,
        request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        self.post("/v1/operator/runner-capabilities:provision", request)
    }
}

/// Closed local setup failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TaskSetupErrorV1 {
    #[error("reviewed Room creation is not ready for setup")]
    InvalidCreation,
    #[error("Task setup operation was not found")]
    NotFound,
    #[error("Task setup operation storage is unavailable")]
    Unavailable,
}

/// Durable staged setup supervisor permanently keyed by draft identity.
#[derive(Clone)]
pub struct TaskSetupSupervisorV1 {
    root: Arc<PathBuf>,
    creation: RoomCreationSupervisorV1,
    vault: FileSecretVaultV1,
    provisioner: Arc<dyn DaemonTaskSetupProvisionerV1>,
    mutation: Arc<Mutex<()>>,
}

impl TaskSetupSupervisorV1 {
    /// Opens the protected operation store.
    ///
    /// # Errors
    ///
    /// Fails closed when owner-only persistence is unavailable.
    pub fn open(
        root: &Path,
        creation: RoomCreationSupervisorV1,
        vault: FileSecretVaultV1,
        provisioner: impl DaemonTaskSetupProvisionerV1,
    ) -> Result<Self, TaskSetupErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| TaskSetupErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            creation,
            vault,
            provisioner: Arc::new(provisioner),
            mutation: Arc::new(Mutex::new(())),
        })
    }

    /// Publishes the immutable setup intent before its first daemon effect,
    /// or resumes the operation already bound to this draft.
    ///
    /// # Errors
    ///
    /// Returns a closed creation or protected-storage failure.
    pub fn start(&self, draft_id: &str) -> Result<TaskSetupStatusV1, TaskSetupErrorV1> {
        let _guard = self.lock();
        let operation = match self.load_unlocked(draft_id) {
            Ok(operation) => operation,
            Err(TaskSetupErrorV1::NotFound) => {
                let mut operation = self.prepare(draft_id)?;
                self.persist_new(&mut operation)?;
                operation
            }
            Err(error) => return Err(error),
        };
        self.reconcile_unlocked(operation).map(Into::into)
    }

    /// Safely retries only the original immutable setup operation.
    ///
    /// # Errors
    ///
    /// Returns exact not-found or protected-storage unavailable.
    pub fn retry(&self, draft_id: &str) -> Result<TaskSetupStatusV1, TaskSetupErrorV1> {
        let _guard = self.lock();
        let operation = self.load_unlocked(draft_id)?;
        self.reconcile_unlocked(operation).map(Into::into)
    }

    /// Loads the browser-safe durable setup status without issuing effects.
    ///
    /// # Errors
    ///
    /// Returns exact not-found or protected-storage unavailable.
    pub fn status(&self, draft_id: &str) -> Result<TaskSetupStatusV1, TaskSetupErrorV1> {
        let _guard = self.lock();
        self.load_unlocked(draft_id).map(Into::into)
    }

    fn prepare(&self, draft_id: &str) -> Result<TaskSetupOperationV1, TaskSetupErrorV1> {
        let creation = self
            .creation
            .status(draft_id)
            .map_err(|_| TaskSetupErrorV1::InvalidCreation)?;
        if creation.state != RoomCreationStateV1::Succeeded {
            return Err(TaskSetupErrorV1::InvalidCreation);
        }
        let response = creation
            .response
            .as_ref()
            .ok_or(TaskSetupErrorV1::InvalidCreation)?;
        let filled = creation
            .review
            .seats
            .iter()
            .filter(|seat| seat.principal_id.is_some())
            .collect::<Vec<_>>();
        if filled.len() != response.member_ids.len() {
            return Err(TaskSetupErrorV1::InvalidCreation);
        }
        let mut member_ids = response.member_ids.iter();
        let mut seats = Vec::with_capacity(creation.review.seats.len());
        for seat in &creation.review.seats {
            seats.push(if seat.principal_id.is_some() {
                self.prepare_filled_seat(
                    seat,
                    member_ids
                        .next()
                        .ok_or(TaskSetupErrorV1::InvalidCreation)?
                        .clone(),
                )?
            } else {
                if seat.required {
                    return Err(TaskSetupErrorV1::InvalidCreation);
                }
                SetupSeatIntentV1 {
                    seat_id: seat.seat_id.clone(),
                    role: seat.role.clone(),
                    required: false,
                    display_name: seat.display_name.clone(),
                    principal_id: None,
                    principal_kind: None,
                    agent_assignment: None,
                    member_id: None,
                    member_capability: None,
                    runner: None,
                }
            });
        }
        let operation_id = next_ulid()?;
        let mut operation = TaskSetupOperationV1 {
            schema: SETUP_SCHEMA_V1.to_owned(),
            draft_id: draft_id.to_owned(),
            operation_id,
            room_id: response.room_id.clone(),
            creation_intent_hash: creation.intent_hash,
            setup_intent_hash: String::new(),
            checkpoint_hash: String::new(),
            seats,
            state: TaskSetupStateV1::Waiting,
            attempts: 0,
            active_stage: None,
            attention: None,
        };
        operation.setup_intent_hash = setup_intent_hash(&operation)?;
        operation.checkpoint_hash = checkpoint_hash(&operation)?;
        validate_operation(&operation, draft_id)?;
        Ok(operation)
    }

    fn prepare_filled_seat(
        &self,
        seat: &RoomDraftSeatV1,
        member_id: String,
    ) -> Result<SetupSeatIntentV1, TaskSetupErrorV1> {
        let principal_id = seat
            .principal_id
            .clone()
            .ok_or(TaskSetupErrorV1::InvalidCreation)?;
        let principal_kind = seat
            .principal_kind
            .ok_or(TaskSetupErrorV1::InvalidCreation)?;
        let member_capability = self.new_capability(SecretKindV1::MembershipAuthority)?;
        let runner = if principal_kind == PrincipalKind::Agent {
            seat.agent_assignment
                .ok_or(TaskSetupErrorV1::InvalidCreation)?;
            Some(RunnerIntentV1 {
                runner_id: next_ulid()?,
                principal_change_id: principal_id.clone(),
                runner_change_id: next_ulid()?,
                capability: self.new_capability(SecretKindV1::RunnerAuthority)?,
            })
        } else {
            if seat.agent_assignment.is_some() {
                return Err(TaskSetupErrorV1::InvalidCreation);
            }
            None
        };
        Ok(SetupSeatIntentV1 {
            seat_id: seat.seat_id.clone(),
            role: seat.role.clone(),
            required: seat.required,
            display_name: seat.display_name.clone(),
            principal_id: Some(principal_id),
            principal_kind: Some(principal_kind),
            agent_assignment: seat.agent_assignment,
            member_id: Some(member_id),
            member_capability: Some(member_capability),
            runner,
        })
    }

    fn new_capability(&self, kind: SecretKindV1) -> Result<CapabilityIntentV1, TaskSetupErrorV1> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| TaskSetupErrorV1::Unavailable)?;
        let secret_reference = self
            .vault
            .store(kind, &bytes)
            .map_err(|_| TaskSetupErrorV1::Unavailable)?;
        bytes.fill(0);
        Ok(CapabilityIntentV1 {
            capability_id: next_ulid()?,
            change_id: next_ulid()?,
            secret_reference,
            provisioned: false,
        })
    }

    fn reconcile_unlocked(
        &self,
        mut operation: TaskSetupOperationV1,
    ) -> Result<TaskSetupOperationV1, TaskSetupErrorV1> {
        if operation.state == TaskSetupStateV1::Ready
            || operation
                .attention
                .as_ref()
                .is_some_and(|attention| !attention.retryable)
        {
            return Ok(operation);
        }
        operation.attempts = operation.attempts.saturating_add(1);
        operation.attention = None;
        while let Some(stage) = next_stage(&operation) {
            operation.state = TaskSetupStateV1::Provisioning;
            operation.active_stage = Some(stage.clone());
            self.persist(&mut operation)?;
            let result = self.apply_stage(&operation, &stage);
            match result {
                Ok(()) => {
                    mark_stage_complete(&mut operation, &stage)?;
                    operation.attention = None;
                    if let Some(next) = next_stage(&operation) {
                        operation.state = TaskSetupStateV1::Provisioning;
                        operation.active_stage = Some(next);
                    } else {
                        operation.state = TaskSetupStateV1::Ready;
                        operation.active_stage = None;
                    }
                    self.persist(&mut operation)?;
                }
                Err(error) => {
                    operation.state = TaskSetupStateV1::NeedsAttention;
                    operation.attention = Some(attention_for(error));
                    self.persist(&mut operation)?;
                    return Ok(operation);
                }
            }
        }
        operation.state = TaskSetupStateV1::Ready;
        operation.active_stage = None;
        operation.attention = None;
        self.persist(&mut operation)?;
        Ok(operation)
    }

    fn apply_stage(
        &self,
        operation: &TaskSetupOperationV1,
        stage: &TaskSetupStageV1,
    ) -> Result<(), TaskSetupAttemptErrorV1> {
        let seat = seat_for_stage(operation, stage).ok_or(TaskSetupAttemptErrorV1::Rejected)?;
        match stage {
            TaskSetupStageV1::MemberCapability { .. } => {
                let intent = seat
                    .member_capability
                    .as_ref()
                    .ok_or(TaskSetupAttemptErrorV1::Rejected)?;
                let bearer = self
                    .resolve_bearer(SecretKindV1::MembershipAuthority, &intent.secret_reference)?;
                let request = MemberCapabilityProvisionRequestV1 {
                    room_id: operation.room_id.clone(),
                    member_id: seat
                        .member_id
                        .clone()
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?,
                    principal_id: seat
                        .principal_id
                        .clone()
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?,
                    principal_kind: seat
                        .principal_kind
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?,
                    role: seat.role.clone(),
                    access_mode: AccessMode::Participant,
                    scopes: vec![
                        "room:attach".to_owned(),
                        "room:act".to_owned(),
                        "room:observe_member".to_owned(),
                    ],
                    capability: SealedCapabilityInputV1 {
                        capability_id: intent.capability_id.clone(),
                        capability_idempotency_key: intent.change_id.clone(),
                        bearer,
                    },
                    expires_at: None,
                };
                let receipt = self.provisioner.provision_member(&request)?;
                if receipt.capability_id != intent.capability_id
                    || receipt.room_id != operation.room_id
                    || receipt.member_id != request.member_id
                    || receipt.principal_id != request.principal_id
                {
                    return Err(TaskSetupAttemptErrorV1::Ambiguous);
                }
            }
            TaskSetupStageV1::RunnerCapability { .. } => {
                let runner = seat
                    .runner
                    .as_ref()
                    .ok_or(TaskSetupAttemptErrorV1::Rejected)?;
                let bearer = self.resolve_bearer(
                    SecretKindV1::RunnerAuthority,
                    &runner.capability.secret_reference,
                )?;
                let request = RunnerCapabilityProvisionRequestV1 {
                    runner_id: runner.runner_id.clone(),
                    owner_principal_id: seat
                        .principal_id
                        .clone()
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?,
                    permitted_memberships: vec![RunnerMembershipProvisionTargetV1 {
                        room_id: operation.room_id.clone(),
                        member_id: seat
                            .member_id
                            .clone()
                            .ok_or(TaskSetupAttemptErrorV1::Rejected)?,
                    }],
                    scopes: vec![
                        "activation:offer_receive".to_owned(),
                        "activation:claim".to_owned(),
                        "activation:complete".to_owned(),
                    ],
                    principal_idempotency_key: runner.principal_change_id.clone(),
                    runner_idempotency_key: runner.runner_change_id.clone(),
                    capability: SealedCapabilityInputV1 {
                        capability_id: runner.capability.capability_id.clone(),
                        capability_idempotency_key: runner.capability.change_id.clone(),
                        bearer,
                    },
                    expires_at: None,
                };
                let receipt = self.provisioner.provision_runner(&request)?;
                if receipt.capability_id != runner.capability.capability_id
                    || receipt.runner_id != runner.runner_id
                    || receipt.owner_principal_id != request.owner_principal_id
                    || receipt.permitted_memberships != request.permitted_memberships
                {
                    return Err(TaskSetupAttemptErrorV1::Ambiguous);
                }
            }
        }
        Ok(())
    }

    fn resolve_bearer(
        &self,
        kind: SecretKindV1,
        reference: &SecretReferenceV1,
    ) -> Result<SealedCapabilityBearerV1, TaskSetupAttemptErrorV1> {
        let secret = self
            .vault
            .resolve(kind, reference)
            .map_err(|_| TaskSetupAttemptErrorV1::OperatorFixRequired)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| TaskSetupAttemptErrorV1::OperatorFixRequired)?;
        Ok(SealedCapabilityBearerV1::from_wire(
            &BearerWireV1::from_bytes(bytes),
        ))
    }

    fn load_unlocked(&self, draft_id: &str) -> Result<TaskSetupOperationV1, TaskSetupErrorV1> {
        validate_draft_id(draft_id)?;
        let path = self.root.join(format!("{draft_id}.json"));
        if !path.exists() {
            return Err(TaskSetupErrorV1::NotFound);
        }
        validate_owner_only_file(&path).map_err(|_| TaskSetupErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| TaskSetupErrorV1::Unavailable)?;
        if metadata.len() > u64::try_from(MAX_OPERATION_BYTES).unwrap_or(u64::MAX) {
            return Err(TaskSetupErrorV1::Unavailable);
        }
        let operation: TaskSetupOperationV1 =
            serde_json::from_slice(&fs::read(path).map_err(|_| TaskSetupErrorV1::Unavailable)?)
                .map_err(|_| TaskSetupErrorV1::Unavailable)?;
        validate_operation(&operation, draft_id)?;
        Ok(operation)
    }

    fn persist(&self, operation: &mut TaskSetupOperationV1) -> Result<(), TaskSetupErrorV1> {
        operation.checkpoint_hash = checkpoint_hash(operation)?;
        let (temporary, target) = self.write_temporary(operation)?;
        let result = fs::rename(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| TaskSetupErrorV1::Unavailable);
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn persist_new(&self, operation: &mut TaskSetupOperationV1) -> Result<(), TaskSetupErrorV1> {
        operation.checkpoint_hash = checkpoint_hash(operation)?;
        let (temporary, target) = self.write_temporary(operation)?;
        let result = fs::hard_link(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| TaskSetupErrorV1::Unavailable);
        let _ = fs::remove_file(temporary);
        result
    }

    fn write_temporary(
        &self,
        operation: &TaskSetupOperationV1,
    ) -> Result<(PathBuf, PathBuf), TaskSetupErrorV1> {
        validate_operation(operation, &operation.draft_id)?;
        let bytes =
            serde_json::to_vec_pretty(operation).map_err(|_| TaskSetupErrorV1::Unavailable)?;
        if bytes.len() > MAX_OPERATION_BYTES {
            return Err(TaskSetupErrorV1::Unavailable);
        }
        let target = self.root.join(format!("{}.json", operation.draft_id));
        let temporary = self
            .root
            .join(format!(".{}.{}.tmp", operation.draft_id, next_ulid()?));
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| TaskSetupErrorV1::Unavailable)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| TaskSetupErrorV1::Unavailable)?;
        Ok((temporary, target))
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the bounded setup status/start/retry API.
pub fn task_setup_router(supervisor: TaskSetupSupervisorV1) -> Router {
    Router::new()
        .route("/api/v1/task-setups/{draft_id}", get(setup_status))
        .route("/api/v1/task-setups/{draft_id}:start", post(start_setup))
        .route("/api/v1/task-setups/{draft_id}:retry", post(retry_setup))
        .with_state(supervisor)
}

async fn setup_status(
    State(supervisor): State<TaskSetupSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<TaskSetupStatusV1>, TaskSetupErrorV1> {
    let status = tokio::task::spawn_blocking(move || supervisor.status(&draft_id))
        .await
        .map_err(|_| TaskSetupErrorV1::Unavailable)??;
    Ok(Json(status))
}

async fn start_setup(
    State(supervisor): State<TaskSetupSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<TaskSetupStatusV1>, TaskSetupErrorV1> {
    let status = tokio::task::spawn_blocking(move || supervisor.start(&draft_id))
        .await
        .map_err(|_| TaskSetupErrorV1::Unavailable)??;
    Ok(Json(status))
}

async fn retry_setup(
    State(supervisor): State<TaskSetupSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<TaskSetupStatusV1>, TaskSetupErrorV1> {
    let status = tokio::task::spawn_blocking(move || supervisor.retry(&draft_id))
        .await
        .map_err(|_| TaskSetupErrorV1::Unavailable)??;
    Ok(Json(status))
}

impl IntoResponse for TaskSetupErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, message, retryable) = match self {
            Self::InvalidCreation => (
                StatusCode::CONFLICT,
                "task_setup_creation_not_ready",
                "the reviewed Room creation is not ready for setup",
                false,
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "task_setup_not_found",
                "the Task setup operation is unavailable",
                false,
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "task_setup_store_unavailable",
                "the protected Task setup store is unavailable",
                true,
            ),
        };
        (
            status,
            Json(serde_json::json!({
                "error": { "code": code, "message": message, "retryable": retryable }
            })),
        )
            .into_response()
    }
}

fn next_stage(operation: &TaskSetupOperationV1) -> Option<TaskSetupStageV1> {
    for seat in &operation.seats {
        if seat
            .member_capability
            .as_ref()
            .is_some_and(|value| !value.provisioned)
        {
            return Some(TaskSetupStageV1::MemberCapability {
                seat_id: seat.seat_id.clone(),
            });
        }
        if seat
            .runner
            .as_ref()
            .is_some_and(|value| !value.capability.provisioned)
        {
            return Some(TaskSetupStageV1::RunnerCapability {
                seat_id: seat.seat_id.clone(),
            });
        }
    }
    None
}

fn seat_for_stage<'a>(
    operation: &'a TaskSetupOperationV1,
    stage: &TaskSetupStageV1,
) -> Option<&'a SetupSeatIntentV1> {
    let seat_id = match stage {
        TaskSetupStageV1::MemberCapability { seat_id }
        | TaskSetupStageV1::RunnerCapability { seat_id } => seat_id,
    };
    operation.seats.iter().find(|seat| &seat.seat_id == seat_id)
}

fn mark_stage_complete(
    operation: &mut TaskSetupOperationV1,
    stage: &TaskSetupStageV1,
) -> Result<(), TaskSetupErrorV1> {
    let seat_id = match stage {
        TaskSetupStageV1::MemberCapability { seat_id }
        | TaskSetupStageV1::RunnerCapability { seat_id } => seat_id,
    };
    let seat = operation
        .seats
        .iter_mut()
        .find(|seat| &seat.seat_id == seat_id)
        .ok_or(TaskSetupErrorV1::Unavailable)?;
    match stage {
        TaskSetupStageV1::MemberCapability { .. } => {
            seat.member_capability
                .as_mut()
                .ok_or(TaskSetupErrorV1::Unavailable)?
                .provisioned = true;
        }
        TaskSetupStageV1::RunnerCapability { .. } => {
            seat.runner
                .as_mut()
                .ok_or(TaskSetupErrorV1::Unavailable)?
                .capability
                .provisioned = true;
        }
    }
    Ok(())
}

fn attention_for(error: TaskSetupAttemptErrorV1) -> TaskSetupAttentionV1 {
    match error {
        TaskSetupAttemptErrorV1::Ambiguous => TaskSetupAttentionV1 {
            code: "daemon_result_ambiguous".to_owned(),
            message: "Retry the original setup operation to reconcile this exact stage.".to_owned(),
            retryable: true,
        },
        TaskSetupAttemptErrorV1::OperatorFixRequired => TaskSetupAttentionV1 {
            code: "setup_credential_unavailable".to_owned(),
            message: "Repair the protected credential configuration, then retry the original setup operation.".to_owned(),
            retryable: true,
        },
        TaskSetupAttemptErrorV1::Rejected => TaskSetupAttentionV1 {
            code: "setup_stage_rejected".to_owned(),
            message: "The exact setup stage was rejected and cannot be replaced automatically.".to_owned(),
            retryable: false,
        },
    }
}

fn validate_operation(
    operation: &TaskSetupOperationV1,
    draft_id: &str,
) -> Result<(), TaskSetupErrorV1> {
    if operation.schema != SETUP_SCHEMA_V1
        || operation.draft_id != draft_id
        || operation.operation_id.parse::<UlidString>().is_err()
        || operation.room_id.parse::<UlidString>().is_err()
        || !is_digest(&operation.creation_intent_hash)
        || setup_intent_hash(operation)? != operation.setup_intent_hash
        || checkpoint_hash(operation)? != operation.checkpoint_hash
        || operation.seats.len() > 64
        || !state_is_coherent(operation)
        || !progress_is_a_prefix(operation)
    {
        return Err(TaskSetupErrorV1::Unavailable);
    }
    for seat in &operation.seats {
        if seat.principal_id.is_none() {
            if seat.required
                || seat.principal_kind.is_some()
                || seat.member_id.is_some()
                || seat.member_capability.is_some()
                || seat.runner.is_some()
            {
                return Err(TaskSetupErrorV1::Unavailable);
            }
            continue;
        }
        if seat
            .principal_id
            .as_deref()
            .and_then(|value| value.parse::<UlidString>().ok())
            .is_none()
            || seat
                .member_id
                .as_deref()
                .and_then(|value| value.parse::<UlidString>().ok())
                .is_none()
            || seat.member_capability.as_ref().is_none()
            || (seat.principal_kind == Some(PrincipalKind::Agent)) != seat.runner.is_some()
            || (seat.principal_kind == Some(PrincipalKind::Agent))
                != seat.agent_assignment.is_some()
        {
            return Err(TaskSetupErrorV1::Unavailable);
        }
        let member = seat
            .member_capability
            .as_ref()
            .ok_or(TaskSetupErrorV1::Unavailable)?;
        if !is_ulid(&member.capability_id) || !is_ulid(&member.change_id) {
            return Err(TaskSetupErrorV1::Unavailable);
        }
        if let Some(runner) = &seat.runner
            && (!is_ulid(&runner.runner_id)
                || runner.principal_change_id != seat.principal_id.as_deref().unwrap_or_default()
                || !is_ulid(&runner.principal_change_id)
                || !is_ulid(&runner.runner_change_id)
                || !is_ulid(&runner.capability.capability_id)
                || !is_ulid(&runner.capability.change_id))
        {
            return Err(TaskSetupErrorV1::Unavailable);
        }
    }
    Ok(())
}

fn state_is_coherent(operation: &TaskSetupOperationV1) -> bool {
    let next = next_stage(operation);
    match operation.state {
        TaskSetupStateV1::Waiting => {
            operation.attempts == 0
                && next.is_some()
                && operation.active_stage.is_none()
                && operation.attention.is_none()
        }
        TaskSetupStateV1::Provisioning => {
            next.is_some()
                && operation.active_stage == next
                && operation.attention.is_none()
                && operation.attempts > 0
        }
        TaskSetupStateV1::Ready => {
            next.is_none()
                && operation.active_stage.is_none()
                && operation.attention.is_none()
                && operation.attempts > 0
        }
        TaskSetupStateV1::NeedsAttention => {
            next.is_some()
                && operation.active_stage == next
                && operation.attempts > 0
                && operation.attention.as_ref().is_some_and(|attention| {
                    [
                        TaskSetupAttemptErrorV1::Ambiguous,
                        TaskSetupAttemptErrorV1::OperatorFixRequired,
                        TaskSetupAttemptErrorV1::Rejected,
                    ]
                    .into_iter()
                    .any(|error| attention == &attention_for(error))
                })
        }
    }
}

fn progress_is_a_prefix(operation: &TaskSetupOperationV1) -> bool {
    let mut pending_seen = false;
    for provisioned in operation.seats.iter().flat_map(|seat| {
        seat.member_capability
            .iter()
            .map(|intent| intent.provisioned)
            .chain(
                seat.runner
                    .iter()
                    .map(|runner| runner.capability.provisioned),
            )
    }) {
        if provisioned && pending_seen {
            return false;
        }
        pending_seen |= !provisioned;
    }
    true
}

fn setup_intent_hash(operation: &TaskSetupOperationV1) -> Result<String, TaskSetupErrorV1> {
    #[derive(Serialize)]
    struct Intent<'a> {
        draft_id: &'a str,
        operation_id: &'a str,
        room_id: &'a str,
        creation_intent_hash: &'a str,
        seats: Vec<IntentSeat<'a>>,
    }
    #[derive(Serialize)]
    struct IntentSeat<'a> {
        seat_id: &'a str,
        role: &'a str,
        required: bool,
        display_name: &'a str,
        principal_id: Option<&'a str>,
        principal_kind: Option<PrincipalKind>,
        agent_assignment: Option<AgentAssignmentModeV1>,
        member_id: Option<&'a str>,
        member_capability_id: Option<&'a str>,
        member_change_id: Option<&'a str>,
        member_secret_reference: Option<&'a str>,
        runner_id: Option<&'a str>,
        runner_principal_change_id: Option<&'a str>,
        runner_change_id: Option<&'a str>,
        runner_capability_id: Option<&'a str>,
        runner_capability_change_id: Option<&'a str>,
        runner_secret_reference: Option<&'a str>,
    }
    let seats = operation
        .seats
        .iter()
        .map(|seat| IntentSeat {
            seat_id: &seat.seat_id,
            role: &seat.role,
            required: seat.required,
            display_name: &seat.display_name,
            principal_id: seat.principal_id.as_deref(),
            principal_kind: seat.principal_kind,
            agent_assignment: seat.agent_assignment,
            member_id: seat.member_id.as_deref(),
            member_capability_id: seat
                .member_capability
                .as_ref()
                .map(|value| value.capability_id.as_str()),
            member_change_id: seat
                .member_capability
                .as_ref()
                .map(|value| value.change_id.as_str()),
            member_secret_reference: seat
                .member_capability
                .as_ref()
                .map(|value| value.secret_reference.as_str()),
            runner_id: seat.runner.as_ref().map(|value| value.runner_id.as_str()),
            runner_principal_change_id: seat
                .runner
                .as_ref()
                .map(|value| value.principal_change_id.as_str()),
            runner_change_id: seat
                .runner
                .as_ref()
                .map(|value| value.runner_change_id.as_str()),
            runner_capability_id: seat
                .runner
                .as_ref()
                .map(|value| value.capability.capability_id.as_str()),
            runner_capability_change_id: seat
                .runner
                .as_ref()
                .map(|value| value.capability.change_id.as_str()),
            runner_secret_reference: seat
                .runner
                .as_ref()
                .map(|value| value.capability.secret_reference.as_str()),
        })
        .collect();
    let bytes = serde_json::to_vec(&Intent {
        draft_id: &operation.draft_id,
        operation_id: &operation.operation_id,
        room_id: &operation.room_id,
        creation_intent_hash: &operation.creation_intent_hash,
        seats,
    })
    .map_err(|_| TaskSetupErrorV1::Unavailable)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn checkpoint_hash(operation: &TaskSetupOperationV1) -> Result<String, TaskSetupErrorV1> {
    let mut value = serde_json::to_value(operation).map_err(|_| TaskSetupErrorV1::Unavailable)?;
    let fields = value.as_object_mut().ok_or(TaskSetupErrorV1::Unavailable)?;
    fields.insert(
        "checkpoint_hash".to_owned(),
        serde_json::Value::String(String::new()),
    );
    let bytes = serde_json::to_vec(&value).map_err(|_| TaskSetupErrorV1::Unavailable)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn is_ulid(value: &str) -> bool {
    value.parse::<UlidString>().is_ok()
}

fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_draft_id(value: &str) -> Result<(), TaskSetupErrorV1> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(TaskSetupErrorV1::InvalidCreation);
    }
    Ok(())
}

fn next_ulid() -> Result<String, TaskSetupErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| TaskSetupErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).map_err(|_| TaskSetupErrorV1::Unavailable)
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, &[u8]), TaskSetupAttemptErrorV1> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(TaskSetupAttemptErrorV1::Ambiguous)?;
    let headers =
        std::str::from_utf8(&bytes[..separator]).map_err(|_| TaskSetupAttemptErrorV1::Ambiguous)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(TaskSetupAttemptErrorV1::Ambiguous)?;
    Ok((status, &bytes[(separator + 4)..]))
}
