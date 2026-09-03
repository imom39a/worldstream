//! Explicit retained setup requests over the verified local Controller transport.

use super::{
    cli_contract::{CommandOptions, CreateArgs, RoomCommand, SetupCommand},
    cli_reference::PublicReference,
    cli_room_setup::read_input,
};
use std::time::Duration;
use worldstream_studio_supervisor::{
    operator_connection::OperatorConnection,
    room_launch::RoomLaunchAssessmentV1,
    room_setup_operations::{
        RoomSetupCreateRequestV1, RoomSetupOperationListV1, RoomSetupOperationStatusV1,
    },
    room_setup_spec::{RoomSetupError, RoomSetupIssueCode, parse_setup_specification},
    rooms::{StudioRoomInventoryPageV1, StudioRoomSummaryV1},
};

#[derive(Debug)]
pub enum RoomOperationExecution {
    Complete(RoomSetupOperationStatusV1),
    Listed(RoomSetupOperationListV1),
    Rooms(StudioRoomInventoryPageV1),
    Room(Box<RoomInspection>),
    Partial {
        operation: String,
        status: Option<RoomSetupOperationStatusV1>,
    },
    Rejected(RoomSetupError),
    AcknowledgementRequired,
    Unavailable,
}

#[derive(Debug)]
pub struct RoomInspection {
    pub room: StudioRoomSummaryV1,
    pub assessment: Option<RoomLaunchAssessmentV1>,
    pub assessment_unavailable: bool,
}

pub fn execute(command: &RoomCommand) -> Option<RoomOperationExecution> {
    match command {
        RoomCommand::Create(arguments) => Some(create(arguments)),
        RoomCommand::List(options) => Some(read_rooms(options, None)),
        RoomCommand::Inspect(arguments) => {
            Some(read_rooms(&arguments.options, Some(&arguments.room)))
        }
        RoomCommand::Setup {
            command: SetupCommand::Status(arguments),
        } => Some(match &arguments.operation {
            Some(operation) => operation_request(&arguments.options, operation, false, &[]),
            None => list_operations(&arguments.options),
        }),
        RoomCommand::Setup {
            command: SetupCommand::Resume(arguments),
        } => Some(operation_request(
            &arguments.options,
            &arguments.operation,
            true,
            &[],
        )),
        _ => None,
    }
}

fn rejected(path: &str) -> RoomOperationExecution {
    RoomOperationExecution::Rejected(RoomSetupError::Specification {
        path: path.to_owned(),
        code: RoomSetupIssueCode::InvalidSpecification,
    })
}

fn create(arguments: &CreateArgs) -> RoomOperationExecution {
    let specification = match read_input(&arguments.source.file)
        .and_then(|bytes| parse_setup_specification(&bytes))
    {
        Ok(specification) => specification,
        Err(issue) => return RoomOperationExecution::Rejected(issue),
    };
    let Ok(body) = serde_json::to_vec(&RoomSetupCreateRequestV1 {
        specification,
        acknowledge_start: arguments.acknowledge_start,
    }) else {
        return rejected("");
    };
    let mut random = [0_u8; 16];
    if getrandom::fill(&mut random).is_err() {
        return RoomOperationExecution::Unavailable;
    }
    let mut operation = String::from("op-");
    for byte in random {
        use std::fmt::Write as _;
        if write!(&mut operation, "{byte:02x}").is_err() {
            return RoomOperationExecution::Unavailable;
        }
    }
    operation_request(&arguments.source.options, &operation, true, &body)
}

fn connection(options: &CommandOptions) -> Option<OperatorConnection> {
    OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(u64::from(options.timeout_seconds)),
    )
    .ok()
}

