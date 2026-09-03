//! Thin, explicit local file and transport adapters for Room setup commands.

use super::cli_contract::{CommandOptions, ExampleArgs, FileArgs, RoomCommand};
use serde::Serialize;
use std::{
    fs,
    io::{Read as _, Write as _},
    path::PathBuf,
    time::{Duration, Instant},
};
use worldstream_core::Blake3DigestV1;
use worldstream_protocol::{
    ActivityPackCatalogResponse, ActivityPackCatalogRevisionResponse, PackReference,
};
use worldstream_studio_supervisor::{
    agent_profiles::{
        AgentHostContractV1, AgentProfileRevisionViewV1, AgentProfileSecretAvailabilityV1,
    },
    operator_connection::OperatorConnection,
    room_drafts::AgentAssignmentModeV1,
    room_setup_spec::{
        RoomSetupError, RoomSetupIssueCode, RoomSetupSpecificationV1, SetupAssignmentV1,
        generate_setup_example, parse_setup_specification, resolve_setup_specification,
    },
    runner_templates::RunnerTemplateCatalogV1,
    verified_control::ControlResponse,
};

#[derive(Debug, Serialize)]
pub struct RoomSetupSummary {
    pub pack: PackReference,
    pub seat_count: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file: Option<PathBuf>,
}

#[derive(Debug)]
pub enum RoomSetupExecution {
    Complete(RoomSetupSummary),
    Rejected(RoomSetupError),
    Failed(RoomSetupError),
    Unavailable,
}

pub fn execute(command: &RoomCommand) -> Option<RoomSetupExecution> {
    match command {
        RoomCommand::Validate(arguments) => Some(validate(arguments)),
        RoomCommand::Example(arguments) => Some(example(arguments)),
        _ => None,
    }
}

fn example(arguments: &ExampleArgs) -> RoomSetupExecution {
    if arguments.interactive {
        return RoomSetupExecution::Rejected(RoomSetupError::Specification {
            path: "/interactive".to_owned(),
            code: RoomSetupIssueCode::ExampleChoicesRequired,
        });
    }
    let started = Instant::now();
    let rejected_pack = || {
        RoomSetupExecution::Rejected(RoomSetupError::Specification {
            path: "/pack".to_owned(),
            code: RoomSetupIssueCode::ExactPackMismatch,
        })
    };
    let Some((name, version)) = arguments.pack.split_once('@') else {
        return rejected_pack();
    };
    let Some(response) = metadata(&arguments.options, started, "/api/v1/activity-packs") else {
        return RoomSetupExecution::Unavailable;
    };
    if response.status != 200 {
        return RoomSetupExecution::Unavailable;
    }
    let Ok(catalog) = serde_json::from_slice::<ActivityPackCatalogResponse>(&response.body) else {
        return RoomSetupExecution::Unavailable;
    };
    if catalog.version != "activity_pack_catalog.v1" {
        return RoomSetupExecution::Unavailable;
    }
    let mut matches = catalog.revisions.into_iter().filter(|entry| {
        entry.pack.id == name && entry.pack.version == version && entry.selectable_for_new_rooms
    });
    let Some(selected) = matches.next() else {
        return rejected_pack();
    };
    if matches.next().is_some() {
        return rejected_pack();
    }
    let Ok(digest) = selected.pack.digest.parse::<Blake3DigestV1>() else {
        return RoomSetupExecution::Unavailable;
    };
    let Some(response) = metadata(
        &arguments.options,
        started,
        &format!("/api/v1/activity-packs/{digest}"),
    ) else {
        return RoomSetupExecution::Unavailable;
    };
    if response.status != 200 {
        return RoomSetupExecution::Unavailable;
    }
    let Ok(detail) = serde_json::from_slice::<ActivityPackCatalogRevisionResponse>(&response.body)
    else {
        return RoomSetupExecution::Unavailable;
    };
    if detail.revision.summary.pack != selected.pack {
        return rejected_pack();
    }
    let specification = match generate_setup_example(&detail) {
        Ok(specification) => specification,
        Err(error) => return RoomSetupExecution::Rejected(error),
    };
    if let Err(result) = check_profiles(&specification, &arguments.options, started)
        .and_then(|()| check_runners(&specification, &arguments.options, started))
    {
        return result;
    }
    write_example(&arguments.output, &specification)
}

