//! Browser-safe Runner and Activation operations with durable typed restart reconciliation.

use std::{
    collections::BTreeSet,
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_protocol::{
    BearerWireV1, MAX_MESSAGE_BYTES, OperatorActivationStatusV1, OperatorRunnerConnectionV1,
    OperatorRunnerFreshnessV1, OperatorRunnerPresenceV1, PackReference, UlidString,
};
use worldstream_runtime::{
    create_owner_only_renameable_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::{Zeroize as _, Zeroizing};

use crate::{
    agent_profiles::{
        AgentExecutionBindingV1, AgentProfileErrorV1, AgentProfileSeatAssignmentV1,
        AgentProfileStoreV1,
    },
    assignment_mcp::{
        AssignedMembershipLaunchSourceV1, AssignedMembershipSourceErrorV1,
        AssignmentMcpLaunchRegistryV1,
    },
    managed_agent_host::{
        ManagedAgentActivationStateV1, ManagedAgentHostOperationsV1, ManagedAgentHostStatusV1,
    },
    runner_templates::{RunnerInstanceStateV1, RunnerSupervisorV1},
    secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
    task_setup::{TaskSetupErrorV1, TaskSetupStateV1, TaskSetupSupervisorV1},
};

const OPERATIONS_SCHEMA_V1: &str = "worldstream/studio-runner-attention/v1";
const TASK_SCHEMA_V1: &str = "worldstream/studio-task-agent-attention/v1";
const RESTART_SCHEMA_V1: &str = "worldstream/studio-runner-restart-operation/v1";
const MAX_ROWS: usize = 256;
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// Closed source observation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunnerAttentionSourceErrorV1 {
    /// The authoritative daemon or protected local assignment source is unavailable.
    #[error("Runner attention source is unavailable")]
    Unavailable,
    /// A source returned malformed or incoherent aggregate data.
    #[error("Runner attention source returned invalid data")]
    InvalidData,
}

/// Closed approved restart failure. No executable, arguments, or process API is exposed.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunnerRestartControlErrorV1 {
    /// The approved instance does not exist.
    #[error("approved Runner instance was not found")]
    NotFound,
    /// The typed restart failed definitively.
    #[error("approved Runner restart failed")]
    Failed,
    /// Restart publication may have happened and must be reconciled.
    #[error("approved Runner restart outcome is unavailable")]
    Ambiguous,
}

/// Closed Supervisor API failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RunnerAttentionErrorV1 {
    /// The requested Room, instance, or operation does not exist.
    #[error("Runner attention target was not found")]
    NotFound,
    /// The same operation identity was bound to another instance.
    #[error("Runner restart operation identity conflicts")]
    Conflict,
    /// Input or retained aggregate data is invalid.
    #[error("Runner attention data is invalid")]
    InvalidData,
    /// Protected restart storage is unavailable.
    #[error("Runner attention storage is unavailable")]
    Unavailable,
}

/// Exact persisted agent-seat assignment used only to bind aggregate reads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSeatAssignmentV1 {
    pub assignment_id: String,
    pub room_id: String,
    pub seat_id: String,
    pub member_id: String,
    pub runner_id: String,
    pub instance_id: Option<String>,
    /// Marks the ready managed-reference inventory that may exist before the
    /// local host has registered daemon Runner presence.
    #[serde(default, skip_serializing_if = "is_false")]
    pub managed_reference: bool,
    pub pack: PackReference,
}

/// Live daemon Runner presence projected without credentials.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerPresenceSnapshotV1 {
    pub runner_id: String,
    pub connection: OperatorRunnerConnectionV1,
    pub freshness: OperatorRunnerFreshnessV1,
    pub maximum_concurrent_activations: u32,
    pub active_activations: u32,
    pub available_activations: u32,
    pub supported_pack_revisions: Vec<PackReference>,
    pub observed_at_unix_ms: u64,
}

/// Durable aggregate Activation intent counts for one exact assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatActivationAggregateV1 {
    pub room_id: String,
    pub member_id: String,
    pub waiting: u32,
    pub leased: u32,
}

/// Bounded local lifecycle state required for restart reconciliation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalRunnerStateV1 {
    Stopped,
    Restarting,
    Running,
    Failed,
    Unavailable,
}

/// One approved local instance observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRunnerInstanceSnapshotV1 {
    pub instance_id: String,
    pub state: LocalRunnerStateV1,
    pub healthy: bool,
}

/// Complete internal aggregate from exact assignment, daemon, and local lifecycle sources.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeRunnerSnapshotV1 {
    pub observed_at_unix_ms: u64,
    pub assignments: Vec<AgentSeatAssignmentV1>,
    pub runners: Vec<RunnerPresenceSnapshotV1>,
    pub activations: Vec<SeatActivationAggregateV1>,
    pub instances: Vec<LocalRunnerInstanceSnapshotV1>,
}

/// Exact aggregate source. It cannot return Invocation Context or payloads.
pub trait RunnerAttentionSourceV1: Send + Sync + 'static {
    /// Reads current assignment, durable Activation counts, presence, and lifecycle facts.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or invalid-data result.
    fn snapshot(&self) -> Result<AuthoritativeRunnerSnapshotV1, RunnerAttentionSourceErrorV1>;
}

/// Typed restart control for one preconfigured instance.
pub trait ApprovedRunnerRestartV1: Send + Sync + 'static {
    /// Restarts the exact installed instance.
    ///
    /// # Errors
    ///
    /// Returns only not-found, definitive failure, or ambiguous publication.
    fn restart(&self, instance_id: &str) -> Result<(), RunnerRestartControlErrorV1>;
}

/// Exact persisted assignment projection. Implementations must validate the
/// immutable Agent Profile assignment and ready Task setup together; external
/// and legacy managed assignments also require an active launch sidecar.
pub trait AgentSeatAssignmentSourceV1: Send + Sync + 'static {
    /// Lists active exact agent-seat assignments in stable order.
    ///
    /// # Errors
    ///
    /// Returns closed unavailable or invalid-data when retained records disagree.
    fn assignments(&self) -> Result<Vec<AgentSeatAssignmentV1>, RunnerAttentionSourceErrorV1>;
}

/// Production exact join across immutable Agent Profile assignments, active
/// assignment MCP sidecars, and durable Task setup status. A ready managed
/// reference is visible before its first local host launch, while external and
/// legacy managed assignments retain their active-sidecar requirement.
#[derive(Clone)]
pub struct PersistedAgentSeatAssignmentSourceV1<S> {
    profiles: AgentProfileStoreV1,
    launches: AssignmentMcpLaunchRegistryV1<S>,
    setup: TaskSetupSupervisorV1,
}

impl<S> PersistedAgentSeatAssignmentSourceV1<S> {
    #[must_use]
    pub const fn new(
        profiles: AgentProfileStoreV1,
        launches: AssignmentMcpLaunchRegistryV1<S>,
        setup: TaskSetupSupervisorV1,
    ) -> Self {
        Self {
            profiles,
            launches,
            setup,
        }
    }
}

