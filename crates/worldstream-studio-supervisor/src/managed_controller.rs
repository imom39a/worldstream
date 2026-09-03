//! Managed Controller-only control; Runtime lifecycle is a separate capability.

use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::{
    control_access::ControlAccess,
    control_admission::protect_managed_operator_routes,
    lifecycle::{
        ConfiguredDaemonLifecycle, DaemonLifecycleControl, DaemonLifecycleFailureV1,
        DaemonLifecycleStateV1, DaemonLifecycleV1,
    },
    managed_http::preserve_peer_information,
    managed_lifecycle::{
        LifecycleAction, LifecycleError, LifecycleLogEntry, LifecycleStage, LifecycleStatus,
        ManagedLifecycle, RuntimeObservation,
    },
    verified_control::ProofService,
};

/// Keeps every retained lifecycle alias on the same ownership-safe implementation.
#[derive(Clone)]
pub enum ControllerLifecycle {
    Foreground(ConfiguredDaemonLifecycle),
    Managed(ManagedLifecycle),
}

impl DaemonLifecycleControl for ControllerLifecycle {
    fn lifecycle(&self) -> DaemonLifecycleV1 {
        self.legacy(None)
    }
    fn start(&self) -> DaemonLifecycleV1 {
        self.legacy(Some(LifecycleAction::Start))
    }
    fn stop(&self) -> DaemonLifecycleV1 {
        self.legacy(Some(LifecycleAction::Stop))
    }
    fn restart(&self) -> DaemonLifecycleV1 {
        self.legacy(Some(LifecycleAction::Restart))
    }
}

type Operation = fn(
    &ManagedLifecycle,
) -> Result<
    crate::managed_lifecycle::LifecycleOperation,
    crate::managed_lifecycle::LifecycleError,
>;

impl ControllerLifecycle {
    fn legacy(&self, operation: Option<LifecycleAction>) -> DaemonLifecycleV1 {
        match self {
            Self::Foreground(control) => match operation {
                None => control.lifecycle(),
                Some(LifecycleAction::Start) => control.start(),
                Some(LifecycleAction::Stop) => control.stop(),
                Some(LifecycleAction::Restart) => control.restart(),
            },
            Self::Managed(control) => {
                let failed = operation.is_some_and(|action| match action {
                    LifecycleAction::Start => control.start().is_err(),
                    LifecycleAction::Stop => control.stop().is_err(),
                    LifecycleAction::Restart => control.restart().is_err(),
                });
                let status = control.status();
                let observed = status
                    .as_ref()
                    .map_or(RuntimeObservation::Unavailable, |status| status.runtime);
                let incomplete = status.as_ref().is_ok_and(|status| {
                    status
                        .operation
                        .as_ref()
                        .is_some_and(|op| op.stage != LifecycleStage::Complete)
                });
                DaemonLifecycleV1 {
                    schema: "worldstream/studio-daemon-lifecycle/v1".into(),
                    state: if failed || incomplete {
                        DaemonLifecycleStateV1::Failed
                    } else {
                        match observed {
                            RuntimeObservation::Stopped => DaemonLifecycleStateV1::Stopped,
                            RuntimeObservation::Ready => DaemonLifecycleStateV1::Running,
                            RuntimeObservation::Starting => DaemonLifecycleStateV1::Starting,
                            RuntimeObservation::Unavailable | RuntimeObservation::Unmanaged => {
                                DaemonLifecycleStateV1::Unavailable
                            }
                        }
                    },
                    operation_id: status
                        .as_ref()
                        .ok()
                        .and_then(|status| status.operation.as_ref())
                        .map_or(0, |op| op.operation_id),
                    managed_by_supervisor: observed == RuntimeObservation::Ready,
                    failure: (failed
                        || incomplete
                        || matches!(
                            observed,
                            RuntimeObservation::Unavailable | RuntimeObservation::Unmanaged
                        ))
                    .then(|| DaemonLifecycleFailureV1 {
                        code: "managed_control_incomplete".into(),
                        explanation: "Managed process control could not be completed or verified."
                            .into(),
                        next_action:
                            "Inspect retained server status before retrying the same operation."
                                .into(),
                    }),
                }
            }
        }
    }
}

