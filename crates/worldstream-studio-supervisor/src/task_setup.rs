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
    AccessMode, BearerWireV1, LobbyLaunchRequest, LobbyLaunchResponse, MAX_MESSAGE_BYTES,
    MemberCapabilityProvisionRequestV1, MemberCapabilityProvisionResponseV1, OperatorRoomSummary,
    OperatorRunnerPresenceV1, PackReference, PrincipalKind, RoomHead,
    RunnerCapabilityProvisionRequestV1, RunnerCapabilityProvisionResponseV1,
    RunnerMembershipProvisionTargetV1, SealedCapabilityBearerV1, SealedCapabilityInputV1,
    UlidString,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::{
    agent_profiles::{
        AgentExecutionBindingV1, AgentProfileErrorV1, AgentProfileMembershipBindingV1,
        AgentProfileSeatAssignmentV1, AgentProfileStoreV1,
    },
    assignment_mcp::{
        AssignedMembershipAuthorityV1, AssignedMembershipLaunchBindingV1,
        AssignedMembershipLaunchSourceV1, AssignedMembershipSourceErrorV1,
        AssignedMembershipSourceV1,
    },
    participant_handoff::{
        HumanSeatAuthorityV1, ParticipantConsoleReadinessSourceV1,
        ParticipantConsoleSessionHealthV1, ParticipantHandoffAuthorityErrorV1,
        ParticipantHandoffAuthoritySourceV1,
    },
    room_creation::{RoomCreationStateV1, RoomCreationSupervisorV1},
    room_drafts::{
        AgentAssignmentModeV1, AgentProfileRevisionReferenceV1, RoomDraftSeatV1,
        RunnerTemplateRevisionReferenceV1,
    },
    runner_templates::{
        ManagedRunnerBindingErrorV1, RunnerFreshnessV1, RunnerInstanceHealthV1,
        RunnerInstanceStateV1, RunnerInstanceStatusV1, RunnerSupervisorV1,
    },
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
    managed_assignment: Option<ManagedRunnerAssignmentV1>,
    capability: CapabilityIntentV1,
}

/// Immutable owner-selected local Runner assignment for a managed Agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedRunnerAssignmentV1 {
    pub instance_id: String,
    pub template_id: String,
    pub template_revision: String,
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
    agent_profile: Option<AgentProfileRevisionReferenceV1>,
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
    pack: PackReference,
    creation_intent_hash: String,
    setup_intent_hash: String,
    checkpoint_hash: String,
    seats: Vec<SetupSeatIntentV1>,
    state: TaskSetupStateV1,
    attempts: u32,
    active_stage: Option<TaskSetupStageV1>,
    attention: Option<TaskSetupAttentionV1>,
    launch: Option<TaskLaunchIntentV1>,
}

/// Closed browser-safe reason why one seat is or is not live-ready.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSeatReadinessReasonV1 {
    Ready,
    OptionalUnfilled,
    SetupIncomplete,
    ConsoleMissing,
    ConsoleStale,
    ConsoleInvalid,
    ConsoleDisconnected,
    RunnerAssignmentMissing,
    RunnerMissing,
    RunnerStale,
    RunnerDisconnected,
    RunnerOverCapacity,
    RunnerIncompatible,
}

/// One exact seat's live launch-gate result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSeatReadinessV1 {
    pub seat_id: String,
    pub required: bool,
    pub ready: bool,
    pub reason: TaskSeatReadinessReasonV1,
}

/// Browser-safe aggregate launch gate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskReadinessV1 {
    pub ready_to_launch: bool,
    pub seats: Vec<TaskSeatReadinessV1>,
}

/// Durable launch reconciliation state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLaunchStateV1 {
    Waiting,
    Retrying,
    Reconciling,
    Launched,
    NeedsAttention,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskLaunchIntentV1 {
    input_id: String,
    based_on_room_seq: u64,
    state: TaskLaunchStateV1,
    attempts: u32,
    response: Option<LobbyLaunchResponse>,
    attention: Option<TaskSetupAttentionV1>,
}

/// Browser-safe durable launch status. The immutable daemon request remains local.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskLaunchStatusV1 {
    pub state: TaskLaunchStateV1,
    pub attempts: u32,
    pub attention: Option<TaskSetupAttentionV1>,
    pub transition_id: Option<String>,
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
    pub agent_profile: Option<AgentProfileRevisionReferenceV1>,
    pub managed_runner: Option<ManagedRunnerAssignmentV1>,
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
    pub readiness: TaskReadinessV1,
    pub launch: Option<TaskLaunchStatusV1>,
}

impl TaskSetupStatusV1 {
    fn from_operation(operation: TaskSetupOperationV1, readiness: TaskReadinessV1) -> Self {
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
                agent_profile: seat.agent_profile,
                managed_runner: seat
                    .runner
                    .as_ref()
                    .and_then(|runner| runner.managed_assignment.clone()),
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
            readiness,
            launch: operation.launch.map(|launch| TaskLaunchStatusV1 {
                state: launch.state,
                attempts: launch.attempts,
                attention: launch.attention,
                transition_id: launch.response.map(|response| response.transition_id),
            }),
        }
    }
}

/// Closed live Runner observation. It contains no authority or transport details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskRunnerObservationV1 {
    Present(OperatorRunnerPresenceV1),
    Missing,
    Unavailable,
}

/// Live Runner presence source for one exact provisioned Runner identity.
pub trait TaskRunnerReadinessSourceV1: Send + Sync + 'static {
    fn presence(&self, runner_id: &str) -> TaskRunnerObservationV1;

    fn select_managed(
        &self,
        _pack: &PackReference,
        _requested: Option<&RunnerTemplateRevisionReferenceV1>,
    ) -> Option<ManagedRunnerAssignmentV1> {
        None
    }

    fn managed_reason(
        &self,
        _assignment: &ManagedRunnerAssignmentV1,
        _pack: &PackReference,
    ) -> TaskSeatReadinessReasonV1 {
        TaskSeatReadinessReasonV1::RunnerAssignmentMissing
    }

    /// Binds an exact provisioned Runner identity and retained authority to
    /// the selected owner-managed instance before setup can complete.
    ///
    /// # Errors
    ///
    /// Returns a closed retryable or terminal setup classification.
    fn bind_managed(
        &self,
        _assignment: &ManagedRunnerAssignmentV1,
        _pack: &PackReference,
        _runner_id: &str,
        _authority: &SecretReferenceV1,
    ) -> Result<(), TaskSetupAttemptErrorV1> {
        Err(TaskSetupAttemptErrorV1::Rejected)
    }
}

