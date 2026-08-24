//! Assignment-bound local MCP observation helper.
//!
//! The public tool boundary never accepts Room, Membership, credential, path,
//! query, or Activity-specific input. A Supervisor-issued context seals one
//! exact participant Membership and keeps participant authority separate from
//! Runner-control authority.

use std::{
    collections::{BTreeSet, HashMap},
    fmt, fs,
    io::{self, BufRead, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use crate::{
    activity_packs::{ActivityPackProxyErrorV1, DaemonActivityPackSource},
    assignment_mcp_actions::{
        AssignmentMcpActionErrorV1, AssignmentMcpActionSchemaErrorV1,
        AssignmentMcpActionSchemaSourceV1, AssignmentMcpSubmitActionV1,
        FixedDaemonAssignmentMcpActionGatewayV1, list_current_action_offers, submit_current_action,
    },
    assignment_mcp_activation_ledger::FileActivationOperationLedgerV1,
    assignment_mcp_activations::{
        ActivationToolErrorV1, AssignedRunnerActivationAuthorityV1, AssignmentActivationToolsV1,
        FixedDaemonRunnerActivationGatewayV1, RunnerActivationGatewayErrorV1,
        SystemActivationLeaseClockV1,
    },
    assignment_mcp_operations::FileAssignmentMcpOperationLedgerV1,
    secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderValue, StatusCode},
    routing::{delete, post},
};
use clap::Parser;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use tungstenite::handshake::client::generate_key;
use tungstenite::{Message, WebSocket, client, http};
use worldstream_protocol::{
    ActionOffer, ActivityPackCatalogAction, BearerWireV1, ClientHello, ClientMode, ErrorCode,
    MAX_MESSAGE_BYTES, ObservationDeliver, PROTOCOL_VERSION, PackReference, PrincipalKind,
    Projection, ProjectionReset, ProtocolErrorBody, REQUIRED_CLIENT_CAPABILITIES, RoomAttached,
    RoomHead, SealedCapabilityBearerV1, ServerWelcome, SyncBranch, UlidString, VersionedEnvelope,
    WEBSOCKET_SUBPROTOCOL, decode_envelope,
};
use worldstream_runtime::{prepare_data_directory, validate_owner_only_file};
use zeroize::{Zeroize, Zeroizing};

const MAX_MCP_LINE_BYTES: usize = 64 * 1024;
const MAX_TEXT_BYTES: usize = 256;
const MAX_ACTIVE_ASSIGNMENTS: usize = 256;
const MAX_OBSERVATIONS: usize = 10_000;
const CONTINUITY_SCHEMA_V1: &str = "worldstream/assignment-mcp-continuity/v1";
const MAX_CONTINUITY_BYTES: u64 = 64 * 1024;
const MAX_SESSION_BYTES: usize = 64 * 1024 * 1024;
const MAX_SESSION_MESSAGES: usize = MAX_SESSION_BYTES / MAX_MESSAGE_BYTES;

/// Bounded production CLI accepted by the local stdio helper executable.
#[derive(Parser)]
#[command(
    name = "worldstream-assignment-mcp",
    version,
    about = "Assignment-bound local WorldStream MCP helper"
)]
pub struct AssignmentMcpCliV1 {
    /// Opaque Supervisor-issued assignment launch registration.
    #[arg(long)]
    launch_reference: String,

    /// Owner-only Supervisor state directory.
    #[arg(long, default_value = ".worldstream/studio")]
    state_dir: PathBuf,
}

impl AssignmentMcpCliV1 {
    #[must_use]
    pub fn launch_reference(&self) -> &str {
        &self.launch_reference
    }

    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

/// Sealed participant authority for one immutable Agent Profile assignment.
pub struct AssignedMembershipAuthorityV1 {
    assignment_id: String,
    profile_id: String,
    profile_revision: String,
    role: String,
    principal_id: String,
    room_id: String,
    member_id: String,
    bearer: SealedCapabilityBearerV1,
}

impl AssignedMembershipAuthorityV1 {
    /// Constructs an exact Supervisor-resolved assignment context.
    ///
    /// # Errors
    ///
    /// Rejects malformed identities or unbounded generic labels.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        assignment_id: &str,
        profile_id: &str,
        profile_revision: &str,
        role: &str,
        principal_id: &str,
        room_id: &str,
        member_id: &str,
        bearer: SealedCapabilityBearerV1,
    ) -> Result<Self, AssignedMembershipSourceErrorV1> {
        if assignment_id.parse::<UlidString>().is_err()
            || principal_id.parse::<UlidString>().is_err()
            || room_id.parse::<UlidString>().is_err()
            || member_id.parse::<UlidString>().is_err()
            || !bounded_label(profile_id)
            || !bounded_label(profile_revision)
            || !bounded_label(role)
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        Ok(Self {
            assignment_id: assignment_id.to_owned(),
            profile_id: profile_id.to_owned(),
            profile_revision: profile_revision.to_owned(),
            role: role.to_owned(),
            principal_id: principal_id.to_owned(),
            room_id: room_id.to_owned(),
            member_id: member_id.to_owned(),
            bearer,
        })
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub fn room_id(&self) -> &str {
        &self.room_id
    }

    #[must_use]
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    #[must_use]
    pub fn member_id(&self) -> &str {
        &self.member_id
    }

    #[must_use]
    pub const fn bearer(&self) -> &SealedCapabilityBearerV1 {
        &self.bearer
    }
}

impl fmt::Debug for AssignedMembershipAuthorityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AssignedMembershipAuthorityV1(REDACTED)")
    }
}

/// Closed assignment resolution failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AssignedMembershipSourceErrorV1 {
    #[error("assignment not found")]
    NotFound,
    #[error("assignment is invalid")]
    Invalid,
    #[error("assignment authority is revoked")]
    Revoked,
    #[error("assignment source is unavailable")]
    Unavailable,
}

/// Supervisor-only source that resolves one exact immutable assignment.
pub trait AssignedMembershipSourceV1: Send + Sync + 'static {
    /// Resolves exact Membership participant authority without exposing it.
    ///
    /// # Errors
    ///
    /// Returns a closed assignment or credential state.
    fn resolve_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipAuthorityV1, AssignedMembershipSourceErrorV1>;
}

/// Exact non-secret launch binding resolved only inside the Supervisor.
#[derive(Clone)]
pub struct AssignedMembershipLaunchBindingV1 {
    pub assignment_id: String,
    pub profile_id: String,
    pub profile_revision: String,
    pub role: String,
    pub principal_id: String,
    pub room_id: String,
    pub member_id: String,
    pub pack: PackReference,
    pub authority_reference: SecretReferenceV1,
    pub runner_id: String,
    pub runner_authority_reference: SecretReferenceV1,
}

/// Supervisor-only source for an immutable launch registration.
pub trait AssignedMembershipLaunchSourceV1: Send + Sync + 'static {
    /// Resolves one exact assignment to non-secret immutable launch material.
    ///
    /// # Errors
    ///
    /// Fails closed for missing, invalid, or revoked assignments.
    fn resolve_launch_binding(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipLaunchBindingV1, AssignedMembershipSourceErrorV1>;
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssignmentMcpLaunchRecordV1 {
    schema: String,
    launch_reference: String,
    assignment_id: String,
    profile_id: String,
    profile_revision: String,
    role: String,
    principal_id: String,
    room_id: String,
    member_id: String,
    pack: PackReference,
    action_schemas: Vec<ActivityPackCatalogAction>,
    authority_reference: SecretReferenceV1,
    daemon: SocketAddr,
    timeout_ms: u64,
    activation_binding_hash: String,
    binding_hash: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssignmentMcpActivationLaunchRecordV1 {
    schema: String,
    launch_reference: String,
    assignment_id: String,
    runner_id: String,
    room_id: String,
    member_id: String,
    pack: PackReference,
    authority_reference: SecretReferenceV1,
    binding_hash: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssignmentMcpActiveLaunchV1 {
    schema: String,
    assignment_id: String,
    launch_reference: String,
}

/// Browser-safe fields needed to join an active assignment to aggregate
/// Runner status. This deliberately cannot be serialized.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ActiveAssignmentScopeV1 {
    pub assignment_id: String,
    pub room_id: String,
    pub member_id: String,
    pub runner_id: String,
    pub pack: PackReference,
}

impl fmt::Debug for ActiveAssignmentScopeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveAssignmentScopeV1")
            .field("assignment_id", &self.assignment_id)
            .field("room_id", &"[redacted]")
            .field("member_id", &"[redacted]")
            .field("runner_id", &"[redacted]")
            .field("pack", &self.pack)
            .finish()
    }
}

#[derive(Serialize)]
struct LaunchBindingHashInputV1<'a> {
    domain: &'static str,
    launch_reference: &'a str,
    assignment_id: &'a str,
    profile_id: &'a str,
    profile_revision: &'a str,
    role: &'a str,
    principal_id: &'a str,
    room_id: &'a str,
    member_id: &'a str,
    pack: &'a PackReference,
    action_schemas: &'a [ActivityPackCatalogAction],
    authority_reference: &'a SecretReferenceV1,
    daemon: SocketAddr,
    timeout_ms: u64,
    activation_binding_hash: &'a str,
}

#[derive(Serialize)]
struct ActivationLaunchBindingHashInputV1<'a> {
    domain: &'static str,
    launch_reference: &'a str,
    assignment_id: &'a str,
    runner_id: &'a str,
    room_id: &'a str,
    member_id: &'a str,
    pack: &'a PackReference,
    authority_reference: &'a SecretReferenceV1,
}

/// Owner-only durable registry issuing opaque assignment MCP launch references.
pub struct AssignmentMcpLaunchRegistryV1<S> {
    root: PathBuf,
    continuity_root: PathBuf,
    source: Arc<S>,
    activity_packs: Option<Arc<dyn DaemonActivityPackSource>>,
    mutation: Arc<Mutex<()>>,
    daemon: SocketAddr,
    timeout: Duration,
}

impl<S> Clone for AssignmentMcpLaunchRegistryV1<S> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            continuity_root: self.continuity_root.clone(),
            source: self.source.clone(),
            activity_packs: self.activity_packs.clone(),
            mutation: self.mutation.clone(),
            daemon: self.daemon,
            timeout: self.timeout,
        }
    }
}

trait AssignmentMcpLaunchControlV1: Send + Sync {
    fn issue_launch(&self, assignment_id: &str) -> Result<String, AssignedMembershipSourceErrorV1>;
    fn revoke_launch(&self, launch_reference: &str) -> Result<(), AssignedMembershipSourceErrorV1>;
}

impl<S> AssignmentMcpLaunchControlV1 for AssignmentMcpLaunchRegistryV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    fn issue_launch(&self, assignment_id: &str) -> Result<String, AssignedMembershipSourceErrorV1> {
        self.issue(assignment_id)
    }

    fn revoke_launch(&self, launch_reference: &str) -> Result<(), AssignedMembershipSourceErrorV1> {
        self.revoke(launch_reference)
    }
}

#[derive(Serialize)]
struct AssignmentMcpLaunchResponseV1 {
    schema: &'static str,
    launch_reference: String,
    helper: &'static str,
}

/// Adds bounded Supervisor issue/revoke routes for opaque MCP launch references.
pub fn assignment_mcp_launch_router<S>(registry: AssignmentMcpLaunchRegistryV1<S>) -> Router
where
    S: AssignedMembershipLaunchSourceV1,
{
    let state: Arc<dyn AssignmentMcpLaunchControlV1> = Arc::new(registry);
    Router::new()
        .route(
            "/v1/agent-profile-assignments/{assignment_id}/mcp-launches",
            post(issue_assignment_mcp_launch),
        )
        .route(
            "/v1/assignment-mcp-launches/{launch_reference}",
            delete(revoke_assignment_mcp_launch),
        )
        .with_state(state)
}

async fn issue_assignment_mcp_launch(
    State(registry): State<Arc<dyn AssignmentMcpLaunchControlV1>>,
    AxumPath(assignment_id): AxumPath<String>,
) -> Result<(StatusCode, Json<AssignmentMcpLaunchResponseV1>), StatusCode> {
    let launch_reference =
        tokio::task::spawn_blocking(move || registry.issue_launch(&assignment_id))
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .map_err(map_launch_status)?;
    Ok((
        StatusCode::CREATED,
        Json(AssignmentMcpLaunchResponseV1 {
            schema: "worldstream/assignment-mcp-launch-response/v1",
            launch_reference,
            helper: "worldstream-assignment-mcp",
        }),
    ))
}

