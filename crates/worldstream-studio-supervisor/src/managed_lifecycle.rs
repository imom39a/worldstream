//! Durable local restart coordination over existing managed-process operations.
//! Runtime ownership and authenticated process control remain at the process seam.

use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_runtime::{SecretSource, create_owner_only_file, validate_data_directory};

use crate::{
    managed_agent_host::{ManagedAgentHostOperationsV1, ManagedAgentHostStateV1},
    protected_publication::{PublicationMode, publish},
    runner_templates::{RunnerInstanceStateV1, RunnerSupervisorV1},
};

const SCHEMA: &str = "worldstream/managed-lifecycle-operation/v1";
const MAX_RECORD_BYTES: usize = 128 * 1024;
const MAX_MANAGED_TARGETS: usize = 512;

/// Fixed process-control or protected-record failure, never raw process output.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LifecycleError {
    #[error("managed lifecycle control is unavailable")]
    Unavailable,
    #[error("managed lifecycle state is invalid or unsafe")]
    Invalid,
    #[error("managed lifecycle publication is uncertain; inspect retained progress")]
    PublicationUncertain,
    #[error("automatic restart of bound Runner instances is not supported in this preview")]
    BoundRunnerRestartUnsupported,
}

/// Current process evidence, independent of retained operation progress.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeObservation {
    Stopped,
    Ready,
    Unavailable,
    Unmanaged,
    Starting,
}

/// Fixed configured Runtime process seam. Implementations must prove ownership
/// before control; start returns only after readiness, stop only after exit.
pub trait RuntimeControl: Send + Sync + 'static {
    fn observe(&self) -> RuntimeObservation;
    /// Starts or reuses the configured Runtime after proving its ownership.
    ///
    /// # Errors
    /// Returns an error when startup or verified readiness is incomplete.
    fn start(&self) -> Result<(), LifecycleError>;
    /// Stops only the verified owned Runtime and confirms its exit.
    ///
    /// # Errors
    /// Returns an error on unknown ownership or incomplete shutdown.
    fn stop(&self) -> Result<(), LifecycleError>;
}

/// Exact locally managed target; external processes cannot enter this set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagedRunnerTarget {
    Template { instance_id: String },
    AgentHost { assignment_id: String },
}

/// Durable checkpoint. This is not a Room Phase or canonical Runtime state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStage {
    ManagedRunnerStop,
    RuntimeStop,
    RuntimeRestart,
    ManagedRunnerRestore,
    Complete,
}

/// Explicit local process intent, distinct from Room setup or Activity state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAction {
    Start,
    Stop,
    Restart,
}

/// Safe restart receipt retained before any process mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleOperation {
    pub schema: String,
    pub operation_id: u64,
    pub action: LifecycleAction,
    pub stage: LifecycleStage,
    pub captured_running: Vec<ManagedRunnerTarget>,
    pub restore_remaining: Vec<ManagedRunnerTarget>,
}

/// Current evidence plus any retained process-operation checkpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleStatus {
    pub runtime: RuntimeObservation,
    pub operation: Option<LifecycleOperation>,
    pub logs_available: bool,
}

/// A bounded operator event, not subprocess output or canonical Room history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleLogEntry {
    pub sequence: u64,
    pub operation_id: u64,
    pub action: LifecycleAction,
    pub stage: LifecycleStage,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LifecycleLog {
    schema: String,
    entries: Vec<LifecycleLogEntry>,
}

/// One Controller's persistent coordinator. Call only while that Controller
/// holds the installation lifetime lease; clones serialize their mutations.
#[derive(Clone)]
pub struct ManagedLifecycle {
    record: Arc<PathBuf>,
    runtime: Arc<dyn RuntimeControl>,
    runners: RunnerSupervisorV1,
    hosts: ManagedAgentHostOperationsV1,
    mutation: Arc<Mutex<()>>,
}

impl ManagedLifecycle {
    /// Opens retained progress without starting or changing processes.
    ///
    /// # Errors
    /// Rejects unsafe state or malformed retained process-operation records.
    pub fn open(
        state: &Path,
        runtime: impl RuntimeControl,
        runners: RunnerSupervisorV1,
        hosts: ManagedAgentHostOperationsV1,
    ) -> Result<Self, LifecycleError> {
        let state = validate_data_directory(state).map_err(|_| LifecycleError::Invalid)?;
        let lifecycle = Self {
            record: Arc::new(state.join("managed-lifecycle.v1.json")),
            runtime: Arc::new(runtime),
            runners,
            hosts,
            mutation: Arc::new(Mutex::new(())),
        };
        if lifecycle.read()?.is_some_and(|operation| {
            operation.stage != LifecycleStage::Complete || operation.action == LifecycleAction::Stop
        }) {
            // A new Controller must preserve the start fence for retained partial work.
            drop(
                lifecycle
                    .hosts
                    .pause_starts()
                    .map_err(|_| LifecycleError::Unavailable)?,
            );
            drop(
                lifecycle
                    .runners
                    .pause_starts()
                    .map_err(|_| LifecycleError::Unavailable)?,
            );
        }
        Ok(lifecycle)
    }

