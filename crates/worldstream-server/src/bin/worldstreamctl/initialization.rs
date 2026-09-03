//! Thin adapters for explicitly requested protected local initialization.

use super::{
    cli_contract::{CommandOptions, InitArgs, OperatorCommand, ServerCommand},
    cli_report::{CommandOutcome, CommandReport, InitializationMode, InitializationOutput},
};
use std::env;
use worldstream_runtime::CliOverrides;
use worldstream_server::CommonConfigArgs;
use worldstream_studio_supervisor::control_access::{ControlAccess, ControlAccessError};
use worldstream_studio_supervisor::initialization_imports::{
    ImportError, InitializationImportRequest, apply_imports, preview_imports,
};
use worldstream_studio_supervisor::local_initialization::{
    InitializationRequest, initialize_local,
};

/// Local-only initialization and explicitly reviewed prerequisite imports.
pub fn execute(command: &OperatorCommand, config: &CommonConfigArgs) -> Option<CommandReport> {
    if let OperatorCommand::Server {
        command: ServerCommand::RotateControlCredential(options),
    } = command
    {
        return Some(rotate_control_access(options));
    }
    let OperatorCommand::Init(args) = command else {
        return None;
    };
    if has_imports(args) {
        return Some(import_prerequisites(args, config));
    }
    Some(initialize(args, config).unwrap_or_else(|()| {
        CommandReport::new(
            "init",
            if args.preview {
                CommandOutcome::Rejected
            } else {
                CommandOutcome::Failed
            },
        )
    }))
}

fn rotate_control_access(options: &CommandOptions) -> CommandReport {
    let outcome = match ControlAccess::open(&options.state_dir).and_then(|access| access.rotate()) {
        Ok(()) => CommandOutcome::Complete,
        Err(ControlAccessError::PublicationUncertain) => CommandOutcome::Failed,
        Err(_) => CommandOutcome::Rejected,
    };
    CommandReport::new("server rotate-control-credential", outcome)
}

fn has_imports(args: &InitArgs) -> bool {
    !args.runner_template.is_empty()
        || !args.provider_declaration.is_empty()
        || !args.client_declaration.is_empty()
        || !args.agent_profile.is_empty()
}

fn initialize(args: &InitArgs, config: &CommonConfigArgs) -> Result<CommandReport, ()> {
    let request = initialization_request(args, config)?;
    let receipt = initialize_local(&request).map_err(|_| ())?;
    let expected_mode = if args.preview {
        "preview"
    } else {
        "initialized"
    };
    if receipt.mode != expected_mode
        || (args.preview && (receipt.config_created || receipt.control_created))
        || receipt.services_started
    {
        return Err(());
    }
    let config_argument = receipt.config_path.to_str().ok_or(())?.to_owned();
    Ok(CommandReport::initialization(InitializationOutput {
        mode: if args.preview {
            InitializationMode::Preview
        } else {
            InitializationMode::Initialized
        },
        config_path: receipt.config_path,
        state_dir: receipt.state_dir,
        data_dir: receipt.data_dir,
        config_created: receipt.config_created,
        control_created: receipt.control_created,
        services_started: false,
        next_config_args: ["--config".to_owned(), config_argument],
    }))
}

fn import_prerequisites(args: &InitArgs, config: &CommonConfigArgs) -> CommandReport {
    let Ok(installation) = initialization_request(args, config) else {
        return CommandReport::new("init", CommandOutcome::Rejected);
    };
    let request = InitializationImportRequest {
        installation,
        runner_templates: args.runner_template.clone(),
        provider_declarations: args.provider_declaration.clone(),
        agent_profiles: args.agent_profile.clone(),
        client_declarations: args.client_declaration.clone(),
        approval: args.approve_imports.clone(),
    };
    if args.preview {
        return match preview_imports(&request) {
            Ok(review) => CommandReport::initialization_import_review(review),
            Err(error) => import_failure(&error),
        };
    }
    match apply_imports(&request) {
        Ok(receipt) => CommandReport::initialization_import_apply(receipt),
        Err(error) => import_failure(&error),
    }
}

fn import_failure(error: &ImportError) -> CommandReport {
    match error {
        ImportError::InitializationRequired => {
            CommandReport::initialization_import_requires_initialization()
        }
        ImportError::ApprovalRequired => CommandReport::initialization_import_requires_approval(),
        ImportError::PublicationUncertain => CommandReport::initialization_import_incomplete(),
        ImportError::Invalid => CommandReport::new("init", CommandOutcome::Rejected),
    }
}

fn initialization_request(
    args: &InitArgs,
    config: &CommonConfigArgs,
) -> Result<InitializationRequest, ()> {
    let mut environment = Vec::new();
    for (key, value) in env::vars_os() {
        if key
            .to_str()
            .is_some_and(|key| key == "WORLDSTREAM_CONFIG" || key.starts_with("WORLDSTREAM__"))
        {
            environment.push((
                key.into_string().map_err(|_| ())?,
                value.into_string().map_err(|_| ())?,
            ));
        }
    }
    Ok(InitializationRequest {
        config: config.config.clone(),
        overrides: CliOverrides {
            bind: config.bind,
            storage_profile: config.storage_profile,
            data_dir: config.data_dir.clone(),
        },
        state_dir: args.options.state_dir.clone(),
        working_directory: env::current_dir().map_err(|_| ())?,
        environment,
        preview: args.preview,
    })
}