async fn revoke_assignment_mcp_launch(
    State(registry): State<Arc<dyn AssignmentMcpLaunchControlV1>>,
    AxumPath(launch_reference): AxumPath<String>,
) -> Result<StatusCode, StatusCode> {
    tokio::task::spawn_blocking(move || registry.revoke_launch(&launch_reference))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map_err(map_launch_status)?;
    Ok(StatusCode::NO_CONTENT)
}

const fn map_launch_status(error: AssignedMembershipSourceErrorV1) -> StatusCode {
    match error {
        AssignedMembershipSourceErrorV1::NotFound => StatusCode::NOT_FOUND,
        AssignedMembershipSourceErrorV1::Invalid => StatusCode::BAD_REQUEST,
        AssignedMembershipSourceErrorV1::Revoked => StatusCode::GONE,
        AssignedMembershipSourceErrorV1::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

impl<S> AssignmentMcpLaunchRegistryV1<S>
where
    S: AssignedMembershipLaunchSourceV1,
{
    /// Opens the Supervisor-owned launch registry for one fixed local daemon.
    ///
    /// # Errors
    ///
    /// Rejects unsafe storage or a non-loopback daemon configuration.
    pub fn open(
        root: &Path,
        continuity_root: &Path,
        source: S,
        daemon: SocketAddr,
        timeout: Duration,
    ) -> Result<Self, AssignedMembershipSourceErrorV1> {
        if !daemon.ip().is_loopback() || timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        Ok(Self {
            root: prepare_data_directory(root)
                .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
            continuity_root: prepare_data_directory(continuity_root)
                .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
            source: Arc::new(source),
            activity_packs: None,
            mutation: Arc::new(Mutex::new(())),
            daemon,
            timeout,
        })
    }

    /// Adds the Host-authorized exact Pack catalog used only while issuing a launch.
    #[must_use]
    pub fn with_activity_packs(mut self, activity_packs: impl DaemonActivityPackSource) -> Self {
        self.activity_packs = Some(Arc::new(activity_packs));
        self
    }

    /// Issues one immutable opaque launch reference for an exact assignment.
    ///
    /// # Errors
    ///
    /// Fails closed if assignment resolution or durable publication fails.
    #[allow(clippy::too_many_lines)]
    pub fn issue(&self, assignment_id: &str) -> Result<String, AssignedMembershipSourceErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = self.active_launch(assignment_id)? {
            return Ok(existing);
        }
        let binding = self.source.resolve_launch_binding(assignment_id)?;
        if binding.assignment_id != assignment_id {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let action_schemas = self.activity_packs.as_ref().map_or_else(
            || Ok(Vec::new()),
            |source| {
                source
                    .revision(&binding.pack.digest)
                    .map(|response| response.revision.actions)
                    .map_err(map_activity_pack_source_error)
            },
        )?;
        let launch_reference = random_reference()?;
        let mut record = AssignmentMcpLaunchRecordV1 {
            schema: "worldstream/assignment-mcp-launch/v1".to_owned(),
            launch_reference: launch_reference.clone(),
            assignment_id: binding.assignment_id,
            profile_id: binding.profile_id,
            profile_revision: binding.profile_revision,
            role: binding.role,
            principal_id: binding.principal_id,
            room_id: binding.room_id,
            member_id: binding.member_id,
            pack: binding.pack.clone(),
            action_schemas,
            authority_reference: binding.authority_reference,
            daemon: self.daemon,
            timeout_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
            activation_binding_hash: String::new(),
            binding_hash: String::new(),
        };
        let mut activation = AssignmentMcpActivationLaunchRecordV1 {
            schema: "worldstream/assignment-mcp-activation-launch/v1".to_owned(),
            launch_reference: launch_reference.clone(),
            assignment_id: record.assignment_id.clone(),
            runner_id: binding.runner_id,
            room_id: record.room_id.clone(),
            member_id: record.member_id.clone(),
            pack: binding.pack,
            authority_reference: binding.runner_authority_reference,
            binding_hash: String::new(),
        };
        activation.binding_hash = activation_launch_binding_hash(&activation)?;
        record
            .activation_binding_hash
            .clone_from(&activation.binding_hash);
        record.binding_hash = launch_binding_hash(&record)?;
        validate_launch_record(&record, &launch_reference)?;
        validate_activation_launch_record(&activation, &launch_reference)?;
        let path = self.root.join(format!("{launch_reference}.json"));
        let activation_path = self
            .root
            .join(format!("{launch_reference}.activation.json"));
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        let activation_bytes = serde_json::to_vec_pretty(&activation)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CONTINUITY_BYTES
            || u64::try_from(activation_bytes.len()).unwrap_or(u64::MAX) > MAX_CONTINUITY_BYTES
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let mut file = worldstream_runtime::create_owner_only_file(&path)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        if file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(path);
            return Err(AssignedMembershipSourceErrorV1::Unavailable);
        }
        let Ok(mut activation_file) = worldstream_runtime::create_owner_only_file(&activation_path)
        else {
            let _ = fs::remove_file(path);
            return Err(AssignedMembershipSourceErrorV1::Unavailable);
        };
        if activation_file
            .write_all(&activation_bytes)
            .and_then(|()| activation_file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(activation_path);
            return Err(AssignedMembershipSourceErrorV1::Unavailable);
        }
        sync_continuity_directory(&self.root)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        if let Err(error) = self.publish_active_launch(assignment_id, &launch_reference) {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(activation_path);
            let _ = sync_continuity_directory(&self.root);
            return Err(error);
        }
        Ok(launch_reference)
    }

    /// Terminally revokes one issued launch reference without mutating it.
    ///
    /// # Errors
    ///
    /// Rejects malformed references or unavailable durable storage.
    pub fn revoke(&self, launch_reference: &str) -> Result<(), AssignedMembershipSourceErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        validate_reference(launch_reference)?;
        let path = self.root.join(format!("{launch_reference}.revoked"));
        match worldstream_runtime::create_owner_only_file(&path) {
            Ok(file) => file
                .sync_all()
                .and_then(|()| sync_continuity_directory(&self.root))
                .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable),
            Err(_) if path.exists() => Ok(()),
            Err(_) => Err(AssignedMembershipSourceErrorV1::Unavailable),
        }
    }

    #[must_use]
    pub fn continuity_root(&self) -> &Path {
        &self.continuity_root
    }

    /// Lists exact active assignment identities without exposing launch references.
    ///
    /// Every retained active record and its participant/Activation sidecars are
    /// revalidated through the same private launch seam used at helper start.
    /// Revoked records are excluded.
    ///
    /// # Errors
    ///
    /// Fails closed for malformed, duplicate, incoherent, or excessive active
    /// records and unavailable owner-only storage.
    pub fn active_assignment_ids(&self) -> Result<Vec<String>, AssignedMembershipSourceErrorV1> {
        let mut assignments = Vec::new();
        let mut seen = BTreeSet::new();
        for entry in
            fs::read_dir(&self.root).map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?
        {
            let path = entry
                .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?
                .path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or(AssignedMembershipSourceErrorV1::Invalid)?;
            let Some(assignment_id) = name.strip_suffix(".active.json") else {
                if name.contains(".active") {
                    return Err(AssignedMembershipSourceErrorV1::Invalid);
                }
                continue;
            };
            if assignment_id.parse::<UlidString>().is_err()
                || !seen.insert(assignment_id.to_owned())
                || assignments.len() >= MAX_ACTIVE_ASSIGNMENTS
            {
                return Err(AssignedMembershipSourceErrorV1::Invalid);
            }
            if self.active_launch(assignment_id)?.is_some() {
                assignments.push(assignment_id.to_owned());
            }
        }
        assignments.sort();
        Ok(assignments)
    }

    /// Lists the exact safe active-assignment join fields without exposing
    /// launch references, authority, principal, or role material.
    ///
    /// # Errors
    ///
    /// Fails closed when an active pointer or either retained sidecar is
    /// malformed, incoherent, revoked, duplicated, or excessive.
    pub(crate) fn active_assignment_scopes(
        &self,
    ) -> Result<Vec<ActiveAssignmentScopeV1>, AssignedMembershipSourceErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let assignment_ids = self.active_assignment_ids()?;
        let mut scopes = Vec::with_capacity(assignment_ids.len());
        for assignment_id in assignment_ids {
            let launch_reference = self
                .active_launch(&assignment_id)?
                .ok_or(AssignedMembershipSourceErrorV1::Invalid)?;
            let participant = read_launch_record(&self.root, &launch_reference)?;
            let activation = read_activation_launch_record(&self.root, &launch_reference)?;
            if participant.assignment_id != assignment_id
                || activation.assignment_id != assignment_id
                || participant.room_id != activation.room_id
                || participant.member_id != activation.member_id
                || participant.pack != activation.pack
                || participant.activation_binding_hash != activation.binding_hash
            {
                return Err(AssignedMembershipSourceErrorV1::Invalid);
            }
            scopes.push(ActiveAssignmentScopeV1 {
                assignment_id,
                room_id: participant.room_id,
                member_id: participant.member_id,
                runner_id: activation.runner_id,
                pack: participant.pack,
            });
        }
        scopes.sort_by(|left, right| left.assignment_id.cmp(&right.assignment_id));
        if scopes
            .windows(2)
            .any(|window| window[0].assignment_id == window[1].assignment_id)
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        Ok(scopes)
    }

    fn active_launch(
        &self,
        assignment_id: &str,
    ) -> Result<Option<String>, AssignedMembershipSourceErrorV1> {
        if assignment_id.parse::<UlidString>().is_err() {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let path = self.root.join(format!("{assignment_id}.active.json"));
        if !path.exists() {
            return Ok(None);
        }
        validate_owner_only_file(&path)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        if fs::metadata(&path)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?
            .len()
            > MAX_CONTINUITY_BYTES
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let active: AssignmentMcpActiveLaunchV1 = serde_json::from_slice(
            &fs::read(path).map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
        if active.schema != "worldstream/assignment-mcp-active-launch/v1"
            || active.assignment_id != assignment_id
            || validate_reference(&active.launch_reference).is_err()
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        if self
            .root
            .join(format!("{}.revoked", active.launch_reference))
            .exists()
        {
            return Ok(None);
        }
        let participant = read_launch_record(&self.root, &active.launch_reference)?;
        let activation = read_activation_launch_record(&self.root, &active.launch_reference)?;
        if participant.assignment_id != assignment_id
            || activation.assignment_id != assignment_id
            || participant.activation_binding_hash != activation.binding_hash
        {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        Ok(Some(active.launch_reference))
    }

    fn publish_active_launch(
        &self,
        assignment_id: &str,
        launch_reference: &str,
    ) -> Result<(), AssignedMembershipSourceErrorV1> {
        let record = AssignmentMcpActiveLaunchV1 {
            schema: "worldstream/assignment-mcp-active-launch/v1".to_owned(),
            assignment_id: assignment_id.to_owned(),
            launch_reference: launch_reference.to_owned(),
        };
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        let target = self.root.join(format!("{assignment_id}.active.json"));
        let temporary = self
            .root
            .join(format!(".{assignment_id}.{launch_reference}.active.tmp"));
        let mut file = worldstream_runtime::create_owner_only_renameable_file(&temporary)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
        let published = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_continuity_file(&temporary, &target))
            .and_then(|()| sync_continuity_directory(&self.root))
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable);
        if published.is_err() {
            let _ = fs::remove_file(temporary);
        }
        published
    }
}

trait AssignmentLeaseV1: Send + Sync {
    fn validate(&self) -> Result<(), AssignedMembershipSourceErrorV1>;
}

struct StaticAssignmentLeaseV1;

impl AssignmentLeaseV1 for StaticAssignmentLeaseV1 {
    fn validate(&self) -> Result<(), AssignedMembershipSourceErrorV1> {
        Ok(())
    }
}

struct FileAssignmentLeaseV1 {
    root: PathBuf,
    launch_reference: String,
    binding_hash: String,
    _lock: fs::File,
}