/// Closed daemon result for launch/reconciliation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskLaunchAttemptErrorV1 {
    Ambiguous,
    OperatorFixRequired,
    Rejected,
}

/// Authenticated daemon boundary for Lobby launch and committed-head observation.
pub trait DaemonTaskLaunchSourceV1: Send + Sync + 'static {
    /// Records or resolves one exact Lobby launch.
    ///
    /// # Errors
    ///
    /// Returns a closed ambiguous, operator-fix, or rejection result.
    fn launch(
        &self,
        room_id: &str,
        request: &LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, TaskLaunchAttemptErrorV1>;

    /// Observes the current committed Room head.
    ///
    /// # Errors
    ///
    /// Returns a closed ambiguous, operator-fix, or rejection result.
    fn room_head(&self, room_id: &str) -> Result<RoomHead, TaskLaunchAttemptErrorV1>;
}

/// Fixed-address authenticated daemon client for live Runner evidence and Lobby launch.
#[derive(Clone)]
pub struct HttpDaemonTaskRuntimeV1 {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
}

impl HttpDaemonTaskRuntimeV1 {
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

    fn request<R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<(u16, Option<R>), TaskLaunchAttemptErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(TaskLaunchAttemptErrorV1::OperatorFixRequired)?;
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| TaskLaunchAttemptErrorV1::OperatorFixRequired)?;
        let host_bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| TaskLaunchAttemptErrorV1::OperatorFixRequired)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(host_bytes).to_wire());
        let body = Zeroizing::new(match body {
            Some(body) => {
                serde_json::to_string(body).map_err(|_| TaskLaunchAttemptErrorV1::Rejected)?
            }
            None => String::new(),
        });
        let request = Zeroizing::new(format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.address,
            bearer.as_str(),
            body.len(),
            body.as_str(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| TaskLaunchAttemptErrorV1::Ambiguous)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| TaskLaunchAttemptErrorV1::Ambiguous)?;
        stream
            .write_all(request.as_bytes())
            .map_err(|_| TaskLaunchAttemptErrorV1::Ambiguous)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut response)
            .map_err(|_| TaskLaunchAttemptErrorV1::Ambiguous)?;
        if response.len() > MAX_MESSAGE_BYTES {
            return Err(TaskLaunchAttemptErrorV1::Ambiguous);
        }
        let (status, body) =
            parse_http_response(&response).map_err(|_| TaskLaunchAttemptErrorV1::Ambiguous)?;
        let parsed = if status == 200 {
            Some(serde_json::from_slice(body).map_err(|_| TaskLaunchAttemptErrorV1::Ambiguous)?)
        } else {
            None
        };
        Ok((status, parsed))
    }

    fn runner_presence(&self, runner_id: &str) -> TaskRunnerObservationV1 {
        if !is_ulid(runner_id) {
            return TaskRunnerObservationV1::Missing;
        }
        match self.request::<OperatorRunnerPresenceV1>(
            "GET",
            &format!("/v1/operator/runners/{runner_id}/presence"),
            None::<&serde_json::Value>,
        ) {
            Ok((200, Some(presence))) => TaskRunnerObservationV1::Present(presence),
            Ok((404, _)) => TaskRunnerObservationV1::Missing,
            _ => TaskRunnerObservationV1::Unavailable,
        }
    }
}

impl DaemonTaskLaunchSourceV1 for HttpDaemonTaskRuntimeV1 {
    fn launch(
        &self,
        room_id: &str,
        request: &LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, TaskLaunchAttemptErrorV1> {
        if !is_ulid(room_id) {
            return Err(TaskLaunchAttemptErrorV1::Rejected);
        }
        let (status, response) = self.request(
            "POST",
            &format!("/v1/operator/rooms/{room_id}/lobby/launch"),
            Some(request),
        )?;
        match (status, response) {
            (200, Some(response)) => Ok(response),
            (401 | 403, _) => Err(TaskLaunchAttemptErrorV1::OperatorFixRequired),
            (400 | 404 | 409 | 422, _) => Err(TaskLaunchAttemptErrorV1::Rejected),
            _ => Err(TaskLaunchAttemptErrorV1::Ambiguous),
        }
    }

    fn room_head(&self, room_id: &str) -> Result<RoomHead, TaskLaunchAttemptErrorV1> {
        if !is_ulid(room_id) {
            return Err(TaskLaunchAttemptErrorV1::Rejected);
        }
        let (status, response) = self.request::<OperatorRoomSummary>(
            "GET",
            &format!("/v1/operator/rooms/{room_id}"),
            None::<&serde_json::Value>,
        )?;
        match (status, response) {
            (200, Some(response)) => Ok(response.room_head),
            (401 | 403, _) => Err(TaskLaunchAttemptErrorV1::OperatorFixRequired),
            (400 | 404 | 409 | 422, _) => Err(TaskLaunchAttemptErrorV1::Rejected),
            _ => Err(TaskLaunchAttemptErrorV1::Ambiguous),
        }
    }
}

/// Combines exact local managed-instance evidence with daemon Runner registration.
#[derive(Clone)]
pub struct LiveTaskRunnerReadinessSourceV1 {
    daemon: HttpDaemonTaskRuntimeV1,
    managed: RunnerSupervisorV1,
}

impl LiveTaskRunnerReadinessSourceV1 {
    #[must_use]
    pub const fn new(daemon: HttpDaemonTaskRuntimeV1, managed: RunnerSupervisorV1) -> Self {
        Self { daemon, managed }
    }
}

impl TaskRunnerReadinessSourceV1 for LiveTaskRunnerReadinessSourceV1 {
    fn presence(&self, runner_id: &str) -> TaskRunnerObservationV1 {
        self.daemon.runner_presence(runner_id)
    }

    fn select_managed(
        &self,
        pack: &PackReference,
        requested: Option<&RunnerTemplateRevisionReferenceV1>,
    ) -> Option<ManagedRunnerAssignmentV1> {
        self.managed
            .statuses()
            .instances
            .into_iter()
            .filter(|instance| {
                managed_candidate_is_live_compatible(instance, pack)
                    && requested.is_none_or(|requested| {
                        instance.template_id == requested.template_id
                            && instance.template_revision == requested.revision
                    })
                    && self
                        .managed
                        .task_runner_binding_available(&instance.instance_id)
            })
            .min_by(|left, right| left.instance_id.cmp(&right.instance_id))
            .map(|instance| ManagedRunnerAssignmentV1 {
                instance_id: instance.instance_id,
                template_id: instance.template_id,
                template_revision: instance.template_revision,
            })
    }

