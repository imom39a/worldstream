//! Durable execution/control state machine.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    journal::{ExecutionJournal, JournalError},
    provider::ProviderKind,
    recovery::{EffectIntent, EffectOutcome, EffectRecord, EffectState, RecoveryError},
    scheduler::{InvocationTicket, ProviderCapacity, ScheduleDecision, SchedulerState},
};

const LEGACY_STATE_SCHEMA: &str = "worldstream/agent-swarm-execution@1";
const STATE_SCHEMA: &str = "worldstream/agent-swarm-execution@2";
const MAX_ID_BYTES: usize = 256;
const MAX_PRIORITY: u8 = 16;
const MAX_ROSTER_MEMBERS: u16 = 16;
const MAX_RETAINED_CANCELLATIONS: usize = 1024;

/// Durable desired execution, separate from authoritative Room status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredExecution {
    Running,
    Paused,
    Stopped,
}

/// Visible operational phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Running,
    Pausing,
    Paused,
    Stopping,
    Stopped,
    RecoveryRequired,
    BlockedUnknown,
}

/// Optional application budget. Time is cumulative wall time while at least
/// one Invocation for the Swarm is active; concurrent turns do not multiply it.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunBudget {
    pub invocation_limit: Option<u64>,
    pub active_time_limit_ms: Option<u64>,
}

impl RunBudget {
    fn valid(self) -> bool {
        self.invocation_limit.is_none_or(|limit| limit > 0)
            && self.active_time_limit_ms.is_none_or(|limit| limit > 0)
    }
}

/// Current durable Invocation association.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveInvocation {
    pub ticket: InvocationTicket,
    pub execution_epoch: u64,
    pub started_at_ms: u64,
    pub resolution: InvocationResolution,
}

/// Supervisor knowledge about a previously admitted Invocation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationResolution {
    Running,
    Completed,
    Failed,
    Terminated,
    Unknown,
}

/// Commands accepted by the execution state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionCommand {
    RegisterSwarm {
        swarm_id: String,
        priority: u8,
        budget: RunBudget,
        /// Exact selected roster counts at registration. An empty map is
        /// retained for backwards-compatible callers and grants no capacity.
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
    Enqueue(InvocationTicket),
    Pause {
        swarm_id: String,
    },
    Stop {
        swarm_id: String,
    },
    CancelInvocation {
        swarm_id: String,
        invocation_id: String,
    },
    Resume {
        swarm_id: String,
    },
    ResolveInvocation {
        swarm_id: String,
        invocation_id: String,
        resolution: InvocationResolution,
    },
    PrepareEffect(EffectIntent),
    MarkEffectDispatched {
        operation_id: String,
    },
    ObserveEffect {
        operation_id: String,
        outcome: EffectOutcome,
    },
    AcknowledgeEffect {
        operation_id: String,
    },
}

/// Durable command receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReceipt {
    pub revision: u64,
    pub phase: Option<ExecutionPhase>,
    /// Exact active Invocation identities this command must terminate and
    /// reconcile before its requested mutation is complete.
    pub stop_invocations: Vec<String>,
}

/// Read model for one Swarm.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmExecutionSnapshot {
    pub swarm_id: String,
    pub desired: DesiredExecution,
    pub phase: ExecutionPhase,
    pub execution_epoch: u64,
    pub priority: u8,
    pub budget: RunBudget,
    pub invocations_started: u64,
    pub active_time_ms: u64,
    pub queued: Vec<InvocationTicket>,
    pub active: Vec<ActiveInvocation>,
    pub unknown_effects: Vec<String>,
}

/// Whole-daemon operational read model. It contains no Room projection and no
/// provider transcript masquerading as accepted facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSnapshot {
    pub revision: u64,
    pub provider_caps: BTreeMap<ProviderKind, u16>,
    pub swarms: Vec<SwarmExecutionSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[doc(hidden)]