impl AssignmentLeaseV1 for FileAssignmentLeaseV1 {
    fn validate(&self) -> Result<(), AssignedMembershipSourceErrorV1> {
        if self
            .root
            .join(format!("{}.revoked", self.launch_reference))
            .exists()
        {
            return Err(AssignedMembershipSourceErrorV1::Revoked);
        }
        let record = read_launch_record(&self.root, &self.launch_reference)?;
        if record.binding_hash != self.binding_hash {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        Ok(())
    }
}

/// Opens one Supervisor-issued launch registration as a sealed MCP server.
///
/// # Errors
///
/// Rejects unknown, concurrently active, revoked, corrupt, or unsafe registrations.
#[allow(clippy::too_many_lines)]
pub fn open_registered_assignment_mcp(
    state_dir: &Path,
    launch_reference: &str,
) -> Result<AssignmentMcpServerV1, AssignedMembershipSourceErrorV1> {
    validate_reference(launch_reference)?;
    let state_dir = prepare_data_directory(state_dir)
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let root = prepare_data_directory(&state_dir.join("assignment-mcp-launches"))
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let record = read_launch_record(&root, launch_reference)?;
    if read_active_launch_reference(&root, &record.assignment_id)? != launch_reference {
        return Err(AssignedMembershipSourceErrorV1::Revoked);
    }
    let activation_record = read_activation_launch_record(&root, launch_reference)?;
    if activation_record.assignment_id != record.assignment_id
        || activation_record.room_id != record.room_id
        || activation_record.member_id != record.member_id
        || activation_record.pack != record.pack
        || activation_record.binding_hash != record.activation_binding_hash
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    if root.join(format!("{launch_reference}.revoked")).exists() {
        return Err(AssignedMembershipSourceErrorV1::Revoked);
    }
    let lock_path = root.join(format!("{launch_reference}.lock"));
    let lock = match worldstream_runtime::create_owner_only_file(&lock_path) {
        Ok(file) => file,
        Err(_) if lock_path.exists() => fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
        Err(_) => return Err(AssignedMembershipSourceErrorV1::Unavailable),
    };
    validate_owner_only_file(&lock_path)
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    lock.try_lock()
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let vault = FileSecretVaultV1::open(&state_dir.join("secrets"))
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let secret = vault
        .resolve(
            SecretKindV1::MembershipAuthority,
            &record.authority_reference,
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
    let mut bytes: [u8; 32] = secret
        .as_bytes()
        .try_into()
        .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
    let membership_bearer = BearerWireV1::from_bytes(bytes);
    bytes.zeroize();
    let authority = AssignedMembershipAuthorityV1::new(
        &record.assignment_id,
        &record.profile_id,
        &record.profile_revision,
        &record.role,
        &record.principal_id,
        &record.room_id,
        &record.member_id,
        SealedCapabilityBearerV1::from_wire(&membership_bearer),
    )?;
    let runner_secret = vault
        .resolve(
            SecretKindV1::RunnerAuthority,
            &activation_record.authority_reference,
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
    let mut runner_bytes: [u8; 32] = runner_secret
        .as_bytes()
        .try_into()
        .map_err(|_| AssignedMembershipSourceErrorV1::Revoked)?;
    let runner_bearer = BearerWireV1::from_bytes(runner_bytes);
    runner_bytes.zeroize();
    let runner_authority = AssignedRunnerActivationAuthorityV1::new(
        &record.assignment_id,
        &record.principal_id,
        &activation_record.runner_id,
        &record.room_id,
        &record.member_id,
        record.pack.clone(),
        SealedCapabilityBearerV1::from_wire(&runner_bearer),
    )
    .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
    let gateway = FixedDaemonAssignedMembershipGatewayV1::open(
        record.daemon,
        Duration::from_millis(record.timeout_ms),
        &state_dir
            .join("assignment-mcp-progress")
            .join(launch_reference),
    )
    .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let actions = AssignmentMcpActionToolsV1 {
        schemas: PinnedAssignmentMcpActionSchemasV1 {
            pack: record.pack.clone(),
            actions: record.action_schemas.clone(),
        },
        gateway: FixedDaemonAssignmentMcpActionGatewayV1::new(
            record.daemon,
            Duration::from_millis(record.timeout_ms),
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
        operations: FileAssignmentMcpOperationLedgerV1::open(
            state_dir
                .join("assignment-mcp-operations")
                .join(launch_reference),
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
    };
    let activation_gateway = FixedDaemonRunnerActivationGatewayV1::new(
        record.daemon,
        Duration::from_millis(record.timeout_ms),
    )
    .map_err(map_runner_activation_open_error)?;
    activation_gateway
        .establish(&runner_authority)
        .map_err(map_runner_activation_open_error)?;
    let activations = AssignmentActivationToolsV1::new(
        runner_authority,
        activation_gateway,
        FileActivationOperationLedgerV1::open(
            state_dir
                .join("assignment-mcp-activations")
                .join(launch_reference),
        )
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
        SystemActivationLeaseClockV1,
    );
    let lease: Arc<dyn AssignmentLeaseV1> = Arc::new(FileAssignmentLeaseV1 {
        root,
        launch_reference: launch_reference.to_owned(),
        binding_hash: record.binding_hash,
        _lock: lock,
    });
    Ok(AssignmentMcpServerV1::open(AssignmentMcpContextV1 {
        authority,
        gateway: Arc::new(gateway),
        lease,
        actions: Some(actions),
        activations: Some(activations),
    }))
}

const fn map_runner_activation_open_error(
    error: RunnerActivationGatewayErrorV1,
) -> AssignedMembershipSourceErrorV1 {
    match error {
        RunnerActivationGatewayErrorV1::Revoked => AssignedMembershipSourceErrorV1::Revoked,
        RunnerActivationGatewayErrorV1::Disconnected
        | RunnerActivationGatewayErrorV1::Unavailable => {
            AssignedMembershipSourceErrorV1::Unavailable
        }
        RunnerActivationGatewayErrorV1::NotAvailable
        | RunnerActivationGatewayErrorV1::Expired
        | RunnerActivationGatewayErrorV1::StaleLease
        | RunnerActivationGatewayErrorV1::AlreadyCompleted
        | RunnerActivationGatewayErrorV1::IdempotencyConflict
        | RunnerActivationGatewayErrorV1::InvalidData => AssignedMembershipSourceErrorV1::Invalid,
    }
}

fn random_reference() -> Result<String, AssignedMembershipSourceErrorV1> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        }))
}

fn validate_reference(reference: &str) -> Result<(), AssignedMembershipSourceErrorV1> {
    if reference.len() == 64
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(AssignedMembershipSourceErrorV1::Invalid)
    }
}

fn launch_binding_hash(
    record: &AssignmentMcpLaunchRecordV1,
) -> Result<String, AssignedMembershipSourceErrorV1> {
    let encoded = serde_json::to_vec(&LaunchBindingHashInputV1 {
        domain: "worldstream/assignment-mcp-launch-binding/v1",
        launch_reference: &record.launch_reference,
        assignment_id: &record.assignment_id,
        profile_id: &record.profile_id,
        profile_revision: &record.profile_revision,
        role: &record.role,
        principal_id: &record.principal_id,
        room_id: &record.room_id,
        member_id: &record.member_id,
        pack: &record.pack,
        action_schemas: &record.action_schemas,
        authority_reference: &record.authority_reference,
        daemon: record.daemon,
        timeout_ms: record.timeout_ms,
        activation_binding_hash: &record.activation_binding_hash,
    })
    .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let canonical = worldstream_core::CanonicalJsonV1::parse(&encoded)
        .and_then(|value| value.to_bytes())
        .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
    Ok(format!("blake3:{}", blake3::hash(&canonical).to_hex()))
}

fn activation_launch_binding_hash(
    record: &AssignmentMcpActivationLaunchRecordV1,
) -> Result<String, AssignedMembershipSourceErrorV1> {
    let encoded = serde_json::to_vec(&ActivationLaunchBindingHashInputV1 {
        domain: "worldstream/assignment-mcp-activation-launch-binding/v1",
        launch_reference: &record.launch_reference,
        assignment_id: &record.assignment_id,
        runner_id: &record.runner_id,
        room_id: &record.room_id,
        member_id: &record.member_id,
        pack: &record.pack,
        authority_reference: &record.authority_reference,
    })
    .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?;
    let canonical = worldstream_core::CanonicalJsonV1::parse(&encoded)
        .and_then(|value| value.to_bytes())
        .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
    Ok(format!("blake3:{}", blake3::hash(&canonical).to_hex()))
}

fn validate_launch_record(
    record: &AssignmentMcpLaunchRecordV1,
    launch_reference: &str,
) -> Result<(), AssignedMembershipSourceErrorV1> {
    validate_reference(launch_reference)?;
    if record.schema != "worldstream/assignment-mcp-launch/v1"
        || record.launch_reference != launch_reference
        || record.assignment_id.parse::<UlidString>().is_err()
        || record.principal_id.parse::<UlidString>().is_err()
        || record.room_id.parse::<UlidString>().is_err()
        || record.member_id.parse::<UlidString>().is_err()
        || record
            .pack
            .digest
            .parse::<worldstream_core::Blake3DigestV1>()
            .is_err()
        || !bounded_label(&record.pack.id)
        || !bounded_label(&record.pack.version)
        || record.action_schemas.len() > 256
        || !bounded_label(&record.profile_id)
        || !bounded_label(&record.profile_revision)
        || !bounded_label(&record.role)
        || !record.daemon.ip().is_loopback()
        || record.timeout_ms == 0
        || record.timeout_ms > 30_000
        || record.binding_hash != launch_binding_hash(record)?
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    Ok(())
}

fn validate_activation_launch_record(
    record: &AssignmentMcpActivationLaunchRecordV1,
    launch_reference: &str,
) -> Result<(), AssignedMembershipSourceErrorV1> {
    validate_reference(launch_reference)?;
    if record.schema != "worldstream/assignment-mcp-activation-launch/v1"
        || record.launch_reference != launch_reference
        || record.assignment_id.parse::<UlidString>().is_err()
        || record.runner_id.parse::<UlidString>().is_err()
        || record.room_id.parse::<UlidString>().is_err()
        || record.member_id.parse::<UlidString>().is_err()
        || record
            .pack
            .digest
            .parse::<worldstream_core::Blake3DigestV1>()
            .is_err()
        || !bounded_label(&record.pack.id)
        || !bounded_label(&record.pack.version)
        || record.binding_hash != activation_launch_binding_hash(record)?
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    Ok(())
}

const fn map_activity_pack_source_error(
    error: ActivityPackProxyErrorV1,
) -> AssignedMembershipSourceErrorV1 {
    match error {
        ActivityPackProxyErrorV1::InvalidRevision | ActivityPackProxyErrorV1::InvalidResponse => {
            AssignedMembershipSourceErrorV1::Invalid
        }
        ActivityPackProxyErrorV1::AuthorityUnavailable
        | ActivityPackProxyErrorV1::DaemonUnavailable
        | ActivityPackProxyErrorV1::RevisionUnavailable => {
            AssignedMembershipSourceErrorV1::Unavailable
        }
    }
}

fn read_launch_record(
    root: &Path,
    launch_reference: &str,
) -> Result<AssignmentMcpLaunchRecordV1, AssignedMembershipSourceErrorV1> {
    validate_reference(launch_reference)?;
    let path = root.join(format!("{launch_reference}.json"));
    validate_owner_only_file(&path).map_err(|_| AssignedMembershipSourceErrorV1::NotFound)?;
    if fs::metadata(&path)
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?
        .len()
        > MAX_CONTINUITY_BYTES
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    let record: AssignmentMcpLaunchRecordV1 = serde_json::from_slice(
        &fs::read(path).map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
    )
    .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
    validate_launch_record(&record, launch_reference)?;
    Ok(record)
}

fn read_activation_launch_record(
    root: &Path,
    launch_reference: &str,
) -> Result<AssignmentMcpActivationLaunchRecordV1, AssignedMembershipSourceErrorV1> {
    validate_reference(launch_reference)?;
    let path = root.join(format!("{launch_reference}.activation.json"));
    validate_owner_only_file(&path).map_err(|_| AssignedMembershipSourceErrorV1::NotFound)?;
    if fs::metadata(&path)
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?
        .len()
        > MAX_CONTINUITY_BYTES
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    let record: AssignmentMcpActivationLaunchRecordV1 = serde_json::from_slice(
        &fs::read(path).map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
    )
    .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
    validate_activation_launch_record(&record, launch_reference)?;
    Ok(record)
}

fn read_active_launch_reference(
    root: &Path,
    assignment_id: &str,
) -> Result<String, AssignedMembershipSourceErrorV1> {
    if assignment_id.parse::<UlidString>().is_err() {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    let path = root.join(format!("{assignment_id}.active.json"));
    validate_owner_only_file(&path).map_err(|_| AssignedMembershipSourceErrorV1::NotFound)?;
    if fs::metadata(&path)
        .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?
        .len()
        > MAX_CONTINUITY_BYTES
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    let active: AssignmentMcpActiveLaunchV1 = serde_json::from_slice(
        &fs::read(path).map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
    )
    .map_err(|_| AssignedMembershipSourceErrorV1::Invalid)?;
    if active.schema != "worldstream/assignment-mcp-active-launch/v1"
        || active.assignment_id != assignment_id
        || validate_reference(&active.launch_reference).is_err()
    {
        return Err(AssignedMembershipSourceErrorV1::Invalid);
    }
    Ok(active.launch_reference)
}

/// Browser/tool-safe Projection Reset material retained below MCP handlers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionResetV1 {
    pub baseline_frame_head: u64,
    pub reset_reason: String,
    pub projection_schema: String,
    pub projection: Projection,
    pub projection_hash: String,
}