    fn managed_reason(
        &self,
        assignment: &ManagedRunnerAssignmentV1,
        pack: &PackReference,
    ) -> TaskSeatReadinessReasonV1 {
        let Some(instance) = self
            .managed
            .statuses()
            .instances
            .into_iter()
            .find(|instance| instance.instance_id == assignment.instance_id)
        else {
            return TaskSeatReadinessReasonV1::RunnerAssignmentMissing;
        };
        if instance.template_id != assignment.template_id
            || instance.template_revision != assignment.template_revision
            || !instance.compatibility.iter().any(|rule| {
                rule.activity_pack_id == pack.id && rule.exact_revisions.contains(&pack.version)
            })
        {
            return TaskSeatReadinessReasonV1::RunnerIncompatible;
        }
        if instance.state != RunnerInstanceStateV1::Running
            || instance.health != RunnerInstanceHealthV1::Healthy
        {
            return TaskSeatReadinessReasonV1::RunnerDisconnected;
        }
        if instance.freshness != RunnerFreshnessV1::Fresh {
            return TaskSeatReadinessReasonV1::RunnerStale;
        }
        if instance.capacity.available == 0 {
            return TaskSeatReadinessReasonV1::RunnerOverCapacity;
        }
        TaskSeatReadinessReasonV1::Ready
    }