fn write_example(
    path: &std::path::Path,
    specification: &RoomSetupSpecificationV1,
) -> RoomSetupExecution {
    let issue = |code| RoomSetupError::Specification {
        path: "/output".to_owned(),
        code,
    };
    let Ok(mut bytes) = serde_json::to_vec_pretty(specification) else {
        return RoomSetupExecution::Failed(issue(RoomSetupIssueCode::OutputUnavailable));
    };
    bytes.push(b'\n');
    let Ok(seat_count) = u16::try_from(specification.seats.len()) else {
        return RoomSetupExecution::Rejected(issue(RoomSetupIssueCode::InvalidSpecification));
    };
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => {
            return RoomSetupExecution::Rejected(issue(
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    RoomSetupIssueCode::OutputExists
                } else {
                    RoomSetupIssueCode::OutputUnavailable
                },
            ));
        }
    };
    if file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .is_err()
    {
        return RoomSetupExecution::Failed(issue(RoomSetupIssueCode::OutputUnavailable));
    }
    RoomSetupExecution::Complete(RoomSetupSummary {
        pack: specification.pack.clone(),
        seat_count,
        output_file: Some(path.to_path_buf()),
    })
}

fn validate(arguments: &FileArgs) -> RoomSetupExecution {
    let started = Instant::now();
    let bytes = match read_input(&arguments.file) {
        Ok(bytes) => bytes,
        Err(error) => return RoomSetupExecution::Rejected(error),
    };
    let specification = match parse_setup_specification(&bytes) {
        Ok(specification) => specification,
        Err(error) => return RoomSetupExecution::Rejected(error),
    };
    let Ok(digest) = specification.pack.digest.parse::<Blake3DigestV1>() else {
        return RoomSetupExecution::Rejected(RoomSetupError::Specification {
            path: "/pack/digest".to_owned(),
            code: RoomSetupIssueCode::InvalidSpecification,
        });
    };
    let path = format!("/api/v1/activity-packs/{digest}");
    let Some(response) = metadata(&arguments.options, started, &path) else {
        return RoomSetupExecution::Unavailable;
    };
    if response.status != 200 {
        return RoomSetupExecution::Unavailable;
    }
    let Ok(catalog) = serde_json::from_slice::<ActivityPackCatalogRevisionResponse>(&response.body)
    else {
        return RoomSetupExecution::Unavailable;
    };
    match resolve_setup_specification(&bytes, &catalog) {
        Ok(specification) => {
            if let Err(result) = check_profiles(&specification, &arguments.options, started) {
                return result;
            }
            if let Err(result) = check_runners(&specification, &arguments.options, started) {
                return result;
            }
            let Ok(seat_count) = u16::try_from(specification.seats.len()) else {
                return RoomSetupExecution::Rejected(RoomSetupError::Specification {
                    path: "/seats".to_owned(),
                    code: RoomSetupIssueCode::SeatIntent,
                });
            };
            RoomSetupExecution::Complete(RoomSetupSummary {
                pack: specification.pack,
                seat_count,
                output_file: None,
            })
        }
        Err(error) => RoomSetupExecution::Rejected(error),
    }
}

fn metadata(options: &CommandOptions, started: Instant, path: &str) -> Option<ControlResponse> {
    let remaining =
        Duration::from_secs(u64::from(options.timeout_seconds)).checked_sub(started.elapsed())?;
    let connection =
        OperatorConnection::open(&options.state_dir, options.controller, remaining).ok()?;
    connection.request("GET", path, b"").ok()
}

fn check_profiles(
    specification: &RoomSetupSpecificationV1,
    options: &CommandOptions,
    started: Instant,
) -> Result<(), RoomSetupExecution> {
    for (index, seat) in specification.seats.iter().enumerate() {
        let Some(assignment) = &seat.assignment else {
            continue;
        };
        let Some(profile) = &assignment.agent_profile else {
            continue;
        };
        let issue_path = format!("/seats/{index}/assignment/agent_profile");
        if !profile_component(&profile.profile_id, false)
            || !profile_component(&profile.revision, true)
        {
            return Err(RoomSetupExecution::Rejected(
                RoomSetupError::Specification {
                    path: issue_path,
                    code: RoomSetupIssueCode::InvalidSpecification,
                },
            ));
        }
        let path = format!(
            "/api/v1/agent-profiles/{}/revisions/{}",
            profile.profile_id, profile.revision
        );
        let response = metadata(options, started, &path).ok_or(RoomSetupExecution::Unavailable)?;
        if response.status == 404 {
            return Err(RoomSetupExecution::Rejected(
                RoomSetupError::Specification {
                    path: issue_path,
                    code: RoomSetupIssueCode::DependencyMissing,
                },
            ));
        }
        if response.status != 200 {
            return Err(RoomSetupExecution::Unavailable);
        }
        let view: AgentProfileRevisionViewV1 =
            serde_json::from_slice(&response.body).map_err(|_| RoomSetupExecution::Unavailable)?;
        check_profile_view(assignment, &view, &issue_path)?;
    }
    Ok(())
}

