//! Stable, secret-free output for the additive operator commands.
//! Legacy command receipts deliberately do not pass through this formatter.

use serde::Serialize;
use std::io::{self, Write};

/// Process statuses shared by new operator commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandExit {
    Complete,
    Failed,
    InvalidArguments,
    Unavailable,
    Partial,
}

impl CommandExit {
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Complete => 0,
            Self::Failed => 1,
            Self::InvalidArguments => 2,
            Self::Unavailable => 3,
            Self::Partial => 4,
        }
    }
}

/// Accepted command results are distinct from canonical Room Outcomes.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "IMO-135 freezes results before their backend commands are implemented"
    )
)]
pub enum CommandOutcome {
    Complete,
    Rejected,
    Failed,
    InvalidArguments,
    NotImplemented,
    ControllerUnavailable,
    StaleEvidence,
    PartialSetup {
        operation_id: PublicReference,
        room_id: Option<PublicReference>,
        stage: SetupStage,
    },
}

/// A public operational identifier, never a secret, path, or request payload.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct PublicReference(String);

impl PublicReference {
    /// Validate the same bounded reference grammar accepted by the operator CLI.
    ///
    /// Callers supply only references from typed operational results, not raw errors.
    ///
    /// # Errors
    /// Returns a fixed diagnostic without retaining or displaying rejected input.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "partial backend results consume this frozen output boundary in subsequent tickets"
        )
    )]
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if value.len() > 128
            || !value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err("expected a bounded public identifier");
        }
        Ok(Self(value.to_owned()))
    }
}

/// Operational setup progress; not an Activity Phase or canonical Room value.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "IMO-135 freezes setup-stage output before durable setup integration"
    )
)]
pub enum SetupStage {
    RoomCreation,
    MemberCapability,
    RunnerCapability,
}

impl SetupStage {
    const fn label(self) -> &'static str {
        match self {
            Self::RoomCreation => "room_creation",
            Self::MemberCapability => "member_capability",
            Self::RunnerCapability => "runner_capability",
        }
    }
}

#[derive(Debug, Serialize)]
struct SetupProgress {
    operation_id: PublicReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    room_id: Option<PublicReference>,
    stage: SetupStage,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CommandStatus {
    Complete,
    Rejected,
    Failed,
    InvalidArguments,
    Unavailable,
    Partial,
}

impl CommandStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
            Self::InvalidArguments => "invalid_arguments",
            Self::Unavailable => "unavailable",
            Self::Partial => "partial",
        }
    }

    const fn exit(self) -> CommandExit {
        match self {
            Self::Complete => CommandExit::Complete,
            Self::Rejected | Self::Failed => CommandExit::Failed,
            Self::InvalidArguments => CommandExit::InvalidArguments,
            Self::Unavailable => CommandExit::Unavailable,
            Self::Partial => CommandExit::Partial,
        }
    }
}

/// A closed result, never a raw request, configuration, or backend error.
#[derive(Debug, Serialize)]
pub struct CommandReport {
    schema: &'static str,
    command: &'static str,
    status: CommandStatus,
    code: &'static str,
    message: &'static str,
    next_action: String,
    #[serde(flatten)]
    setup: Option<SetupProgress>,
}

impl CommandReport {
    #[must_use]
    pub fn invalid_arguments(command: &'static str) -> Self {
        Self::new(command, CommandOutcome::InvalidArguments)
    }

    #[must_use]
    pub fn not_implemented(command: &'static str) -> Self {
        Self::new(command, CommandOutcome::NotImplemented)
    }

