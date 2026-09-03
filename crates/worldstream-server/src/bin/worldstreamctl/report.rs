//! Stable, secret-free output for the additive operator commands.
//! Legacy command receipts deliberately do not pass through this formatter.

use crate::cli_reference::PublicReference;
use serde::Serialize;
use std::io::{self, Write};
#[cfg(feature = "cli-operator-preview")]
use worldstream_studio_supervisor::initialization_imports::{ImportApplyV1, ImportReviewV1};

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
    PartialLifecycle {
        stage: LifecycleStage,
    },
}

/// Incomplete managed-process work, separate from Room setup progress.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "IMO-135 freezes managed lifecycle partial results before process integration"
    )
)]
pub enum LifecycleStage {
    ManagedRunnerStop,
    RuntimeStop,
    RuntimeRestart,
    ManagedRunnerRestore,
}

impl LifecycleStage {
    const fn label(self) -> &'static str {
        match self {
            Self::ManagedRunnerStop => "managed_runner_stop",
            Self::RuntimeStop => "runtime_stop",
            Self::RuntimeRestart => "runtime_restart",
            Self::ManagedRunnerRestore => "managed_runner_restore",
        }
    }
}

#[derive(Debug, Serialize)]
struct LifecycleProgress {
    stage: LifecycleStage,
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

/// Explicit non-secret local initialization output, not a configuration dump.
#[cfg(feature = "cli-operator-preview")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationMode {
    Preview,
    Initialized,
}

/// Explicit non-secret local initialization output, not a configuration dump.
#[cfg(feature = "cli-operator-preview")]
#[derive(Debug, Serialize)]
pub struct InitializationOutput {
    pub mode: InitializationMode,
    pub config_path: std::path::PathBuf,
    pub state_dir: std::path::PathBuf,
    pub data_dir: std::path::PathBuf,
    pub config_created: bool,
    pub control_created: bool,
    pub services_started: bool,
    pub next_config_args: [String; 2],
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
    #[serde(flatten)]
    lifecycle: Option<LifecycleProgress>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    initialization: Option<InitializationOutput>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    import_review: Option<ImportReviewV1>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    import_apply: Option<ImportApplyV1>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<worldstream_studio_supervisor::managed_lifecycle::LifecycleStatus>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    retained_server:
        Option<worldstream_studio_supervisor::retained_server_inspection::RetainedServerInspection>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    logs: Option<Vec<worldstream_studio_supervisor::managed_lifecycle::LifecycleLogEntry>>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    room_setup: Option<crate::cli_room_setup::RoomSetupSummary>,
    #[cfg(feature = "cli-operator-preview")]
    #[serde(skip_serializing_if = "Option::is_none")]
    setup_issue: Option<worldstream_studio_supervisor::room_setup_spec::RoomSetupError>,
}

impl CommandReport {
    #[cfg(feature = "cli-operator-preview")]
    pub fn bound_runner_restart_unsupported() -> Self {
        let mut report = Self::new("server restart", CommandOutcome::Rejected);
        report.code = "managed_runner_restart_unsupported";
        report.message = "Automatic restart of bound Runner instances is not supported in this preview. No processes were stopped.";
        "Use server stop, then server start, and explicitly start the required agents."
            .clone_into(&mut report.next_action);
        report
    }

    #[cfg(feature = "cli-operator-preview")]
    pub fn unavailable_server(command: &'static str, state: &std::path::Path) -> Self {
        let mut report = Self::new(command, CommandOutcome::ControllerUnavailable);
        report.retained_server = Some(
            worldstream_studio_supervisor::retained_server_inspection::inspect_retained_server(
                state,
            ),
        );
        report
    }
    #[cfg(feature = "cli-operator-preview")]
    pub fn logs(
        command: &'static str,
        entries: Vec<worldstream_studio_supervisor::managed_lifecycle::LifecycleLogEntry>,
    ) -> Self {
        let mut report = Self::new(command, CommandOutcome::Complete);
        report.logs = Some(entries);
        report
    }
    #[cfg(feature = "cli-operator-preview")]
    pub fn room_setup(
        command: &'static str,
        result: crate::cli_room_setup::RoomSetupExecution,
    ) -> Self {
        use crate::cli_room_setup::RoomSetupExecution;
        match result {
            RoomSetupExecution::Complete(summary) => {
                let mut report = Self::new(command, CommandOutcome::Complete);
                report.room_setup = Some(summary);
                report
            }
            RoomSetupExecution::Rejected(issue) => {
                let mut report = Self::new(command, CommandOutcome::Rejected);
                report.setup_issue = Some(issue);
                report
            }
            RoomSetupExecution::Failed(issue) => {
                let mut report = Self::new(command, CommandOutcome::Failed);
                report.setup_issue = Some(issue);
                report
            }
            RoomSetupExecution::Unavailable => {
                Self::new(command, CommandOutcome::ControllerUnavailable)
            }
        }
    }
    #[cfg(feature = "cli-operator-preview")]
    pub fn server(
        command: &'static str,
        outcome: CommandOutcome,
        server: worldstream_studio_supervisor::managed_lifecycle::LifecycleStatus,
    ) -> Self {
        let mut report = Self::new(command, outcome);
        report.server = Some(server);
        report
    }
    #[cfg(feature = "cli-operator-preview")]
    #[must_use]
    pub fn initialization_import_requires_initialization() -> Self {
        let mut report = Self::new("init", CommandOutcome::Rejected);
        report.code = "initialization_required";
        report.message = "Initialize the protected installation before reviewing imports.";
        "Run worldstreamctl init with the selected configuration and state directory, then repeat import preview."
            .clone_into(&mut report.next_action);
        report
    }

