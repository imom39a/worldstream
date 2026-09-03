//! Thin CLI adapter over the shared proof-bound Controller and lifecycle APIs.

use super::{
    cli_contract::{OperatorCommand, ServerCommand},
    cli_report::{CommandOutcome, CommandReport, LifecycleStage},
};
use std::{env, path::PathBuf, time::Duration};
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_server::CommonConfigArgs;
use worldstream_studio_supervisor::{
    managed_controller::{ManagedServerLogsResponse, ManagedServerResponse},
    managed_lifecycle::{LifecycleStage as BackendStage, RuntimeObservation},
    operator_connection::{ControllerExecutables, OperatorConnection},
};

#[allow(
    clippy::too_many_lines,
    reason = "keep the closed server command dispatch and outcome mapping together"
)]
pub fn execute(command: &OperatorCommand, config: &CommonConfigArgs) -> Option<CommandReport> {
    let OperatorCommand::Server { command } = command else {
        return None;
    };
    let logs_path;
    let (name, options, method, path) = match command {
        ServerCommand::Status(options) => (
            "server status",
            options,
            "GET",
            "/api/v1/control/server/status",
        ),
        ServerCommand::Start(options) => (
            "server start",
            options,
            "POST",
            "/api/v1/control/server/start",
        ),
        ServerCommand::Stop(options) => (
            "server stop",
            options,
            "POST",
            "/api/v1/control/server/stop",
        ),
        ServerCommand::Restart(options) => (
            "server restart",
            options,
            "POST",
            "/api/v1/control/server/restart",
        ),
        ServerCommand::ControllerStop(options) => ("server controller-stop", options, "POST", ""),
        ServerCommand::Logs(args) => {
            logs_path = format!("/api/v1/control/server/logs?tail={}", args.tail);
            ("server logs", &args.options, "GET", logs_path.as_str())
        }
        ServerCommand::RotateControlCredential(_) => return None,
    };
    let unavailable = || CommandReport::unavailable_server(name, &options.state_dir);
    let Ok(connection) = OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(options.timeout_seconds.into()),
    ) else {
        return Some(unavailable());
    };
    if matches!(command, ServerCommand::Start(_)) {
        let prepared = (|| {
            let selected = config
                .config
                .clone()
                .or_else(|| env::var_os("WORLDSTREAM_CONFIG").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from(".worldstream/worldstream.toml"));
            let directory = env::current_dir().map_err(|_| ())?;
            let load_path = connection
                .start_config_path(&selected, &directory)
                .map_err(|_| ())?;
            let loader = ConfigLoader::from_process(
                Some(load_path),
                CliOverrides {
                    bind: config.bind,
                    storage_profile: config.storage_profile,
                    data_dir: config.data_dir.clone(),
                },
            )
            .map_err(|_| ())?;
            let executables = shipped_executables()?;
            connection
                .ensure_started(&loader, &selected, &directory, &executables)
                .map_err(|_| ())
        })();
        if prepared.is_err() {
            return Some(unavailable());
        }
    }
    if matches!(command, ServerCommand::ControllerStop(_)) {
        return Some(match connection.stop_controller() {
            Ok(()) => CommandReport::new(name, CommandOutcome::Complete),
            Err(_) => unavailable(),
        });
    }
    if let ServerCommand::Logs(args) = command {
        let response = connection
            .request(method, path, b"")
            .ok()
            .filter(|response| response.status == 200)
            .and_then(|response| {
                serde_json::from_slice::<ManagedServerLogsResponse>(&response.body).ok()
            })
            .filter(|response| {
                response.schema == "worldstream/managed-server-logs/v1"
                    && response.entries.len() <= usize::from(args.tail)
                    && response
                        .entries
                        .iter()
                        .all(|entry| entry.sequence > 0 && entry.operation_id > 0)
                    && response
                        .entries
                        .windows(2)
                        .all(|pair| pair[0].sequence < pair[1].sequence)
            });
        return Some(match response {
            Some(response) => CommandReport::logs(name, response.entries),
            None => unavailable(),
        });
    }
    let result = connection.request(method, path, b"").ok();
    if matches!(command, ServerCommand::Restart(_))
        && result
            .as_ref()
            .is_some_and(|response| response.status == 409)
    {
        return Some(CommandReport::bound_runner_restart_unsupported());
    }
    let result = result
        .filter(|response| response.status == 200)
        .and_then(|response| serde_json::from_slice::<ManagedServerResponse>(&response.body).ok())
        .filter(|response| response.schema == "worldstream/managed-server/v1");
    let Some(response) = result else {
        return Some(unavailable());
    };
    let outcome = if let Some(operation) = &response.server.operation
        && operation.stage != BackendStage::Complete
    {
        CommandOutcome::PartialLifecycle {
            stage: match operation.stage {
                BackendStage::ManagedRunnerStop => LifecycleStage::ManagedRunnerStop,
                BackendStage::RuntimeStop => LifecycleStage::RuntimeStop,
                BackendStage::RuntimeRestart => LifecycleStage::RuntimeRestart,
                BackendStage::ManagedRunnerRestore => LifecycleStage::ManagedRunnerRestore,
                BackendStage::Complete => unreachable!("incomplete operation checked above"),
            },
        }
    } else if !response.completed {
        CommandOutcome::Failed
    } else if matches!(
        response.server.runtime,
        RuntimeObservation::Unavailable
            | RuntimeObservation::Unmanaged
            | RuntimeObservation::Starting
    ) {
        CommandOutcome::StaleEvidence
    } else {
        CommandOutcome::Complete
    };
    Some(CommandReport::server(name, outcome, response.server))
}

fn shipped_executables() -> Result<ControllerExecutables, ()> {
    let cli = env::current_exe().map_err(|_| ())?;
    let directory = cli.parent().ok_or(())?;
    let binary =
        |name: &str| -> PathBuf { directory.join(format!("{name}{}", env::consts::EXE_SUFFIX)) };
    Ok(ControllerExecutables {
        controller: binary("worldstream-studio-supervisor"),
        runtime: binary("worldstreamd"),
        assignment_mcp: binary("worldstream-assignment-mcp"),
    })
}