    #[must_use]
    pub fn new(command: &'static str, outcome: CommandOutcome) -> Self {
        let mut setup = None;
        let (status, code, message, next_action) = match outcome {
            CommandOutcome::Complete => (
                CommandStatus::Complete,
                "complete",
                "The operator command completed.",
                "No further action is required for this command.",
            ),
            CommandOutcome::Rejected => (
                CommandStatus::Rejected,
                "operation_rejected",
                "The operator command was rejected.",
                "Review the command requirements and current operator state before retrying.",
            ),
            CommandOutcome::Failed => (
                CommandStatus::Failed,
                "operation_failed",
                "The operator command failed.",
                "Inspect current operator state before retrying; do not assume retained work was rolled back.",
            ),
            CommandOutcome::InvalidArguments => (
                CommandStatus::InvalidArguments,
                "invalid_arguments",
                "Invalid operator arguments.",
                "Check the command help and provide complete bounded input.",
            ),
            CommandOutcome::NotImplemented => (
                CommandStatus::Unavailable,
                "not_implemented",
                "This operator command is not implemented yet.",
                "Use the documented existing operator interface until this backend is available.",
            ),
            CommandOutcome::StaleEvidence => (
                CommandStatus::Unavailable,
                "stale_evidence",
                "Current state could not be verified; retained evidence is stale.",
                "Check controller availability, then repeat this read-only command.",
            ),
            CommandOutcome::ControllerUnavailable => (
                CommandStatus::Unavailable,
                "controller_unavailable",
                "The local controller is unavailable.",
                "Check the initialized control state and controller availability.",
            ),
            CommandOutcome::PartialSetup {
                operation_id,
                room_id,
                stage,
            } => {
                setup = Some(SetupProgress {
                    operation_id,
                    room_id,
                    stage,
                });
                (
                    CommandStatus::Partial,
                    "setup_incomplete",
                    "Room setup is incomplete; retained work has not been rolled back.",
                    "",
                )
            }
        };
        let next_action = setup.as_ref().map_or_else(
            || next_action.to_owned(),
            |progress| format!(
                "Inspect 'worldstreamctl room setup status {operation}', then resume that intent with 'worldstreamctl room setup resume {operation}' using the same installation options.",
                operation = progress.operation_id.0,
            ),
        );
        Self {
            schema: "worldstream/operator-command/v1",
            command,
            status,
            code,
            message,
            next_action,
            setup,
        }
    }

    /// Write the result to stdout and bounded diagnostics to stderr.
    ///
    /// # Errors
    /// Returns an I/O error if either output cannot be written; never returns a
    /// successful command exit after output failure.
    pub fn write(
        &self,
        json: bool,
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) -> io::Result<CommandExit> {
        if json {
            let mut document = serde_json::to_vec(self)
                .map_err(|_| io::Error::other("operator result could not be encoded"))?;
            document.push(b'\n');
            stdout.write_all(&document)?;
        } else {
            writeln!(
                stdout,
                "{}: {} ({})",
                self.command,
                self.status.label(),
                self.code
            )?;
            if let Some(progress) = &self.setup {
                writeln!(stdout, "Operation: {}", progress.operation_id.0)?;
                if let Some(room_id) = &progress.room_id {
                    writeln!(stdout, "Room: {}", room_id.0)?;
                }
                writeln!(stdout, "Stage: {}", progress.stage.label())?;
            }
        }
        if !matches!(self.status, CommandStatus::Complete) {
            writeln!(stderr, "{} {}", self.message, self.next_action)?;
        }
        Ok(self.status.exit())
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandOutcome, CommandReport, PublicReference, SetupStage};

    #[test]
    fn unavailable_json_is_one_literal_document_with_separate_diagnostics() -> std::io::Result<()> {
        let report = CommandReport::not_implemented("server status");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = report.write(true, &mut stdout, &mut stderr)?;

        assert_eq!(exit.code(), 3);
        assert_eq!(
            stdout,
            br#"{"schema":"worldstream/operator-command/v1","command":"server status","status":"unavailable","code":"not_implemented","message":"This operator command is not implemented yet.","next_action":"Use the documented existing operator interface until this backend is available."}
"#
        );
        assert_eq!(
            stderr,
            b"This operator command is not implemented yet. Use the documented existing operator interface until this backend is available.\n"
        );

        stdout.clear();
        stderr.clear();
        assert_eq!(report.write(false, &mut stdout, &mut stderr)?.code(), 3);
        assert_eq!(stdout, b"server status: unavailable (not_implemented)\n");
        assert_eq!(
            stderr,
            b"This operator command is not implemented yet. Use the documented existing operator interface until this backend is available.\n"
        );
        Ok(())
    }

