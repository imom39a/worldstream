//! Owner-only local control plane for the long-lived execution supervisor.
//!
//! The endpoint publication is protected by the same owner-only data directory
//! as the execution journal. The listener accepts loopback connections only;
//! every request also proves knowledge of a random generation secret that is
//! stored solely in that protected publication. Dropping a client has no
//! effect on daemon or Swarm execution state.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_renameable_file, validate_data_directory, validate_owner_only_file,
};

use super::journal::JournalError;
use super::{
    EffectIntent, EffectOutcome, ExecutionCommand, ExecutionError, ExecutionReceipt,
    ExecutionSnapshot, ExecutionSupervisor, FileExecutionJournal, InvocationResolution,
    InvocationTicket, NativeProcessSpawner, OwnedInvocation, PreparedInvocation, ProcessError,
    ProcessExit, ProviderKind, RunBudget, ScheduleDecision,
};

const ENDPOINT_FILE: &str = "execution-control.v1.json";
const ENDPOINT_SCHEMA: &str = "worldstream/agent-swarm-execution-endpoint@1";
const REQUEST_SCHEMA: &str = "worldstream/agent-swarm-execution-control-request@1";
const RESPONSE_SCHEMA: &str = "worldstream/agent-swarm-execution-control-response@1";
const MAX_ENDPOINT_BYTES: u64 = 16 * 1024;
// Launch carries a fully prepared Invocation whose prompt may be 4 MiB. Keep
// framing aligned with the native guard's bounded request envelope.
const MAX_REQUEST_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RETAINED_COMPLETIONS: usize = 1024;
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_GRACE: Duration = Duration::from_secs(3);

/// Explicit commands exposed by the local execution daemon.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlCommand {
    Status {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        swarm_id: Option<String>,
    },
    Pause {
        swarm_id: String,
    },
    Resume {
        swarm_id: String,
    },
    Stop {
        swarm_id: String,
    },
    /// Interrupts only one exact active Invocation. It does not change the
    /// Swarm's desired execution state.
    CancelInvocation {
        swarm_id: String,
        invocation_id: String,
    },
    RegisterSwarm {
        swarm_id: String,
        priority: u8,
        budget: RunBudget,
        /// Selected member count per provider from the authoritative roster.
        /// Omitted by legacy clients, an empty map grants no automatic slots.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        roster_provider_counts: BTreeMap<ProviderKind, u16>,
    },
    SetProviderCap {
        provider: ProviderKind,
        limit: u16,
    },
    SetPriority {
        swarm_id: String,
        priority: u8,
    },
    SetBudget {
        swarm_id: String,
        budget: RunBudget,
    },
    Enqueue {
        ticket: InvocationTicket,
    },
    ResolveInvocation {
        swarm_id: String,
        invocation_id: String,
        resolution: InvocationResolution,
    },
    /// Starts one already-admitted exact request. The daemon rejects prepared
    /// requests that do not match the supervisor's active ticket identity.
    Launch {
        ticket: InvocationTicket,
        prepared: Box<PreparedInvocation>,
    },
    /// Returns a retained terminal process result without consuming it.
    Collect {
        invocation_id: String,
    },
    /// Durably resolves and releases a previously collected normal result.
    AcknowledgeCompletion {
        invocation_id: String,
    },
    /// Persists an exact write-ahead intent before any external target contact.
    PrepareEffect {
        intent: EffectIntent,
    },
    /// Persists the final pre-contact cut for an already prepared effect.
    MarkEffectDispatched {
        operation_id: String,
    },
    /// Records an exact target observation; ambiguous outcomes remain blocked.
    ObserveEffect {
        operation_id: String,
        outcome: EffectOutcome,
    },
    /// Acknowledges only an effect with a retained terminal target outcome.
    AcknowledgeEffect {
        operation_id: String,
    },
    /// Durably reserves eligible tickets. It does not launch a provider;
    /// callers must keep this command behind an in-daemon coordinator that can
    /// immediately bind each returned admission to an owned process tree.
    Tick,
}

/// Successful daemon result. Status is operational state only and never a
/// projection of authoritative Room facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ControlResult {
    Status(ExecutionSnapshot),
    Mutation(ExecutionReceipt),
    Launch(InvocationLaunchReceipt),
    Tick(DaemonTick),
    Completion(InvocationProcessResult),
}

