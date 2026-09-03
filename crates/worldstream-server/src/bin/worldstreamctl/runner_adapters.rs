//! Public seat selectors adapt to existing owned Runner managers.

use super::cli_contract::{CommandOptions, RunnerCommand, SeatArgs};
use std::time::Duration;
use worldstream_studio_supervisor::{
    managed_agent_host::ManagedAgentHostStateV1,
    operator_connection::OperatorConnection,
    runner_templates::RunnerInstanceStateV1,
    scoped_runners::{RoomRunnerExecutionV1, RoomRunnerListV1, RoomRunnerStatusV1},
};

#[derive(Debug)]
pub enum RunnerExecution {
    Observed(RoomRunnerStatusV1),
    Listed(RoomRunnerListV1),
    Partial {
        operation: String,
        seat: String,
        status: Option<RoomRunnerStatusV1>,
    },
    Failed(RoomRunnerStatusV1),
    Rejected,
    Unavailable,
}

#[derive(Clone, Copy)]
enum Action {
    Inspect,
    Start,
    Stop,
}

pub fn execute(command: &RunnerCommand) -> Option<RunnerExecution> {
    match command {
        RunnerCommand::List(arguments) => {
            Some(list(&arguments.options, arguments.operation.as_deref()))
        }
        RunnerCommand::Inspect(arguments) => Some(operate(arguments, Action::Inspect)),
        RunnerCommand::Start(arguments) => Some(operate(arguments, Action::Start)),
        RunnerCommand::Stop(arguments) => Some(operate(arguments, Action::Stop)),
        RunnerCommand::ExportCredentials(_) => None,
    }
}

fn connection(options: &CommandOptions) -> Option<OperatorConnection> {
    OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(u64::from(options.timeout_seconds)),
    )
    .ok()
}

fn list(options: &CommandOptions, operation: Option<&str>) -> RunnerExecution {
    let Some(connection) = connection(options) else {
        return RunnerExecution::Unavailable;
    };
    let path = operation.map_or_else(
        || "/api/v1/room-runners".to_owned(),
        |operation| format!("/api/v1/room-runners?operation={operation}"),
    );
    let Ok(response) = connection.request("GET", &path, &[]) else {
        return RunnerExecution::Unavailable;
    };
    if matches!(response.status, 400 | 404 | 409) {
        return RunnerExecution::Rejected;
    }
    if response.status != 200 {
        return RunnerExecution::Unavailable;
    }
    let Ok(list) = serde_json::from_slice::<RoomRunnerListV1>(&response.body) else {
        return RunnerExecution::Unavailable;
    };
    if list.version != "room_runners.v1" {
        return RunnerExecution::Unavailable;
    }
    RunnerExecution::Listed(list)
}

fn operate(arguments: &SeatArgs, action: Action) -> RunnerExecution {
    let Some(connection) = connection(&arguments.options) else {
        return RunnerExecution::Unavailable;
    };
    let (method, suffix) = match action {
        Action::Inspect => ("GET", ""),
        Action::Start => ("POST", "/start"),
        Action::Stop => ("POST", "/stop"),
    };
    let path = format!(
        "/api/v1/room-setup-operations/{}/seats/{}/runner{suffix}",
        arguments.operation, arguments.seat
    );
    let uncertain = |status| match action {
        Action::Inspect => RunnerExecution::Unavailable,
        _ => RunnerExecution::Partial {
            operation: arguments.operation.clone(),
            seat: arguments.seat.clone(),
            status,
        },
    };
    let Ok(response) = connection.request(method, &path, &[]) else {
        return uncertain(None);
    };
    if matches!(response.status, 400 | 404 | 409) {
        return RunnerExecution::Rejected;
    }
    if response.status != 200 {
        return uncertain(None);
    }
    let Ok(status) = serde_json::from_slice::<RoomRunnerStatusV1>(&response.body) else {
        return uncertain(None);
    };
    if status.version != "room_runner.v1"
        || status.operation != arguments.operation
        || status.seat != arguments.seat
    {
        return uncertain(None);
    }
    if matches!(action, Action::Inspect) {
        return RunnerExecution::Observed(status);
    }
    let completed = match &status.execution {
        RoomRunnerExecutionV1::External {} => return RunnerExecution::Rejected,
        RoomRunnerExecutionV1::Unavailable {} => return uncertain(Some(status)),
        RoomRunnerExecutionV1::ManagedTemplate { instance } => {
            if matches!(
                instance.state,
                RunnerInstanceStateV1::Failed | RunnerInstanceStateV1::Unavailable
            ) {
                return RunnerExecution::Failed(status);
            }
            matches!(
                (action, instance.state),
                (Action::Start, RunnerInstanceStateV1::Running)
                    | (Action::Stop, RunnerInstanceStateV1::Stopped)
            )
        }
        RoomRunnerExecutionV1::ManagedReference { host } => match host {
            Some(host) => {
                if host.state == ManagedAgentHostStateV1::NeedsAttention {
                    return RunnerExecution::Failed(status);
                }
                matches!(
                    (action, host.state),
                    (Action::Start, ManagedAgentHostStateV1::Running)
                        | (Action::Stop, ManagedAgentHostStateV1::Stopped)
                )
            }
            None => matches!(action, Action::Stop),
        },
    };
    if completed {
        RunnerExecution::Observed(status)
    } else {
        uncertain(Some(status))
    }
}
