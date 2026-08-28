//! Room-seat-scoped browser adapter for bounded managed reference host control.
//!
//! Assignment identities remain Supervisor-internal: the adapter derives the
//! one persisted assignment from the exact Room and seat before invoking the
//! existing assignment-scoped host operation.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::Deserialize;

use crate::{
    agent_profiles::{AgentProfileErrorV1, AgentProfileStoreV1},
    managed_agent_host::{
        ManagedAgentHostErrorV1, ManagedAgentHostOperationsV1, ManagedAgentHostStatusV1,
    },
};

#[derive(Clone)]
struct ManagedHostSeatStateV1 {
    profiles: AgentProfileStoreV1,
    hosts: ManagedAgentHostOperationsV1,
}

/// Adds Room-seat routes while resolving the persisted assignment identity from
/// the exact Room and seat. The returned host status retains its normal
/// browser-safe assignment identifier for correlation.
pub fn managed_agent_host_seat_router(
    profiles: AgentProfileStoreV1,
    hosts: ManagedAgentHostOperationsV1,
) -> Router {
    Router::new()
        .route(
            "/api/v1/rooms/{room_id}/agent-seats/{seat_id}/managed-host/start",
            post(start),
        )
        .route(
            "/api/v1/rooms/{room_id}/agent-seats/{seat_id}/managed-host/retry",
            post(start),
        )
        .route(
            "/api/v1/rooms/{room_id}/agent-seats/{seat_id}/managed-host/stop",
            post(stop),
        )
        .with_state(ManagedHostSeatStateV1 { profiles, hosts })
}

async fn start(
    State(state): State<ManagedHostSeatStateV1>,
    AxumPath((room_id, seat_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ManagedAgentHostStatusV1>, StatusCode> {
    dispatch(
        state,
        room_id,
        seat_id,
        headers,
        body,
        ManagedHostSeatOperationV1::Start,
    )
    .await
}

/// Stops only the host already bound to the exact persisted Room seat.
///
/// This resolution reads the durable assignment mapping and deliberately does
/// not resolve provider credentials or construct a new launch plan.
async fn stop(
    State(state): State<ManagedHostSeatStateV1>,
    AxumPath((room_id, seat_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ManagedAgentHostStatusV1>, StatusCode> {
    dispatch(
        state,
        room_id,
        seat_id,
        headers,
        body,
        ManagedHostSeatOperationV1::Stop,
    )
    .await
}

#[derive(Clone, Copy)]
enum ManagedHostSeatOperationV1 {
    Start,
    Stop,
}

async fn dispatch(
    state: ManagedHostSeatStateV1,
    room_id: String,
    seat_id: String,
    headers: HeaderMap,
    body: Bytes,
    operation: ManagedHostSeatOperationV1,
) -> Result<Json<ManagedAgentHostStatusV1>, StatusCode> {
    if !is_json_content_type(&headers) {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let request: ManagedHostSeatActionRequestV1 =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if request.schema != "worldstream/studio-managed-agent-host-action/v1" {
        return Err(StatusCode::BAD_REQUEST);
    }
    tokio::task::spawn_blocking(move || {
        let assignment = state
            .profiles
            .assignment_for_room_seat(&room_id, &seat_id)
            .map_err(map_profile_error)?;
        match operation {
            ManagedHostSeatOperationV1::Start => state.hosts.start(&assignment.assignment_id),
            ManagedHostSeatOperationV1::Stop => state.hosts.stop(&assignment.assignment_id),
        }
        .map_err(map_host_error)
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    .map(Json)
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedHostSeatActionRequestV1 {
    schema: String,
}

const fn map_profile_error(error: AgentProfileErrorV1) -> StatusCode {
    match error {
        AgentProfileErrorV1::InvalidProfile | AgentProfileErrorV1::InvalidAssignment => {
            StatusCode::BAD_REQUEST
        }
        AgentProfileErrorV1::NotFound => StatusCode::NOT_FOUND,
        AgentProfileErrorV1::ImmutableRevisionConflict
        | AgentProfileErrorV1::ImmutableAssignmentConflict
        | AgentProfileErrorV1::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

const fn map_host_error(error: ManagedAgentHostErrorV1) -> StatusCode {
    match error {
        ManagedAgentHostErrorV1::InvalidInput => StatusCode::BAD_REQUEST,
        ManagedAgentHostErrorV1::ImmutableProfileConflict => StatusCode::CONFLICT,
        ManagedAgentHostErrorV1::AtCapacity => StatusCode::TOO_MANY_REQUESTS,
        ManagedAgentHostErrorV1::Unavailable | ManagedAgentHostErrorV1::Ambiguous => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        ManagedAgentHostErrorV1::Corrupt => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::is_json_content_type;
    use axum::http::{HeaderMap, HeaderValue, header::CONTENT_TYPE};

    #[test]
    fn action_requires_json_content_type() {
        let mut json = HeaderMap::new();
        json.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(is_json_content_type(&json));
        let mut text = HeaderMap::new();
        text.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(!is_json_content_type(&text));
        assert!(!is_json_content_type(&HeaderMap::new()));
    }
}