pub struct PersistedExecution {
    schema: String,
    revision: u64,
    clean_shutdown: bool,
    /// Durable human overrides. The serialized name is retained so existing
    /// `@1` journals keep their digest and migrate as manual policy.
    #[serde(rename = "provider_caps")]
    manual_provider_caps: BTreeMap<ProviderKind, u16>,
    scheduler: SchedulerState,
    swarms: BTreeMap<String, SwarmState>,
    queued: Vec<InvocationTicket>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    cancellations: BTreeMap<String, CancellationRecord>,
    effects: BTreeMap<String, EffectRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CancellationOutcome {
    Pending,
    Terminated,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CancellationRecord {
    swarm_id: String,
    outcome: CancellationOutcome,
    requested_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SwarmState {
    desired: DesiredExecution,
    phase: ExecutionPhase,
    execution_epoch: u64,
    priority: u8,
    budget: RunBudget,
    invocations_started: u64,
    active_time_ms: u64,
    budget_segment_started_ms: Option<u64>,
    active: BTreeMap<String, ActiveInvocation>,
    /// The authoritative roster-derived registration for this Swarm. Empty is
    /// omitted so journals written before roster capacity remain digest-valid.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    roster_provider_counts: BTreeMap<ProviderKind, u16>,
}

impl Default for PersistedExecution {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA.to_owned(),
            revision: 0,
            clean_shutdown: true,
            manual_provider_caps: BTreeMap::new(),
            scheduler: SchedulerState::default(),
            swarms: BTreeMap::new(),
            queued: Vec::new(),
            cancellations: BTreeMap::new(),
            effects: BTreeMap::new(),
        }
    }
}

/// Single-writer execution supervisor. A TUI client may detach without calling
/// `close_cleanly`; the separate daemon retains this object and keeps running.
pub struct ExecutionSupervisor<J> {
    journal: J,
    state: PersistedExecution,
}

impl<J: ExecutionJournal> ExecutionSupervisor<J> {
    /// Opens durable state and enters recovery rather than launching work after
    /// an unclean process exit.
    ///
    /// # Errors
    /// Fails closed for corrupt state or unavailable durability.
    pub fn open(journal: J, now_ms: u64) -> Result<Self, ExecutionError> {
        let mut state = journal.load()?.unwrap_or_default();
        migrate_state(&mut state)?;
        validate_state(&state)?;
        if !state.clean_shutdown {
            recover_after_crash(&mut state, now_ms);
        }
        state.clean_shutdown = false;
        state.revision = state.revision.saturating_add(1);
        journal.store(&state)?;
        Ok(Self { journal, state })
    }

    /// Applies and durably records one control mutation.
    ///
    /// # Errors
    /// Invalid phases, stale identities and journal failures leave prior state
    /// unchanged.
    pub fn command(
        &mut self,
        command: ExecutionCommand,
        now_ms: u64,
    ) -> Result<ExecutionReceipt, ExecutionError> {
        let mut next = self.state.clone();
        tick_budgets(&mut next, now_ms);
        let (phase_swarm, stop_invocations) = apply_command(&mut next, command, now_ms)?;
        next.revision = next.revision.saturating_add(1);
        self.journal.store(&next)?;
        self.state = next;
        Ok(ExecutionReceipt {
            revision: self.state.revision,
            phase: phase_swarm
                .as_deref()
                .and_then(|swarm_id| self.state.swarms.get(swarm_id))
                .map(|swarm| swarm.phase),
            stop_invocations,
        })
    }

    /// Reserves all currently available provider slots and Invocation budgets.
    /// The caller then refreshes Room context and starts only these exact
    /// tickets. Reservation is journaled before this method returns.
    ///
    /// # Errors
    /// Journal failure returns no dispatchable decision.
    pub fn schedule(&mut self, now_ms: u64) -> Result<Vec<ScheduleDecision>, ExecutionError> {
        let mut next = self.state.clone();
        tick_budgets(&mut next, now_ms);
        let priorities = next
            .swarms
            .iter()
            .map(|(id, swarm)| (id.clone(), swarm.priority))
            .collect::<BTreeMap<_, _>>();
        let blocked = next
            .swarms
            .iter()
            .filter(|(_, swarm)| swarm.phase != ExecutionPhase::Running)
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        let active = next
            .swarms
            .values()
            .flat_map(|swarm| swarm.active.values().map(|active| active.ticket.clone()))
            .collect::<Vec<_>>();
        let capacities = effective_provider_caps(&next)
            .into_iter()
            .map(|(provider, limit)| (provider, ProviderCapacity { limit, active: 0 }))
            .collect::<BTreeMap<_, _>>();
        let candidates = next
            .queued
            .iter()
            .filter(|ticket| budget_allows(&next, ticket, now_ms))
            .cloned()
            .collect::<Vec<_>>();
        let proposed =
            next.scheduler
                .schedule(&candidates, &active, &capacities, &priorities, &blocked);
        let mut remaining_by_swarm = next
            .swarms
            .iter()
            .map(|(id, swarm)| (id.clone(), remaining_invocations(swarm)))
            .collect::<BTreeMap<_, _>>();
        let mut admitted = Vec::new();
        for decision in proposed {
            let remaining = remaining_by_swarm
                .get_mut(&decision.ticket.swarm_id)
                .ok_or(ExecutionError::InvalidState)?;
            if *remaining == Some(0) {
                continue;
            }
            if let Some(value) = remaining {
                *value = value.saturating_sub(1);
            }
            admit(&mut next, &decision.ticket, now_ms)?;
            admitted.push(decision);
        }
        if admitted.is_empty() && next == self.state {
            return Ok(Vec::new());
        }
        next.revision = next.revision.saturating_add(1);
        self.journal.store(&next)?;
        self.state = next;
        Ok(admitted)
    }