/// One generic authorized Observation Frame without routing identity.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationDeliveryV1 {
    pub frame_seq: u64,
    pub cause_room_seq: u64,
    pub frame_kind: String,
    pub observation_schema: String,
    pub observation: Value,
    pub frame_payload_hash: String,
}

/// Current materialized assignment-bound stream state reusable by later MCP tools.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipStreamSnapshotV1 {
    pub room_head: RoomHead,
    pub pack: PackReference,
    pub cursor: Option<u64>,
    pub frame_head: u64,
    pub retained_floor: u64,
    /// Exact current Action Offers materialized across reset and retained-frame reconnects.
    pub current_action_offers: Vec<ActionOffer>,
    pub projection_reset: Option<ProjectionResetV1>,
    pub observations: Vec<ObservationDeliveryV1>,
}

impl MembershipStreamSnapshotV1 {
    /// Returns the exact currently materialized Action Offers for IMO-75.
    #[must_use]
    pub fn action_offers(&self) -> &[ActionOffer] {
        &self.current_action_offers
    }
}

/// Closed transport and authority failures from the assignment-bound client.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AssignedMembershipGatewayErrorV1 {
    #[error("daemon disconnected")]
    Disconnected,
    #[error("assignment authority was rejected")]
    Revoked,
    #[error("Cursor requires a Projection Reset")]
    StaleCursor,
    #[error("daemon is unavailable")]
    Unavailable,
    #[error("daemon returned invalid assignment data")]
    InvalidData,
}

/// Reusable assignment-bound transport below generic MCP handlers.
pub trait AssignedMembershipGatewayV1: Send + Sync + 'static {
    /// Attaches, installs the exact synchronization barrier, and returns only
    /// the authorized Projection/Observation material for this Membership.
    ///
    /// # Errors
    ///
    /// Returns closed reconnect, revocation, Cursor, or daemon failures.
    fn synchronize(
        &self,
        authority: &AssignedMembershipAuthorityV1,
    ) -> Result<MembershipStreamSnapshotV1, AssignedMembershipGatewayErrorV1>;

    /// Durably advances only the sealed Membership Cursor.
    ///
    /// # Errors
    ///
    /// Returns closed reconnect, revocation, Cursor, or daemon failures.
    fn acknowledge(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        through_frame_seq: u64,
    ) -> Result<u64, AssignedMembershipGatewayErrorV1>;
}

/// Fixed-daemon production transport for one sealed assignment context.
#[derive(Clone)]
pub struct FixedDaemonAssignedMembershipGatewayV1 {
    address: SocketAddr,
    timeout: Duration,
    continuity: Arc<Mutex<HashMap<String, StreamContinuityV1>>>,
    continuity_root: Option<Arc<PathBuf>>,
}

impl fmt::Debug for FixedDaemonAssignedMembershipGatewayV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FixedDaemonAssignedMembershipGatewayV1(REDACTED)")
    }
}

struct ReadBudgetV1 {
    deadline: Instant,
    bytes: usize,
    messages: usize,
}

struct IncomingEnvelopeV1 {
    message_type: String,
    body: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncAckedV1 {
    through_frame_head: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationAckedV1 {
    room_id: String,
    member_id: String,
    cursor: u64,
}

struct AckSessionResultV1 {
    cursor: u64,
    current_action_offers: Vec<ActionOffer>,
}

impl ReadBudgetV1 {
    fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            bytes: 0,
            messages: 0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StreamContinuityV1 {
    cursor: Option<u64>,
    current_action_offers: Vec<ActionOffer>,
    initialized: bool,
    pending_ack: Option<(Option<u64>, u64)>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StreamContinuityRecordV1 {
    schema: String,
    assignment_id: String,
    continuity: StreamContinuityV1,
}

impl FixedDaemonAssignedMembershipGatewayV1 {
    #[must_use]
    pub fn new(address: SocketAddr, timeout: Duration) -> Self {
        Self {
            address,
            timeout,
            continuity: Arc::new(Mutex::new(HashMap::new())),
            continuity_root: None,
        }
    }

    /// Opens a production transport with owner-only durable Cursor/offer continuity.
    ///
    /// # Errors
    ///
    /// Rejects an unavailable or unsafe continuity directory.
    pub fn open(
        address: SocketAddr,
        timeout: Duration,
        continuity_root: &Path,
    ) -> Result<Self, AssignedMembershipGatewayErrorV1> {
        let continuity_root = prepare_data_directory(continuity_root)
            .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?;
        Ok(Self {
            address,
            timeout,
            continuity: Arc::new(Mutex::new(HashMap::new())),
            continuity_root: Some(Arc::new(continuity_root)),
        })
    }

    fn continuity_for(
        &self,
        assignment_id: &str,
    ) -> Result<StreamContinuityV1, AssignedMembershipGatewayErrorV1> {
        if let Some(state) = self
            .continuity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(assignment_id)
            .cloned()
        {
            return Ok(state);
        }
        let Some(root) = &self.continuity_root else {
            return Ok(StreamContinuityV1::default());
        };
        let path = root.join(format!("{assignment_id}.json"));
        if !path.exists() {
            return Ok(StreamContinuityV1::default());
        }
        validate_owner_only_file(&path)
            .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?;
        if fs::metadata(&path)
            .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?
            .len()
            > MAX_CONTINUITY_BYTES
        {
            return Err(AssignedMembershipGatewayErrorV1::InvalidData);
        }
        let record: StreamContinuityRecordV1 = serde_json::from_slice(
            &fs::read(path).map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?,
        )
        .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
        if record.schema != CONTINUITY_SCHEMA_V1 || record.assignment_id != assignment_id {
            return Err(AssignedMembershipGatewayErrorV1::InvalidData);
        }
        self.continuity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(assignment_id.to_owned(), record.continuity.clone());
        Ok(record.continuity)
    }

    fn persist_continuity(
        &self,
        assignment_id: &str,
        continuity: StreamContinuityV1,
    ) -> Result<(), AssignedMembershipGatewayErrorV1> {
        self.continuity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(assignment_id.to_owned(), continuity.clone());
        let Some(root) = &self.continuity_root else {
            return Ok(());
        };
        let record = StreamContinuityRecordV1 {
            schema: CONTINUITY_SCHEMA_V1.to_owned(),
            assignment_id: assignment_id.to_owned(),
            continuity,
        };
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CONTINUITY_BYTES {
            return Err(AssignedMembershipGatewayErrorV1::InvalidData);
        }
        let target = root.join(format!("{assignment_id}.json"));
        let temporary = root.join(format!(".{assignment_id}.{}.tmp", next_protocol_ulid()?));
        let mut file = worldstream_runtime::create_owner_only_renameable_file(&temporary)
            .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?;
        let persisted = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_continuity_file(&temporary, &target))
            .and_then(|()| sync_continuity_directory(root.as_ref()))
            .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable);
        if persisted.is_err() {
            let _ = fs::remove_file(temporary);
        }
        persisted
    }

    fn connect(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        budget: &mut ReadBudgetV1,
    ) -> Result<WebSocket<TcpStream>, AssignedMembershipGatewayErrorV1> {
        let stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| AssignedMembershipGatewayErrorV1::Disconnected)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| AssignedMembershipGatewayErrorV1::Disconnected)?;
        let mut authorization = Zeroizing::new(Vec::with_capacity(
            "Bearer ".len() + authority.bearer().as_str().len(),
        ));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(authority.bearer().as_str().as_bytes());
        let authorization = HeaderValue::from_maybe_shared(Bytes::from_owner(authorization))
            .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("ws://{}/v1/stream", self.address))
            .header("Host", self.address.to_string())
            .header("Authorization", authorization)
            .header("Sec-WebSocket-Protocol", WEBSOCKET_SUBPROTOCOL)
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key())
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
        let (mut socket, response) =
            client(request, stream).map_err(|_| AssignedMembershipGatewayErrorV1::Revoked)?;
        if response.status() != http::StatusCode::SWITCHING_PROTOCOLS {
            return Err(AssignedMembershipGatewayErrorV1::Revoked);
        }
        Self::send(
            &mut socket,
            "client.hello",
            &ClientHello {
                client_name: "worldstream-assignment-mcp".to_owned(),
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
                mode: ClientMode::Participant,
                supported_protocols: vec![PROTOCOL_VERSION.to_owned()],
                capabilities: REQUIRED_CLIENT_CAPABILITIES
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
        )?;
        let welcome: ServerWelcome = Self::read_type(&mut socket, budget, "server.welcome")?;
        if welcome.selected_protocol != PROTOCOL_VERSION
            || welcome.maximum_message_bytes != MAX_MESSAGE_BYTES
            || welcome.authenticated_principal.kind != PrincipalKind::Agent
            || welcome.authenticated_principal.principal_id != authority.principal_id()
        {
            return Err(AssignedMembershipGatewayErrorV1::Revoked);
        }
        Ok(socket)
    }

    fn attach_and_sync(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        after_cursor: Option<u64>,
        retained_offers: &[ActionOffer],
        continuity_initialized: bool,
    ) -> Result<MembershipStreamSnapshotV1, AssignedMembershipGatewayErrorV1> {
        let mut budget = ReadBudgetV1::new(self.timeout);
        let mut socket = self.connect(authority, &mut budget)?;
        Self::send(
            &mut socket,
            "room.attach",
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "after_frame_seq": after_cursor,
            }),
        )?;
        let attached: RoomAttached = Self::read_type(&mut socket, &mut budget, "room.attached")?;
        validate_attached(&attached, authority)?;
        let (projection_reset, mut observations, mut current_action_offers) = match &attached.sync {
            SyncBranch::ProjectionReset {
                baseline_frame_head,
                reason,
            } => {
                let reset: ProjectionReset =
                    Self::read_type(&mut socket, &mut budget, "projection.reset")?;
                validate_reset(&reset, &attached, authority, *baseline_frame_head, reason)?;
                let current_action_offers = reset.projection.action_offers.clone();
                (
                    Some(ProjectionResetV1 {
                        baseline_frame_head: reset.baseline_frame_head,
                        reset_reason: reset.reset_reason,
                        projection_schema: reset.projection_schema,
                        projection: reset.projection,
                        projection_hash: reset.projection_hash,
                    }),
                    Vec::new(),
                    current_action_offers,
                )
            }
            SyncBranch::RetainedFrames {
                cursor_exclusive,
                through_frame_head,
            } => {
                let expected = through_frame_head.saturating_sub(*cursor_exclusive);
                if expected > u64::try_from(MAX_OBSERVATIONS).unwrap_or(u64::MAX) {
                    return Err(AssignedMembershipGatewayErrorV1::InvalidData);
                }
                if !continuity_initialized && attached.frame_head > 0 {
                    return Err(AssignedMembershipGatewayErrorV1::StaleCursor);
                }
                let mut current_action_offers = retained_offers.to_vec();
                let mut observations = Vec::with_capacity(usize::try_from(expected).unwrap_or(0));
                for _ in 0..expected {
                    let delivery: ObservationDeliver =
                        Self::read_type(&mut socket, &mut budget, "observation.deliver")?;
                    validate_delivery(&delivery, authority)?;
                    update_action_offers(&mut current_action_offers, &delivery.observation)?;
                    observations.push(delivery.into());
                }
                (None, observations, current_action_offers)
            }
        };
        Self::send(
            &mut socket,
            "room.sync_ack",
            &serde_json::json!({
                "room_id": authority.room_id(),
                "member_id": authority.member_id(),
                "through_frame_head": attached.frame_head,
                "sync_token": attached.sync_token,
            }),
        )?;
        Self::read_sync_acked(
            &mut socket,
            &mut budget,
            authority,
            attached.frame_head,
            &mut observations,
            &mut current_action_offers,
        )?;
        let synchronized_frame_head = observations
            .last()
            .map_or(attached.frame_head, |delivery| delivery.frame_seq);
        Ok(MembershipStreamSnapshotV1 {
            room_head: attached.room_head,
            pack: attached.pack,
            cursor: attached.cursor,
            frame_head: synchronized_frame_head,
            retained_floor: attached.retained_floor,
            current_action_offers,
            projection_reset,
            observations,
        })
    }

    fn acknowledge_on_synced_session(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        after_cursor: Option<u64>,
        retained_offers: &[ActionOffer],
        through_frame_seq: u64,
    ) -> Result<AckSessionResultV1, AssignedMembershipGatewayErrorV1> {
        let mut budget = ReadBudgetV1::new(self.timeout);
        let mut socket = self.connect(authority, &mut budget)?;
        Self::send(
            &mut socket,
            "room.attach",
            &serde_json::json!({
                "room_id": authority.room_id(), "member_id": authority.member_id(), "after_frame_seq": after_cursor,
            }),
        )?;
        let attached: RoomAttached = Self::read_type(&mut socket, &mut budget, "room.attached")?;
        validate_attached(&attached, authority)?;
        let mut observations = Vec::new();
        let mut current_action_offers = retained_offers.to_vec();
        match &attached.sync {
            SyncBranch::ProjectionReset {
                baseline_frame_head,
                reason,
            } => {
                let reset: ProjectionReset =
                    Self::read_type(&mut socket, &mut budget, "projection.reset")?;
                validate_reset(&reset, &attached, authority, *baseline_frame_head, reason)?;
                current_action_offers = reset.projection.action_offers;
            }
            SyncBranch::RetainedFrames {
                cursor_exclusive,
                through_frame_head,
            } => {
                for _ in 0..through_frame_head.saturating_sub(*cursor_exclusive) {
                    let delivery: ObservationDeliver =
                        Self::read_type(&mut socket, &mut budget, "observation.deliver")?;
                    validate_delivery(&delivery, authority)?;
                    update_action_offers(&mut current_action_offers, &delivery.observation)?;
                    observations.push(delivery.into());
                }
            }
        }
        Self::send(
            &mut socket,
            "room.sync_ack",
            &serde_json::json!({
                "room_id": authority.room_id(), "member_id": authority.member_id(),
                "through_frame_head": attached.frame_head, "sync_token": attached.sync_token,
            }),
        )?;
        Self::read_sync_acked(
            &mut socket,
            &mut budget,
            authority,
            attached.frame_head,
            &mut observations,
            &mut current_action_offers,
        )?;
        Self::send(
            &mut socket,
            "observation.ack",
            &serde_json::json!({
                "room_id": authority.room_id(), "member_id": authority.member_id(),
                "through_frame_seq": through_frame_seq,
            }),
        )?;
        let body: ObservationAckedV1 =
            Self::read_type(&mut socket, &mut budget, "observation.acked")?;
        if body.room_id != authority.room_id() || body.member_id != authority.member_id() {
            return Err(AssignedMembershipGatewayErrorV1::Revoked);
        }
        Ok(AckSessionResultV1 {
            cursor: body.cursor,
            current_action_offers,
        })
    }

    fn send(
        socket: &mut WebSocket<TcpStream>,
        message_type: &str,
        body: &impl Serialize,
    ) -> Result<(), AssignedMembershipGatewayErrorV1> {
        let envelope = VersionedEnvelope {
            protocol: PROTOCOL_VERSION.to_owned(),
            message_type: message_type.to_owned(),
            message_id: next_protocol_ulid()?,
            request_id: None,
            body,
        };
        let mut text = serde_json::to_string(&envelope)
            .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
        let result = socket
            .send(Message::Text(text.clone().into()))
            .map_err(|_| AssignedMembershipGatewayErrorV1::Disconnected);
        text.zeroize();
        result
    }

    fn read_type<T: DeserializeOwned>(
        socket: &mut WebSocket<TcpStream>,
        budget: &mut ReadBudgetV1,
        expected: &str,
    ) -> Result<T, AssignedMembershipGatewayErrorV1> {
        let envelope = read_message(socket, budget)?;
        if envelope.message_type == "error" {
            return Err(classify_daemon_error(envelope.body));
        }
        if envelope.message_type != expected {
            return Err(AssignedMembershipGatewayErrorV1::InvalidData);
        }
        serde_json::from_value(envelope.body)
            .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)
    }

    fn read_sync_acked(
        socket: &mut WebSocket<TcpStream>,
        budget: &mut ReadBudgetV1,
        authority: &AssignedMembershipAuthorityV1,
        through_frame_head: u64,
        observations: &mut Vec<ObservationDeliveryV1>,
        current_action_offers: &mut Vec<ActionOffer>,
    ) -> Result<(), AssignedMembershipGatewayErrorV1> {
        loop {
            let envelope = read_message(socket, budget)?;
            match envelope.message_type.as_str() {
                "error" => return Err(classify_daemon_error(envelope.body)),
                "room.sync_acked" => {
                    let ack: SyncAckedV1 = serde_json::from_value(envelope.body)
                        .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
                    if ack.through_frame_head != through_frame_head {
                        return Err(AssignedMembershipGatewayErrorV1::InvalidData);
                    }
                    return Ok(());
                }
                "observation.deliver" => {
                    if observations.len() >= MAX_OBSERVATIONS {
                        return Err(AssignedMembershipGatewayErrorV1::InvalidData);
                    }
                    let delivery: ObservationDeliver = serde_json::from_value(envelope.body)
                        .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
                    validate_delivery(&delivery, authority)?;
                    let expected = observations
                        .last()
                        .map_or(through_frame_head.saturating_add(1), |prior| {
                            prior.frame_seq.saturating_add(1)
                        });
                    if delivery.frame_seq != expected {
                        return Err(AssignedMembershipGatewayErrorV1::InvalidData);
                    }
                    update_action_offers(current_action_offers, &delivery.observation)?;
                    observations.push(delivery.into());
                }
                _ => return Err(AssignedMembershipGatewayErrorV1::InvalidData),
            }
        }
    }
}

