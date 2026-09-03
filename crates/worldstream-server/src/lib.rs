//! Bounded public HTTP/WebSocket gateway. Domain and storage decisions stay
//! behind [`GatewayBackend`]; the default backend fails closed until a
//! supervisor wires the verified storage seams into this process.

mod args;
#[cfg(feature = "cli-operator-preview")]
pub mod managed_control;
pub mod operator_packs;
pub mod operator_storage;
pub mod operator_transfer;
mod pack_startup;
mod postgres_backend;
mod rate_limit;
mod sqlite_backend;

pub mod telemetry;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    fmt,
    fmt::Write as _,
    net::SocketAddr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};

use axum::{
    Json, Router,
    body::Bytes,
    extract::ws::{CloseFrame, Message, WebSocket},
    extract::{ConnectInfo, FromRequestParts, Path, RawQuery, Request, State, WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, options, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use worldstream_core::{
    Blake3DigestV1, CapabilityBearerV1, CapabilityScopeSetV1, CapabilityScopeV1,
    RoomAdmissionQueueSnapshotV1,
};
use worldstream_protocol::{
    ACTIVITY_PACK_CATALOG_VERSION, AccessMode, ActionAccepted, ActionRejected, ActionSubmit,
    ActivationClaim, ActivationLeaseOperation, ActivationOfferRequest, ActivationOffers,
    ActivationOperationReply, ActivationResultCode, ActivityPackCatalogAction,
    ActivityPackCatalogResponse, ActivityPackCatalogRevisionDetail,
    ActivityPackCatalogRevisionResponse, ActivityPackCatalogRevisionSummary,
    ActivityPackCatalogRole, ActivityPackCatalogSchema, ActivityPackLobbyCompatibility,
    BROWSER_WS_TICKET_VERSION, BearerWireV1, BrowserWebSocketTicketIssueResponse, ClientHello,
    ClientMode, CreateRoomRequest, CreateRoomResponse, ErrorBody, ErrorCode, ErrorEnvelope,
    LobbyLaunchRequest, LobbyLaunchResponse, MemberCapabilityProvisionRequestV1,
    MemberCapabilityProvisionResponseV1, ObservationAck, ObservationDeliver,
    OperatorActivationStatusV1, OperatorBackupProfileStatus, OperatorLiveBackupPrepareRequest,
    OperatorLiveBackupStatus, OperatorRoomInventoryPage, OperatorRoomInventoryRequest,
    OperatorRoomSummary, OperatorRunnerConnectionV1, OperatorRunnerFreshnessV1,
    OperatorRunnerPresenceV1, PackReference, ProjectionReset, ProjectionResponse, ProtocolEnvelope,
    ReplayResponse, RoomAttach, RoomAttached, RoomSyncAck, RunnerCapabilityProvisionRequestV1,
    RunnerCapabilityProvisionResponseV1, RunnerHello, RunnerReady, ServerWelcome, TimerFireRequest,
    TimerFireResponse, UlidString, VersionedEnvelope, WEBSOCKET_SUBPROTOCOL, decode_envelope,
};
use worldstream_runtime::{
    CompatibilitySummary, EffectiveConfig, ManifestError, StorageProfile, embedded_manifest,
};

use rate_limit::{
    ActiveWebSocketPermit, AdmissionTarget, GatewayAdmission, GatewayRateLimiter, PeerIdentity,
    PendingWebSocketPermit, RateLimitRejection,
};

pub use args::CommonConfigArgs;
pub use pack_startup::{
    StartupPackFactsV1, StartupPackRegistryDiagnosticsV1, StartupPackRegistryErrorV1,
    StartupPackRegistryV1, assemble_startup_pack_registry, pack_deployment_binding,
    verify_startup_pack_readiness_seal,
};
pub use postgres_backend::{PostgresGatewayBackend, read_postgres_dsn};
pub use sqlite_backend::SqliteGatewayBackend;

/// Exact source commit embedded by the crate build script.
pub const BUILD_REVISION: &str = env!("WORLDSTREAM_BUILD_REVISION");

/// The daemon's local telemetry boundary. Events are serialized and emitted
/// by the telemetry worker into the existing structured logging pipeline; no
/// network exporter or readiness dependency is implied.
#[derive(Clone, Copy, Debug, Default)]
pub struct StructuredLogTelemetryExporter;

impl telemetry::TelemetryExporter for StructuredLogTelemetryExporter {
    fn export(&self, batch: &[telemetry::TelemetryEventV1]) -> Result<(), telemetry::ExportError> {
        for event in batch {
            let line = event
                .json_line()
                .map_err(|_| telemetry::ExportError::Serialization)?;
            tracing::info!(target: "worldstream.telemetry", event = %line, "telemetry event");
        }
        Ok(())
    }
}

struct TelemetryRuntimeOwner {
    runtime: Mutex<Option<telemetry::TelemetryRuntime>>,
}

impl Drop for TelemetryRuntimeOwner {
    fn drop(&mut self) {
        let runtime = self
            .runtime
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(runtime) = runtime
            && runtime.shutdown(Duration::from_secs(3)) == telemetry::FlushOutcome::TimedOut
        {
            tracing::warn!(
                "telemetry shutdown flush timed out; remaining diagnostics were dropped"
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InternalQueueSnapshot {
    pub(crate) name: &'static str,
    pub(crate) capacity_scope: &'static str,
    pub(crate) capacity: usize,
    pub(crate) process_current: usize,
    pub(crate) process_high_water: usize,
    pub(crate) unit_high_water: usize,
    pub(crate) activity_total: u64,
    pub(crate) completion_total: u64,
    pub(crate) backpressure_total: u64,
}

impl InternalQueueSnapshot {
    fn room_admission(snapshot: RoomAdmissionQueueSnapshotV1) -> Self {
        Self {
            name: "room_admission_lane",
            capacity_scope: "per_room",
            capacity: snapshot.capacity,
            process_current: snapshot.process_current,
            process_high_water: snapshot.process_high_water,
            unit_high_water: snapshot.unit_high_water,
            activity_total: snapshot.admitted_total,
            completion_total: snapshot.completed_total,
            backpressure_total: snapshot.full_total,
        }
    }
}

fn atomic_max_usize(target: &AtomicUsize, candidate: usize) {
    let mut current = target.load(Ordering::Acquire);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

/// Maximum number of Observation Frames buffered by one connection. A
/// reconnecting client must use another attach/cursor step rather than making
/// the WebSocket writer hold an unbounded burst for a slow consumer.
pub const MAX_OUTBOUND_FRAME_BURST: usize = 256;
const MAX_OUTBOUND_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const WEBSOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY_ENV: &str =
    "WORLDSTREAM_TEST_KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY";
const KILL_AFTER_ACTIVATION_CLAIM_BEFORE_REPLY_ENV: &str =
    "WORLDSTREAM_TEST_KILL_AFTER_ACTIVATION_CLAIM_BEFORE_REPLY";
static ACTIVATION_CLAIM_BOUNDARY_TRIGGERED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
#[cfg(target_os = "linux")]
const CRASH_EVIDENCE_ENABLE_ENV: &str = "WORLDSTREAM_TEST_CRASH_EVIDENCE_ENABLE";
#[cfg(any(target_os = "linux", test))]
const CRASH_EVIDENCE_ENABLE_VALUE: &str = "worldstream-linux-process-evidence-v1";
#[cfg(target_os = "linux")]
const CRASH_EVIDENCE_POINT_ENV: &str = "WORLDSTREAM_TEST_CRASH_EVIDENCE_POINT";
#[cfg(target_os = "linux")]
const CRASH_EVIDENCE_MATCH_ID_ENV: &str = "WORLDSTREAM_TEST_CRASH_EVIDENCE_MATCH_ID";
#[cfg(target_os = "linux")]
const CRASH_EVIDENCE_MARKER_ENV: &str = "WORLDSTREAM_TEST_CRASH_EVIDENCE_MARKER";
#[cfg(target_os = "linux")]
const CRASH_MARKER_SCHEMA: &str = "worldstream/test-crash-boundary-ready/v1";

#[cfg(any(target_os = "linux", test))]
fn crash_evidence_configuration_matches(
    enabled: Option<&str>,
    configured_point: Option<&str>,
    configured_match_id: Option<&str>,
    operation: &str,
    boundary: &str,
    match_id: &str,
) -> bool {
    enabled == Some(CRASH_EVIDENCE_ENABLE_VALUE)
        && configured_point == Some(&format!("{operation}:{boundary}"))
        && configured_match_id == Some(match_id)
}

#[cfg(any(target_os = "linux", test))]
fn crash_evidence_point_is_valid(point: &str) -> bool {
    matches!(
        point,
        "room_create:before_commit"
            | "room_create:after_commit_before_publication"
            | "room_create:after_publication_before_reply"
            | "action:before_commit"
            | "action:after_commit_before_publication"
            | "action:after_publication_before_reply"
            | "timer:before_commit"
            | "timer:after_commit_before_publication"
            | "timer:after_publication_before_reply"
            | "activation_lease:before_commit"
            | "activation_lease:after_commit_before_publication"
            | "activation_lease:after_publication_before_reply"
    )
}

#[cfg(any(target_os = "linux", test))]
fn crash_evidence_match_id_is_valid(match_id: &str) -> bool {
    !match_id.is_empty()
        && match_id.len() <= 512
        && match_id.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(any(target_os = "linux", test))]
fn crash_publication_contract(operation: &str) -> &'static str {
    match operation {
        "room_create" => "telemetry_only_no_preexisting_room_observer",
        "action" | "timer" => "telemetry_and_live_room_frames",
        "activation_lease" => "activation_telemetry_no_room_frame",
        _ => "unknown_operation",
    }
}

/// Pauses at one exact, test-only process boundary after durably publishing a
/// private marker. The Linux evidence harness observes the marker and sends
/// `SIGKILL` itself; this function never labels an abort or graceful exit as a
/// process-kill result. A create-new marker makes the seam one-shot across a
/// same-data-directory restart.
///
/// The production process is unchanged unless the exact enable value is
/// present. Once enabled, a point outside the closed twelve-cell matrix, an
/// unsafe identity shape, or an unsafe marker path aborts. Non-target calls
/// return normally so a later configured boundary can traverse earlier seams;
/// a wrong-but-well-formed identity simply never emits a marker and therefore
/// cannot be promoted by the bounded harness.
fn pause_for_process_crash_evidence(operation: &str, boundary: &str, match_id: &str) {
    #[cfg(not(target_os = "linux"))]
    let _ = (operation, boundary, match_id);

    #[cfg(target_os = "linux")]
    {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::path::Path;

        let enabled = std::env::var(CRASH_EVIDENCE_ENABLE_ENV).ok();
        if enabled.as_deref() != Some(CRASH_EVIDENCE_ENABLE_VALUE) {
            return;
        }
        let configured_point = std::env::var(CRASH_EVIDENCE_POINT_ENV).ok();
        let configured_match_id = std::env::var(CRASH_EVIDENCE_MATCH_ID_ENV).ok();
        if configured_point
            .as_deref()
            .is_none_or(|point| !crash_evidence_point_is_valid(point))
            || configured_match_id
                .as_deref()
                .is_none_or(|configured| !crash_evidence_match_id_is_valid(configured))
        {
            tracing::error!("opt-in crash evidence point or operation identity is invalid");
            std::process::abort();
        }
        if !crash_evidence_configuration_matches(
            enabled.as_deref(),
            configured_point.as_deref(),
            configured_match_id.as_deref(),
            operation,
            boundary,
            match_id,
        ) {
            return;
        }

        let marker_path = std::env::var(CRASH_EVIDENCE_MARKER_ENV).ok();
        let Some(marker_path) = marker_path.as_deref().map(Path::new) else {
            tracing::error!("opt-in crash evidence marker path is missing");
            std::process::abort();
        };
        if !marker_path.is_absolute() || marker_path.parent().is_none_or(|parent| !parent.is_dir())
        {
            tracing::error!("opt-in crash evidence marker path is unsafe");
            std::process::abort();
        }
        let marker_value = json!({
            "schema": CRASH_MARKER_SCHEMA,
            "operation": operation,
            "boundary": boundary,
            "match_id": match_id,
            "publication_contract": crash_publication_contract(operation),
        });
        let marker_bytes = serde_json::to_vec(&marker_value)
            .unwrap_or_else(|_| unreachable!("fixed crash marker is serializable"));
        let marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(marker_path);
        let Ok(mut marker) = marker else {
            tracing::error!("opt-in crash evidence marker could not be created exactly once");
            std::process::abort();
        };
        if std::io::Write::write_all(&mut marker, &marker_bytes).is_err()
            || std::io::Write::write_all(&mut marker, b"\n").is_err()
            || marker.sync_all().is_err()
        {
            tracing::error!("opt-in crash evidence marker could not be persisted");
            std::process::abort();
        }
        drop(marker);
        tracing::warn!(
            operation,
            boundary,
            match_id,
            "opt-in crash evidence boundary reached; awaiting external SIGKILL"
        );
        loop {
            std::thread::park_timeout(Duration::from_secs(60));
        }
    }
}

/// Result of one backend operation. Implementations must map storage and
/// authority failures to these closed, non-secret errors.
#[derive(Clone, Debug, Error)]
pub enum BackendError {
    #[error("storage is unavailable")]
    StorageUnavailable,
    #[error("the operation is not authorized")]
    Forbidden,
    #[error("the requested room or membership is unavailable")]
    NotFound,
    #[error("the exact Activity Pack revision is unavailable")]
    ActivityPackRevisionUnavailable,
    #[error("the operation is temporarily busy")]
    Busy,
    #[error("the operation was rejected")]
    Rejected,
    #[error("the operation could not be resolved safely")]
    Indeterminate,
    #[error("the operation conflicts with an existing identity")]
    Conflict,
    #[error("the backend returned an invalid result")]
    InvalidResult,
    #[error("the room is faulted and only verified retained data is readable")]
    RoomFaulted,
    #[error("the room is quarantined")]
    RoomQuarantined,
    #[error("the operation is not applicable in the current Activity phase")]
    WrongPhase,
}

impl BackendError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::StorageUnavailable => ErrorCode::StorageUnavailable,
            Self::Forbidden => ErrorCode::Forbidden,
            Self::NotFound => ErrorCode::RoomNotFound,
            Self::ActivityPackRevisionUnavailable => ErrorCode::ActivityPackRevisionUnavailable,
            Self::Busy => ErrorCode::RoomBusy,
            Self::Rejected => ErrorCode::InvalidPayload,
            Self::Indeterminate => ErrorCode::CommitIndeterminate,
            Self::Conflict => ErrorCode::IdempotencyConflict,
            Self::InvalidResult => ErrorCode::Internal,
            Self::RoomFaulted => ErrorCode::RoomFaulted,
            Self::RoomQuarantined => ErrorCode::RoomQuarantined,
            Self::WrongPhase => ErrorCode::WrongPhase,
        }
    }
}

/// One bounded action result from a Room backend.
#[derive(Clone, Debug)]
pub enum ActionReply {
    Accepted(ActionAccepted),
    Rejected(ActionRejected),
}

/// The complete attach barrier plus exactly one retained/reset branch.
#[derive(Clone, Debug)]
pub struct AttachReply {
    pub attached: RoomAttached,
    pub reset: Option<ProjectionReset>,
    pub frames: Vec<ObservationDeliver>,
}

/// Explicit operator request for one new Room-member Capability.
///
/// The request carries only public identity and typed policy inputs. The
/// authenticated `HostOperator` bearer is taken from the HTTP Authorization
/// header and is never accepted as the resulting member bearer.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberCapabilityIssueRequest {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub scopes: Vec<CapabilityScopeV1>,
    pub idempotency_key: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// One-time plaintext delivery of a newly registered member Capability.
///
/// The bearer is returned only in this response. The authority store receives
/// only its token hash, and the type deliberately redacts the bearer in Debug.
#[derive(Clone, Deserialize, Serialize)]
pub struct MemberCapabilityIssueResponse {
    pub capability_id: String,
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub scopes: CapabilityScopeSetV1,
    pub bearer: String,
}

impl std::fmt::Debug for MemberCapabilityIssueResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemberCapabilityIssueResponse")
            .field("capability_id", &self.capability_id)
            .field("room_id", &self.room_id)
            .field("member_id", &self.member_id)
            .field("principal_id", &self.principal_id)
            .field("scopes", &self.scopes)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

/// One public Room/Member target permitted to an external Runner.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerMembershipTarget {
    pub room_id: String,
    pub member_id: String,
}

/// Explicit operator request for one Runner registration and control
/// Capability. Principal and Runner authority records are provisioned as
/// durable, idempotent operational changes before the Capability is issued.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerCapabilityIssueRequest {
    pub runner_id: String,
    pub owner_principal_id: String,
    pub permitted_memberships: Vec<RunnerMembershipTarget>,
    pub scopes: Vec<CapabilityScopeV1>,
    pub principal_idempotency_key: String,
    pub runner_idempotency_key: String,
    pub capability_idempotency_key: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// One-time plaintext delivery of a newly registered Runner Capability.
#[derive(Clone, Deserialize, Serialize)]
pub struct RunnerCapabilityIssueResponse {
    pub capability_id: String,
    pub runner_id: String,
    pub owner_principal_id: String,
    pub permitted_memberships: Vec<RunnerMembershipTarget>,
    pub scopes: CapabilityScopeSetV1,
    pub bearer: String,
}

impl std::fmt::Debug for RunnerCapabilityIssueResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerCapabilityIssueResponse")
            .field("capability_id", &self.capability_id)
            .field("runner_id", &self.runner_id)
            .field("owner_principal_id", &self.owner_principal_id)
            .field("permitted_memberships", &self.permitted_memberships)
            .field("scopes", &self.scopes)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

/// Immutable authenticated context for one gateway request or WebSocket.
///
/// The session identity is intentionally separate from the capability bearer.
/// Backends must bind synchronization-token issuance and acknowledgement to
/// [`Self::session_id`]; possession of the same bearer on another connection
/// is not sufficient to consume a token.
pub struct GatewaySession {
    session_id: UlidString,
    bearer: CapabilityBearerV1,
    bearer_wire: Option<BearerWireV1>,
}

const BROWSER_TICKET_PREFIX: &str = "wst1:";
const BROWSER_TICKET_BYTES: usize = 32;
const BROWSER_TICKET_WIRE_LENGTH: usize = BROWSER_TICKET_PREFIX.len() + BROWSER_TICKET_BYTES * 2;
const BROWSER_TICKET_TTL: Duration = Duration::from_secs(15);
const BROWSER_TICKET_TTL_MS: u64 = 15_000;
const BROWSER_ADMISSION_CLOSE_REASON: &str = "browser authorization failed";
const BROWSER_TICKET_CAPACITY: usize = 256;
const FIRST_CLIENT_HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const POST_WELCOME_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const OPERATOR_ROOM_CREATE: &str = "room-create";
const OPERATOR_MEMBER_CAPABILITY: &str = "member-capability-issue";
const OPERATOR_RUNNER_CAPABILITY: &str = "runner-capability-issue";

fn provisioned_scopes(values: Vec<String>) -> Result<CapabilityScopeSetV1, BackendError> {
    let scopes = values
        .into_iter()
        .map(|value| match value.as_str() {
            "room:attach" => Ok(CapabilityScopeV1::RoomAttach),
            "room:act" => Ok(CapabilityScopeV1::RoomAct),
            "room:observe_public" => Ok(CapabilityScopeV1::RoomObservePublic),
            "room:observe_member" => Ok(CapabilityScopeV1::RoomObserveMember),
            "room:replay" => Ok(CapabilityScopeV1::RoomReplay),
            "activation:offer_receive" => Ok(CapabilityScopeV1::ActivationOfferReceive),
            "activation:claim" => Ok(CapabilityScopeV1::ActivationClaim),
            "activation:complete" => Ok(CapabilityScopeV1::ActivationComplete),
            _ => Err(BackendError::Rejected),
        })
        .collect::<Result<Vec<_>, _>>()?;
    CapabilityScopeSetV1::new(scopes).map_err(|_| BackendError::Rejected)
}

/// The Role is a property of participant access only. A roleless operator or
/// spectator request must stay roleless all the way through provision retry.
pub(crate) fn member_capability_access_role_valid(
    access_mode: AccessMode,
    role: Option<&str>,
) -> bool {
    match access_mode {
        AccessMode::Participant => role.is_some_and(|value| !value.is_empty()),
        AccessMode::Spectator | AccessMode::Operator => role.is_none(),
    }
}

fn scope_names(scopes: &CapabilityScopeSetV1) -> Vec<String> {
    scopes
        .iter()
        .map(|scope| match scope {
            CapabilityScopeV1::RoomAttach => "room:attach",
            CapabilityScopeV1::RoomAct => "room:act",
            CapabilityScopeV1::RoomObservePublic => "room:observe_public",
            CapabilityScopeV1::RoomObserveMember => "room:observe_member",
            CapabilityScopeV1::RoomReplay => "room:replay",
            CapabilityScopeV1::ActivationOfferReceive => "activation:offer_receive",
            CapabilityScopeV1::ActivationClaim => "activation:claim",
            CapabilityScopeV1::ActivationComplete => "activation:complete",
            CapabilityScopeV1::OperatorRoomAdmin | CapabilityScopeV1::OperatorBackup => {
                unreachable!("sealed setup provisioning never accepts operator scopes")
            }
        })
        .map(str::to_owned)
        .collect()
}

pub(crate) fn runner_hello_is_bounded(request: &RunnerHello) -> bool {
    if request.maximum_concurrent_activations == 0
        || request.maximum_concurrent_activations > 64
        || request.supported_pack_ids.len() > 64
        || request.supported_pack_revisions.len() > 64
        || request
            .supported_pack_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
    {
        return false;
    }
    let mut exact = HashSet::new();
    request.supported_pack_revisions.iter().all(|pack| {
        !pack.id.is_empty()
            && pack.id.len() <= 256
            && !pack.version.is_empty()
            && pack.version.len() <= 64
            && pack
                .digest
                .parse::<worldstream_core::PackDigestV1>()
                .is_ok()
            && exact.insert((
                pack.id.as_str(),
                pack.version.as_str(),
                pack.digest.as_str(),
            ))
    })
}
const OPERATOR_TIMER_FIRE: &str = "timer-fire";
const OPERATOR_LOBBY_LAUNCH: &str = "lobby-launch";
const OPERATOR_ACTIVITY_PACK_CATALOG: &str = "activity-pack-catalog";
const OPERATOR_ROOM_INVENTORY: &str = "room-inventory";
const OPERATOR_ROOM_DETAIL: &str = "room-detail";
const OPERATOR_ACTIVATION_STATUS: &str = "activation-status";
const OPERATOR_BACKUP_PROFILE: &str = "backup-profile";
const OPERATOR_LIVE_BACKUP: &str = "live-backup";
const OPERATOR_RUNNER_PRESENCE: &str = "runner-presence";
const RUNNER_PRESENCE_VERSION: &str = "worldstream/operator-runner-presence/v1";
const RUNNER_PRESENCE_STALE_AFTER: Duration = Duration::from_secs(45);

/// Fills a caller-owned buffer from the platform's operating-system CSPRNG.
///
/// The error remains at the caller's typed fail-closed boundary: ticket
/// issuance maps it to a closed HTTP error, while the `SQLite` adapter maps it
/// to `BackendError::StorageUnavailable`.
pub(crate) fn fill_random_bytes(bytes: &mut [u8]) -> Result<(), getrandom::Error> {
    getrandom::fill(bytes)
}

#[derive(Debug, Eq, PartialEq)]
enum BrowserTicketError {
    Capacity,
    RandomnessUnavailable,
}

struct BrowserTicketEntry {
    origin: String,
    expires_at: std::time::Instant,
    session: GatewaySession,
}

/// In-memory, origin-bound admission tickets for native browser `WebSockets`.
///
/// Only a digest of the ticket is retained. The raw ticket is returned once by
/// the HTTP issuance response and is consumed by the first WebSocket frame.
/// This store is intentionally owned by [`OperatorState`] and is never backed
/// by a database, file, cache, or process-restart mechanism.
struct BrowserTicketStore {
    entries: Mutex<HashMap<[u8; BROWSER_TICKET_BYTES], BrowserTicketEntry>>,
    capacity: usize,
    ttl: Duration,
}

impl BrowserTicketStore {
    fn new() -> Self {
        Self::with_limits(BROWSER_TICKET_CAPACITY, BROWSER_TICKET_TTL)
    }

    fn with_limits(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            ttl,
        }
    }

    fn issue(&self, session: GatewaySession, origin: String) -> Result<String, BrowserTicketError> {
        self.issue_at(session, origin, std::time::Instant::now())
    }

    fn issue_at(
        &self,
        session: GatewaySession,
        origin: String,
        now: std::time::Instant,
    ) -> Result<String, BrowserTicketError> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.expires_at > now);
        if entries.len() >= self.capacity {
            return Err(BrowserTicketError::Capacity);
        }

        let mut bytes = [0_u8; BROWSER_TICKET_BYTES];
        fill_random_bytes(&mut bytes).map_err(|_| BrowserTicketError::RandomnessUnavailable)?;
        let ticket = browser_ticket_wire(bytes);
        let digest = *Blake3DigestV1::hash(ticket.as_bytes()).as_bytes();
        if entries.contains_key(&digest) {
            return Err(BrowserTicketError::RandomnessUnavailable);
        }
        entries.insert(
            digest,
            BrowserTicketEntry {
                origin,
                expires_at: now + self.ttl,
                session,
            },
        );
        Ok(ticket)
    }

    fn consume(&self, ticket: &str, origin: &str) -> Option<GatewaySession> {
        self.consume_at(ticket, origin, std::time::Instant::now())
    }

    fn consume_at(
        &self,
        ticket: &str,
        origin: &str,
        now: std::time::Instant,
    ) -> Option<GatewaySession> {
        if ticket.len() != BROWSER_TICKET_WIRE_LENGTH
            || !ticket.starts_with(BROWSER_TICKET_PREFIX)
            || !ticket.as_bytes()[BROWSER_TICKET_PREFIX.len()..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return None;
        }
        let digest = *Blake3DigestV1::hash(ticket.as_bytes()).as_bytes();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.remove(&digest)?;
        if entry.expires_at <= now || entry.origin != origin {
            return None;
        }
        Some(entry.session)
    }
}

fn browser_ticket_wire(bytes: [u8; BROWSER_TICKET_BYTES]) -> String {
    let mut wire = String::with_capacity(BROWSER_TICKET_WIRE_LENGTH);
    wire.push_str(BROWSER_TICKET_PREFIX);
    for byte in bytes {
        let _ = write!(wire, "{byte:02x}");
    }
    wire
}

impl GatewaySession {
    #[cfg(test)]
    fn new(session_id: UlidString, bearer: CapabilityBearerV1) -> Self {
        Self {
            session_id,
            bearer,
            bearer_wire: None,
        }
    }

    fn new_with_wire(
        session_id: UlidString,
        bearer: CapabilityBearerV1,
        bearer_wire: BearerWireV1,
    ) -> Self {
        Self {
            session_id,
            bearer,
            bearer_wire: Some(bearer_wire),
        }
    }

    /// Reconstructs an owned bearer only at the storage authority boundary.
    /// The transport wire representation remains redacted and is never
    /// formatted or returned to callers.
    pub(crate) fn owned_bearer(&self) -> Option<CapabilityBearerV1> {
        let wire = self.bearer_wire.as_ref()?.to_wire();
        let wire = BearerWireV1::parse(&wire).ok()?;
        Some(CapabilityBearerV1::from_bytes(wire.into_bytes()))
    }

    /// Returns the canonical identity for this authenticated transport session.
    #[must_use]
    pub const fn session_id(&self) -> &UlidString {
        &self.session_id
    }

    /// Returns the validated Core bearer for authority lookup.
    #[must_use]
    pub const fn bearer(&self) -> &CapabilityBearerV1 {
        &self.bearer
    }
}

impl std::fmt::Debug for GatewaySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewaySession")
            .field("session_id", &self.session_id)
            .field("bearer", &"[REDACTED]")
            .field("bearer_wire", &"[REDACTED]")
            .finish()
    }
}