    /// Reads evidence and retained progress; never starts either process.
    ///
    /// # Errors
    /// Rejects malformed or unsafe retained progress.
    pub fn status(&self) -> Result<LifecycleStatus, LifecycleError> {
        Ok(LifecycleStatus {
            runtime: self.runtime.observe(),
            operation: self.read()?,
            logs_available: self.logs(1).is_ok(),
        })
    }

    /// Reads recent closed lifecycle events without starting or changing anything.
    ///
    /// # Errors
    /// Rejects unsafe, missing, stale or oversized logs and invalid tail bounds.
    pub fn logs(&self, tail: usize) -> Result<Vec<LifecycleLogEntry>, LifecycleError> {
        if !(1..=1000).contains(&tail) {
            return Err(LifecycleError::Invalid);
        }
        let entries = self.read_logs()?;
        if let Some(operation) = self.read()?
            && !entries.last().is_some_and(|entry| {
                entry.operation_id == operation.operation_id
                    && entry.action == operation.action
                    && entry.stage == operation.stage
            })
        {
            return Err(LifecycleError::Unavailable);
        }
        Ok(entries[entries.len().saturating_sub(tail)..].to_vec())
    }

    fn read_logs(&self) -> Result<Vec<LifecycleLogEntry>, LifecycleError> {
        let path = self.record.with_file_name("managed-lifecycle-log.v1.json");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(LifecycleError::Unavailable),
            Ok(_) => {}
        }
        let bytes = SecretSource::File(path)
            .read_bounded(256 * 1024)
            .map_err(|_| LifecycleError::Invalid)?;
        let log: LifecycleLog =
            serde_json::from_slice(&bytes).map_err(|_| LifecycleError::Invalid)?;
        if log.schema != "worldstream/managed-lifecycle-log/v1"
            || log.entries.is_empty()
            || log.entries.len() > 1000
            || log
                .entries
                .iter()
                .any(|entry| entry.sequence == 0 || entry.operation_id == 0)
            || log.entries.windows(2).any(|pair| {
                pair[0].sequence.checked_add(1) != Some(pair[1].sequence)
                    || pair[0].operation_id > pair[1].operation_id
            })
        {
            return Err(LifecycleError::Invalid);
        }
        Ok(log.entries)
    }

    fn append_log(&self, operation: &LifecycleOperation) -> Result<(), LifecycleError> {
        let mut entries = self.read_logs()?;
        let sequence = entries
            .last()
            .map_or(0, |entry| entry.sequence)
            .checked_add(1)
            .ok_or(LifecycleError::Invalid)?;
        entries.push(LifecycleLogEntry {
            sequence,
            operation_id: operation.operation_id,
            action: operation.action,
            stage: operation.stage,
        });
        if entries.len() > 1000 {
            entries.remove(0);
        }
        let bytes = serde_json::to_vec(&LifecycleLog {
            schema: "worldstream/managed-lifecycle-log/v1".into(),
            entries,
        })
        .map_err(|_| LifecycleError::Invalid)?;
        let target = self.record.with_file_name("managed-lifecycle-log.v1.json");
        let mode = if target
            .try_exists()
            .map_err(|_| LifecycleError::Unavailable)?
        {
            PublicationMode::Replace
        } else {
            PublicationMode::CreateNew
        };
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| LifecycleError::Unavailable)?;
        let temporary = self.record.with_file_name(format!(
            ".managed-log-{}.tmp",
            blake3::hash(&nonce).to_hex()
        ));
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| LifecycleError::Unavailable)?;
        let result = match file.write_all(&bytes) {
            Ok(()) => publish(file, &temporary, &target, mode),
            Err(error) => {
                drop(file);
                Err(error)
            }
        };
        let _ = fs::remove_file(&temporary);
        result.map_err(|_| LifecycleError::PublicationUncertain)
    }

    /// Restarts the Runtime and restores only the captured owned managed set.
    ///
    /// # Errors
    /// Returns a closed error while retaining the last durable checkpoint.
    pub fn restart(&self) -> Result<LifecycleOperation, LifecycleError> {
        self.operate(LifecycleAction::Restart)
    }

    /// Starts the Runtime without restoring previously stopped managed targets.
    ///
    /// # Errors
    /// Retains an incomplete startup for inspection and retry.
    pub fn start(&self) -> Result<LifecycleOperation, LifecycleError> {
        self.operate(LifecycleAction::Start)
    }

    /// Stops owned managed targets before stopping the verified Runtime.
    ///
    /// # Errors
    /// Retains incomplete shutdown and never claims unverified process exit.
    pub fn stop(&self) -> Result<LifecycleOperation, LifecycleError> {
        self.operate(LifecycleAction::Stop)
    }

    fn operate(&self, action: LifecycleAction) -> Result<LifecycleOperation, LifecycleError> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut starts_paused, previous_host_pause) = self
            .hosts
            .pause_starts()
            .map_err(|_| LifecycleError::Unavailable)?;
        let (mut runner_starts_paused, previous_runner_pause) = self
            .runners
            .pause_starts()
            .map_err(|_| LifecycleError::Unavailable)?;
        let previous = self.read()?;
        let mut operation = match previous {
            Some(operation) if operation.stage != LifecycleStage::Complete => {
                if operation.action != action {
                    return Err(LifecycleError::Unavailable);
                }
                operation
            }
            previous => {
                let captured_running = if action == LifecycleAction::Start {
                    Vec::new()
                } else {
                    self.capture_running()?
                };
                if action == LifecycleAction::Restart
                    && captured_running.iter().any(|target| match target {
                        ManagedRunnerTarget::Template { instance_id } => {
                            !self.runners.task_runner_binding_available(instance_id)
                        }
                        ManagedRunnerTarget::AgentHost { .. } => false,
                    })
                {
                    *starts_paused = previous_host_pause;
                    *runner_starts_paused = previous_runner_pause;
                    return Err(LifecycleError::BoundRunnerRestartUnsupported);
                }
                let operation = LifecycleOperation {
                    schema: SCHEMA.to_owned(),
                    action,
                    operation_id: previous
                        .map_or(0, |operation| operation.operation_id)
                        .checked_add(1)
                        .ok_or(LifecycleError::Unavailable)?,
                    stage: if action == LifecycleAction::Start {
                        LifecycleStage::RuntimeRestart
                    } else {
                        LifecycleStage::ManagedRunnerStop
                    },
                    restore_remaining: if action == LifecycleAction::Restart {
                        captured_running.clone()
                    } else {
                        Vec::new()
                    },
                    captured_running,
                };
                self.persist(&operation)?;
                operation
            }
        };
        loop {
            match operation.stage {
                LifecycleStage::ManagedRunnerStop => {
                    for target in &operation.captured_running {
                        self.stop_target(target)?;
                    }
                    operation.stage = LifecycleStage::RuntimeStop;
                }
                LifecycleStage::RuntimeStop => {
                    self.runtime.stop()?;
                    operation.stage = if action == LifecycleAction::Stop {
                        LifecycleStage::Complete
                    } else {
                        LifecycleStage::RuntimeRestart
                    };
                }
                LifecycleStage::RuntimeRestart => {
                    self.runtime.start()?;
                    operation.stage = if action == LifecycleAction::Restart {
                        LifecycleStage::ManagedRunnerRestore
                    } else {
                        LifecycleStage::Complete
                    };
                }
                LifecycleStage::ManagedRunnerRestore => {
                    while let Some(target) = operation.restore_remaining.first() {
                        self.restore_target(target)?;
                        operation.restore_remaining.remove(0);
                        self.persist(&operation)?;
                    }
                    operation.stage = LifecycleStage::Complete;
                }
                LifecycleStage::Complete => {
                    *starts_paused = action == LifecycleAction::Stop;
                    *runner_starts_paused = action == LifecycleAction::Stop;
                    return Ok(operation);
                }
            }
            self.persist(&operation)?;
        }
    }

    fn capture_running(&self) -> Result<Vec<ManagedRunnerTarget>, LifecycleError> {
        let mut captured = self
            .runners
            .owned_instances()
            .map_err(|_| LifecycleError::Unavailable)?
            .into_iter()
            .map(|instance_id| ManagedRunnerTarget::Template { instance_id })
            .collect::<Vec<_>>();
        captured.extend(
            self.hosts
                .owned_assignments()
                .map_err(|_| LifecycleError::Unavailable)?
                .into_iter()
                .map(|assignment_id| ManagedRunnerTarget::AgentHost { assignment_id }),
        );
        if captured.len() > MAX_MANAGED_TARGETS {
            return Err(LifecycleError::Unavailable);
        }
        Ok(captured)
    }

    fn stop_target(&self, target: &ManagedRunnerTarget) -> Result<(), LifecycleError> {
        match target {
            ManagedRunnerTarget::Template { instance_id } => {
                let status = self
                    .runners
                    .stop(instance_id)
                    .ok_or(LifecycleError::Unavailable)?;
                if !status.instances.iter().any(|instance| {
                    instance.instance_id == *instance_id
                        && instance.state == RunnerInstanceStateV1::Stopped
                }) {
                    return Err(LifecycleError::Unavailable);
                }
            }
            ManagedRunnerTarget::AgentHost { assignment_id } => {
                let status = self
                    .hosts
                    .stop(assignment_id)
                    .map_err(|_| LifecycleError::Unavailable)?;
                if status.state != ManagedAgentHostStateV1::Stopped {
                    return Err(LifecycleError::Unavailable);
                }
            }
        }
        Ok(())
    }

    fn restore_target(&self, target: &ManagedRunnerTarget) -> Result<(), LifecycleError> {
        match target {
            ManagedRunnerTarget::Template { instance_id } => {
                // A retained Runner capability is not evidence that its current
                // assignment is still eligible. Without an exact eligibility
                // source, leave bound restoration pending rather than launching.
                if !self.runners.task_runner_binding_available(instance_id) {
                    return Err(LifecycleError::Unavailable);
                }
                let status = self
                    .runners
                    .start_permitted(instance_id)
                    .ok_or(LifecycleError::Unavailable)?;
                if !status.instances.iter().any(|instance| {
                    instance.instance_id == *instance_id
                        && instance.managed_by_supervisor
                        && instance.state == RunnerInstanceStateV1::Running
                }) {
                    return Err(LifecycleError::Unavailable);
                }
            }
            ManagedRunnerTarget::AgentHost { assignment_id } => {
                let status = self
                    .hosts
                    .start_permitted(assignment_id)
                    .map_err(|_| LifecycleError::Unavailable)?;
                if status.state != ManagedAgentHostStateV1::Running {
                    return Err(LifecycleError::Unavailable);
                }
            }
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<LifecycleOperation>, LifecycleError> {
        read_operation_record(self.record.as_ref())
    }

    fn persist(&self, operation: &LifecycleOperation) -> Result<(), LifecycleError> {
        validate_operation(operation)?;
        let mode = if self.read()?.is_some() {
            PublicationMode::Replace
        } else {
            PublicationMode::CreateNew
        };
        let bytes = serde_json::to_vec(operation).map_err(|_| LifecycleError::Invalid)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(LifecycleError::Invalid);
        }
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| LifecycleError::Unavailable)?;
        let temporary = self.record.with_file_name(format!(
            ".managed-lifecycle-{}.tmp",
            blake3::hash(&nonce).to_hex()
        ));
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| LifecycleError::Unavailable)?;
        let result = match file.write_all(&bytes) {
            Ok(()) => publish(file, &temporary, self.record.as_ref(), mode),
            Err(error) => {
                drop(file);
                Err(error)
            }
        };
        let _ = fs::remove_file(&temporary);
        result.map_err(|_| LifecycleError::PublicationUncertain)?;
        // Diagnostics cannot undo or block a durable lifecycle checkpoint.
        // Reads expose a missing/stale log through logs_available, never as fresh success.
        let _ = self.append_log(operation);
        Ok(())
    }
}

