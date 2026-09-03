//! Scoped participant transfer into explicit owner-protected local files.

use super::cli_contract::{ClientCommand, ExportArgs, OperatorCommand, RunnerCommand};
use serde::Serialize;
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};
use worldstream_runtime::{create_owner_only_file, validate_data_directory};
use worldstream_studio_supervisor::{
    operator_connection::OperatorConnection,
    scoped_connections::{MembershipCredentialsV1, RunnerCredentialsV1},
};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    Membership,
    Runner,
}

#[derive(Debug, Serialize)]
pub struct ExportSummary {
    pub kind: ExportKind,
    pub operation: String,
    pub seat: String,
    pub output_file: PathBuf,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportIssue {
    InvalidSelection,
    OutputExists,
    UnsafeDestination,
    CredentialUnavailable,
    WriteIncomplete,
}

#[derive(Debug)]
pub enum ParticipantExecution {
    Exported(ExportSummary),
    Rejected(ExportIssue),
    Failed(ExportIssue),
    Unavailable,
}

pub fn execute(command: &OperatorCommand) -> Option<ParticipantExecution> {
    match command {
        OperatorCommand::Client {
            command: ClientCommand::ExportCredentials(arguments),
        } => Some(export(arguments, ExportKind::Membership)),
        OperatorCommand::Runner {
            command: RunnerCommand::ExportCredentials(arguments),
        } => Some(export(arguments, ExportKind::Runner)),
        _ => None,
    }
}

fn destination(path: &Path) -> Result<PathBuf, ExportIssue> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = validate_data_directory(parent).map_err(|_| ExportIssue::UnsafeDestination)?;
    let filename = path.file_name().ok_or(ExportIssue::UnsafeDestination)?;
    let output = parent.join(filename);
    match fs::symlink_metadata(&output) {
        Ok(_) => Err(ExportIssue::OutputExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output),
        Err(_) => Err(ExportIssue::UnsafeDestination),
    }
}

fn export(arguments: &ExportArgs, kind: ExportKind) -> ParticipantExecution {
    let selection = &arguments.seat;
    if !valid_reference(&selection.operation) || !valid_reference(&selection.seat) {
        return ParticipantExecution::Rejected(ExportIssue::InvalidSelection);
    }
    let output = match destination(&arguments.output) {
        Ok(output) => output,
        Err(issue) => return ParticipantExecution::Rejected(issue),
    };
    let options = &selection.options;
    let Ok(connection) = OperatorConnection::open(
        &options.state_dir,
        options.controller,
        Duration::from_secs(u64::from(options.timeout_seconds)),
    ) else {
        return ParticipantExecution::Unavailable;
    };
    let suffix = match kind {
        ExportKind::Membership => "membership-credentials",
        ExportKind::Runner => "runner-credentials",
    };
    let endpoint = format!(
        "/api/v1/room-setup-operations/{}/seats/{}/{suffix}",
        selection.operation, selection.seat
    );
    let Ok(response) = connection.request("POST", &endpoint, &[]) else {
        return ParticipantExecution::Unavailable;
    };
    let secret_body = Zeroizing::new(response.body);
    if matches!(response.status, 400 | 404 | 409) {
        return ParticipantExecution::Rejected(ExportIssue::CredentialUnavailable);
    }
    if response.status != 200 {
        return ParticipantExecution::Unavailable;
    }
    let Some(bytes) = transfer_bytes(kind, &secret_body, &selection.operation, &selection.seat)
    else {
        return ParticipantExecution::Rejected(ExportIssue::CredentialUnavailable);
    };
    let Ok(mut file) = create_owner_only_file(&output) else {
        return ParticipantExecution::Rejected(ExportIssue::UnsafeDestination);
    };
    if file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .is_err()
    {
        // This file was exclusively created by this invocation. If removal fails,
        // it remains owner-protected and the receipt never claims success.
        drop(file);
        let _ = fs::remove_file(&output);
        return ParticipantExecution::Failed(ExportIssue::WriteIncomplete);
    }
    ParticipantExecution::Exported(ExportSummary {
        kind,
        operation: selection.operation.clone(),
        seat: selection.seat.clone(),
        output_file: output,
    })
}

fn valid_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn transfer_bytes(
    kind: ExportKind,
    bytes: &[u8],
    operation: &str,
    seat: &str,
) -> Option<Zeroizing<Vec<u8>>> {
    let encoded = match kind {
        ExportKind::Membership => {
            let document: MembershipCredentialsV1 = serde_json::from_slice(bytes).ok()?;
            if document.schema != "worldstream/membership-credentials/v1"
                || document.operation != operation
                || document.seat != seat
            {
                return None;
            }
            serde_json::to_vec_pretty(&document).ok()?
        }
        ExportKind::Runner => {
            let document: RunnerCredentialsV1 = serde_json::from_slice(bytes).ok()?;
            if document.schema != "worldstream/runner-credentials/v1"
                || document.operation != operation
                || document.seat != seat
            {
                return None;
            }
            serde_json::to_vec_pretty(&document).ok()?
        }
    };
    Some(Zeroizing::new(encoded))
}