#[cfg(unix)]
fn replace_continuity_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_continuity_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path is not Unicode",
        )
    })?;
    let target = target.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path is not Unicode",
        )
    })?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_continuity_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_continuity_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

impl AssignedMembershipGatewayV1 for FixedDaemonAssignedMembershipGatewayV1 {
    fn synchronize(
        &self,
        authority: &AssignedMembershipAuthorityV1,
    ) -> Result<MembershipStreamSnapshotV1, AssignedMembershipGatewayErrorV1> {
        let retained = self.continuity_for(authority.assignment_id())?;
        let candidates = retained.pending_ack.map_or_else(
            || vec![retained.cursor],
            |(previous, target)| vec![Some(target), previous],
        );
        let mut last_error = AssignedMembershipGatewayErrorV1::StaleCursor;
        for candidate in candidates {
            match self.attach_and_sync(
                authority,
                candidate,
                &retained.current_action_offers,
                retained.initialized,
            ) {
                Ok(snapshot) => {
                    self.persist_continuity(
                        authority.assignment_id(),
                        StreamContinuityV1 {
                            cursor: snapshot.cursor,
                            current_action_offers: snapshot.current_action_offers.clone(),
                            initialized: true,
                            pending_ack: None,
                        },
                    )?;
                    return Ok(snapshot);
                }
                Err(AssignedMembershipGatewayErrorV1::StaleCursor) => {
                    last_error = AssignedMembershipGatewayErrorV1::StaleCursor;
                }
                Err(error) => return Err(error),
            }
        }
        match self.attach_and_sync(authority, None, &[], false) {
            Ok(snapshot) => {
                self.persist_continuity(
                    authority.assignment_id(),
                    StreamContinuityV1 {
                        cursor: snapshot.cursor,
                        current_action_offers: snapshot.current_action_offers.clone(),
                        initialized: true,
                        pending_ack: None,
                    },
                )?;
                Ok(snapshot)
            }
            Err(AssignedMembershipGatewayErrorV1::StaleCursor) => Err(last_error),
            Err(error) => Err(error),
        }
    }

    fn acknowledge(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        through_frame_seq: u64,
    ) -> Result<u64, AssignedMembershipGatewayErrorV1> {
        let mut continuity = self.continuity_for(authority.assignment_id())?;
        let previous = continuity.cursor;
        let retained_offers = continuity.current_action_offers.clone();
        continuity.pending_ack = Some((previous, through_frame_seq));
        self.persist_continuity(authority.assignment_id(), continuity)?;
        match self.acknowledge_on_synced_session(
            authority,
            previous,
            &retained_offers,
            through_frame_seq,
        ) {
            Ok(result) => {
                let mut state = self.continuity_for(authority.assignment_id())?;
                state.cursor = Some(result.cursor);
                state.current_action_offers = result.current_action_offers;
                state.pending_ack = None;
                self.persist_continuity(authority.assignment_id(), state)?;
                Ok(result.cursor)
            }
            Err(error) => Err(error),
        }
    }
}

fn validate_attached(
    attached: &RoomAttached,
    authority: &AssignedMembershipAuthorityV1,
) -> Result<(), AssignedMembershipGatewayErrorV1> {
    if attached.room_id != authority.room_id()
        || attached.member_id != authority.member_id()
        || attached.room_head.room_id != authority.room_id()
        || attached.principal_kind != PrincipalKind::Agent
        || attached.access_mode != worldstream_protocol::AccessMode::Participant
        || attached.role.as_deref() != Some(authority.role())
        || attached.pack.digest != attached.room_head.pack_digest
        || attached.retained_floor > attached.frame_head
        || attached
            .cursor
            .is_some_and(|cursor| cursor > attached.frame_head)
    {
        return Err(AssignedMembershipGatewayErrorV1::Revoked);
    }
    if attached.membership_status != "enabled" {
        return Err(AssignedMembershipGatewayErrorV1::Revoked);
    }
    Ok(())
}

fn validate_reset(
    reset: &ProjectionReset,
    attached: &RoomAttached,
    authority: &AssignedMembershipAuthorityV1,
    baseline_frame_head: u64,
    reason: &str,
) -> Result<(), AssignedMembershipGatewayErrorV1> {
    let reconstructed = serde_json::json!({
        "action_offers": &reset.projection.action_offers,
        "authorized_core": &reset.projection.core,
        "projection": &reset.projection.activity,
        "projection_schema": &reset.projection_schema,
    });
    let encoded = serde_json::to_vec(&reconstructed)
        .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
    let canonical = worldstream_core::CanonicalJsonV1::parse(&encoded)
        .and_then(|value| value.to_bytes())
        .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
    let computed = worldstream_core::projection_hash_for_canonical_bytes(&canonical)
        .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
    if reset.room_id != authority.room_id()
        || reset.member_id != authority.member_id()
        || reset.room_head != attached.room_head
        || reset.room_health != attached.room_health
        || reset.integrity_generation != attached.integrity_generation
        || reset.baseline_frame_head != baseline_frame_head
        || reset.baseline_frame_head != attached.frame_head
        || reset.reset_reason != reason
        || reset.projection_schema.is_empty()
        || reset.projection_hash != computed.to_string()
    {
        return Err(AssignedMembershipGatewayErrorV1::InvalidData);
    }
    Ok(())
}

fn read_message(
    socket: &mut WebSocket<TcpStream>,
    budget: &mut ReadBudgetV1,
) -> Result<IncomingEnvelopeV1, AssignedMembershipGatewayErrorV1> {
    loop {
        if Instant::now() >= budget.deadline || budget.messages >= MAX_SESSION_MESSAGES {
            return Err(AssignedMembershipGatewayErrorV1::Unavailable);
        }
        match socket.read() {
            Ok(Message::Text(text)) if text.len() <= MAX_MESSAGE_BYTES => {
                budget.bytes = budget
                    .bytes
                    .checked_add(text.len())
                    .filter(|bytes| *bytes <= MAX_SESSION_BYTES)
                    .ok_or(AssignedMembershipGatewayErrorV1::InvalidData)?;
                budget.messages += 1;
                let envelope = decode_envelope::<Value>(text.as_bytes())
                    .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
                return Ok(IncomingEnvelopeV1 {
                    message_type: envelope.message_type,
                    body: envelope.body,
                });
            }
            Ok(Message::Text(_) | Message::Binary(_)) => {
                return Err(AssignedMembershipGatewayErrorV1::InvalidData);
            }
            Ok(Message::Ping(value)) => {
                budget.messages += 1;
                socket
                    .send(Message::Pong(value))
                    .map_err(|_| AssignedMembershipGatewayErrorV1::Disconnected)?;
            }
            Ok(Message::Close(_)) | Err(_) => {
                return Err(AssignedMembershipGatewayErrorV1::Disconnected);
            }
            Ok(Message::Pong(_) | Message::Frame(_)) => {
                budget.messages += 1;
            }
        }
    }
}