    fn bind_managed(
        &self,
        assignment: &ManagedRunnerAssignmentV1,
        pack: &PackReference,
        runner_id: &str,
        authority: &SecretReferenceV1,
    ) -> Result<(), TaskSetupAttemptErrorV1> {
        match self.managed_reason(assignment, pack) {
            TaskSeatReadinessReasonV1::Ready => {}
            TaskSeatReadinessReasonV1::RunnerStale
            | TaskSeatReadinessReasonV1::RunnerDisconnected
            | TaskSeatReadinessReasonV1::RunnerOverCapacity => {
                return Err(TaskSetupAttemptErrorV1::Ambiguous);
            }
            TaskSeatReadinessReasonV1::RunnerAssignmentMissing
            | TaskSeatReadinessReasonV1::RunnerIncompatible
            | TaskSeatReadinessReasonV1::OptionalUnfilled
            | TaskSeatReadinessReasonV1::SetupIncomplete
            | TaskSeatReadinessReasonV1::ConsoleMissing
            | TaskSeatReadinessReasonV1::ConsoleStale
            | TaskSeatReadinessReasonV1::ConsoleInvalid
            | TaskSeatReadinessReasonV1::ConsoleDisconnected
            | TaskSeatReadinessReasonV1::RunnerMissing => {
                return Err(TaskSetupAttemptErrorV1::Rejected);
            }
        }
        self.managed
            .bind_task_runner_authority(&assignment.instance_id, runner_id, authority)
            .map_err(|error| match error {
                ManagedRunnerBindingErrorV1::Unavailable => TaskSetupAttemptErrorV1::Ambiguous,
                ManagedRunnerBindingErrorV1::NotFound | ManagedRunnerBindingErrorV1::Conflict => {
                    TaskSetupAttemptErrorV1::Rejected
                }
            })
    }
}

fn managed_candidate_is_live_compatible(
    instance: &RunnerInstanceStatusV1,
    pack: &PackReference,
) -> bool {
    instance.managed_by_supervisor
        && instance.state == RunnerInstanceStateV1::Running
        && instance.health == RunnerInstanceHealthV1::Healthy
        && instance.freshness == RunnerFreshnessV1::Fresh
        && instance.capacity.available > 0
        && instance.compatibility.iter().any(|rule| {
            rule.activity_pack_id == pack.id && rule.exact_revisions.contains(&pack.version)
        })
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
    #[error("Task is not ready to launch")]
    NotReady,
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
    console: Option<Arc<dyn ParticipantConsoleReadinessSourceV1>>,
    runners: Option<Arc<dyn TaskRunnerReadinessSourceV1>>,
    launcher: Option<Arc<dyn DaemonTaskLaunchSourceV1>>,
    profiles: Option<AgentProfileStoreV1>,
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
            console: None,
            runners: None,
            launcher: None,
            profiles: None,
            mutation: Arc::new(Mutex::new(())),
        })
    }

    /// Injects the three bounded live boundaries used by readiness and launch.
    #[must_use]
    pub fn with_launch_readiness(
        mut self,
        console: impl ParticipantConsoleReadinessSourceV1 + 'static,
        runners: impl TaskRunnerReadinessSourceV1,
        launcher: impl DaemonTaskLaunchSourceV1,
    ) -> Self {
        self.console = Some(Arc::new(console));
        self.runners = Some(Arc::new(runners));
        self.launcher = Some(Arc::new(launcher));
        self
    }

    /// Enables immutable exact Agent Profile-to-Membership binding.
    #[must_use]
    pub fn with_agent_profiles(mut self, profiles: AgentProfileStoreV1) -> Self {
        self.profiles = Some(profiles);
        self
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
        let operation = self.reconcile_unlocked(operation)?;
        Ok(self.status_for(operation))
    }

    /// Safely retries only the original immutable setup operation.
    ///
    /// # Errors
    ///
    /// Returns exact not-found or protected-storage unavailable.
    pub fn retry(&self, draft_id: &str) -> Result<TaskSetupStatusV1, TaskSetupErrorV1> {
        let _guard = self.lock();
        let operation = self.load_unlocked(draft_id)?;
        let operation = self.reconcile_unlocked(operation)?;
        Ok(self.status_for(operation))
    }

    /// Loads the browser-safe durable setup status without issuing effects.
    ///
    /// # Errors
    ///
    /// Returns exact not-found or protected-storage unavailable.
    pub fn status(&self, draft_id: &str) -> Result<TaskSetupStatusV1, TaskSetupErrorV1> {
        let _guard = self.lock();
        let operation = self.load_unlocked(draft_id)?;
        Ok(self.status_for(operation))
    }

    /// Launches or reconciles only the original stable Lobby input.
    ///
    /// # Errors
    ///
    /// Returns not-ready, not-found, or protected-storage unavailable.
    pub fn launch(&self, draft_id: &str) -> Result<TaskSetupStatusV1, TaskSetupErrorV1> {
        let _guard = self.lock();
        let mut operation = self.load_unlocked(draft_id)?;
        let launcher = self
            .launcher
            .as_ref()
            .ok_or(TaskSetupErrorV1::Unavailable)?;
        if operation.launch.is_some() {
            self.reconcile_launch(&mut operation, launcher.as_ref())?;
            return Ok(self.status_for(operation));
        }
        let readiness = self.readiness_for(&operation);
        if operation.state != TaskSetupStateV1::Ready || !readiness.ready_to_launch {
            return Err(TaskSetupErrorV1::NotReady);
        }
        let creation = self
            .creation
            .status(draft_id)
            .map_err(|_| TaskSetupErrorV1::InvalidCreation)?;
        let head = creation
            .response
            .as_ref()
            .map(|response| &response.room_head)
            .ok_or(TaskSetupErrorV1::InvalidCreation)?;
        operation.launch = Some(TaskLaunchIntentV1 {
            input_id: next_ulid()?,
            based_on_room_seq: head.room_seq,
            state: TaskLaunchStateV1::Waiting,
            attempts: 0,
            response: None,
            attention: None,
        });
        self.persist(&mut operation)?;
        self.reconcile_launch(&mut operation, launcher.as_ref())?;
        Ok(self.status_for(operation))
    }

    fn status_for(&self, operation: TaskSetupOperationV1) -> TaskSetupStatusV1 {
        let readiness = self.readiness_for(&operation);
        TaskSetupStatusV1::from_operation(operation, readiness)
    }

    fn readiness_for(&self, operation: &TaskSetupOperationV1) -> TaskReadinessV1 {
        let seats = operation
            .seats
            .iter()
            .map(|seat| {
                let reason = self.seat_readiness_reason(operation, seat);
                TaskSeatReadinessV1 {
                    seat_id: seat.seat_id.clone(),
                    required: seat.required,
                    ready: matches!(
                        reason,
                        TaskSeatReadinessReasonV1::Ready
                            | TaskSeatReadinessReasonV1::OptionalUnfilled
                    ),
                    reason,
                }
            })
            .collect::<Vec<_>>();
        let ready_to_launch =
            operation.state == TaskSetupStateV1::Ready && seats.iter().all(|seat| seat.ready);
        TaskReadinessV1 {
            ready_to_launch,
            seats,
        }
    }

    fn seat_readiness_reason(
        &self,
        operation: &TaskSetupOperationV1,
        seat: &SetupSeatIntentV1,
    ) -> TaskSeatReadinessReasonV1 {
        let Some(member_id) = seat.member_id.as_deref() else {
            return if seat.required {
                TaskSeatReadinessReasonV1::SetupIncomplete
            } else {
                TaskSeatReadinessReasonV1::OptionalUnfilled
            };
        };
        if operation.state != TaskSetupStateV1::Ready {
            return TaskSeatReadinessReasonV1::SetupIncomplete;
        }
        match seat.principal_kind {
            Some(PrincipalKind::Human) => {
                let Some(console) = &self.console else {
                    return TaskSeatReadinessReasonV1::ConsoleMissing;
                };
                match console.session_health(&operation.room_id, member_id) {
                    ParticipantConsoleSessionHealthV1::Usable => TaskSeatReadinessReasonV1::Ready,
                    ParticipantConsoleSessionHealthV1::Missing => {
                        TaskSeatReadinessReasonV1::ConsoleMissing
                    }
                    ParticipantConsoleSessionHealthV1::Stale => {
                        TaskSeatReadinessReasonV1::ConsoleStale
                    }
                    ParticipantConsoleSessionHealthV1::Invalid => {
                        TaskSeatReadinessReasonV1::ConsoleInvalid
                    }
                    ParticipantConsoleSessionHealthV1::Disconnected => {
                        TaskSeatReadinessReasonV1::ConsoleDisconnected
                    }
                }
            }
            Some(PrincipalKind::Agent) => self.agent_readiness_reason(operation, seat),
            None => TaskSeatReadinessReasonV1::SetupIncomplete,
        }
    }

    fn agent_readiness_reason(
        &self,
        operation: &TaskSetupOperationV1,
        seat: &SetupSeatIntentV1,
    ) -> TaskSeatReadinessReasonV1 {
        let Some(runners) = &self.runners else {
            return TaskSeatReadinessReasonV1::RunnerMissing;
        };
        let Some(runner) = &seat.runner else {
            return TaskSeatReadinessReasonV1::RunnerAssignmentMissing;
        };
        if seat.agent_profile.is_none() {
            return TaskSeatReadinessReasonV1::RunnerAssignmentMissing;
        }
        if seat.agent_assignment == Some(AgentAssignmentModeV1::Managed) {
            let Some(assignment) = &runner.managed_assignment else {
                return TaskSeatReadinessReasonV1::RunnerAssignmentMissing;
            };
            let reason = runners.managed_reason(assignment, &operation.pack);
            if reason != TaskSeatReadinessReasonV1::Ready {
                return reason;
            }
        }
        let presence = match runners.presence(&runner.runner_id) {
            TaskRunnerObservationV1::Present(presence) => presence,
            TaskRunnerObservationV1::Missing => return TaskSeatReadinessReasonV1::RunnerMissing,
            TaskRunnerObservationV1::Unavailable => {
                return TaskSeatReadinessReasonV1::RunnerDisconnected;
            }
        };
        if presence.runner_id != runner.runner_id {
            return TaskSeatReadinessReasonV1::RunnerMissing;
        }
        if presence.connection == worldstream_protocol::OperatorRunnerConnectionV1::Disconnected {
            return TaskSeatReadinessReasonV1::RunnerDisconnected;
        }
        if presence.freshness == worldstream_protocol::OperatorRunnerFreshnessV1::Stale {
            return TaskSeatReadinessReasonV1::RunnerStale;
        }
        if !presence
            .supported_pack_revisions
            .iter()
            .any(|pack| pack == &operation.pack)
        {
            return TaskSeatReadinessReasonV1::RunnerIncompatible;
        }
        if presence.available_activations == 0 {
            return TaskSeatReadinessReasonV1::RunnerOverCapacity;
        }
        TaskSeatReadinessReasonV1::Ready
    }

    #[allow(clippy::too_many_lines)]
    fn reconcile_launch(
        &self,
        operation: &mut TaskSetupOperationV1,
        launcher: &dyn DaemonTaskLaunchSourceV1,
    ) -> Result<(), TaskSetupErrorV1> {
        if operation
            .launch
            .as_ref()
            .ok_or(TaskSetupErrorV1::Unavailable)?
            .state
            == TaskLaunchStateV1::Launched
        {
            return Ok(());
        }
        if operation
            .launch
            .as_ref()
            .is_some_and(|launch| launch.response.is_none())
        {
            {
                let launch = operation
                    .launch
                    .as_mut()
                    .ok_or(TaskSetupErrorV1::Unavailable)?;
                launch.state = TaskLaunchStateV1::Retrying;
                launch.attempts = launch.attempts.saturating_add(1);
                launch.attention = None;
            }
            self.persist(operation)?;
            let request = operation.launch.as_ref().map_or_else(
                || Err(TaskSetupErrorV1::Unavailable),
                |launch| {
                    Ok(LobbyLaunchRequest {
                        input_id: launch.input_id.clone(),
                        based_on_room_seq: launch.based_on_room_seq,
                    })
                },
            )?;
            match launcher.launch(&operation.room_id, &request) {
                Ok(response)
                    if response.room_id == operation.room_id
                        && response.input_id == request.input_id
                        && response.room_head.room_id == operation.room_id
                        && response.room_head.room_seq > request.based_on_room_seq =>
                {
                    let launch = operation
                        .launch
                        .as_mut()
                        .ok_or(TaskSetupErrorV1::Unavailable)?;
                    launch.response = Some(response);
                    launch.state = TaskLaunchStateV1::Reconciling;
                    self.persist(operation)?;
                }
                Ok(_) => {
                    let launch = operation
                        .launch
                        .as_mut()
                        .ok_or(TaskSetupErrorV1::Unavailable)?;
                    launch.state = TaskLaunchStateV1::NeedsAttention;
                    launch.attention =
                        Some(launch_attention(TaskLaunchAttemptErrorV1::Rejected, false));
                    self.persist(operation)?;
                    return Ok(());
                }
                Err(error) => {
                    let launch = operation
                        .launch
                        .as_mut()
                        .ok_or(TaskSetupErrorV1::Unavailable)?;
                    launch.state = TaskLaunchStateV1::NeedsAttention;
                    launch.attention = Some(launch_attention(error, true));
                    self.persist(operation)?;
                    return Ok(());
                }
            }
        }
        let launch = operation
            .launch
            .as_mut()
            .ok_or(TaskSetupErrorV1::Unavailable)?;
        if launch.response.is_none() {
            return Err(TaskSetupErrorV1::Unavailable);
        }
        // The host-authenticated launch response is the daemon's authoritative
        // committed receipt. It is exact-bound into the durable checkpoint, so
        // a later Room head may advance without invalidating launch observation.
        launch.state = TaskLaunchStateV1::Launched;
        launch.attention = None;
        self.persist(operation)
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
        let pack = creation
            .review
            .pack
            .clone()
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
                    &pack,
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
                    agent_profile: None,
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
            pack,
            creation_intent_hash: creation.intent_hash,
            setup_intent_hash: String::new(),
            checkpoint_hash: String::new(),
            seats,
            state: TaskSetupStateV1::Waiting,
            attempts: 0,
            active_stage: None,
            attention: None,
            launch: None,
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
        pack: &PackReference,
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
            let assignment = seat
                .agent_assignment
                .ok_or(TaskSetupErrorV1::InvalidCreation)?;
            let managed_assignment = if assignment == AgentAssignmentModeV1::Managed {
                self.runners
                    .as_ref()
                    .and_then(|runners| runners.select_managed(pack, seat.runner_template.as_ref()))
            } else {
                None
            };
            Some(RunnerIntentV1 {
                runner_id: next_ulid()?,
                principal_change_id: principal_id.clone(),
                runner_change_id: next_ulid()?,
                managed_assignment,
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
            agent_profile: seat.agent_profile.clone(),
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

    #[allow(clippy::too_many_lines)]
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
                if let Some(assignment) = &runner.managed_assignment {
                    self.runners
                        .as_ref()
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?
                        .bind_managed(
                            assignment,
                            &operation.pack,
                            &runner.runner_id,
                            &runner.capability.secret_reference,
                        )?;
                }
                if let Some(profiles) = &self.profiles {
                    let profile = seat
                        .agent_profile
                        .clone()
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?;
                    let member_id = seat
                        .member_id
                        .clone()
                        .ok_or(TaskSetupAttemptErrorV1::Rejected)?;
                    let execution = match seat.agent_assignment {
                        Some(AgentAssignmentModeV1::External) => AgentExecutionBindingV1::External,
                        Some(AgentAssignmentModeV1::Managed) => AgentExecutionBindingV1::Managed {
                            runner_id: runner.runner_id.clone(),
                        },
                        None => return Err(TaskSetupAttemptErrorV1::Rejected),
                    };
                    profiles
                        .reconcile_membership(&AgentProfileSeatAssignmentV1 {
                            schema: "worldstream/studio-agent-profile-assignment/v1".to_owned(),
                            assignment_id: member_id.clone(),
                            draft_id: operation.draft_id.clone(),
                            seat_id: seat.seat_id.clone(),
                            profile,
                            membership: AgentProfileMembershipBindingV1 {
                                room_id: operation.room_id.clone(),
                                member_id,
                                principal_id: seat
                                    .principal_id
                                    .clone()
                                    .ok_or(TaskSetupAttemptErrorV1::Rejected)?,
                                role: seat.role.clone(),
                            },
                            execution,
                        })
                        .map_err(|error| match error {
                            AgentProfileErrorV1::Unavailable => TaskSetupAttemptErrorV1::Ambiguous,
                            AgentProfileErrorV1::InvalidProfile
                            | AgentProfileErrorV1::ImmutableRevisionConflict
                            | AgentProfileErrorV1::InvalidAssignment
                            | AgentProfileErrorV1::ImmutableAssignmentConflict
                            | AgentProfileErrorV1::NotFound => TaskSetupAttemptErrorV1::Rejected,
                        })?;
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
        load_task_setup_operation(&self.root, draft_id)
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

fn load_task_setup_operation(
    root: &Path,
    draft_id: &str,
) -> Result<TaskSetupOperationV1, TaskSetupErrorV1> {
    validate_draft_id(draft_id)?;
    let path = root.join(format!("{draft_id}.json"));
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

impl ParticipantHandoffAuthoritySourceV1 for TaskSetupSupervisorV1 {
    fn resolve_provisioned_human_seat(
        &self,
        draft_id: &str,
        seat_id: &str,
    ) -> Result<HumanSeatAuthorityV1, ParticipantHandoffAuthorityErrorV1> {
        let _guard = self.lock();
        let operation = self.load_unlocked(draft_id).map_err(|error| match error {
            TaskSetupErrorV1::NotFound | TaskSetupErrorV1::InvalidCreation => {
                ParticipantHandoffAuthorityErrorV1::SeatNotFound
            }
            TaskSetupErrorV1::NotReady | TaskSetupErrorV1::Unavailable => {
                ParticipantHandoffAuthorityErrorV1::Unavailable
            }
        })?;
        let seat = operation
            .seats
            .iter()
            .find(|seat| seat.seat_id == seat_id)
            .ok_or(ParticipantHandoffAuthorityErrorV1::SeatNotFound)?;
        if seat.principal_kind != Some(PrincipalKind::Human) {
            return Err(if seat.principal_kind.is_some() {
                ParticipantHandoffAuthorityErrorV1::NotHuman
            } else {
                ParticipantHandoffAuthorityErrorV1::NotProvisioned
            });
        }
        let capability = seat
            .member_capability
            .as_ref()
            .filter(|capability| capability.provisioned)
            .ok_or(ParticipantHandoffAuthorityErrorV1::NotProvisioned)?;
        if operation.state != TaskSetupStateV1::Ready {
            return Err(ParticipantHandoffAuthorityErrorV1::NotProvisioned);
        }
        let secret = self
            .vault
            .resolve(
                SecretKindV1::MembershipAuthority,
                &capability.secret_reference,
            )
            .map_err(|_| ParticipantHandoffAuthorityErrorV1::AuthorityInvalid)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| ParticipantHandoffAuthorityErrorV1::AuthorityInvalid)?;
        HumanSeatAuthorityV1::new(
            &operation.room_id,
            seat.member_id
                .as_deref()
                .ok_or(ParticipantHandoffAuthorityErrorV1::NotProvisioned)?,
            SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bytes)),
        )
    }
}

impl AssignedMembershipSourceV1 for TaskSetupSupervisorV1 {
    fn resolve_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipAuthorityV1, AssignedMembershipSourceErrorV1> {
        let _guard = self.lock();
        let assignment = self
            .profiles
            .as_ref()
            .ok_or(AssignedMembershipSourceErrorV1::Unavailable)?
            .assignment(assignment_id)
            .map_err(|error| match error {
                AgentProfileErrorV1::NotFound => AssignedMembershipSourceErrorV1::NotFound,
                AgentProfileErrorV1::InvalidAssignment
                | AgentProfileErrorV1::ImmutableAssignmentConflict
                | AgentProfileErrorV1::InvalidProfile
                | AgentProfileErrorV1::ImmutableRevisionConflict => {
                    AssignedMembershipSourceErrorV1::Invalid
                }
                AgentProfileErrorV1::Unavailable => AssignedMembershipSourceErrorV1::Unavailable,
            })?;
        let operation = self
            .load_unlocked(&assignment.draft_id)
            .map_err(|error| match error {
                TaskSetupErrorV1::NotFound | TaskSetupErrorV1::InvalidCreation => {
                    AssignedMembershipSourceErrorV1::NotFound
                }
                TaskSetupErrorV1::NotReady | TaskSetupErrorV1::Unavailable => {
                    AssignedMembershipSourceErrorV1::Unavailable
                }
            })?;
        let seat = operation
            .seats
            .iter()
            .find(|seat| seat.seat_id == assignment.seat_id)
            .ok_or(AssignedMembershipSourceErrorV1::NotFound)?;
        if operation.state != TaskSetupStateV1::Ready
            || operation.room_id != assignment.membership.room_id
            || seat.principal_kind != Some(PrincipalKind::Agent)
            || seat.member_id.as_deref() != Some(&assignment.membership.member_id)
            || seat.principal_id.as_deref() != Some(&assignment.membership.principal_id)
            || seat.role != assignment.membership.role
            || seat.agent_profile.as_ref() != Some(&assignment.profile)
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let capability = seat
            .member_capability
            .as_ref()
            .filter(|capability| capability.provisioned)
            .ok_or(AssignedMembershipSourceErrorV1::Revoked)?;
        let secret = self
            .vault
            .resolve(
                SecretKindV1::MembershipAuthority,
                &capability.secret_reference,
            )
            .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
        AssignedMembershipAuthorityV1::new(
            &assignment.assignment_id,
            &assignment.profile.profile_id,
            &assignment.profile.revision,
            &assignment.membership.role,
            &assignment.membership.principal_id,
            &assignment.membership.room_id,
            &assignment.membership.member_id,
            SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bytes)),
        )
    }
}

