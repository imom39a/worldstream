//! Bounded local control-plane boundary between Studio and `worldstreamd`.
//!
//! Room authority and all Room mutations remain behind the daemon's supported
//! APIs. Lifecycle control is limited to one fixed local daemon configuration.

pub mod activity_packs;
pub mod agent_profiles;
pub mod assignment_mcp;
pub mod assignment_mcp_actions;
pub mod assignment_mcp_activation_ledger;
pub mod assignment_mcp_activations;
pub mod assignment_mcp_operations;
pub mod attention_inbox;
pub mod backups;
pub mod client_bindings;
pub mod lifecycle;
pub mod managed_activation_status;
pub mod managed_agent_host;
pub mod managed_agent_host_seats;
pub mod model_provider_credentials;
pub mod participant_handoff;
pub mod room_creation;
pub mod room_drafts;
pub mod rooms;
pub mod runner_attention;
pub mod runner_templates;

pub mod secrets;
pub mod startup_authority;
pub mod task_setup;
pub mod task_templates;

use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};

const STATUS_SCHEMA_V1: &str = "worldstream/studio-daemon-status/v1";
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_VERSION_FIELD_BYTES: usize = 128;

/// Connectivity between the Supervisor and `worldstreamd`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonConnectivityV1 {
    /// The Supervisor reached and recognized the daemon health endpoint.
    Connected,
    /// No recognized daemon health response was available.
    Unavailable,
}

/// Process liveness reported by `worldstreamd`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonHealthV1 {
    /// The daemon returned its exact live health response.
    Live,
    /// Liveness could not be established.
    Unavailable,
}

/// Durable-runtime readiness reported by `worldstreamd`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonReadinessV1 {
    /// The daemon is ready to serve its supported runtime operations.
    Ready,
    /// The daemon is live but its own readiness probe is not ready.
    NotReady,
    /// Readiness could not be obtained.
    Unavailable,
}

/// Bounded failure vocabulary; transport error strings never cross the API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonUnavailableReasonV1 {
    /// The local daemon listener could not be reached within the timeout.
    ConnectionFailed,
    /// The listener replied, but not with the expected bounded daemon schema.
    InvalidResponse,
    /// The Supervisor's bounded probe worker could not complete.
    ProbeFailed,
}

/// Selected version identity safe for the Studio status surface.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonVersionV1 {
    /// Product compatibility version from the daemon manifest.
    pub product: String,
    /// Daemon build version from the daemon manifest.
    pub build_version: String,
    /// Source revision embedded in the daemon build.
    pub source_revision: String,
}

/// Complete typed response exposed to Studio.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonStatusV1 {
    /// Stable response schema identifier.
    pub schema: String,
    /// Supervisor-to-daemon connectivity.
    pub connectivity: DaemonConnectivityV1,
    /// Daemon process liveness.
    pub health: DaemonHealthV1,
    /// Daemon durable-runtime readiness.
    pub readiness: DaemonReadinessV1,
    /// Manifest-backed daemon version, only when verified by the probe.
    pub version: Option<DaemonVersionV1>,
    /// Closed failure reason, absent for a fully available status.
    pub unavailable_reason: Option<DaemonUnavailableReasonV1>,
}

impl DaemonStatusV1 {
    /// A fail-closed status for an unreachable daemon.
    #[must_use]
    pub fn unavailable() -> Self {
        Self::unavailable_for(DaemonUnavailableReasonV1::ConnectionFailed)
    }

    fn unavailable_for(reason: DaemonUnavailableReasonV1) -> Self {
        Self {
            schema: STATUS_SCHEMA_V1.to_owned(),
            connectivity: DaemonConnectivityV1::Unavailable,
            health: DaemonHealthV1::Unavailable,
            readiness: DaemonReadinessV1::Unavailable,
            version: None,
            unavailable_reason: Some(reason),
        }
    }
}

/// The only capability injected into the Studio status API.
pub trait DaemonStatusSource: Send + Sync + 'static {
    /// Returns one bounded snapshot; it cannot execute arbitrary commands.
    fn status(&self) -> DaemonStatusV1;
}

/// Live local HTTP implementation backed by the daemon's existing probes.
#[derive(Clone, Debug)]
pub struct HttpDaemonStatusSource {
    address: SocketAddr,
    timeout: Duration,
}

