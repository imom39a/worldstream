//! Managed Runtime control on the same listener as its existing operator API.

use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use tokio::sync::watch;
use worldstream_studio_supervisor::{
    managed_http::{AcceptedLocalSocket, local_proof_router, preserve_peer_information},
    verified_control::ProofService,
};

use crate::{OperatorState, ReadinessProbeResult, RuntimeReadiness, operator_router};

#[derive(Clone)]
struct RuntimeControlState {
    proof: ProofService,
    readiness: RuntimeReadiness,
    shutdown: watch::Sender<bool>,
}

/// Adds generation-scoped status/stop without granting Host or Room authority.
/// Serve with `AcceptedLocalSocket` connection information from the real listener.
pub fn managed_runtime_router(
    state: OperatorState,
    proof: ProofService,
    shutdown: watch::Sender<bool>,
) -> Router {
    let control = RuntimeControlState {
        proof: proof.clone(),
        readiness: state.readiness,
        shutdown,
    };
    preserve_peer_information(
        operator_router(state)
            .merge(local_proof_router(proof))
            .merge(
                Router::new()
                    .route("/api/v1/control/status", get(status))
                    .route("/api/v1/control/stop", post(stop))
                    .with_state(control),
            ),
    )
}

fn admitted(
    control: &RuntimeControlState,
    socket: AcceptedLocalSocket,
    headers: &HeaderMap,
) -> bool {
    socket.peer.ip().is_loopback()
        && socket.local.is_some()
        && control.proof.authenticate_runtime(headers)
}

#[derive(Serialize)]
struct ManagedRuntimeStatus {
    schema: &'static str,
    ready: bool,
}

async fn status(
    State(control): State<RuntimeControlState>,
    ConnectInfo(socket): ConnectInfo<AcceptedLocalSocket>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if !admitted(&control, socket, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(ManagedRuntimeStatus {
        schema: "worldstream/managed-runtime-status/v1",
        ready: matches!(control.readiness.probe(), ReadinessProbeResult::Ready),
    })
    .into_response()
}

#[derive(Serialize)]
struct StopAccepted {
    schema: &'static str,
    accepted: bool,
}

async fn stop(
    State(control): State<RuntimeControlState>,
    ConnectInfo(socket): ConnectInfo<AcceptedLocalSocket>,
    headers: HeaderMap,
) -> Response {
    if !admitted(&control, socket, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if control.shutdown.send(true).is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    (
        StatusCode::ACCEPTED,
        Json(StopAccepted {
            schema: "worldstream/managed-runtime-stop/v1",
            accepted: true,
        }),
    )
        .into_response()
}