/// Successful owned-process launch. PID is diagnostic and is never accepted
/// back as signalling authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationLaunchReceipt {
    pub invocation_id: String,
    pub pid: Option<u32>,
}

/// One retained provider process observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationProcessResult {
    pub invocation_id: String,
    pub swarm_id: String,
    pub resolution: InvocationResolution,
    pub exit: Option<ProcessExit>,
}

/// Lightweight retained-result identity included in daemon ticks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationCompletion {
    pub invocation_id: String,
    pub swarm_id: String,
    pub resolution: InvocationResolution,
}

/// One coordinator tick. Admissions are durable reservations; completions
/// remain retained until separately collected and acknowledged.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonTick {
    pub admissions: Vec<ScheduleDecision>,
    pub completions: Vec<InvocationCompletion>,
}

/// Closed, stable control failure vocabulary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlFault {
    #[error("the control request is invalid")]
    InvalidRequest,
    #[error("the requested execution object was not found")]
    NotFound,
    #[error("the request conflicts with retained execution state")]
    Conflict,
    #[error("execution recovery is required")]
    RecoveryRequired,
    #[error("the execution budget is exhausted")]
    BudgetExhausted,
    #[error("the protected execution control plane is unavailable")]
    Unavailable,
}

/// Non-secret daemon readiness information.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonPublication {
    pub endpoint: SocketAddr,
    pub process_id: u32,
}

/// Long-lived single-owner execution daemon.
pub struct ExecutionDaemon {
    supervisor: ExecutionSupervisor<FileExecutionJournal>,
    listener: TcpListener,
    publication: EndpointPublication,
    spawner: Option<NativeProcessSpawner>,
    running: BTreeMap<String, OwnedProcess>,
    completed: BTreeMap<String, InvocationProcessResult>,
    completed_requests: BTreeMap<String, String>,
}

struct OwnedProcess {
    swarm_id: String,
    request_digest: String,
    process: OwnedInvocation,
}

impl ExecutionDaemon {
    /// Acquires the sole journal lease, opens crash-safe supervisor state,
    /// binds a fresh loopback listener, and atomically publishes its secret.
    ///
    /// # Errors
    /// Fails closed for unsafe storage, another live owner, corrupt retained
    /// state, listener failure, randomness failure, or uncertain publication.
    pub fn bind(root: &Path) -> Result<Self, DaemonError> {
        Self::bind_inner(root, None)
    }

    /// Binds a process-capable daemon around the exact fixed guard binary.
    ///
    /// # Errors
    /// Includes [`Self::bind`] failures and invalid guard executables.
    pub fn bind_with_guard(root: &Path, guard_executable: &Path) -> Result<Self, DaemonError> {
        // Acquire the sole journal lease before stale-profile cleanup. A
        // competing daemon must never remove a live owner's materialization.
        let mut daemon = Self::bind_inner(root, None)?;
        let profile_root = daemon
            .publication
            .path
            .parent()
            .ok_or(DaemonError::UnsafeStorage)?
            .join("provider-profiles");
        daemon.spawner = Some(
            NativeProcessSpawner::with_default_capture(guard_executable)?
                .with_profile_root(&profile_root)?,
        );
        Ok(daemon)
    }