/// Public operations required by the gateway. This is intentionally expressed
/// in protocol values rather than Core, authority, or `SQLite` types.
///
/// The transport contract deliberately does not synthesize Core Genesis
/// inputs or claim storage authority. The concrete `SQLite` and `PostgreSQL`
/// adapters provide those Core-owned seams; [`UnavailableBackend`] remains the
/// fail-closed default when no verified storage adapter has been configured.
pub trait GatewayBackend: Send + Sync + 'static {
    /// Returns aggregate bounded Room Admission Lane metrics without Room IDs.
    #[must_use]
    fn room_admission_queue_snapshot(&self) -> RoomAdmissionQueueSnapshotV1 {
        RoomAdmissionQueueSnapshotV1::default()
    }

    /// Resolves the authenticated principal used by transport rate limiting.
    ///
    /// Implementations must authenticate through the same durable authority
    /// seam as ordinary operations. The returned identifier remains inside
    /// the gateway and is immediately reduced to a process-keyed fingerprint.
    ///
    /// # Errors
    ///
    /// Returns a closed error when authentication or authority storage is
    /// unavailable. Callers must reject the operation before semantic work.
    fn admission_principal(&self, session: &GatewaySession) -> Result<String, BackendError>;

    /// Lists exact Activity Pack revisions compiled into this runtime.
    ///
    /// # Errors
    ///
    /// Returns a closed error unless the caller has host-operator authority or
    /// the validated embedded registry is unavailable.
    fn activity_pack_catalog(
        &self,
        _session: &GatewaySession,
    ) -> Result<ActivityPackCatalogResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Reads one exact installed Activity Pack revision by semantic digest.
    ///
    /// # Errors
    ///
    /// Returns an explicit unavailable result for malformed or unknown
    /// digests. Implementations must not fall back by pack ID or version.
    fn activity_pack_revision(
        &self,
        _session: &GatewaySession,
        _revision_digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Authorizes a host diagnostic read of bounded Runner presence.
    ///
    /// # Errors
    ///
    /// Returns a closed error unless the caller has deployment-level
    /// host-operator diagnostic authority.
    fn authorize_operator_runner_presence(
        &self,
        _session: &GatewaySession,
    ) -> Result<(), BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Authenticates a new Room-client handshake.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when authentication or storage is unavailable.
    fn hello(
        &self,
        session: &GatewaySession,
        hello: &ClientHello,
    ) -> Result<ServerWelcome, BackendError>;
    /// Creates one durably committed Room.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when the request is unauthorized, conflicting, or cannot be persisted.
    fn create_room(
        &self,
        session: &GatewaySession,
        request: CreateRoomRequest,
    ) -> Result<CreateRoomResponse, BackendError>;
    /// Authenticates and records one fixed host Lobby launch `ExternalInput`.
    ///
    /// # Errors
    ///
    /// Returns a closed error for invalid authority, phase, identity reuse, or persistence.
    fn launch_lobby(
        &self,
        _session: &GatewaySession,
        _room_id: &str,
        _request: LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    /// Reads the caller-authorized current Projection.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when authorization, Room state, or storage is unavailable.
    fn projection(
        &self,
        session: &GatewaySession,
        room_id: &str,
    ) -> Result<ProjectionResponse, BackendError>;
    /// Reads one exact historical Projection under the caller's present
    /// member capability and the storage adapter's replay fences.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when authorization, historical
    /// membership, integrity, replay, or storage verification fails.
    fn replay(
        &self,
        _session: &GatewaySession,
        _room_id: &str,
        _at_room_seq: u64,
    ) -> Result<ReplayResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    /// Captures a complete attach barrier and retained/reset branch.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when the Membership cannot attach or the barrier is unavailable.
    fn attach(
        &self,
        session: &GatewaySession,
        request: RoomAttach,
    ) -> Result<AttachReply, BackendError>;
    /// Consumes the Session-bound synchronization acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when the token or storage state is invalid.
    fn sync_ack(
        &self,
        session: &GatewaySession,
        request: RoomSyncAck,
    ) -> Result<Vec<ObservationDeliver>, BackendError>;
    /// Reads authorized frames after one connection-local delivered sequence.
    /// This does not consume a synchronization binding or advance the shared
    /// Membership Cursor.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when authorization, observation
    /// integrity, or storage is unavailable.
    fn live_observation_suffix(
        &self,
        session: &GatewaySession,
        room_id: &str,
        member_id: &str,
        after_frame_seq: u64,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        let _ = (session, room_id, member_id, after_frame_seq);
        Err(BackendError::StorageUnavailable)
    }
    /// Advances the shared Membership Observation Cursor.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] when the acknowledgement is unauthorized or invalid.
    fn observation_ack(
        &self,
        session: &GatewaySession,
        request: ObservationAck,
    ) -> Result<Option<u64>, BackendError>;
    /// Submits one already strictly parsed Action to the Room supervisor.
    ///
    /// # Errors
    ///
    /// Returns a closed [`BackendError`] for transient, authority, conflict, or storage failures.
    fn action(
        &self,
        session: &GatewaySession,
        request: ActionSubmit,
    ) -> Result<ActionReply, BackendError>;

    /// Consumes one exact due Timer generation under HostOperator/Room-root
    /// authority and the ordinary Core/SQLite commit fences.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when authentication, authority, due
    /// state, recovery, or durable commit verification fails.
    fn fire_timer(
        &self,
        _session: &GatewaySession,
        _room_id: &str,
        _request: TimerFireRequest,
    ) -> Result<TimerFireResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Lists the bounded host-authorized Room inventory in stable Room-ID
    /// order.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, bounds, storage, or canonical-identity
    /// failure.
    fn operator_room_inventory(
        &self,
        _session: &GatewaySession,
        _request: OperatorRoomInventoryRequest,
    ) -> Result<OperatorRoomInventoryPage, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Reads one bounded host-authorized Room detail by stable Room identity.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, unavailable-Room, storage, or
    /// canonical-identity failure.
    fn operator_room_detail(
        &self,
        _session: &GatewaySession,
        _room_id: &str,
    ) -> Result<OperatorRoomSummary, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Reads bounded durable Activation state counts for one exact Room
    /// Membership under host diagnostic authority.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, unavailable target, storage, overflow, or
    /// canonical-identity failure without returning Activation records.
    fn operator_activation_status(
        &self,
        _session: &GatewaySession,
        _room_id: &str,
        _member_id: &str,
    ) -> Result<OperatorActivationStatusV1, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Reports the configured storage profile's live-backup health without
    /// starting an operation.
    ///
    /// # Errors
    ///
    /// Returns a closed authority or storage failure.
    fn operator_backup_profile(
        &self,
        _session: &GatewaySession,
    ) -> Result<OperatorBackupProfileStatus, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Creates or reconciles one stable verified live-backup preparation.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, destination, verification, or storage
    /// failure without changing the running source.
    fn operator_live_backup(
        &self,
        _session: &GatewaySession,
        _request: OperatorLiveBackupPrepareRequest,
    ) -> Result<OperatorLiveBackupStatus, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Issues one operator-authorized, Room-member-bound Capability.
    ///
    /// Implementations must return the bearer only on successful creation;
    /// the durable authority record contains its hash, never plaintext.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when authentication, target binding,
    /// authority mutation, or storage fails.
    fn issue_member_capability(
        &self,
        _session: &GatewaySession,
        _request: MemberCapabilityIssueRequest,
    ) -> Result<MemberCapabilityIssueResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Provisions one Runner identity and its bounded control Capability.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the operator request, target
    /// Memberships, authority changes, or Capability registration fails.
    fn issue_runner_capability(
        &self,
        _session: &GatewaySession,
        _request: RunnerCapabilityIssueRequest,
    ) -> Result<RunnerCapabilityIssueResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Registers one caller-sealed member Capability idempotently.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the exact sealed input conflicts,
    /// its Membership binding is invalid, or durable authority is unavailable.
    fn provision_member_capability(
        &self,
        _session: &GatewaySession,
        _request: MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Registers one caller-sealed Runner-control Capability idempotently.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when any exact identity, Membership
    /// binding, or authority transition is invalid or unavailable.
    fn provision_runner_capability(
        &self,
        _session: &GatewaySession,
        _request: RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Validates a Runner control handshake against the authenticated
    /// capability binding. No private Room data is released here.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the Runner handshake or its
    /// authenticated capability cannot be validated.
    fn runner_hello(
        &self,
        _session: &GatewaySession,
        _request: RunnerHello,
    ) -> Result<RunnerReady, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Reads durable pending Activation offers for one authorized target.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the target is unauthorized or the
    /// durable offer receipt cannot be read safely.
    fn activation_offers(
        &self,
        _session: &GatewaySession,
        _request: ActivationOfferRequest,
    ) -> Result<ActivationOffers, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Claims one durable Activation and returns its exact private context
    /// only after the backend's authority and writer fences succeed.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the claim is unauthorized, stale,
    /// conflicting, or cannot be durably committed.
    fn activation_claim(
        &self,
        _session: &GatewaySession,
        _request: ActivationClaim,
    ) -> Result<ActivationOperationReply, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Renews one currently leased Activation under its expected generation.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the lease is unauthorized, stale,
    /// conflicting, or cannot be durably committed.
    fn activation_renew(
        &self,
        _session: &GatewaySession,
        _request: ActivationLeaseOperation,
    ) -> Result<ActivationOperationReply, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Releases one currently leased Activation back to the pending queue.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the lease is unauthorized, stale,
    /// conflicting, or cannot be durably committed.
    fn activation_release(
        &self,
        _session: &GatewaySession,
        _request: ActivationLeaseOperation,
    ) -> Result<ActivationOperationReply, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Completes one currently leased Activation with a bounded disposition.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the lease is unauthorized, stale,
    /// conflicting, or cannot be durably committed.
    fn activation_complete(
        &self,
        _session: &GatewaySession,
        _request: ActivationLeaseOperation,
    ) -> Result<ActivationOperationReply, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// One bounded scheduler tick: maintain leases and, where supported, commit
    /// due timers through existing authority and Core paths. Returns Rooms
    /// requiring post-commit live publication.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when durable lease maintenance cannot be
    /// completed safely.
    fn scheduler_tick(&self) -> Result<Vec<String>, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    /// Retires connection-local synchronization state after a transport
    /// session closes. Implementations must keep this in-memory and
    /// non-blocking because the transport invokes it during teardown.
    fn retire_session(&self, _session_id: &UlidString) {}
}

/// Runs one synchronous backend operation away from Tokio worker threads.
///
pub(crate) fn activity_pack_catalog_from_registry(
    registry: &worldstream_core::PackRegistryV1,
) -> ActivityPackCatalogResponse {
    ActivityPackCatalogResponse {
        version: ACTIVITY_PACK_CATALOG_VERSION.to_owned(),
        revisions: registry
            .catalog_revisions()
            .map(|revision| ActivityPackCatalogRevisionSummary {
                pack: PackReference {
                    id: revision.descriptor.pack_id,
                    version: revision.descriptor.explanatory_version,
                    digest: revision.revision_digest.to_string(),
                },
                name: revision.descriptor.name,
                selectable_for_new_rooms: revision.selectable_for_new_rooms,
                runnable_for_retained_rooms: revision.runnable_for_retained_rooms,
            })
            .collect(),
    }
}

pub(crate) fn activity_pack_revision_from_registry(
    registry: &worldstream_core::PackRegistryV1,
    revision_digest: &str,
) -> Result<ActivityPackCatalogRevisionResponse, BackendError> {
    let revision_digest = revision_digest
        .parse::<worldstream_core::PackDigestV1>()
        .map_err(|_| BackendError::ActivityPackRevisionUnavailable)?;
    let revision = registry
        .catalog_revision(&revision_digest)
        .map_err(|error| match error {
            worldstream_core::PackRegistryErrorV1::MissingRevision(_) => {
                BackendError::ActivityPackRevisionUnavailable
            }
            _ => BackendError::InvalidResult,
        })?;
    let descriptor = &revision.descriptor;
    let configuration_schema = activity_pack_schema_from_registry(
        registry,
        &revision_digest,
        &descriptor.configuration_schema,
    )?;
    let actions = descriptor
        .actions
        .iter()
        .map(|action| {
            Ok(ActivityPackCatalogAction {
                action_type: action.action_type.clone(),
                payload_schema: activity_pack_schema_from_registry(
                    registry,
                    &revision_digest,
                    &action.payload_schema,
                )?,
            })
        })
        .collect::<Result<Vec<_>, BackendError>>()?;
    let summary = ActivityPackCatalogRevisionSummary {
        pack: PackReference {
            id: descriptor.pack_id.clone(),
            version: descriptor.explanatory_version.clone(),
            digest: revision_digest.to_string(),
        },
        name: descriptor.name.clone(),
        selectable_for_new_rooms: revision.selectable_for_new_rooms,
        runnable_for_retained_rooms: revision.runnable_for_retained_rooms,
    };
    let lobby_compatibility = descriptor
        .stimulus_schemas
        .get(worldstream_core::HOST_LAUNCH_INPUT_TYPE)
        .map(|reference| {
            let launch_schema =
                activity_pack_schema_from_registry(registry, &revision_digest, reference)?;
            if launch_schema.schema
                != json!({
                    "additionalProperties": false,
                    "type": "object",
                })
            {
                return Err(BackendError::InvalidResult);
            }
            Ok(ActivityPackLobbyCompatibility {
                contract: worldstream_core::AGENT_HEIST_LOBBY_CONTRACT.to_owned(),
                configuration_schema: configuration_schema.clone(),
            })
        })
        .transpose()?;
    Ok(ActivityPackCatalogRevisionResponse {
        version: ACTIVITY_PACK_CATALOG_VERSION.to_owned(),
        revision: ActivityPackCatalogRevisionDetail {
            summary,
            roles: descriptor
                .roles
                .iter()
                .map(|role| ActivityPackCatalogRole {
                    role: role.role.clone(),
                    minimum: role.minimum,
                    maximum: role.maximum,
                })
                .collect(),
            configuration_schema,
            actions,
            lobby_compatibility,
        },
    })
}

fn activity_pack_schema_from_registry(
    registry: &worldstream_core::PackRegistryV1,
    revision_digest: &worldstream_core::PackDigestV1,
    reference: &worldstream_core::SchemaReferenceV1,
) -> Result<ActivityPackCatalogSchema, BackendError> {
    let schema = registry
        .resolve_schema(revision_digest, reference)
        .map_err(|_| BackendError::InvalidResult)?;
    let schema = serde_json::to_value(schema).map_err(|_| BackendError::InvalidResult)?;
    Ok(ActivityPackCatalogSchema {
        schema_id: reference.schema_id.clone(),
        schema_digest: reference.schema_digest.to_string(),
        schema,
    })
}

/// `GatewayBackend` is intentionally provider-neutral and synchronous, while
/// `PostgreSQL`'s client is a synchronous client. Every HTTP/WebSocket caller
/// must cross this boundary before invoking the backend so a provider cannot
/// attempt to start or block a runtime from inside an async handler.
async fn backend_call<T, F>(backend: Arc<dyn GatewayBackend>, call: F) -> Result<T, BackendError>
where
    T: Send + 'static,
    F: FnOnce(&dyn GatewayBackend) -> Result<T, BackendError> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || call(backend.as_ref())).await {
        Ok(result) => result,
        Err(_) => Err(BackendError::StorageUnavailable),
    }
}

async fn admit_authenticated_http(
    state: &OperatorState,
    session: &Arc<GatewaySession>,
    targets: &[AdmissionTarget<'_>],
    operator_endpoint: Option<&str>,
    correlation: telemetry::CorrelationV1,
) -> Result<(), ResponseError> {
    let principal_id = match resolve_admission_principal(state, session).await {
        Ok(principal_id) => principal_id,
        Err(error) => {
            let reason = if error.envelope.error.code == ErrorCode::Forbidden {
                telemetry::ReasonCodeV1::Unauthorized
            } else {
                telemetry::ReasonCodeV1::StorageUnavailable
            };
            record_admission_with_correlation(state.telemetry.as_ref(), reason, correlation);
            return Err(error);
        }
    };
    let capability_hash = session.bearer().token_hash();
    state
        .rate_limiter
        .admit(GatewayAdmission {
            principal_id: Some(&principal_id),
            capability_material: Some(capability_hash.storage_bytes()),
            operator_endpoint,
            targets,
            ..GatewayAdmission::default()
        })
        .map_err(|_| rate_limited_response())
}

async fn resolve_admission_principal(
    state: &OperatorState,
    session: &Arc<GatewaySession>,
) -> Result<String, ResponseError> {
    let authentication_session = Arc::clone(session);
    backend_call(Arc::clone(&state.backend), move |backend| {
        backend.admission_principal(&authentication_session)
    })
    .await
    .map_err(ResponseError::from)
}

/// Backend used by the process shell until a verified storage-backed
/// supervisor is installed. It makes no fake room or action success.
#[derive(Debug, Default)]
pub struct UnavailableBackend;

impl GatewayBackend for UnavailableBackend {
    fn admission_principal(&self, _: &GatewaySession) -> Result<String, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    fn hello(&self, _: &GatewaySession, _: &ClientHello) -> Result<ServerWelcome, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn create_room(
        &self,
        _: &GatewaySession,
        _: CreateRoomRequest,
    ) -> Result<CreateRoomResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn projection(&self, _: &GatewaySession, _: &str) -> Result<ProjectionResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn attach(&self, _: &GatewaySession, _: RoomAttach) -> Result<AttachReply, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn sync_ack(
        &self,
        _: &GatewaySession,
        _: RoomSyncAck,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn live_observation_suffix(
        &self,
        _: &GatewaySession,
        _: &str,
        _: &str,
        _: u64,
    ) -> Result<Vec<ObservationDeliver>, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn observation_ack(
        &self,
        _: &GatewaySession,
        _: ObservationAck,
    ) -> Result<Option<u64>, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
    fn action(&self, _: &GatewaySession, _: ActionSubmit) -> Result<ActionReply, BackendError> {
        Err(BackendError::StorageUnavailable)
    }

    fn fire_timer(
        &self,
        _: &GatewaySession,
        _: &str,
        _: TimerFireRequest,
    ) -> Result<TimerFireResponse, BackendError> {
        Err(BackendError::StorageUnavailable)
    }
}

/// Immutable state for the operator-only process shell.
#[derive(Clone)]
pub struct OperatorState {
    config: EffectiveConfig,
    compatibility: CompatibilitySummary,
    backend: Arc<dyn GatewayBackend>,
    browser_tickets: Arc<BrowserTicketStore>,
    telemetry: Option<telemetry::TelemetryHandle>,
    telemetry_runtime: Option<Arc<TelemetryRuntimeOwner>>,
    scheduler_runtime: Option<Arc<SchedulerRuntimeOwner>>,
    readiness: RuntimeReadiness,
    engine: EngineVersion,
    live_streams: LiveStreamRegistry,
    runner_presence: RunnerPresenceRegistry,
    rate_limiter: GatewayRateLimiter,
}

/// Owns the process scheduler loop and publication of its committed timers.
/// Activity transitions remain authorized Core commit concerns.
struct SchedulerRuntimeOwner {
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl SchedulerRuntimeOwner {
    fn start(
        backend: Arc<dyn GatewayBackend>,
        live_streams: LiveStreamRegistry,
    ) -> Result<Arc<Self>, BackendError> {
        let initial_rooms = backend.scheduler_tick()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| BackendError::StorageUnavailable)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("worldstream-activation-scheduler".to_owned())
            .spawn(move || {
                runtime.block_on(async {
                    for room in initial_rooms {
                        publish_live_frames(Arc::clone(&backend), &live_streams, &room).await;
                    }
                });
                while !thread_stop.load(Ordering::Acquire) {
                    match backend.scheduler_tick() {
                        Ok(rooms) => runtime.block_on(async {
                            for room in rooms {
                                publish_live_frames(Arc::clone(&backend), &live_streams, &room)
                                    .await;
                            }
                        }),
                        Err(error) => tracing::warn!(?error, "Runtime scheduler tick failed"),
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .map_err(|_| BackendError::StorageUnavailable)?;
        Ok(Arc::new(Self {
            stop,
            thread: Mutex::new(Some(thread)),
        }))
    }
}

impl Drop for SchedulerRuntimeOwner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let thread = self
            .thread
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Process-level facts used by the operator readiness probe.
///
/// The daemon only marks schema, durable storage, and writer facts after the
/// startup-selected adapter has completed its provider-specific verification,
/// marks authority facts only after durable bootstrap completes, and marks
/// scheduler readiness only after the first durable lease-maintenance tick
/// succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RuntimeReadiness {
    schema_verified: bool,
    storage_writable: bool,
    writer_running: bool,
    scheduler_running: bool,
    authority_bootstrapped: bool,
}

impl RuntimeReadiness {
    /// Facts before a selected storage runtime has been opened.
    #[must_use]
    pub const fn not_initialized() -> Self {
        Self {
            schema_verified: false,
            storage_writable: false,
            writer_running: false,
            scheduler_running: false,
            authority_bootstrapped: false,
        }
    }

    /// Facts established by a successful durable-store startup probe.
    ///
    /// This intentionally does not claim authority bootstrap or scheduler
    /// readiness; those facts are added by the daemon after their own probes.
    #[must_use]
    pub const fn durable_store_verified() -> Self {
        Self {
            schema_verified: true,
            storage_writable: true,
            writer_running: true,
            scheduler_running: false,
            authority_bootstrapped: false,
        }
    }

    /// Compatibility spelling for the bundled `SQLite` startup path.
    #[must_use]
    pub const fn sqlite_store_verified() -> Self {
        Self::durable_store_verified()
    }

    /// Records that the durable generation-one host authority was installed or
    /// exactly replayed during startup. Scheduler state is intentionally
    /// unchanged.
    #[must_use]
    pub const fn with_authority_bootstrapped(mut self) -> Self {
        self.authority_bootstrapped = true;
        self
    }

    /// Records that a scheduler runtime owner has actually started.
    #[must_use]
    pub const fn with_scheduler_running(mut self) -> Self {
        self.scheduler_running = true;
        self
    }

    /// Deterministic fixture for exercising the HTTP ready branch in parent
    /// tests. It is not used by the daemon and must never be used as runtime
    /// evidence or to manufacture authority.
    #[cfg(test)]
    #[must_use]
    pub const fn ready_for_tests() -> Self {
        Self {
            schema_verified: true,
            storage_writable: true,
            writer_running: true,
            scheduler_running: true,
            authority_bootstrapped: true,
        }
    }

    fn probe(self) -> ReadinessProbeResult {
        let checks = ReadinessChecks {
            schema: self.schema_verified,
            storage: self.storage_writable,
            writer: self.writer_running,
            scheduler: self.scheduler_running,
            authority_bootstrap: self.authority_bootstrapped,
        };
        if !self.schema_verified {
            return ReadinessProbeResult::NotReady {
                code: ErrorCode::StorageNotInitialized,
                message: "durable storage schema has not been verified; inspect startup storage errors and restart after correcting the data directory".to_owned(),
                retryable: true,
                checks,
            };
        }
        if !self.storage_writable {
            return ReadinessProbeResult::NotReady {
                code: ErrorCode::StorageUnavailable,
                message: "durable storage is not writable; correct the configured data directory or filesystem permissions".to_owned(),
                retryable: true,
                checks,
            };
        }
        if !self.writer_running {
            return ReadinessProbeResult::NotReady {
                code: ErrorCode::StorageUnavailable,
                message: "the durable storage writer is not running; restart the daemon after resolving the writer failure".to_owned(),
                retryable: true,
                checks,
            };
        }
        if !self.scheduler_running {
            let message = if self.authority_bootstrapped {
                "the configured runtime has no running scheduler; readiness cannot be claimed until the scheduler is started"
            } else {
                "the configured runtime has no running scheduler and operator authority is not bootstrapped; the current configuration contract provides neither prerequisite for readiness"
            };
            return ReadinessProbeResult::NotReady {
                code: ErrorCode::StorageNotInitialized,
                message: message.to_owned(),
                retryable: false,
                checks,
            };
        }
        if !self.authority_bootstrapped {
            return ReadinessProbeResult::NotReady {
                code: ErrorCode::StorageNotInitialized,
                message: "operator authority is not bootstrapped; provide an owner-readable bootstrap secret through the supported configuration contract".to_owned(),
                retryable: false,
                checks,
            };
        }
        ReadinessProbeResult::Ready
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct ReadinessChecks {
    schema: bool,
    storage: bool,
    writer: bool,
    scheduler: bool,
    authority_bootstrap: bool,
}

#[derive(Debug, Eq, PartialEq)]
enum ReadinessProbeResult {
    Ready,
    NotReady {
        code: ErrorCode,
        message: String,
        retryable: bool,
        checks: ReadinessChecks,
    },
}

impl OperatorState {
    /// Builds state from validated config and the embedded manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if the embedded manifest is invalid or its product
    /// version differs from this binary package.
    pub fn new(config: EffectiveConfig) -> Result<Self, ServerError> {
        let compatibility = embedded_manifest()?.summary();
        if compatibility.contracts.product != env!("CARGO_PKG_VERSION") {
            return Err(ServerError::BuildVersionMismatch {
                package: env!("CARGO_PKG_VERSION"),
                manifest: compatibility.contracts.product.clone(),
            });
        }
        let profile = config.storage.profile;
        Ok(Self {
            config,
            compatibility,
            backend: Arc::new(UnavailableBackend),
            browser_tickets: Arc::new(BrowserTicketStore::new()),
            telemetry: None,
            telemetry_runtime: None,
            scheduler_runtime: None,
            readiness: RuntimeReadiness::not_initialized(),
            engine: EngineVersion::not_initialized(profile),
            live_streams: LiveStreamRegistry::default(),
            runner_presence: RunnerPresenceRegistry::default(),
            rate_limiter: GatewayRateLimiter::new()
                .map_err(|_| ServerError::RateLimiterInitialization)?,
        })
    }

    /// Builds the gateway with an already-authorized, storage-backed Room
    /// supervisor. The supervisor remains responsible for Core semantics.
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn GatewayBackend>) -> Self {
        self.backend = backend;
        self
    }

    /// Installs an optional, already-started telemetry producer handle.
    ///
    /// The caller retains ownership of the corresponding
    /// [`telemetry::TelemetryRuntime`]. The default state has no telemetry
    /// producer, so existing callers remain a no-op. Every submission is
    /// best-effort and non-blocking; it cannot affect backend outcomes.
    #[must_use]
    pub fn with_telemetry(mut self, handle: telemetry::TelemetryHandle) -> Self {
        self.telemetry = Some(handle);
        self
    }

    /// Installs a runtime whose lifetime is tied to all router state clones.
    /// The last state owner performs the telemetry module's bounded shutdown
    /// flush. Exporter health remains diagnostic-only.
    #[must_use]
    pub fn with_telemetry_runtime(mut self, runtime: telemetry::TelemetryRuntime) -> Self {
        self.telemetry = Some(runtime.handle());
        self.telemetry_runtime = Some(Arc::new(TelemetryRuntimeOwner {
            runtime: Mutex::new(Some(runtime)),
        }));
        self
    }

    /// Starts the durable lease-maintenance scheduler and records readiness
    /// only after its first backend tick succeeds.
    ///
    /// # Errors
    ///
    /// Returns the backend failure from the startup tick; readiness remains
    /// fail-closed in that case.
    pub fn with_scheduler(mut self) -> Result<Self, BackendError> {
        let runtime =
            SchedulerRuntimeOwner::start(Arc::clone(&self.backend), self.live_streams.clone())?;
        self.scheduler_runtime = Some(runtime);
        self.readiness = self.readiness.with_scheduler_running();
        Ok(self)
    }

    /// Records facts established by a runtime-owned startup probe.
    #[must_use]
    pub fn with_readiness(mut self, readiness: RuntimeReadiness) -> Self {
        self.readiness = readiness;
        self
    }

    /// Records one exact provider engine identity after a successful startup
    /// probe. The caller must supply an adapter-authenticated identity; this
    /// method never infers or probes engine state itself.
    #[must_use]
    pub fn with_verified_engine(mut self, exact_identity: String) -> Self {
        self.engine = EngineVersion::verified(self.config.storage.profile, exact_identity);
        self
    }

    /// Records the exact bundled `SQLite` identity after a successful store open.
    #[must_use]
    pub fn with_verified_sqlite_engine(
        self,
        version: &'static str,
        source_id: &'static str,
    ) -> Self {
        self.with_verified_engine(format!("sqlite/{version}; source_id={source_id}"))
    }
}

/// Builds the complete HTTP surface for the operator runtime.
pub fn operator_router(state: OperatorState) -> Router {
    let middleware_state = state.clone();
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version))
        .route("/metrics", get(metrics))
        .route("/v1/operator/activity-packs", get(activity_pack_catalog))
        .route(
            "/v1/operator/activity-packs/{revision_digest}",
            get(activity_pack_revision),
        )
        .route("/v1/rooms", post(create_room))
        .route(
            "/v1/operator/member-capabilities",
            post(issue_member_capability),
        )
        .route(
            "/v1/operator/member-capabilities:provision",
            post(provision_member_capability),
        )
        .route(
            "/v1/operator/runner-capabilities",
            post(issue_runner_capability),
        )
        .route(
            "/v1/operator/runner-capabilities:provision",
            post(provision_runner_capability),
        )
        .route("/v1/operator/rooms", get(operator_room_inventory))
        .route("/v1/operator/rooms/{room_id}", get(operator_room_detail))
        .route(
            "/v1/operator/rooms/{room_id}/members/{member_id}/presence",
            get(operator_member_presence),
        )
        .route(
            "/v1/operator/rooms/{room_id}/members/{member_id}/activation-status",
            get(operator_activation_status),
        )
        .route(
            "/v1/operator/runners/{runner_id}/presence",
            get(operator_runner_presence),
        )
        .route("/v1/operator/backups/health", get(operator_backup_profile))
        .route("/v1/operator/backups", post(operator_live_backup))
        .route("/v1/operator/rooms/{room_id}/timers/fire", post(fire_timer))
        .route(
            "/v1/operator/rooms/{room_id}/lobby/launch",
            post(launch_lobby),
        )
        .route("/v1/rooms/{room_id}/projection", get(current_projection))
        .route("/v1/rooms/{room_id}/replay", get(historical_replay))
        .route(
            "/v1/stream/ticket",
            options(browser_ticket_preflight).post(issue_browser_ticket),
        )
        .route("/v1/stream", get(room_stream))
        .route("/v1/runner/stream", get(runner_stream))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            middleware_state,
            http_ip_admission,
        ))
}

async fn http_ip_admission(
    State(state): State<OperatorState>,
    request: Request,
    next: Next,
) -> Response {
    let peer = PeerIdentity::from_ip(
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip()),
    );
    if state
        .rate_limiter
        .admit(GatewayAdmission {
            peer: Some(peer),
            ..GatewayAdmission::default()
        })
        .is_err()
    {
        return rate_limited_response().into_response();
    }
    next.run(request).await
}

async fn healthz() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse { status: "ok" }))
}

async fn metrics(State(state): State<OperatorState>) -> impl IntoResponse {
    let mut body = state.telemetry.as_ref().map_or_else(
        || telemetry::TelemetryMetrics::default().prometheus_text(),
        telemetry::TelemetryHandle::prometheus_text,
    );
    let telemetry_queue = state.telemetry.as_ref().map_or_else(
        || telemetry::TelemetryMetrics::default().queue_snapshot(telemetry::DEFAULT_QUEUE_CAPACITY),
        telemetry::TelemetryHandle::queue_snapshot,
    );
    let admission_queue =
        InternalQueueSnapshot::room_admission(state.backend.room_admission_queue_snapshot());
    let [frame_queue, payload_queue] = state.live_streams.queue_snapshots();
    append_internal_queue_metrics(
        &mut body,
        &[
            telemetry_queue,
            telemetry::dns_resolver_queue_snapshot(),
            admission_queue,
            frame_queue,
            payload_queue,
        ],
    );
    body.push_str(&state.rate_limiter.prometheus_text());
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

fn append_internal_queue_metrics(body: &mut String, queues: &[InternalQueueSnapshot]) {
    body.push_str("# TYPE worldstream_internal_queue_capacity gauge\n");
    body.push_str("# TYPE worldstream_internal_queue_process_current gauge\n");
    body.push_str("# TYPE worldstream_internal_queue_process_high_water gauge\n");
    body.push_str("# TYPE worldstream_internal_queue_unit_high_water gauge\n");
    body.push_str("# TYPE worldstream_internal_queue_activity_total counter\n");
    body.push_str("# TYPE worldstream_internal_queue_completion_total counter\n");
    body.push_str("# TYPE worldstream_internal_queue_backpressure_total counter\n");
    for queue in queues {
        let _ = writeln!(
            body,
            "worldstream_internal_queue_capacity{{queue=\"{}\",scope=\"{}\"}} {}",
            queue.name, queue.capacity_scope, queue.capacity
        );
        let _ = writeln!(
            body,
            "worldstream_internal_queue_process_current{{queue=\"{}\"}} {}",
            queue.name, queue.process_current
        );
        let _ = writeln!(
            body,
            "worldstream_internal_queue_process_high_water{{queue=\"{}\"}} {}",
            queue.name, queue.process_high_water
        );
        let _ = writeln!(
            body,
            "worldstream_internal_queue_unit_high_water{{queue=\"{}\"}} {}",
            queue.name, queue.unit_high_water
        );
        let _ = writeln!(
            body,
            "worldstream_internal_queue_activity_total{{queue=\"{}\"}} {}",
            queue.name, queue.activity_total
        );
        let _ = writeln!(
            body,
            "worldstream_internal_queue_completion_total{{queue=\"{}\"}} {}",
            queue.name, queue.completion_total
        );
        let _ = writeln!(
            body,
            "worldstream_internal_queue_backpressure_total{{queue=\"{}\"}} {}",
            queue.name, queue.backpressure_total
        );
    }
}

async fn readyz(State(state): State<OperatorState>) -> axum::response::Response {
    match state.readiness.probe() {
        ReadinessProbeResult::Ready => {
            (StatusCode::OK, Json(ReadyResponse { status: "ready" })).into_response()
        }
        ReadinessProbeResult::NotReady {
            code,
            message,
            retryable,
            checks,
        } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    retryable,
                    details: Some(json!({ "checks": checks })),
                },
            }),
        )
            .into_response(),
    }
}