    #[cfg(feature = "cli-operator-preview")]
    #[must_use]
    pub fn initialization_import_requires_approval() -> Self {
        let mut report = Self::new("init", CommandOutcome::Rejected);
        report.code = "import_approval_required";
        report.message = "Exact reviewed import approval is required.";
        "Repeat import preview and supply its exact digest with --approve-imports and the same declaration files."
            .clone_into(&mut report.next_action);
        report
    }

    #[cfg(feature = "cli-operator-preview")]
    #[must_use]
    pub fn initialization_import_review(review: ImportReviewV1) -> Self {
        let mut report = Self::new("init", CommandOutcome::Complete);
        report.message = "Local import review completed; no services were started.";
        "Review the exact declarations and targets, then repeat the import arguments with --approve-imports and import_review.digest."
            .clone_into(&mut report.next_action);
        report.import_review = Some(review);
        report
    }

    #[cfg(feature = "cli-operator-preview")]
    #[must_use]
    pub fn initialization_import_apply(applied: ImportApplyV1) -> Self {
        let mut report = Self::new("init", CommandOutcome::Complete);
        report.message = "Reviewed local imports completed; no services were started.";
        report.import_apply = Some(applied);
        report
    }

    #[cfg(feature = "cli-operator-preview")]
    #[must_use]
    pub fn initialization_import_incomplete() -> Self {
        let mut report = Self::new("init", CommandOutcome::Failed);
        report.message = "Import publication may be incomplete.";
        "Inspect the installation, then retry the exact reviewed declaration files and approval digest; do not assume rollback."
            .clone_into(&mut report.next_action);
        report
    }

    #[cfg(feature = "cli-operator-preview")]
    #[must_use]
    pub fn initialization(output: InitializationOutput) -> Self {
        let mut report = Self::new("init", CommandOutcome::Complete);
        report.message = match &output.mode {
            InitializationMode::Preview => {
                "Local initialization preview completed; no services were started."
            }
            InitializationMode::Initialized => {
                "Local initialization completed; no services were started."
            }
        };
        "Use initialization.next_config_args to select this configuration explicitly for subsequent commands."
            .clone_into(&mut report.next_action);
        report.initialization = Some(output);
        report
    }
    #[must_use]
    pub fn invalid_arguments(command: &'static str) -> Self {
        Self::new(command, CommandOutcome::InvalidArguments)
    }