fn classify_daemon_error(body: Value) -> AssignedMembershipGatewayErrorV1 {
    let Ok(error) = serde_json::from_value::<ProtocolErrorBody>(body) else {
        return AssignedMembershipGatewayErrorV1::InvalidData;
    };
    match error.code {
        ErrorCode::CursorAhead | ErrorCode::CursorOutOfRange => {
            AssignedMembershipGatewayErrorV1::StaleCursor
        }
        ErrorCode::Unauthenticated
        | ErrorCode::Forbidden
        | ErrorCode::MembershipNotFound
        | ErrorCode::MembershipNotEnabled
        | ErrorCode::RoomNotFound => AssignedMembershipGatewayErrorV1::Revoked,
        ErrorCode::StorageUnavailable
        | ErrorCode::StorageNotInitialized
        | ErrorCode::RoomBusy
        | ErrorCode::RateLimited => AssignedMembershipGatewayErrorV1::Unavailable,
        ErrorCode::SlowConsumer => AssignedMembershipGatewayErrorV1::Disconnected,
        ErrorCode::ConfigInvalid
        | ErrorCode::Internal
        | ErrorCode::UnsupportedProtocol
        | ErrorCode::InvalidEnvelope
        | ErrorCode::MessageTooLarge
        | ErrorCode::ActivityPackRevisionUnavailable
        | ErrorCode::RoomFaulted
        | ErrorCode::RoomQuarantined
        | ErrorCode::SyncBarrierMismatch
        | ErrorCode::IdempotencyConflict
        | ErrorCode::WrongPhase
        | ErrorCode::CommitIndeterminate
        | ErrorCode::InvalidPayload
        | ErrorCode::ActivityFault => AssignedMembershipGatewayErrorV1::InvalidData,
    }
}

fn validate_delivery(
    delivery: &ObservationDeliver,
    authority: &AssignedMembershipAuthorityV1,
) -> Result<(), AssignedMembershipGatewayErrorV1> {
    if delivery.room_id != authority.room_id() || delivery.member_id != authority.member_id() {
        return Err(AssignedMembershipGatewayErrorV1::Revoked);
    }
    Ok(())
}

impl From<ObservationDeliver> for ObservationDeliveryV1 {
    fn from(delivery: ObservationDeliver) -> Self {
        Self {
            frame_seq: delivery.frame_seq,
            cause_room_seq: delivery.cause_room_seq,
            frame_kind: delivery.frame_kind,
            observation_schema: delivery.observation_schema,
            observation: delivery.observation,
            frame_payload_hash: delivery.frame_payload_hash,
        }
    }
}

fn update_action_offers(
    current: &mut Vec<ActionOffer>,
    observation: &Value,
) -> Result<(), AssignedMembershipGatewayErrorV1> {
    if let Some(offers) = observation.get("action_offers") {
        *current = serde_json::from_value(offers.clone())
            .map_err(|_| AssignedMembershipGatewayErrorV1::InvalidData)?;
    }
    Ok(())
}

fn next_protocol_ulid() -> Result<UlidString, AssignedMembershipGatewayErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    std::str::from_utf8(&encoded)
        .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)?
        .parse()
        .map_err(|_| AssignedMembershipGatewayErrorV1::Unavailable)
}

/// Factory that issues one sealed MCP context from one assignment identity.
pub struct AssignmentMcpSupervisorV1<S, G> {
    source: Arc<S>,
    gateway: Arc<G>,
}

impl<S, G> AssignmentMcpSupervisorV1<S, G>
where
    S: AssignedMembershipSourceV1,
    G: AssignedMembershipGatewayV1,
{
    #[must_use]
    pub fn new(source: S, gateway: G) -> Self {
        Self {
            source: Arc::new(source),
            gateway: Arc::new(gateway),
        }
    }

    /// Issues one non-serializable context bound to exactly one assignment.
    ///
    /// # Errors
    ///
    /// Fails closed when the assignment or its participant authority is not usable.
    pub fn issue_context(
        &self,
        assignment_id: &str,
    ) -> Result<AssignmentMcpContextV1, AssignedMembershipSourceErrorV1> {
        if assignment_id.parse::<UlidString>().is_err() {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let authority = self.source.resolve_assignment(assignment_id)?;
        if authority.assignment_id != assignment_id {
            return Err(AssignedMembershipSourceErrorV1::Invalid);
        }
        let gateway: Arc<dyn AssignedMembershipGatewayV1> = self.gateway.clone();
        Ok(AssignmentMcpContextV1 {
            authority,
            gateway,
            lease: Arc::new(StaticAssignmentLeaseV1),
            actions: None,
            activations: None,
        })
    }
}

/// Non-serializable Supervisor-issued context consumed by one stdio helper.
pub struct AssignmentMcpContextV1 {
    authority: AssignedMembershipAuthorityV1,
    gateway: Arc<dyn AssignedMembershipGatewayV1>,
    lease: Arc<dyn AssignmentLeaseV1>,
    actions: Option<AssignmentMcpActionToolsV1>,
    activations: Option<AssignmentMcpActivationToolsV1>,
}

type AssignmentMcpActivationToolsV1 = AssignmentActivationToolsV1<
    FixedDaemonRunnerActivationGatewayV1,
    FileActivationOperationLedgerV1,
    SystemActivationLeaseClockV1,
>;

struct AssignmentMcpActionToolsV1 {
    schemas: PinnedAssignmentMcpActionSchemasV1,
    gateway: FixedDaemonAssignmentMcpActionGatewayV1,
    operations: FileAssignmentMcpOperationLedgerV1,
}

struct PinnedAssignmentMcpActionSchemasV1 {
    pack: PackReference,
    actions: Vec<ActivityPackCatalogAction>,
}

impl AssignmentMcpActionSchemaSourceV1 for PinnedAssignmentMcpActionSchemasV1 {
    fn exact_action(
        &self,
        pack: &PackReference,
        action_type: &str,
    ) -> Result<ActivityPackCatalogAction, AssignmentMcpActionSchemaErrorV1> {
        if pack != &self.pack {
            return Err(AssignmentMcpActionSchemaErrorV1::Missing);
        }
        self.actions
            .iter()
            .find(|action| action.action_type == action_type)
            .cloned()
            .ok_or(AssignmentMcpActionSchemaErrorV1::Missing)
    }
}

impl fmt::Debug for AssignmentMcpContextV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AssignmentMcpContextV1(REDACTED)")
    }
}

/// Safe MCP failure with an exact recovery action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentMcpErrorV1 {
    InvalidInput,
    AssignmentMissing,
    AssignmentRevoked,
    Disconnected,
    StaleCursor,
    DaemonUnavailable,
    InvalidDaemonData,
    AckBeyondDelivery,
    ActionObservationRequired,
    ActionUnoffered,
    ActionPayloadInvalid,
    ActionOperationConflict,
    ActionAmbiguous,
    NoActivation,
    ActivationLeaseExpired,
    ActivationStaleCursor,
    ActivationAlreadyCompleted,
    ActivationCompletionPending,
    ActivationIdempotencyConflict,
}

impl AssignmentMcpErrorV1 {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "assignment_tool_input_invalid",
            Self::AssignmentMissing => "assignment_not_found",
            Self::AssignmentRevoked => "assignment_authority_revoked",
            Self::Disconnected => "assignment_daemon_disconnected",
            Self::StaleCursor => "assignment_cursor_stale",
            Self::DaemonUnavailable => "assignment_daemon_unavailable",
            Self::InvalidDaemonData => "assignment_daemon_data_invalid",
            Self::AckBeyondDelivery => "assignment_ack_beyond_delivery",
            Self::ActionObservationRequired => "assignment_action_observation_required",
            Self::ActionUnoffered => "assignment_action_unoffered",
            Self::ActionPayloadInvalid => "assignment_action_payload_invalid",
            Self::ActionOperationConflict => "assignment_action_operation_conflict",
            Self::ActionAmbiguous => "assignment_action_response_ambiguous",
            Self::NoActivation => "assignment_activation_none_available",
            Self::ActivationLeaseExpired => "assignment_activation_lease_expired",
            Self::ActivationStaleCursor => "assignment_activation_cursor_stale",
            Self::ActivationAlreadyCompleted => "assignment_activation_already_completed",
            Self::ActivationCompletionPending => "assignment_activation_completion_pending",
            Self::ActivationIdempotencyConflict => "assignment_activation_operation_conflict",
        }
    }

    #[must_use]
    pub const fn next_action(self) -> &'static str {
        match self {
            Self::Disconnected | Self::DaemonUnavailable => "retry_observe",
            Self::StaleCursor => "retry_observe_for_projection_reset",
            Self::AssignmentMissing | Self::AssignmentRevoked | Self::InvalidDaemonData => {
                "return_to_task_setup"
            }
            Self::InvalidInput => "correct_tool_input",
            Self::AckBeyondDelivery => "observe_before_acknowledging",
            Self::ActionObservationRequired => "observe_then_list_current_offers",
            Self::ActionUnoffered | Self::ActionPayloadInvalid => "list_current_offers",
            Self::ActionOperationConflict => "use_new_operation_id",
            Self::ActionAmbiguous => "retry_same_operation_id",
            Self::NoActivation => "wait_then_request_next_activation",
            Self::ActivationLeaseExpired
            | Self::ActivationStaleCursor
            | Self::ActivationAlreadyCompleted => "request_next_activation",
            Self::ActivationCompletionPending => "retry_same_completion",
            Self::ActivationIdempotencyConflict => "restore_exact_completion_input",
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::InvalidInput => "The tool input does not match the assignment-bound contract.",
            Self::AssignmentMissing => "The assigned Task is no longer available.",
            Self::AssignmentRevoked => "The assigned Membership authority is no longer valid.",
            Self::Disconnected => {
                "The local daemon connection was lost; retry observation to resume from the durable Cursor."
            }
            Self::StaleCursor => {
                "The retained range is unavailable; retry observation to install the authorized Projection Reset."
            }
            Self::DaemonUnavailable => {
                "The local daemon is unavailable; retry observation after it is ready."
            }
            Self::InvalidDaemonData => {
                "The local daemon returned data outside the assigned Membership contract."
            }
            Self::AckBeyondDelivery => {
                "Acknowledge only an Observation Frame returned by this helper."
            }
            Self::ActionObservationRequired => {
                "Observe the assigned Membership before listing or submitting Actions."
            }
            Self::ActionUnoffered => "The requested Action is not in the exact current offer list.",
            Self::ActionPayloadInvalid => {
                "The Action payload does not match the exact declared schema."
            }
            Self::ActionOperationConflict => {
                "That operation identity is already bound to different Action input."
            }
            Self::ActionAmbiguous => {
                "The Action response is uncertain; retry the exact same operation identity."
            }
            Self::NoActivation => "No pending Activation is available for this assignment.",
            Self::ActivationLeaseExpired => "The exact Activation lease expired before completion.",
            Self::ActivationStaleCursor => {
                "The Activation Cursor or lease generation is no longer current."
            }
            Self::ActivationAlreadyCompleted => "The exact Activation was already completed.",
            Self::ActivationCompletionPending => {
                "Activation completion is pending; retry the same completion."
            }
            Self::ActivationIdempotencyConflict => {
                "The completion input conflicts with the retained exact operation."
            }
        }
    }
}

/// Stateful generic MCP surface for one sealed assignment context.
pub struct AssignmentMcpServerV1 {
    context: AssignmentMcpContextV1,
    current: Option<MembershipStreamSnapshotV1>,
    highest_delivered: Option<u64>,
    initialized: bool,
}

impl AssignmentMcpServerV1 {
    #[must_use]
    pub const fn open(context: AssignmentMcpContextV1) -> Self {
        Self {
            context,
            current: None,
            highest_delivered: None,
            initialized: false,
        }
    }

    /// Returns the exact materialized internal view for downstream bounded tools.
    #[must_use]
    pub const fn current_materialized_view(&self) -> Option<&MembershipStreamSnapshotV1> {
        self.current.as_ref()
    }

    /// Lists the single Task sealed into this helper context.
    ///
    /// # Errors
    ///
    /// Rejects any input fields, including routing or authority material.
    pub fn list_assigned_tasks(&self, arguments: Value) -> Result<Value, AssignmentMcpErrorV1> {
        decode_empty(arguments)?;
        self.context.lease.validate().map_err(map_source_error)?;
        Ok(serde_json::json!({
            "schema": "worldstream/assignment-task-list/v1",
            "tasks": [{
                "task_id": self.context.authority.assignment_id,
                "profile": {
                    "profile_id": self.context.authority.profile_id,
                    "revision": self.context.authority.profile_revision,
                },
                "role": self.context.authority.role,
            }],
        }))
    }