async fn version(State(state): State<OperatorState>) -> Json<VersionResponse> {
    let contracts = &state.compatibility.contracts;
    Json(VersionResponse {
        product_build: ProductBuild {
            product: contracts.product.clone(),
            binary: "worldstreamd",
            build_version: contracts.product.clone(),
            source_revision: BUILD_REVISION,
        },
        wire: contracts.wire.clone(),
        config: contracts.config,
        storage_schema: contracts.storage_schema,
        core_schema_version: contracts.core_schema_version.clone(),
        hash_suite: contracts.hash_suite.clone(),
        manifest: state.compatibility,
        engine: state.engine,
    })
}

async fn activity_pack_catalog(
    State(state): State<OperatorState>,
    headers: HeaderMap,
) -> ResponseResult<ActivityPackCatalogResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = Arc::new(authenticated_session(&headers)?);
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_ACTIVITY_PACK_CATALOG),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.activity_pack_catalog(&session)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

async fn activity_pack_revision(
    State(state): State<OperatorState>,
    Path(revision_digest): Path<String>,
    headers: HeaderMap,
) -> ResponseResult<ActivityPackCatalogRevisionResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = Arc::new(authenticated_session(&headers)?);
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_ACTIVITY_PACK_CATALOG),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.activity_pack_revision(&session, &revision_digest)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

async fn operator_runner_presence(
    State(state): State<OperatorState>,
    Path(runner_id): Path<String>,
    headers: HeaderMap,
) -> ResponseResult<OperatorRunnerPresenceV1> {
    let correlation = traceparent_correlation(&headers);
    let session = Arc::new(authenticated_session(&headers)?);
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_RUNNER_PRESENCE),
        correlation,
    )
    .await?;
    runner_id
        .parse::<UlidString>()
        .map_err(|_| ResponseError::from(BackendError::Rejected))?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.authorize_operator_runner_presence(&session)
    })
    .await
    .map_err(ResponseError::from)?;
    state
        .runner_presence
        .get(&runner_id)
        .map(Json)
        .ok_or_else(|| ResponseError::from(BackendError::NotFound))
}

async fn create_room(
    State(state): State<OperatorState>,
    headers: HeaderMap,
    body: Bytes,
) -> ResponseResult<CreateRoomResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let request = match strict_json::<CreateRoomRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Invalid,
                correlation,
            );
            return Err(error);
        }
    };
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_ROOM_CREATE),
        correlation,
    )
    .await?;
    let crash_match_id = request.idempotency_key.clone();
    pause_for_process_crash_evidence("room_create", "before_commit", &crash_match_id);
    let backend = Arc::clone(&state.backend);
    match backend_call(backend, move |backend| {
        backend.create_room(&session, request)
    })
    .await
    {
        Ok(response) => {
            pause_for_process_crash_evidence(
                "room_create",
                "after_commit_before_publication",
                &crash_match_id,
            );
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            record_commit_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            // A newly created Room cannot have a pre-existing attached
            // observer. Its only non-reply post-commit publication is the
            // admission/commit telemetry above; the marker records that
            // narrower contract explicitly instead of claiming a live frame.
            pause_for_process_crash_evidence(
                "room_create",
                "after_publication_before_reply",
                &crash_match_id,
            );
            Ok(Json(response))
        }
        Err(error) => {
            let reason = reason_for_backend_error(&error);
            record_admission_with_correlation(state.telemetry.as_ref(), reason, correlation);
            record_commit_with_correlation(state.telemetry.as_ref(), reason, correlation);
            Err(ResponseError::from(error))
        }
    }
}

/// Registers a member Capability through the authenticated `HostOperator`
/// authority path. The response is explicitly non-cacheable because its
/// bearer is a one-time delivery secret.
async fn issue_member_capability(
    State(state): State<OperatorState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let request = match strict_json::<MemberCapabilityIssueRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Invalid,
                correlation,
            );
            return Err(error);
        }
    };
    let targets = [AdmissionTarget {
        room_id: &request.room_id,
        member_id: Some(&request.member_id),
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_MEMBER_CAPABILITY),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    match backend_call(backend, move |backend| {
        backend.issue_member_capability(&session, request)
    })
    .await
    {
        Ok(response) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
            Ok((headers, Json(response)))
        }
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                reason_for_backend_error(&error),
                correlation,
            );
            Err(ResponseError::from(error))
        }
    }
}

async fn issue_runner_capability(
    State(state): State<OperatorState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let request = match strict_json::<RunnerCapabilityIssueRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Invalid,
                correlation,
            );
            return Err(error);
        }
    };
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_RUNNER_CAPABILITY),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    match backend_call(backend, move |backend| {
        backend.issue_runner_capability(&session, request)
    })
    .await
    {
        Ok(response) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
            Ok((headers, Json(response)))
        }
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                reason_for_backend_error(&error),
                correlation,
            );
            Err(ResponseError::from(error))
        }
    }
}

async fn provision_member_capability(
    State(state): State<OperatorState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let request = strict_json::<MemberCapabilityProvisionRequestV1>(&body).inspect_err(|_| {
        record_admission_with_correlation(
            state.telemetry.as_ref(),
            telemetry::ReasonCodeV1::Invalid,
            correlation,
        );
    })?;
    let targets = [AdmissionTarget {
        room_id: &request.room_id,
        member_id: Some(&request.member_id),
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_MEMBER_CAPABILITY),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    let result = backend_call(backend, move |backend| {
        backend.provision_member_capability(&session, request)
    })
    .await;
    sealed_provision_response(&state, correlation, result)
}

async fn provision_runner_capability(
    State(state): State<OperatorState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let request = strict_json::<RunnerCapabilityProvisionRequestV1>(&body).inspect_err(|_| {
        record_admission_with_correlation(
            state.telemetry.as_ref(),
            telemetry::ReasonCodeV1::Invalid,
            correlation,
        );
    })?;
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_RUNNER_CAPABILITY),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    let result = backend_call(backend, move |backend| {
        backend.provision_runner_capability(&session, request)
    })
    .await;
    sealed_provision_response(&state, correlation, result)
}

fn sealed_provision_response<T: Serialize>(
    state: &OperatorState,
    correlation: telemetry::CorrelationV1,
    result: Result<T, BackendError>,
) -> Result<Response, ResponseError> {
    match result {
        Ok(response) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
            Ok((headers, Json(response)).into_response())
        }
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                reason_for_backend_error(&error),
                correlation,
            );
            Err(ResponseError::from(error))
        }
    }
}

async fn operator_room_inventory(
    State(state): State<OperatorState>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> ResponseResult<OperatorRoomInventoryPage> {
    let session = Arc::new(authenticated_session(&headers)?);
    let request = parse_operator_room_inventory_query(raw_query.as_deref())?;
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_ROOM_INVENTORY),
        traceparent_correlation(&headers),
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.operator_room_inventory(&session, request)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

async fn operator_room_detail(
    State(state): State<OperatorState>,
    Path(room_id): Path<String>,
    headers: HeaderMap,
) -> ResponseResult<OperatorRoomSummary> {
    let session = Arc::new(authenticated_session(&headers)?);
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: None,
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_ROOM_DETAIL),
        traceparent_correlation(&headers),
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.operator_room_detail(&session, &room_id)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

/// Ephemeral gateway evidence, never Room state or participant content.
#[derive(Serialize)]
struct MembershipPresence {
    version: &'static str,
    synchronized: bool,
}

async fn operator_member_presence(
    State(state): State<OperatorState>,
    Path((room_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ResponseResult<MembershipPresence> {
    let session = Arc::new(authenticated_session(&headers)?);
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: Some(&member_id),
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_ROOM_DETAIL),
        traceparent_correlation(&headers),
    )
    .await?;
    member_id
        .parse::<UlidString>()
        .map_err(|_| ResponseError::from(BackendError::Rejected))?;
    let registry = state.live_streams.clone();
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.operator_room_detail(&session, &room_id)?;
        let synchronized = registry
            .snapshots_for_room(&room_id)
            .into_iter()
            .filter(|candidate| candidate.member_id == member_id && !candidate.sender.is_closed())
            .any(|candidate| {
                let authorized = backend
                    .live_observation_suffix(
                        &candidate.session,
                        &room_id,
                        &member_id,
                        candidate.last_delivered_frame_seq,
                    )
                    .is_ok();
                if !authorized {
                    registry.close(&candidate.session_id, ErrorCode::Unauthenticated);
                }
                authorized && registry.is_registered(&candidate.session_id)
            });
        Ok(MembershipPresence {
            version: "membership_presence.v1",
            synchronized,
        })
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

async fn operator_activation_status(
    State(state): State<OperatorState>,
    Path((room_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ResponseResult<OperatorActivationStatusV1> {
    let session = Arc::new(authenticated_session(&headers)?);
    room_id
        .parse::<worldstream_core::RoomId>()
        .map_err(|_| ResponseError::from(BackendError::NotFound))?;
    member_id
        .parse::<worldstream_core::MemberId>()
        .map_err(|_| ResponseError::from(BackendError::NotFound))?;
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: Some(&member_id),
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_ACTIVATION_STATUS),
        traceparent_correlation(&headers),
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.operator_activation_status(&session, &room_id, &member_id)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

async fn operator_backup_profile(
    State(state): State<OperatorState>,
    headers: HeaderMap,
) -> ResponseResult<OperatorBackupProfileStatus> {
    let session = Arc::new(authenticated_session(&headers)?);
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_BACKUP_PROFILE),
        traceparent_correlation(&headers),
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.operator_backup_profile(&session)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

async fn operator_live_backup(
    State(state): State<OperatorState>,
    headers: HeaderMap,
    Json(request): Json<OperatorLiveBackupPrepareRequest>,
) -> ResponseResult<OperatorLiveBackupStatus> {
    let session = Arc::new(authenticated_session(&headers)?);
    admit_authenticated_http(
        &state,
        &session,
        &[],
        Some(OPERATOR_LIVE_BACKUP),
        traceparent_correlation(&headers),
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    backend_call(backend, move |backend| {
        backend.operator_live_backup(&session, request)
    })
    .await
    .map(Json)
    .map_err(ResponseError::from)
}

fn parse_operator_room_inventory_query(
    raw_query: Option<&str>,
) -> Result<OperatorRoomInventoryRequest, ResponseError> {
    let mut request = OperatorRoomInventoryRequest {
        after_room_id: None,
        limit: worldstream_protocol::DEFAULT_OPERATOR_ROOM_PAGE_SIZE,
    };
    let Some(raw_query) = raw_query else {
        return Ok(request);
    };
    if raw_query.is_empty() || raw_query.len() > 256 {
        return Err(ResponseError::from(BackendError::Rejected));
    }
    let mut saw_after = false;
    let mut saw_limit = false;
    for part in raw_query.split('&') {
        let Some((name, value)) = part.split_once('=') else {
            return Err(ResponseError::from(BackendError::Rejected));
        };
        match name {
            "after_room_id" if !saw_after && !value.is_empty() => {
                saw_after = true;
                request.after_room_id = Some(value.to_owned());
            }
            "limit" if !saw_limit => {
                saw_limit = true;
                request.limit = value
                    .parse::<usize>()
                    .ok()
                    .filter(|limit| {
                        (1..=worldstream_protocol::MAX_OPERATOR_ROOM_PAGE_SIZE).contains(limit)
                    })
                    .ok_or_else(|| ResponseError::from(BackendError::Rejected))?;
            }
            _ => return Err(ResponseError::from(BackendError::Rejected)),
        }
    }
    Ok(request)
}

async fn fire_timer(
    State(state): State<OperatorState>,
    Path(room_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ResponseResult<TimerFireResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let request = match strict_json::<TimerFireRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Invalid,
                correlation,
            );
            return Err(error);
        }
    };
    if request.validate_bounds().is_err() {
        record_admission_with_correlation(
            state.telemetry.as_ref(),
            telemetry::ReasonCodeV1::Invalid,
            correlation,
        );
        return Err(ResponseError::from(BackendError::Rejected));
    }
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: None,
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_TIMER_FIRE),
        correlation,
    )
    .await?;
    let crash_match_id = format!("{}#{}", request.timer_id, request.generation);
    pause_for_process_crash_evidence("timer", "before_commit", &crash_match_id);
    let backend = Arc::clone(&state.backend);
    let backend_room_id = room_id.clone();
    match backend_call(backend, move |backend| {
        backend.fire_timer(&session, &backend_room_id, request)
    })
    .await
    {
        Ok(response) => {
            pause_for_process_crash_evidence(
                "timer",
                "after_commit_before_publication",
                &crash_match_id,
            );
            // The backend has durably committed the timer transition before
            // this post-commit publication.  Keep the HTTP response behind
            // the same live-stream seam used by accepted WebSocket actions so
            // externally fired timers advance already-registered observers.
            publish_live_frames(Arc::clone(&state.backend), &state.live_streams, &room_id).await;
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            record_commit_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            record_timer_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            pause_for_process_crash_evidence(
                "timer",
                "after_publication_before_reply",
                &crash_match_id,
            );
            Ok(Json(response))
        }
        Err(error) => {
            let reason = reason_for_backend_error(&error);
            record_admission_with_correlation(state.telemetry.as_ref(), reason, correlation);
            record_commit_with_correlation(state.telemetry.as_ref(), reason, correlation);
            record_timer_with_correlation(state.telemetry.as_ref(), reason, correlation);
            Err(ResponseError::from(error))
        }
    }
}

async fn launch_lobby(
    State(state): State<OperatorState>,
    Path(room_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ResponseResult<LobbyLaunchResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = Arc::new(authenticated_session(&headers)?);
    let request = strict_json::<LobbyLaunchRequest>(&body)?;
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: None,
    }];
    admit_authenticated_http(
        &state,
        &session,
        &targets,
        Some(OPERATOR_LOBBY_LAUNCH),
        correlation,
    )
    .await?;
    let backend = Arc::clone(&state.backend);
    let backend_room_id = room_id.clone();
    match backend_call(backend, move |backend| {
        backend.launch_lobby(&session, &backend_room_id, request)
    })
    .await
    {
        Ok(response) => {
            publish_live_frames(Arc::clone(&state.backend), &state.live_streams, &room_id).await;
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            record_commit_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            Ok(Json(response))
        }
        Err(error) => {
            let reason = reason_for_backend_error(&error);
            record_admission_with_correlation(state.telemetry.as_ref(), reason, correlation);
            record_commit_with_correlation(state.telemetry.as_ref(), reason, correlation);
            Err(ResponseError::from(error))
        }
    }
}

async fn current_projection(
    State(state): State<OperatorState>,
    Path(room_id): Path<String>,
    headers: HeaderMap,
) -> ResponseResult<ProjectionResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: None,
    }];
    admit_authenticated_http(&state, &session, &targets, None, correlation).await?;
    let backend = Arc::clone(&state.backend);
    match backend_call(backend, move |backend| {
        backend.projection(&session, &room_id)
    })
    .await
    {
        Ok(response) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            Ok(Json(response))
        }
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                reason_for_backend_error(&error),
                correlation,
            );
            Err(ResponseError::from(error))
        }
    }
}

async fn historical_replay(
    State(state): State<OperatorState>,
    Path(room_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> ResponseResult<ReplayResponse> {
    let correlation = traceparent_correlation(&headers);
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let at_room_seq = match parse_replay_query(raw_query.as_deref()) {
        Ok(value) => value,
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Invalid,
                correlation,
            );
            return Err(error);
        }
    };
    let targets = [AdmissionTarget {
        room_id: &room_id,
        member_id: None,
    }];
    admit_authenticated_http(&state, &session, &targets, None, correlation).await?;
    let backend = Arc::clone(&state.backend);
    match backend_call(backend, move |backend| {
        backend.replay(&session, &room_id, at_room_seq)
    })
    .await
    {
        Ok(response) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Accepted,
                correlation,
            );
            Ok(Json(response))
        }
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                reason_for_backend_error(&error),
                correlation,
            );
            Err(ResponseError::from(error))
        }
    }
}

fn parse_replay_query(raw_query: Option<&str>) -> Result<u64, ResponseError> {
    let Some(raw_query) = raw_query else {
        return Err(ResponseError::from(BackendError::Rejected));
    };
    let mut parts = raw_query.split('&');
    let Some(part) = parts.next() else {
        return Err(ResponseError::from(BackendError::Rejected));
    };
    if parts.next().is_some() {
        return Err(ResponseError::from(BackendError::Rejected));
    }
    let Some((name, value)) = part.split_once('=') else {
        return Err(ResponseError::from(BackendError::Rejected));
    };
    if name != "at_room_seq" || value.is_empty() {
        return Err(ResponseError::from(BackendError::Rejected));
    }
    value
        .parse::<u64>()
        .map_err(|_| ResponseError::from(BackendError::Rejected))
}

/// Issues one origin-bound, short-lived browser admission ticket. The bearer
/// is accepted only through the normal Authorization header and is never
/// copied into the response, ticket, URL, or diagnostics.
async fn issue_browser_ticket(
    State(state): State<OperatorState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let origin = match browser_origin(&headers) {
        Ok(origin) => origin,
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    let session = match authenticated_session(&headers) {
        Ok(session) => Arc::new(session),
        Err(error) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Unauthorized,
                correlation,
            );
            return Err(error);
        }
    };
    admit_authenticated_http(&state, &session, &[], None, correlation).await?;
    let session = Arc::try_unwrap(session)
        .map_err(|_| ResponseError::from(BackendError::StorageUnavailable))?;
    let ticket = match state.browser_tickets.issue(session, origin.clone()) {
        Ok(ticket) => ticket,
        Err(BrowserTicketError::Capacity) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::Busy,
                correlation,
            );
            return Err(ResponseError::from(BackendError::Busy));
        }
        Err(BrowserTicketError::RandomnessUnavailable) => {
            record_admission_with_correlation(
                state.telemetry.as_ref(),
                telemetry::ReasonCodeV1::StorageUnavailable,
                correlation,
            );
            return Err(ResponseError::from(BackendError::StorageUnavailable));
        }
    };
    record_admission_with_correlation(
        state.telemetry.as_ref(),
        telemetry::ReasonCodeV1::Accepted,
        correlation,
    );
    let mut response_headers = browser_cors_headers(&origin);
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response_headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    Ok((
        response_headers,
        Json(BrowserWebSocketTicketIssueResponse {
            version: BROWSER_WS_TICKET_VERSION.to_owned(),
            ticket,
            expires_in_ms: BROWSER_TICKET_TTL_MS,
        }),
    ))
}

async fn browser_ticket_preflight(headers: HeaderMap) -> Result<impl IntoResponse, ResponseError> {
    let origin = browser_origin(&headers)?;
    let mut response_headers = browser_cors_headers(&origin);
    response_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((StatusCode::NO_CONTENT, response_headers))
}

fn browser_origin(headers: &HeaderMap) -> Result<String, ResponseError> {
    let Some(raw) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    if raw.len() > 256 || raw.contains('@') {
        return Err(ResponseError::from(BackendError::Forbidden));
    }
    let uri = raw
        .parse::<axum::http::Uri>()
        .map_err(|_| ResponseError::from(BackendError::Forbidden))?;
    let Some(scheme) = uri.scheme_str() else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    if (scheme != "http" && scheme != "https")
        || (uri.path() != "" && uri.path() != "/")
        || uri.query().is_some()
    {
        return Err(ResponseError::from(BackendError::Forbidden));
    }
    let Some(authority) = uri.authority() else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    let host = authority.host();
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        return Err(ResponseError::from(BackendError::Forbidden));
    }
    Ok(raw.to_owned())
}

fn browser_cors_headers(origin: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Authorization, Content-Type"),
    );
    headers
}

type ResponseResult<T> = Result<Json<T>, ResponseError>;

#[derive(Debug)]
struct ResponseError {
    status: StatusCode,
    envelope: ErrorEnvelope,
}

impl From<BackendError> for ResponseError {
    fn from(error: BackendError) -> Self {
        let status = match &error {
            BackendError::Forbidden => StatusCode::FORBIDDEN,
            BackendError::NotFound | BackendError::ActivityPackRevisionUnavailable => {
                StatusCode::NOT_FOUND
            }
            BackendError::Busy => StatusCode::TOO_MANY_REQUESTS,
            BackendError::Conflict | BackendError::WrongPhase => StatusCode::CONFLICT,
            BackendError::StorageUnavailable
            | BackendError::Indeterminate
            | BackendError::RoomFaulted
            | BackendError::RoomQuarantined => StatusCode::SERVICE_UNAVAILABLE,
            BackendError::Rejected | BackendError::InvalidResult => StatusCode::BAD_REQUEST,
        };
        let code = error.code();
        let retryable = matches!(
            code,
            ErrorCode::StorageUnavailable
                | ErrorCode::CommitIndeterminate
                | ErrorCode::RoomBusy
                | ErrorCode::RoomFaulted
        );
        Self {
            status,
            envelope: ErrorEnvelope::new(code, safe_message(code), retryable),
        }
    }
}

impl IntoResponse for ResponseError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.envelope)).into_response()
    }
}

fn rate_limited_response() -> ResponseError {
    ResponseError {
        status: StatusCode::TOO_MANY_REQUESTS,
        envelope: ErrorEnvelope::new(
            ErrorCode::RateLimited,
            safe_message(ErrorCode::RateLimited),
            true,
        ),
    }
}

fn safe_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::StorageUnavailable => "storage is unavailable",
        ErrorCode::CommitIndeterminate => "the operation result is not yet resolved",
        ErrorCode::Forbidden => "the capability is not authorized for this operation",
        ErrorCode::RoomBusy => "the room is temporarily busy",
        ErrorCode::RateLimited => "gateway admission limit exceeded",
        ErrorCode::RoomNotFound => "the requested room is unavailable",
        ErrorCode::ActivityPackRevisionUnavailable => {
            "the exact Activity Pack revision is unavailable"
        }
        ErrorCode::IdempotencyConflict => {
            "the operation identity conflicts with an existing request"
        }
        ErrorCode::InvalidPayload => "the request payload is invalid",
        ErrorCode::WrongPhase => "the Activity is not waiting in Lobby",
        _ => "the request could not be completed",
    }
}

#[cfg(test)]
fn bearer(headers: &HeaderMap) -> Result<CapabilityBearerV1, ResponseError> {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    let wire =
        BearerWireV1::parse(token).map_err(|_| ResponseError::from(BackendError::Forbidden))?;
    Ok(CapabilityBearerV1::from_bytes(wire.into_bytes()))
}

fn authenticated_session(headers: &HeaderMap) -> Result<GatewaySession, ResponseError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| ResponseError::from(BackendError::Forbidden))?;
    let wire =
        BearerWireV1::parse(token).map_err(|_| ResponseError::from(BackendError::Forbidden))?;
    let bearer = CapabilityBearerV1::from_bytes(
        BearerWireV1::parse(&wire.to_wire())
            .map_err(|_| ResponseError::from(BackendError::Forbidden))?
            .into_bytes(),
    );
    let session_id = next_ulid().ok_or_else(|| ResponseError::from(BackendError::Indeterminate))?;
    Ok(GatewaySession::new_with_wire(session_id, bearer, wire))
}

fn strict_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, ResponseError> {
    if body.len() > worldstream_protocol::MAX_MESSAGE_BYTES {
        return Err(ResponseError::from(BackendError::Rejected));
    }
    serde_json::from_slice(body).map_err(|_| ResponseError::from(BackendError::Rejected))
}

fn next_ulid() -> Option<UlidString> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let timestamp_ms = u64::try_from(elapsed.as_millis()).ok()?;
    if timestamp_ms >= (1_u64 << 48) {
        return None;
    }
    let mut randomness = [0_u8; 10];
    fill_random_bytes(&mut randomness).ok()?;
    ulid_from_parts(timestamp_ms, randomness)
}

fn ulid_from_parts(timestamp_ms: u64, randomness: [u8; 10]) -> Option<UlidString> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    if timestamp_ms >= (1_u64 << 48) {
        return None;
    }
    let mut value = [0_u8; 16];
    value[..6].copy_from_slice(&timestamp_ms.to_be_bytes()[2..]);
    value[6..].copy_from_slice(&randomness);
    let mut numeric = u128::from_be_bytes(value);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(numeric & 0x1f) as usize];
        numeric >>= 5;
    }
    std::str::from_utf8(&encoded).ok()?.parse().ok()
}

#[derive(Clone, Copy)]
struct TransportPeer(PeerIdentity);

impl<S> FromRequestParts<S> for TransportPeer
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip());
        Ok(Self(PeerIdentity::from_ip(ip)))
    }
}

#[derive(Clone)]
struct StreamAdmission {
    limiter: GatewayRateLimiter,
    peer: PeerIdentity,
    principal_id: String,
}

impl StreamAdmission {
    fn admit_base(&self, session: &GatewaySession) -> Result<(), RateLimitRejection> {
        self.admit_message(session, WebSocketAdmissionScope::default())
    }

    fn admit_message(
        &self,
        session: &GatewaySession,
        scope: WebSocketAdmissionScope<'_>,
    ) -> Result<(), RateLimitRejection> {
        let targets = scope.target.as_slice();
        let capability_hash = session.bearer().token_hash();
        self.limiter.admit(GatewayAdmission {
            peer: Some(self.peer),
            principal_id: Some(&self.principal_id),
            capability_material: Some(capability_hash.storage_bytes()),
            session_id: Some(session.session_id().as_str()),
            activation_operation: scope.activation_operation,
            targets,
            ..GatewayAdmission::default()
        })
    }
}

async fn room_stream(
    State(state): State<OperatorState>,
    TransportPeer(peer): TransportPeer,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let origin = websocket_origin(&headers)?;
    let upgrade = require_websocket_subprotocol(upgrade)?;
    let telemetry = state.telemetry.clone();
    let pending = state
        .rate_limiter
        .reserve_websocket()
        .map_err(|_| rate_limited_response())?;
    if headers.contains_key(header::AUTHORIZATION) {
        let session = Arc::new(authenticated_session(&headers)?);
        let principal_id = resolve_admission_principal(&state, &session).await?;
        let admission = StreamAdmission {
            limiter: state.rate_limiter.clone(),
            peer,
            principal_id,
        };
        admission
            .admit_base(&session)
            .map_err(|_| rate_limited_response())?;
        let active = pending
            .activate(&admission.principal_id)
            .map_err(|_| rate_limited_response())?;
        return Ok(upgrade.on_upgrade(move |socket| {
            stream_loop(
                socket,
                state.backend,
                state.live_streams,
                state.runner_presence,
                session,
                admission,
                telemetry,
                StreamEndpoint::Room,
                correlation,
                active,
            )
        }));
    }
    let Some(origin) = origin else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    let tickets = Arc::clone(&state.browser_tickets);
    Ok(upgrade.on_upgrade(move |socket| {
        browser_stream_loop(
            socket,
            state.backend,
            state.live_streams,
            state.runner_presence,
            tickets,
            origin,
            peer,
            state.rate_limiter,
            pending,
            telemetry,
            StreamEndpoint::Room,
            correlation,
        )
    }))
}

