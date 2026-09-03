//! Room-addressed adapter for the retained setup launch intent. No new workflow store.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    room_creation::RoomCreationSupervisorV1,
    task_setup::{
        TaskLaunchApplicabilityV1, TaskLaunchStateV1, TaskLaunchStatusV1, TaskReadinessV1,
        TaskSetupErrorV1, TaskSetupStateV1, TaskSetupStatusV1, TaskSetupSupervisorV1,
    },
};

/// Live operational evidence only; not canonical Room state or participant content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomLaunchAssessmentV1 {
    pub version: String,
    pub operation: String,
    pub room_id: String,
    pub provisioning_complete: bool,
    pub applicability: TaskLaunchApplicabilityV1,
    pub readiness: TaskReadinessV1,
    pub launch: Option<TaskLaunchStatusV1>,
}

impl From<&TaskSetupStatusV1> for RoomLaunchAssessmentV1 {
    fn from(status: &TaskSetupStatusV1) -> Self {
        Self {
            version: "room_launch_assessment.v1".to_owned(),
            operation: status.draft_id.clone(),
            room_id: status.room_id.clone(),
            provisioning_complete: status.state == TaskSetupStateV1::Ready,
            applicability: status.launch_applicability,
            readiness: status.readiness.clone(),
            launch: status.launch.clone(),
        }
    }
}

#[derive(Clone)]
struct LaunchAdapter {
    creation: RoomCreationSupervisorV1,
    setup: TaskSetupSupervisorV1,
}

impl LaunchAdapter {
    fn operation(&self, room: &str) -> Result<String, StatusCode> {
        if room.len() != 26 || !room.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let statuses = self
            .creation
            .statuses()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let mut matching = statuses
            .into_iter()
            .filter(|status| status.room_id.as_deref() == Some(room));
        let operation = matching.next().ok_or(StatusCode::NOT_FOUND)?.draft_id;
        if matching.next().is_some() {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(operation)
    }

    fn request(
        &self,
        room: &str,
        launch: bool,
    ) -> Result<(StatusCode, RoomLaunchAssessmentV1), StatusCode> {
        let operation = self.operation(room)?;
        let status = self
            .setup
            .status(&operation)
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !launch {
            return Ok((StatusCode::OK, (&status).into()));
        }
        if status.launch_applicability == TaskLaunchApplicabilityV1::ActiveAtGenesis {
            return Ok((StatusCode::CONFLICT, (&status).into()));
        }
        match self.setup.launch(&operation) {
            Ok(status) => {
                let code = if status
                    .launch
                    .as_ref()
                    .is_some_and(|launch| launch.state == TaskLaunchStateV1::Launched)
                {
                    StatusCode::OK
                } else {
                    StatusCode::ACCEPTED
                };
                Ok((code, (&status).into()))
            }
            Err(TaskSetupErrorV1::NotReady) => {
                let current = self
                    .setup
                    .status(&operation)
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                Ok((StatusCode::CONFLICT, (&current).into()))
            }
            Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
}

/// Must be merged before the Controller's complete operator admission layer.
pub fn room_launch_router(
    creation: RoomCreationSupervisorV1,
    setup: TaskSetupSupervisorV1,
) -> Router {
    Router::new()
        .route("/api/v1/rooms/{room}/launch", get(inspect).post(launch))
        .layer(DefaultBodyLimit::max(1))
        .with_state(LaunchAdapter { creation, setup })
}

async fn inspect(State(adapter): State<LaunchAdapter>, Path(room): Path<String>) -> Response {
    request(adapter, room, false).await
}

async fn launch(
    State(adapter): State<LaunchAdapter>,
    Path(room): Path<String>,
    body: Bytes,
) -> Response {
    if !body.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    request(adapter, room, true).await
}

async fn request(adapter: LaunchAdapter, room: String, launch: bool) -> Response {
    match tokio::task::spawn_blocking(move || adapter.request(&room, launch)).await {
        Ok(Ok((code, assessment))) => (code, Json(assessment)).into_response(),
        Ok(Err(code)) => code.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