impl<S> AgentSeatAssignmentSourceV1 for PersistedAgentSeatAssignmentSourceV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    fn assignments(&self) -> Result<Vec<AgentSeatAssignmentV1>, RunnerAttentionSourceErrorV1> {
        let scopes = self
            .launches
            .active_assignment_scopes()
            .map_err(map_assignment_source_error)?;
        let mut assignments = Vec::with_capacity(scopes.len());
        for scope in &scopes {
            let profile = self
                .profiles
                .assignment(&scope.assignment_id)
                .map_err(map_agent_profile_error)?;
            assignments.push(
                self.join_assignment(&profile, Some(scope))?
                    .ok_or(RunnerAttentionSourceErrorV1::InvalidData)?,
            );
        }
        for profile in self
            .profiles
            .assignments()
            .map_err(map_agent_profile_error)?
            .assignments
        {
            if !matches!(
                &profile.execution,
                AgentExecutionBindingV1::ManagedReference { .. }
            ) || scopes
                .iter()
                .any(|scope| scope.assignment_id == profile.assignment_id)
            {
                continue;
            }
            if let Some(assignment) = self.join_assignment(&profile, None)? {
                assignments.push(assignment);
            }
        }
        assignments.sort_by(|left, right| {
            (&left.room_id, &left.seat_id).cmp(&(&right.room_id, &right.seat_id))
        });
        if assignments.len() > MAX_ROWS
            || assignments.windows(2).any(|window| {
                window[0].assignment_id == window[1].assignment_id
                    || (window[0].room_id == window[1].room_id
                        && window[0].seat_id == window[1].seat_id)
            })
        {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        Ok(assignments)
    }
}