    #[test]
    fn invalid_arguments_have_a_readable_summary_and_closed_diagnostic() -> std::io::Result<()> {
        let report = CommandReport::invalid_arguments("room setup resume");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = report.write(false, &mut stdout, &mut stderr)?;

        assert_eq!(exit.code(), 2);
        assert_eq!(
            stdout,
            b"room setup resume: invalid_arguments (invalid_arguments)\n"
        );
        assert_eq!(
            stderr,
            b"Invalid operator arguments. Check the command help and provide complete bounded input.\n"
        );
        Ok(())
    }

    #[test]
    fn stale_evidence_is_unavailable_in_both_output_formats() -> std::io::Result<()> {
        let report = CommandReport::new("pack list", CommandOutcome::StaleEvidence);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = report.write(true, &mut stdout, &mut stderr)?;

        assert_eq!(exit.code(), 3);
        assert_eq!(
            stdout,
            br#"{"schema":"worldstream/operator-command/v1","command":"pack list","status":"unavailable","code":"stale_evidence","message":"Current state could not be verified; retained evidence is stale.","next_action":"Check controller availability, then repeat this read-only command."}
"#
        );
        assert_eq!(
            stderr,
            b"Current state could not be verified; retained evidence is stale. Check controller availability, then repeat this read-only command.\n"
        );

        stdout.clear();
        stderr.clear();
        assert_eq!(report.write(false, &mut stdout, &mut stderr)?.code(), 3);
        assert_eq!(stdout, b"pack list: unavailable (stale_evidence)\n");
        assert_eq!(
            stderr,
            b"Current state could not be verified; retained evidence is stale. Check controller availability, then repeat this read-only command.\n"
        );
        Ok(())
    }

    #[test]
    fn terminal_outcomes_preserve_literal_json_and_exit_contracts() -> std::io::Result<()> {
        for (outcome, expected_exit, expected_stdout, expected_stderr) in [
            (
                CommandOutcome::Complete,
                0,
                concat!(
                    r#"{"schema":"worldstream/operator-command/v1","command":"server status","status":"complete","code":"complete","message":"The operator command completed.","next_action":"No further action is required for this command."}"#,
                    "\n"
                ),
                "",
            ),
            (
                CommandOutcome::Rejected,
                1,
                concat!(
                    r#"{"schema":"worldstream/operator-command/v1","command":"server status","status":"rejected","code":"operation_rejected","message":"The operator command was rejected.","next_action":"Review the command requirements and current operator state before retrying."}"#,
                    "\n"
                ),
                "The operator command was rejected. Review the command requirements and current operator state before retrying.\n",
            ),
            (
                CommandOutcome::Failed,
                1,
                concat!(
                    r#"{"schema":"worldstream/operator-command/v1","command":"server status","status":"failed","code":"operation_failed","message":"The operator command failed.","next_action":"Inspect current operator state before retrying; do not assume retained work was rolled back."}"#,
                    "\n"
                ),
                "The operator command failed. Inspect current operator state before retrying; do not assume retained work was rolled back.\n",
            ),
            (
                CommandOutcome::ControllerUnavailable,
                3,
                concat!(
                    r#"{"schema":"worldstream/operator-command/v1","command":"server status","status":"unavailable","code":"controller_unavailable","message":"The local controller is unavailable.","next_action":"Check the initialized control state and controller availability."}"#,
                    "\n"
                ),
                "The local controller is unavailable. Check the initialized control state and controller availability.\n",
            ),
            (
                CommandOutcome::InvalidArguments,
                2,
                concat!(
                    r#"{"schema":"worldstream/operator-command/v1","command":"server status","status":"invalid_arguments","code":"invalid_arguments","message":"Invalid operator arguments.","next_action":"Check the command help and provide complete bounded input."}"#,
                    "\n"
                ),
                "Invalid operator arguments. Check the command help and provide complete bounded input.\n",
            ),
        ] {
            let report = CommandReport::new("server status", outcome);
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            assert_eq!(
                report.write(true, &mut stdout, &mut stderr)?.code(),
                expected_exit
            );
            assert_eq!(stdout, expected_stdout.as_bytes());
            assert_eq!(stderr, expected_stderr.as_bytes());
            assert!(serde_json::from_slice::<serde_json::Value>(&stdout).is_ok());
        }
        Ok(())
    }

