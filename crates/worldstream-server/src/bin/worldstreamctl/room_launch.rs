//! Explicit Room launch and read-only launch assessment over verified control.

use super::{cli_contract::RoomArgs, cli_reference::PublicReference};
use std::time::Duration;
use worldstream_studio_supervisor::{
    operator_connection::OperatorConnection, room_launch::RoomLaunchAssessmentV1,
    task_setup::TaskLaunchStateV1,
};

#[derive(Debug)]
pub enum RoomLaunchExecution {
    Complete(RoomLaunchAssessmentV1),
    Partial {
        room: String,
        assessment: Option<RoomLaunchAssessmentV1>,
    },
    Rejected(Option<RoomLaunchAssessmentV1>),
    Unavailable,
}

pub fn execute(arguments: &RoomArgs) -> RoomLaunchExecution {
    if PublicReference::parse(&arguments.room).is_err() {
        return RoomLaunchExecution::Rejected(None);
    }
    let options = &arguments.options;
    let Ok(connection) = OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(u64::from(options.timeout_seconds)),
    ) else {
        return RoomLaunchExecution::Unavailable;
    };
    let partial = |assessment| RoomLaunchExecution::Partial {
        room: arguments.room.clone(),
        assessment,
    };
    let path = format!("/api/v1/rooms/{}/launch", arguments.room);
    let Ok(response) = connection.request("POST", &path, &[]) else {
        return partial(None);
    };
    if matches!(response.status, 400 | 404) {
        return RoomLaunchExecution::Rejected(None);
    }
    let Some(assessment) = decode_assessment(&response.body, &arguments.room) else {
        return partial(None);
    };
    match response.status {
        200 if assessment
            .launch
            .as_ref()
            .is_some_and(|launch| launch.state == TaskLaunchStateV1::Launched) =>
        {
            RoomLaunchExecution::Complete(assessment)
        }
        409 => RoomLaunchExecution::Rejected(Some(assessment)),
        _ => partial(Some(assessment)),
    }
}

pub fn read_assessment(
    connection: &OperatorConnection,
    room: &str,
) -> Result<Option<RoomLaunchAssessmentV1>, ()> {
    let response = connection
        .request("GET", &format!("/api/v1/rooms/{room}/launch"), &[])
        .map_err(|_| ())?;
    if response.status == 404 {
        return Ok(None); // A Room created outside this installation has no retained setup here.
    }
    if response.status != 200 {
        return Err(());
    }
    decode_assessment(&response.body, room).map(Some).ok_or(())
}

fn decode_assessment(bytes: &[u8], room: &str) -> Option<RoomLaunchAssessmentV1> {
    let assessment: RoomLaunchAssessmentV1 = serde_json::from_slice(bytes).ok()?;
    (assessment.version == "room_launch_assessment.v1"
        && assessment.room_id == room
        && PublicReference::parse(&assessment.operation).is_ok())
    .then_some(assessment)
}