    /// Synchronizes and returns generic authorized Projection/Observation data.
    ///
    /// # Errors
    ///
    /// Rejects any input fields and returns only safe closed recovery guidance.
    pub fn observe(&mut self, arguments: Value) -> Result<Value, AssignmentMcpErrorV1> {
        decode_empty(arguments)?;
        self.context.lease.validate().map_err(map_source_error)?;
        let snapshot = self
            .context
            .gateway
            .synchronize(&self.context.authority)
            .map_err(map_gateway_error)?;
        validate_snapshot(&snapshot, self.context.authority.room_id())?;
        self.context.lease.validate().map_err(map_source_error)?;
        self.highest_delivered = snapshot
            .observations
            .iter()
            .map(|observation| observation.frame_seq)
            .max();
        let result = safe_snapshot(&snapshot);
        self.current = Some(snapshot);
        Ok(result)
    }

    /// Advances the sealed Membership Cursor through one delivered frame.
    ///
    /// # Errors
    ///
    /// Rejects unknown fields, impossible progress, and closed gateway failures.
    pub fn acknowledge(&mut self, arguments: Value) -> Result<Value, AssignmentMcpErrorV1> {
        let arguments: AckArguments =
            serde_json::from_value(arguments).map_err(|_| AssignmentMcpErrorV1::InvalidInput)?;
        self.context.lease.validate().map_err(map_source_error)?;
        if self
            .highest_delivered
            .is_none_or(|highest| arguments.through_frame_seq > highest)
        {
            return Err(AssignmentMcpErrorV1::AckBeyondDelivery);
        }
        let cursor = self
            .context
            .gateway
            .acknowledge(&self.context.authority, arguments.through_frame_seq)
            .map_err(map_gateway_error)?;
        self.context.lease.validate().map_err(map_source_error)?;
        if cursor < arguments.through_frame_seq {
            return Err(AssignmentMcpErrorV1::InvalidDaemonData);
        }
        if let Some(current) = &mut self.current {
            current.cursor = Some(cursor);
        }
        Ok(serde_json::json!({
            "schema": "worldstream/assignment-observation-ack/v1",
            "cursor": cursor,
            "next_action": "continue",
        }))
    }

    /// Lists exact current offers with their pinned Pack schemas.
    ///
    /// # Errors
    ///
    /// Requires a current valid observation, exact pinned schemas, and a live assignment lease.
    pub fn list_current_action_offers(
        &self,
        arguments: Value,
    ) -> Result<Value, AssignmentMcpErrorV1> {
        decode_empty(arguments)?;
        self.context.lease.validate().map_err(map_source_error)?;
        let tools = self
            .context
            .actions
            .as_ref()
            .ok_or(AssignmentMcpErrorV1::DaemonUnavailable)?;
        let snapshot = self
            .current
            .as_ref()
            .ok_or(AssignmentMcpErrorV1::ActionObservationRequired)?;
        serde_json::to_value(
            list_current_action_offers(snapshot, &tools.schemas).map_err(map_action_error)?,
        )
        .map_err(|_| AssignmentMcpErrorV1::InvalidDaemonData)
    }

    /// Submits one exact listed offer through the sealed participant authority.
    ///
    /// # Errors
    ///
    /// Returns only closed input, authority, durability, transport, or daemon-data failures.
    pub fn submit_action(&self, arguments: Value) -> Result<Value, AssignmentMcpErrorV1> {
        let arguments: AssignmentMcpSubmitActionV1 =
            serde_json::from_value(arguments).map_err(|_| AssignmentMcpErrorV1::InvalidInput)?;
        self.context.lease.validate().map_err(map_source_error)?;
        let tools = self
            .context
            .actions
            .as_ref()
            .ok_or(AssignmentMcpErrorV1::DaemonUnavailable)?;
        let snapshot = self
            .current
            .as_ref()
            .ok_or(AssignmentMcpErrorV1::ActionObservationRequired)?;
        let result = submit_current_action(
            &self.context.authority,
            snapshot,
            &tools.schemas,
            &tools.operations,
            &tools.gateway,
            arguments,
        )
        .map_err(map_action_error)?;
        self.context.lease.validate().map_err(map_source_error)?;
        serde_json::to_value(result).map_err(|_| AssignmentMcpErrorV1::InvalidDaemonData)
    }

    /// Acquires or resumes only the next Activation in the sealed Runner scope.
    ///
    /// # Errors
    ///
    /// Returns distinct safe lease, Cursor, authority, transport, or retained-state failures.
    pub fn next_activation(&self, arguments: &Value) -> Result<Value, AssignmentMcpErrorV1> {
        self.context.lease.validate().map_err(map_source_error)?;
        let tools = self
            .context
            .activations
            .as_ref()
            .ok_or(AssignmentMcpErrorV1::DaemonUnavailable)?;
        let result = tools
            .next_activation(arguments)
            .map_err(|error| map_activation_error(&error))?;
        self.context.lease.validate().map_err(map_source_error)?;
        serde_json::to_value(result).map_err(|_| AssignmentMcpErrorV1::InvalidDaemonData)
    }