    fn bind_inner(root: &Path, spawner: Option<NativeProcessSpawner>) -> Result<Self, DaemonError> {
        let journal = FileExecutionJournal::open(root)?;
        let root = validate_data_directory(root).map_err(|_| DaemonError::UnsafeStorage)?;
        let supervisor = ExecutionSupervisor::open(journal, now_ms()?)?;
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .map_err(|_| DaemonError::Unavailable)?;
        let endpoint = listener
            .local_addr()
            .map_err(|_| DaemonError::Unavailable)?;
        if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
            return Err(DaemonError::Unavailable);
        }
        let publication = EndpointPublication::publish(&root, endpoint)?;
        Ok(Self {
            supervisor,
            listener,
            publication,
            spawner,
            running: BTreeMap::new(),
            completed: BTreeMap::new(),
            completed_requests: BTreeMap::new(),
        })
    }

    #[must_use]
    pub fn publication(&self) -> DaemonPublication {
        DaemonPublication {
            endpoint: self.publication.record.endpoint,
            process_id: self.publication.record.process_id,
        }
    }

    /// Serves clients until the process is terminated. Client disconnects are
    /// request-local and never change desired execution.
    ///
    /// # Errors
    /// Listener failure terminates the daemon so an external owner can restart
    /// it and invoke the supervisor's conservative crash recovery.
    pub fn run(&mut self) -> Result<(), DaemonError> {
        self.listener
            .set_nonblocking(true)
            .map_err(|_| DaemonError::Unavailable)?;
        loop {
            self.poll_owned(now_ms()?)?;
            match self.listener.accept() {
                Ok((stream, peer)) => self.handle_stream(stream, peer),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return Err(DaemonError::Unavailable),
            }
        }
    }

    /// Serves exactly one connection. This is public to support embedding in a
    /// host event loop and deterministic lifecycle tests.
    ///
    /// # Errors
    /// Returns only listener-level failures. Invalid or unauthorized requests
    /// receive a closed response and leave the daemon available.
    pub fn serve_once(&mut self) -> Result<(), DaemonError> {
        self.listener
            .set_nonblocking(false)
            .map_err(|_| DaemonError::Unavailable)?;
        let (stream, peer) = self
            .listener
            .accept()
            .map_err(|_| DaemonError::Unavailable)?;
        self.handle_stream(stream, peer);
        Ok(())
    }

    fn handle_stream(&mut self, mut stream: TcpStream, peer: SocketAddr) {
        if !peer.ip().is_loopback() {
            return;
        }
        // Accepted sockets can inherit the listener's nonblocking mode on
        // macOS. A client may connect before writing its frame; wait for that
        // frame within the IO timeout instead of rejecting WouldBlock as an
        // invalid request.
        if stream.set_nonblocking(false).is_err() {
            return;
        }
        let _ = stream.set_read_timeout(Some(DEFAULT_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(DEFAULT_IO_TIMEOUT));
        let response = match read_frame::<WireRequest>(&mut stream, MAX_REQUEST_BYTES) {
            Ok(request)
                if request.schema == REQUEST_SCHEMA
                    && constant_time_equal(
                        request.generation.as_bytes(),
                        self.publication.record.generation.as_bytes(),
                    ) =>
            {
                self.execute(request.command)
            }
            Ok(_) | Err(_) => WireResponse::fault(ControlFault::InvalidRequest),
        };
        let _ = write_frame(&mut stream, &response, MAX_RESPONSE_BYTES);
    }

    #[allow(clippy::too_many_lines)]
    fn execute(&mut self, command: ControlCommand) -> WireResponse {
        let Ok(now) = now_ms() else {
            return WireResponse::fault(ControlFault::Unavailable);
        };
        if self.poll_owned(now).is_err() {
            return WireResponse::fault(ControlFault::Unavailable);
        }
        let result: Result<ControlResult, ControlFault> = match command {
            ControlCommand::Status { swarm_id } => {
                let mut snapshot = self.supervisor.snapshot(now);
                if let Some(swarm_id) = swarm_id {
                    snapshot.swarms.retain(|swarm| swarm.swarm_id == swarm_id);
                    if snapshot.swarms.is_empty() {
                        return WireResponse::fault(ControlFault::NotFound);
                    }
                }
                Ok(ControlResult::Status(snapshot))
            }
            ControlCommand::Pause { swarm_id } => {
                self.mutate(ExecutionCommand::Pause { swarm_id }, now)
            }
            ControlCommand::Resume { swarm_id } => {
                self.mutate(ExecutionCommand::Resume { swarm_id }, now)
            }
            ControlCommand::Stop { swarm_id } => self.stop_swarm(&swarm_id, now),
            ControlCommand::CancelInvocation {
                swarm_id,
                invocation_id,
            } => self.cancel_invocation(&swarm_id, &invocation_id, now),
            ControlCommand::RegisterSwarm {
                swarm_id,
                priority,
                budget,
                roster_provider_counts,
            } => self.mutate(
                ExecutionCommand::RegisterSwarm {
                    swarm_id,
                    priority,
                    budget,
                    roster_provider_counts,
                },
                now,
            ),
            ControlCommand::SetProviderCap { provider, limit } => {
                self.mutate(ExecutionCommand::SetProviderCap { provider, limit }, now)
            }
            ControlCommand::SetPriority { swarm_id, priority } => {
                self.mutate(ExecutionCommand::SetPriority { swarm_id, priority }, now)
            }
            ControlCommand::SetBudget { swarm_id, budget } => {
                self.mutate(ExecutionCommand::SetBudget { swarm_id, budget }, now)
            }
            ControlCommand::Enqueue { ticket } => {
                self.mutate(ExecutionCommand::Enqueue(ticket), now)
            }
            ControlCommand::ResolveInvocation {
                swarm_id,
                invocation_id,
                resolution,
            } => self.resolve_recovered(
                ExecutionCommand::ResolveInvocation {
                    swarm_id,
                    invocation_id,
                    resolution,
                },
                now,
            ),
            ControlCommand::Launch { ticket, prepared } => self.launch(ticket, *prepared, now),
            ControlCommand::Collect { invocation_id } => self.collect(&invocation_id),
            ControlCommand::AcknowledgeCompletion { invocation_id } => {
                self.acknowledge_completion(&invocation_id, now)
            }
            ControlCommand::PrepareEffect { intent } => {
                self.mutate(ExecutionCommand::PrepareEffect(intent), now)
            }
            ControlCommand::MarkEffectDispatched { operation_id } => {
                self.mutate(ExecutionCommand::MarkEffectDispatched { operation_id }, now)
            }
            ControlCommand::ObserveEffect {
                operation_id,
                outcome,
            } => self.mutate(
                ExecutionCommand::ObserveEffect {
                    operation_id,
                    outcome,
                },
                now,
            ),
            ControlCommand::AcknowledgeEffect { operation_id } => {
                self.mutate(ExecutionCommand::AcknowledgeEffect { operation_id }, now)
            }
            ControlCommand::Tick => self.tick(now),
        };
        match result {
            Ok(result) => WireResponse::success(result),
            Err(fault) => WireResponse::fault(fault),
        }
    }

    fn mutate(
        &mut self,
        command: ExecutionCommand,
        now: u64,
    ) -> Result<ControlResult, ControlFault> {
        self.supervisor
            .command(command, now)
            .map(ControlResult::Mutation)
            .map_err(|error| map_execution_error(&error))
    }

    fn resolve_recovered(
        &mut self,
        command: ExecutionCommand,
        now: u64,
    ) -> Result<ControlResult, ControlFault> {
        let ExecutionCommand::ResolveInvocation { invocation_id, .. } = &command else {
            return Err(ControlFault::InvalidRequest);
        };
        if self.running.contains_key(invocation_id) || self.completed.contains_key(invocation_id) {
            return Err(ControlFault::Conflict);
        }
        self.mutate(command, now)
    }

    fn launch(
        &mut self,
        ticket: InvocationTicket,
        prepared: PreparedInvocation,
        now: u64,
    ) -> Result<ControlResult, ControlFault> {
        let snapshot = self.supervisor.snapshot(now);
        let active = snapshot
            .swarms
            .iter()
            .find(|swarm| swarm.swarm_id == ticket.swarm_id)
            .and_then(|swarm| {
                swarm
                    .active
                    .iter()
                    .find(|active| active.ticket.invocation_id == ticket.invocation_id)
            })
            .ok_or(ControlFault::NotFound)?;
        if active.resolution != InvocationResolution::Running
            || active.ticket != ticket
            || ticket.invocation_id != prepared.invocation_id
            || active.ticket.provider != prepared.provider
            || active.ticket.member_id != prepared.member_id
            || active.ticket.configuration_revision != prepared.configuration_revision
        {
            return Err(ControlFault::Conflict);
        }
        let request_digest = prepared_request_digest(&prepared)?;
        if let Some(owned) = self.running.get(&prepared.invocation_id) {
            if owned.swarm_id != ticket.swarm_id || owned.request_digest != request_digest {
                return Err(ControlFault::Conflict);
            }
            return Ok(ControlResult::Launch(InvocationLaunchReceipt {
                invocation_id: prepared.invocation_id.clone(),
                pid: owned.process.pid(),
            }));
        }
        if let Some(result) = self.completed.get(&prepared.invocation_id) {
            if result.swarm_id != ticket.swarm_id
                || self.completed_requests.get(&prepared.invocation_id) != Some(&request_digest)
            {
                return Err(ControlFault::Conflict);
            }
            return Ok(ControlResult::Launch(InvocationLaunchReceipt {
                invocation_id: prepared.invocation_id.clone(),
                pid: None,
            }));
        }
        if self.completed.len() >= MAX_RETAINED_COMPLETIONS {
            return Err(ControlFault::Conflict);
        }
        let spawner = self.spawner.as_ref().ok_or(ControlFault::Unavailable)?;
        let Ok(process) = spawner.spawn(&prepared) else {
            self.supervisor
                .command(
                    ExecutionCommand::ResolveInvocation {
                        swarm_id: ticket.swarm_id,
                        invocation_id: prepared.invocation_id,
                        resolution: InvocationResolution::Failed,
                    },
                    now,
                )
                .map_err(|error| map_execution_error(&error))?;
            return Err(ControlFault::Unavailable);
        };
        let receipt = InvocationLaunchReceipt {
            invocation_id: prepared.invocation_id.clone(),
            pid: process.pid(),
        };
        self.running.insert(
            prepared.invocation_id,
            OwnedProcess {
                swarm_id: ticket.swarm_id,
                request_digest,
                process,
            },
        );
        Ok(ControlResult::Launch(receipt))
    }

    fn tick(&mut self, now: u64) -> Result<ControlResult, ControlFault> {
        let admissions = if self.completed.len() >= MAX_RETAINED_COMPLETIONS {
            Vec::new()
        } else {
            self.supervisor
                .schedule(now)
                .map_err(|error| map_execution_error(&error))?
        };
        let completions = self
            .completed
            .values()
            .map(|result| InvocationCompletion {
                invocation_id: result.invocation_id.clone(),
                swarm_id: result.swarm_id.clone(),
                resolution: result.resolution,
            })
            .collect();
        Ok(ControlResult::Tick(DaemonTick {
            admissions,
            completions,
        }))
    }

    fn collect(&self, invocation_id: &str) -> Result<ControlResult, ControlFault> {
        self.completed
            .get(invocation_id)
            .cloned()
            .map(ControlResult::Completion)
            .ok_or(ControlFault::NotFound)
    }

    fn acknowledge_completion(
        &mut self,
        invocation_id: &str,
        now: u64,
    ) -> Result<ControlResult, ControlFault> {
        let result = self
            .completed
            .get(invocation_id)
            .ok_or(ControlFault::NotFound)?;
        if !matches!(
            result.resolution,
            InvocationResolution::Completed | InvocationResolution::Failed
        ) {
            return Err(ControlFault::RecoveryRequired);
        }
        let receipt = self
            .supervisor
            .command(
                ExecutionCommand::ResolveInvocation {
                    swarm_id: result.swarm_id.clone(),
                    invocation_id: result.invocation_id.clone(),
                    resolution: result.resolution,
                },
                now,
            )
            .map_err(|error| map_execution_error(&error))?;
        self.completed.remove(invocation_id);
        self.completed_requests.remove(invocation_id);
        Ok(ControlResult::Mutation(receipt))
    }

    fn poll_owned(&mut self, now: u64) -> Result<(), DaemonError> {
        if self.completed.len() >= MAX_RETAINED_COMPLETIONS {
            return Ok(());
        }
        let identifiers = self.running.keys().cloned().collect::<Vec<_>>();
        for invocation_id in identifiers {
            if self.completed.len() >= MAX_RETAINED_COMPLETIONS {
                break;
            }
            let observation = self
                .running
                .get_mut(&invocation_id)
                .ok_or(DaemonError::Unavailable)?
                .process
                .poll();
            match observation {
                Ok(None) => {}
                Ok(Some(exit)) => {
                    let owned = self
                        .running
                        .remove(&invocation_id)
                        .ok_or(DaemonError::Unavailable)?;
                    let resolution = if exit.success {
                        InvocationResolution::Completed
                    } else {
                        InvocationResolution::Failed
                    };
                    self.completed.insert(
                        invocation_id.clone(),
                        InvocationProcessResult {
                            invocation_id: invocation_id.clone(),
                            swarm_id: owned.swarm_id,
                            resolution,
                            exit: Some(exit),
                        },
                    );
                    self.completed_requests
                        .insert(invocation_id, owned.request_digest);
                }
                Err(_) => {
                    let owned = self
                        .running
                        .remove(&invocation_id)
                        .ok_or(DaemonError::Unavailable)?;
                    self.supervisor.command(
                        ExecutionCommand::ResolveInvocation {
                            swarm_id: owned.swarm_id.clone(),
                            invocation_id: invocation_id.clone(),
                            resolution: InvocationResolution::Unknown,
                        },
                        now,
                    )?;
                    self.completed.insert(
                        invocation_id.clone(),
                        InvocationProcessResult {
                            invocation_id: invocation_id.clone(),
                            swarm_id: owned.swarm_id,
                            resolution: InvocationResolution::Unknown,
                            exit: None,
                        },
                    );
                    self.completed_requests
                        .insert(invocation_id, owned.request_digest);
                }
            }
        }
        Ok(())
    }

    fn stop_swarm(&mut self, swarm_id: &str, now: u64) -> Result<ControlResult, ControlFault> {
        let mut receipt = self
            .supervisor
            .command(
                ExecutionCommand::Stop {
                    swarm_id: swarm_id.to_owned(),
                },
                now,
            )
            .map_err(|error| map_execution_error(&error))?;
        for invocation_id in receipt.stop_invocations.clone() {
            let resolution = if let Some(mut owned) = self.running.remove(&invocation_id) {
                match owned.process.stop(STOP_GRACE) {
                    Ok(_) => InvocationResolution::Terminated,
                    Err(_) => InvocationResolution::Unknown,
                }
            } else if let Some(result) = self.completed.remove(&invocation_id) {
                self.completed_requests.remove(&invocation_id);
                result.resolution
            } else {
                // The ticket was admitted but never launched by this sole
                // daemon owner, so no process tree exists to terminate.
                InvocationResolution::Terminated
            };
            receipt = self
                .supervisor
                .command(
                    ExecutionCommand::ResolveInvocation {
                        swarm_id: swarm_id.to_owned(),
                        invocation_id,
                        resolution,
                    },
                    now,
                )
                .map_err(|error| map_execution_error(&error))?;
        }
        Ok(ControlResult::Mutation(receipt))
    }

    fn cancel_invocation(
        &mut self,
        swarm_id: &str,
        invocation_id: &str,
        now: u64,
    ) -> Result<ControlResult, ControlFault> {
        if let Some(result) = self.completed.get(invocation_id) {
            return if result.swarm_id == swarm_id {
                Err(ControlFault::Conflict)
            } else {
                Err(ControlFault::NotFound)
            };
        }
        let mut receipt = self
            .supervisor
            .command(
                ExecutionCommand::CancelInvocation {
                    swarm_id: swarm_id.to_owned(),
                    invocation_id: invocation_id.to_owned(),
                },
                now,
            )
            .map_err(|error| map_execution_error(&error))?;
        if receipt.stop_invocations.is_empty() {
            return Ok(ControlResult::Mutation(receipt));
        }
        if receipt.stop_invocations.as_slice() != [invocation_id] {
            return Err(ControlFault::Unavailable);
        }
        let resolution = if let Some(mut owned) = self.running.remove(invocation_id) {
            if owned.swarm_id != swarm_id {
                return Err(ControlFault::Conflict);
            }
            match owned.process.stop(STOP_GRACE) {
                Ok(_) => InvocationResolution::Terminated,
                Err(_) => InvocationResolution::Unknown,
            }
        } else {
            // This sole daemon admitted but never launched the exact ticket.
            InvocationResolution::Terminated
        };
        receipt = self
            .supervisor
            .command(
                ExecutionCommand::ResolveInvocation {
                    swarm_id: swarm_id.to_owned(),
                    invocation_id: invocation_id.to_owned(),
                    resolution,
                },
                now,
            )
            .map_err(|error| map_execution_error(&error))?;
        Ok(ControlResult::Mutation(receipt))
    }
}