/// Read-only production assignment resolver used by the local stdio MCP helper.
#[derive(Clone)]
pub struct FileAssignedMembershipSourceV1 {
    root: Arc<PathBuf>,
    profiles: AgentProfileStoreV1,
    vault: FileSecretVaultV1,
}

impl FileAssignedMembershipSourceV1 {
    /// Opens the exact protected Task setup store without provisioning effects.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe or unavailable owner-only data directory.
    pub fn open(
        root: &Path,
        profiles: AgentProfileStoreV1,
        vault: FileSecretVaultV1,
    ) -> Result<Self, AssignedMembershipSourceErrorV1> {
        let root = prepare_data_directory(root)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            profiles,
            vault,
        })
    }
}

impl AssignedMembershipSourceV1 for FileAssignedMembershipSourceV1 {
    fn resolve_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipAuthorityV1, AssignedMembershipSourceErrorV1> {
        let assignment = self
            .profiles
            .assignment(assignment_id)
            .map_err(|error| match error {
                AgentProfileErrorV1::NotFound => AssignedMembershipSourceErrorV1::NotFound,
                AgentProfileErrorV1::InvalidAssignment
                | AgentProfileErrorV1::ImmutableAssignmentConflict
                | AgentProfileErrorV1::InvalidProfile
                | AgentProfileErrorV1::ImmutableRevisionConflict => {
                    AssignedMembershipSourceErrorV1::Invalid
                }
                AgentProfileErrorV1::Unavailable => AssignedMembershipSourceErrorV1::Unavailable,
            })?;
        let operation = load_task_setup_operation(&self.root, &assignment.draft_id).map_err(
            |error| match error {
                TaskSetupErrorV1::NotFound | TaskSetupErrorV1::InvalidCreation => {
                    AssignedMembershipSourceErrorV1::NotFound
                }
                TaskSetupErrorV1::NotReady | TaskSetupErrorV1::Unavailable => {
                    AssignedMembershipSourceErrorV1::Unavailable
                }
            },
        )?;
        resolve_file_assignment_authority(&operation, &assignment, &self.vault)
    }
}

