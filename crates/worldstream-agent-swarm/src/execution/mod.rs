//! Local Agent Swarm execution supervision.
//!
//! This module owns provider command construction, bounded native process
//! trees, crash-safe operational state, and cross-Swarm admission. It does not
//! own Room facts or participant authority. A coordinator must refresh
//! authorized Room context before preparing and launching each scheduled
//! [`InvocationTicket`], then submit verified output through an exact Pack
//! Action before acknowledging the retained completion.

pub mod daemon;
pub mod journal;
pub mod local_codex;
pub mod process;
pub mod provider;
pub mod qualification;
pub mod recovery;
pub mod scheduler;
pub mod supervisor;

pub use daemon::{
    ControlCommand, ControlFault, ControlResult, DaemonError, DaemonPublication, DaemonTick,
    ExecutionControlClient, ExecutionDaemon, InvocationCompletion, InvocationLaunchReceipt,
    InvocationProcessResult,
};
pub use journal::{ExecutionJournal, FileExecutionJournal, JournalError, MemoryExecutionJournal};
pub use process::{NativeProcessSpawner, OwnedCommand, OwnedInvocation, ProcessError, ProcessExit};
pub use provider::{
    ACTION_PROPOSAL_SCHEMA, ActionProposal, ConfigurationResolution, EphemeralProviderFile,
    EphemeralProviderProfile, InvocationOutput, InvocationRequest, NativeProviderProbe,
    OutputContract, PreparedInvocation, ProposedArtifact, ProviderAdapter, ProviderBlocker,
    ProviderCapabilities, ProviderError, ProviderInspection, ProviderKind,
    ProviderQualificationBinding, ProviderRegistry, QualifiedSelection, ResourcePolicy,
    SessionSelection, decode_action_proposal,
};
pub use qualification::{
    InstalledProviderQualification, ProviderQualification, QualificationError,
};
pub use recovery::{
    EffectFence, EffectIntent, EffectOutcome, EffectProbe, EffectRecord, EffectState,
    RecoveryError, reconcile_effect,
};
pub use scheduler::{
    InvocationKind, InvocationTicket, ProviderCapacity, ScheduleDecision, SchedulerState,
};
pub use supervisor::{
    ActiveInvocation, DesiredExecution, ExecutionCommand, ExecutionError, ExecutionPhase,
    ExecutionReceipt, ExecutionSnapshot, ExecutionSupervisor, InvocationResolution, RunBudget,
    SwarmExecutionSnapshot,
};