impl<S> PersistedAgentSeatAssignmentSourceV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    fn join_assignment(
        &self,
        profile: &AgentProfileSeatAssignmentV1,
        scope: Option<&crate::assignment_mcp::ActiveAssignmentScopeV1>,
    ) -> Result<Option<AgentSeatAssignmentV1>, RunnerAttentionSourceErrorV1> {
        if let Some(scope) = scope
            && (profile.assignment_id != scope.assignment_id
                || profile.membership.room_id != scope.room_id
                || profile.membership.member_id != scope.member_id)
        {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let setup = self
            .setup
            .status(&profile.draft_id)
            .map_err(map_task_setup_error)?;
        let metadata = self
            .setup
            .exact_metadata(&profile.draft_id)
            .map_err(map_task_setup_error)?;
        if setup.room_id != metadata.room_id || setup.state != metadata.state {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        if scope.is_none() && metadata.state != TaskSetupStateV1::Ready {
            return Ok(None);
        }
        if setup.room_id != profile.membership.room_id {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let seat = setup
            .seats
            .iter()
            .find(|seat| seat.seat_id == profile.seat_id)
            .ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
        if seat.member_id.as_deref() != Some(profile.membership.member_id.as_str()) {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let (runner_id, instance_id, pack) = match &profile.execution {
            AgentExecutionBindingV1::External => {
                if seat.managed_runner.is_some() {
                    return Err(RunnerAttentionSourceErrorV1::InvalidData);
                }
                let scope = scope.ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
                (scope.runner_id.clone(), None, scope.pack.clone())
            }
            AgentExecutionBindingV1::Managed { runner_id } => {
                let scope = scope.ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
                let managed = seat
                    .managed_runner
                    .as_ref()
                    .ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
                if runner_id != &scope.runner_id {
                    return Err(RunnerAttentionSourceErrorV1::InvalidData);
                }
                (
                    scope.runner_id.clone(),
                    Some(managed.instance_id.clone()),
                    scope.pack.clone(),
                )
            }
            AgentExecutionBindingV1::ManagedReference {
                runner_id,
                instance_id,
                template_id,
                template_revision,
            } => {
                let managed = self
                    .setup
                    .managed_reference_metadata(profile)
                    .map_err(map_task_setup_error)?;
                if runner_id != &managed.runner_id
                    || instance_id != &managed.instance_id
                    || template_id != &managed.template_id
                    || template_revision != &managed.template_revision
                {
                    return Err(RunnerAttentionSourceErrorV1::InvalidData);
                }
                if let Some(scope) = scope
                    && (managed.runner_id != scope.runner_id || managed.pack != scope.pack)
                {
                    return Err(RunnerAttentionSourceErrorV1::InvalidData);
                }
                (managed.runner_id, Some(managed.instance_id), managed.pack)
            }
        };
        Ok(Some(AgentSeatAssignmentV1 {
            assignment_id: profile.assignment_id.clone(),
            room_id: profile.membership.room_id.clone(),
            seat_id: profile.seat_id.clone(),
            member_id: profile.membership.member_id.clone(),
            runner_id,
            instance_id,
            managed_reference: matches!(
                &profile.execution,
                AgentExecutionBindingV1::ManagedReference { .. }
            ),
            pack,
        }))
    }
}

/// Host-authorized daemon aggregate boundary.
pub trait DaemonRunnerAttentionSourceV1: Send + Sync + 'static {
    /// Reads live presence for one exact persisted Runner identity.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or invalid-data result.
    fn presence(
        &self,
        runner_id: &str,
    ) -> Result<RunnerPresenceSnapshotV1, RunnerAttentionSourceErrorV1>;

    /// Reads Pending and Leased counts for one exact persisted Room/Membership.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or invalid-data result.
    fn activation_counts(
        &self,
        room_id: &str,
        member_id: &str,
    ) -> Result<DaemonActivationCountsV1, RunnerAttentionSourceErrorV1>;
}

/// Strict wire-compatible aggregate returned by the daemon operator route.
pub type DaemonActivationCountsV1 = OperatorActivationStatusV1;

/// Production composition of exact local assignments, daemon aggregates, and
/// approved local Runner instance lifecycle.
#[derive(Clone)]
pub struct LiveRunnerAttentionSourceV1<A, D> {
    assignments: A,
    daemon: D,
    runners: RunnerSupervisorV1,
}

impl<A, D> LiveRunnerAttentionSourceV1<A, D> {
    #[must_use]
    pub const fn new(assignments: A, daemon: D, runners: RunnerSupervisorV1) -> Self {
        Self {
            assignments,
            daemon,
            runners,
        }
    }
}

impl<A, D> RunnerAttentionSourceV1 for LiveRunnerAttentionSourceV1<A, D>
where
    A: AgentSeatAssignmentSourceV1,
    D: DaemonRunnerAttentionSourceV1,
{
    fn snapshot(&self) -> Result<AuthoritativeRunnerSnapshotV1, RunnerAttentionSourceErrorV1> {
        let assignments = self.assignments.assignments()?;
        if assignments.len() > MAX_ROWS {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let mut runner_ids = assignments
            .iter()
            .map(|assignment| assignment.runner_id.clone())
            .collect::<Vec<_>>();
        runner_ids.sort();
        runner_ids.dedup();
        let mut runners = Vec::with_capacity(runner_ids.len());
        for runner_id in &runner_ids {
            match self.daemon.presence(runner_id) {
                Ok(runner) => runners.push(runner),
                Err(RunnerAttentionSourceErrorV1::Unavailable)
                    if assignments
                        .iter()
                        .filter(|assignment| assignment.runner_id == *runner_id)
                        .all(|assignment| assignment.managed_reference) => {}
                Err(error) => return Err(error),
            }
        }
        let mut observed_at_unix_ms = runners
            .iter()
            .map(|runner| runner.observed_at_unix_ms)
            .max()
            .unwrap_or(1);
        let activations = assignments
            .iter()
            .map(|assignment| {
                let counts = self
                    .daemon
                    .activation_counts(&assignment.room_id, &assignment.member_id)?;
                if counts.version != "worldstream/operator-activation-status/v1"
                    || counts.observed_at_unix_ms == 0
                {
                    return Err(RunnerAttentionSourceErrorV1::InvalidData);
                }
                observed_at_unix_ms = observed_at_unix_ms.max(counts.observed_at_unix_ms);
                Ok(SeatActivationAggregateV1 {
                    room_id: assignment.room_id.clone(),
                    member_id: assignment.member_id.clone(),
                    waiting: counts.waiting,
                    leased: counts.leased,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let instances = self
            .runners
            .statuses()
            .instances
            .into_iter()
            .map(|instance| LocalRunnerInstanceSnapshotV1 {
                instance_id: instance.instance_id,
                state: match instance.state {
                    RunnerInstanceStateV1::Stopped => LocalRunnerStateV1::Stopped,
                    RunnerInstanceStateV1::Starting | RunnerInstanceStateV1::Stopping => {
                        LocalRunnerStateV1::Restarting
                    }
                    RunnerInstanceStateV1::Running => LocalRunnerStateV1::Running,
                    RunnerInstanceStateV1::Failed => LocalRunnerStateV1::Failed,
                    RunnerInstanceStateV1::Unavailable => LocalRunnerStateV1::Unavailable,
                },
                healthy: matches!(
                    instance.health,
                    crate::runner_templates::RunnerInstanceHealthV1::Healthy
                ),
            })
            .collect();
        Ok(AuthoritativeRunnerSnapshotV1 {
            observed_at_unix_ms,
            assignments,
            runners,
            activations,
            instances,
        })
    }
}

impl ApprovedRunnerRestartV1 for RunnerSupervisorV1 {
    fn restart(&self, instance_id: &str) -> Result<(), RunnerRestartControlErrorV1> {
        let response = RunnerSupervisorV1::restart(self, instance_id)
            .ok_or(RunnerRestartControlErrorV1::NotFound)?;
        let instance = response
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .ok_or(RunnerRestartControlErrorV1::Failed)?;
        match instance.state {
            RunnerInstanceStateV1::Starting
            | RunnerInstanceStateV1::Stopping
            | RunnerInstanceStateV1::Running => Ok(()),
            RunnerInstanceStateV1::Stopped
            | RunnerInstanceStateV1::Failed
            | RunnerInstanceStateV1::Unavailable => Err(RunnerRestartControlErrorV1::Failed),
        }
    }
}

/// Fixed-loopback authenticated daemon client for bounded operational reads.
#[derive(Clone)]
pub struct HttpDaemonRunnerAttentionSourceV1 {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
    managed: Option<crate::managed_daemon_transport::ManagedDaemonTransport>,
}

impl HttpDaemonRunnerAttentionSourceV1 {
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
            managed: None,
        }
    }

    /// Reads existing Runner evidence on the proved managed Runtime connection.
    #[must_use]
    pub fn new_managed(
        address: SocketAddr,
        timeout: Duration,
        vault: FileSecretVaultV1,
        host_authority: Option<SecretReferenceV1>,
        ownership: crate::process_ownership::ProcessOwnership,
    ) -> Self {
        Self {
            address,
            timeout,
            vault,
            host_authority,
            managed: Some(
                crate::managed_daemon_transport::ManagedDaemonTransport::new(
                    ownership, address, timeout,
                ),
            ),
        }
    }

    fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, RunnerAttentionSourceErrorV1> {
        if !self.address.ip().is_loopback()
            || self.timeout.is_zero()
            || self.timeout > Duration::from_secs(30)
        {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(RunnerAttentionSourceErrorV1::Unavailable)?;
        if let Some(transport) = &self.managed {
            let response = transport
                .request("GET", path, b"", MAX_MESSAGE_BYTES, || {
                    let secret = self
                        .vault
                        .resolve(SecretKindV1::HostAuthority, reference)
                        .map_err(|_| ())?;
                    let mut bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                    let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
                    bytes.zeroize();
                    let token = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
                    let mut header = axum::http::HeaderValue::from_str(&token).map_err(|_| ())?;
                    header.set_sensitive(true);
                    Ok::<_, ()>(header)
                })
                .map_err(|error| match error {
                    crate::verified_control::ControlTransportError::Protocol => {
                        RunnerAttentionSourceErrorV1::InvalidData
                    }
                    _ => RunnerAttentionSourceErrorV1::Unavailable,
                })?;
            if response.status != 200 {
                return Err(if matches!(response.status, 400 | 409 | 422) {
                    RunnerAttentionSourceErrorV1::InvalidData
                } else {
                    RunnerAttentionSourceErrorV1::Unavailable
                });
            }
            return serde_json::from_slice(&response.body)
                .map_err(|_| RunnerAttentionSourceErrorV1::InvalidData);
        }
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| RunnerAttentionSourceErrorV1::Unavailable)?;
        let mut bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RunnerAttentionSourceErrorV1::Unavailable)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        bytes.zeroize();
        let request = Zeroizing::new(format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            self.address,
            bearer.as_str(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| RunnerAttentionSourceErrorV1::Unavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .and_then(|()| stream.write_all(request.as_bytes()))
            .map_err(|_| RunnerAttentionSourceErrorV1::Unavailable)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut response)
            .map_err(|_| RunnerAttentionSourceErrorV1::Unavailable)?;
        if response.len() > MAX_MESSAGE_BYTES {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let (status, body) = parse_http_response(&response)?;
        if status != 200 {
            return Err(if status == 404 {
                RunnerAttentionSourceErrorV1::Unavailable
            } else if matches!(status, 400 | 409 | 422) {
                RunnerAttentionSourceErrorV1::InvalidData
            } else {
                RunnerAttentionSourceErrorV1::Unavailable
            });
        }
        serde_json::from_slice(body).map_err(|_| RunnerAttentionSourceErrorV1::InvalidData)
    }
}

impl DaemonRunnerAttentionSourceV1 for HttpDaemonRunnerAttentionSourceV1 {
    fn presence(
        &self,
        runner_id: &str,
    ) -> Result<RunnerPresenceSnapshotV1, RunnerAttentionSourceErrorV1> {
        if runner_id.parse::<UlidString>().is_err() {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        let presence: OperatorRunnerPresenceV1 =
            self.get(&format!("/v1/operator/runners/{runner_id}/presence"))?;
        if presence.version != "worldstream/operator-runner-presence/v1"
            || presence.runner_id != runner_id
        {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        Ok(RunnerPresenceSnapshotV1 {
            runner_id: presence.runner_id,
            connection: presence.connection,
            freshness: presence.freshness,
            maximum_concurrent_activations: presence.maximum_concurrent_activations,
            active_activations: presence.active_activations,
            available_activations: presence.available_activations,
            supported_pack_revisions: presence.supported_pack_revisions,
            observed_at_unix_ms: presence.observed_at_unix_ms,
        })
    }

    fn activation_counts(
        &self,
        room_id: &str,
        member_id: &str,
    ) -> Result<DaemonActivationCountsV1, RunnerAttentionSourceErrorV1> {
        if room_id.parse::<UlidString>().is_err() || member_id.parse::<UlidString>().is_err() {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        }
        self.get(&format!(
            "/v1/operator/rooms/{room_id}/members/{member_id}/activation-status"
        ))
    }
}

/// Freshness of the composed Supervisor snapshot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerAttentionFreshnessV1 {
    Live,
    Stale,
    Unavailable,
}

impl RunnerAttentionFreshnessV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Exact compatibility of a Runner with the assigned immutable pack revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerCompatibilityV1 {
    Compatible,
    Incompatible,
    Unavailable,
}

/// Browser-safe advertised and used capacity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerAttentionCapacityV1 {
    pub advertised: u32,
    pub in_use: u32,
    pub available: u32,
}

/// Closed agent-seat Activation state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationAttentionStateV1 {
    Idle,
    Waiting,
    Leased,
    Delayed,
    Attention,
    Unavailable,
}

/// Browser-safe Activation counts. No Activation or claim identity is present.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationAttentionV1 {
    pub state: ActivationAttentionStateV1,
    pub waiting: u32,
    pub leased: u32,
}

/// Task-detail status for one exact persisted agent seat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSeatAttentionV1 {
    pub seat_id: String,
    pub instance_id: Option<String>,
    pub compatibility: RunnerCompatibilityV1,
    pub capacity: RunnerAttentionCapacityV1,
    pub activation: ActivationAttentionV1,
    pub freshness: RunnerAttentionFreshnessV1,
    pub next_action: String,
}

/// Browser-safe Task detail response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAgentAttentionResponseV1 {
    pub schema: String,
    pub freshness: RunnerAttentionFreshnessV1,
    pub observed_at_unix_ms: Option<u64>,
    pub seats: Vec<AgentSeatAttentionV1>,
}

/// Browser-safe registered Runner status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerAttentionStatusV1 {
    pub runner_id: String,
    pub instance_id: Option<String>,
    pub connection: OperatorRunnerConnectionV1,
    pub freshness: RunnerAttentionFreshnessV1,
    pub capacity: RunnerAttentionCapacityV1,
    pub compatible_assignments: u32,
    pub incompatible_assignments: u32,
    pub observed_at_unix_ms: Option<u64>,
    pub next_action: String,
}

/// Closed durable restart progress.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerRestartOperationStateV1 {
    Requested,
    Restarting,
    Reconciling,
    Succeeded,
    Failed,
}