fn check_profile_view(
    assignment: &SetupAssignmentV1,
    view: &AgentProfileRevisionViewV1,
    issue_path: &str,
) -> Result<(), RoomSetupExecution> {
    let rejected = |code| {
        RoomSetupExecution::Rejected(RoomSetupError::Specification {
            path: issue_path.to_owned(),
            code,
        })
    };
    let Some(expected) = &assignment.agent_profile else {
        return Err(rejected(RoomSetupIssueCode::Required));
    };
    if expected.profile_id != view.profile_id || expected.revision != view.revision {
        return Err(rejected(RoomSetupIssueCode::DependencyIncompatible));
    }
    if view
        .secret_settings
        .iter()
        .any(|setting| setting.availability == AgentProfileSecretAvailabilityV1::Unavailable)
    {
        return Err(RoomSetupExecution::Unavailable);
    }
    if view
        .secret_settings
        .iter()
        .any(|setting| setting.availability != AgentProfileSecretAvailabilityV1::Configured)
    {
        return Err(rejected(RoomSetupIssueCode::DependencyMissing));
    }
    match (&assignment.mode, &view.host_contract) {
        (_, AgentHostContractV1::GenericMcp) => Ok(()),
        (
            AgentAssignmentModeV1::Managed,
            AgentHostContractV1::ManagedReference {
                runner_template, ..
            },
        ) if assignment.runner_template.as_ref() == Some(runner_template) => Ok(()),
        _ => Err(rejected(RoomSetupIssueCode::DependencyIncompatible)),
    }
}

fn check_runners(
    specification: &RoomSetupSpecificationV1,
    options: &CommandOptions,
    started: Instant,
) -> Result<(), RoomSetupExecution> {
    let selected: Vec<_> = specification
        .seats
        .iter()
        .enumerate()
        .filter_map(|(index, seat)| {
            seat.assignment
                .as_ref()
                .and_then(|assignment| assignment.runner_template.as_ref())
                .map(|reference| (index, reference))
        })
        .collect();
    if selected.is_empty() {
        return Ok(());
    }
    for (index, reference) in &selected {
        if !profile_component(&reference.template_id, false)
            || !profile_component(&reference.revision, true)
        {
            return Err(RoomSetupExecution::Rejected(
                RoomSetupError::Specification {
                    path: format!("/seats/{index}/assignment/runner_template"),
                    code: RoomSetupIssueCode::InvalidSpecification,
                },
            ));
        }
    }
    let response = metadata(options, started, "/api/v1/runner-templates")
        .ok_or(RoomSetupExecution::Unavailable)?;
    if response.status != 200 {
        return Err(RoomSetupExecution::Unavailable);
    }
    let catalog: RunnerTemplateCatalogV1 =
        serde_json::from_slice(&response.body).map_err(|_| RoomSetupExecution::Unavailable)?;
    if catalog.schema != "worldstream/studio-runner-template-catalog/v1" {
        return Err(RoomSetupExecution::Unavailable);
    }
    for (index, reference) in selected {
        let mut matches = catalog.templates.iter().filter(|template| {
            template.template_id == reference.template_id && template.revision == reference.revision
        });
        let Some(template) = matches.next() else {
            return Err(RoomSetupExecution::Rejected(
                RoomSetupError::Specification {
                    path: format!("/seats/{index}/assignment/runner_template"),
                    code: RoomSetupIssueCode::DependencyMissing,
                },
            ));
        };
        if matches.next().is_some() {
            return Err(RoomSetupExecution::Unavailable);
        }
        if !template.compatibility.iter().any(|rule| {
            rule.activity_pack_id == specification.pack.id
                && rule.exact_revisions.contains(&specification.pack.version)
        }) {
            return Err(RoomSetupExecution::Rejected(
                RoomSetupError::Specification {
                    path: format!("/seats/{index}/assignment/runner_template"),
                    code: RoomSetupIssueCode::DependencyIncompatible,
                },
            ));
        }
    }
    Ok(())
}

fn profile_component(value: &str, revision: bool) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_')
                || (revision && (byte.is_ascii_uppercase() || byte == b'.'))
        })
}

fn read_input(path: &std::path::Path) -> Result<Vec<u8>, RoomSetupError> {
    let invalid = || RoomSetupError::Specification {
        path: String::new(),
        code: RoomSetupIssueCode::InvalidSpecification,
    };
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() > 1_048_576 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| invalid())?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    Ok(bytes)
}