impl AssignedMembershipLaunchSourceV1 for FileAssignedMembershipSourceV1 {
    fn resolve_launch_binding(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipLaunchBindingV1, AssignedMembershipSourceErrorV1> {
        let assignment = self
            .profiles
            .assignment(assignment_id)
            .map_err(map_profile_assignment_error)?;
        let operation = load_task_setup_operation(&self.root, &assignment.draft_id)
            .map_err(map_assignment_setup_error)?;
        let seat = validated_agent_assignment_seat(&operation, &assignment)?;
        let capability = seat
            .member_capability
            .as_ref()
            .filter(|capability| capability.provisioned)
            .ok_or(AssignedMembershipSourceErrorV1::Revoked)?;
        if self
            .vault
            .inspect(
                SecretKindV1::MembershipAuthority,
                &capability.secret_reference,
            )
            .availability
            != crate::secrets::SecretAvailabilityV1::Configured
        {
            return Err(AssignedMembershipSourceErrorV1::Revoked);
        }
        let runner = seat
            .runner
            .as_ref()
            .filter(|runner| runner.capability.provisioned)
            .ok_or(AssignedMembershipSourceErrorV1::Revoked)?;
        if self
            .vault
            .inspect(
                SecretKindV1::RunnerAuthority,
                &runner.capability.secret_reference,
            )
            .availability
            != crate::secrets::SecretAvailabilityV1::Configured
        {
            return Err(AssignedMembershipSourceErrorV1::Revoked);
        }
        Ok(AssignedMembershipLaunchBindingV1 {
            assignment_id: assignment.assignment_id,
            profile_id: assignment.profile.profile_id,
            profile_revision: assignment.profile.revision,
            role: assignment.membership.role,
            principal_id: assignment.membership.principal_id,
            room_id: assignment.membership.room_id,
            member_id: assignment.membership.member_id,
            pack: operation.pack.clone(),
            authority_reference: capability.secret_reference.clone(),
            runner_id: runner.runner_id.clone(),
            runner_authority_reference: runner.capability.secret_reference.clone(),
        })
    }
}

fn map_profile_assignment_error(error: AgentProfileErrorV1) -> AssignedMembershipSourceErrorV1 {
    match error {
        AgentProfileErrorV1::NotFound => AssignedMembershipSourceErrorV1::NotFound,
        AgentProfileErrorV1::InvalidAssignment
        | AgentProfileErrorV1::ImmutableAssignmentConflict
        | AgentProfileErrorV1::InvalidProfile
        | AgentProfileErrorV1::ImmutableRevisionConflict => {
            AssignedMembershipSourceErrorV1::Invalid
        }
        AgentProfileErrorV1::Unavailable => AssignedMembershipSourceErrorV1::Unavailable,
    }
}

fn map_assignment_setup_error(error: TaskSetupErrorV1) -> AssignedMembershipSourceErrorV1 {
    match error {
        TaskSetupErrorV1::NotFound | TaskSetupErrorV1::InvalidCreation => {
            AssignedMembershipSourceErrorV1::NotFound
        }
        TaskSetupErrorV1::NotReady | TaskSetupErrorV1::Unavailable => {
            AssignedMembershipSourceErrorV1::Unavailable
        }
    }
}

fn validated_agent_assignment_seat<'a>(
    operation: &'a TaskSetupOperationV1,
    assignment: &AgentProfileSeatAssignmentV1,
) -> Result<&'a SetupSeatIntentV1, AssignedMembershipSourceErrorV1> {
    let seat = operation
        .seats
        .iter()
        .find(|seat| seat.seat_id == assignment.seat_id)
        .ok_or(AssignedMembershipSourceErrorV1::NotFound)?;
    if operation.state != TaskSetupStateV1::Ready
        || operation.room_id != assignment.membership.room_id
        || seat.principal_kind != Some(PrincipalKind::Agent)
        || seat.member_id.as_deref() != Some(&assignment.membership.member_id)
        || seat.principal_id.as_deref() != Some(&assignment.membership.principal_id)
        || seat.role != assignment.membership.role
        || seat.agent_profile.as_ref() != Some(&assignment.profile)
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    Ok(seat)
}