/// Durable stable restart attempt and safe final outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerRestartOperationV1 {
    schema: String,
    pub operation_id: String,
    pub instance_id: String,
    pub attempts: u32,
    pub state: RunnerRestartOperationStateV1,
    pub explanation: String,
    pub next_action: String,
}

/// Browser-safe Operations response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerAttentionOperationsResponseV1 {
    pub schema: String,
    pub freshness: RunnerAttentionFreshnessV1,
    pub observed_at_unix_ms: Option<u64>,
    pub runners: Vec<RunnerAttentionStatusV1>,
    pub managed_hosts: Vec<ManagedAgentHostStatusV1>,
    pub restart_attempts: Vec<RunnerRestartOperationV1>,
}

/// Owner-only durable restart operation store.
#[derive(Clone)]
pub struct FileRunnerRestartStoreV1 {
    root: Arc<PathBuf>,
    mutation: Arc<Mutex<()>>,
}

impl FileRunnerRestartStoreV1 {
    /// Opens the protected restart operation directory.
    ///
    /// # Errors
    ///
    /// Returns unavailable when owner-only storage cannot be prepared.
    pub fn open(root: &Path) -> Result<Self, RunnerAttentionErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| RunnerAttentionErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            mutation: Arc::new(Mutex::new(())),
        })
    }

    fn begin(
        &self,
        instance_id: &str,
        operation_id: &str,
    ) -> Result<(RunnerRestartOperationV1, bool), RunnerAttentionErrorV1> {
        validate_instance_id(instance_id)?;
        validate_operation_id(operation_id)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = self.load(operation_id)? {
            return if existing.instance_id == instance_id {
                Ok((existing, false))
            } else {
                Err(RunnerAttentionErrorV1::Conflict)
            };
        }
        if self.list()?.len() >= MAX_ROWS {
            return Err(RunnerAttentionErrorV1::Unavailable);
        }
        let record = RunnerRestartOperationV1 {
            schema: RESTART_SCHEMA_V1.to_owned(),
            operation_id: operation_id.to_owned(),
            instance_id: instance_id.to_owned(),
            attempts: 0,
            state: RunnerRestartOperationStateV1::Requested,
            explanation: "Approved Runner restart is queued.".to_owned(),
            next_action: "Wait for restart progress.".to_owned(),
        };
        self.persist(&record)?;
        Ok((record, true))
    }

    fn update(&self, record: &RunnerRestartOperationV1) -> Result<(), RunnerAttentionErrorV1> {
        validate_restart(record)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        self.persist(record)
    }

    fn load(
        &self,
        operation_id: &str,
    ) -> Result<Option<RunnerRestartOperationV1>, RunnerAttentionErrorV1> {
        validate_operation_id(operation_id)?;
        let path = self.root.join(format!("{operation_id}.json"));
        if !path.exists() {
            return Ok(None);
        }
        validate_owner_only_file(&path).map_err(|_| RunnerAttentionErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| RunnerAttentionErrorV1::Unavailable)?;
        if metadata.len() == 0 || metadata.len() > MAX_RECORD_BYTES {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
        let record: RunnerRestartOperationV1 = serde_json::from_slice(
            &fs::read(path).map_err(|_| RunnerAttentionErrorV1::Unavailable)?,
        )
        .map_err(|_| RunnerAttentionErrorV1::InvalidData)?;
        validate_restart(&record)?;
        if record.operation_id != operation_id {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
        Ok(Some(record))
    }

    fn list(&self) -> Result<Vec<RunnerRestartOperationV1>, RunnerAttentionErrorV1> {
        let mut records = Vec::new();
        for entry in
            fs::read_dir(self.root.as_ref()).map_err(|_| RunnerAttentionErrorV1::Unavailable)?
        {
            let path = entry
                .map_err(|_| RunnerAttentionErrorV1::Unavailable)?
                .path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let operation_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or(RunnerAttentionErrorV1::InvalidData)?;
            let record = self
                .load(operation_id)?
                .ok_or(RunnerAttentionErrorV1::InvalidData)?;
            records.push(record);
            if records.len() > MAX_ROWS {
                return Err(RunnerAttentionErrorV1::InvalidData);
            }
        }
        records.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        Ok(records)
    }

    fn persist(&self, record: &RunnerRestartOperationV1) -> Result<(), RunnerAttentionErrorV1> {
        let bytes = serde_json::to_vec(record).map_err(|_| RunnerAttentionErrorV1::InvalidData)?;
        if bytes.is_empty() || bytes.len() > usize::try_from(MAX_RECORD_BYTES).unwrap_or(usize::MAX)
        {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
        let target = self.root.join(format!("{}.json", record.operation_id));
        let temporary = self.root.join(format!(
            ".{}.{}.tmp",
            record.operation_id,
            temporary_suffix()?
        ));
        let mut file = create_owner_only_renameable_file(&temporary)
            .map_err(|_| RunnerAttentionErrorV1::Unavailable)?;
        let write_result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_file(&temporary, &target))
            .and_then(|()| sync_directory(self.root.as_ref()));
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(RunnerAttentionErrorV1::Unavailable);
        }
        Ok(())
    }
}

/// Composes exact assignment, durable Activation, live presence, and approved lifecycle facts.
pub struct RunnerAttentionSupervisorV1<S, R> {
    source: S,
    restart: R,
    store: FileRunnerRestartStoreV1,
    retained: Arc<Mutex<Option<AuthoritativeRunnerSnapshotV1>>>,
    managed_hosts: Option<ManagedAgentHostOperationsV1>,
}

impl<S: Clone, R: Clone> Clone for RunnerAttentionSupervisorV1<S, R> {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            restart: self.restart.clone(),
            store: self.store.clone(),
            retained: Arc::clone(&self.retained),
            managed_hosts: self.managed_hosts.clone(),
        }
    }
}