pub(crate) fn read_operation_record(
    path: &Path,
) -> Result<Option<LifecycleOperation>, LifecycleError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(LifecycleError::Unavailable),
        Ok(_) => {}
    }
    let bytes = SecretSource::File(path.to_path_buf())
        .read_bounded(MAX_RECORD_BYTES)
        .map_err(|_| LifecycleError::Invalid)?;
    let operation: LifecycleOperation =
        serde_json::from_slice(&bytes).map_err(|_| LifecycleError::Invalid)?;
    validate_operation(&operation)?;
    Ok(Some(operation))
}

fn validate_operation(operation: &LifecycleOperation) -> Result<(), LifecycleError> {
    if operation.schema != SCHEMA
        || operation.operation_id == 0
        || operation.captured_running.len() > MAX_MANAGED_TARGETS
        || operation.restore_remaining.len() > operation.captured_running.len()
        || operation
            .restore_remaining
            .iter()
            .any(|target| !operation.captured_running.contains(target))
    {
        return Err(LifecycleError::Invalid);
    }
    for target in &operation.captured_running {
        let identity = match target {
            ManagedRunnerTarget::Template { instance_id } => instance_id,
            ManagedRunnerTarget::AgentHost { assignment_id } => assignment_id,
        };
        if identity.is_empty()
            || identity.len() > 128
            || !identity
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || b"._-".contains(&value))
        {
            return Err(LifecycleError::Invalid);
        }
    }
    Ok(())
}