fn resolve_file_assignment_authority(
    operation: &TaskSetupOperationV1,
    assignment: &AgentProfileSeatAssignmentV1,
    vault: &FileSecretVaultV1,
) -> Result<AssignedMembershipAuthorityV1, AssignedMembershipSourceErrorV1> {
    let seat = operation
        .seats
        .iter()
        .find(|seat| seat.seat_id == assignment.seat_id)
        .ok_or(AssignedMembershipSourceErrorV1::NotFound)?;
    if operation.state != TaskSetupStateV1::Ready
        || operation.room_id != assignment.membership.room_id
        || seat.principal_kind != Some(PrincipalKind::Agent)
        || seat.member_id.as_deref() != Some(&assignment.membership.member_id)
        || seat.principal_id.as_deref() != Some(&assignment.membership.principal_id)
        || seat.role != assignment.membership.role
        || seat.agent_profile.as_ref() != Some(&assignment.profile)
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    let capability = seat
        .member_capability
        .as_ref()
        .filter(|capability| capability.provisioned)
        .ok_or(AssignedMembershipSourceErrorV1::Revoked)?;
    let secret = vault
        .resolve(
            SecretKindV1::MembershipAuthority,
            &capability.secret_reference,
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
    let bytes: [u8; 32] = secret
        .as_bytes()
        .try_into()
        .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
    AssignedMembershipAuthorityV1::new(
        &assignment.assignment_id,
        &assignment.profile.profile_id,
        &assignment.profile.revision,
        &assignment.membership.role,
        &assignment.membership.principal_id,
        &assignment.membership.room_id,
        &assignment.membership.member_id,
        SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bytes)),
    )
}

