//! Explicit scoped credential delivery. These documents cross only the
//! authenticated operator transport into an explicitly protected output file;
//! they are deliberately not Debug or ordinary operator-report payloads.

use std::net::SocketAddr;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use worldstream_protocol::{
    PackReference, RunnerMembershipProvisionTargetV1, SealedCapabilityBearerV1,
};

use crate::task_setup::{TaskSetupErrorV1, TaskSetupSupervisorV1};

/// Secret-bearing direct-client file, never a CLI report.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipCredentialsV1 {
    pub schema: String,
    pub operation: String,
    pub seat: String,
    pub runtime_url: String,
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub pack: PackReference,
    pub role: String,
    pub scopes: Vec<String>,
    pub bearer: SealedCapabilityBearerV1,
}

/// Secret-bearing external-Runner file. It carries no Membership credential.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerCredentialsV1 {
    pub schema: String,
    pub operation: String,
    pub seat: String,
    pub runtime_url: String,
    pub runner_id: String,
    pub owner_principal_id: String,
    pub pack: PackReference,
    pub permitted_memberships: Vec<RunnerMembershipProvisionTargetV1>,
    pub scopes: Vec<String>,
    pub bearer: SealedCapabilityBearerV1,
}

#[derive(Clone)]
struct CredentialRoutes {
    setup: TaskSetupSupervisorV1,
    daemon: SocketAddr,
}

/// These routes must be merged inside the authenticated operator boundary;
/// unlike browser redemption routes they are never Origin-only exceptions.
pub fn scoped_credentials_router(setup: TaskSetupSupervisorV1, daemon: SocketAddr) -> Router {
    Router::new()
        .route(
            "/api/v1/room-setup-operations/{operation}/seats/{seat}/membership-credentials",
            post(export_membership),
        )
        .route(
            "/api/v1/room-setup-operations/{operation}/seats/{seat}/runner-credentials",
            post(export_runner),
        )
        .with_state(CredentialRoutes { setup, daemon })
}

async fn export_membership(
    State(routes): State<CredentialRoutes>,
    Path((operation, seat)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if !body.is_empty() {
        return export_error(TaskSetupErrorV1::InvalidCreation);
    }
    match tokio::task::spawn_blocking(move || {
        routes
            .setup
            .export_membership_credentials(&operation, &seat, routes.daemon)
    })
    .await
    {
        Ok(Ok(document)) => no_store(Json(document).into_response()),
        Ok(Err(error)) => export_error(error),
        Err(_) => export_error(TaskSetupErrorV1::Unavailable),
    }
}

async fn export_runner(
    State(routes): State<CredentialRoutes>,
    Path((operation, seat)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if !body.is_empty() {
        return export_error(TaskSetupErrorV1::InvalidCreation);
    }
    match tokio::task::spawn_blocking(move || {
        routes
            .setup
            .export_runner_credentials(&operation, &seat, routes.daemon)
    })
    .await
    {
        Ok(Ok(document)) => no_store(Json(document).into_response()),
        Ok(Err(error)) => export_error(error),
        Err(_) => export_error(TaskSetupErrorV1::Unavailable),
    }
}

fn export_error(error: TaskSetupErrorV1) -> Response {
    let (status, code) = match error {
        TaskSetupErrorV1::NotFound => (StatusCode::NOT_FOUND, "scoped_seat_not_found"),
        TaskSetupErrorV1::InvalidCreation => (StatusCode::BAD_REQUEST, "scoped_seat_invalid"),
        TaskSetupErrorV1::NotReady => (StatusCode::CONFLICT, "scoped_authority_not_provisioned"),
        TaskSetupErrorV1::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "scoped_authority_unavailable",
        ),
    };
    no_store((status, Json(serde_json::json!({"error":{"code":code}}))).into_response())
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