impl HttpDaemonStatusSource {
    /// Creates a live source for one already-configured daemon address.
    #[must_use]
    pub const fn new(address: SocketAddr, timeout: Duration) -> Self {
        Self { address, timeout }
    }

    fn request(&self, path: &str) -> Result<HttpResponse, ProbeError> {
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| ProbeError::Connection)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|_| ProbeError::Connection)?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(|_| ProbeError::Connection)?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
            self.address
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|_| ProbeError::Connection)?;
        let mut bytes = Vec::new();
        stream
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ProbeError::Connection)?;
        if bytes.len() > usize::try_from(MAX_RESPONSE_BYTES).unwrap_or(usize::MAX) {
            return Err(ProbeError::InvalidResponse);
        }
        parse_http_response(&bytes)
    }

    fn connected_status(&self) -> Result<DaemonStatusV1, ProbeError> {
        let health = self.request("/healthz")?;
        let health_body: HealthWire = health.json()?;
        if health.status != 200 || health_body.status != "ok" {
            return Err(ProbeError::InvalidResponse);
        }

        let readiness = match self.request("/readyz")? {
            response if response.status == 200 => DaemonReadinessV1::Ready,
            response if response.status == 503 => DaemonReadinessV1::NotReady,
            _ => return Err(ProbeError::InvalidResponse),
        };

        let version_response = self.request("/version")?;
        if version_response.status != 200 {
            return Err(ProbeError::InvalidResponse);
        }
        let wire: VersionWire = version_response.json()?;
        if wire.product_build.binary != "worldstreamd"
            || !version_field_is_bounded(&wire.product_build.product)
            || !version_field_is_bounded(&wire.product_build.build_version)
            || !version_field_is_bounded(&wire.product_build.source_revision)
        {
            return Err(ProbeError::InvalidResponse);
        }

        Ok(DaemonStatusV1 {
            schema: STATUS_SCHEMA_V1.to_owned(),
            connectivity: DaemonConnectivityV1::Connected,
            health: DaemonHealthV1::Live,
            readiness,
            version: Some(DaemonVersionV1 {
                product: wire.product_build.product,
                build_version: wire.product_build.build_version,
                source_revision: wire.product_build.source_revision,
            }),
            unavailable_reason: None,
        })
    }
}

impl DaemonStatusSource for HttpDaemonStatusSource {
    fn status(&self) -> DaemonStatusV1 {
        match self.connected_status() {
            Ok(status) => status,
            Err(ProbeError::Connection) => DaemonStatusV1::unavailable(),
            Err(ProbeError::InvalidResponse) => {
                DaemonStatusV1::unavailable_for(DaemonUnavailableReasonV1::InvalidResponse)
            }
        }
    }
}

/// Builds the Supervisor's deliberately small HTTP surface.
pub fn supervisor_router(source: impl DaemonStatusSource) -> Router {
    let source: Arc<dyn DaemonStatusSource> = Arc::new(source);
    Router::new()
        .route("/api/v1/daemon/status", get(daemon_status))
        .with_state(source)
}

/// Builds the complete bounded status and configured-lifecycle surface.
pub fn supervisor_router_with_lifecycle(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
) -> Router {
    supervisor_router(source).merge(lifecycle::lifecycle_router(lifecycle))
}

/// Builds the complete bounded Supervisor surface, including browser-safe
/// secret availability. Raw secret material has no route into this router.
pub fn supervisor_router_with_lifecycle_and_secrets(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
) -> Router {
    supervisor_router_with_lifecycle(source, lifecycle).merge(secrets::secret_status_router(vault))
}

/// Builds the bounded Supervisor surface with immutable installed Runner
/// Template catalogs and typed instance lifecycle controls.
pub fn supervisor_router_with_lifecycle_secrets_and_runners(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
) -> Router {
    supervisor_router_with_lifecycle_and_secrets(source, lifecycle, vault)
        .merge(runner_templates::runner_router(runners))
}

/// Builds the bounded Supervisor surface with the exact installed Activity
/// Pack catalog. Host authority is retained and resolved only by the injected
/// daemon source; it never crosses into the browser response.
pub fn supervisor_router_with_lifecycle_secrets_runners_and_activity_packs(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
) -> Router {
    supervisor_router_with_lifecycle_secrets_and_runners(source, lifecycle, vault, runners)
        .merge(activity_packs::activity_pack_router(activity_packs))
}