    /// Completes only the exact currently retained Activation lease.
    ///
    /// # Errors
    ///
    /// Returns distinct safe precondition, lease, authority, retry, or retained-state failures.
    pub fn complete_activation(&self, arguments: Value) -> Result<Value, AssignmentMcpErrorV1> {
        self.context.lease.validate().map_err(map_source_error)?;
        let tools = self
            .context
            .activations
            .as_ref()
            .ok_or(AssignmentMcpErrorV1::DaemonUnavailable)?;
        let result = tools
            .complete(arguments)
            .map_err(|error| map_activation_error(&error))?;
        self.context.lease.validate().map_err(map_source_error)?;
        serde_json::to_value(result).map_err(|_| AssignmentMcpErrorV1::InvalidDaemonData)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AckArguments {
    through_frame_seq: u64,
}

fn decode_empty(value: Value) -> Result<(), AssignmentMcpErrorV1> {
    serde_json::from_value::<EmptyArguments>(value)
        .map(|_| ())
        .map_err(|_| AssignmentMcpErrorV1::InvalidInput)
}

fn validate_snapshot(
    snapshot: &MembershipStreamSnapshotV1,
    room_id: &str,
) -> Result<(), AssignmentMcpErrorV1> {
    let authorized_values = serde_json::to_value((
        &snapshot.pack,
        &snapshot.current_action_offers,
        &snapshot.projection_reset,
        &snapshot.observations,
    ))
    .map_err(|_| AssignmentMcpErrorV1::InvalidDaemonData)?;
    if snapshot.room_head.room_id != room_id
        || snapshot.pack.digest != snapshot.room_head.pack_digest
        || snapshot.retained_floor > snapshot.frame_head
        || snapshot
            .cursor
            .is_some_and(|cursor| cursor > snapshot.frame_head)
        || snapshot.observations.len() > MAX_OBSERVATIONS
        || snapshot
            .observations
            .windows(2)
            .any(|pair| pair[0].frame_seq >= pair[1].frame_seq)
        || snapshot
            .observations
            .iter()
            .any(|observation| observation.frame_seq > snapshot.frame_head)
        || snapshot.observations.first().is_some_and(|observation| {
            observation.frame_seq < snapshot.retained_floor
                || snapshot
                    .projection_reset
                    .as_ref()
                    .map(|reset| reset.baseline_frame_head)
                    .or(snapshot.cursor)
                    .is_some_and(|cursor| observation.frame_seq != cursor.saturating_add(1))
        })
        || snapshot
            .observations
            .windows(2)
            .any(|pair| pair[1].frame_seq != pair[0].frame_seq.saturating_add(1))
        || snapshot
            .observations
            .last()
            .is_some_and(|observation| observation.frame_seq != snapshot.frame_head)
        || snapshot.projection_reset.as_ref().is_some_and(|reset| {
            reset.baseline_frame_head > snapshot.frame_head
                || (snapshot.observations.is_empty()
                    && reset.baseline_frame_head != snapshot.frame_head)
        })
        || contains_prohibited_material(&authorized_values)
    {
        return Err(AssignmentMcpErrorV1::InvalidDaemonData);
    }
    Ok(())
}

fn contains_prohibited_material(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            let lowercase = text.to_ascii_lowercase();
            lowercase.contains("wsb1:")
                || lowercase.contains("wst1:")
                || lowercase.starts_with("bearer ")
        }
        Value::Array(values) => values.iter().any(contains_prohibited_material),
        Value::Object(object) => object.iter().any(|(key, child)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "room_id"
                    | "member_id"
                    | "membership_id"
                    | "bearer"
                    | "authorization"
                    | "credential"
                    | "token_hash"
                    | "secret_reference"
                    | "secret_ref"
                    | "sync_token"
            ) || contains_prohibited_material(child)
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn safe_snapshot(snapshot: &MembershipStreamSnapshotV1) -> Value {
    serde_json::json!({
        "schema": "worldstream/assignment-observation/v1",
        "head": {
            "room_seq": snapshot.room_head.room_seq,
            "genesis_or_transition_hash": snapshot.room_head.genesis_or_transition_hash,
            "core_schema_version": snapshot.room_head.core_schema_version,
            "pack_digest": snapshot.room_head.pack_digest,
            "core_state_hash": snapshot.room_head.core_state_hash,
            "activity_state_hash": snapshot.room_head.activity_state_hash,
            "authoritative_state_hash": snapshot.room_head.authoritative_state_hash,
        },
        "pack": snapshot.pack,
        "stream": {
            "cursor": snapshot.cursor,
            "frame_head": snapshot.frame_head,
            "retained_floor": snapshot.retained_floor,
        },
        "action_offers": snapshot.current_action_offers,
        "projection_reset": snapshot.projection_reset,
        "observations": snapshot.observations,
        "next_action": if snapshot.observations.is_empty() { "wait_or_observe_again" } else { "process_then_acknowledge" },
    })
}

fn map_gateway_error(error: AssignedMembershipGatewayErrorV1) -> AssignmentMcpErrorV1 {
    match error {
        AssignedMembershipGatewayErrorV1::Disconnected => AssignmentMcpErrorV1::Disconnected,
        AssignedMembershipGatewayErrorV1::Revoked => AssignmentMcpErrorV1::AssignmentRevoked,
        AssignedMembershipGatewayErrorV1::StaleCursor => AssignmentMcpErrorV1::StaleCursor,
        AssignedMembershipGatewayErrorV1::Unavailable => AssignmentMcpErrorV1::DaemonUnavailable,
        AssignedMembershipGatewayErrorV1::InvalidData => AssignmentMcpErrorV1::InvalidDaemonData,
    }
}

fn map_source_error(error: AssignedMembershipSourceErrorV1) -> AssignmentMcpErrorV1 {
    match error {
        AssignedMembershipSourceErrorV1::NotFound => AssignmentMcpErrorV1::AssignmentMissing,
        AssignedMembershipSourceErrorV1::Revoked => AssignmentMcpErrorV1::AssignmentRevoked,
        AssignedMembershipSourceErrorV1::Invalid | AssignedMembershipSourceErrorV1::Unavailable => {
            AssignmentMcpErrorV1::DaemonUnavailable
        }
    }
}

const fn map_action_error(error: AssignmentMcpActionErrorV1) -> AssignmentMcpErrorV1 {
    match error {
        AssignmentMcpActionErrorV1::InvalidInput => AssignmentMcpErrorV1::InvalidInput,
        AssignmentMcpActionErrorV1::Unoffered => AssignmentMcpErrorV1::ActionUnoffered,
        AssignmentMcpActionErrorV1::InvalidPayload => AssignmentMcpErrorV1::ActionPayloadInvalid,
        AssignmentMcpActionErrorV1::OperationConflict => {
            AssignmentMcpErrorV1::ActionOperationConflict
        }
        AssignmentMcpActionErrorV1::AmbiguousRetrySameOperation => {
            AssignmentMcpErrorV1::ActionAmbiguous
        }
        AssignmentMcpActionErrorV1::AssignmentRevoked => AssignmentMcpErrorV1::AssignmentRevoked,
        AssignmentMcpActionErrorV1::OperationUnavailable
        | AssignmentMcpActionErrorV1::SchemaUnavailable => AssignmentMcpErrorV1::DaemonUnavailable,
        AssignmentMcpActionErrorV1::InvalidOfferData
        | AssignmentMcpActionErrorV1::InvalidDaemonData => AssignmentMcpErrorV1::InvalidDaemonData,
    }
}

const fn map_activation_error(error: &ActivationToolErrorV1) -> AssignmentMcpErrorV1 {
    match error {
        ActivationToolErrorV1::InvalidArguments => AssignmentMcpErrorV1::InvalidInput,
        ActivationToolErrorV1::NoActivation => AssignmentMcpErrorV1::NoActivation,
        ActivationToolErrorV1::LeaseExpired => AssignmentMcpErrorV1::ActivationLeaseExpired,
        ActivationToolErrorV1::AuthorityRevoked => AssignmentMcpErrorV1::AssignmentRevoked,
        ActivationToolErrorV1::StaleCursor => AssignmentMcpErrorV1::ActivationStaleCursor,
        ActivationToolErrorV1::AlreadyCompleted => AssignmentMcpErrorV1::ActivationAlreadyCompleted,
        ActivationToolErrorV1::CompletionPending => {
            AssignmentMcpErrorV1::ActivationCompletionPending
        }
        ActivationToolErrorV1::IdempotencyConflict => {
            AssignmentMcpErrorV1::ActivationIdempotencyConflict
        }
        ActivationToolErrorV1::Disconnected => AssignmentMcpErrorV1::Disconnected,
        ActivationToolErrorV1::Unavailable => AssignmentMcpErrorV1::DaemonUnavailable,
        ActivationToolErrorV1::InvalidRetainedState | ActivationToolErrorV1::InvalidDaemonData => {
            AssignmentMcpErrorV1::InvalidDaemonData
        }
    }
}

/// Runs the bounded line-delimited JSON-RPC MCP server over local stdio.
///
/// # Errors
///
/// Returns only local I/O failures. Invalid requests receive safe MCP errors.
pub fn run_assignment_mcp_stdio(
    server: &mut AssignmentMcpServerV1,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<()> {
    loop {
        let Some(line) = read_bounded_line(input)? else {
            return Ok(());
        };
        let response = if line.too_long {
            Some(rpc_error(&Value::Null, AssignmentMcpErrorV1::InvalidInput))
        } else {
            std::str::from_utf8(&line.bytes).map_or_else(
                |_| Some(rpc_error(&Value::Null, AssignmentMcpErrorV1::InvalidInput)),
                |line| handle_rpc(server, line),
            )
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut *output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

struct BoundedLineV1 {
    bytes: Vec<u8>,
    too_long: bool,
}

fn read_bounded_line(input: &mut impl BufRead) -> io::Result<Option<BoundedLineV1>> {
    let mut bytes = Vec::new();
    let mut too_long = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() && !too_long {
                Ok(None)
            } else {
                Ok(Some(BoundedLineV1 { bytes, too_long }))
            };
        }
        let through = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let remaining = MAX_MCP_LINE_BYTES.saturating_sub(bytes.len());
        let retained = through.min(remaining);
        bytes.extend_from_slice(&available[..retained]);
        too_long |= through > retained;
        let ended = available.get(through.saturating_sub(1)) == Some(&b'\n');
        input.consume(through);
        if ended {
            return Ok(Some(BoundedLineV1 { bytes, too_long }));
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCallParams {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct InitializeParamsV1 {
    protocol_version: String,
    capabilities: Value,
    client_info: McpClientInfoV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpClientInfoV1 {
    name: String,
    version: String,
}

fn empty_object() -> Value {
    serde_json::json!({})
}

fn handle_rpc(server: &mut AssignmentMcpServerV1, line: &str) -> Option<Value> {
    let request: RpcRequest = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(_) => return Some(json_rpc_error(&Value::Null, -32700, "Parse error")),
    };
    let is_notification = request.id.is_none();
    let id = request.id.unwrap_or(Value::Null);
    if request.jsonrpc != "2.0" || !bounded_label(&request.method) {
        return (!is_notification).then(|| json_rpc_error(&id, -32600, "Invalid Request"));
    }
    match request.method.as_str() {
        "initialize" if !is_notification && !server.initialized => {
            let parameters: InitializeParamsV1 = match serde_json::from_value(request.params) {
                Ok(parameters) => parameters,
                Err(_) => return Some(json_rpc_error(&id, -32602, "Invalid params")),
            };
            if parameters.protocol_version != "2025-06-18"
                || !bounded_label(&parameters.client_info.name)
                || !bounded_label(&parameters.client_info.version)
                || contains_prohibited_material(&parameters.capabilities)
            {
                return Some(json_rpc_error(&id, -32602, "Invalid params"));
            }
            server.initialized = true;
            Some(serde_json::json!({
                "jsonrpc":"2.0", "id":id,
                "result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"worldstream-assignment-mcp","version":env!("CARGO_PKG_VERSION")}}
            }))
        }
        "notifications/initialized"
            if is_notification
                && server.initialized
                && (matches!(request.params, Value::Null)
                    || request.params == serde_json::json!({})) =>
        {
            None
        }
        "tools/list"
            if server.initialized
                && !is_notification
                && (matches!(request.params, Value::Null)
                    || request.params == serde_json::json!({})) =>
        {
            Some(serde_json::json!({
            "jsonrpc":"2.0", "id":id, "result":{"tools":tool_definitions()}
            }))
        }
        "tools/call" if server.initialized && !is_notification => {
            Some(call_tool(server, &id, request.params))
        }
        "initialize" | "notifications/initialized" | "tools/list" | "tools/call" => {
            (!is_notification).then(|| json_rpc_error(&id, -32600, "Invalid Request"))
        }
        _ => (!is_notification).then(|| json_rpc_error(&id, -32601, "Method not found")),
    }
}

fn call_tool(server: &mut AssignmentMcpServerV1, id: &Value, params: Value) -> Value {
    let params: ToolCallParams = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(_) => return rpc_error(id, AssignmentMcpErrorV1::InvalidInput),
    };
    let result = match params.name.as_str() {
        "worldstream.list_assigned_tasks" => server.list_assigned_tasks(params.arguments),
        "worldstream.observe" => server.observe(params.arguments),
        "worldstream.acknowledge" => server.acknowledge(params.arguments),
        "worldstream.list_current_action_offers" => {
            server.list_current_action_offers(params.arguments)
        }
        "worldstream.submit_action" => server.submit_action(params.arguments),
        "worldstream.next_activation" => server.next_activation(&params.arguments),
        "worldstream.complete_activation" => server.complete_activation(params.arguments),
        _ => Err(AssignmentMcpErrorV1::InvalidInput),
    };
    match result {
        Ok(value) => serde_json::json!({
            "jsonrpc":"2.0", "id":id,
            "result":{"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}
        }),
        Err(error) => rpc_tool_error(id, error),
    }
}

fn tool_definitions() -> Value {
    serde_json::json!([
        {"name":"worldstream.list_assigned_tasks","description":"List only Tasks sealed into this assignment context.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"worldstream.observe","description":"Resume the assigned Membership Observation Stream from its durable Cursor.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"worldstream.acknowledge","description":"Durably acknowledge processed assigned Observation Frames.","inputSchema":{"type":"object","properties":{"through_frame_seq":{"type":"integer","minimum":0}},"required":["through_frame_seq"],"additionalProperties":false}},
        {"name":"worldstream.list_current_action_offers","description":"List exact current Action Offers and their pinned payload schemas.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"worldstream.submit_action","description":"Submit one exact currently offered Action using a stable operation identity.","inputSchema":{"type":"object","properties":{"operation_id":{"type":"string"},"offer_id":{"type":"string"},"precondition":{"type":"object","properties":{"room_seq":{"type":"integer","minimum":0},"head_hash":{"type":"string"}},"required":["room_seq","head_hash"],"additionalProperties":false},"payload":{}},"required":["operation_id","offer_id","precondition","payload"],"additionalProperties":false}},
        {"name":"worldstream.next_activation","description":"Acquire or resume the next Activation in this assignment's sealed Runner scope.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"worldstream.complete_activation","description":"Complete the exact currently leased Activation with its safe preconditions.","inputSchema":{"type":"object","properties":{"activation_cursor":{"type":"integer","minimum":1},"lease_generation":{"type":"integer","minimum":1},"context_hash":{"type":"string"},"disposition":{"type":"string","enum":["handled","declined","failed"]}},"required":["activation_cursor","lease_generation","context_hash","disposition"],"additionalProperties":false}}
    ])
}

fn rpc_tool_error(id: &Value, error: AssignmentMcpErrorV1) -> Value {
    let safe = safe_error(error);
    serde_json::json!({
        "jsonrpc":"2.0", "id":id,
        "result":{"content":[{"type":"text","text":safe.to_string()}],"structuredContent":safe,"isError":true}
    })
}

fn rpc_error(id: &Value, error: AssignmentMcpErrorV1) -> Value {
    serde_json::json!({
        "jsonrpc":"2.0", "id":id,
        "error":{"code":-32602,"message":"Assignment-bound MCP request rejected.","data":safe_error(error)}
    })
}

fn json_rpc_error(id: &Value, code: i32, message: &'static str) -> Value {
    serde_json::json!({
        "jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}
    })
}

fn safe_error(error: AssignmentMcpErrorV1) -> Value {
    serde_json::json!({
        "code":error.code(), "message":error.message(), "next_action":error.next_action(),
        "retryable":matches!(error, AssignmentMcpErrorV1::Disconnected | AssignmentMcpErrorV1::StaleCursor | AssignmentMcpErrorV1::DaemonUnavailable | AssignmentMcpErrorV1::ActionAmbiguous | AssignmentMcpErrorV1::NoActivation | AssignmentMcpErrorV1::ActivationCompletionPending)
    })
}

fn bounded_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::{fs, time::Duration};

    use super::{
        AssignedMembershipGatewayErrorV1, FixedDaemonAssignedMembershipGatewayV1,
        StreamContinuityV1, update_action_offers,
    };
    use serde_json::json;
    use tempfile::tempdir;
    use worldstream_protocol::ActionOffer;

    #[test]
    fn retained_frames_keep_or_replace_only_exact_canonical_action_offers() {
        let mut current = vec![offer("first")];
        update_action_offers(&mut current, &json!({"value": 1}))
            .unwrap_or_else(|error| panic!("unchanged offers: {error:?}"));
        assert_eq!(current, vec![offer("first")]);

        update_action_offers(
            &mut current,
            &json!({"value": 2, "action_offers": [offer("second")]}),
        )
        .unwrap_or_else(|error| panic!("changed offers: {error:?}"));
        assert_eq!(current, vec![offer("second")]);
        assert_eq!(
            update_action_offers(
                &mut current,
                &json!({"action_offers":[{"action_type":"unsafe"}]})
            ),
            Err(AssignedMembershipGatewayErrorV1::InvalidData),
        );
    }

    #[test]
    fn durable_continuity_survives_restart_without_authority_or_routing_material() {
        const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
        const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
        const BEARER: &str =
            "wsb1:abababababababababababababababababababababababababababababababab";

        let directory = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
        let continuity_root = directory.path().join("continuity");
        let first = FixedDaemonAssignedMembershipGatewayV1::open(
            "127.0.0.1:9410"
                .parse()
                .unwrap_or_else(|error| panic!("local address: {error}")),
            Duration::from_millis(50),
            &continuity_root,
        )
        .unwrap_or_else(|error| panic!("first gateway: {error:?}"));
        first
            .persist_continuity(
                ASSIGNMENT,
                StreamContinuityV1 {
                    cursor: Some(8),
                    current_action_offers: vec![offer("increment")],
                    initialized: true,
                    pending_ack: Some((Some(7), 8)),
                },
            )
            .unwrap_or_else(|error| panic!("persist continuity: {error:?}"));
        drop(first);

        let restarted = FixedDaemonAssignedMembershipGatewayV1::open(
            "127.0.0.1:9410"
                .parse()
                .unwrap_or_else(|error| panic!("local address: {error}")),
            Duration::from_millis(50),
            &continuity_root,
        )
        .unwrap_or_else(|error| panic!("restarted gateway: {error:?}"));
        let retained = restarted
            .continuity_for(ASSIGNMENT)
            .unwrap_or_else(|error| panic!("load continuity: {error:?}"));
        assert_eq!(retained.cursor, Some(8));
        assert_eq!(retained.current_action_offers, vec![offer("increment")]);
        assert!(retained.initialized);
        assert_eq!(retained.pending_ack, Some((Some(7), 8)));

        let persisted = fs::read_to_string(continuity_root.join(format!("{ASSIGNMENT}.json")))
            .unwrap_or_else(|error| panic!("read continuity: {error}"));
        for prohibited in [ROOM, MEMBER, BEARER, "room_id", "member_id", "bearer"] {
            assert!(!persisted.contains(prohibited), "leaked {prohibited}");
        }
    }

    fn offer(action_type: &str) -> ActionOffer {
        ActionOffer {
            domain: "worldstream/action-offer/v1".to_owned(),
            action_type: action_type.to_owned(),
            payload_schema_digest: format!("blake3:{}", "a".repeat(64)),
            eligibility_window: None,
        }
    }
}