async fn runner_stream(
    State(state): State<OperatorState>,
    TransportPeer(peer): TransportPeer,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<impl IntoResponse, ResponseError> {
    let correlation = traceparent_correlation(&headers);
    let origin = websocket_origin(&headers)?;
    let upgrade = require_websocket_subprotocol(upgrade)?;
    let telemetry = state.telemetry.clone();
    let pending = state
        .rate_limiter
        .reserve_websocket()
        .map_err(|_| rate_limited_response())?;
    if headers.contains_key(header::AUTHORIZATION) {
        let session = Arc::new(authenticated_session(&headers)?);
        let principal_id = resolve_admission_principal(&state, &session).await?;
        let admission = StreamAdmission {
            limiter: state.rate_limiter.clone(),
            peer,
            principal_id,
        };
        admission
            .admit_base(&session)
            .map_err(|_| rate_limited_response())?;
        let active = pending
            .activate(&admission.principal_id)
            .map_err(|_| rate_limited_response())?;
        return Ok(upgrade.on_upgrade(move |socket| {
            stream_loop(
                socket,
                state.backend,
                state.live_streams,
                state.runner_presence,
                session,
                admission,
                telemetry,
                StreamEndpoint::Runner,
                correlation,
                active,
            )
        }));
    }
    let Some(origin) = origin else {
        return Err(ResponseError::from(BackendError::Forbidden));
    };
    let tickets = Arc::clone(&state.browser_tickets);
    Ok(upgrade.on_upgrade(move |socket| {
        browser_stream_loop(
            socket,
            state.backend,
            state.live_streams,
            state.runner_presence,
            tickets,
            origin,
            peer,
            state.rate_limiter,
            pending,
            telemetry,
            StreamEndpoint::Runner,
            correlation,
        )
    }))
}

fn require_websocket_subprotocol(
    upgrade: WebSocketUpgrade,
) -> Result<WebSocketUpgrade, ResponseError> {
    if !upgrade
        .requested_protocols()
        .any(|protocol| protocol.as_bytes() == WEBSOCKET_SUBPROTOCOL.as_bytes())
    {
        return Err(ResponseError::from(BackendError::Rejected));
    }
    let upgrade = upgrade.protocols([WEBSOCKET_SUBPROTOCOL]);
    if upgrade.selected_protocol().is_none() {
        return Err(ResponseError::from(BackendError::Rejected));
    }
    Ok(upgrade)
}

fn websocket_origin(headers: &HeaderMap) -> Result<Option<String>, ResponseError> {
    if headers.contains_key(header::ORIGIN) {
        return browser_origin(headers).map(Some);
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
async fn browser_stream_loop(
    mut socket: WebSocket,
    backend: Arc<dyn GatewayBackend>,
    live_streams: LiveStreamRegistry,
    runner_presence: RunnerPresenceRegistry,
    tickets: Arc<BrowserTicketStore>,
    origin: String,
    peer: PeerIdentity,
    rate_limiter: GatewayRateLimiter,
    pending: PendingWebSocketPermit,
    telemetry: Option<telemetry::TelemetryHandle>,
    endpoint: StreamEndpoint,
    correlation: telemetry::CorrelationV1,
) {
    if rate_limiter
        .admit(GatewayAdmission {
            peer: Some(peer),
            ..GatewayAdmission::default()
        })
        .is_err()
    {
        let _ = send_error(&mut socket, None, ErrorCode::RateLimited, true).await;
        return;
    }
    let Some(ticket) = receive_browser_ticket(socket.recv(), BROWSER_TICKET_TTL).await else {
        close_browser_admission(&mut socket).await;
        record_admission_with_correlation(
            telemetry.as_ref(),
            telemetry::ReasonCodeV1::Unauthorized,
            correlation,
        );
        return;
    };
    let Some(session) = tickets.consume(ticket.as_ref(), &origin) else {
        close_browser_admission(&mut socket).await;
        record_admission_with_correlation(
            telemetry.as_ref(),
            telemetry::ReasonCodeV1::Unauthorized,
            correlation,
        );
        return;
    };
    let session = Arc::new(session);
    let authentication_session = Arc::clone(&session);
    let Ok(principal_id) = backend_call(Arc::clone(&backend), move |backend| {
        backend.admission_principal(&authentication_session)
    })
    .await
    else {
        close_browser_admission(&mut socket).await;
        record_admission_with_correlation(
            telemetry.as_ref(),
            telemetry::ReasonCodeV1::Unauthorized,
            correlation,
        );
        return;
    };
    let admission = StreamAdmission {
        limiter: rate_limiter,
        peer,
        principal_id,
    };
    if admission.admit_base(&session).is_err() {
        let _ = send_error(&mut socket, None, ErrorCode::RateLimited, true).await;
        return;
    }
    let Ok(active) = pending.activate(&admission.principal_id) else {
        let _ = send_error(&mut socket, None, ErrorCode::RateLimited, true).await;
        return;
    };
    stream_loop(
        socket,
        backend,
        live_streams,
        runner_presence,
        session,
        admission,
        telemetry,
        endpoint,
        correlation,
        active,
    )
    .await;
}

async fn receive_browser_ticket<'a, F>(receive: F, timeout: Duration) -> Option<String>
where
    F: Future<Output = Option<Result<Message, axum::Error>>> + 'a,
{
    match receive_websocket_message(receive, timeout).await {
        Some(Message::Text(ticket)) => Some(ticket.to_string()),
        _ => None,
    }
}

async fn receive_websocket_message<'a, F>(receive: F, timeout: Duration) -> Option<Message>
where
    F: Future<Output = Option<Result<Message, axum::Error>>> + 'a,
{
    match tokio::time::timeout(timeout, receive).await {
        Ok(Some(Ok(message))) => Some(message),
        _ => None,
    }
}

async fn close_browser_admission(socket: &mut WebSocket) {
    let _ = send_websocket_message(
        socket.send(browser_admission_close_message()),
        WEBSOCKET_SEND_TIMEOUT,
    )
    .await;
}

fn browser_admission_close_message() -> Message {
    Message::Close(Some(CloseFrame {
        code: 1008,
        reason: BROWSER_ADMISSION_CLOSE_REASON.into(),
    }))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn stream_loop(
    mut socket: WebSocket,
    backend: Arc<dyn GatewayBackend>,
    live_streams: LiveStreamRegistry,
    runner_presence: RunnerPresenceRegistry,
    session: Arc<GatewaySession>,
    admission: StreamAdmission,
    telemetry: Option<telemetry::TelemetryHandle>,
    endpoint: StreamEndpoint,
    correlation: telemetry::CorrelationV1,
    _connection_permit: ActiveWebSocketPermit,
) {
    let Some(_active_session) = ActiveGatewaySession::reserve(
        Arc::clone(&backend),
        live_streams.clone(),
        session.session_id().clone(),
    ) else {
        let _ = send_error(&mut socket, None, ErrorCode::Internal, false).await;
        return;
    };
    let Some(message) = receive_websocket_message(socket.recv(), FIRST_CLIENT_HELLO_TIMEOUT).await
    else {
        return;
    };
    if admission.admit_base(&session).is_err() {
        let _ = send_error(&mut socket, None, ErrorCode::RateLimited, true).await;
        return;
    }
    let Message::Text(text) = message else {
        return;
    };
    let raw = text.as_bytes();
    let hello = match decode_envelope::<ClientHello>(raw) {
        Ok(envelope) if envelope.message_type == "client.hello" => envelope,
        _ => {
            let _ = send_error(&mut socket, None, ErrorCode::InvalidEnvelope, false).await;
            record_admission_with_correlation(
                telemetry.as_ref(),
                telemetry::ReasonCodeV1::Invalid,
                correlation,
            );
            return;
        }
    };
    if let Some(missing) = hello.body.missing_required_capability() {
        let _ = send_error(
            &mut socket,
            Some(&hello.message_id),
            ErrorCode::InvalidPayload,
            false,
        )
        .await;
        tracing::debug!(
            capability = missing,
            "client hello is missing a required capability"
        );
        record_admission_with_correlation(
            telemetry.as_ref(),
            telemetry::ReasonCodeV1::Invalid,
            correlation,
        );
        return;
    }
    if (endpoint == StreamEndpoint::Runner) != (hello.body.mode == ClientMode::Runner) {
        let _ = send_error(
            &mut socket,
            Some(&hello.message_id),
            ErrorCode::InvalidPayload,
            false,
        )
        .await;
        record_admission_with_correlation(
            telemetry.as_ref(),
            telemetry::ReasonCodeV1::Invalid,
            correlation,
        );
        return;
    }
    let hello_body = hello.body.clone();
    let hello_session = Arc::clone(&session);
    let welcome = match backend_call(Arc::clone(&backend), move |backend| {
        backend.hello(&hello_session, &hello_body)
    })
    .await
    {
        Ok(welcome) => welcome,
        Err(error) => {
            let _ = send_error(
                &mut socket,
                Some(&hello.message_id),
                error.code(),
                matches!(
                    error,
                    BackendError::RoomFaulted
                        | BackendError::RoomQuarantined
                        | BackendError::Indeterminate
                        | BackendError::Busy
                        | BackendError::StorageUnavailable
                ),
            )
            .await;
            record_admission_with_correlation(
                telemetry.as_ref(),
                reason_for_backend_error(&error),
                correlation,
            );
            return;
        }
    };
    if !welcome_matches_session(&welcome, &session) {
        let _ = send_error(
            &mut socket,
            Some(&hello.message_id),
            ErrorCode::Internal,
            false,
        )
        .await;
        tracing::error!("gateway backend returned a welcome for a different session");
        record_admission_with_correlation(
            telemetry.as_ref(),
            telemetry::ReasonCodeV1::Invalid,
            correlation,
        );
        return;
    }
    let heartbeat_interval = Duration::from_millis(welcome.heartbeat_interval_ms.max(1));
    if send_body(
        &mut socket,
        "server.welcome",
        Some(&hello.message_id),
        welcome,
    )
    .await
    .is_err()
    {
        record_frame_with_correlation(
            telemetry.as_ref(),
            telemetry::FrameDeliveryOutcomeV1::Failed,
            1,
            correlation,
        );
        return;
    }
    record_admission_with_correlation(
        telemetry.as_ref(),
        telemetry::ReasonCodeV1::Accepted,
        correlation,
    );
    let mut live = false;
    let (push_sender, mut push_receiver) = mpsc::channel(LIVE_PUSH_CAPACITY);
    let (close_sender, mut close_receiver) = watch::channel(None);
    let mut attached_stream = None;
    let mut runner = RunnerConnectionState {
        mode: hello.body.mode,
        ready: false,
        runner_id: None,
    };
    let mut active_runner_presence = None;
    let mut heartbeat = tokio::time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // `interval` ticks immediately once; the welcome itself is the initial
    // inbound liveness signal, so schedule the first ping one full interval
    // after it instead of emitting a redundant ping beside the welcome.
    heartbeat.tick().await;
    let mut idle_deadline = tokio::time::Instant::now() + POST_WELCOME_IDLE_TIMEOUT;
    loop {
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(idle_deadline) => break,
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break; };
                idle_deadline = tokio::time::Instant::now() + POST_WELCOME_IDLE_TIMEOUT;
                if runner.ready {
                    runner_presence.touch(session.session_id());
                }
                let Message::Text(text) = message else {
                    if admission.admit_base(&session).is_err() {
                        let _ = send_error(&mut socket, None, ErrorCode::RateLimited, true).await;
                        break;
                    }
                    continue;
                };
                let raw = text.as_bytes();
                let value: VersionedEnvelope<Value> = match decode_envelope(raw) {
                    Ok(value) => value,
                    Err(error) => {
                        if admission.admit_base(&session).is_err() {
                            let _ = send_error(&mut socket, None, ErrorCode::RateLimited, true).await;
                            break;
                        }
                        let code = if matches!(error, worldstream_protocol::EnvelopeError::TooLarge) {
                            ErrorCode::MessageTooLarge
                        } else {
                            ErrorCode::InvalidEnvelope
                        };
                        let _ = send_error(&mut socket, None, code, false).await;
                        continue;
                    }
                };
                if admission
                    .admit_message(&session, websocket_admission_scope(&value))
                    .is_err()
                {
                    let request_id = reply_request_id(
                        value.request_id.as_ref(),
                        &value.message_id,
                    );
                    let _ = send_error(
                        &mut socket,
                        request_id,
                        ErrorCode::RateLimited,
                        true,
                    )
                    .await;
                    break;
                }
                let result = dispatch_message(
                    &mut socket,
                    Arc::clone(&backend),
                    &live_streams,
                    &push_sender,
                    &close_sender,
                    &session,
                    value,
                    &mut live,
                    &mut attached_stream,
                    &mut runner,
                    &runner_presence,
                    &mut active_runner_presence,
                    telemetry.as_ref(),
                    correlation,
                )
                .await;
                if result.is_err() { break; }
            }
            push = push_receiver.recv() => {
                match push {
                    Some(LivePush::Frame(push)) => {
                        if send_websocket_message(
                            socket.send(push.message),
                            WEBSOCKET_SEND_TIMEOUT,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    None => break,
                }
            }
            close = close_receiver.changed() => {
                if close.is_ok() {
                    let code = *close_receiver.borrow();
                    if let Some(code) = code {
                        let _ = send_error(&mut socket, None, code, true).await;
                    }
                }
                break;
            }
            _ = heartbeat.tick() => {
                if send_body(&mut socket, "server.ping", None, serde_json::json!({})).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn welcome_matches_session(welcome: &ServerWelcome, session: &GatewaySession) -> bool {
    welcome.session_id.as_str() == session.session_id().as_str()
}

#[derive(Clone, Debug)]
struct RunnerConnectionState {
    mode: ClientMode,
    ready: bool,
    runner_id: Option<String>,
}

#[derive(Clone, Default)]
struct RunnerPresenceRegistry {
    entries: Arc<Mutex<HashMap<String, RunnerPresenceEntry>>>,
}

struct RunnerPresenceEntry {
    session_id: UlidString,
    connected: bool,
    maximum: u32,
    active: HashSet<String>,
    supported_pack_revisions: Vec<PackReference>,
    observed_at_unix_ms: u64,
}

impl RunnerPresenceRegistry {
    fn register(&self, session_id: &UlidString, hello: &RunnerHello) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(
            hello.runner_id.clone(),
            RunnerPresenceEntry {
                session_id: session_id.clone(),
                connected: true,
                maximum: hello.maximum_concurrent_activations,
                active: HashSet::new(),
                supported_pack_revisions: hello.supported_pack_revisions.clone(),
                observed_at_unix_ms: unix_time_ms(),
            },
        );
    }

    fn touch(&self, session_id: &UlidString) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries
            .values_mut()
            .find(|entry| &entry.session_id == session_id && entry.connected)
        {
            entry.observed_at_unix_ms = unix_time_ms();
        }
    }

    fn activation_started(&self, runner_id: &str, activation_id: &str) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(runner_id)
            && entry.connected
        {
            entry.active.insert(activation_id.to_owned());
            entry.observed_at_unix_ms = unix_time_ms();
        }
    }

    fn activation_finished(&self, runner_id: &str, activation_id: &str) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(runner_id) {
            entry.active.remove(activation_id);
            entry.observed_at_unix_ms = unix_time_ms();
        }
    }

    fn disconnect(&self, runner_id: &str, session_id: &UlidString) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(runner_id)
            && &entry.session_id == session_id
        {
            entry.connected = false;
            entry.active.clear();
            entry.observed_at_unix_ms = unix_time_ms();
        }
    }

    fn get(&self, runner_id: &str) -> Option<OperatorRunnerPresenceV1> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.get(runner_id)?;
        let active = u32::try_from(entry.active.len()).unwrap_or(u32::MAX);
        let stale = unix_time_ms().saturating_sub(entry.observed_at_unix_ms)
            > u64::try_from(RUNNER_PRESENCE_STALE_AFTER.as_millis()).unwrap_or(u64::MAX);
        Some(OperatorRunnerPresenceV1 {
            version: RUNNER_PRESENCE_VERSION.to_owned(),
            runner_id: runner_id.to_owned(),
            connection: if entry.connected {
                OperatorRunnerConnectionV1::Connected
            } else {
                OperatorRunnerConnectionV1::Disconnected
            },
            freshness: if stale {
                OperatorRunnerFreshnessV1::Stale
            } else {
                OperatorRunnerFreshnessV1::Fresh
            },
            maximum_concurrent_activations: entry.maximum,
            active_activations: active.min(entry.maximum),
            available_activations: entry.maximum.saturating_sub(active),
            supported_pack_revisions: entry.supported_pack_revisions.clone(),
            observed_at_unix_ms: entry.observed_at_unix_ms,
        })
    }
}

struct ActiveRunnerPresence {
    registry: RunnerPresenceRegistry,
    runner_id: String,
    session_id: UlidString,
}

impl Drop for ActiveRunnerPresence {
    fn drop(&mut self) {
        self.registry.disconnect(&self.runner_id, &self.session_id);
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .unwrap_or(0)
}

fn checked_unix_time_ms() -> Result<u64, BackendError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| BackendError::StorageUnavailable)?;
    u64::try_from(elapsed.as_millis()).map_err(|_| BackendError::InvalidResult)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamEndpoint {
    Room,
    Runner,
}

struct AttachedStream {
    room_id: String,
    member_id: String,
}

const LIVE_PUSH_CAPACITY: usize = MAX_OUTBOUND_FRAME_BURST;

#[derive(Clone, Default)]
struct LiveStreamRegistry {
    sessions: Arc<Mutex<HashMap<String, LiveStreamRegistration>>>,
    active_session_ids: Arc<Mutex<HashSet<UlidString>>>,
    publication_lock: Arc<AsyncMutex<()>>,
    queue_metrics: Arc<LiveStreamQueueMetrics>,
}

struct ActiveGatewaySession {
    backend: Arc<dyn GatewayBackend>,
    registry: LiveStreamRegistry,
    session_id: UlidString,
}

impl ActiveGatewaySession {
    fn reserve(
        backend: Arc<dyn GatewayBackend>,
        registry: LiveStreamRegistry,
        session_id: UlidString,
    ) -> Option<Self> {
        if !registry.reserve_session(&session_id) {
            return None;
        }
        Some(Self {
            backend,
            registry,
            session_id,
        })
    }
}

impl Drop for ActiveGatewaySession {
    fn drop(&mut self) {
        self.registry.unregister(self.session_id.as_str());
        self.registry.release_session(&self.session_id);
        self.backend.retire_session(&self.session_id);
    }
}

struct LiveStreamRegistration {
    session: Arc<GatewaySession>,
    room_id: String,
    member_id: String,
    last_delivered_frame_seq: u64,
    sender: mpsc::Sender<LivePush>,
    queued_frames: Arc<AtomicUsize>,
    queued_payload_bytes: Arc<AtomicUsize>,
    close: watch::Sender<Option<ErrorCode>>,
}

#[derive(Clone)]
struct LiveStreamSnapshot {
    session_id: String,
    session: Arc<GatewaySession>,
    room_id: String,
    member_id: String,
    last_delivered_frame_seq: u64,
    sender: mpsc::Sender<LivePush>,
    queued_frames: Arc<AtomicUsize>,
    queued_payload_bytes: Arc<AtomicUsize>,
}

#[derive(Debug, Default)]
struct LiveQueueCounters {
    process_current: AtomicUsize,
    process_high_water: AtomicUsize,
    unit_high_water: AtomicUsize,
    activity_total: AtomicU64,
    completion_total: AtomicU64,
    backpressure_total: AtomicU64,
}

impl LiveQueueCounters {
    fn try_reserve(
        self: &Arc<Self>,
        unit_current: Arc<AtomicUsize>,
        amount: usize,
        capacity: usize,
        activity_amount: u64,
    ) -> Option<LiveQueueReservation> {
        let Ok(previous) =
            unit_current.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(amount).filter(|next| *next <= capacity)
            })
        else {
            self.note_backpressure();
            return None;
        };
        let unit_depth = previous + amount;
        let process_depth = self.process_current.fetch_add(amount, Ordering::AcqRel) + amount;
        atomic_max_usize(&self.process_high_water, process_depth);
        atomic_max_usize(&self.unit_high_water, unit_depth);
        self.activity_total
            .fetch_add(activity_amount, Ordering::Relaxed);
        Some(LiveQueueReservation {
            unit_current,
            metrics: Arc::clone(self),
            amount,
            activity_amount,
        })
    }

    fn note_backpressure(&self) {
        self.backpressure_total.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self, name: &'static str, capacity: usize) -> InternalQueueSnapshot {
        InternalQueueSnapshot {
            name,
            capacity_scope: "per_connection",
            capacity,
            process_current: self.process_current.load(Ordering::Acquire),
            process_high_water: self.process_high_water.load(Ordering::Acquire),
            unit_high_water: self.unit_high_water.load(Ordering::Acquire),
            activity_total: self.activity_total.load(Ordering::Relaxed),
            completion_total: self.completion_total.load(Ordering::Relaxed),
            backpressure_total: self.backpressure_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Default)]
struct LiveStreamQueueMetrics {
    frames: Arc<LiveQueueCounters>,
    payload_bytes: Arc<LiveQueueCounters>,
}

struct LiveQueueReservation {
    unit_current: Arc<AtomicUsize>,
    metrics: Arc<LiveQueueCounters>,
    amount: usize,
    activity_amount: u64,
}

impl Drop for LiveQueueReservation {
    fn drop(&mut self) {
        let unit_previous = self.unit_current.fetch_sub(self.amount, Ordering::AcqRel);
        let process_previous = self
            .metrics
            .process_current
            .fetch_sub(self.amount, Ordering::AcqRel);
        debug_assert!(unit_previous >= self.amount);
        debug_assert!(process_previous >= self.amount);
        self.metrics
            .completion_total
            .fetch_add(self.activity_amount, Ordering::Relaxed);
    }
}

struct OutboundPayloadReservation {
    _reservation: LiveQueueReservation,
}

impl OutboundPayloadReservation {
    fn try_new(
        queued_payload_bytes: Arc<AtomicUsize>,
        payload_bytes: usize,
        metrics: &Arc<LiveQueueCounters>,
    ) -> Option<Self> {
        let activity_amount = u64::try_from(payload_bytes).unwrap_or(u64::MAX);
        metrics
            .try_reserve(
                queued_payload_bytes,
                payload_bytes,
                MAX_OUTBOUND_BUFFER_BYTES,
                activity_amount,
            )
            .map(|reservation| Self {
                _reservation: reservation,
            })
    }
}

struct OutboundFrameReservation {
    _reservation: LiveQueueReservation,
}

impl OutboundFrameReservation {
    fn try_new(queued_frames: Arc<AtomicUsize>, metrics: &Arc<LiveQueueCounters>) -> Option<Self> {
        metrics
            .try_reserve(queued_frames, 1, LIVE_PUSH_CAPACITY, 1)
            .map(|reservation| Self {
                _reservation: reservation,
            })
    }
}

struct LiveFramePush {
    #[cfg(test)]
    frame: ObservationDeliver,
    message: Message,
    _frame_reservation: OutboundFrameReservation,
    _payload_reservation: OutboundPayloadReservation,
}

enum LivePush {
    Frame(LiveFramePush),
}

impl LiveStreamRegistry {
    fn is_registered(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session_id)
    }
    fn register(
        &self,
        session: Arc<GatewaySession>,
        room_id: String,
        member_id: String,
        last_delivered_frame_seq: u64,
        sender: mpsc::Sender<LivePush>,
        close: watch::Sender<Option<ErrorCode>>,
    ) -> Result<(), BackendError> {
        let session_id = session.session_id().to_string();
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match sessions.entry(session_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(LiveStreamRegistration {
                    session,
                    room_id,
                    member_id,
                    last_delivered_frame_seq,
                    sender,
                    queued_frames: Arc::new(AtomicUsize::new(0)),
                    queued_payload_bytes: Arc::new(AtomicUsize::new(0)),
                    close,
                });
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => Err(BackendError::StorageUnavailable),
        }
    }

    fn reserve_session(&self, session_id: &UlidString) -> bool {
        self.active_session_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session_id.clone())
    }

    fn release_session(&self, session_id: &UlidString) {
        self.active_session_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    fn unregister(&self, session_id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    fn snapshots_for_room(&self, room_id: &str) -> Vec<LiveStreamSnapshot> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|registration| registration.room_id == room_id)
            .map(|registration| LiveStreamSnapshot {
                session_id: registration.session.session_id().to_string(),
                session: Arc::clone(&registration.session),
                room_id: registration.room_id.clone(),
                member_id: registration.member_id.clone(),
                last_delivered_frame_seq: registration.last_delivered_frame_seq,
                sender: registration.sender.clone(),
                queued_frames: Arc::clone(&registration.queued_frames),
                queued_payload_bytes: Arc::clone(&registration.queued_payload_bytes),
            })
            .collect()
    }

    fn mark_delivered(&self, session_id: &str, frame_seq: u64) {
        if let Some(registration) = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(session_id)
        {
            registration.last_delivered_frame_seq = frame_seq;
        }
    }

    fn close(&self, session_id: &str, code: ErrorCode) {
        if let Some(registration) = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id)
        {
            let _ = registration.close.send(Some(code));
        }
    }

    fn queue_snapshots(&self) -> [InternalQueueSnapshot; 2] {
        [
            self.queue_metrics
                .frames
                .snapshot("websocket_live_push_frame_queue", LIVE_PUSH_CAPACITY),
            self.queue_metrics.payload_bytes.snapshot(
                "websocket_outbound_payload_bytes",
                MAX_OUTBOUND_BUFFER_BYTES,
            ),
        ]
    }
}

fn prepare_observation_batch(
    frames: Vec<ObservationDeliver>,
) -> Result<Vec<Message>, OutboundMessageError> {
    if frames.len() > MAX_OUTBOUND_FRAME_BURST {
        return Err(OutboundMessageError::SlowConsumer);
    }
    let mut payload_bytes = 0usize;
    let mut messages = Vec::with_capacity(frames.len());
    for frame in frames {
        let message = prepare_body_message("observation.deliver", None, &frame)?;
        payload_bytes = payload_bytes
            .checked_add(outbound_message_payload_bytes(&message))
            .ok_or(OutboundMessageError::SlowConsumer)?;
        if payload_bytes > MAX_OUTBOUND_BUFFER_BYTES {
            return Err(OutboundMessageError::SlowConsumer);
        }
        messages.push(message);
    }
    Ok(messages)
}

fn prepare_live_push(
    frame: &ObservationDeliver,
    queued_frames: Arc<AtomicUsize>,
    queued_payload_bytes: Arc<AtomicUsize>,
    metrics: &LiveStreamQueueMetrics,
) -> Result<LivePush, OutboundMessageError> {
    let message = prepare_body_message("observation.deliver", None, frame)?;
    let payload_bytes = outbound_message_payload_bytes(&message);
    let Some(payload_reservation) = OutboundPayloadReservation::try_new(
        queued_payload_bytes,
        payload_bytes,
        &metrics.payload_bytes,
    ) else {
        return Err(OutboundMessageError::SlowConsumer);
    };
    let Some(frame_reservation) = OutboundFrameReservation::try_new(queued_frames, &metrics.frames)
    else {
        return Err(OutboundMessageError::SlowConsumer);
    };
    Ok(LivePush::Frame(LiveFramePush {
        #[cfg(test)]
        frame: frame.clone(),
        message,
        _frame_reservation: frame_reservation,
        _payload_reservation: payload_reservation,
    }))
}

async fn publish_live_frames(
    backend: Arc<dyn GatewayBackend>,
    registry: &LiveStreamRegistry,
    room_id: &str,
) {
    let _publication_guard = registry.publication_lock.lock().await;
    for snapshot in registry.snapshots_for_room(room_id) {
        let session = Arc::clone(&snapshot.session);
        let snapshot_room_id = snapshot.room_id.clone();
        let snapshot_member_id = snapshot.member_id.clone();
        let after_frame_seq = snapshot.last_delivered_frame_seq;
        let Ok(frames) = backend_call(Arc::clone(&backend), move |backend| {
            backend.live_observation_suffix(
                &session,
                &snapshot_room_id,
                &snapshot_member_id,
                after_frame_seq,
            )
        })
        .await
        else {
            registry.close(&snapshot.session_id, ErrorCode::StorageUnavailable);
            continue;
        };
        if frames.len() > LIVE_PUSH_CAPACITY {
            registry.queue_metrics.frames.note_backpressure();
            registry.close(&snapshot.session_id, ErrorCode::SlowConsumer);
            continue;
        }
        for frame in frames {
            if frame.room_id != snapshot.room_id
                || frame.member_id != snapshot.member_id
                || frame.frame_seq <= snapshot.last_delivered_frame_seq
            {
                registry.close(&snapshot.session_id, ErrorCode::Internal);
                break;
            }
            let frame_seq = frame.frame_seq;
            match prepare_live_push(
                &frame,
                Arc::clone(&snapshot.queued_frames),
                Arc::clone(&snapshot.queued_payload_bytes),
                &registry.queue_metrics,
            ) {
                Ok(push) => {
                    if snapshot.sender.try_send(push).is_err() {
                        registry.queue_metrics.frames.note_backpressure();
                        registry.close(&snapshot.session_id, ErrorCode::SlowConsumer);
                        break;
                    }
                    registry.mark_delivered(&snapshot.session_id, frame_seq);
                }
                Err(OutboundMessageError::SlowConsumer) => {
                    registry.close(&snapshot.session_id, ErrorCode::SlowConsumer);
                    break;
                }
                Err(OutboundMessageError::Internal) => {
                    registry.close(&snapshot.session_id, ErrorCode::Internal);
                    break;
                }
            }
        }
    }
}

fn should_kill_after_action_commit(
    configured_action_id: Option<&str>,
    action_id: &str,
    duplicate: bool,
) -> bool {
    configured_action_id == Some(action_id) && !duplicate
}

fn kill_after_action_commit_before_reply() -> ! {
    std::process::abort()
}

fn should_kill_after_activation_claim(
    configured_claim_id: Option<&str>,
    claim_id: &str,
    successful: bool,
    previously_triggered: bool,
) -> bool {
    configured_claim_id == Some(claim_id) && successful && !previously_triggered
}

fn activation_claim_boundary_was_triggered(claim_id: &str) -> bool {
    let triggered = ACTIVATION_CLAIM_BOUNDARY_TRIGGERED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut triggered = triggered
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    !triggered.insert(claim_id.to_owned())
}

fn kill_after_activation_claim_before_reply() -> ! {
    std::process::abort()
}

#[derive(Clone, Copy, Default)]
struct WebSocketAdmissionScope<'a> {
    target: Option<AdmissionTarget<'a>>,
    activation_operation: Option<&'static str>,
}