impl<S, R> RunnerAttentionSupervisorV1<S, R>
where
    S: RunnerAttentionSourceV1,
    R: ApprovedRunnerRestartV1,
{
    #[must_use]
    pub fn new(source: S, restart: R, store: FileRunnerRestartStoreV1) -> Self {
        Self {
            source,
            restart,
            store,
            retained: Arc::new(Mutex::new(None)),
            managed_hosts: None,
        }
    }

    /// Adds managed reference hosts to the existing bounded Operations projection.
    #[must_use]
    pub fn with_managed_hosts(mut self, managed_hosts: ManagedAgentHostOperationsV1) -> Self {
        self.managed_hosts = Some(managed_hosts);
        self
    }

    /// Returns current registered Runner Operations and restart attempts.
    ///
    /// # Errors
    ///
    /// Returns a closed storage or invalid-data result.
    pub fn operations(
        &self,
    ) -> Result<RunnerAttentionOperationsResponseV1, RunnerAttentionErrorV1> {
        let observed = self.observe()?;
        self.reconcile_restarts(if observed.freshness == RunnerAttentionFreshnessV1::Live {
            observed.snapshot.as_ref()
        } else {
            None
        })?;
        let restart_attempts = self.store.list()?;
        let runners = observed
            .snapshot
            .as_ref()
            .map_or_else(Vec::new, build_runner_statuses);
        let mut managed_hosts = self
            .managed_hosts
            .as_ref()
            .map_or_else(|| Ok(Vec::new()), ManagedAgentHostOperationsV1::statuses)
            .map_err(|_| RunnerAttentionErrorV1::Unavailable)?;
        if observed.freshness == RunnerAttentionFreshnessV1::Live
            && let Some(snapshot) = &observed.snapshot
        {
            apply_live_activation_status(&mut managed_hosts, snapshot);
        }
        Ok(RunnerAttentionOperationsResponseV1 {
            schema: OPERATIONS_SCHEMA_V1.to_owned(),
            freshness: observed.freshness,
            observed_at_unix_ms: observed
                .snapshot
                .as_ref()
                .map(|value| value.observed_at_unix_ms),
            runners,
            managed_hosts,
            restart_attempts,
        })
    }

    /// Returns exact agent-seat Runner and aggregate Activation attention for one Room.
    ///
    /// # Errors
    ///
    /// Returns not-found for an unknown Room and closed invalid/storage errors otherwise.
    pub fn task(
        &self,
        room_id: &str,
    ) -> Result<TaskAgentAttentionResponseV1, RunnerAttentionErrorV1> {
        validate_ulid(room_id)?;
        let observed = self.observe()?;
        let Some(snapshot) = observed.snapshot.as_ref() else {
            return Ok(TaskAgentAttentionResponseV1 {
                schema: TASK_SCHEMA_V1.to_owned(),
                freshness: RunnerAttentionFreshnessV1::Unavailable,
                observed_at_unix_ms: None,
                seats: Vec::new(),
            });
        };
        let assignments = snapshot
            .assignments
            .iter()
            .filter(|assignment| assignment.room_id == room_id)
            .collect::<Vec<_>>();
        if assignments.is_empty() {
            return Err(RunnerAttentionErrorV1::NotFound);
        }
        let seats = assignments
            .into_iter()
            .map(|assignment| build_seat(snapshot, assignment, observed.freshness))
            .collect::<Vec<_>>();
        let freshness = seats.iter().fold(observed.freshness, |current, seat| {
            least_fresh(current, seat.freshness)
        });
        Ok(TaskAgentAttentionResponseV1 {
            schema: TASK_SCHEMA_V1.to_owned(),
            freshness,
            observed_at_unix_ms: Some(snapshot.observed_at_unix_ms),
            seats,
        })
    }

    /// Requests one idempotent typed restart for an approved local instance.
    ///
    /// # Errors
    ///
    /// Returns not-found, conflict, invalid-data, or protected-storage unavailable.
    pub fn restart(
        &self,
        instance_id: &str,
        operation_id: &str,
    ) -> Result<RunnerRestartOperationV1, RunnerAttentionErrorV1> {
        let observed = self.observe()?;
        if observed.freshness != RunnerAttentionFreshnessV1::Live {
            return Err(RunnerAttentionErrorV1::Unavailable);
        }
        let snapshot = observed.snapshot.ok_or(RunnerAttentionErrorV1::NotFound)?;
        if !snapshot
            .instances
            .iter()
            .any(|instance| instance.instance_id == instance_id)
        {
            return Err(RunnerAttentionErrorV1::NotFound);
        }
        let (mut record, created) = self.store.begin(instance_id, operation_id)?;
        if !created {
            return Ok(record);
        }
        record.attempts = 1;
        record.state = RunnerRestartOperationStateV1::Restarting;
        set_restart_message(
            &mut record,
            "Approved Runner restart was accepted.",
            "Wait while the Supervisor reconciles Runner state.",
        );
        self.store.update(&record)?;
        match self.restart.restart(instance_id) {
            Ok(()) | Err(RunnerRestartControlErrorV1::Ambiguous) => {
                record.state = RunnerRestartOperationStateV1::Reconciling;
                set_restart_message(
                    &mut record,
                    "Restart was issued; authoritative state is being reconciled.",
                    "Wait for Runner presence, capacity, assignments, and Activation counts to refresh.",
                );
            }
            Err(RunnerRestartControlErrorV1::NotFound) => {
                record.state = RunnerRestartOperationStateV1::Failed;
                set_restart_message(
                    &mut record,
                    "The approved Runner instance is no longer installed.",
                    "Restore the approved Runner installation, then retry restart.",
                );
            }
            Err(RunnerRestartControlErrorV1::Failed) => {
                record.state = RunnerRestartOperationStateV1::Failed;
                set_restart_message(
                    &mut record,
                    "The approved Runner restart failed.",
                    "Inspect bounded Runner diagnostics, then retry restart.",
                );
            }
        }
        self.store.update(&record)?;
        if record.state == RunnerRestartOperationStateV1::Reconciling {
            self.reconcile_restarts(Some(&snapshot))?;
            record = self
                .store
                .load(operation_id)?
                .ok_or(RunnerAttentionErrorV1::InvalidData)?;
        }
        Ok(record)
    }

    fn observe(&self) -> Result<ObservedSnapshot, RunnerAttentionErrorV1> {
        match self.source.snapshot() {
            Ok(snapshot) => {
                validate_snapshot(&snapshot)?;
                *self.retained.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(snapshot.clone());
                Ok(ObservedSnapshot {
                    snapshot: Some(snapshot),
                    freshness: RunnerAttentionFreshnessV1::Live,
                })
            }
            Err(RunnerAttentionSourceErrorV1::Unavailable) => {
                let retained = self
                    .retained
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                Ok(ObservedSnapshot {
                    freshness: if retained.is_some() {
                        RunnerAttentionFreshnessV1::Stale
                    } else {
                        RunnerAttentionFreshnessV1::Unavailable
                    },
                    snapshot: retained,
                })
            }
            Err(RunnerAttentionSourceErrorV1::InvalidData) => {
                Err(RunnerAttentionErrorV1::InvalidData)
            }
        }
    }

    fn reconcile_restarts(
        &self,
        snapshot: Option<&AuthoritativeRunnerSnapshotV1>,
    ) -> Result<(), RunnerAttentionErrorV1> {
        for mut record in self.store.list()? {
            if !matches!(
                record.state,
                RunnerRestartOperationStateV1::Restarting
                    | RunnerRestartOperationStateV1::Reconciling
            ) {
                continue;
            }
            let Some(snapshot) = snapshot else {
                continue;
            };
            let Some(instance) = snapshot
                .instances
                .iter()
                .find(|instance| instance.instance_id == record.instance_id)
            else {
                record.state = RunnerRestartOperationStateV1::Failed;
                set_restart_message(
                    &mut record,
                    "The approved Runner instance disappeared during reconciliation.",
                    "Restore the approved Runner installation, then retry restart.",
                );
                self.store.update(&record)?;
                continue;
            };
            match instance.state {
                LocalRunnerStateV1::Running if instance.healthy => {
                    let assignments = snapshot.assignments.iter().filter(|assignment| {
                        assignment.instance_id.as_deref() == Some(record.instance_id.as_str())
                    });
                    let assignment_count = assignments.clone().count();
                    let reconciled = assignment_count == 0
                        || assignments.into_iter().all(|assignment| {
                            snapshot.runners.iter().any(|runner| {
                                runner.runner_id == assignment.runner_id
                                    && runner.connection == OperatorRunnerConnectionV1::Connected
                                    && runner.freshness == OperatorRunnerFreshnessV1::Fresh
                            }) && snapshot.activations.iter().any(|activation| {
                                activation.room_id == assignment.room_id
                                    && activation.member_id == assignment.member_id
                            })
                        });
                    if reconciled {
                        record.state = RunnerRestartOperationStateV1::Succeeded;
                        set_restart_message(
                            &mut record,
                            "Runner restart reconciled from authoritative assignment, capacity, and Activation state.",
                            "No operator action is required.",
                        );
                        self.store.update(&record)?;
                    }
                }
                LocalRunnerStateV1::Failed | LocalRunnerStateV1::Unavailable => {
                    record.state = RunnerRestartOperationStateV1::Failed;
                    set_restart_message(
                        &mut record,
                        "Runner restart could not establish a healthy approved instance.",
                        "Inspect bounded Runner diagnostics, then retry restart.",
                    );
                    self.store.update(&record)?;
                }
                LocalRunnerStateV1::Stopped
                | LocalRunnerStateV1::Restarting
                | LocalRunnerStateV1::Running => {}
            }
        }
        Ok(())
    }
}