/// Re-openable client. It reads the current protected publication for every
/// request, so a daemon restart cannot silently reuse stale authority.
pub struct ExecutionControlClient {
    root: PathBuf,
    timeout: Duration,
}

impl ExecutionControlClient {
    /// Opens an existing owner-only daemon state directory without starting a
    /// daemon or mutating execution state.
    ///
    /// # Errors
    /// Rejects unsafe storage and unreasonable timeouts.
    pub fn open(root: &Path, timeout: Duration) -> Result<Self, DaemonError> {
        if timeout.is_zero() || timeout > Duration::from_mins(1) {
            return Err(DaemonError::InvalidRequest);
        }
        let root = validate_data_directory(root).map_err(|_| DaemonError::UnsafeStorage)?;
        Ok(Self { root, timeout })
    }

    /// Sends one command to the exact currently published generation.
    ///
    /// # Errors
    /// Connection loss, malformed/truncated responses, stale publications, and
    /// daemon faults are returned without retrying against another endpoint.
    pub fn request(&self, command: ControlCommand) -> Result<ControlResult, DaemonError> {
        let record = read_endpoint(&self.root)?;
        let mut stream = TcpStream::connect_timeout(&record.endpoint, self.timeout)
            .map_err(|_| DaemonError::Unavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| DaemonError::Unavailable)?;
        write_frame(
            &mut stream,
            &WireRequest {
                schema: REQUEST_SCHEMA.to_owned(),
                generation: record.generation,
                command,
            },
            MAX_REQUEST_BYTES,
        )?;
        let response: WireResponse = read_frame(&mut stream, MAX_RESPONSE_BYTES)?;
        if response.schema != RESPONSE_SCHEMA {
            return Err(DaemonError::InvalidResponse);
        }
        match (response.result, response.fault) {
            (Some(result), None) => Ok(result),
            (None, Some(fault)) => Err(DaemonError::Control(fault)),
            _ => Err(DaemonError::InvalidResponse),
        }
    }
}

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("the execution daemon request is invalid")]
    InvalidRequest,
    #[error("the execution daemon response is invalid")]
    InvalidResponse,
    #[error("execution daemon storage is unsafe")]
    UnsafeStorage,
    #[error("the execution daemon is unavailable")]
    Unavailable,
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    #[error(transparent)]
    Process(#[from] ProcessError),
    #[error(transparent)]
    Control(#[from] ControlFault),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct EndpointRecord {
    schema: String,
    endpoint: SocketAddr,
    process_id: u32,
    generation: String,
}

struct EndpointPublication {
    path: PathBuf,
    record: EndpointRecord,
}

impl EndpointPublication {
    fn publish(root: &Path, endpoint: SocketAddr) -> Result<Self, DaemonError> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| DaemonError::Unavailable)?;
        let generation = random.iter().fold(String::new(), |mut value, byte| {
            let _ = write!(value, "{byte:02x}");
            value
        });
        let record = EndpointRecord {
            schema: ENDPOINT_SCHEMA.to_owned(),
            endpoint,
            process_id: std::process::id(),
            generation,
        };
        validate_endpoint(&record)?;
        let path = root.join(ENDPOINT_FILE);
        if path.exists() {
            validate_owner_only_file(&path).map_err(|_| DaemonError::UnsafeStorage)?;
        }
        let mut suffix = [0_u8; 12];
        getrandom::fill(&mut suffix).map_err(|_| DaemonError::Unavailable)?;
        let suffix = suffix.iter().fold(String::new(), |mut value, byte| {
            let _ = write!(value, "{byte:02x}");
            value
        });
        let staged = root.join(format!(".{suffix}.execution-control.tmp"));
        let bytes = serde_json::to_vec_pretty(&record).map_err(|_| DaemonError::Unavailable)?;
        let mut file =
            create_owner_only_renameable_file(&staged).map_err(|_| DaemonError::Unavailable)?;
        let result = (|| {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| DaemonError::Unavailable)?;
            drop(file);
            if path.exists() {
                atomicwrites::replace_atomic(&staged, &path)
            } else {
                atomicwrites::move_atomic(&staged, &path)
            }
            .map_err(|_| DaemonError::Unavailable)
        })();
        let _ = fs::remove_file(staged);
        result?;
        Ok(Self { path, record })
    }
}