fn websocket_admission_scope(envelope: &VersionedEnvelope<Value>) -> WebSocketAdmissionScope<'_> {
    let activation_operation = match envelope.message_type.as_str() {
        "activation.claim" => Some("activation.claim"),
        "activation.renew" => Some("activation.renew"),
        "activation.release" => Some("activation.release"),
        "activation.complete" => Some("activation.complete"),
        _ => None,
    };
    let target = if matches!(
        envelope.message_type.as_str(),
        "room.attach" | "room.sync_ack" | "observation.ack" | "action.submit" | "activation.offer"
    ) {
        envelope.body.as_object().and_then(|body| {
            Some(AdmissionTarget {
                room_id: body.get("room_id")?.as_str()?,
                member_id: Some(body.get("member_id")?.as_str()?),
            })
        })
    } else {
        None
    };
    WebSocketAdmissionScope {
        target,
        activation_operation,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn dispatch_message(
    socket: &mut WebSocket,
    backend: Arc<dyn GatewayBackend>,
    live_streams: &LiveStreamRegistry,
    push_sender: &mpsc::Sender<LivePush>,
    close_sender: &watch::Sender<Option<ErrorCode>>,
    session: &Arc<GatewaySession>,
    envelope: VersionedEnvelope<Value>,
    live: &mut bool,
    attached_stream: &mut Option<AttachedStream>,
    runner: &mut RunnerConnectionState,
    runner_presence: &RunnerPresenceRegistry,
    active_runner_presence: &mut Option<ActiveRunnerPresence>,
    telemetry: Option<&telemetry::TelemetryHandle>,
    correlation: telemetry::CorrelationV1,
) -> Result<(), ()> {
    let VersionedEnvelope {
        message_type,
        message_id,
        request_id: explicit_request_id,
        body,
        ..
    } = envelope;
    let request_id = reply_request_id(explicit_request_id.as_ref(), &message_id);
    match message_type.as_str() {
        "runner.hello" => {
            if runner.mode != ClientMode::Runner || runner.ready {
                send_error(socket, request_id, ErrorCode::InvalidPayload, false).await?;
                return Ok(());
            }
            let request = decode_body::<RunnerHello>(body).map_err(|_| ())?;
            let presence_hello = request.clone();
            match backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| backend.runner_hello(&session, request)
            })
            .await
            {
                Ok(ready) => {
                    runner.ready = true;
                    runner.runner_id = Some(ready.runner_id.clone());
                    runner_presence.register(session.session_id(), &presence_hello);
                    *active_runner_presence = Some(ActiveRunnerPresence {
                        registry: runner_presence.clone(),
                        runner_id: ready.runner_id.clone(),
                        session_id: session.session_id().clone(),
                    });
                    record_admission_with_correlation(
                        telemetry,
                        telemetry::ReasonCodeV1::Accepted,
                        correlation,
                    );
                    send_body(socket, "runner.ready", request_id, ready).await?;
                }
                Err(error) => {
                    let reason = reason_for_backend_error(&error);
                    record_admission_with_correlation(telemetry, reason, correlation);
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                }
            }
        }
        "activation.offer" => {
            if runner.mode != ClientMode::Runner || !runner.ready {
                send_error(socket, request_id, ErrorCode::SyncBarrierMismatch, false).await?;
                return Ok(());
            }
            let request = decode_body::<ActivationOfferRequest>(body).map_err(|_| ())?;
            match backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| backend.activation_offers(&session, request)
            })
            .await
            {
                Ok(reply) => {
                    record_activation_with_correlation(
                        telemetry,
                        telemetry::ActivationPhaseV1::IntentCreated,
                        telemetry::ReasonCodeV1::Accepted,
                        correlation,
                    );
                    send_body(socket, "activation.offers", request_id, reply).await?;
                }
                Err(error) => {
                    record_activation_with_correlation(
                        telemetry,
                        telemetry::ActivationPhaseV1::IntentCreated,
                        reason_for_backend_error(&error),
                        correlation,
                    );
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                }
            }
        }
        "activation.claim" => {
            if runner.mode != ClientMode::Runner || !runner.ready {
                send_error(socket, request_id, ErrorCode::SyncBarrierMismatch, false).await?;
                return Ok(());
            }
            let request = decode_body::<ActivationClaim>(body).map_err(|_| ())?;
            let claim_id = request.claim_id.clone();
            let activation_id = request.activation_id.clone();
            let claim_runner_id = request.runner_id.clone();
            pause_for_process_crash_evidence("activation_lease", "before_commit", &claim_id);
            match backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| backend.activation_claim(&session, request)
            })
            .await
            {
                Ok(reply) => {
                    if reply.code == ActivationResultCode::Granted {
                        runner_presence.activation_started(&claim_runner_id, &activation_id);
                        pause_for_process_crash_evidence(
                            "activation_lease",
                            "after_commit_before_publication",
                            &claim_id,
                        );
                    }
                    record_activation_with_correlation(
                        telemetry,
                        telemetry::ActivationPhaseV1::Claimed,
                        telemetry::ReasonCodeV1::Accepted,
                        correlation,
                    );
                    let configured_claim_id =
                        std::env::var(KILL_AFTER_ACTIVATION_CLAIM_BEFORE_REPLY_ENV).ok();
                    let previously_triggered =
                        if configured_claim_id.as_deref() == Some(claim_id.as_str()) {
                            activation_claim_boundary_was_triggered(&claim_id)
                        } else {
                            false
                        };
                    if should_kill_after_activation_claim(
                        configured_claim_id.as_deref(),
                        &claim_id,
                        reply.code == ActivationResultCode::Granted,
                        previously_triggered,
                    ) {
                        tracing::warn!(
                            claim_id = %claim_id,
                            "opt-in test seam terminating after durable Activation claim"
                        );
                        kill_after_activation_claim_before_reply();
                    }
                    if reply.code == ActivationResultCode::Granted {
                        pause_for_process_crash_evidence(
                            "activation_lease",
                            "after_publication_before_reply",
                            &claim_id,
                        );
                    }
                    send_body(socket, "activation.claimed", request_id, reply).await?;
                }
                Err(error) => {
                    record_activation_with_correlation(
                        telemetry,
                        telemetry::ActivationPhaseV1::Claimed,
                        reason_for_backend_error(&error),
                        correlation,
                    );
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                }
            }
        }
        "activation.renew" | "activation.release" | "activation.complete" => {
            if runner.mode != ClientMode::Runner || !runner.ready {
                send_error(socket, request_id, ErrorCode::SyncBarrierMismatch, false).await?;
                return Ok(());
            }
            let request = decode_body::<ActivationLeaseOperation>(body).map_err(|_| ())?;
            let lease_activation_id = request.activation_id.clone();
            let lease_runner_id = request.runner_id.clone();
            let operation = message_type.clone();
            let result = backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| match operation.as_str() {
                    "activation.renew" => backend.activation_renew(&session, request),
                    "activation.release" => backend.activation_release(&session, request),
                    "activation.complete" => backend.activation_complete(&session, request),
                    _ => unreachable!("matched activation lease operation"),
                }
            })
            .await;
            let phase = match message_type.as_str() {
                "activation.renew" | "activation.release" => telemetry::ActivationPhaseV1::Leased,
                "activation.complete" => telemetry::ActivationPhaseV1::Completed,
                _ => unreachable!("matched activation lease operation"),
            };
            let response_type = match message_type.as_str() {
                "activation.renew" => "activation.renewed",
                "activation.release" => "activation.released",
                "activation.complete" => "activation.completed",
                _ => unreachable!("matched activation lease operation"),
            };
            match result {
                Ok(reply) => {
                    if matches!(
                        reply.code,
                        ActivationResultCode::Released | ActivationResultCode::Completed
                    ) {
                        runner_presence.activation_finished(&lease_runner_id, &lease_activation_id);
                    }
                    record_activation_with_correlation(
                        telemetry,
                        phase,
                        telemetry::ReasonCodeV1::Accepted,
                        correlation,
                    );
                    send_body(socket, response_type, request_id, reply).await?;
                }
                Err(error) => {
                    record_activation_with_correlation(
                        telemetry,
                        phase,
                        reason_for_backend_error(&error),
                        correlation,
                    );
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                }
            }
        }
        "room.attach" => {
            // A fresh attach must acknowledge its own synchronization; it cannot
            // inherit the previous stream's readiness evidence.
            live_streams.unregister(session.session_id().as_str());
            let request = decode_body::<RoomAttach>(body).map_err(|_| ())?;
            let reply = match backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| backend.attach(&session, request)
            })
            .await
            {
                Ok(reply) => reply,
                Err(error) => {
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        0,
                        correlation,
                    );
                    return Ok(());
                }
            };
            let AttachReply {
                attached,
                reset,
                frames,
            } = reply;
            let frame_count = frames.len();
            let prepared_frames = match prepare_observation_batch(frames) {
                Ok(prepared) => prepared,
                Err(error) => {
                    send_error(
                        socket,
                        request_id,
                        error.code(),
                        error == OutboundMessageError::SlowConsumer,
                    )
                    .await?;
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        frame_count,
                        correlation,
                    );
                    return Err(());
                }
            };
            *attached_stream = Some(AttachedStream {
                room_id: attached.room_id.clone(),
                member_id: attached.member_id.clone(),
            });
            record_frame_with_correlation(
                telemetry,
                telemetry::FrameDeliveryOutcomeV1::Queued,
                frame_count,
                correlation,
            );
            if send_body(socket, "room.attached", request_id, attached)
                .await
                .is_err()
            {
                record_frame_with_correlation(
                    telemetry,
                    telemetry::FrameDeliveryOutcomeV1::Failed,
                    0,
                    correlation,
                );
                return Err(());
            }
            if let Some(reset) = reset
                && send_body(socket, "projection.reset", None, reset)
                    .await
                    .is_err()
            {
                record_frame_with_correlation(
                    telemetry,
                    telemetry::FrameDeliveryOutcomeV1::Failed,
                    0,
                    correlation,
                );
                return Err(());
            }
            for message in prepared_frames {
                if send_websocket_message(socket.send(message), WEBSOCKET_SEND_TIMEOUT)
                    .await
                    .is_err()
                {
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        1,
                        correlation,
                    );
                    return Err(());
                }
                record_frame_with_correlation(
                    telemetry,
                    telemetry::FrameDeliveryOutcomeV1::Delivered,
                    1,
                    correlation,
                );
            }
        }
        "room.sync_ack" => {
            let request = decode_body::<RoomSyncAck>(body).map_err(|_| ())?;
            let sync_room_id = request.room_id.clone();
            let sync_member_id = request.member_id.clone();
            let through_frame_head = request.through_frame_head;
            let sync_result = {
                let _publication_guard = live_streams.publication_lock.lock().await;
                match backend_call(Arc::clone(&backend), {
                    let session = Arc::clone(session);
                    move |backend| backend.sync_ack(&session, request)
                })
                .await
                {
                    Ok(frames) => {
                        let frame_count = frames.len();
                        let registration_frame_seq = frames
                            .iter()
                            .map(|frame| frame.frame_seq)
                            .max()
                            .unwrap_or(through_frame_head);
                        match prepare_observation_batch(frames) {
                            Ok(prepared_frames) => {
                                let registration = if let Some(attached) = attached_stream.as_ref()
                                    && attached.room_id == sync_room_id
                                    && attached.member_id == sync_member_id
                                {
                                    live_streams.register(
                                        Arc::clone(session),
                                        sync_room_id.clone(),
                                        sync_member_id.clone(),
                                        registration_frame_seq,
                                        push_sender.clone(),
                                        close_sender.clone(),
                                    )
                                } else {
                                    Ok(())
                                };
                                registration
                                    .map(|()| (prepared_frames, frame_count))
                                    .map_err(SyncDeliveryError::Backend)
                            }
                            Err(error) => Err(SyncDeliveryError::Outbound { error, frame_count }),
                        }
                    }
                    Err(error) => Err(SyncDeliveryError::Backend(error)),
                }
            };
            let (prepared_frames, frame_count) = match sync_result {
                Ok(prepared) => prepared,
                Err(SyncDeliveryError::Backend(error)) => {
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        0,
                        correlation,
                    );
                    return Ok(());
                }
                Err(SyncDeliveryError::Outbound { error, frame_count }) => {
                    send_error(
                        socket,
                        request_id,
                        error.code(),
                        error == OutboundMessageError::SlowConsumer,
                    )
                    .await?;
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        frame_count,
                        correlation,
                    );
                    return Err(());
                }
            };
            record_frame_with_correlation(
                telemetry,
                telemetry::FrameDeliveryOutcomeV1::Queued,
                frame_count,
                correlation,
            );
            *live = true;
            for message in prepared_frames {
                if send_websocket_message(socket.send(message), WEBSOCKET_SEND_TIMEOUT)
                    .await
                    .is_err()
                {
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        1,
                        correlation,
                    );
                    return Err(());
                }
                record_frame_with_correlation(
                    telemetry,
                    telemetry::FrameDeliveryOutcomeV1::Delivered,
                    1,
                    correlation,
                );
            }
            if send_body(
                socket,
                "room.sync_acked",
                request_id,
                serde_json::json!({"through_frame_head": through_frame_head}),
            )
            .await
            .is_err()
            {
                record_frame_with_correlation(
                    telemetry,
                    telemetry::FrameDeliveryOutcomeV1::Failed,
                    0,
                    correlation,
                );
                return Err(());
            }
        }
        "observation.ack" => {
            let request = decode_body::<ObservationAck>(body).map_err(|_| ())?;
            let room_id = request.room_id.clone();
            let member_id = request.member_id.clone();
            let cursor = match backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| backend.observation_ack(&session, request)
            })
            .await
            {
                Ok(cursor) => cursor,
                Err(error) => {
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                    record_frame_with_correlation(
                        telemetry,
                        telemetry::FrameDeliveryOutcomeV1::Failed,
                        0,
                        correlation,
                    );
                    return Ok(());
                }
            };
            record_frame_with_correlation(
                telemetry,
                telemetry::FrameDeliveryOutcomeV1::Acknowledged,
                1,
                correlation,
            );
            send_body(
                socket,
                "observation.acked",
                request_id,
                serde_json::json!({
                    "room_id": room_id,
                    "member_id": member_id,
                    "cursor": cursor,
                }),
            )
            .await?;
        }
        "action.submit" => {
            let request = decode_body::<ActionSubmit>(body).map_err(|_| ())?;
            let action_id = request.action_id.clone();
            let action_room_id = request.room_id.clone();
            if request.validate_bounds().is_err() {
                send_error(socket, request_id, ErrorCode::InvalidPayload, false).await?;
                record_admission_with_correlation(
                    telemetry,
                    telemetry::ReasonCodeV1::Invalid,
                    correlation,
                );
                return Ok(());
            }
            if !*live {
                send_error(socket, request_id, ErrorCode::SyncBarrierMismatch, true).await?;
                record_admission_with_correlation(
                    telemetry,
                    telemetry::ReasonCodeV1::Invalid,
                    correlation,
                );
                return Ok(());
            }
            pause_for_process_crash_evidence("action", "before_commit", &action_id);
            let reply = match backend_call(Arc::clone(&backend), {
                let session = Arc::clone(session);
                move |backend| backend.action(&session, request)
            })
            .await
            {
                Ok(reply) => reply,
                Err(error) => {
                    send_error(socket, request_id, error.code(), is_retryable(&error)).await?;
                    let reason = reason_for_backend_error(&error);
                    record_admission_with_correlation(telemetry, reason, correlation);
                    record_commit_with_correlation(telemetry, reason, correlation);
                    return Ok(());
                }
            };
            match reply {
                ActionReply::Accepted(reply) => {
                    pause_for_process_crash_evidence(
                        "action",
                        "after_commit_before_publication",
                        &action_id,
                    );
                    record_admission_with_correlation(
                        telemetry,
                        telemetry::ReasonCodeV1::Accepted,
                        correlation,
                    );
                    record_commit_with_correlation(
                        telemetry,
                        telemetry::ReasonCodeV1::Accepted,
                        correlation,
                    );
                    let configured_action_id =
                        std::env::var(KILL_AFTER_ACTION_COMMIT_BEFORE_REPLY_ENV).ok();
                    if should_kill_after_action_commit(
                        configured_action_id.as_deref(),
                        &action_id,
                        reply.duplicate,
                    ) {
                        tracing::warn!(
                            action_id = %action_id,
                            "opt-in test seam terminating after durable Action commit"
                        );
                        kill_after_action_commit_before_reply();
                    }
                    publish_live_frames(Arc::clone(&backend), live_streams, &action_room_id).await;
                    pause_for_process_crash_evidence(
                        "action",
                        "after_publication_before_reply",
                        &action_id,
                    );
                    send_body(socket, "action.accepted", request_id, reply).await?;
                }
                ActionReply::Rejected(reply) => {
                    record_admission_with_correlation(
                        telemetry,
                        telemetry::ReasonCodeV1::Rejected,
                        correlation,
                    );
                    record_commit_with_correlation(
                        telemetry,
                        telemetry::ReasonCodeV1::Rejected,
                        correlation,
                    );
                    send_body(socket, "action.rejected", request_id, reply).await?;
                }
            }
        }
        "client.pong" => {}
        _ => send_error(socket, request_id, ErrorCode::InvalidEnvelope, false).await?,
    }
    Ok(())
}

fn reply_request_id<'a>(
    explicit_request_id: Option<&'a UlidString>,
    message_id: &'a UlidString,
) -> Option<&'a UlidString> {
    explicit_request_id.or(Some(message_id))
}

fn decode_body<T: DeserializeOwned>(body: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(body)
}

fn record_admission_with_correlation(
    telemetry: Option<&telemetry::TelemetryHandle>,
    reason: telemetry::ReasonCodeV1,
    correlation: telemetry::CorrelationV1,
) {
    record_telemetry_with_correlation(
        telemetry,
        telemetry::EventDetailsV1::Admission {
            outcome: match reason {
                telemetry::ReasonCodeV1::Accepted => telemetry::AdmissionOutcomeV1::Accepted,
                telemetry::ReasonCodeV1::Conflict => telemetry::AdmissionOutcomeV1::Conflict,
                telemetry::ReasonCodeV1::Busy => telemetry::AdmissionOutcomeV1::Busy,
                telemetry::ReasonCodeV1::Unauthorized => {
                    telemetry::AdmissionOutcomeV1::Unauthorized
                }
                _ => telemetry::AdmissionOutcomeV1::Rejected,
            },
        },
        reason,
        Vec::new(),
        correlation,
    );
}

fn record_commit_with_correlation(
    telemetry: Option<&telemetry::TelemetryHandle>,
    reason: telemetry::ReasonCodeV1,
    correlation: telemetry::CorrelationV1,
) {
    record_telemetry_with_correlation(
        telemetry,
        telemetry::EventDetailsV1::CommitOutcome {
            outcome: match reason {
                telemetry::ReasonCodeV1::Accepted => telemetry::CommitOutcomeV1::Committed,
                telemetry::ReasonCodeV1::Conflict => telemetry::CommitOutcomeV1::Conflict,
                telemetry::ReasonCodeV1::CommitIndeterminate => {
                    telemetry::CommitOutcomeV1::Indeterminate
                }
                telemetry::ReasonCodeV1::RetryExhausted => {
                    telemetry::CommitOutcomeV1::RetryableKnownAbsent
                }
                telemetry::ReasonCodeV1::Rejected | telemetry::ReasonCodeV1::Invalid => {
                    telemetry::CommitOutcomeV1::Rejected
                }
                _ => telemetry::CommitOutcomeV1::Fault,
            },
        },
        reason,
        Vec::new(),
        correlation,
    );
}

fn record_frame_with_correlation(
    telemetry: Option<&telemetry::TelemetryHandle>,
    outcome: telemetry::FrameDeliveryOutcomeV1,
    batch_size: usize,
    correlation: telemetry::CorrelationV1,
) {
    record_telemetry_with_correlation(
        telemetry,
        telemetry::EventDetailsV1::FrameDelivery { outcome },
        match outcome {
            telemetry::FrameDeliveryOutcomeV1::Queued
            | telemetry::FrameDeliveryOutcomeV1::Delivered
            | telemetry::FrameDeliveryOutcomeV1::Acknowledged => telemetry::ReasonCodeV1::Accepted,
            telemetry::FrameDeliveryOutcomeV1::ResetRequired => {
                telemetry::ReasonCodeV1::RecoveryRequired
            }
            telemetry::FrameDeliveryOutcomeV1::Failed => telemetry::ReasonCodeV1::CollectorSlow,
        },
        vec![("batch_size".to_owned(), batch_size.to_string())],
        correlation,
    );
}

fn record_telemetry_with_correlation(
    telemetry: Option<&telemetry::TelemetryHandle>,
    details: telemetry::EventDetailsV1,
    reason: telemetry::ReasonCodeV1,
    attributes: Vec<(String, String)>,
    correlation: telemetry::CorrelationV1,
) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let _ = telemetry.submit(telemetry::TelemetryDraftV1 {
        event: match details {
            telemetry::EventDetailsV1::Admission { .. } => telemetry::EventKindV1::Admission,
            telemetry::EventDetailsV1::CommitOutcome { .. } => {
                telemetry::EventKindV1::CommitOutcome
            }
            telemetry::EventDetailsV1::Timer { .. } => telemetry::EventKindV1::Timer,
            telemetry::EventDetailsV1::FrameDelivery { .. } => {
                telemetry::EventKindV1::FrameDelivery
            }
            telemetry::EventDetailsV1::Activation { .. } => telemetry::EventKindV1::Activation,
            telemetry::EventDetailsV1::Recovery { .. } => telemetry::EventKindV1::Recovery,
            telemetry::EventDetailsV1::Migration { .. } => telemetry::EventKindV1::Migration,
            telemetry::EventDetailsV1::StorageDiagnostic { .. } => {
                telemetry::EventKindV1::StorageDiagnostic
            }
        },
        reason,
        correlation,
        adapter: None,
        details,
        observed_at_ms: None,
        attributes,
    });
}

fn record_timer_with_correlation(
    telemetry: Option<&telemetry::TelemetryHandle>,
    reason: telemetry::ReasonCodeV1,
    correlation: telemetry::CorrelationV1,
) {
    let phase = match reason {
        telemetry::ReasonCodeV1::Accepted => telemetry::TimerPhaseV1::Fired,
        telemetry::ReasonCodeV1::Busy
        | telemetry::ReasonCodeV1::Timeout
        | telemetry::ReasonCodeV1::StorageUnavailable
        | telemetry::ReasonCodeV1::CommitIndeterminate => telemetry::TimerPhaseV1::Retried,
        _ => telemetry::TimerPhaseV1::Obsolete,
    };
    record_telemetry_with_correlation(
        telemetry,
        telemetry::EventDetailsV1::Timer { phase },
        reason,
        Vec::new(),
        correlation,
    );
}

fn record_activation_with_correlation(
    telemetry: Option<&telemetry::TelemetryHandle>,
    phase: telemetry::ActivationPhaseV1,
    reason: telemetry::ReasonCodeV1,
    correlation: telemetry::CorrelationV1,
) {
    record_telemetry_with_correlation(
        telemetry,
        telemetry::EventDetailsV1::Activation { phase },
        reason,
        Vec::new(),
        correlation,
    );
}

fn traceparent_correlation(headers: &HeaderMap) -> telemetry::CorrelationV1 {
    headers
        .get("traceparent")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| telemetry::TraceParentV1::parse(value).ok())
        .map_or_else(
            telemetry::CorrelationV1::none,
            telemetry::CorrelationV1::from_traceparent,
        )
}

fn reason_for_backend_error(error: &BackendError) -> telemetry::ReasonCodeV1 {
    match error {
        BackendError::Forbidden => telemetry::ReasonCodeV1::Unauthorized,
        BackendError::NotFound
        | BackendError::ActivityPackRevisionUnavailable
        | BackendError::Rejected
        | BackendError::InvalidResult => telemetry::ReasonCodeV1::Invalid,
        BackendError::Busy => telemetry::ReasonCodeV1::Busy,
        BackendError::Conflict | BackendError::WrongPhase => telemetry::ReasonCodeV1::Conflict,
        BackendError::StorageUnavailable => telemetry::ReasonCodeV1::StorageUnavailable,
        BackendError::Indeterminate => telemetry::ReasonCodeV1::CommitIndeterminate,
        BackendError::RoomFaulted | BackendError::RoomQuarantined => {
            telemetry::ReasonCodeV1::RecoveryRequired
        }
    }
}

fn is_retryable(error: &BackendError) -> bool {
    matches!(
        error,
        BackendError::RoomFaulted
            | BackendError::Indeterminate
            | BackendError::Busy
            | BackendError::StorageUnavailable
    )
}

async fn send_error(
    socket: &mut WebSocket,
    request_id: Option<&UlidString>,
    code: ErrorCode,
    retryable: bool,
) -> Result<(), ()> {
    send_body(
        socket,
        "error",
        request_id,
        worldstream_protocol::ProtocolErrorBody {
            code,
            message: safe_message(code).to_owned(),
            retryable,
            details: None,
        },
    )
    .await
}

async fn send_body<T: Serialize>(
    socket: &mut WebSocket,
    message_type: &str,
    request_id: Option<&UlidString>,
    body: T,
) -> Result<(), ()> {
    let message = prepare_body_message(message_type, request_id, body).map_err(|_| ())?;
    send_websocket_message(socket.send(message), WEBSOCKET_SEND_TIMEOUT).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutboundMessageError {
    Internal,
    SlowConsumer,
}

enum SyncDeliveryError {
    Backend(BackendError),
    Outbound {
        error: OutboundMessageError,
        frame_count: usize,
    },
}

impl OutboundMessageError {
    const fn code(self) -> ErrorCode {
        match self {
            Self::Internal => ErrorCode::Internal,
            Self::SlowConsumer => ErrorCode::SlowConsumer,
        }
    }
}

fn prepare_body_message<T: Serialize>(
    message_type: &str,
    request_id: Option<&UlidString>,
    body: T,
) -> Result<Message, OutboundMessageError> {
    let Some(message_id) = next_ulid() else {
        return Err(OutboundMessageError::Internal);
    };
    let envelope = ProtocolEnvelope {
        protocol: worldstream_protocol::PROTOCOL_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id,
        request_id: request_id.cloned(),
        body,
    };
    let text = serde_json::to_string(&envelope).map_err(|_| OutboundMessageError::Internal)?;
    if text.len() > worldstream_protocol::MAX_MESSAGE_BYTES {
        return Err(OutboundMessageError::SlowConsumer);
    }
    Ok(Message::Text(text.into()))
}

fn outbound_message_payload_bytes(message: &Message) -> usize {
    match message {
        Message::Text(text) => text.len(),
        Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
        Message::Close(Some(frame)) => frame.reason.len(),
        Message::Close(None) => 0,
    }
}

async fn send_websocket_message<F, E>(send: F, timeout: Duration) -> Result<(), ()>
where
    F: Future<Output = Result<(), E>>,
{
    match tokio::time::timeout(timeout, send).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

/// Liveness response that deliberately contains no storage status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HealthResponse {
    /// Event-loop/process status.
    pub status: &'static str,
}

/// Manifest-backed process build details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProductBuild {
    /// Product contract version from the manifest.
    pub product: String,
    /// Binary package name.
    pub binary: &'static str,
    /// Build version, intentionally sourced from the manifest contract.
    pub build_version: String,
    /// Optional build-system source revision, never guessed at runtime.
    pub source_revision: &'static str,
}

/// Truthful engine state selected and verified by runtime startup.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EngineVersion {
    /// Startup-selected profile.
    pub profile: StorageProfile,
    /// Initialization state.
    pub status: &'static str,
    /// Exact engine identity once verified; absent before runtime startup.
    pub exact_identity: Option<String>,
}

impl EngineVersion {
    fn not_initialized(profile: StorageProfile) -> Self {
        Self {
            profile,
            status: "not_initialized",
            exact_identity: None,
        }
    }