    #[must_use]
    pub fn snapshot(&self, now_ms: u64) -> ExecutionSnapshot {
        let swarms = self
            .state
            .swarms
            .iter()
            .map(|(swarm_id, swarm)| SwarmExecutionSnapshot {
                swarm_id: swarm_id.clone(),
                desired: swarm.desired,
                phase: swarm.phase,
                execution_epoch: swarm.execution_epoch,
                priority: swarm.priority,
                budget: swarm.budget,
                invocations_started: swarm.invocations_started,
                active_time_ms: elapsed_time(swarm, now_ms),
                queued: self
                    .state
                    .queued
                    .iter()
                    .filter(|ticket| &ticket.swarm_id == swarm_id)
                    .cloned()
                    .collect(),
                active: swarm.active.values().cloned().collect(),
                unknown_effects: self
                    .state
                    .effects
                    .values()
                    .filter(|effect| {
                        effect.intent.swarm_id == *swarm_id
                            && matches!(
                                effect.state,
                                EffectState::Unknown | EffectState::Reconciling
                            )
                    })
                    .map(|effect| effect.intent.operation_id.clone())
                    .collect(),
            })
            .collect();
        ExecutionSnapshot {
            revision: self.state.revision,
            provider_caps: effective_provider_caps(&self.state),
            swarms,
        }
    }