    #[test]
    fn partial_setup_reports_created_room_and_bounded_same_intent_resume() -> std::io::Result<()> {
        let report = CommandReport::new(
            "room create",
            CommandOutcome::PartialSetup {
                operation_id: PublicReference::parse(
                    "room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3",
                )
                .map_err(std::io::Error::other)?,
                room_id: Some(
                    PublicReference::parse("01ARZ3NDEKTSV4RRFFQ69G5FAW")
                        .map_err(std::io::Error::other)?,
                ),
                stage: SetupStage::MemberCapability,
            },
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        assert_eq!(report.write(true, &mut stdout, &mut stderr)?.code(), 4);
        assert_eq!(
            stdout,
            br#"{"schema":"worldstream/operator-command/v1","command":"room create","status":"partial","code":"setup_incomplete","message":"Room setup is incomplete; retained work has not been rolled back.","next_action":"Inspect 'worldstreamctl room setup status room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3', then resume that intent with 'worldstreamctl room setup resume room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3' using the same installation options.","operation_id":"room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3","room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAW","stage":"member_capability"}
"#
        );
        assert_eq!(
            stderr,
            b"Room setup is incomplete; retained work has not been rolled back. Inspect 'worldstreamctl room setup status room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3', then resume that intent with 'worldstreamctl room setup resume room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3' using the same installation options.\n"
        );

        stdout.clear();
        stderr.clear();
        assert_eq!(report.write(false, &mut stdout, &mut stderr)?.code(), 4);
        assert_eq!(
            stdout,
            b"room create: partial (setup_incomplete)\nOperation: room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3\nRoom: 01ARZ3NDEKTSV4RRFFQ69G5FAW\nStage: member_capability\n"
        );
        assert_eq!(
            stderr,
            b"Room setup is incomplete; retained work has not been rolled back. Inspect 'worldstreamctl room setup status room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3', then resume that intent with 'worldstreamctl room setup resume room-create-operation-01ARZ3NDEKTSV4RRFFQ69G5FB3' using the same installation options.\n"
        );
        Ok(())
    }

    #[test]
    fn partial_setup_does_not_invent_a_room_when_creation_is_unconfirmed() -> std::io::Result<()> {
        for (stage, expected_stage) in [
            (SetupStage::RoomCreation, "room_creation"),
            (SetupStage::RunnerCapability, "runner_capability"),
        ] {
            let report = CommandReport::new(
                "room setup resume",
                CommandOutcome::PartialSetup {
                    operation_id: PublicReference::parse("operation.1_local:retry")
                        .map_err(std::io::Error::other)?,
                    room_id: None,
                    stage,
                },
            );
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();

            assert_eq!(report.write(true, &mut stdout, &mut stderr)?.code(), 4);
            let document: serde_json::Value =
                serde_json::from_slice(&stdout).map_err(std::io::Error::other)?;
            assert!(document.get("room_id").is_none());
            assert_eq!(document["operation_id"], "operation.1_local:retry");
            assert_eq!(document["stage"], expected_stage);
        }
        Ok(())
    }
}