/// Builds the complete bounded Supervisor surface with host-authorized Room
/// inventory/detail forwarding. The Room source retains its own exact opaque
/// Host authority reference; no bearer or reference enters browser state.
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_and_rooms(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_and_activity_packs(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
    )
    .merge(rooms::room_router(rooms))
}

/// Builds the complete Supervisor surface with owner-only planning drafts.
/// Draft routes persist wizard state only and cannot create Rooms or authority.
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_and_drafts(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
    drafts: room_drafts::RoomDraftStoreV1,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_and_rooms(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
    )
    .merge(room_drafts::room_draft_router(drafts))
}

/// Builds the complete Supervisor surface with durable, pathless live-backup
/// orchestration. Backup destinations remain beneath the fixed local root.
#[allow(clippy::too_many_arguments)]
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_and_backups(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
    drafts: room_drafts::RoomDraftStoreV1,
    backups: backups::BackupOperationsV1,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_and_drafts(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
        drafts,
    )
    .merge(backups::backup_router(backups))
}

/// Builds the complete bounded Supervisor surface with one durable exact-once
/// Room-creation operation permanently keyed by each reviewed draft.
#[allow(clippy::too_many_arguments)]
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_and_creation(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
    drafts: room_drafts::RoomDraftStoreV1,
    backups: backups::BackupOperationsV1,
    room_creation: room_creation::RoomCreationSupervisorV1,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_and_backups(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
        drafts,
        backups,
    )
    .merge(room_creation::room_creation_router(room_creation))
}

/// Builds the complete bounded Supervisor surface with durable post-Genesis
/// participant and Runner-control setup orchestration.
#[allow(clippy::too_many_arguments)]
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_and_setup(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
    drafts: room_drafts::RoomDraftStoreV1,
    backups: backups::BackupOperationsV1,
    room_creation: room_creation::RoomCreationSupervisorV1,
    task_setup: task_setup::TaskSetupSupervisorV1,
    agent_profiles: agent_profiles::AgentProfileStoreV1,
    participant_handoff: participant_handoff::ParticipantHandoffBrokerV1,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_and_creation(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
        drafts,
        backups,
        room_creation,
    )
    .merge(task_setup::task_setup_router(task_setup))
    .merge(agent_profiles::agent_profile_router(agent_profiles))
    .merge(participant_handoff::participant_handoff_router(participant_handoff))
}

/// Builds the complete Supervisor surface with immutable Task Template
/// publication and draft-only instantiation. This route layer has no daemon
/// Room-creation capability.
#[allow(clippy::too_many_arguments)]
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_setup_and_templates(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
    drafts: room_drafts::RoomDraftStoreV1,
    backups: backups::BackupOperationsV1,
    room_creation: room_creation::RoomCreationSupervisorV1,
    task_setup: task_setup::TaskSetupSupervisorV1,
    agent_profiles: agent_profiles::AgentProfileStoreV1,
    participant_handoff: participant_handoff::ParticipantHandoffBrokerV1,
    task_templates: task_templates::TaskTemplateStoreV1,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_and_setup(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
        drafts,
        backups,
        room_creation,
        task_setup,
        agent_profiles,
        participant_handoff,
    )
    .merge(task_templates::task_template_router(task_templates))
}