impl Drop for EndpointPublication {
    fn drop(&mut self) {
        let Ok(current) = read_endpoint_path(&self.path) else {
            return;
        };
        if constant_time_equal(
            current.generation.as_bytes(),
            self.record.generation.as_bytes(),
        ) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    schema: String,
    generation: String,
    command: ControlCommand,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WireResponse {
    schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<ControlResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fault: Option<ControlFault>,
}

impl WireResponse {
    fn success(result: ControlResult) -> Self {
        Self {
            schema: RESPONSE_SCHEMA.to_owned(),
            result: Some(result),
            fault: None,
        }
    }

    fn fault(fault: ControlFault) -> Self {
        Self {
            schema: RESPONSE_SCHEMA.to_owned(),
            result: None,
            fault: Some(fault),
        }
    }
}

fn read_endpoint(root: &Path) -> Result<EndpointRecord, DaemonError> {
    read_endpoint_path(&root.join(ENDPOINT_FILE))
}

fn read_endpoint_path(path: &Path) -> Result<EndpointRecord, DaemonError> {
    validate_owner_only_file(path).map_err(|_| DaemonError::UnsafeStorage)?;
    let metadata = fs::metadata(path).map_err(|_| DaemonError::Unavailable)?;
    if metadata.len() > MAX_ENDPOINT_BYTES {
        return Err(DaemonError::InvalidResponse);
    }
    let record: EndpointRecord =
        serde_json::from_slice(&fs::read(path).map_err(|_| DaemonError::Unavailable)?)
            .map_err(|_| DaemonError::InvalidResponse)?;
    validate_endpoint(&record)?;
    Ok(record)
}

fn validate_endpoint(record: &EndpointRecord) -> Result<(), DaemonError> {
    if record.schema != ENDPOINT_SCHEMA
        || !record.endpoint.ip().is_loopback()
        || record.endpoint.port() == 0
        || record.process_id == 0
        || record.generation.len() != 64
        || !record
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DaemonError::InvalidResponse);
    }
    Ok(())
}