/// Builds the bounded setup status/start/retry API.
pub fn task_setup_router(supervisor: TaskSetupSupervisorV1) -> Router {
    Router::new()
        .route("/api/v1/task-setups/{draft_id}", get(setup_status))
        .route("/api/v1/task-setups/{draft_id}:start", post(start_setup))
        .route("/api/v1/task-setups/{draft_id}:retry", post(retry_setup))
        .route("/api/v1/task-setups/{draft_id}:launch", post(launch_task))
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

async fn launch_task(
    State(supervisor): State<TaskSetupSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<TaskSetupStatusV1>, TaskSetupErrorV1> {
    let status = tokio::task::spawn_blocking(move || supervisor.launch(&draft_id))
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
            Self::NotReady => (
                StatusCode::CONFLICT,
                "task_not_ready_to_launch",
                "every required seat must be live-ready before launch",
                true,
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

fn launch_attention(error: TaskLaunchAttemptErrorV1, retryable: bool) -> TaskSetupAttentionV1 {
    let (code, message) = match error {
        TaskLaunchAttemptErrorV1::Ambiguous => (
            "launch_result_ambiguous",
            "Retry the original Lobby launch to reconcile its committed transition.",
        ),
        TaskLaunchAttemptErrorV1::OperatorFixRequired => (
            "launch_authority_unavailable",
            "Repair the protected host authority, then retry the original Lobby launch.",
        ),
        TaskLaunchAttemptErrorV1::Rejected => (
            "launch_rejected",
            "The daemon rejected this Lobby launch; retry only the original launch after correcting the condition.",
        ),
    };
    TaskSetupAttentionV1 {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
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
        || operation.pack.id.is_empty()
        || operation.pack.version.is_empty()
        || !is_digest(&operation.pack.digest)
        || !is_digest(&operation.creation_intent_hash)
        || setup_intent_hash(operation)? != operation.setup_intent_hash
        || checkpoint_hash(operation)? != operation.checkpoint_hash
        || operation.seats.len() > 64
        || !state_is_coherent(operation)
        || !progress_is_a_prefix(operation)
        || !launch_is_coherent(operation)
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
                || seat.agent_profile.is_some()
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
            || (seat.principal_kind != Some(PrincipalKind::Agent) && seat.agent_profile.is_some())
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
                || !is_ulid(&runner.capability.change_id)
                || (seat.agent_assignment == Some(AgentAssignmentModeV1::Managed))
                    != runner.managed_assignment.is_some())
        {
            return Err(TaskSetupErrorV1::Unavailable);
        }
    }
    Ok(())
}

fn launch_is_coherent(operation: &TaskSetupOperationV1) -> bool {
    let Some(launch) = &operation.launch else {
        return true;
    };
    if !is_ulid(&launch.input_id) || operation.state != TaskSetupStateV1::Ready {
        return false;
    }
    (match launch.state {
        TaskLaunchStateV1::Waiting => {
            launch.attempts == 0 && launch.response.is_none() && launch.attention.is_none()
        }
        TaskLaunchStateV1::Retrying => {
            launch.attempts > 0 && launch.response.is_none() && launch.attention.is_none()
        }
        TaskLaunchStateV1::Reconciling | TaskLaunchStateV1::Launched => {
            launch.attempts > 0 && launch.response.is_some() && launch.attention.is_none()
        }
        TaskLaunchStateV1::NeedsAttention => launch.attempts > 0 && launch.attention.is_some(),
    }) && launch.response.as_ref().is_none_or(|response| {
        response.room_id == operation.room_id
            && response.input_id == launch.input_id
            && response.room_head.room_id == operation.room_id
            && response.room_head.room_seq > launch.based_on_room_seq
    })
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
        pack: &'a PackReference,
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
        agent_profile: Option<&'a AgentProfileRevisionReferenceV1>,
        member_id: Option<&'a str>,
        member_capability_id: Option<&'a str>,
        member_change_id: Option<&'a str>,
        member_secret_reference: Option<&'a str>,
        runner_id: Option<&'a str>,
        runner_principal_change_id: Option<&'a str>,
        runner_change_id: Option<&'a str>,
        managed_assignment: Option<&'a ManagedRunnerAssignmentV1>,
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
            agent_profile: seat.agent_profile.as_ref(),
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
            managed_assignment: seat
                .runner
                .as_ref()
                .and_then(|value| value.managed_assignment.as_ref()),
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
        pack: &operation.pack,
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

#[cfg(test)]
mod readiness_selection_tests {
    use super::*;
    use crate::runner_templates::{RunnerCapacityStatusV1, RunnerCompatibilityRuleV1};

    fn candidate(instance_id: &str, health: RunnerInstanceHealthV1) -> RunnerInstanceStatusV1 {
        RunnerInstanceStatusV1 {
            instance_id: instance_id.to_owned(),
            template_id: "managed-runner".to_owned(),
            template_revision: "r1".to_owned(),
            state: RunnerInstanceStateV1::Running,
            operation_id: 1,
            managed_by_supervisor: true,
            compatibility: vec![RunnerCompatibilityRuleV1 {
                activity_pack_id: "counter".to_owned(),
                exact_revisions: vec!["2.0.0".to_owned()],
            }],
            capacity: RunnerCapacityStatusV1 {
                maximum: 2,
                in_use: 1,
                available: 1,
            },
            health,
            freshness: RunnerFreshnessV1::Fresh,
            observed_at_unix_ms: Some(1),
            failure: None,
        }
    }

    #[test]
    fn managed_selection_skips_an_unhealthy_first_instance_for_a_healthy_second() {
        let pack = PackReference {
            id: "counter".to_owned(),
            version: "2.0.0".to_owned(),
            digest: "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_owned(),
        };
        let candidates = [
            candidate("managed-01", RunnerInstanceHealthV1::Unavailable),
            candidate("managed-02", RunnerInstanceHealthV1::Healthy),
        ];

        let selected = candidates
            .iter()
            .find(|instance| managed_candidate_is_live_compatible(instance, &pack));
        assert_eq!(
            selected.map(|instance| instance.instance_id.as_str()),
            Some("managed-02")
        );
    }
}