/// Builds the production Studio surface with browser publication through named
/// owner-installed model-provider credentials.  The earlier helper remains for
/// internal callers that publish the legacy v1 opaque-reference representation.
#[allow(clippy::too_many_arguments)]
pub fn supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_setup_templates_and_model_provider_credentials(
    source: impl DaemonStatusSource,
    lifecycle: impl lifecycle::DaemonLifecycleControl,
    vault: secrets::FileSecretVaultV1,
    runners: runner_templates::RunnerSupervisorV1,
    activity_packs: impl activity_packs::DaemonActivityPackSource,
    rooms: impl rooms::DaemonRoomSource,
    drafts: room_drafts::RoomDraftStoreV1,
    backups: backups::BackupOperationsV1,
    room_creation: room_creation::RoomCreationSupervisorV1,
    task_setup: task_setup::TaskSetupSupervisorV1,
    agent_profiles: agent_profiles::AgentProfileStoreV1,
    model_provider_credentials: model_provider_credentials::ModelProviderCredentialRegistryV1,
    participant_handoff: participant_handoff::ParticipantHandoffBrokerV1,
    task_templates: task_templates::TaskTemplateStoreV1,
) -> Router {
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_and_creation(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
        drafts,
        backups,
        room_creation,
    )
    .merge(task_setup::task_setup_router(task_setup))
    .merge(agent_profiles::agent_profile_router_with_credentials(
        agent_profiles,
        model_provider_credentials,
    ))
    .merge(participant_handoff::participant_handoff_router(participant_handoff))
    .merge(task_templates::task_template_router(task_templates))
}

async fn daemon_status(State(source): State<Arc<dyn DaemonStatusSource>>) -> Json<DaemonStatusV1> {
    let fallback = DaemonStatusV1::unavailable_for(DaemonUnavailableReasonV1::ProbeFailed);
    let status = tokio::task::spawn_blocking(move || source.status())
        .await
        .unwrap_or(fallback);
    Json(status)
}

#[derive(Debug)]
enum ProbeError {
    Connection,
    InvalidResponse,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, ProbeError> {
        serde_json::from_slice(&self.body).map_err(|_| ProbeError::InvalidResponse)
    }
}

#[derive(Deserialize)]
struct HealthWire {
    status: String,
}

#[derive(Deserialize)]
struct VersionWire {
    product_build: ProductBuildWire,
}

#[derive(Deserialize)]
struct ProductBuildWire {
    product: String,
    binary: String,
    build_version: String,
    source_revision: String,
}

fn version_field_is_bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_VERSION_FIELD_BYTES
}

fn parse_http_response(bytes: &[u8]) -> Result<HttpResponse, ProbeError> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(ProbeError::InvalidResponse)?;
    let headers =
        std::str::from_utf8(&bytes[..separator]).map_err(|_| ProbeError::InvalidResponse)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(ProbeError::InvalidResponse)?;
    let body = bytes[(separator + 4)..].to_vec();
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use super::{
        DaemonConnectivityV1, DaemonHealthV1, DaemonReadinessV1, DaemonStatusSource,
        DaemonStatusV1, DaemonVersionV1, supervisor_router,
    };

    #[derive(Clone)]
    struct FixedStatus(DaemonStatusV1);

    impl DaemonStatusSource for FixedStatus {
        fn status(&self) -> DaemonStatusV1 {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn studio_status_reports_a_connected_daemon_through_the_bounded_api() {
        let expected = DaemonStatusV1 {
            schema: "worldstream/studio-daemon-status/v1".to_owned(),
            connectivity: DaemonConnectivityV1::Connected,
            health: DaemonHealthV1::Live,
            readiness: DaemonReadinessV1::Ready,
            version: Some(DaemonVersionV1 {
                product: "0.1.0".to_owned(),
                build_version: "0.1.0".to_owned(),
                source_revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            }),
            unavailable_reason: None,
        };
        let response = supervisor_router(FixedStatus(expected.clone()))
            .oneshot(
                Request::builder()
                    .uri("/api/v1/daemon/status")
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
        let actual: DaemonStatusV1 = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("status JSON: {error}"));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn studio_status_reports_an_unavailable_daemon_without_version_claims() {
        let expected = DaemonStatusV1::unavailable();
        let response = supervisor_router(FixedStatus(expected.clone()))
            .oneshot(
                Request::builder()
                    .uri("/api/v1/daemon/status")
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
        let actual: DaemonStatusV1 = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("status JSON: {error}"));
        assert_eq!(actual, expected);
        assert_eq!(actual.connectivity, DaemonConnectivityV1::Unavailable);
        assert_eq!(actual.health, DaemonHealthV1::Unavailable);
        assert_eq!(actual.readiness, DaemonReadinessV1::Unavailable);
        assert!(actual.version.is_none());
    }

    #[tokio::test]
    async fn supervisor_does_not_expose_an_arbitrary_command_route() {
        let response = supervisor_router(FixedStatus(DaemonStatusV1::unavailable()))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/command")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 404);
    }
}