    #[must_use]
    pub fn not_implemented(command: &'static str) -> Self {
        Self::new(command, CommandOutcome::NotImplemented)
    }

    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "keep the closed command outcome and stable output mapping together"
    )]
    pub fn new(command: &'static str, outcome: CommandOutcome) -> Self {
        let mut setup = None;
        let mut lifecycle = None;
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
            CommandOutcome::PartialLifecycle { stage } => {
                lifecycle = Some(LifecycleProgress { stage });
                (
                    CommandStatus::Partial,
                    "lifecycle_incomplete",
                    "Managed lifecycle work is incomplete.",
                    "Inspect 'worldstreamctl server status' and 'worldstreamctl runner list' using the same installation options before taking further action.",
                )
            }
        };
        let next_action = setup.as_ref().map_or_else(
            || next_action.to_owned(),
            |progress| format!(
                "Inspect 'worldstreamctl room setup status {operation}', then resume that intent with 'worldstreamctl room setup resume {operation}' using the same installation options.",
                operation = progress.operation_id.as_str(),
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
            lifecycle,
            #[cfg(feature = "cli-operator-preview")]
            initialization: None,
            #[cfg(feature = "cli-operator-preview")]
            import_review: None,
            #[cfg(feature = "cli-operator-preview")]
            import_apply: None,
            #[cfg(feature = "cli-operator-preview")]
            server: None,
            #[cfg(feature = "cli-operator-preview")]
            retained_server: None,
            #[cfg(feature = "cli-operator-preview")]
            logs: None,
            #[cfg(feature = "cli-operator-preview")]
            room_setup: None,
            #[cfg(feature = "cli-operator-preview")]
            setup_issue: None,
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
            #[cfg(feature = "cli-operator-preview")]
            #[allow(
                clippy::unnecessary_debug_formatting,
                reason = "Human-readable paths must remain quoted and escaped."
            )]
            if let Some(initialization) = &self.initialization {
                writeln!(stdout, "Config: {:?}", initialization.config_path)?;
                writeln!(stdout, "State: {:?}", initialization.state_dir)?;
                writeln!(stdout, "Data: {:?}", initialization.data_dir)?;
                let arguments = serde_json::to_string(&initialization.next_config_args)
                    .map_err(|_| io::Error::other("operator result could not be encoded"))?;
                writeln!(stdout, "Next config arguments: {arguments}")?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(review) = &self.import_review {
                let document = serde_json::to_string_pretty(review)
                    .map_err(|_| io::Error::other("operator review could not be encoded"))?;
                writeln!(stdout, "{document}")?;
                writeln!(stdout, "{}", self.next_action)?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(applied) = &self.import_apply {
                let document = serde_json::to_string_pretty(applied)
                    .map_err(|_| io::Error::other("operator result could not be encoded"))?;
                writeln!(stdout, "{document}")?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(server) = &self.server {
                let document = serde_json::to_string_pretty(server)
                    .map_err(|_| io::Error::other("server status could not be encoded"))?;
                writeln!(stdout, "{document}")?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(retained) = &self.retained_server {
                let document = serde_json::to_string_pretty(retained).map_err(|_| {
                    io::Error::other("retained server evidence could not be encoded")
                })?;
                writeln!(
                    stdout,
                    "Retained evidence only; this is not live process health.\n{document}"
                )?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(logs) = &self.logs {
                let document = serde_json::to_string_pretty(logs)
                    .map_err(|_| io::Error::other("server logs could not be encoded"))?;
                writeln!(stdout, "{document}")?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(summary) = &self.room_setup {
                let document = serde_json::to_string_pretty(summary)
                    .map_err(|_| io::Error::other("setup summary could not be encoded"))?;
                writeln!(stdout, "{document}")?;
            }
            #[cfg(feature = "cli-operator-preview")]
            if let Some(issue) = &self.setup_issue {
                let document = serde_json::to_string(issue)
                    .map_err(|_| io::Error::other("setup diagnostic could not be encoded"))?;
                writeln!(stdout, "{document}")?;
            }
            if let Some(progress) = &self.setup {
                writeln!(stdout, "Operation: {}", progress.operation_id.as_str())?;
                if let Some(room_id) = &progress.room_id {
                    writeln!(stdout, "Room: {}", room_id.as_str())?;
                }
                writeln!(stdout, "Stage: {}", progress.stage.label())?;
            }
            if let Some(progress) = &self.lifecycle {
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
    use super::{CommandOutcome, CommandReport, LifecycleStage, PublicReference, SetupStage};

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

    #[test]
    fn partial_lifecycle_has_its_own_literal_result_without_room_setup_guidance()
    -> std::io::Result<()> {
        let report = CommandReport::new(
            "server restart",
            CommandOutcome::PartialLifecycle {
                stage: LifecycleStage::ManagedRunnerRestore,
            },
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(report.write(true, &mut stdout, &mut stderr)?.code(), 4);
        assert_eq!(stdout, br#"{"schema":"worldstream/operator-command/v1","command":"server restart","status":"partial","code":"lifecycle_incomplete","message":"Managed lifecycle work is incomplete.","next_action":"Inspect 'worldstreamctl server status' and 'worldstreamctl runner list' using the same installation options before taking further action.","stage":"managed_runner_restore"}
"#);
        assert_eq!(stderr, b"Managed lifecycle work is incomplete. Inspect 'worldstreamctl server status' and 'worldstreamctl runner list' using the same installation options before taking further action.\n");

        for (stage, expected_stdout) in [
            (
                LifecycleStage::ManagedRunnerStop,
                "server restart: partial (lifecycle_incomplete)\nStage: managed_runner_stop\n",
            ),
            (
                LifecycleStage::RuntimeStop,
                "server restart: partial (lifecycle_incomplete)\nStage: runtime_stop\n",
            ),
            (
                LifecycleStage::RuntimeRestart,
                "server restart: partial (lifecycle_incomplete)\nStage: runtime_restart\n",
            ),
            (
                LifecycleStage::ManagedRunnerRestore,
                "server restart: partial (lifecycle_incomplete)\nStage: managed_runner_restore\n",
            ),
        ] {
            let report =
                CommandReport::new("server restart", CommandOutcome::PartialLifecycle { stage });
            stdout.clear();
            stderr.clear();
            assert_eq!(report.write(false, &mut stdout, &mut stderr)?.code(), 4);
            assert_eq!(stdout, expected_stdout.as_bytes());
            assert_eq!(stderr, b"Managed lifecycle work is incomplete. Inspect 'worldstreamctl server status' and 'worldstreamctl runner list' using the same installation options before taking further action.\n");
        }
        Ok(())
    }
}
