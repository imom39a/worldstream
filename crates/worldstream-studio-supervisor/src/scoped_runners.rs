//! Room-seat controls delegate to the existing exact process owners.
use std::path::PathBuf;

use crate::{
    agent_profiles::{AgentExecutionBindingV1, AgentProfileStoreV1},
    managed_agent_host::{ManagedAgentHostOperationsV1, ManagedAgentHostStatusV1},
    room_drafts::{AgentAssignmentModeV1, RunnerTemplateRevisionReferenceV1},
    room_setup_operations::{runner_supports_pack, validate_profile_assignment},
    room_setup_spec::SetupAssignmentV1,
    runner_templates::{RunnerInstanceStatusV1, RunnerSupervisorV1, RunnerTemplateRegistryV1},
    task_setup::{
        TaskSetupErrorV1, TaskSetupSeatStatusV1, TaskSetupStateV1, TaskSetupStatusV1,
        TaskSetupSupervisorV1,
    },
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoomRunnerExecutionV1 {
    External {},
    ManagedTemplate {
        instance: RunnerInstanceStatusV1,
    },
    ManagedReference {
        host: Option<ManagedAgentHostStatusV1>,
    },
    Unavailable {},
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRunnerStatusV1 {
    pub version: String,
    pub operation: String,
    pub seat: String,
    pub room_id: String,
    pub runner_id: Option<String>,
    pub execution: RoomRunnerExecutionV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRunnerListV1 {
    pub version: String,
    pub runners: Vec<RoomRunnerStatusV1>,
}

#[derive(Clone)]
pub struct RoomRunnerControlV1 {
    setup: TaskSetupSupervisorV1,
    profiles: AgentProfileStoreV1,
    runners: RunnerSupervisorV1,
    installed_templates: PathBuf,
    hosts: Option<ManagedAgentHostOperationsV1>,
}

enum Target {
    External,
    Template(String),
    Reference(String),
}

impl RoomRunnerControlV1 {
    #[must_use]
    pub fn new(
        setup: TaskSetupSupervisorV1,
        profiles: AgentProfileStoreV1,
        runners: RunnerSupervisorV1,
        installed_templates: PathBuf,
    ) -> Self {
        Self {
            setup,
            profiles,
            runners,
            installed_templates,
            hosts: None,
        }
    }

    #[must_use]
    pub fn with_managed_hosts(mut self, hosts: ManagedAgentHostOperationsV1) -> Self {
        self.hosts = Some(hosts);
        self
    }

    fn target(
        &self,
        status: &TaskSetupStatusV1,
        seat: &TaskSetupSeatStatusV1,
        runner_id: &str,
    ) -> Result<Target, TaskSetupErrorV1> {
        if seat.agent_assignment == Some(AgentAssignmentModeV1::External) {
            return Ok(Target::External);
        }
        let managed = seat
            .managed_runner
            .as_ref()
            .ok_or(TaskSetupErrorV1::NotReady)?;
        if seat.agent_profile.is_some() {
            let assignment = self
                .profiles
                .assignment_for_room_seat(&status.room_id, &seat.seat_id)
                .map_err(|_| TaskSetupErrorV1::Unavailable)?;
            let exact = self.setup.managed_reference_metadata(&assignment)?;
            if assignment.draft_id != status.draft_id
                || exact.runner_id != runner_id
                || exact.instance_id != managed.instance_id
                || exact.template_id != managed.template_id
                || exact.template_revision != managed.template_revision
            {
                return Err(TaskSetupErrorV1::Unavailable);
            }
            match assignment.execution {
                AgentExecutionBindingV1::ManagedReference {
                    runner_id: assigned,
                    instance_id,
                    template_id,
                    template_revision,
                } if assigned == runner_id
                    && instance_id == managed.instance_id
                    && template_id == managed.template_id
                    && template_revision == managed.template_revision =>
                {
                    return Ok(Target::Reference(assignment.assignment_id));
                }
                AgentExecutionBindingV1::Managed {
                    runner_id: assigned,
                } if assigned == runner_id => {}
                _ => return Err(TaskSetupErrorV1::Unavailable),
            }
        }
        if !self
            .runners
            .task_runner_binding_matches(&managed.instance_id, runner_id)
        {
            return Err(TaskSetupErrorV1::Unavailable);
        }
        Ok(Target::Template(managed.instance_id.clone()))
    }

    fn inspect(
        &self,
        operation: &str,
        seat_id: &str,
    ) -> Result<RoomRunnerStatusV1, TaskSetupErrorV1> {
        let (status, runner_id) = self.setup.scoped_runner_snapshot(operation, seat_id)?;
        let seat = status
            .seats
            .iter()
            .find(|seat| seat.seat_id == seat_id)
            .ok_or(TaskSetupErrorV1::NotFound)?;
        let execution = match self.target(&status, seat, &runner_id) {
            Ok(Target::External) => RoomRunnerExecutionV1::External {},
            Ok(Target::Template(instance_id)) => self
                .runners
                .statuses()
                .instances
                .into_iter()
                .find(|instance| instance.instance_id == instance_id)
                .map_or(RoomRunnerExecutionV1::Unavailable {}, |instance| {
                    RoomRunnerExecutionV1::ManagedTemplate { instance }
                }),
            Ok(Target::Reference(assignment_id)) => match &self.hosts {
                Some(hosts) => RoomRunnerExecutionV1::ManagedReference {
                    host: hosts
                        .statuses()
                        .map_err(|_| TaskSetupErrorV1::Unavailable)?
                        .into_iter()
                        .find(|host| host.assignment_id == assignment_id),
                },
                None => RoomRunnerExecutionV1::Unavailable {},
            },
            Err(_) => RoomRunnerExecutionV1::Unavailable {},
        };
        Ok(RoomRunnerStatusV1 {
            version: "room_runner.v1".to_owned(),
            operation: operation.to_owned(),
            seat: seat_id.to_owned(),
            room_id: status.room_id,
            runner_id: Some(runner_id),
            execution,
        })
    }

    fn validate_launch(
        &self,
        status: &TaskSetupStatusV1,
        seat: &TaskSetupSeatStatusV1,
    ) -> Result<(), TaskSetupErrorV1> {
        if status.state != TaskSetupStateV1::Ready
            || seat.member_authority != "provisioned"
            || seat.runner_authority != "provisioned"
        {
            return Err(TaskSetupErrorV1::NotReady);
        }
        let managed = seat
            .managed_runner
            .as_ref()
            .ok_or(TaskSetupErrorV1::NotReady)?;
        let manifest = RunnerTemplateRegistryV1::review_installed(
            &self.installed_templates,
            &managed.template_id,
            &managed.template_revision,
        )
        .map_err(|_| TaskSetupErrorV1::NotReady)?;
        let exact = self.setup.exact_metadata(&status.draft_id)?;
        if !self
            .runners
            .approved_template_matches(&managed.instance_id, &manifest)
            || !runner_supports_pack(&manifest.compatibility, &exact.pack)
        {
            return Err(TaskSetupErrorV1::NotReady);
        }
        if let Some(profile) = &seat.agent_profile {
            let view = self
                .profiles
                .revision(&profile.profile_id, &profile.revision)
                .map_err(|_| TaskSetupErrorV1::NotReady)?;
            validate_profile_assignment(
                &SetupAssignmentV1 {
                    mode: AgentAssignmentModeV1::Managed,
                    agent_profile: Some(profile.clone()),
                    runner_template: Some(RunnerTemplateRevisionReferenceV1 {
                        template_id: managed.template_id.clone(),
                        revision: managed.template_revision.clone(),
                    }),
                },
                &view,
            )
            .map_err(|_| TaskSetupErrorV1::NotReady)?;
        }
        Ok(())
    }

    fn change(
        &self,
        operation: &str,
        seat_id: &str,
        start: bool,
    ) -> Result<RoomRunnerStatusV1, TaskSetupErrorV1> {
        let (status, runner_id) = self.setup.scoped_runner_snapshot(operation, seat_id)?;
        let seat = status
            .seats
            .iter()
            .find(|seat| seat.seat_id == seat_id)
            .ok_or(TaskSetupErrorV1::NotFound)?;
        let target = self.target(&status, seat, &runner_id)?;
        if matches!(target, Target::External) {
            return Err(TaskSetupErrorV1::NotReady);
        }
        // Stopping an owned exact target never depends on current launch approval.
        if start {
            self.validate_launch(&status, seat)?;
        }
        match target {
            Target::External => return Err(TaskSetupErrorV1::NotReady),
            Target::Template(instance) => {
                if start {
                    self.runners.start(&instance)
                } else {
                    self.runners.stop(&instance)
                }
                .ok_or(TaskSetupErrorV1::Unavailable)?;
            }
            Target::Reference(assignment) => {
                let hosts = self.hosts.as_ref().ok_or(TaskSetupErrorV1::Unavailable)?;
                if start {
                    hosts.start(&assignment)
                } else {
                    hosts.stop(&assignment)
                }
                .map_err(|_| TaskSetupErrorV1::Unavailable)?;
            }
        }
        self.inspect(operation, seat_id)
    }

    fn list(&self, operation: Option<&str>) -> Result<RoomRunnerListV1, TaskSetupErrorV1> {
        let setups = if let Some(operation) = operation {
            vec![self.setup.status(operation)?]
        } else {
            self.setup.statuses()?
        };
        let mut runners = Vec::new();
        for setup in setups {
            for seat in &setup.seats {
                if seat.principal_kind == Some(worldstream_protocol::PrincipalKind::Agent) {
                    runners.push(self.inspect(&setup.draft_id, &seat.seat_id)?);
                }
            }
        }
        Ok(RoomRunnerListV1 {
            version: "room_runners.v1".to_owned(),
            runners,
        })
    }
}

pub fn room_runner_router(control: RoomRunnerControlV1) -> Router {
    Router::new()
        .route("/api/v1/room-runners", get(list))
        .route(
            "/api/v1/room-setup-operations/{operation}/seats/{seat}/runner",
            get(inspect),
        )
        .route(
            "/api/v1/room-setup-operations/{operation}/seats/{seat}/runner/start",
            post(start),
        )
        .route(
            "/api/v1/room-setup-operations/{operation}/seats/{seat}/runner/stop",
            post(stop),
        )
        .with_state(control)
}

async fn list(State(control): State<RoomRunnerControlV1>, RawQuery(query): RawQuery) -> Response {
    let operation = match query {
        None => None,
        Some(query) => {
            let Some(operation) = query.strip_prefix("operation=") else {
                return respond::<RoomRunnerListV1>(Ok(Err(TaskSetupErrorV1::InvalidCreation)));
            };
            if operation.is_empty()
                || operation.len() > 64
                || !operation.as_bytes()[0].is_ascii_alphanumeric()
                || !operation
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return respond::<RoomRunnerListV1>(Ok(Err(TaskSetupErrorV1::InvalidCreation)));
            }
            Some(operation.to_owned())
        }
    };
    respond(tokio::task::spawn_blocking(move || control.list(operation.as_deref())).await)
}
async fn inspect(
    State(control): State<RoomRunnerControlV1>,
    Path((operation, seat)): Path<(String, String)>,
) -> Response {
    respond(tokio::task::spawn_blocking(move || control.inspect(&operation, &seat)).await)
}
async fn start(
    State(control): State<RoomRunnerControlV1>,
    Path((operation, seat)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    mutate(control, operation, seat, body, true).await
}
async fn stop(
    State(control): State<RoomRunnerControlV1>,
    Path((operation, seat)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    mutate(control, operation, seat, body, false).await
}
async fn mutate(
    control: RoomRunnerControlV1,
    operation: String,
    seat: String,
    body: Bytes,
    start: bool,
) -> Response {
    if !body.is_empty() {
        return respond::<RoomRunnerStatusV1>(Ok(Err(TaskSetupErrorV1::InvalidCreation)));
    }
    respond(tokio::task::spawn_blocking(move || control.change(&operation, &seat, start)).await)
}
fn respond<T: Serialize>(
    result: Result<Result<T, TaskSetupErrorV1>, tokio::task::JoinError>,
) -> Response {
    let mut response = match result {
        Ok(Ok(value)) => Json(value).into_response(),
        error => {
            let (status, code) = match error {
                Ok(Err(TaskSetupErrorV1::NotFound)) => {
                    (StatusCode::NOT_FOUND, "room_runner_not_found")
                }
                Ok(Err(TaskSetupErrorV1::InvalidCreation)) => {
                    (StatusCode::BAD_REQUEST, "room_runner_invalid")
                }
                Ok(Err(TaskSetupErrorV1::NotReady)) => {
                    (StatusCode::CONFLICT, "room_runner_not_managed_or_not_ready")
                }
                _ => (StatusCode::SERVICE_UNAVAILABLE, "room_runner_unavailable"),
            };
            (status, Json(serde_json::json!({"error":{"code":code}}))).into_response()
        }
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
