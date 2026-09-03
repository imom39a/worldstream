//! Actual accepted-socket evidence for managed local HTTP listeners.

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, Request, State, connect_info::Connected,
        rejection::JsonRejection,
    },
    http::StatusCode,
    middleware::{Next, from_fn},
    response::{IntoResponse, Response},
    routing::post,
    serve::IncomingStream,
};
use tokio::net::TcpListener;

use crate::verified_control::{ProofRequest, ProofService};

/// Evidence installed by the listener, never supplied by HTTP headers.
#[derive(Clone, Copy, Debug)]
pub struct AcceptedLocalSocket {
    pub peer: SocketAddr,
    pub local: Option<SocketAddr>,
}

impl Connected<IncomingStream<'_, TcpListener>> for AcceptedLocalSocket {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Self {
            peer: *stream.remote_addr(),
            local: stream.io().local_addr().ok(),
        }
    }
}

/// Preserves existing peer-based admission while retaining actual local evidence.
pub fn preserve_peer_information(router: Router) -> Router {
    router.layer(from_fn(preserve_peer))
}

async fn preserve_peer(mut request: Request, next: Next) -> Response {
    if let Some(ConnectInfo(socket)) = request
        .extensions()
        .get::<ConnectInfo<AcceptedLocalSocket>>()
        .copied()
    {
        request.extensions_mut().insert(ConnectInfo(socket.peer));
    }
    next.run(request).await
}

/// The only unauthenticated managed proof endpoint. It discloses no authority.
pub fn local_proof_router(proof: ProofService) -> Router {
    Router::new()
        .route("/api/v1/control/proof", post(answer_proof))
        .layer(DefaultBodyLimit::max(512))
        .with_state(proof)
}

async fn answer_proof(
    State(proof): State<ProofService>,
    ConnectInfo(socket): ConnectInfo<AcceptedLocalSocket>,
    request: Result<Json<ProofRequest>, JsonRejection>,
) -> Response {
    if !socket.peer.ip().is_loopback() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (Some(local), Ok(Json(request))) = (socket.local, request) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match proof.respond(&request, local) {
        Ok(document) => Json(document).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