    /// Records an intentional daemon shutdown. It is valid only after all
    /// owned process trees are terminal.
    ///
    /// # Errors
    /// Refuses to erase recovery evidence for active/unknown work.
    pub fn close_cleanly(mut self) -> Result<(), ExecutionError> {
        if self
            .state
            .swarms
            .values()
            .any(|swarm| !swarm.active.is_empty())
            || self.state.effects.values().any(|effect| {
                matches!(
                    effect.state,
                    EffectState::Dispatched | EffectState::Unknown | EffectState::Reconciling
                )
            })
        {
            return Err(ExecutionError::RecoveryRequired);
        }
        self.state.clean_shutdown = true;
        self.state.revision = self.state.revision.saturating_add(1);
        self.journal.store(&self.state)?;
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
fn apply_command(
    state: &mut PersistedExecution,
    command: ExecutionCommand,
    now_ms: u64,
) -> Result<(Option<String>, Vec<String>), ExecutionError> {
    match command {
        ExecutionCommand::RegisterSwarm {
            swarm_id,
            priority,
            budget,
            roster_provider_counts,
        } => {
            validate_id(&swarm_id)?;
            validate_policy(priority, budget)?;
            validate_roster_provider_counts(&roster_provider_counts)?;
            if state.swarms.contains_key(&swarm_id) {
                return Err(ExecutionError::Conflict);
            }
            state.swarms.insert(
                swarm_id.clone(),
                SwarmState {
                    desired: DesiredExecution::Stopped,
                    phase: ExecutionPhase::Stopped,
                    execution_epoch: 0,
                    priority,
                    budget,
                    invocations_started: 0,
                    active_time_ms: 0,
                    budget_segment_started_ms: None,
                    active: BTreeMap::new(),
                    roster_provider_counts,
                },
            );
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::SetProviderCap { provider, limit } => {
            state.manual_provider_caps.insert(provider, limit);
            Ok((None, Vec::new()))
        }
        ExecutionCommand::SetPriority { swarm_id, priority } => {
            if !(1..=MAX_PRIORITY).contains(&priority) {
                return Err(ExecutionError::InvalidCommand);
            }
            state
                .swarms
                .get_mut(&swarm_id)
                .ok_or(ExecutionError::NotFound)?
                .priority = priority;
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::SetBudget { swarm_id, budget } => {
            if !budget.valid() {
                return Err(ExecutionError::InvalidCommand);
            }
            state
                .swarms
                .get_mut(&swarm_id)
                .ok_or(ExecutionError::NotFound)?
                .budget = budget;
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::Enqueue(ticket) => {
            validate_ticket(&ticket)?;
            if !state.swarms.contains_key(&ticket.swarm_id) {
                return Err(ExecutionError::NotFound);
            }
            if state
                .queued
                .iter()
                .any(|item| item.invocation_id == ticket.invocation_id)
                || state
                    .swarms
                    .values()
                    .any(|swarm| swarm.active.contains_key(&ticket.invocation_id))
                || state.cancellations.contains_key(&ticket.invocation_id)
            {
                return Err(ExecutionError::Conflict);
            }
            let swarm_id = ticket.swarm_id.clone();
            state.queued.push(ticket);
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::Pause { swarm_id } => {
            let swarm = state
                .swarms
                .get_mut(&swarm_id)
                .ok_or(ExecutionError::NotFound)?;
            swarm.desired = DesiredExecution::Paused;
            swarm.phase = if swarm.active.is_empty() {
                close_budget_segment(swarm, now_ms);
                ExecutionPhase::Paused
            } else {
                ExecutionPhase::Pausing
            };
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::Stop { swarm_id } => {
            let swarm = state
                .swarms
                .get_mut(&swarm_id)
                .ok_or(ExecutionError::NotFound)?;
            swarm.desired = DesiredExecution::Stopped;
            let targets = swarm.active.keys().cloned().collect::<Vec<_>>();
            swarm.phase = if targets.is_empty() {
                close_budget_segment(swarm, now_ms);
                ExecutionPhase::Stopped
            } else {
                ExecutionPhase::Stopping
            };
            Ok((Some(swarm_id), targets))
        }
        ExecutionCommand::CancelInvocation {
            swarm_id,
            invocation_id,
        } => {
            validate_id(&swarm_id)?;
            validate_id(&invocation_id)?;
            let swarm = state
                .swarms
                .get(&swarm_id)
                .ok_or(ExecutionError::NotFound)?;
            if let Some(record) = state.cancellations.get(&invocation_id) {
                if record.swarm_id != swarm_id {
                    return Err(ExecutionError::Conflict);
                }
                return match record.outcome {
                    CancellationOutcome::Pending => {
                        let active = swarm
                            .active
                            .get(&invocation_id)
                            .ok_or(ExecutionError::InvalidState)?;
                        if active.resolution == InvocationResolution::Running {
                            Ok((Some(swarm_id), vec![invocation_id]))
                        } else {
                            Err(ExecutionError::RecoveryRequired)
                        }
                    }
                    CancellationOutcome::Terminated | CancellationOutcome::Unknown => {
                        Ok((Some(swarm_id), Vec::new()))
                    }
                };
            }
            let active = swarm
                .active
                .get(&invocation_id)
                .ok_or(ExecutionError::NotFound)?;
            if active.resolution != InvocationResolution::Running {
                return Err(ExecutionError::RecoveryRequired);
            }
            retain_cancellation_capacity(state)?;
            state.cancellations.insert(
                invocation_id.clone(),
                CancellationRecord {
                    swarm_id: swarm_id.clone(),
                    outcome: CancellationOutcome::Pending,
                    requested_revision: state.revision.saturating_add(1),
                },
            );
            Ok((Some(swarm_id), vec![invocation_id]))
        }
        ExecutionCommand::Resume { swarm_id } => {
            let unresolved_effect = state.effects.values().any(|effect| {
                effect.intent.swarm_id == swarm_id
                    && matches!(
                        effect.state,
                        EffectState::Unknown | EffectState::Reconciling
                    )
            });
            let swarm = state
                .swarms
                .get_mut(&swarm_id)
                .ok_or(ExecutionError::NotFound)?;
            if unresolved_effect
                || swarm
                    .active
                    .values()
                    .any(|active| active.resolution == InvocationResolution::Unknown)
            {
                return Err(ExecutionError::RecoveryRequired);
            }
            if budget_exhausted(swarm, now_ms) {
                return Err(ExecutionError::BudgetExhausted);
            }
            swarm.execution_epoch = swarm.execution_epoch.saturating_add(1);
            swarm.desired = DesiredExecution::Running;
            swarm.phase = ExecutionPhase::Running;
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::ResolveInvocation {
            swarm_id,
            invocation_id,
            resolution,
        } => {
            if resolution == InvocationResolution::Running {
                return Err(ExecutionError::InvalidCommand);
            }
            let cancellation_recorded = match state.cancellations.get(&invocation_id) {
                Some(record) if record.swarm_id != swarm_id => {
                    return Err(ExecutionError::Conflict);
                }
                Some(record)
                    if matches!(
                        record.outcome,
                        CancellationOutcome::Pending | CancellationOutcome::Unknown
                    ) =>
                {
                    true
                }
                Some(_) => return Err(ExecutionError::Conflict),
                None => false,
            };
            if cancellation_recorded
                && !matches!(
                    resolution,
                    InvocationResolution::Terminated | InvocationResolution::Unknown
                )
            {
                return Err(ExecutionError::Conflict);
            }
            let swarm = state
                .swarms
                .get_mut(&swarm_id)
                .ok_or(ExecutionError::NotFound)?;
            let active = swarm
                .active
                .get_mut(&invocation_id)
                .ok_or(ExecutionError::NotFound)?;
            if resolution == InvocationResolution::Unknown {
                active.resolution = resolution;
                swarm.phase = ExecutionPhase::BlockedUnknown;
            } else {
                swarm.active.remove(&invocation_id);
                settle_phase(swarm, now_ms);
            }
            if cancellation_recorded {
                let record = state
                    .cancellations
                    .get_mut(&invocation_id)
                    .ok_or(ExecutionError::InvalidState)?;
                record.outcome = match resolution {
                    InvocationResolution::Terminated => CancellationOutcome::Terminated,
                    InvocationResolution::Unknown => CancellationOutcome::Unknown,
                    InvocationResolution::Running
                    | InvocationResolution::Completed
                    | InvocationResolution::Failed => return Err(ExecutionError::InvalidState),
                };
            }
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::PrepareEffect(intent) => {
            let operation_id = intent.operation_id.clone();
            let swarm_id = intent.swarm_id.clone();
            if !state.swarms.contains_key(&swarm_id) {
                return Err(ExecutionError::NotFound);
            }
            let record = EffectRecord::prepare(intent)?;
            if let Some(existing) = state.effects.get(&operation_id) {
                if existing == &record {
                    return Ok((Some(swarm_id), Vec::new()));
                }
                return Err(ExecutionError::Conflict);
            }
            state.effects.insert(operation_id, record);
            Ok((Some(swarm_id), Vec::new()))
        }
        ExecutionCommand::MarkEffectDispatched { operation_id } => {
            let record = state
                .effects
                .get_mut(&operation_id)
                .ok_or(ExecutionError::NotFound)?;
            record.mark_dispatched()?;
            Ok((Some(record.intent.swarm_id.clone()), Vec::new()))
        }
        ExecutionCommand::ObserveEffect {
            operation_id,
            outcome,
        } => {
            let record = state
                .effects
                .get_mut(&operation_id)
                .ok_or(ExecutionError::NotFound)?;
            record.observe(outcome)?;
            if record.state == EffectState::Unknown
                && let Some(swarm) = state.swarms.get_mut(&record.intent.swarm_id)
            {
                swarm.phase = ExecutionPhase::BlockedUnknown;
            }
            Ok((Some(record.intent.swarm_id.clone()), Vec::new()))
        }
        ExecutionCommand::AcknowledgeEffect { operation_id } => {
            let record = state
                .effects
                .get_mut(&operation_id)
                .ok_or(ExecutionError::NotFound)?;
            record.acknowledge()?;
            Ok((Some(record.intent.swarm_id.clone()), Vec::new()))
        }
    }
}

fn admit(
    state: &mut PersistedExecution,
    ticket: &InvocationTicket,
    now_ms: u64,
) -> Result<(), ExecutionError> {
    let swarm = state
        .swarms
        .get_mut(&ticket.swarm_id)
        .ok_or(ExecutionError::NotFound)?;
    if swarm.phase != ExecutionPhase::Running || budget_exhausted(swarm, now_ms) {
        return Err(ExecutionError::BudgetExhausted);
    }
    let position = state
        .queued
        .iter()
        .position(|item| item.invocation_id == ticket.invocation_id)
        .ok_or(ExecutionError::NotFound)?;
    let ticket = state.queued.remove(position);
    if swarm.active.is_empty() {
        swarm.budget_segment_started_ms = Some(now_ms);
    }
    swarm.invocations_started = swarm.invocations_started.saturating_add(1);
    swarm.active.insert(
        ticket.invocation_id.clone(),
        ActiveInvocation {
            ticket,
            execution_epoch: swarm.execution_epoch,
            started_at_ms: now_ms,
            resolution: InvocationResolution::Running,
        },
    );
    if remaining_invocations(swarm) == Some(0) {
        swarm.desired = DesiredExecution::Paused;
        swarm.phase = ExecutionPhase::Pausing;
    }
    Ok(())
}

fn recover_after_crash(state: &mut PersistedExecution, now_ms: u64) {
    for effect in state.effects.values_mut() {
        effect.restore_after_crash();
    }
    for (swarm_id, swarm) in &mut state.swarms {
        close_budget_segment(swarm, now_ms);
        for active in swarm.active.values_mut() {
            active.resolution = InvocationResolution::Unknown;
        }
        let unknown_effect = state.effects.values().any(|effect| {
            effect.intent.swarm_id == *swarm_id
                && matches!(
                    effect.state,
                    EffectState::Unknown | EffectState::Reconciling
                )
        });
        swarm.phase = if unknown_effect {
            ExecutionPhase::BlockedUnknown
        } else if !swarm.active.is_empty() || swarm.desired == DesiredExecution::Running {
            ExecutionPhase::RecoveryRequired
        } else {
            match swarm.desired {
                DesiredExecution::Running => ExecutionPhase::RecoveryRequired,
                DesiredExecution::Paused => ExecutionPhase::Paused,
                DesiredExecution::Stopped => ExecutionPhase::Stopped,
            }
        };
    }
    for record in state.cancellations.values_mut() {
        if record.outcome == CancellationOutcome::Pending {
            record.outcome = CancellationOutcome::Unknown;
        }
    }
}

fn tick_budgets(state: &mut PersistedExecution, now_ms: u64) {
    for swarm in state.swarms.values_mut() {
        if swarm.phase == ExecutionPhase::Running && budget_exhausted(swarm, now_ms) {
            swarm.desired = DesiredExecution::Paused;
            swarm.phase = if swarm.active.is_empty() {
                close_budget_segment(swarm, now_ms);
                ExecutionPhase::Paused
            } else {
                ExecutionPhase::Pausing
            };
        }
    }
}

fn settle_phase(swarm: &mut SwarmState, now_ms: u64) {
    if !swarm.active.is_empty() {
        return;
    }
    close_budget_segment(swarm, now_ms);
    swarm.phase = match swarm.desired {
        DesiredExecution::Running => {
            if swarm.phase == ExecutionPhase::RecoveryRequired {
                ExecutionPhase::RecoveryRequired
            } else {
                ExecutionPhase::Running
            }
        }
        DesiredExecution::Paused => ExecutionPhase::Paused,
        DesiredExecution::Stopped => ExecutionPhase::Stopped,
    };
}

fn close_budget_segment(swarm: &mut SwarmState, now_ms: u64) {
    if let Some(started) = swarm.budget_segment_started_ms.take() {
        swarm.active_time_ms = swarm
            .active_time_ms
            .saturating_add(now_ms.saturating_sub(started));
    }
}

fn elapsed_time(swarm: &SwarmState, now_ms: u64) -> u64 {
    swarm.active_time_ms.saturating_add(
        swarm
            .budget_segment_started_ms
            .map_or(0, |started| now_ms.saturating_sub(started)),
    )
}

fn remaining_invocations(swarm: &SwarmState) -> Option<u64> {
    swarm
        .budget
        .invocation_limit
        .map(|limit| limit.saturating_sub(swarm.invocations_started))
}

fn budget_exhausted(swarm: &SwarmState, now_ms: u64) -> bool {
    remaining_invocations(swarm) == Some(0)
        || swarm
            .budget
            .active_time_limit_ms
            .is_some_and(|limit| elapsed_time(swarm, now_ms) >= limit)
}

fn budget_allows(state: &PersistedExecution, ticket: &InvocationTicket, now_ms: u64) -> bool {
    state.swarms.get(&ticket.swarm_id).is_some_and(|swarm| {
        swarm.phase == ExecutionPhase::Running && !budget_exhausted(swarm, now_ms)
    })
}

/// Computes the one daemon-wide policy without turning independent Swarm
/// rosters into additive subscription concurrency. A roster registration sets
/// the automatic value to the largest selected count for that provider; one
/// durable human override replaces that value, including with zero.
fn effective_provider_caps(state: &PersistedExecution) -> BTreeMap<ProviderKind, u16> {
    let mut effective = BTreeMap::new();
    for swarm in state.swarms.values() {
        for (&provider, &count) in &swarm.roster_provider_counts {
            effective
                .entry(provider)
                .and_modify(|limit: &mut u16| *limit = (*limit).max(count))
                .or_insert(count);
        }
    }
    for (&provider, &limit) in &state.manual_provider_caps {
        effective.insert(provider, limit);
    }
    effective
}

fn validate_roster_provider_counts(
    counts: &BTreeMap<ProviderKind, u16>,
) -> Result<(), ExecutionError> {
    let mut total = 0_u16;
    for count in counts.values() {
        if *count == 0 || *count > MAX_ROSTER_MEMBERS {
            return Err(ExecutionError::InvalidCommand);
        }
        total = total
            .checked_add(*count)
            .ok_or(ExecutionError::InvalidCommand)?;
    }
    if total > MAX_ROSTER_MEMBERS {
        return Err(ExecutionError::InvalidCommand);
    }
    Ok(())
}

fn retain_cancellation_capacity(state: &mut PersistedExecution) -> Result<(), ExecutionError> {
    if state.cancellations.len() < MAX_RETAINED_CANCELLATIONS {
        return Ok(());
    }
    let oldest = state
        .cancellations
        .iter()
        .filter(|(_, record)| record.outcome == CancellationOutcome::Terminated)
        .min_by_key(|(_, record)| record.requested_revision)
        .map(|(invocation_id, _)| invocation_id.clone())
        .ok_or(ExecutionError::Conflict)?;
    state.cancellations.remove(&oldest);
    Ok(())
}

fn validate_state(state: &PersistedExecution) -> Result<(), ExecutionError> {
    if state.schema != STATE_SCHEMA {
        return Err(ExecutionError::InvalidState);
    }
    for (id, swarm) in &state.swarms {
        validate_id(id)?;
        validate_policy(swarm.priority, swarm.budget)?;
        validate_roster_provider_counts(&swarm.roster_provider_counts)
            .map_err(|_| ExecutionError::InvalidState)?;
        if swarm.execution_epoch == 0 && swarm.phase != ExecutionPhase::Stopped {
            return Err(ExecutionError::InvalidState);
        }
        for (invocation_id, active) in &swarm.active {
            if invocation_id != &active.ticket.invocation_id || active.ticket.swarm_id != *id {
                return Err(ExecutionError::InvalidState);
            }
            validate_ticket(&active.ticket)?;
        }
    }
    let mut invocations = BTreeSet::new();
    for ticket in &state.queued {
        validate_ticket(ticket)?;
        if !state.swarms.contains_key(&ticket.swarm_id)
            || !invocations.insert(ticket.invocation_id.as_str())
        {
            return Err(ExecutionError::InvalidState);
        }
    }
    for swarm in state.swarms.values() {
        for id in swarm.active.keys() {
            if !invocations.insert(id) {
                return Err(ExecutionError::InvalidState);
            }
        }
    }
    if state.cancellations.len() > MAX_RETAINED_CANCELLATIONS {
        return Err(ExecutionError::InvalidState);
    }
    for (invocation_id, record) in &state.cancellations {
        validate_id(invocation_id).map_err(|_| ExecutionError::InvalidState)?;
        validate_id(&record.swarm_id).map_err(|_| ExecutionError::InvalidState)?;
        let swarm = state
            .swarms
            .get(&record.swarm_id)
            .ok_or(ExecutionError::InvalidState)?;
        let active = swarm.active.get(invocation_id);
        let valid = match record.outcome {
            CancellationOutcome::Pending => {
                active.is_some_and(|active| active.resolution == InvocationResolution::Running)
            }
            CancellationOutcome::Terminated => {
                active.is_none() && !invocations.contains(invocation_id.as_str())
            }
            CancellationOutcome::Unknown => {
                active.is_some_and(|active| active.resolution == InvocationResolution::Unknown)
            }
        };
        if !valid {
            return Err(ExecutionError::InvalidState);
        }
    }
    for (operation_id, effect) in &state.effects {
        if operation_id != &effect.intent.operation_id
            || !state.swarms.contains_key(&effect.intent.swarm_id)
            || !effect.intent.valid()
        {
            return Err(ExecutionError::InvalidState);
        }
    }
    Ok(())
}

fn migrate_state(state: &mut PersistedExecution) -> Result<(), ExecutionError> {
    match state.schema.as_str() {
        STATE_SCHEMA => Ok(()),
        LEGACY_STATE_SCHEMA
            if state
                .swarms
                .values()
                .all(|swarm| swarm.roster_provider_counts.is_empty()) =>
        {
            // Every v1 provider cap came from SetProviderCap. Preserve those
            // values as manual overrides; absent roster counts remain closed.
            STATE_SCHEMA.clone_into(&mut state.schema);
            Ok(())
        }
        _ => Err(ExecutionError::InvalidState),
    }
}

fn validate_ticket(ticket: &InvocationTicket) -> Result<(), ExecutionError> {
    for value in [
        ticket.invocation_id.as_str(),
        ticket.swarm_id.as_str(),
        ticket.member_id.as_str(),
    ] {
        validate_id(value)?;
    }
    if ticket.configuration_revision == 0 || ticket.due_sequence == 0 {
        return Err(ExecutionError::InvalidCommand);
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), ExecutionError> {
    if value.is_empty() || value.len() > MAX_ID_BYTES || value.contains('\0') {
        Err(ExecutionError::InvalidCommand)
    } else {
        Ok(())
    }
}

fn validate_policy(priority: u8, budget: RunBudget) -> Result<(), ExecutionError> {
    if !(1..=MAX_PRIORITY).contains(&priority) || !budget.valid() {
        Err(ExecutionError::InvalidCommand)
    } else {
        Ok(())
    }
}

/// Closed state-machine failures.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("execution command is invalid")]
    InvalidCommand,
    #[error("execution state is invalid")]
    InvalidState,
    #[error("execution object was not found")]
    NotFound,
    #[error("execution identity conflicts with retained state")]
    Conflict,
    #[error("execution recovery is required")]
    RecoveryRequired,
    #[error("execution budget is exhausted")]
    BudgetExhausted,
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_one_state_migrates_existing_caps_as_manual_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut state = PersistedExecution::default();
        LEGACY_STATE_SCHEMA.clone_into(&mut state.schema);
        apply_command(
            &mut state,
            ExecutionCommand::RegisterSwarm {
                swarm_id: "legacy".to_owned(),
                priority: 1,
                budget: RunBudget::default(),
                roster_provider_counts: BTreeMap::new(),
            },
            0,
        )?;
        apply_command(
            &mut state,
            ExecutionCommand::SetProviderCap {
                provider: ProviderKind::Codex,
                limit: 2,
            },
            0,
        )?;

        let encoded = serde_json::to_string(&state)?;
        assert!(encoded.contains("\"schema\":\"worldstream/agent-swarm-execution@1\""));
        assert!(encoded.contains("\"provider_caps\""));
        assert!(!encoded.contains("roster_provider_counts"));
        let mut decoded: PersistedExecution = serde_json::from_str(&encoded)?;
        migrate_state(&mut decoded)?;

        assert_eq!(decoded.schema, STATE_SCHEMA);
        assert_eq!(
            effective_provider_caps(&decoded),
            BTreeMap::from([(ProviderKind::Codex, 2)])
        );
        Ok(())
    }
}