fn read_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut TcpStream,
    maximum: u64,
) -> Result<T, DaemonError> {
    let mut bytes = Vec::new();
    let mut bounded = BufReader::new(stream).take(maximum.saturating_add(1));
    let count = bounded
        .read_until(b'\n', &mut bytes)
        .map_err(|_| DaemonError::Unavailable)?;
    if count == 0
        || u64::try_from(count).unwrap_or(u64::MAX) > maximum
        || bytes.last() != Some(&b'\n')
    {
        return Err(DaemonError::InvalidResponse);
    }
    bytes.pop();
    serde_json::from_slice(&bytes).map_err(|_| DaemonError::InvalidResponse)
}

fn write_frame<T: Serialize>(
    stream: &mut TcpStream,
    value: &T,
    maximum: u64,
) -> Result<(), DaemonError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| DaemonError::Unavailable)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) >= maximum {
        return Err(DaemonError::Unavailable);
    }
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .and_then(|()| stream.flush())
        .map_err(|_| DaemonError::Unavailable)
}

fn now_ms() -> Result<u64, DaemonError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| DaemonError::Unavailable)?
        .as_millis();
    u64::try_from(millis).map_err(|_| DaemonError::Unavailable)
}

fn prepared_request_digest(prepared: &PreparedInvocation) -> Result<String, ControlFault> {
    let bytes = serde_json::to_vec(prepared).map_err(|_| ControlFault::InvalidRequest)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn map_execution_error(error: &ExecutionError) -> ControlFault {
    match error {
        ExecutionError::InvalidCommand | ExecutionError::InvalidState => {
            ControlFault::InvalidRequest
        }
        ExecutionError::NotFound => ControlFault::NotFound,
        ExecutionError::Conflict => ControlFault::Conflict,
        ExecutionError::RecoveryRequired => ControlFault::RecoveryRequired,
        ExecutionError::BudgetExhausted => ControlFault::BudgetExhausted,
        ExecutionError::Journal(_) | ExecutionError::Recovery(_) => ControlFault::Unavailable,
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(left.get(index).copied().unwrap_or_default())
            ^ usize::from(right.get(index).copied().unwrap_or_default());
    }
    difference == 0
}