fn valid_operation(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= 64
        && operation
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn operation_request(
    options: &CommandOptions,
    operation: &str,
    mutate: bool,
    body: &[u8],
) -> RoomOperationExecution {
    if !valid_operation(operation) {
        return rejected("/operation");
    }
    let Some(connection) = connection(options) else {
        return RoomOperationExecution::Unavailable;
    };
    let suffix = if mutate && body.is_empty() {
        "/resume"
    } else {
        ""
    };
    let path = format!("/api/v1/room-setup-operations/{operation}{suffix}");
    let uncertain = || {
        if mutate {
            RoomOperationExecution::Partial {
                operation: operation.to_owned(),
                status: None,
            }
        } else {
            RoomOperationExecution::Unavailable
        }
    };
    let Ok(response) = connection.request(if mutate { "POST" } else { "GET" }, &path, body) else {
        return uncertain();
    };
    if response.status == 400 {
        return match serde_json::from_slice::<RoomSetupError>(&response.body) {
            Ok(issue) if bounded_issue(&issue) => RoomOperationExecution::Rejected(issue),
            _ => rejected("/operation"),
        };
    }
    if matches!(response.status, 404 | 409) {
        if response.status == 409
            && serde_json::from_slice::<serde_json::Value>(&response.body)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/error/code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .as_deref()
                == Some("room_setup_acknowledgement_required")
        {
            return RoomOperationExecution::AcknowledgementRequired;
        }
        return rejected("/operation");
    }
    if !matches!(response.status, 200..=202) {
        return uncertain();
    }
    let Ok(status) = serde_json::from_slice::<RoomSetupOperationStatusV1>(&response.body) else {
        return uncertain();
    };
    if status.version != "room_setup_operation.v1"
        || status.operation != operation
        || status
            .room_id
            .as_ref()
            .is_some_and(|room| PublicReference::parse(room).is_err())
    {
        return uncertain();
    }
    if status.complete {
        RoomOperationExecution::Complete(status)
    } else {
        RoomOperationExecution::Partial {
            operation: operation.to_owned(),
            status: Some(status),
        }
    }
}

fn bounded_issue(issue: &RoomSetupError) -> bool {
    let path = match issue {
        RoomSetupError::Specification { path, .. } => path,
        RoomSetupError::Configuration(issue) => &issue.path,
    };
    path.len() <= 512
        && !path.chars().any(char::is_control)
        && (path.is_empty() || path.starts_with('/'))
}

fn list_operations(options: &CommandOptions) -> RoomOperationExecution {
    let Some(connection) = connection(options) else {
        return RoomOperationExecution::Unavailable;
    };
    let Ok(response) = connection.request("GET", "/api/v1/room-setup-operations", &[]) else {
        return RoomOperationExecution::Unavailable;
    };
    if response.status != 200 {
        return RoomOperationExecution::Unavailable;
    }
    let Ok(list) = serde_json::from_slice::<RoomSetupOperationListV1>(&response.body) else {
        return RoomOperationExecution::Unavailable;
    };
    if list.version != "room_setup_operations.v1" {
        return RoomOperationExecution::Unavailable;
    }
    RoomOperationExecution::Listed(list)
}

fn read_rooms(options: &CommandOptions, room: Option<&String>) -> RoomOperationExecution {
    let path = match room {
        Some(room) if PublicReference::parse(room).is_ok() => format!("/api/v1/rooms/{room}"),
        Some(_) => return rejected("/room"),
        None => "/api/v1/rooms".to_owned(),
    };
    let Some(connection) = connection(options) else {
        return RoomOperationExecution::Unavailable;
    };
    let Ok(response) = connection.request("GET", &path, &[]) else {
        return RoomOperationExecution::Unavailable;
    };
    if response.status == 404 {
        return rejected("/room");
    }
    if response.status != 200 {
        return RoomOperationExecution::Unavailable;
    }
    if let Some(expected) = room {
        let Ok(detail) = serde_json::from_slice::<StudioRoomSummaryV1>(&response.body) else {
            return RoomOperationExecution::Unavailable;
        };
        if &detail.room_id != expected {
            return RoomOperationExecution::Unavailable;
        }
        let assessment = super::cli_room_launch::read_assessment(&connection, expected);
        RoomOperationExecution::Room(Box::new(RoomInspection {
            room: detail,
            assessment_unavailable: assessment.is_err(),
            assessment: assessment.ok().flatten(),
        }))
    } else {
        let Ok(page) = serde_json::from_slice::<StudioRoomInventoryPageV1>(&response.body) else {
            return RoomOperationExecution::Unavailable;
        };
        if page.schema != "worldstream/studio-room-inventory/v1" {
            return RoomOperationExecution::Unavailable;
        }
        RoomOperationExecution::Rooms(page)
    }
}