/// Closed lifecycle response. A partial operation is never a successful completion.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedServerResponse {
    pub schema: String,
    pub completed: bool,
    pub server: LifecycleStatus,
}

/// Closed diagnostic events. Subprocess output and credentials are never included.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedServerLogsResponse {
    pub schema: String,
    pub entries: Vec<LifecycleLogEntry>,
}

/// Fixed authenticated operations; this router must be wrapped by admission.
pub fn managed_lifecycle_router(control: ManagedLifecycle) -> Router {
    Router::new()
        .route("/api/v1/control/server/status", get(server_status))
        .route("/api/v1/control/server/logs", get(server_logs))
        .route("/api/v1/control/server/start", post(server_start))
        .route("/api/v1/control/server/stop", post(server_stop))
        .route("/api/v1/control/server/restart", post(server_restart))
        .with_state(control)
}

async fn server_logs(
    State(control): State<ManagedLifecycle>,
    request: Request,
) -> Result<Json<ManagedServerLogsResponse>, StatusCode> {
    let tail = request
        .uri()
        .query()
        .and_then(|query| query.strip_prefix("tail="))
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|tail| (1..=1000).contains(tail))
        .ok_or(StatusCode::BAD_REQUEST)?;
    tokio::task::spawn_blocking(move || {
        let entries = control
            .logs(tail)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        Ok(Json(ManagedServerLogsResponse {
            schema: "worldstream/managed-server-logs/v1".into(),
            entries,
        }))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}

async fn server_status(
    State(control): State<ManagedLifecycle>,
) -> Result<Json<ManagedServerResponse>, StatusCode> {
    server_operation(control, None).await
}
async fn server_start(
    State(control): State<ManagedLifecycle>,
) -> Result<Json<ManagedServerResponse>, StatusCode> {
    server_operation(control, Some(ManagedLifecycle::start)).await
}
async fn server_stop(
    State(control): State<ManagedLifecycle>,
) -> Result<Json<ManagedServerResponse>, StatusCode> {
    server_operation(control, Some(ManagedLifecycle::stop)).await
}
async fn server_restart(
    State(control): State<ManagedLifecycle>,
) -> Result<Json<ManagedServerResponse>, StatusCode> {
    server_operation(control, Some(ManagedLifecycle::restart)).await
}
async fn server_operation(
    control: ManagedLifecycle,
    operation: Option<Operation>,
) -> Result<Json<ManagedServerResponse>, StatusCode> {
    tokio::task::spawn_blocking(move || {
        let result = operation.map(|action| action(&control));
        if matches!(
            result,
            Some(Err(LifecycleError::BoundRunnerRestartUnsupported))
        ) {
            return Err(StatusCode::CONFLICT);
        }
        let completed = result.is_none_or(|result| result.is_ok());
        let server = control
            .status()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        Ok(Json(ManagedServerResponse {
            schema: "worldstream/managed-server/v1".into(),
            completed,
            server,
        }))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}

/// Adds the fixed Controller shutdown endpoint and protects the complete graph.
/// Only the bounded socket-proof handler bypasses installation admission.
pub fn managed_controller_router(
    router: Router,
    control: ControlAccess,
    proof: ProofService,
    shutdown: watch::Sender<bool>,
) -> Router {
    let router = router.merge(
        Router::new()
            .route("/api/v1/control/controller-stop", post(stop))
            .with_state(shutdown),
    );
    preserve_peer_information(protect_managed_operator_routes(router, control, proof))
}

#[derive(Serialize)]
struct StopAccepted {
    schema: &'static str,
    accepted: bool,
}

async fn stop(
    State(shutdown): State<watch::Sender<bool>>,
) -> Result<(StatusCode, Json<StopAccepted>), StatusCode> {
    shutdown
        .send(true)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(StopAccepted {
            schema: "worldstream/managed-controller-stop/v1",
            accepted: true,
        }),
    ))
}