fn apply_live_activation_status(
    hosts: &mut [ManagedAgentHostStatusV1],
    snapshot: &AuthoritativeRunnerSnapshotV1,
) {
    for host in hosts {
        let Some(assignment) = snapshot
            .assignments
            .iter()
            .find(|assignment| assignment.assignment_id == host.assignment_id)
        else {
            continue;
        };
        let Some(activation) = snapshot.activations.iter().find(|activation| {
            activation.room_id == assignment.room_id && activation.member_id == assignment.member_id
        }) else {
            continue;
        };
        host.activation.state = if activation.leased > 0 {
            ManagedAgentActivationStateV1::Leased
        } else if activation.waiting > 0 {
            ManagedAgentActivationStateV1::Waiting
        } else {
            ManagedAgentActivationStateV1::Idle
        };
    }
}

#[derive(Clone)]
struct RunnerAttentionRouterStateV1 {
    api: Arc<dyn RunnerAttentionApiV1>,
}

trait RunnerAttentionApiV1: Send + Sync {
    fn operations(&self) -> Result<RunnerAttentionOperationsResponseV1, RunnerAttentionErrorV1>;
    fn task(&self, room_id: &str) -> Result<TaskAgentAttentionResponseV1, RunnerAttentionErrorV1>;
    fn restart(
        &self,
        instance_id: &str,
        operation_id: &str,
    ) -> Result<RunnerRestartOperationV1, RunnerAttentionErrorV1>;
}

impl<S, R> RunnerAttentionApiV1 for RunnerAttentionSupervisorV1<S, R>
where
    S: RunnerAttentionSourceV1,
    R: ApprovedRunnerRestartV1,
{
    fn operations(&self) -> Result<RunnerAttentionOperationsResponseV1, RunnerAttentionErrorV1> {
        RunnerAttentionSupervisorV1::operations(self)
    }

    fn task(&self, room_id: &str) -> Result<TaskAgentAttentionResponseV1, RunnerAttentionErrorV1> {
        RunnerAttentionSupervisorV1::task(self, room_id)
    }

    fn restart(
        &self,
        instance_id: &str,
        operation_id: &str,
    ) -> Result<RunnerRestartOperationV1, RunnerAttentionErrorV1> {
        RunnerAttentionSupervisorV1::restart(self, instance_id, operation_id)
    }
}

/// Exact typed restart request. Unknown or command-shaped fields are rejected.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerRestartRequestV1 {
    operation_id: String,
}

#[derive(Serialize)]
struct RunnerAttentionErrorResponseV1 {
    code: &'static str,
    next_action: &'static str,
}

/// Builds bounded read/restart routes with no arbitrary process or command input.
pub fn runner_attention_router<S, R>(supervisor: RunnerAttentionSupervisorV1<S, R>) -> Router
where
    S: RunnerAttentionSourceV1,
    R: ApprovedRunnerRestartV1,
{
    Router::new()
        .route("/api/v1/runner-attention", get(get_runner_attention))
        .route(
            "/api/v1/rooms/{room_id}/agent-attention",
            get(get_task_agent_attention),
        )
        .route(
            "/api/v1/runner-attention/{instance_id}/restart",
            post(post_runner_restart),
        )
        .layer(axum::extract::DefaultBodyLimit::max(1024))
        .with_state(RunnerAttentionRouterStateV1 {
            api: Arc::new(supervisor),
        })
}

async fn get_runner_attention(State(state): State<RunnerAttentionRouterStateV1>) -> Response {
    let api = Arc::clone(&state.api);
    match tokio::task::spawn_blocking(move || api.operations()).await {
        Ok(Ok(status)) => Json(status).into_response(),
        Ok(Err(error)) => error_response(error),
        Err(_) => error_response(RunnerAttentionErrorV1::Unavailable),
    }
}

async fn get_task_agent_attention(
    State(state): State<RunnerAttentionRouterStateV1>,
    AxumPath(room_id): AxumPath<String>,
) -> Response {
    let api = Arc::clone(&state.api);
    match tokio::task::spawn_blocking(move || api.task(&room_id)).await {
        Ok(Ok(status)) => Json(status).into_response(),
        Ok(Err(error)) => error_response(error),
        Err(_) => error_response(RunnerAttentionErrorV1::Unavailable),
    }
}

async fn post_runner_restart(
    State(state): State<RunnerAttentionRouterStateV1>,
    AxumPath(instance_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    let request: RunnerRestartRequestV1 = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(RunnerAttentionErrorV1::InvalidData),
    };
    let api = Arc::clone(&state.api);
    match tokio::task::spawn_blocking(move || api.restart(&instance_id, &request.operation_id))
        .await
    {
        Ok(Ok(operation)) => Json(operation).into_response(),
        Ok(Err(error)) => error_response(error),
        Err(_) => error_response(RunnerAttentionErrorV1::Unavailable),
    }
}

fn error_response(error: RunnerAttentionErrorV1) -> Response {
    let (status, code, next_action) = match error {
        RunnerAttentionErrorV1::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "Refresh approved Runner and Task assignments.",
        ),
        RunnerAttentionErrorV1::Conflict => (
            StatusCode::CONFLICT,
            "operation_conflict",
            "Use a new operation identity for a different approved instance.",
        ),
        RunnerAttentionErrorV1::InvalidData => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "Refresh status and retry the typed operation.",
        ),
        RunnerAttentionErrorV1::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Retry status; inspect bounded diagnostics if unavailable persists.",
        ),
    };
    (
        status,
        Json(RunnerAttentionErrorResponseV1 { code, next_action }),
    )
        .into_response()
}

struct ObservedSnapshot {
    snapshot: Option<AuthoritativeRunnerSnapshotV1>,
    freshness: RunnerAttentionFreshnessV1,
}