    fn verified(profile: StorageProfile, exact_identity: String) -> Self {
        Self {
            profile,
            status: "verified",
            exact_identity: Some(exact_identity),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ReadyResponse {
    status: &'static str,
}

/// Complete operator `/version` response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VersionResponse {
    /// Product and build identity.
    pub product_build: ProductBuild,
    /// Wire version.
    pub wire: String,
    /// Config version.
    pub config: u32,
    /// Storage schema version.
    pub storage_schema: u32,
    /// Core state schema identifier.
    pub core_schema_version: String,
    /// Hash suite identifier.
    pub hash_suite: String,
    /// Embedded manifest identity and pinned engines.
    pub manifest: CompatibilitySummary,
    /// Selected engine and the runtime-verified identity, when startup reached it.
    pub engine: EngineVersion,
}

/// Process-shell construction failures.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Embedded compatibility validation failed.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// Cargo package and canonical manifest product versions diverged.
    #[error("binary package version {package} differs from manifest product {manifest}")]
    BuildVersionMismatch {
        package: &'static str,
        manifest: String,
    },
    /// The process-local fingerprint key could not be generated.
    #[error("gateway rate limiter initialization failed")]
    RateLimiterInitialization,
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::str::FromStr;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use axum::{
        body::Body,
        extract::ws::Message,
        http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use worldstream_core::{
        AuthorityBootstrapV1, AuthorityCheckedAt, AuthorityV1, CapabilityBearerV1, CapabilityId,
        PrincipalKindV1, builtin_worldstream_registry, counter_v1_digest, counter_v2_digest,
        counter_v3_digest, counter_v4_digest,
    };
    use worldstream_protocol::{
        AccessMode, ActionSubmit, BEARER_WIRE_PREFIX, BearerWireV1,
        BrowserWebSocketTicketIssueResponse, ClientHello, CreateMember, CreateRoomRequest,
        CreateRoomResponse, ErrorCode, ErrorEnvelope, LobbyLaunchResponse, ObservationAck,
        ObservationDeliver, OperatorRunnerConnectionV1, OperatorRunnerPresenceV1, PackReference,
        PrincipalKind, Projection, ProjectionResponse, ReplayResponse, RoomAttach, RoomHead,
        RoomSyncAck, RunnerHello, ServerWelcome, TimerFireResponse, UlidString,
    };
    use worldstream_runtime::{EffectiveConfig, StorageProfile};
    use worldstream_sqlite::SqliteRoomStore;

    use super::telemetry::{
        CommitOutcomeV1, EventDetailsV1, ExportError, MAX_EVENT_BYTES, TelemetryEventV1,
        TelemetryExporter, TelemetryRuntime,
    };

    #[tokio::test]
    async fn router_ip_admission_returns_one_safe_429_after_the_exact_burst() {
        let mut state = super::OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"));
        state.rate_limiter = super::rate_limit::GatewayRateLimiter::fixed_for_integration_tests(2);
        let app = super::operator_router(state);
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/healthz")
                        .body(Body::empty())
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error envelope: {error}"));
        assert_eq!(error.error.code, ErrorCode::RateLimited);
        assert!(error.error.retryable);
        assert!(error.error.details.is_none());
        assert!(!String::from_utf8_lossy(&body).contains("unattributed"));
    }

    #[test]
    fn websocket_target_classification_uses_composite_membership_identity() {
        let envelope: worldstream_protocol::VersionedEnvelope<serde_json::Value> =
            serde_json::from_value(serde_json::json!({
                "protocol": "0.1",
                "type": "action.submit",
                "message_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                "body": {
                    "room_id": "room-private",
                    "member_id": "member-private"
                }
            }))
            .unwrap_or_else(|error| unreachable!("envelope: {error}"));
        let scope = super::websocket_admission_scope(&envelope);
        let target = scope
            .target
            .unwrap_or_else(|| unreachable!("targeted message"));
        assert_eq!(target.room_id, "room-private");
        assert_eq!(target.member_id, Some("member-private"));
        assert!(scope.activation_operation.is_none());

        let mut untargeted = envelope;
        untargeted.message_type = "client.pong".to_owned();
        let scope = super::websocket_admission_scope(&untargeted);
        assert!(scope.target.is_none());
        assert!(scope.activation_operation.is_none());

        for operation in [
            "activation.claim",
            "activation.renew",
            "activation.release",
            "activation.complete",
        ] {
            untargeted.message_type = operation.to_owned();
            let scope = super::websocket_admission_scope(&untargeted);
            assert!(scope.target.is_none());
            assert_eq!(scope.activation_operation, Some(operation));
        }
    }

    #[test]
    fn kill_boundary_matches_only_one_non_duplicate_action() {
        assert!(super::should_kill_after_action_commit(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
            false,
        ));
        assert!(!super::should_kill_after_action_commit(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
            true,
        ));
        assert!(!super::should_kill_after_action_commit(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            "01ARZ3NDEKTSV4RRFFQ69G5FC7",
            false,
        ));
        assert!(!super::should_kill_after_action_commit(
            None,
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
            false,
        ));
    }

    #[test]
    fn lost_claim_boundary_matches_only_first_successful_exact_claim() {
        assert!(super::should_kill_after_activation_claim(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
            "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            true,
            false,
        ));
        assert!(!super::should_kill_after_activation_claim(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
            "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            false,
            false,
        ));
        assert!(!super::should_kill_after_activation_claim(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
            "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            true,
            true,
        ));
        assert!(!super::should_kill_after_activation_claim(
            Some("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
            "01ARZ3NDEKTSV4RRFFQ69G5FAX",
            true,
            false,
        ));
        assert!(!super::should_kill_after_activation_claim(
            None,
            "01ARZ3NDEKTSV4RRFFQ69G5FAW",
            true,
            false,
        ));
    }

    #[test]
    fn process_crash_boundary_requires_the_exact_four_part_opt_in() {
        let enabled = Some(super::CRASH_EVIDENCE_ENABLE_VALUE);
        let match_id = "01ARZ3NDEKTSV4RRFFQ69G5FC6";
        assert!(super::crash_evidence_configuration_matches(
            enabled,
            Some("action:after_commit_before_publication"),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            "action",
            "after_commit_before_publication",
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
        ));
        assert!(!super::crash_evidence_configuration_matches(
            None,
            Some("action:after_commit_before_publication"),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            "action",
            "after_commit_before_publication",
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
        ));
        assert!(!super::crash_evidence_configuration_matches(
            enabled,
            Some("action:after_publication_before_reply"),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC6"),
            "action",
            "after_commit_before_publication",
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
        ));
        assert!(!super::crash_evidence_configuration_matches(
            enabled,
            Some("action:after_commit_before_publication"),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FC7"),
            "action",
            "after_commit_before_publication",
            "01ARZ3NDEKTSV4RRFFQ69G5FC6",
        ));
        assert_eq!(
            super::crash_publication_contract("room_create"),
            "telemetry_only_no_preexisting_room_observer"
        );
        assert_eq!(
            super::crash_publication_contract("action"),
            "telemetry_and_live_room_frames"
        );
        assert_eq!(
            super::crash_publication_contract("timer"),
            "telemetry_and_live_room_frames"
        );
        assert_eq!(
            super::crash_publication_contract("activation_lease"),
            "activation_telemetry_no_room_frame"
        );
        for operation in ["room_create", "action", "timer", "activation_lease"] {
            for target in [
                "after_commit_before_publication",
                "after_publication_before_reply",
            ] {
                let configured = format!("{operation}:{target}");
                assert!(super::crash_evidence_point_is_valid(&configured));
                assert!(super::crash_evidence_match_id_is_valid(match_id));
                assert!(!super::crash_evidence_configuration_matches(
                    enabled,
                    Some(&configured),
                    Some(match_id),
                    operation,
                    "before_commit",
                    match_id,
                ));
                assert!(super::crash_evidence_configuration_matches(
                    enabled,
                    Some(&configured),
                    Some(match_id),
                    operation,
                    target,
                    match_id,
                ));
            }
        }
        assert!(!super::crash_evidence_point_is_valid("action:after_reply"));
        assert!(!super::crash_evidence_match_id_is_valid(""));
    }
    use super::{
        BROWSER_ADMISSION_CLOSE_REASON, BROWSER_TICKET_PREFIX, BROWSER_TICKET_TTL,
        BROWSER_TICKET_WIRE_LENGTH, BUILD_REVISION, BrowserTicketError, BrowserTicketStore,
        GatewayBackend, GatewaySession, MemberCapabilityIssueResponse, OperatorState,
        RunnerCapabilityIssueResponse, RuntimeReadiness, SqliteGatewayBackend, VersionResponse,
        authenticated_session, bearer, browser_admission_close_message, browser_origin, next_ulid,
        operator_router, receive_browser_ticket, record_activation_with_correlation,
        record_timer_with_correlation, telemetry, websocket_origin, welcome_matches_session,
    };

    #[derive(Clone, Default)]
    struct RecordingExporter {
        events: Arc<Mutex<Vec<TelemetryEventV1>>>,
    }

    impl TelemetryExporter for RecordingExporter {
        fn export(&self, batch: &[TelemetryEventV1]) -> Result<(), ExportError> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(batch);
            Ok(())
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct FailingExporter;

    impl TelemetryExporter for FailingExporter {
        fn export(&self, _: &[TelemetryEventV1]) -> Result<(), ExportError> {
            Err(ExportError::Unavailable)
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct SuccessfulCreateBackend;

    impl super::GatewayBackend for SuccessfulCreateBackend {
        fn admission_principal(
            &self,
            _: &super::GatewaySession,
        ) -> Result<String, super::BackendError> {
            Ok("test-principal".to_owned())
        }

        fn activity_pack_catalog(
            &self,
            _: &super::GatewaySession,
        ) -> Result<worldstream_protocol::ActivityPackCatalogResponse, super::BackendError>
        {
            let registry = builtin_worldstream_registry()
                .map_err(|_| super::BackendError::StorageUnavailable)?;
            Ok(super::activity_pack_catalog_from_registry(&registry))
        }

        fn activity_pack_revision(
            &self,
            _: &super::GatewaySession,
            revision_digest: &str,
        ) -> Result<worldstream_protocol::ActivityPackCatalogRevisionResponse, super::BackendError>
        {
            let registry = builtin_worldstream_registry()
                .map_err(|_| super::BackendError::StorageUnavailable)?;
            super::activity_pack_revision_from_registry(&registry, revision_digest)
        }

        fn authorize_operator_runner_presence(
            &self,
            _: &super::GatewaySession,
        ) -> Result<(), super::BackendError> {
            Ok(())
        }

        fn hello(
            &self,
            _: &super::GatewaySession,
            _: &ClientHello,
        ) -> Result<ServerWelcome, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn create_room(
            &self,
            _: &super::GatewaySession,
            _: CreateRoomRequest,
        ) -> Result<CreateRoomResponse, super::BackendError> {
            Ok(CreateRoomResponse {
                room_id: "room".to_owned(),
                member_ids: Vec::new(),
                room_head: RoomHead {
                    room_id: "room".to_owned(),
                    room_seq: 0,
                    genesis_or_transition_hash: "hash".to_owned(),
                    core_schema_version: "schema".to_owned(),
                    pack_digest: "digest".to_owned(),
                    core_state_hash: "hash".to_owned(),
                    activity_state_hash: "hash".to_owned(),
                    authoritative_state_hash: "hash".to_owned(),
                },
            })
        }

        fn launch_lobby(
            &self,
            _: &super::GatewaySession,
            room_id: &str,
            request: worldstream_protocol::LobbyLaunchRequest,
        ) -> Result<worldstream_protocol::LobbyLaunchResponse, super::BackendError> {
            if room_id == "wrong-phase" {
                return Err(super::BackendError::WrongPhase);
            }
            if room_id == "conflict" {
                return Err(super::BackendError::Conflict);
            }
            Ok(worldstream_protocol::LobbyLaunchResponse {
                room_id: room_id.to_owned(),
                input_id: request.input_id,
                transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FC7".to_owned(),
                room_head: RoomHead {
                    room_id: room_id.to_owned(),
                    room_seq: 1,
                    genesis_or_transition_hash: "hash".to_owned(),
                    core_schema_version: "schema".to_owned(),
                    pack_digest: "digest".to_owned(),
                    core_state_hash: "hash".to_owned(),
                    activity_state_hash: "hash".to_owned(),
                    authoritative_state_hash: "hash".to_owned(),
                },
                duplicate: room_id == "duplicate",
            })
        }

        fn projection(
            &self,
            _: &super::GatewaySession,
            _: &str,
        ) -> Result<ProjectionResponse, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn replay(
            &self,
            _: &super::GatewaySession,
            room_id: &str,
            at_room_seq: u64,
        ) -> Result<ReplayResponse, super::BackendError> {
            Ok(ReplayResponse {
                room_id: room_id.to_owned(),
                pack: PackReference {
                    id: "counter".to_owned(),
                    version: "2".to_owned(),
                    digest: "digest".to_owned(),
                },
                requested_room_seq: at_room_seq,
                room_head: RoomHead {
                    room_id: room_id.to_owned(),
                    room_seq: at_room_seq,
                    genesis_or_transition_hash: "genesis-hash".to_owned(),
                    core_schema_version: "schema".to_owned(),
                    pack_digest: "digest".to_owned(),
                    core_state_hash: "core-hash".to_owned(),
                    activity_state_hash: "activity-hash".to_owned(),
                    authoritative_state_hash: "authority-hash".to_owned(),
                },
                projection: Projection {
                    core: serde_json::json!({"value": 1}),
                    activity: serde_json::json!({"status": "historical"}),
                    action_offers: Vec::new(),
                },
                projection_hash: "projection-hash".to_owned(),
                verification: "verified".to_owned(),
                room_health: "healthy".to_owned(),
                integrity_generation: 1,
            })
        }

        fn attach(
            &self,
            _: &super::GatewaySession,
            _: RoomAttach,
        ) -> Result<super::AttachReply, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn sync_ack(
            &self,
            _: &super::GatewaySession,
            _: RoomSyncAck,
        ) -> Result<Vec<ObservationDeliver>, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn live_observation_suffix(
            &self,
            _: &super::GatewaySession,
            room_id: &str,
            member_id: &str,
            after_frame_seq: u64,
        ) -> Result<Vec<ObservationDeliver>, super::BackendError> {
            let next_frame_seq = if room_id == "timer-publication" { 2 } else { 1 };
            let frame = ObservationDeliver {
                room_id: room_id.to_owned(),
                member_id: member_id.to_owned(),
                frame_seq: next_frame_seq,
                cause_room_seq: next_frame_seq,
                frame_kind: "transition".to_owned(),
                observation_schema: "counter/v1".to_owned(),
                observation: serde_json::json!({"value": 1}),
                frame_payload_hash: "hash".to_owned(),
            };
            if room_id == "frame-overflow" {
                return Ok((1..=257)
                    .map(|frame_seq| ObservationDeliver {
                        frame_seq,
                        ..frame.clone()
                    })
                    .collect());
            }
            if room_id == "byte-overflow" {
                return Ok((1..=17)
                    .map(|frame_seq| ObservationDeliver {
                        frame_seq,
                        observation: serde_json::json!({
                            "padding": "x".repeat(256 * 1024),
                        }),
                        ..frame.clone()
                    })
                    .collect());
            }
            if after_frame_seq >= next_frame_seq {
                return Ok(Vec::new());
            }
            Ok(vec![frame])
        }

        fn observation_ack(
            &self,
            _: &super::GatewaySession,
            _: ObservationAck,
        ) -> Result<Option<u64>, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn action(
            &self,
            _: &super::GatewaySession,
            _: ActionSubmit,
        ) -> Result<super::ActionReply, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn fire_timer(
            &self,
            _: &super::GatewaySession,
            room_id: &str,
            request: worldstream_protocol::TimerFireRequest,
        ) -> Result<TimerFireResponse, super::BackendError> {
            Ok(TimerFireResponse {
                room_id: room_id.to_owned(),
                timer_id: request.timer_id,
                generation: request.generation,
                transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ0".to_owned(),
                room_head: RoomHead {
                    room_id: room_id.to_owned(),
                    room_seq: if room_id == "timer-publication" { 2 } else { 1 },
                    genesis_or_transition_hash: "hash".to_owned(),
                    core_schema_version: "schema".to_owned(),
                    pack_digest: "digest".to_owned(),
                    core_state_hash: "hash".to_owned(),
                    activity_state_hash: "hash".to_owned(),
                    authoritative_state_hash: "hash".to_owned(),
                },
                duplicate: false,
            })
        }

        fn operator_room_inventory(
            &self,
            _: &super::GatewaySession,
            request: worldstream_protocol::OperatorRoomInventoryRequest,
        ) -> Result<worldstream_protocol::OperatorRoomInventoryPage, super::BackendError> {
            let first = operator_room_fixture("01ARZ3NDEKTSV4RRFFQ69G5FQ0", 1);
            let second = operator_room_fixture("01ARZ3NDEKTSV4RRFFQ69G5FQ1", 2);
            match (request.after_room_id.as_deref(), request.limit) {
                (None, 1) => Ok(worldstream_protocol::OperatorRoomInventoryPage {
                    rooms: vec![first],
                    next_after_room_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FQ0".to_owned()),
                }),
                (Some("01ARZ3NDEKTSV4RRFFQ69G5FQ0"), 1) => {
                    Ok(worldstream_protocol::OperatorRoomInventoryPage {
                        rooms: vec![second],
                        next_after_room_id: None,
                    })
                }
                (Some("01ARZ3NDEKTSV4RRFFQ69G5FQ1"), 1) => {
                    Ok(worldstream_protocol::OperatorRoomInventoryPage {
                        rooms: Vec::new(),
                        next_after_room_id: None,
                    })
                }
                _ => Err(super::BackendError::Rejected),
            }
        }

        fn operator_room_detail(
            &self,
            _: &super::GatewaySession,
            room_id: &str,
        ) -> Result<worldstream_protocol::OperatorRoomSummary, super::BackendError> {
            match room_id {
                "01ARZ3NDEKTSV4RRFFQ69G5FQ0" => Ok(operator_room_fixture(room_id, 1)),
                "01ARZ3NDEKTSV4RRFFQ69G5FQ1" => Ok(operator_room_fixture(room_id, 2)),
                _ => Err(super::BackendError::NotFound),
            }
        }

        fn operator_activation_status(
            &self,
            _: &super::GatewaySession,
            room_id: &str,
            member_id: &str,
        ) -> Result<worldstream_protocol::OperatorActivationStatusV1, super::BackendError> {
            if member_id != "01ARZ3NDEKTSV4RRFFQ69G5FQ1" {
                return Err(super::BackendError::NotFound);
            }
            match room_id {
                "01ARZ3NDEKTSV4RRFFQ69G5FQ3" => {
                    return Err(super::BackendError::StorageUnavailable);
                }
                "01ARZ3NDEKTSV4RRFFQ69G5FQ4" => return Err(super::BackendError::Forbidden),
                "01ARZ3NDEKTSV4RRFFQ69G5FQ0" => {}
                _ => return Err(super::BackendError::NotFound),
            }
            Ok(worldstream_protocol::OperatorActivationStatusV1 {
                version: "worldstream/operator-activation-status/v1".to_owned(),
                waiting: 2,
                leased: 1,
                observed_at_unix_ms: 1_777_000_000_000,
            })
        }

        fn operator_backup_profile(
            &self,
            _: &super::GatewaySession,
        ) -> Result<worldstream_protocol::OperatorBackupProfileStatus, super::BackendError>
        {
            Ok(worldstream_protocol::OperatorBackupProfileStatus {
                storage_profile: worldstream_protocol::OperatorBackupStorageProfile::SqliteBundled,
                storage_health: worldstream_protocol::OperatorBackupStorageHealth::Healthy,
                live_backup_supported: true,
                verification: worldstream_protocol::OperatorBackupVerification::Unavailable,
                freshness: worldstream_protocol::OperatorDataFreshness::Unavailable {
                    reason: "no_operation_selected".to_owned(),
                },
            })
        }

        fn operator_live_backup(
            &self,
            _: &super::GatewaySession,
            request: worldstream_protocol::OperatorLiveBackupPrepareRequest,
        ) -> Result<worldstream_protocol::OperatorLiveBackupStatus, super::BackendError> {
            Ok(worldstream_protocol::OperatorLiveBackupStatus {
                operation_id: request.operation_id,
                storage_profile: worldstream_protocol::OperatorBackupStorageProfile::SqliteBundled,
                storage_health: worldstream_protocol::OperatorBackupStorageHealth::Healthy,
                native_verification: worldstream_protocol::OperatorBackupVerification::Pass,
                semantic_verification: worldstream_protocol::OperatorBackupVerification::Pass,
                freshness: worldstream_protocol::OperatorDataFreshness::Fresh {
                    observed_at: "2026-08-23T20:00:00Z".to_owned(),
                },
                artifact: Some(worldstream_protocol::OperatorLiveBackupArtifactSummary {
                    artifact_name: "backup.sqlite3".to_owned(),
                    byte_length: 4096,
                    blake3_digest: "a".repeat(64),
                    semantic_digest: "b".repeat(64),
                }),
                unavailable_reason: None,
            })
        }
    }

    fn operator_room_fixture(
        room_id: &str,
        room_seq: u64,
    ) -> worldstream_protocol::OperatorRoomSummary {
        worldstream_protocol::OperatorRoomSummary {
            room_id: room_id.to_owned(),
            room_head: RoomHead {
                room_id: room_id.to_owned(),
                room_seq,
                genesis_or_transition_hash: "blake3:lineage".to_owned(),
                core_schema_version: "worldstream.core-room-state.v1".to_owned(),
                pack_digest: "blake3:pack".to_owned(),
                core_state_hash: "blake3:core".to_owned(),
                activity_state_hash: "blake3:activity".to_owned(),
                authoritative_state_hash: "blake3:authoritative".to_owned(),
            },
            pack: PackReference {
                id: "worldstream.counter".to_owned(),
                version: "2.0.0".to_owned(),
                digest: "blake3:pack".to_owned(),
            },
            integrity: worldstream_protocol::OperatorRoomIntegrity {
                status: worldstream_protocol::OperatorRoomIntegrityStatus::Healthy,
                generation: 1,
            },
            activity_phase: worldstream_protocol::OperatorActivityPhase::Unavailable {
                reason: "operator_membership_required".to_owned(),
            },
            freshness: worldstream_protocol::OperatorDataFreshness::Fresh {
                observed_at: "2026-08-23T20:00:00Z".to_owned(),
            },
        }
    }

    #[derive(Debug, Default)]
    struct AdmissionCountingBackend {
        create_calls: AtomicUsize,
        runner_capability_calls: AtomicUsize,
    }

    impl super::GatewayBackend for AdmissionCountingBackend {
        fn admission_principal(
            &self,
            _: &super::GatewaySession,
        ) -> Result<String, super::BackendError> {
            Ok("counted-principal".to_owned())
        }

        fn hello(
            &self,
            _: &super::GatewaySession,
            _: &ClientHello,
        ) -> Result<ServerWelcome, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn create_room(
            &self,
            _: &super::GatewaySession,
            _: CreateRoomRequest,
        ) -> Result<CreateRoomResponse, super::BackendError> {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            Err(super::BackendError::StorageUnavailable)
        }

        fn projection(
            &self,
            _: &super::GatewaySession,
            _: &str,
        ) -> Result<ProjectionResponse, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn attach(
            &self,
            _: &super::GatewaySession,
            _: RoomAttach,
        ) -> Result<super::AttachReply, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn sync_ack(
            &self,
            _: &super::GatewaySession,
            _: RoomSyncAck,
        ) -> Result<Vec<ObservationDeliver>, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn observation_ack(
            &self,
            _: &super::GatewaySession,
            _: ObservationAck,
        ) -> Result<Option<u64>, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn action(
            &self,
            _: &super::GatewaySession,
            _: ActionSubmit,
        ) -> Result<super::ActionReply, super::BackendError> {
            Err(super::BackendError::StorageUnavailable)
        }

        fn issue_runner_capability(
            &self,
            _: &super::GatewaySession,
            _: super::RunnerCapabilityIssueRequest,
        ) -> Result<RunnerCapabilityIssueResponse, super::BackendError> {
            self.runner_capability_calls.fetch_add(1, Ordering::SeqCst);
            Err(super::BackendError::StorageUnavailable)
        }
    }

    #[derive(Debug, Default)]
    struct NestedRuntimeBackend;

    impl super::GatewayBackend for NestedRuntimeBackend {
        fn admission_principal(
            &self,
            _: &super::GatewaySession,
        ) -> Result<String, super::BackendError> {
            Ok("audit-principal".to_owned())
        }

        fn hello(
            &self,
            session: &super::GatewaySession,
            _: &ClientHello,
        ) -> Result<ServerWelcome, super::BackendError> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|error| unreachable!("nested audit runtime: {error}"));
            runtime.block_on(async {});
            Ok(ServerWelcome {
                session_id: session.session_id().clone(),
                selected_protocol: "room.v1".to_owned(),
                server_version: "audit".to_owned(),
                heartbeat_interval_ms: 1_000,
                maximum_message_bytes: worldstream_protocol::MAX_MESSAGE_BYTES,
                authenticated_principal: worldstream_protocol::Principal {
                    principal_id: "audit".to_owned(),
                    kind: worldstream_protocol::PrincipalKind::Human,
                },
            })
        }

        fn create_room(
            &self,
            session: &super::GatewaySession,
            request: CreateRoomRequest,
        ) -> Result<CreateRoomResponse, super::BackendError> {
            super::UnavailableBackend.create_room(session, request)
        }

        fn projection(
            &self,
            session: &super::GatewaySession,
            room_id: &str,
        ) -> Result<ProjectionResponse, super::BackendError> {
            super::UnavailableBackend.projection(session, room_id)
        }

        fn attach(
            &self,
            session: &super::GatewaySession,
            request: RoomAttach,
        ) -> Result<super::AttachReply, super::BackendError> {
            super::UnavailableBackend.attach(session, request)
        }

        fn sync_ack(
            &self,
            session: &super::GatewaySession,
            request: RoomSyncAck,
        ) -> Result<Vec<ObservationDeliver>, super::BackendError> {
            super::UnavailableBackend.sync_ack(session, request)
        }

        fn live_observation_suffix(
            &self,
            session: &super::GatewaySession,
            room_id: &str,
            member_id: &str,
            after_frame_seq: u64,
        ) -> Result<Vec<ObservationDeliver>, super::BackendError> {
            super::UnavailableBackend.live_observation_suffix(
                session,
                room_id,
                member_id,
                after_frame_seq,
            )
        }

        fn observation_ack(
            &self,
            session: &super::GatewaySession,
            request: ObservationAck,
        ) -> Result<Option<u64>, super::BackendError> {
            super::UnavailableBackend.observation_ack(session, request)
        }

        fn action(
            &self,
            session: &super::GatewaySession,
            request: ActionSubmit,
        ) -> Result<super::ActionReply, super::BackendError> {
            super::UnavailableBackend.action(session, request)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn backend_call_runs_nested_runtime_backend_off_tokio_worker() {
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = Arc::new(GatewaySession::new(
            session_id,
            CapabilityBearerV1::from_bytes([0x55; 32]),
        ));
        let hello = ClientHello {
            client_name: "audit".to_owned(),
            client_version: "1".to_owned(),
            mode: worldstream_protocol::ClientMode::Participant,
            supported_protocols: vec!["room.v1".to_owned()],
            capabilities: vec!["cursor_ack".to_owned(), "projection_reset".to_owned()],
        };

        let result = super::backend_call(Arc::new(NestedRuntimeBackend), move |backend| {
            backend.hello(&session, &hello)
        })
        .await;

        let welcome = result.unwrap_or_else(|error| unreachable!("blocking backend call: {error}"));
        assert_eq!(welcome.selected_protocol, "room.v1");
    }

    #[tokio::test]
    async fn operator_room_inventory_pages_by_stable_room_identity() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/operator/rooms?limit=1")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(first.status(), StatusCode::OK);
        let body = first
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let page: worldstream_protocol::OperatorRoomInventoryPage = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("inventory JSON: {error}"));
        assert_eq!(page.rooms.len(), 1);
        assert_eq!(page.rooms[0].room_id, "01ARZ3NDEKTSV4RRFFQ69G5FQ0");
        assert_eq!(
            page.next_after_room_id.as_deref(),
            Some("01ARZ3NDEKTSV4RRFFQ69G5FQ0")
        );

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/v1/operator/rooms?after_room_id=01ARZ3NDEKTSV4RRFFQ69G5FQ0&limit=1")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(second.status(), StatusCode::OK);
        let body = second
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let page: worldstream_protocol::OperatorRoomInventoryPage = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("inventory JSON: {error}"));
        assert_eq!(page.rooms[0].room_id, "01ARZ3NDEKTSV4RRFFQ69G5FQ1");
        assert!(page.next_after_room_id.is_none());

        let empty = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        )
        .oneshot(
            Request::builder()
                .uri("/v1/operator/rooms?after_room_id=01ARZ3NDEKTSV4RRFFQ69G5FQ1&limit=1")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
        let body = empty
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let page: worldstream_protocol::OperatorRoomInventoryPage = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("empty inventory JSON: {error}"));
        assert!(page.rooms.is_empty());
        assert!(page.next_after_room_id.is_none());
    }

    #[tokio::test]
    async fn operator_room_detail_is_privacy_bounded_and_phase_explicitly_unavailable() {
        let response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        )
        .oneshot(
            Request::builder()
                .uri("/v1/operator/rooms/01ARZ3NDEKTSV4RRFFQ69G5FQ0")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("detail JSON: {error}"));
        assert_eq!(value["room_head"]["room_seq"], 1);
        assert_eq!(value["pack"]["id"], "worldstream.counter");
        assert_eq!(value["integrity"]["status"], "healthy");
        assert_eq!(value["activity_phase"]["status"], "unavailable");
        assert_eq!(value["freshness"]["status"], "fresh");
        let serialized = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| unreachable!("UTF-8 body: {error}"));
        for private_field in [
            "member_id",
            "principal_id",
            "invocation",
            "activation",
            "cursor",
        ] {
            assert!(
                !serialized.contains(private_field),
                "leaked {private_field}"
            );
        }
    }

    #[tokio::test]
    async fn operator_activation_status_is_exact_and_private_data_free() {
        let response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        )
        .oneshot(
            Request::builder()
                .uri("/v1/operator/rooms/01ARZ3NDEKTSV4RRFFQ69G5FQ0/members/01ARZ3NDEKTSV4RRFFQ69G5FQ1/activation-status")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("activation status JSON: {error}"));
        assert_eq!(
            value,
            serde_json::json!({
                "version": "worldstream/operator-activation-status/v1",
                "waiting": 2,
                "leased": 1,
                "observed_at_unix_ms": 1_777_000_000_000_u64
            })
        );
        let serialized = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| unreachable!("UTF-8 body: {error}"));
        for private_field in ["activation_id", "claim_id", "context", "reason_code"] {
            assert!(
                !serialized.contains(private_field),
                "leaked {private_field}"
            );
        }
    }

    #[tokio::test]
    async fn operator_activation_status_distinguishes_closed_failures_without_private_data() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let member = "01ARZ3NDEKTSV4RRFFQ69G5FQ1";
        for (room, authorization, expected) in [
            ("01ARZ3NDEKTSV4RRFFQ69G5FQ0", false, StatusCode::FORBIDDEN),
            ("01ARZ3NDEKTSV4RRFFQ69G5FQ2", true, StatusCode::NOT_FOUND),
            (
                "01ARZ3NDEKTSV4RRFFQ69G5FQ3",
                true,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            ("01ARZ3NDEKTSV4RRFFQ69G5FQ4", true, StatusCode::FORBIDDEN),
        ] {
            let mut request = Request::builder().uri(format!(
                "/v1/operator/rooms/{room}/members/{member}/activation-status"
            ));
            if authorization {
                request = request.header(header::AUTHORIZATION, auth_header());
            }
            let response = app
                .clone()
                .oneshot(
                    request
                        .body(Body::empty())
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), expected, "{room}");
            let body = response
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes();
            let serialized = String::from_utf8(body.to_vec())
                .unwrap_or_else(|error| unreachable!("UTF-8 body: {error}"));
            for private in [room, member, "activation_id", "claim_id", "context"] {
                assert!(!serialized.contains(private), "leaked {private}");
            }
        }
    }

    #[tokio::test]
    async fn operator_room_inventory_rejects_unbounded_or_ambiguous_queries() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        for query in ["limit=0", "limit=101", "limit=1&limit=1", "unknown=1"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/operator/rooms?{query}"))
                        .header(header::AUTHORIZATION, auth_header())
                        .body(Body::empty())
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }
    }

    #[tokio::test]
    async fn operator_room_inventory_reports_storage_unavailable_without_synthetic_rooms() {
        let response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}")),
        )
        .oneshot(
            Request::builder()
                .uri("/v1/operator/rooms?limit=1")
                .header(header::AUTHORIZATION, auth_header())
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("unavailable JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::StorageUnavailable);
    }

    #[tokio::test]
    async fn operator_live_backup_is_pathless_and_profile_health_is_independent() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/operator/backups/health")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("health request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("health response: {error}"));
        assert_eq!(health.status(), StatusCode::OK);
        let backup = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/operator/backups")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"operation_id":"stable-backup"}"#))
                    .unwrap_or_else(|error| unreachable!("backup request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("backup response: {error}"));
        assert_eq!(backup.status(), StatusCode::OK);
        let bytes = backup
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("backup body: {error}"))
            .to_bytes();
        let status: worldstream_protocol::OperatorLiveBackupStatus = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| unreachable!("backup JSON: {error}"));
        assert_eq!(status.operation_id, "stable-backup");
        assert_eq!(
            status.native_verification,
            worldstream_protocol::OperatorBackupVerification::Pass
        );

        let rejected_path = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/backups")
                .header(header::AUTHORIZATION, auth_header())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"operation_id":"stable-backup","destination":"/private/sentinel/backup.sqlite3"}"#,
                ))
                .unwrap_or_else(|error| unreachable!("path request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("path response: {error}"));
        assert!(rejected_path.status().is_client_error());
    }

    #[tokio::test]
    async fn live_publication_does_not_duplicate_after_registered_catch_up() {
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = Arc::new(GatewaySession::new(
            session_id,
            CapabilityBearerV1::from_bytes([0x44; 32]),
        ));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (close_sender, _close_receiver) = tokio::sync::watch::channel(None);
        let registry = super::LiveStreamRegistry::default();
        registry
            .register(
                Arc::clone(&session),
                "room".to_owned(),
                "member".to_owned(),
                0,
                sender,
                close_sender,
            )
            .unwrap_or_else(|error| unreachable!("register stream: {error:?}"));

        super::publish_live_frames(Arc::new(SuccessfulCreateBackend), &registry, "room").await;
        let Some(super::LivePush::Frame(push)) = receiver.recv().await else {
            unreachable!("live frame was not queued")
        };
        assert_eq!(push.frame.frame_seq, 1);
        assert_eq!(push.frame.member_id, "member");

        super::publish_live_frames(Arc::new(SuccessfulCreateBackend), &registry, "room").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), receiver.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn publication_waits_for_registration_barrier_without_a_gap() {
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = Arc::new(GatewaySession::new(
            session_id,
            CapabilityBearerV1::from_bytes([0x45; 32]),
        ));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (close_sender, _close_receiver) = tokio::sync::watch::channel(None);
        let registry = super::LiveStreamRegistry::default();
        let publication_guard = registry.publication_lock.lock().await;
        let registry_for_publish = registry.clone();
        let backend = Arc::new(SuccessfulCreateBackend);
        let publisher = tokio::spawn(async move {
            super::publish_live_frames(backend, &registry_for_publish, "room").await;
        });

        registry
            .register(
                Arc::clone(&session),
                "room".to_owned(),
                "member".to_owned(),
                0,
                sender,
                close_sender,
            )
            .unwrap_or_else(|error| unreachable!("register stream: {error:?}"));
        assert!(receiver.try_recv().is_err());
        drop(publication_guard);
        publisher
            .await
            .unwrap_or_else(|error| unreachable!("publisher task: {error}"));
        let Some(super::LivePush::Frame(push)) = receiver.recv().await else {
            unreachable!("live frame was lost across the registration barrier")
        };
        assert_eq!(push.frame.frame_seq, 1);
    }

    #[tokio::test]
    async fn live_publication_keeps_membership_streams_private() {
        let participant_id = "01ARZ3NDEKTSV4RRFFQ69G5FAX"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let spectator_id = "01ARZ3NDEKTSV4RRFFQ69G5FAY"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let participant = Arc::new(GatewaySession::new(
            participant_id,
            CapabilityBearerV1::from_bytes([0x46; 32]),
        ));
        let spectator = Arc::new(GatewaySession::new(
            spectator_id,
            CapabilityBearerV1::from_bytes([0x47; 32]),
        ));
        let (participant_sender, mut participant_receiver) =
            tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (spectator_sender, mut spectator_receiver) =
            tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (participant_close, _participant_close_receiver) = tokio::sync::watch::channel(None);
        let (spectator_close, _spectator_close_receiver) = tokio::sync::watch::channel(None);
        let registry = super::LiveStreamRegistry::default();
        registry
            .register(
                participant,
                "room".to_owned(),
                "participant".to_owned(),
                0,
                participant_sender,
                participant_close,
            )
            .unwrap_or_else(|error| unreachable!("register participant: {error:?}"));
        registry
            .register(
                spectator,
                "room".to_owned(),
                "spectator".to_owned(),
                0,
                spectator_sender,
                spectator_close,
            )
            .unwrap_or_else(|error| unreachable!("register spectator: {error:?}"));

        super::publish_live_frames(Arc::new(SuccessfulCreateBackend), &registry, "room").await;
        let Some(super::LivePush::Frame(participant_push)) = participant_receiver.recv().await
        else {
            unreachable!("participant frame was not queued")
        };
        let Some(super::LivePush::Frame(spectator_push)) = spectator_receiver.recv().await else {
            unreachable!("spectator frame was not queued")
        };
        let participant_frame = participant_push.frame;
        let spectator_frame = spectator_push.frame;
        assert_eq!(participant_frame.member_id, "participant");
        assert_eq!(spectator_frame.member_id, "spectator");
        assert_ne!(participant_frame.member_id, spectator_frame.member_id);
    }

    #[tokio::test]
    async fn live_frame_count_overflow_closes_with_typed_error() {
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAZ"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = Arc::new(GatewaySession::new(
            session_id,
            CapabilityBearerV1::from_bytes([0x48; 32]),
        ));
        let (sender, _receiver) = tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (close_sender, mut close_receiver) = tokio::sync::watch::channel(None);
        let registry = super::LiveStreamRegistry::default();
        registry
            .register(
                session,
                "frame-overflow".to_owned(),
                "member".to_owned(),
                0,
                sender,
                close_sender,
            )
            .unwrap_or_else(|error| unreachable!("register stream: {error:?}"));

        super::publish_live_frames(
            Arc::new(SuccessfulCreateBackend),
            &registry,
            "frame-overflow",
        )
        .await;
        tokio::time::timeout(Duration::from_millis(50), close_receiver.changed())
            .await
            .unwrap_or_else(|_| unreachable!("frame-count overflow did not close the connection"))
            .unwrap_or_else(|error| unreachable!("slow-consumer close: {error}"));
        assert_eq!(*close_receiver.borrow(), Some(ErrorCode::SlowConsumer));
        assert!(registry.snapshots_for_room("frame-overflow").is_empty());
        let [frames, _] = registry.queue_snapshots();
        assert_eq!(frames.capacity, super::LIVE_PUSH_CAPACITY);
        assert_eq!(frames.backpressure_total, 1);
    }

    #[tokio::test]
    async fn live_payload_byte_overflow_closes_with_typed_error() {
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FB1"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = Arc::new(GatewaySession::new(
            session_id,
            CapabilityBearerV1::from_bytes([0x4a; 32]),
        ));
        let (sender, _receiver) = tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (close_sender, mut close_receiver) = tokio::sync::watch::channel(None);
        let registry = super::LiveStreamRegistry::default();
        registry
            .register(
                session,
                "byte-overflow".to_owned(),
                "member".to_owned(),
                0,
                sender,
                close_sender,
            )
            .unwrap_or_else(|error| unreachable!("register stream: {error:?}"));

        super::publish_live_frames(
            Arc::new(SuccessfulCreateBackend),
            &registry,
            "byte-overflow",
        )
        .await;
        tokio::time::timeout(Duration::from_millis(50), close_receiver.changed())
            .await
            .unwrap_or_else(|_| unreachable!("payload-byte overflow did not close the connection"))
            .unwrap_or_else(|error| unreachable!("slow-consumer close: {error}"));
        assert_eq!(*close_receiver.borrow(), Some(ErrorCode::SlowConsumer));
        assert!(registry.snapshots_for_room("byte-overflow").is_empty());
        let [frames, payload] = registry.queue_snapshots();
        assert!(frames.activity_total > 0);
        assert!(payload.activity_total > 0);
        assert!(payload.unit_high_water <= super::MAX_OUTBOUND_BUFFER_BYTES);
        assert_eq!(payload.backpressure_total, 1);
    }

    #[test]
    fn observation_batch_admission_enforces_exact_frame_and_byte_limits() {
        fn frame(frame_seq: u64, padding_bytes: usize) -> ObservationDeliver {
            ObservationDeliver {
                room_id: "room".to_owned(),
                member_id: "member".to_owned(),
                frame_seq,
                cause_room_seq: frame_seq,
                frame_kind: "transition".to_owned(),
                observation_schema: "counter/v1".to_owned(),
                observation: serde_json::json!({
                    "padding": "x".repeat(padding_bytes),
                }),
                frame_payload_hash: "hash".to_owned(),
            }
        }

        assert_eq!(super::LIVE_PUSH_CAPACITY, 256);

        let exact_frame_limit = (1..=super::MAX_OUTBOUND_FRAME_BURST as u64)
            .map(|frame_seq| frame(frame_seq, 0))
            .collect();
        let prepared = super::prepare_observation_batch(exact_frame_limit)
            .unwrap_or_else(|error| unreachable!("exact frame limit: {error:?}"));
        assert_eq!(prepared.len(), 256);

        let over_frame_limit = (1..=super::MAX_OUTBOUND_FRAME_BURST as u64 + 1)
            .map(|frame_seq| frame(frame_seq, 0))
            .collect();
        assert_eq!(
            super::prepare_observation_batch(over_frame_limit),
            Err(super::OutboundMessageError::SlowConsumer)
        );

        let over_byte_limit = (1..=17)
            .map(|frame_seq| frame(frame_seq, 256 * 1024))
            .collect();
        assert_eq!(
            super::prepare_observation_batch(over_byte_limit),
            Err(super::OutboundMessageError::SlowConsumer)
        );
    }

    #[test]
    fn live_payload_budget_accepts_exact_limit_and_releases_on_drop() {
        let queued_payload_bytes = Arc::new(AtomicUsize::new(0));
        let metrics = Arc::new(super::LiveQueueCounters::default());
        let reservation = super::OutboundPayloadReservation::try_new(
            Arc::clone(&queued_payload_bytes),
            super::MAX_OUTBOUND_BUFFER_BYTES,
            &metrics,
        )
        .unwrap_or_else(|| unreachable!("exact byte limit must be admitted"));
        assert_eq!(
            queued_payload_bytes.load(Ordering::Acquire),
            super::MAX_OUTBOUND_BUFFER_BYTES
        );
        assert!(
            super::OutboundPayloadReservation::try_new(
                Arc::clone(&queued_payload_bytes),
                1,
                &metrics,
            )
            .is_none()
        );
        assert_eq!(metrics.backpressure_total.load(Ordering::Relaxed), 1);
        drop(reservation);
        assert_eq!(queued_payload_bytes.load(Ordering::Acquire), 0);
        assert_eq!(metrics.process_current.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn websocket_send_timeout_rejects_a_stalled_sender() {
        let stalled = std::future::pending::<Result<(), std::convert::Infallible>>();
        assert!(
            super::send_websocket_message(stalled, Duration::from_millis(1))
                .await
                .is_err()
        );
        assert!(
            super::send_websocket_message(
                std::future::ready(Ok::<(), std::convert::Infallible>(())),
                Duration::from_millis(1),
            )
            .await
            .is_ok()
        );
        assert!(super::WEBSOCKET_SEND_TIMEOUT <= Duration::from_secs(10));
        assert!(!super::WEBSOCKET_SEND_TIMEOUT.is_zero());
    }

    fn auth_header() -> HeaderValue {
        HeaderValue::from_str(&format!(
            "Bearer {BEARER_WIRE_PREFIX}{}",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        ))
        .unwrap_or_else(|error| unreachable!("header value is valid ASCII: {error}"))
    }

    fn create_room_body() -> Body {
        Body::from(
            r#"{"pack":{"id":"pack","version":"1","digest":"digest"},"configuration":{},"members":[],"idempotency_key":"request"}"#,
        )
    }

    fn lobby_launch_body() -> Body {
        Body::from(r#"{"input_id":"01ARZ3NDEKTSV4RRFFQ69G5FC6","based_on_room_seq":0}"#)
    }

    #[tokio::test]
    async fn lobby_launch_requires_authenticated_host_admission() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/operator/rooms/room/lobby/launch")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(lobby_launch_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn runner_presence_is_host_authorized_bounded_and_retains_disconnect_state() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(Arc::new(SuccessfulCreateBackend));
        let registry = state.runner_presence.clone();
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
            .parse::<UlidString>()
            .unwrap_or_else(|_| unreachable!("session id"));
        let runner_id = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
        registry.register(
            &session_id,
            &RunnerHello {
                runner_id: runner_id.to_owned(),
                maximum_concurrent_activations: 2,
                supported_pack_ids: vec!["worldstream.agent-heist".to_owned()],
                supported_pack_revisions: vec![PackReference {
                    id: "worldstream.agent-heist".to_owned(),
                    version: "1.0.0".to_owned(),
                    digest:
                        "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_owned(),
                }],
            },
        );
        let app = operator_router(state);
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/operator/runners/{runner_id}/presence"))
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);
        let connected = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/operator/runners/{runner_id}/presence"))
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(connected.status(), StatusCode::OK);
        let connected: OperatorRunnerPresenceV1 = serde_json::from_slice(
            &connected
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("presence: {error}"));
        assert_eq!(connected.connection, OperatorRunnerConnectionV1::Connected);
        assert_eq!(connected.available_activations, 2);
        assert_eq!(connected.supported_pack_revisions.len(), 1);

        registry.disconnect(runner_id, &session_id);
        let disconnected = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/operator/runners/{runner_id}/presence"))
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        let disconnected: OperatorRunnerPresenceV1 = serde_json::from_slice(
            &disconnected
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("presence: {error}"));
        assert_eq!(
            disconnected.connection,
            OperatorRunnerConnectionV1::Disconnected
        );

        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/v1/operator/runners/01ARZ3NDEKTSV4RRFFQ69G5FAY/presence")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn lobby_launch_has_bounded_success_and_wrong_phase_outcomes() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let success = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/operator/rooms/room/lobby/launch")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(lobby_launch_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(success.status(), StatusCode::OK);

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/operator/rooms/duplicate/lobby/launch")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(lobby_launch_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(duplicate.status(), StatusCode::OK);
        let duplicate: LobbyLaunchResponse = serde_json::from_slice(
            &duplicate
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("Lobby duplicate: {error}"));
        assert!(duplicate.duplicate);

        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/operator/rooms/conflict/lobby/launch")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(lobby_launch_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let wrong_phase = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/operator/rooms/wrong-phase/lobby/launch")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(lobby_launch_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(wrong_phase.status(), StatusCode::CONFLICT);
        let envelope: ErrorEnvelope = serde_json::from_slice(
            &wrong_phase
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("error envelope: {error}"));
        assert_eq!(envelope.error.code, ErrorCode::WrongPhase);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn activity_pack_catalog_lists_and_reads_exact_revisions() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/operator/activity-packs")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(list.status(), StatusCode::OK);
        let list: worldstream_protocol::ActivityPackCatalogResponse = serde_json::from_slice(
            &list
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("catalog: {error}"));
        assert_eq!(
            list.version,
            worldstream_protocol::ACTIVITY_PACK_CATALOG_VERSION
        );
        assert_eq!(list.revisions.len(), 7);
        assert!(
            list.revisions
                .windows(2)
                .all(|pair| { pair[0].pack.digest < pair[1].pack.digest })
        );
        let retained = list
            .revisions
            .iter()
            .find(|revision| revision.pack.digest == counter_v1_digest().to_string())
            .unwrap_or_else(|| unreachable!("retained Counter revision"));
        assert!(!retained.selectable_for_new_rooms);
        assert!(retained.runnable_for_retained_rooms);
        let retained_v3 = list
            .revisions
            .iter()
            .find(|revision| revision.pack.digest == counter_v3_digest().to_string())
            .unwrap_or_else(|| unreachable!("retained Counter v3 revision"));
        assert!(!retained_v3.selectable_for_new_rooms);
        assert!(retained_v3.runnable_for_retained_rooms);
        let selectable_v4 = list
            .revisions
            .iter()
            .find(|revision| revision.pack.digest == counter_v4_digest().to_string())
            .unwrap_or_else(|| unreachable!("selectable Counter v4 revision"));
        assert!(selectable_v4.selectable_for_new_rooms);
        assert!(selectable_v4.runnable_for_retained_rooms);

        let selected_digest = counter_v2_digest().to_string();
        let detail = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/operator/activity-packs/{selected_digest}"))
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(detail.status(), StatusCode::OK);
        let detail: worldstream_protocol::ActivityPackCatalogRevisionResponse =
            serde_json::from_slice(
                &detail
                    .into_body()
                    .collect()
                    .await
                    .unwrap_or_else(|error| unreachable!("body: {error}"))
                    .to_bytes(),
            )
            .unwrap_or_else(|error| unreachable!("detail: {error}"));
        assert_eq!(detail.revision.summary.pack.digest, selected_digest);
        assert!(!detail.revision.roles.is_empty());
        assert!(!detail.revision.actions.is_empty());
        assert_eq!(
            detail.revision.configuration_schema.schema_id,
            "counter/configuration/v1"
        );
        assert!(detail.revision.lobby_compatibility.is_none());

        let lobby_digest = worldstream_core::agent_heist_lobby_digest().to_string();
        let lobby_detail = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/operator/activity-packs/{lobby_digest}"))
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(lobby_detail.status(), StatusCode::OK);
        let lobby_detail: worldstream_protocol::ActivityPackCatalogRevisionResponse =
            serde_json::from_slice(
                &lobby_detail
                    .into_body()
                    .collect()
                    .await
                    .unwrap_or_else(|error| unreachable!("body: {error}"))
                    .to_bytes(),
            )
            .unwrap_or_else(|error| unreachable!("Lobby detail: {error}"));
        assert_eq!(lobby_detail.revision.summary.pack.digest, lobby_digest);
        assert_eq!(
            lobby_detail
                .revision
                .lobby_compatibility
                .as_ref()
                .map(|compatibility| compatibility.contract.as_str()),
            Some(worldstream_core::AGENT_HEIST_LOBBY_CONTRACT)
        );
    }

    #[tokio::test]
    async fn unknown_activity_pack_revision_is_explicitly_unavailable() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/operator/activity-packs/blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let error: ErrorEnvelope = serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .unwrap_or_else(|error| unreachable!("body: {error}"))
                .to_bytes(),
        )
        .unwrap_or_else(|error| unreachable!("error: {error}"));
        assert_eq!(error.error.code, ErrorCode::ActivityPackRevisionUnavailable);
        assert!(!error.error.retryable);
    }

    #[tokio::test]
    async fn authenticated_rejection_does_not_call_the_semantic_backend() {
        let backend = Arc::new(AdmissionCountingBackend::default());
        let mut state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(backend.clone());
        state.rate_limiter =
            super::rate_limit::GatewayRateLimiter::fixed_dimension_for_integration_tests(
                super::rate_limit::AdmissionDimension::Principal,
                1,
            );
        let app = operator_router(state);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(create_room_body())
                    .unwrap_or_else(|error| unreachable!("first request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("first response: {error}"));
        assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(backend.create_calls.load(Ordering::SeqCst), 1);

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(create_room_body())
                    .unwrap_or_else(|error| unreachable!("second request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("second response: {error}"));
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(backend.create_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn runner_capability_with_exactly_256_targets_consumes_one_operator_admission() {
        let backend = Arc::new(AdmissionCountingBackend::default());
        let mut state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(backend.clone());
        state.rate_limiter =
            super::rate_limit::GatewayRateLimiter::fixed_dimension_for_integration_tests(
                super::rate_limit::AdmissionDimension::Operator,
                1,
            );
        let app = operator_router(state);
        let targets = (0..256)
            .map(|index| {
                serde_json::json!({
                    "room_id": format!("room-{index}"),
                    "member_id": format!("member-{index}"),
                })
            })
            .collect::<Vec<_>>();
        let body = serde_json::json!({
            "runner_id": "runner-boundary",
            "owner_principal_id": "owner-boundary",
            "permitted_memberships": targets,
            "scopes": [],
            "principal_idempotency_key": "principal-change-boundary",
            "runner_idempotency_key": "runner-change-boundary",
            "capability_idempotency_key": "capability-change-boundary",
            "expires_at": null,
        })
        .to_string();

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/operator/runner-capabilities")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap_or_else(|error| unreachable!("first request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("first response: {error}"));
        assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(backend.runner_capability_calls.load(Ordering::SeqCst), 1);

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/operator/runner-capabilities")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap_or_else(|error| unreachable!("second request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("second response: {error}"));
        assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(backend.runner_capability_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn operator_timer_fire_route_is_bounded_and_does_not_echo_payload() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(Arc::new(SuccessfulCreateBackend));
        let response = operator_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/operator/rooms/01ARZ3NDEKTSV4RRFFQ69G5FQ0/timers/fire")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FQ1","generation":1}"#,
                    ))
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("timer response JSON: {error}"));
        assert!(value.get("canonical_payload").is_none());
        assert_eq!(value["generation"], 1);
    }

    #[tokio::test]
    async fn operator_timer_fire_publishes_only_the_new_committed_frame() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(Arc::new(SuccessfulCreateBackend));
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FB0"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = Arc::new(GatewaySession::new(
            session_id,
            CapabilityBearerV1::from_bytes([0x49; 32]),
        ));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(super::LIVE_PUSH_CAPACITY);
        let (close_sender, _close_receiver) = tokio::sync::watch::channel(None);
        state
            .live_streams
            .register(
                session,
                "timer-publication".to_owned(),
                "member".to_owned(),
                1,
                sender,
                close_sender,
            )
            .unwrap_or_else(|error| unreachable!("register stream: {error:?}"));

        let response = operator_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/operator/rooms/timer-publication/timers/fire")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"timer_id":"01ARZ3NDEKTSV4RRFFQ69G5FB1","generation":1}"#,
                    ))
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::OK);

        let Some(super::LivePush::Frame(push)) = receiver.recv().await else {
            unreachable!("timer commit did not advance the registered live stream")
        };
        let frame = push.frame;
        assert_eq!(frame.frame_seq, 2);
        assert_eq!(frame.cause_room_seq, 2);
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_millis(10), receiver.recv()).await,
                Err(_) | Ok(None)
            ),
            "the external timer commit must publish exactly one newly committed frame"
        );
    }

    #[tokio::test]
    async fn replay_route_requires_auth_and_returns_exact_response_shape() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(Arc::new(SuccessfulCreateBackend));
        let app = operator_router(state);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms/replay-room/replay?at_room_seq=7")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("Replay response JSON: {error}"));
        let keys = value
            .as_object()
            .unwrap_or_else(|| unreachable!("Replay response object"))
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "room_id",
                "pack",
                "requested_room_seq",
                "room_head",
                "projection",
                "projection_hash",
                "verification",
                "room_health",
                "integrity_generation",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(value["room_id"], "replay-room");
        assert_eq!(value["requested_room_seq"], 7);
        assert_eq!(value["verification"], "verified");

        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms/replay-room/replay?at_room_seq=7")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(unauthenticated.status(), StatusCode::FORBIDDEN);

        let invalid_query = app
            .oneshot(
                Request::builder()
                    .uri("/v1/rooms/replay-room/replay?at_room_seq=7&extra=1")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(invalid_query.status(), StatusCode::BAD_REQUEST);
        let body = invalid_query
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::InvalidPayload);
    }

    #[tokio::test]
    async fn health_is_liveness_only_and_ready_is_truthful() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"));
        let app = operator_router(state);

        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(health.status(), 200);

        let ready = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(ready.status(), 503);
        let body = ready
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::StorageNotInitialized);
    }

    #[tokio::test]
    async fn daemon_readiness_ready_branch_requires_all_runtime_facts() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_readiness(RuntimeReadiness::ready_for_tests());
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        assert_eq!(body.as_ref(), br#"{"status":"ready"}"#);
    }

    #[tokio::test]
    async fn daemon_readiness_reports_unconfigured_scheduler_and_authority() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_readiness(RuntimeReadiness::sqlite_store_verified());
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 503);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::StorageNotInitialized);
        assert!(!error.error.retryable);
        assert!(error.error.message.contains("scheduler"));
        assert!(error.error.message.contains("authority"));
        let checks = &error
            .error
            .details
            .unwrap_or_else(|| unreachable!("checks"))["checks"];
        assert_eq!(checks["schema"], true);
        assert_eq!(checks["storage"], true);
        assert_eq!(checks["writer"], true);
        assert_eq!(checks["scheduler"], false);
        assert_eq!(checks["authority_bootstrap"], false);
    }

    #[tokio::test]
    async fn daemon_readiness_keeps_scheduler_boundary_after_authority_bootstrap() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_readiness(
                RuntimeReadiness::sqlite_store_verified().with_authority_bootstrapped(),
            );
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 503);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::StorageNotInitialized);
        assert!(!error.error.retryable);
        assert!(error.error.message.contains("scheduler"));
        assert!(!error.error.message.contains("not bootstrapped"));
        let checks = &error
            .error
            .details
            .unwrap_or_else(|| unreachable!("checks"))["checks"];
        assert_eq!(checks["schema"], true);
        assert_eq!(checks["storage"], true);
        assert_eq!(checks["writer"], true);
        assert_eq!(checks["scheduler"], false);
        assert_eq!(checks["authority_bootstrap"], true);
    }

    #[test]
    fn daemon_readiness_rejects_an_existing_file_as_data_directory() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("tempdir: {error}"));
        let file = directory.path().join("not-a-directory");
        std::fs::write(&file, b"not a directory")
            .unwrap_or_else(|error| unreachable!("fixture file: {error}"));
        let result = worldstream_runtime::prepare_data_directory(&file);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn http_room_admission_and_commit_are_post_result_telemetry() {
        let exporter = RecordingExporter::default();
        let events = Arc::clone(&exporter.events);
        let runtime = TelemetryRuntime::new(
            super::telemetry::TelemetryConfig {
                queue_capacity: 8,
                batch_size: 8,
                ..super::telemetry::TelemetryConfig::default()
            },
            Arc::new(exporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let metrics = runtime.metrics();
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_backend(Arc::new(SuccessfulCreateBackend))
            .with_telemetry(runtime.handle());
        let app = operator_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(
                        "traceparent",
                        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(create_room_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);

        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            super::telemetry::FlushOutcome::Flushed
        );
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.enqueued, 2);
        assert_eq!(snapshot.dropped, 0);
        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].details,
            EventDetailsV1::Admission {
                outcome: super::telemetry::AdmissionOutcomeV1::Accepted
            }
        ));
        assert!(matches!(
            events[1].details,
            EventDetailsV1::CommitOutcome {
                outcome: CommitOutcomeV1::Committed
            }
        ));
        assert!(events.iter().all(|event| {
            let encoded = event
                .json_line()
                .unwrap_or_else(|error| unreachable!("bounded event: {error}"));
            assert!(encoded.len() <= MAX_EVENT_BYTES);
            !encoded.contains("request") && !encoded.contains("00010203")
        }));
        assert_eq!(
            events[0]
                .correlation
                .traceparent
                .map(telemetry::TraceParentV1::to_header),
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned())
        );
        assert_eq!(events[0].correlation, events[1].correlation);
    }

    #[tokio::test]
    async fn metrics_route_is_bounded_and_available_when_exporter_is_unhealthy() {
        let runtime = TelemetryRuntime::new(
            super::telemetry::TelemetryConfig::default(),
            Arc::new(FailingExporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let handle = runtime.handle();
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_telemetry(handle),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("metrics body: {error}"))
            .to_bytes();
        let body = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| unreachable!("metrics UTF-8: {error}"));
        assert!(body.contains("worldstream_telemetry_events_total{event=\"activation\"} 0"));
        assert!(body.contains("worldstream_telemetry_queue_capacity 256"));
        assert!(body.contains("worldstream_telemetry_queued 0"));
        for (queue, scope, capacity) in [
            ("telemetry_exporter", "global", 256),
            ("telemetry_dns_resolver_queue", "global", 2),
            ("room_admission_lane", "per_room", 256),
            ("websocket_live_push_frame_queue", "per_connection", 256),
            (
                "websocket_outbound_payload_bytes",
                "per_connection",
                4 * 1024 * 1024,
            ),
        ] {
            assert!(body.contains(&format!(
                "worldstream_internal_queue_capacity{{queue=\"{queue}\",scope=\"{scope}\"}} {capacity}"
            )));
            assert!(body.contains(&format!(
                "worldstream_internal_queue_process_current{{queue=\"{queue}\"}} 0"
            )));
            assert!(body.contains(&format!(
                "worldstream_internal_queue_backpressure_total{{queue=\"{queue}\"}} 0"
            )));
        }
        assert!(!body.contains("room_id"));
        assert!(!body.contains("authorization"));
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            super::telemetry::FlushOutcome::Flushed
        );
    }

    #[test]
    fn lifecycle_producers_emit_only_typed_post_result_facts() {
        let exporter = RecordingExporter::default();
        let events = Arc::clone(&exporter.events);
        let runtime = TelemetryRuntime::new(
            super::telemetry::TelemetryConfig {
                queue_capacity: 8,
                batch_size: 8,
                ..super::telemetry::TelemetryConfig::default()
            },
            Arc::new(exporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let correlation = telemetry::CorrelationV1::from_traceparent(
            telemetry::TraceParentV1::parse(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            )
            .unwrap_or_else(|error| unreachable!("valid traceparent: {error}")),
        );
        let handle = runtime.handle();
        record_timer_with_correlation(
            Some(&handle),
            telemetry::ReasonCodeV1::Accepted,
            correlation,
        );
        record_activation_with_correlation(
            Some(&handle),
            telemetry::ActivationPhaseV1::Claimed,
            telemetry::ReasonCodeV1::Accepted,
            correlation,
        );
        record_activation_with_correlation(
            Some(&handle),
            telemetry::ActivationPhaseV1::Completed,
            telemetry::ReasonCodeV1::Conflict,
            correlation,
        );
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            super::telemetry::FlushOutcome::Flushed
        );
        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(matches!(
            events[0].details,
            EventDetailsV1::Timer {
                phase: super::telemetry::TimerPhaseV1::Fired
            }
        ));
        assert!(matches!(
            events[1].details,
            EventDetailsV1::Activation {
                phase: super::telemetry::ActivationPhaseV1::Claimed
            }
        ));
        assert_eq!(events[2].reason, super::telemetry::ReasonCodeV1::Conflict);
        assert!(events.iter().all(|event| {
            event.correlation == correlation
                && event.attributes.is_empty()
                && event.json_line().is_ok()
        }));
    }

    #[tokio::test]
    async fn exporter_failure_does_not_change_readiness_or_route_outcome() {
        let runtime = TelemetryRuntime::new(
            super::telemetry::TelemetryConfig::default(),
            Arc::new(FailingExporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let handle = runtime.handle();
        let metrics = runtime.metrics();
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_telemetry(handle.clone());
        let app = operator_router(state);

        let ready = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(ready.status(), 503);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(create_room_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 503);

        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            super::telemetry::FlushOutcome::Flushed
        );
        assert!(metrics.snapshot().exporter_failures >= 1);
        let body = ready
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let error: ErrorEnvelope = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("error JSON: {error}"));
        assert_eq!(error.error.code, ErrorCode::StorageNotInitialized);
    }

    #[tokio::test]
    async fn router_state_owns_runtime_until_drop_and_flushes_post_result_events() {
        let exporter = RecordingExporter::default();
        let events = Arc::clone(&exporter.events);
        let runtime = TelemetryRuntime::new(
            super::telemetry::TelemetryConfig {
                queue_capacity: 8,
                batch_size: 8,
                ..super::telemetry::TelemetryConfig::default()
            },
            Arc::new(exporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("valid state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend))
                .with_telemetry_runtime(runtime),
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/rooms")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(create_room_body())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        drop(app);

        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].details,
            EventDetailsV1::Admission {
                outcome: super::telemetry::AdmissionOutcomeV1::Accepted
            }
        ));
        assert!(matches!(
            events[1].details,
            EventDetailsV1::CommitOutcome {
                outcome: CommitOutcomeV1::Committed
            }
        ));
    }

    #[tokio::test]
    async fn version_is_manifest_backed_and_engine_is_uninitialized() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"));
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("version JSON: {error}"));
        assert_eq!(value["product_build"]["product"], "0.1.0");
        assert_eq!(value["product_build"]["binary"], "worldstreamd");
        assert_eq!(value["product_build"]["source_revision"], BUILD_REVISION);
        assert_eq!(BUILD_REVISION.len(), 40);
        assert!(
            BUILD_REVISION
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(value["wire"], "0.1");
        assert_eq!(
            value["manifest"]["schema"],
            "worldstream/storage-compatibility-manifest/v1"
        );
        assert_eq!(value["manifest"]["release_ready"], false);
        assert_eq!(value["engine"]["status"], "not_initialized");
        assert!(value["engine"]["exact_identity"].is_null());

        let _: Option<VersionResponse> = None;
    }

    #[tokio::test]
    async fn daemon_version_reports_verified_sqlite_identity_after_startup() {
        let state = OperatorState::new(EffectiveConfig::default())
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_verified_sqlite_engine("3.53.4", "source-id");
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("version JSON: {error}"));
        assert_eq!(value["engine"]["status"], "verified");
        assert_eq!(
            value["engine"]["exact_identity"],
            "sqlite/3.53.4; source_id=source-id"
        );
    }

    #[tokio::test]
    async fn daemon_version_reports_startup_selected_postgres_identity() {
        let mut config = EffectiveConfig::default();
        config.storage.profile = StorageProfile::PostgresPrimary;
        let state = OperatorState::new(config)
            .unwrap_or_else(|error| unreachable!("valid state: {error}"))
            .with_verified_engine("postgresql/17.11; server_version_num=170011".to_owned());
        let app = operator_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("version JSON: {error}"));
        assert_eq!(value["engine"]["profile"], "postgres-primary");
        assert_eq!(value["engine"]["status"], "verified");
        assert_eq!(
            value["engine"]["exact_identity"],
            "postgresql/17.11; server_version_num=170011"
        );
    }

    #[test]
    fn authorization_requires_canonical_wire_and_decodes_to_core_bearer() {
        let wire = format!(
            "{BEARER_WIRE_PREFIX}{}",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {wire}"))
                .unwrap_or_else(|error| unreachable!("header value is valid ASCII: {error}")),
        );
        let parsed =
            bearer(&headers).unwrap_or_else(|error| unreachable!("canonical bearer: {error:?}"));
        assert_eq!(
            parsed.token_hash(),
            CapabilityBearerV1::from_bytes([
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
                0x1c, 0x1d, 0x1e, 0x1f,
            ])
            .token_hash()
        );
    }

    #[test]
    fn authorization_rejects_legacy_or_noncanonical_values_without_echoing_them() {
        for value in [
            "Bearer raw-secret",
            "Bearer WSB1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "Bearer wsb1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f ",
            "Basic wsb1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_str(value)
                    .unwrap_or_else(|error| unreachable!("test header value: {error}")),
            );
            let Err(error) = bearer(&headers) else {
                unreachable!("noncanonical bearer was accepted")
            };
            assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
            let rendered = format!("{error:?}");
            assert!(!rendered.contains("00010203"));
            assert!(!rendered.contains("raw-secret"));
        }
    }

    #[test]
    fn session_identity_is_independent_from_bearer_and_redacted_in_diagnostics() {
        let first_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let second_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let first = GatewaySession::new(first_id, CapabilityBearerV1::from_bytes([0x5a; 32]));
        let second = GatewaySession::new(second_id, CapabilityBearerV1::from_bytes([0x5a; 32]));

        assert_ne!(first.session_id(), second.session_id());
        assert_eq!(first.bearer().token_hash(), second.bearer().token_hash());
        assert!(!format!("{first:?}").contains("5a5a5a"));
    }

    #[test]
    fn generated_transport_ulids_are_canonical_and_do_not_wrap_at_the_old_boundary() {
        let mut generated = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let value = next_ulid().unwrap_or_else(|| unreachable!("OS-backed ULID"));
            assert_eq!(value.as_str().len(), 26);
            assert!(generated.insert(value));
        }

        let before_old_wrap = super::ulid_from_parts(1_787_375_000_000, [0; 10])
            .unwrap_or_else(|| unreachable!("ULID before old wrap"));
        let after_old_wrap = super::ulid_from_parts(1_787_375_000_000, [1; 10])
            .unwrap_or_else(|| unreachable!("ULID after old wrap"));
        assert_ne!(before_old_wrap, after_old_wrap);
    }

    #[test]
    fn active_session_identity_collision_fails_closed_until_release() {
        let registry = super::LiveStreamRegistry::default();
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        assert!(registry.reserve_session(&session_id));
        assert!(!registry.reserve_session(&session_id));
        registry.release_session(&session_id);
        assert!(registry.reserve_session(&session_id));
    }

    #[test]
    fn backend_welcome_must_echo_the_transport_session_identity() {
        let session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        let session = GatewaySession::new(session_id, CapabilityBearerV1::from_bytes([0x33; 32]));
        let matching = worldstream_protocol::ServerWelcome {
            session_id: session.session_id().clone(),
            selected_protocol: "0.1".to_owned(),
            server_version: "0.1.0".to_owned(),
            heartbeat_interval_ms: 1_000,
            maximum_message_bytes: worldstream_protocol::MAX_MESSAGE_BYTES,
            authenticated_principal: worldstream_protocol::Principal {
                principal_id: "principal".to_owned(),
                kind: worldstream_protocol::PrincipalKind::Human,
            },
        };
        assert!(welcome_matches_session(&matching, &session));

        let mut mismatched = matching;
        mismatched.session_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
            .parse()
            .unwrap_or_else(|error| unreachable!("ULID: {error}"));
        assert!(!welcome_matches_session(&mismatched, &session));
    }

    #[test]
    fn replies_correlate_to_explicit_request_id_and_fall_back_to_message_id() {
        let message_id: worldstream_protocol::UlidString = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|error| unreachable!("message ID: {error}"));
        let request_id: worldstream_protocol::UlidString = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
            .parse()
            .unwrap_or_else(|error| unreachable!("request ID: {error}"));
        let with_request = worldstream_protocol::VersionedEnvelope {
            protocol: "0.1".to_owned(),
            message_type: "action.submit".to_owned(),
            message_id: message_id.clone(),
            request_id: Some(request_id.clone()),
            body: serde_json::Value::Null,
        };
        assert_eq!(
            super::reply_request_id(with_request.request_id.as_ref(), &with_request.message_id),
            Some(&request_id)
        );

        let without_request = worldstream_protocol::VersionedEnvelope {
            request_id: None,
            ..with_request
        };
        assert_eq!(
            super::reply_request_id(
                without_request.request_id.as_ref(),
                &without_request.message_id
            ),
            Some(&message_id)
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn operator_member_capability_route_proves_counter_live_path() {
        let file = tempfile::NamedTempFile::new()
            .unwrap_or_else(|error| unreachable!("temporary SQLite file: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| unreachable!("SQLite store: {error}"));
        let host_bytes = [0xa9_u8; 32];
        let host_bearer = CapabilityBearerV1::from_bytes(host_bytes);
        let host_principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|error| unreachable!("host principal: {error}"));
        let host_capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FC3")
            .unwrap_or_else(|error| unreachable!("host capability: {error}"));
        AuthorityV1::new(Arc::new(store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|error| unreachable!("bootstrap change: {error}")),
                    host_principal.clone(),
                    PrincipalKindV1::Human,
                    host_capability,
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| unreachable!("bootstrap shape: {error}")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|error| unreachable!("bootstrap time: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("bootstrap authority: {error}"));

        let registry = Arc::new(
            builtin_worldstream_registry()
                .unwrap_or_else(|error| unreachable!("WorldStream registry: {error}")),
        );
        let counter = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| unreachable!("Counter v2: {error}"));
        let backend = Arc::new(SqliteGatewayBackend::new(store, registry));
        let host_wire = BearerWireV1::from_bytes(host_bytes);
        let host_session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FC5"
                .parse()
                .unwrap_or_else(|error| unreachable!("host session: {error}")),
            CapabilityBearerV1::from_bytes(host_bytes),
            host_wire,
        );
        let created = backend
            .create_room(
                &host_session,
                CreateRoomRequest {
                    pack: PackReference {
                        id: counter.descriptor().pack_id.clone(),
                        version: counter.descriptor().explanatory_version.clone(),
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: serde_json::json!({
                        "initial_value": 0,
                        "maximum_value": 16
                    }),
                    members: vec![CreateMember {
                        principal_id: host_principal.to_string(),
                        principal_kind: PrincipalKind::Human,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    }],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD0".to_owned(), // gitleaks:allow - fixture ULID
                },
            )
            .unwrap_or_else(|error| unreachable!("Counter room: {error:?}"));
        let issue_body = serde_json::json!({
            "room_id": created.room_id,
            "member_id": created.member_ids[0],
            "principal_id": host_principal.to_string(),
            "scopes": ["room:attach", "room:observe_member", "room:act"],
            "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD1", // gitleaks:allow
            "expires_at": null
        });
        let host_header = format!("Bearer {}", BearerWireV1::from_bytes(host_bytes).to_wire());

        let wrong_principal_body = serde_json::json!({
            "room_id": issue_body["room_id"],
            "member_id": issue_body["member_id"],
            "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FD2",
            "scopes": issue_body["scopes"],
            "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD3" // gitleaks:allow
        });
        let wrong_principal_response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                .with_backend(backend.clone()),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/member-capabilities")
                .header(header::AUTHORIZATION, &host_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(wrong_principal_body.to_string()))
                .unwrap_or_else(|error| unreachable!("wrong principal request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("wrong principal response: {error}"));
        assert_eq!(wrong_principal_response.status(), 403);

        let invalid_scope_body = serde_json::json!({
            "room_id": issue_body["room_id"],
            "member_id": issue_body["member_id"],
            "principal_id": issue_body["principal_id"],
            "scopes": ["operator:room_admin"],
            "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD4" // gitleaks:allow
        });
        let invalid_scope_response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                .with_backend(backend.clone()),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/member-capabilities")
                .header(header::AUTHORIZATION, &host_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(invalid_scope_body.to_string()))
                .unwrap_or_else(|error| unreachable!("invalid scope request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("invalid scope response: {error}"));
        assert_eq!(invalid_scope_response.status(), 400);

        let issue_response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                .with_backend(backend.clone()),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/member-capabilities")
                .header(header::AUTHORIZATION, &host_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(issue_body.to_string()))
                .unwrap_or_else(|error| unreachable!("issue request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("issue response: {error}"));
        assert_eq!(issue_response.status(), 200);
        assert_eq!(
            issue_response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let issue_bytes = issue_response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("issue body: {error}"))
            .to_bytes();
        let issued: super::MemberCapabilityIssueResponse = serde_json::from_slice(&issue_bytes)
            .unwrap_or_else(|error| unreachable!("issued capability JSON: {error}"));
        assert_eq!(
            issued.room_id,
            issue_body["room_id"]
                .as_str()
                .unwrap_or_else(|| unreachable!("issued room ID"))
        );
        assert_eq!(
            issued.member_id,
            issue_body["member_id"]
                .as_str()
                .unwrap_or_else(|| unreachable!("issued member ID"))
        );
        assert_ne!(issued.bearer, host_header.trim_start_matches("Bearer "));
        assert!(!format!("{issued:?}").contains(&issued.bearer));

        let retry_response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                .with_backend(backend.clone()),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/member-capabilities")
                .header(header::AUTHORIZATION, &host_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(issue_body.to_string()))
                .unwrap_or_else(|error| unreachable!("retry request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("retry response: {error}"));
        assert_eq!(retry_response.status(), 409);
        let retry_bytes = retry_response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("retry body: {error}"))
            .to_bytes();
        assert!(!String::from_utf8_lossy(&retry_bytes).contains(&issued.bearer));

        let member_wire = BearerWireV1::parse(&issued.bearer)
            .unwrap_or_else(|error| unreachable!("issued bearer: {error}"));
        let member_session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FD5"
                .parse()
                .unwrap_or_else(|error| unreachable!("member session: {error}")),
            CapabilityBearerV1::from_bytes(
                BearerWireV1::parse(&issued.bearer)
                    .unwrap_or_else(|error| unreachable!("issued bearer copy: {error}"))
                    .into_bytes(),
            ),
            member_wire,
        );
        let attached = backend
            .attach(
                &member_session,
                RoomAttach {
                    room_id: issue_body["room_id"]
                        .as_str()
                        .unwrap_or_else(|| unreachable!("room ID"))
                        .to_owned(),
                    member_id: issue_body["member_id"]
                        .as_str()
                        .unwrap_or_else(|| unreachable!("member ID"))
                        .to_owned(),
                    after_frame_seq: None,
                },
            )
            .unwrap_or_else(|error| unreachable!("member attach: {error:?}"));
        assert_eq!(attached.attached.pack.id, "worldstream.counter");
        let sync_token = attached.attached.sync_token.clone();
        let room_id = attached.attached.room_id.clone();
        let member_id = attached.attached.member_id.clone();
        let through_frame_head = attached.attached.frame_head;
        let synced = backend
            .sync_ack(
                &member_session,
                RoomSyncAck {
                    room_id: room_id.clone(),
                    member_id: member_id.clone(),
                    through_frame_head,
                    sync_token,
                },
            )
            .unwrap_or_else(|error| unreachable!("member sync ack: {error:?}"));
        assert!(synced.is_empty());
        assert_eq!(
            backend
                .observation_ack(
                    &member_session,
                    ObservationAck {
                        room_id: room_id.clone(),
                        member_id: member_id.clone(),
                        through_frame_seq: through_frame_head,
                    },
                )
                .unwrap_or_else(|error| unreachable!("member observation ack: {error:?}")),
            None
        );

        let action = backend
            .action(
                &member_session,
                ActionSubmit {
                    room_id,
                    member_id,
                    action_id: "01ARZ3NDEKTSV4RRFFQ69G5FD6".to_owned(),
                    based_on_room_seq: 0,
                    action_type: "increment".to_owned(),
                    payload: serde_json::json!({}),
                },
            )
            .unwrap_or_else(|error| unreachable!("Counter live action: {error:?}"));
        match action {
            super::ActionReply::Accepted(value) => {
                assert_eq!(value.room_head.room_seq, 1);
                assert!(!value.duplicate);
            }
            super::ActionReply::Rejected(value) => {
                unreachable!("Counter action rejected: {value:?}")
            }
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn operator_member_capability_provisions_agent_principal_idempotently() {
        let file = tempfile::NamedTempFile::new()
            .unwrap_or_else(|error| unreachable!("temporary SQLite file: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| unreachable!("SQLite store: {error}"));
        let host_bytes = [0xa9_u8; 32];
        let host_bearer = CapabilityBearerV1::from_bytes(host_bytes);
        let host_principal = "01ARZ3NDEKTSV4RRFFQ69G5FE0"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|error| unreachable!("host principal: {error}"));
        let host_capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FE1")
            .unwrap_or_else(|error| unreachable!("host capability: {error}"));
        AuthorityV1::new(Arc::new(store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FE2"
                        .parse()
                        .unwrap_or_else(|error| unreachable!("bootstrap change: {error}")),
                    host_principal,
                    PrincipalKindV1::Human,
                    host_capability,
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| unreachable!("bootstrap authority: {error}")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|error| unreachable!("bootstrap time: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("apply bootstrap: {error}"));

        let registry = Arc::new(
            builtin_worldstream_registry()
                .unwrap_or_else(|error| unreachable!("WorldStream registry: {error}")),
        );
        let counter = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| unreachable!("Counter v2: {error}"));
        let backend = Arc::new(SqliteGatewayBackend::new(store, registry));
        let host_session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FE3"
                .parse()
                .unwrap_or_else(|error| unreachable!("host session: {error}")),
            CapabilityBearerV1::from_bytes(host_bytes),
            BearerWireV1::from_bytes(host_bytes),
        );
        let agent_principal = "01ARZ3NDEKTSV4RRFFQ69G5FE4";
        let created = backend
            .create_room(
                &host_session,
                CreateRoomRequest {
                    pack: PackReference {
                        id: counter.descriptor().pack_id.clone(),
                        version: counter.descriptor().explanatory_version.clone(),
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: serde_json::json!({
                        "initial_value": 0,
                        "maximum_value": 16
                    }),
                    members: vec![CreateMember {
                        principal_id: agent_principal.to_owned(),
                        principal_kind: PrincipalKind::Agent,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    }],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FE5".to_owned(), // gitleaks:allow
                },
            )
            .unwrap_or_else(|error| unreachable!("Agent Counter room: {error:?}"));
        let issue_body = serde_json::json!({
            "room_id": created.room_id,
            "member_id": created.member_ids[0],
            "principal_id": agent_principal,
            "scopes": ["room:attach", "room:observe_member", "room:act"],
            "idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FE6", // gitleaks:allow
            "expires_at": null
        });
        let host_header = format!("Bearer {}", BearerWireV1::from_bytes(host_bytes).to_wire());
        let issue = |body: serde_json::Value| {
            let backend = backend.clone();
            let host_header = host_header.clone();
            async move {
                operator_router(
                    OperatorState::new(EffectiveConfig::default())
                        .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                        .with_backend(backend),
                )
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/operator/member-capabilities")
                        .header(header::AUTHORIZATION, &host_header)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap_or_else(|error| unreachable!("member capability request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("member capability response: {error}"))
            }
        };

        // This is the production-shaped defect: the Room has an Agent seat,
        // but no authority Principal row yet. The route must provision it and
        // return a member-bound bearer without exposing the HostOperator one.
        let first = issue(issue_body.clone()).await;
        assert_eq!(first.status(), 200);
        let first_bytes = first
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("first member capability body: {error}"))
            .to_bytes();
        let issued: MemberCapabilityIssueResponse = serde_json::from_slice(&first_bytes)
            .unwrap_or_else(|error| unreachable!("first member capability JSON: {error}"));
        assert_eq!(issued.principal_id, agent_principal);
        assert!(!issued.bearer.contains(&host_header));
        assert!(!format!("{issued:?}").contains(&issued.bearer));

        // A retry of the same capability change is a conflict and never
        // re-delivers the one-time bearer.
        let retry = issue(issue_body.clone()).await;
        let retry_status = retry.status();
        let retry_bytes = retry
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("retry member capability body: {error}"))
            .to_bytes();
        assert_eq!(
            retry_status,
            409,
            "retry body: {}",
            String::from_utf8_lossy(&retry_bytes)
        );
        assert!(!String::from_utf8_lossy(&retry_bytes).contains(&issued.bearer));

        // Reusing the capability change ID with a different request is also
        // rejected, while the already-created Principal remains unchanged.
        let mut conflict_body = issue_body.clone();
        conflict_body["scopes"] = serde_json::json!(["room:attach"]);
        let conflict = issue(conflict_body).await;
        assert_eq!(conflict.status(), 409);

        // A mismatched principal is rejected before any authority mutation.
        let mut wrong_principal_body = issue_body.clone();
        wrong_principal_body["principal_id"] = serde_json::json!("01ARZ3NDEKTSV4RRFFQ69G5FE7");
        wrong_principal_body["idempotency_key"] = serde_json::json!("01ARZ3NDEKTSV4RRFFQ69G5FE8");
        let wrong_principal = issue(wrong_principal_body).await;
        assert_eq!(wrong_principal.status(), 403);

        // A different capability change for the same exact Agent seat proves
        // the existing-Principal branch and preserves the same binding.
        let mut existing_principal_body = issue_body;
        existing_principal_body["idempotency_key"] =
            serde_json::json!("01ARZ3NDEKTSV4RRFFQ69G5FE9");
        let existing_principal = issue(existing_principal_body).await;
        assert_eq!(existing_principal.status(), 200);
        let existing_bytes = existing_principal
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("existing principal body: {error}"))
            .to_bytes();
        let second: MemberCapabilityIssueResponse = serde_json::from_slice(&existing_bytes)
            .unwrap_or_else(|error| unreachable!("existing principal JSON: {error}"));
        assert_eq!(second.room_id, created.room_id);
        assert_eq!(second.member_id, created.member_ids[0]);
        assert_eq!(second.principal_id, agent_principal);

        // Studio setup seals the bearer and Capability identity before the
        // first POST. A response lost after the authority commit can then
        // repeat the exact request and recover the same secret-free receipt.
        let sealed_body = serde_json::json!({
            "room_id": created.room_id,
            "member_id": created.member_ids[0],
            "principal_id": agent_principal,
            "principal_kind": "agent",
            "role": "counter",
            "access_mode": "participant",
            "scopes": ["room:attach", "room:observe_member", "room:act"],
            "capability": {
                "capability_id": "01ARZ3NDEKTSV4RRFFQ69G5FF0",
                "capability_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FF1",
                "bearer": BearerWireV1::from_bytes([0x44; 32]).to_wire()
            },
            "expires_at": null
        });
        let provision = |body: serde_json::Value| {
            let backend = backend.clone();
            let host_header = host_header.clone();
            async move {
                operator_router(
                    OperatorState::new(EffectiveConfig::default())
                        .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                        .with_backend(backend),
                )
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/operator/member-capabilities:provision")
                        .header(header::AUTHORIZATION, &host_header)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap_or_else(|error| unreachable!("sealed request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("sealed response: {error}"))
            }
        };
        let first_sealed = provision(sealed_body.clone()).await;
        assert_eq!(first_sealed.status(), 200);
        let repeated_sealed = provision(sealed_body.clone()).await;
        assert_eq!(repeated_sealed.status(), 200);
        let mut changed = sealed_body;
        changed["scopes"] = serde_json::json!(["room:attach"]);
        assert_eq!(provision(changed).await.status(), 409);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn operator_runner_capability_route_proves_runner_authority_path() {
        let file = tempfile::NamedTempFile::new()
            .unwrap_or_else(|error| unreachable!("temporary SQLite file: {error}"));
        let store = SqliteRoomStore::open(file.path())
            .unwrap_or_else(|error| unreachable!("SQLite store: {error}"));
        let host_bytes = [0xa9_u8; 32];
        let host_bearer = CapabilityBearerV1::from_bytes(host_bytes);
        let host_principal = "01ARZ3NDEKTSV4RRFFQ69G5FC2"
            .parse::<worldstream_core::PrincipalId>()
            .unwrap_or_else(|error| unreachable!("host principal: {error}"));
        let host_capability = CapabilityId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FC3")
            .unwrap_or_else(|error| unreachable!("host capability: {error}"));
        AuthorityV1::new(Arc::new(store.clone()))
            .bootstrap(
                AuthorityBootstrapV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FC4"
                        .parse()
                        .unwrap_or_else(|error| unreachable!("bootstrap change: {error}")),
                    host_principal,
                    PrincipalKindV1::Human,
                    host_capability,
                    host_bearer.token_hash(),
                    None,
                )
                .unwrap_or_else(|error| unreachable!("bootstrap shape: {error}")),
                "2026-08-15T12:00:00Z"
                    .parse::<AuthorityCheckedAt>()
                    .unwrap_or_else(|error| unreachable!("bootstrap time: {error}")),
            )
            .unwrap_or_else(|error| unreachable!("bootstrap authority: {error}"));

        let registry = Arc::new(
            builtin_worldstream_registry()
                .unwrap_or_else(|error| unreachable!("WorldStream registry: {error}")),
        );
        let counter = registry
            .load_retained(&counter_v2_digest())
            .unwrap_or_else(|error| unreachable!("Counter v2: {error}"));
        let backend = Arc::new(SqliteGatewayBackend::new(store, registry));
        let host_wire = BearerWireV1::from_bytes(host_bytes);
        let host_session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FC5"
                .parse()
                .unwrap_or_else(|error| unreachable!("host session: {error}")),
            CapabilityBearerV1::from_bytes(host_bytes),
            host_wire,
        );
        let owner_principal = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
        let created = backend
            .create_room(
                &host_session,
                CreateRoomRequest {
                    pack: PackReference {
                        id: counter.descriptor().pack_id.clone(),
                        version: counter.descriptor().explanatory_version.clone(),
                        digest: counter_v2_digest().to_string(),
                    },
                    configuration: serde_json::json!({
                        "initial_value": 0,
                        "maximum_value": 16
                    }),
                    members: vec![CreateMember {
                        principal_id: owner_principal.to_owned(),
                        principal_kind: PrincipalKind::Agent,
                        role: Some("counter".to_owned()),
                        access_mode: AccessMode::Participant,
                    }],
                    idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD1".to_owned(), // gitleaks:allow
                },
            )
            .unwrap_or_else(|error| unreachable!("Counter room: {error:?}"));

        let host_header = format!("Bearer {}", BearerWireV1::from_bytes(host_bytes).to_wire());
        let runner_id = "01ARZ3NDEKTSV4RRFFQ69G5FD2";
        let issue_body = serde_json::json!({
            "runner_id": runner_id,
            "owner_principal_id": owner_principal,
            "permitted_memberships": [{
                "room_id": created.room_id,
                "member_id": created.member_ids[0]
            }],
            "scopes": [
                "activation:offer_receive",
                "activation:claim",
                "activation:complete"
            ],
            "principal_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD3", // gitleaks:allow
            "runner_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD4", // gitleaks:allow
            "capability_idempotency_key": "01ARZ3NDEKTSV4RRFFQ69G5FD5", // gitleaks:allow
            "expires_at": null
        });
        let response = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                .with_backend(backend.clone()),
        )
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/runner-capabilities")
                .header(header::AUTHORIZATION, &host_header)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(issue_body.to_string()))
                .unwrap_or_else(|error| unreachable!("Runner capability request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("Runner capability response: {error}"));
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::PRAGMA], "no-cache");
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("Runner capability body: {error}"))
            .to_bytes();
        let issued: RunnerCapabilityIssueResponse = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("Runner capability JSON: {error}"));
        assert_eq!(issued.runner_id, runner_id);
        assert_eq!(issued.owner_principal_id, owner_principal);
        assert_eq!(issued.permitted_memberships.len(), 1);
        assert_eq!(issued.scopes.iter().count(), 3);
        assert!(issued.bearer.starts_with("wsb1:"));

        let runner_wire = BearerWireV1::parse(&issued.bearer)
            .unwrap_or_else(|error| unreachable!("issued Runner bearer: {error}"));
        let runner_session = GatewaySession::new_with_wire(
            "01ARZ3NDEKTSV4RRFFQ69G5FD6"
                .parse()
                .unwrap_or_else(|error| unreachable!("Runner session: {error}")),
            CapabilityBearerV1::from_bytes(
                BearerWireV1::parse(&issued.bearer)
                    .unwrap_or_else(|error| unreachable!("Runner bearer copy: {error}"))
                    .into_bytes(),
            ),
            runner_wire,
        );
        let ready = backend
            .runner_hello(
                &runner_session,
                RunnerHello {
                    runner_id: runner_id.to_owned(),
                    maximum_concurrent_activations: 4,
                    supported_pack_ids: vec!["worldstream.counter".to_owned()],
                    supported_pack_revisions: Vec::new(),
                },
            )
            .unwrap_or_else(|error| unreachable!("Runner hello: {error:?}"));
        assert_eq!(ready.runner_id, runner_id);
        let offers = backend
            .activation_offers(
                &runner_session,
                worldstream_protocol::ActivationOfferRequest {
                    operation_id: "01ARZ3NDEKTSV4RRFFQ69G5FD7".to_owned(),
                    runner_id: runner_id.to_owned(),
                    room_id: created.room_id,
                    member_id: created.member_ids[0].clone(),
                },
            )
            .unwrap_or_else(|error| unreachable!("Runner offer poll: {error:?}"));
        assert!(offers.offers.is_empty());
    }

    fn browser_test_session(byte: u8) -> GatewaySession {
        GatewaySession::new(
            next_ulid().unwrap_or_else(|| unreachable!("test session id")),
            CapabilityBearerV1::from_bytes([byte; 32]),
        )
    }

    #[test]
    fn browser_ticket_is_origin_bound_single_use_and_never_retained_as_plaintext() {
        let store = BrowserTicketStore::with_limits(4, Duration::from_secs(15));
        let now = std::time::Instant::now();
        let origin = "http://127.0.0.1:5173";
        let ticket = store
            .issue_at(browser_test_session(0x21), origin.to_owned(), now)
            .unwrap_or_else(|error| unreachable!("ticket issue: {error:?}"));

        assert_eq!(ticket.len(), BROWSER_TICKET_WIRE_LENGTH);
        assert!(ticket.starts_with(BROWSER_TICKET_PREFIX));
        assert!(store.consume_at(&ticket, origin, now).is_some());
        assert!(store.consume_at(&ticket, origin, now).is_none());

        let wrong_origin = store
            .issue_at(browser_test_session(0x22), origin.to_owned(), now)
            .unwrap_or_else(|error| unreachable!("ticket issue: {error:?}"));
        assert!(
            store
                .consume_at(&wrong_origin, "http://localhost:5173", now)
                .is_none()
        );
        assert!(store.consume_at(&wrong_origin, origin, now).is_none());
    }

    #[test]
    fn browser_ticket_capacity_and_expiry_fail_closed() {
        let store = BrowserTicketStore::with_limits(1, Duration::from_secs(5));
        let now = std::time::Instant::now();
        let origin = "http://localhost:4173".to_owned();
        let first = store
            .issue_at(browser_test_session(0x31), origin.clone(), now)
            .unwrap_or_else(|error| unreachable!("ticket issue: {error:?}"));
        assert_eq!(
            store.issue_at(browser_test_session(0x32), origin.clone(), now),
            Err(BrowserTicketError::Capacity)
        );
        assert!(
            store
                .consume_at(&first, &origin, now + Duration::from_secs(6))
                .is_none()
        );
        assert!(
            store
                .issue_at(
                    browser_test_session(0x33),
                    origin,
                    now + Duration::from_secs(6)
                )
                .is_ok()
        );
    }

    #[test]
    fn browser_origin_policy_accepts_loopback_origins_only() {
        for origin in [
            "http://127.0.0.1:5173",
            "https://localhost:9443",
            "http://[::1]:4173",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::ORIGIN,
                HeaderValue::from_str(origin)
                    .unwrap_or_else(|error| unreachable!("origin header: {error}")),
            );
            assert_eq!(browser_origin(&headers).ok().as_deref(), Some(origin));
        }
        for origin in [
            "https://evil.example",
            "null",
            "http://127.0.0.1:5173/private",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::ORIGIN,
                HeaderValue::from_str(origin)
                    .unwrap_or_else(|error| unreachable!("origin header: {error}")),
            );
            assert_eq!(
                browser_origin(&headers).err().map(|error| error.status),
                Some(StatusCode::FORBIDDEN)
            );
        }
        assert_eq!(
            browser_origin(&HeaderMap::new())
                .err()
                .map(|error| error.status),
            Some(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn header_capable_path_remains_independent_of_browser_ticket_and_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, auth_header());
        let session = authenticated_session(&headers)
            .unwrap_or_else(|error| unreachable!("header session: {error:?}"));
        assert!(!session.session_id().as_str().is_empty());
        assert!(
            websocket_origin(&headers)
                .unwrap_or_else(|error| unreachable!("header origin: {error:?}"))
                .is_none()
        );
    }

    async fn websocket_handshake(path: &'static str, protocol: Option<&'static str>) -> String {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}")),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| unreachable!("listener: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("listener address: {error}"));
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });
        let response = tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect(address)
                .unwrap_or_else(|error| unreachable!("handshake connect: {error}"));
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap_or_else(|error| unreachable!("handshake timeout: {error}"));
            let protocol_header = protocol
                .map(|value| format!("Sec-WebSocket-Protocol: {value}\r\n"))
                .unwrap_or_default();
            let rfc6455_nonce = ["dGhlIHNhbXBsZSBu", "b25jZQ=="].concat();
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: http://127.0.0.1:5173\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {rfc6455_nonce}\r\n{protocol_header}\r\n"
            );
            stream
                .write_all(request.as_bytes())
                .unwrap_or_else(|error| unreachable!("handshake write: {error}"));
            let mut response = Vec::with_capacity(2048);
            let mut chunk = [0_u8; 1024];
            while !response.windows(4).any(|value| value == b"\r\n\r\n") {
                assert!(response.len() <= 16 * 1024, "handshake response is bounded");
                let count = stream
                    .read(&mut chunk)
                    .unwrap_or_else(|error| unreachable!("handshake read: {error}"));
                assert!(count > 0, "handshake response ended before its headers");
                response.extend_from_slice(&chunk[..count]);
            }
            String::from_utf8(response)
                .unwrap_or_else(|error| unreachable!("handshake response encoding: {error}"))
        })
        .await
        .unwrap_or_else(|error| unreachable!("handshake task: {error}"));
        server.abort();
        let _ = server.await;
        response
    }

    #[tokio::test]
    async fn websocket_routes_require_and_echo_the_exact_wire_subprotocol() {
        assert_eq!(
            worldstream_protocol::WEBSOCKET_SUBPROTOCOL,
            format!(
                "worldstream.json.v{}",
                worldstream_protocol::PROTOCOL_VERSION
            )
        );
        for path in ["/v1/stream", "/v1/runner/stream"] {
            let accepted =
                websocket_handshake(path, Some(worldstream_protocol::WEBSOCKET_SUBPROTOCOL)).await;
            assert!(accepted.starts_with("HTTP/1.1 101 "), "{accepted}");
            assert!(
                accepted.contains(&format!(
                    "\r\nsec-websocket-protocol: {}\r\n",
                    worldstream_protocol::WEBSOCKET_SUBPROTOCOL
                )),
                "{accepted}"
            );

            for unsupported in [None, Some("worldstream.json.v9.9")] {
                let rejected = websocket_handshake(path, unsupported).await;
                assert!(rejected.starts_with("HTTP/1.1 400 "), "{rejected}");
                assert!(!rejected.contains("sec-websocket-protocol"));
            }
        }
    }

    #[tokio::test]
    async fn browser_ticket_route_requires_origin_and_returns_no_store_opaque_ticket() {
        let app = operator_router(
            OperatorState::new(EffectiveConfig::default())
                .unwrap_or_else(|error| unreachable!("operator state: {error}"))
                .with_backend(Arc::new(SuccessfulCreateBackend)),
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stream/ticket")
                    .header(header::AUTHORIZATION, auth_header())
                    .header(header::ORIGIN, "http://127.0.0.1:5173")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("ticket request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("ticket response: {error}"));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://127.0.0.1:5173"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("ticket body: {error}"))
            .to_bytes();
        let issued: BrowserWebSocketTicketIssueResponse = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("ticket JSON: {error}"));
        assert_eq!(
            issued.version,
            worldstream_protocol::BROWSER_WS_TICKET_VERSION
        );
        assert!(issued.ticket.starts_with(BROWSER_TICKET_PREFIX));
        assert_eq!(issued.expires_in_ms, 15_000);
        assert!(!format!("{issued:?}").contains(&issued.ticket));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/stream/ticket")
                    .header(header::AUTHORIZATION, auth_header())
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("missing-origin request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("missing-origin response: {error}"));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("missing-origin body: {error}"))
            .to_bytes();
        assert!(!String::from_utf8_lossy(&body).contains("00010203"));
    }

    #[tokio::test]
    async fn browser_admission_timeout_is_bounded_and_closes_without_secret_disclosure() {
        let pending = std::future::pending::<Option<Result<Message, axum::Error>>>();
        let received = receive_browser_ticket(pending, Duration::from_millis(1)).await;
        assert!(received.is_none());
        let pending = std::future::pending::<Option<Result<Message, axum::Error>>>();
        assert!(
            super::receive_websocket_message(pending, Duration::from_millis(1))
                .await
                .is_none()
        );
        assert!(BROWSER_TICKET_TTL <= Duration::from_secs(15));
        assert!(super::FIRST_CLIENT_HELLO_TIMEOUT <= Duration::from_secs(10));
        assert!(super::POST_WELCOME_IDLE_TIMEOUT <= Duration::from_secs(90));
        let Message::Close(Some(close)) = browser_admission_close_message() else {
            unreachable!("browser admission must close with a close frame");
        };
        assert_eq!(close.code, 1008);
        assert_eq!(close.reason, BROWSER_ADMISSION_CLOSE_REASON);
        assert!(!close.reason.contains(BROWSER_TICKET_PREFIX));
        assert!(!close.reason.contains("Bearer"));
    }

    #[test]
    fn sealed_member_provisioning_rejects_invalid_access_mode_role_pairs() {
        assert!(super::member_capability_access_role_valid(
            AccessMode::Participant,
            Some("counter")
        ));
        assert!(super::member_capability_access_role_valid(
            AccessMode::Operator,
            None
        ));
        assert!(super::member_capability_access_role_valid(
            AccessMode::Spectator,
            None
        ));
        assert!(!super::member_capability_access_role_valid(
            AccessMode::Participant,
            None
        ));
        assert!(!super::member_capability_access_role_valid(
            AccessMode::Operator,
            Some("counter")
        ));
        assert!(!super::member_capability_access_role_valid(
            AccessMode::Spectator,
            Some("counter")
        ));
    }
}