fn build_runner_statuses(snapshot: &AuthoritativeRunnerSnapshotV1) -> Vec<RunnerAttentionStatusV1> {
    let mut rows = snapshot
        .runners
        .iter()
        .map(|runner| {
            let assignments = snapshot
                .assignments
                .iter()
                .filter(|assignment| assignment.runner_id == runner.runner_id)
                .collect::<Vec<_>>();
            let compatible = assignments
                .iter()
                .filter(|assignment| supports(runner, &assignment.pack))
                .count();
            let incompatible = assignments.len().saturating_sub(compatible);
            let freshness = runner_freshness(runner);
            RunnerAttentionStatusV1 {
                runner_id: runner.runner_id.clone(),
                instance_id: assignments
                    .iter()
                    .find_map(|assignment| assignment.instance_id.clone()),
                connection: runner.connection,
                freshness,
                capacity: capacity(runner),
                compatible_assignments: u32::try_from(compatible).unwrap_or(u32::MAX),
                incompatible_assignments: u32::try_from(incompatible).unwrap_or(u32::MAX),
                observed_at_unix_ms: Some(runner.observed_at_unix_ms),
                next_action: runner_next_action(runner, incompatible > 0),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.runner_id.cmp(&right.runner_id));
    rows
}

fn build_seat(
    snapshot: &AuthoritativeRunnerSnapshotV1,
    assignment: &AgentSeatAssignmentV1,
    source_freshness: RunnerAttentionFreshnessV1,
) -> AgentSeatAttentionV1 {
    let runner = snapshot
        .runners
        .iter()
        .find(|runner| runner.runner_id == assignment.runner_id);
    let aggregate = snapshot.activations.iter().find(|activation| {
        activation.room_id == assignment.room_id && activation.member_id == assignment.member_id
    });
    let local = assignment.instance_id.as_deref().and_then(|instance_id| {
        snapshot
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
    });
    let compatibility = runner.map_or(RunnerCompatibilityV1::Unavailable, |runner| {
        if supports(runner, &assignment.pack) {
            RunnerCompatibilityV1::Compatible
        } else {
            RunnerCompatibilityV1::Incompatible
        }
    });
    let freshness = runner.map_or(RunnerAttentionFreshnessV1::Unavailable, runner_freshness);
    let freshness = least_fresh(source_freshness, freshness);
    let capacity = runner.map_or(
        RunnerAttentionCapacityV1 {
            advertised: 0,
            in_use: 0,
            available: 0,
        },
        capacity,
    );
    let activation = activation_attention(aggregate, runner, local, compatibility);
    AgentSeatAttentionV1 {
        seat_id: assignment.seat_id.clone(),
        instance_id: assignment.instance_id.clone(),
        compatibility,
        capacity,
        activation,
        freshness,
        next_action: seat_next_action(activation, compatibility, runner),
    }
}

fn activation_attention(
    aggregate: Option<&SeatActivationAggregateV1>,
    runner: Option<&RunnerPresenceSnapshotV1>,
    local: Option<&LocalRunnerInstanceSnapshotV1>,
    compatibility: RunnerCompatibilityV1,
) -> ActivationAttentionV1 {
    let Some(aggregate) = aggregate else {
        return ActivationAttentionV1 {
            state: ActivationAttentionStateV1::Unavailable,
            waiting: 0,
            leased: 0,
        };
    };
    let local_delayed = local.is_some_and(|instance| {
        !matches!(instance.state, LocalRunnerStateV1::Running) || !instance.healthy
    });
    let unavailable = runner.is_none_or(|runner| {
        runner.connection == OperatorRunnerConnectionV1::Disconnected
            || runner.freshness == OperatorRunnerFreshnessV1::Stale
    });
    let state = if aggregate.leased > 0 && local_delayed {
        ActivationAttentionStateV1::Delayed
    } else if aggregate.leased > 0 {
        ActivationAttentionStateV1::Leased
    } else if aggregate.waiting > 0
        && (unavailable
            || compatibility != RunnerCompatibilityV1::Compatible
            || runner.is_none_or(|runner| runner.available_activations == 0))
    {
        ActivationAttentionStateV1::Attention
    } else if aggregate.waiting > 0 {
        ActivationAttentionStateV1::Waiting
    } else {
        ActivationAttentionStateV1::Idle
    };
    ActivationAttentionV1 {
        state,
        waiting: aggregate.waiting,
        leased: aggregate.leased,
    }
}

fn seat_next_action(
    activation: ActivationAttentionV1,
    compatibility: RunnerCompatibilityV1,
    runner: Option<&RunnerPresenceSnapshotV1>,
) -> String {
    match activation.state {
        ActivationAttentionStateV1::Idle => "No operator action is required.".to_owned(),
        ActivationAttentionStateV1::Waiting => {
            "Wait for the compatible Runner to claim work.".to_owned()
        }
        ActivationAttentionStateV1::Leased => "Wait for the current lease to complete.".to_owned(),
        ActivationAttentionStateV1::Delayed => {
            "Wait for the retained lease to expire or reconcile.".to_owned()
        }
        ActivationAttentionStateV1::Unavailable => {
            "Retry status; inspect bounded diagnostics if unavailable persists.".to_owned()
        }
        ActivationAttentionStateV1::Attention => {
            if compatibility == RunnerCompatibilityV1::Incompatible {
                "Assign an exactly compatible Runner revision, then retry status.".to_owned()
            } else if runner.is_some_and(|runner| runner.available_activations == 0) {
                "Restore compatible Runner capacity, then retry status.".to_owned()
            } else {
                "Retry status; inspect bounded Runner diagnostics if presence remains unavailable."
                    .to_owned()
            }
        }
    }
}

fn runner_next_action(runner: &RunnerPresenceSnapshotV1, incompatible: bool) -> String {
    if incompatible {
        "Assign an exactly compatible Runner revision.".to_owned()
    } else if runner.connection == OperatorRunnerConnectionV1::Disconnected
        || runner.freshness == OperatorRunnerFreshnessV1::Stale
    {
        "Retry status; inspect bounded Runner diagnostics if presence remains unavailable."
            .to_owned()
    } else if runner.available_activations == 0 {
        "Restore Runner capacity or wait for active work to complete.".to_owned()
    } else {
        "No operator action is required.".to_owned()
    }
}

fn capacity(runner: &RunnerPresenceSnapshotV1) -> RunnerAttentionCapacityV1 {
    RunnerAttentionCapacityV1 {
        advertised: runner.maximum_concurrent_activations,
        in_use: runner.active_activations,
        available: runner.available_activations,
    }
}

fn runner_freshness(runner: &RunnerPresenceSnapshotV1) -> RunnerAttentionFreshnessV1 {
    if runner.connection == OperatorRunnerConnectionV1::Disconnected {
        RunnerAttentionFreshnessV1::Unavailable
    } else if runner.freshness == OperatorRunnerFreshnessV1::Stale {
        RunnerAttentionFreshnessV1::Stale
    } else {
        RunnerAttentionFreshnessV1::Live
    }
}

const fn least_fresh(
    left: RunnerAttentionFreshnessV1,
    right: RunnerAttentionFreshnessV1,
) -> RunnerAttentionFreshnessV1 {
    match (left, right) {
        (RunnerAttentionFreshnessV1::Unavailable, _)
        | (_, RunnerAttentionFreshnessV1::Unavailable) => RunnerAttentionFreshnessV1::Unavailable,
        (RunnerAttentionFreshnessV1::Stale, _) | (_, RunnerAttentionFreshnessV1::Stale) => {
            RunnerAttentionFreshnessV1::Stale
        }
        _ => RunnerAttentionFreshnessV1::Live,
    }
}

fn supports(runner: &RunnerPresenceSnapshotV1, pack: &PackReference) -> bool {
    runner.supported_pack_revisions.iter().any(|candidate| {
        candidate.id == pack.id
            && candidate.version == pack.version
            && candidate.digest == pack.digest
    })
}

fn validate_snapshot(
    snapshot: &AuthoritativeRunnerSnapshotV1,
) -> Result<(), RunnerAttentionErrorV1> {
    if snapshot.observed_at_unix_ms == 0
        || snapshot.assignments.len() > MAX_ROWS
        || snapshot.runners.len() > MAX_ROWS
        || snapshot.activations.len() > MAX_ROWS
        || snapshot.instances.len() > MAX_ROWS
    {
        return Err(RunnerAttentionErrorV1::InvalidData);
    }
    let mut assignment_keys = BTreeSet::new();
    for assignment in &snapshot.assignments {
        validate_ulid(&assignment.room_id)?;
        validate_ulid(&assignment.member_id)?;
        validate_ulid(&assignment.runner_id)?;
        validate_instance_id(&assignment.seat_id)?;
        if let Some(instance_id) = &assignment.instance_id {
            validate_instance_id(instance_id)?;
        }
        if !valid_pack(&assignment.pack)
            || !assignment_keys.insert((assignment.room_id.clone(), assignment.seat_id.clone()))
        {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
    }
    let mut runners = BTreeSet::new();
    for runner in &snapshot.runners {
        validate_ulid(&runner.runner_id)?;
        if runner.maximum_concurrent_activations == 0
            || runner.active_activations > runner.maximum_concurrent_activations
            || runner
                .active_activations
                .checked_add(runner.available_activations)
                != Some(runner.maximum_concurrent_activations)
            || runner.supported_pack_revisions.len() > MAX_ROWS
            || runner
                .supported_pack_revisions
                .iter()
                .any(|pack| !valid_pack(pack))
            || !runners.insert(&runner.runner_id)
        {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
    }
    let mut activations = BTreeSet::new();
    for activation in &snapshot.activations {
        validate_ulid(&activation.room_id)?;
        validate_ulid(&activation.member_id)?;
        if !activations.insert((&activation.room_id, &activation.member_id)) {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
    }
    let mut instances = BTreeSet::new();
    for instance in &snapshot.instances {
        validate_instance_id(&instance.instance_id)?;
        if !instances.insert(&instance.instance_id) {
            return Err(RunnerAttentionErrorV1::InvalidData);
        }
    }
    Ok(())
}

fn validate_restart(record: &RunnerRestartOperationV1) -> Result<(), RunnerAttentionErrorV1> {
    if record.schema != RESTART_SCHEMA_V1
        || record.attempts > 1
        || record.explanation.is_empty()
        || record.explanation.len() > 512
        || record.next_action.is_empty()
        || record.next_action.len() > 512
        || record.explanation.contains(['\0', '\r', '\n'])
        || record.next_action.contains(['\0', '\r', '\n'])
        || match record.state {
            RunnerRestartOperationStateV1::Requested => record.attempts != 0,
            RunnerRestartOperationStateV1::Restarting
            | RunnerRestartOperationStateV1::Reconciling
            | RunnerRestartOperationStateV1::Succeeded
            | RunnerRestartOperationStateV1::Failed => record.attempts != 1,
        }
    {
        return Err(RunnerAttentionErrorV1::InvalidData);
    }
    validate_operation_id(&record.operation_id)?;
    validate_instance_id(&record.instance_id)
}

fn set_restart_message(
    record: &mut RunnerRestartOperationV1,
    explanation: &str,
    next_action: &str,
) {
    explanation.clone_into(&mut record.explanation);
    next_action.clone_into(&mut record.next_action);
}

fn validate_operation_id(value: &str) -> Result<(), RunnerAttentionErrorV1> {
    validate_ulid(value)
}

fn validate_ulid(value: &str) -> Result<(), RunnerAttentionErrorV1> {
    value
        .parse::<UlidString>()
        .map(|_| ())
        .map_err(|_| RunnerAttentionErrorV1::InvalidData)
}

fn validate_instance_id(value: &str) -> Result<(), RunnerAttentionErrorV1> {
    if !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        Ok(())
    } else {
        Err(RunnerAttentionErrorV1::InvalidData)
    }
}

fn valid_pack(pack: &PackReference) -> bool {
    !pack.id.is_empty()
        && pack.id.len() <= 128
        && !pack.version.is_empty()
        && pack.version.len() <= 128
        && pack.digest.starts_with("blake3:")
        && pack.digest.len() == 71
        && pack.digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive a field reference"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}

const fn map_assignment_source_error(
    error: AssignedMembershipSourceErrorV1,
) -> RunnerAttentionSourceErrorV1 {
    match error {
        AssignedMembershipSourceErrorV1::Unavailable => RunnerAttentionSourceErrorV1::Unavailable,
        AssignedMembershipSourceErrorV1::NotFound
        | AssignedMembershipSourceErrorV1::Invalid
        | AssignedMembershipSourceErrorV1::Revoked => RunnerAttentionSourceErrorV1::InvalidData,
    }
}

const fn map_agent_profile_error(error: AgentProfileErrorV1) -> RunnerAttentionSourceErrorV1 {
    match error {
        AgentProfileErrorV1::Unavailable => RunnerAttentionSourceErrorV1::Unavailable,
        AgentProfileErrorV1::InvalidProfile
        | AgentProfileErrorV1::ImmutableRevisionConflict
        | AgentProfileErrorV1::InvalidAssignment
        | AgentProfileErrorV1::ImmutableAssignmentConflict
        | AgentProfileErrorV1::NotFound => RunnerAttentionSourceErrorV1::InvalidData,
    }
}

const fn map_task_setup_error(error: TaskSetupErrorV1) -> RunnerAttentionSourceErrorV1 {
    match error {
        TaskSetupErrorV1::Unavailable => RunnerAttentionSourceErrorV1::Unavailable,
        TaskSetupErrorV1::InvalidCreation
        | TaskSetupErrorV1::NotFound
        | TaskSetupErrorV1::NotReady => RunnerAttentionSourceErrorV1::InvalidData,
    }
}

fn parse_http_response(response: &[u8]) -> Result<(u16, &[u8]), RunnerAttentionSourceErrorV1> {
    let boundary = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
    let headers = std::str::from_utf8(&response[..boundary])
        .map_err(|_| RunnerAttentionSourceErrorV1::InvalidData)?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
    let mut content_length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(RunnerAttentionSourceErrorV1::InvalidData);
        };
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(RunnerAttentionSourceErrorV1::InvalidData);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| RunnerAttentionSourceErrorV1::InvalidData)?,
            );
        }
    }
    let body = response
        .get(boundary + 4..)
        .ok_or(RunnerAttentionSourceErrorV1::InvalidData)?;
    if content_length != Some(body.len()) {
        return Err(RunnerAttentionSourceErrorV1::InvalidData);
    }
    Ok((status, body))
}

fn temporary_suffix() -> Result<String, RunnerAttentionErrorV1> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RunnerAttentionErrorV1::Unavailable)?;
    let mut encoded = String::with_capacity(32);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source
        .to_str()
        .ok_or_else(|| std::io::Error::other("source path is not Unicode"))?;
    let target = target
        .to_str()
        .ok_or_else(|| std::io::Error::other("target path is not Unicode"))?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}
