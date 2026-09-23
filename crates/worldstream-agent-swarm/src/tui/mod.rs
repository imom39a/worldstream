//! Interactive terminal reducer, rendering, and session driver.

use crate::{
    ApplicationError, ArtifactPath, ArtifactWorkspace, AuthoritativeArtifactRef, SwarmApplication,
    backend::{
        BackendError, ExactSwarmAction, SwarmActionReceipt, SwarmActor, SwarmBackend,
        SwarmObservation,
    },
    domain::{
        AcceptanceCriterion, CoordinatorOutcome, CoordinatorServiceView, CreateSwarm,
        DEFAULT_CORRECTION_FAILURE_LIMIT, DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS,
        MemberConfiguration, ProviderConfigurationState, ProviderEffectiveState, SwarmSummary,
        SwarmView, ValidatedCreateSwarm,
    },
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{CrosstermBackend, TestBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    fmt::Write as _,
    io::{self, Stdout},
    path::PathBuf,
};
#[cfg(not(feature = "managed-local-runtime"))]
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum Input {
    Up,
    Down,
    Open,
    New,
    Refresh,
    StageNextAction,
    BeginSuggestion,
    BeginDirection,
    BeginConfiguration,
    BeginExecutionPolicy,
    ToggleTarget,
    Text { text: String },
    Paste { text: String },
    Backspace,
    InspectArtifact,
    StageNextPolicy,
    Pause,
    Stop,
    Resume,
    Quit,
    Home,
    Resize { width: u16, height: u16 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    None,
    OpenSelected,
    StartCreation,
    Refresh,
    StageNextAction,
    StageNextPolicy,
    Pause,
    Stop,
    Resume,
    Detach,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalState {
    pub selected_member: usize,
    pub selected_swarm: usize,
    pub width: u16,
    pub height: u16,
    pub status: String,
    pub workflow_detail: bool,
    pub artifact_index: usize,
    pub artifact_preview: Option<ArtifactPreview>,
    pub creation_form: Option<String>,
}
impl Default for TerminalState {
    fn default() -> Self {
        Self {
            selected_member: 0,
            selected_swarm: 0,
            width: 100,
            height: 30,
            status: "ready".into(),
            workflow_detail: false,
            artifact_index: 0,
            artifact_preview: None,
            creation_form: None,
        }
    }
}
impl TerminalState {
    pub fn handle(&mut self, input: &Input, count: usize) -> Command {
        match input {
            Input::Up => self.selected_member = self.selected_member.saturating_sub(1),
            Input::Down if count > 0 => {
                self.selected_member = (self.selected_member + 1).min(count - 1);
            }
            Input::Down | Input::Home => self.selected_member = 0,
            Input::Open => return Command::OpenSelected,
            Input::New => return Command::StartCreation,
            Input::Refresh => return Command::Refresh,
            Input::StageNextAction => return Command::StageNextAction,
            Input::BeginSuggestion
            | Input::BeginDirection
            | Input::BeginConfiguration
            | Input::BeginExecutionPolicy
            | Input::ToggleTarget
            | Input::Text { .. }
            | Input::Paste { .. }
            | Input::Backspace
            | Input::InspectArtifact => {}
            Input::StageNextPolicy => return Command::StageNextPolicy,
            Input::Pause => return Command::Pause,
            Input::Stop => return Command::Stop,
            Input::Resume => return Command::Resume,
            Input::Quit => return Command::Detach,
            Input::Resize { width, height } => {
                self.width = *width;
                self.height = *height;
                self.status = format!("terminal {width}×{height}");
            }
        }
        Command::None
    }
}

#[derive(Debug, Error)]
pub enum TuiError {
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("application failed: {0}")]
    Application(#[from] crate::ApplicationError),
    #[error("script ended before detach")]
    ScriptExhausted,
    #[error("new requested without a creation request")]
    MissingCreateRequest,
    #[error("the TUI action plan has no remaining human Action")]
    MissingAction,
    #[error("the TUI policy plan has no remaining execution policy")]
    MissingPolicy,
    #[error("the TUI may stage only an authorized human Action")]
    WorkerActionNotAllowed,
    #[error("execution control failed: {0}")]
    Execution(String),
}

/// Operational state from the separate local execution daemon. These values
/// are never presented as authoritative Room facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionView {
    pub desired: String,
    pub phase: String,
    pub active: usize,
    pub queued: usize,
    pub invocations_started: u64,
    pub unknown_effects: usize,
    pub priority: u8,
    pub invocation_limit: Option<u64>,
    pub active_time_limit_ms: Option<u64>,
    pub provider_caps: BTreeMap<String, u16>,
    pub coordinator: Option<CoordinatorServiceView>,
}

const MAX_ARTIFACT_PREVIEW_BYTES: u64 = 64 * 1024;
const MAX_ARTIFACT_PREVIEW_CHARS: usize = 8 * 1024;
const MAX_ARTIFACT_PREVIEW_LINES: usize = 80;

/// Bounded, digest-verified textual content for one authoritative Room
/// artifact. The preview never grants a path additional authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactPreview {
    pub path: ArtifactPath,
    pub digest: String,
    pub media_type: String,
    pub byte_length: u64,
    pub content: String,
    pub truncated: bool,
}

/// Narrow content-inspection port used only after the TUI selects an exact
/// authoritative descriptor from the current participant observation.
pub trait ArtifactInspection {
    /// Returns a bounded textual preview or a fail-closed explanation.
    ///
    /// # Errors
    /// Rejects unavailable storage, untrusted/ambiguous paths, unsupported
    /// media, digest mismatch, non-UTF-8 bytes, and oversized artifacts.
    fn inspect(
        &self,
        observation: &SwarmObservation,
        selected: &AuthoritativeArtifactRef,
    ) -> Result<ArtifactPreview, String>;
}

/// Artifact inspector backed by the protected content-addressed workspace.
pub struct WorkspaceArtifactInspector {
    protected_root: PathBuf,
}

impl WorkspaceArtifactInspector {
    #[must_use]
    pub fn new(protected_root: impl Into<PathBuf>) -> Self {
        Self {
            protected_root: protected_root.into(),
        }
    }
}

impl ArtifactInspection for WorkspaceArtifactInspector {
    fn inspect(
        &self,
        observation: &SwarmObservation,
        selected: &AuthoritativeArtifactRef,
    ) -> Result<ArtifactPreview, String> {
        if !textual_media_type(&selected.media_type) {
            return Err(format!(
                "artifact media type is not safely previewable: {}",
                selected.media_type
            ));
        }
        let workspace =
            ArtifactWorkspace::open(&observation.swarm.working_area, &self.protected_root)
                .map_err(|error| error.to_string())?;
        let (path, artifact) = workspace
            .capture_authoritative_bounded(
                &observation.activity,
                selected,
                MAX_ARTIFACT_PREVIEW_BYTES,
            )
            .map_err(|error| error.to_string())?;
        let bytes = workspace
            .read(&artifact)
            .map_err(|error| error.to_string())?;
        let text = String::from_utf8(bytes).map_err(|_| {
            "authoritative artifact is not valid UTF-8 text and was not displayed".to_owned()
        })?;
        let (content, truncated) = sanitized_bounded_preview(&text);
        Ok(ArtifactPreview {
            path,
            digest: selected.digest.clone(),
            media_type: selected.media_type.clone(),
            byte_length: artifact.byte_length(),
            content,
            truncated,
        })
    }
}

struct NoArtifactInspection;

impl ArtifactInspection for NoArtifactInspection {
    fn inspect(
        &self,
        _observation: &SwarmObservation,
        _selected: &AuthoritativeArtifactRef,
    ) -> Result<ArtifactPreview, String> {
        Err("verified artifact preview is unavailable without protected artifact state".to_owned())
    }
}

impl ArtifactInspection for Option<WorkspaceArtifactInspector> {
    fn inspect(
        &self,
        observation: &SwarmObservation,
        selected: &AuthoritativeArtifactRef,
    ) -> Result<ArtifactPreview, String> {
        self.as_ref().map_or_else(
            || NoArtifactInspection.inspect(observation, selected),
            |inspector| inspector.inspect(observation, selected),
        )
    }
}

/// Explicit execution policy staged and confirmed by the human. These values
/// affect only daemon admission; they never mutate authoritative Room facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionPolicy {
    SetProviderCap {
        provider: String,
        limit: u16,
    },
    SetPriority {
        priority: u8,
    },
    SetBudget {
        invocation_limit: Option<u64>,
        active_time_limit_ms: Option<u64>,
    },
}

/// Narrow TUI port for execution state and controls. Room Actions continue to
/// flow only through [`SwarmApplication`].
pub trait ExecutionControls {
    /// Registers a newly created authoritative Swarm view in stopped state,
    /// including its exact selected roster for initial provider capacity.
    ///
    /// # Errors
    /// Returns a closed daemon-control or persistence failure.
    fn register(&mut self, swarm: &SwarmView) -> Result<(), String>;
    /// Reads operational state without changing it.
    ///
    /// # Errors
    /// Returns a closed daemon-control or state-decoding failure.
    fn status(&self, swarm_id: &crate::SwarmId) -> Result<Option<ExecutionView>, String>;
    /// Requests drain-to-pause for one Swarm.
    ///
    /// # Errors
    /// Returns a closed daemon-control or persistence failure.
    fn pause(&mut self, swarm_id: &crate::SwarmId) -> Result<ExecutionView, String>;
    /// Requests interruption of the Swarm's owned process trees.
    ///
    /// # Errors
    /// Returns a closed daemon-control, process, or persistence failure.
    fn stop(&mut self, swarm_id: &crate::SwarmId) -> Result<ExecutionView, String>;
    /// Explicitly resumes a reconciled Swarm.
    ///
    /// # Errors
    /// Returns a closed daemon-control or recovery-state failure.
    fn resume(&mut self, swarm_id: &crate::SwarmId) -> Result<ExecutionView, String>;
    /// Applies one explicitly staged operational scheduling policy.
    ///
    /// # Errors
    /// Returns a closed validation, daemon-control, or persistence failure.
    fn apply_policy(
        &mut self,
        swarm_id: &crate::SwarmId,
        policy: &ExecutionPolicy,
    ) -> Result<ExecutionView, String>;
}

struct NoExecutionControls;

impl ExecutionControls for NoExecutionControls {
    fn register(&mut self, _swarm: &SwarmView) -> Result<(), String> {
        Ok(())
    }

    fn status(&self, _swarm_id: &crate::SwarmId) -> Result<Option<ExecutionView>, String> {
        Ok(None)
    }

    fn pause(&mut self, _swarm_id: &crate::SwarmId) -> Result<ExecutionView, String> {
        Err("no execution daemon is attached".to_owned())
    }

    fn stop(&mut self, _swarm_id: &crate::SwarmId) -> Result<ExecutionView, String> {
        Err("no execution daemon is attached".to_owned())
    }

    fn resume(&mut self, _swarm_id: &crate::SwarmId) -> Result<ExecutionView, String> {
        Err("no execution daemon is attached".to_owned())
    }

    fn apply_policy(
        &mut self,
        _swarm_id: &crate::SwarmId,
        _policy: &ExecutionPolicy,
    ) -> Result<ExecutionView, String> {
        Err("no execution daemon is attached".to_owned())
    }
}
#[derive(Clone, Copy)]
pub enum Dashboard<'a> {
    List(&'a [SwarmSummary]),
    Swarm {
        view: &'a SwarmView,
        observation: Option<&'a SwarmObservation>,
        execution: Option<&'a ExecutionView>,
    },
}
pub trait TuiSession {
    /// # Errors
    /// Returns a terminal setup failure.
    fn enter(&mut self) -> Result<(), TuiError>;
    /// # Errors
    /// Returns a terminal draw failure.
    fn draw(&mut self, state: &TerminalState, dashboard: Dashboard<'_>) -> Result<(), TuiError>;
    /// # Errors
    /// Returns an input transport failure.
    fn next_input(&mut self) -> Result<Input, TuiError>;
    /// # Errors
    /// Returns a terminal restoration failure.
    fn restore(&mut self) -> Result<(), TuiError>;
}

#[derive(Debug, Serialize)]
pub struct TuiReceipt {
    pub status: &'static str,
    pub created: bool,
    pub swarm_id: Option<String>,
    pub room_id: Option<String>,
    pub actions_submitted: usize,
    pub execution_commands: usize,
    pub policies_applied: usize,
}

/// # Errors
/// Returns application, terminal, or incomplete-script failures after restoring the session.
pub fn run<B: SwarmBackend, S: TuiSession>(
    app: &mut SwarmApplication<B>,
    session: &mut S,
    request: Option<&CreateSwarm>,
) -> Result<TuiReceipt, TuiError> {
    run_with_actions(app, session, request, Vec::new())
}

/// Runs the TUI with an explicit sequence of caller-retained human Actions.
/// Pressing `a` stages the next Action and displays its target; Enter confirms
/// it on the following input cycle. No Action is refreshed or rewritten.
///
/// # Errors
/// Returns application, terminal, stale-Action, or incomplete-script failures
/// after restoring the terminal session.
pub fn run_with_actions<B: SwarmBackend, S: TuiSession>(
    app: &mut SwarmApplication<B>,
    session: &mut S,
    request: Option<&CreateSwarm>,
    actions: Vec<ExactSwarmAction>,
) -> Result<TuiReceipt, TuiError> {
    let mut execution = NoExecutionControls;
    run_with_actions_and_execution(app, session, &mut execution, request, actions)
}

/// Runs the TUI against an attached execution-control port. Operational
/// controls never mutate Room state or infer workflow completion.
///
/// # Errors
/// Returns application, execution-control, terminal, stale-Action, or
/// incomplete-script failures after restoring the terminal.
pub fn run_with_actions_and_execution<B: SwarmBackend, S: TuiSession, E: ExecutionControls>(
    app: &mut SwarmApplication<B>,
    session: &mut S,
    execution: &mut E,
    request: Option<&CreateSwarm>,
    actions: Vec<ExactSwarmAction>,
) -> Result<TuiReceipt, TuiError> {
    run_with_plans_and_execution(app, session, execution, request, actions, Vec::new())
}

/// Runs the TUI with explicit Room Action and execution-policy plans. Both are
/// staged separately and require a later Enter before mutation.
///
/// # Errors
/// Returns closed application, daemon, or terminal failures after restoration.
pub fn run_with_plans_and_execution<B: SwarmBackend, S: TuiSession, E: ExecutionControls>(
    app: &mut SwarmApplication<B>,
    session: &mut S,
    execution: &mut E,
    request: Option<&CreateSwarm>,
    actions: Vec<ExactSwarmAction>,
    policies: Vec<ExecutionPolicy>,
) -> Result<TuiReceipt, TuiError> {
    let artifacts = NoArtifactInspection;
    run_with_plans_execution_and_artifacts(
        app, session, execution, &artifacts, request, actions, policies,
    )
}

/// Runs the TUI with the same staged mutation model plus a narrowly scoped
/// authoritative artifact inspector. The inspector cannot receive arbitrary
/// user-entered paths; it receives only descriptors discovered in the current
/// Room observation.
///
/// # Errors
/// Returns closed application, execution, artifact, or terminal failures after
/// terminal restoration.
pub fn run_with_plans_execution_and_artifacts<
    B: SwarmBackend,
    S: TuiSession,
    E: ExecutionControls,
    A: ArtifactInspection,
>(
    app: &mut SwarmApplication<B>,
    session: &mut S,
    execution: &mut E,
    artifacts: &A,
    request: Option<&CreateSwarm>,
    actions: Vec<ExactSwarmAction>,
    policies: Vec<ExecutionPolicy>,
) -> Result<TuiReceipt, TuiError> {
    let result = run_inner(
        app, session, execution, artifacts, request, actions, policies,
    );
    let restored = session.restore();
    match (result, restored) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(receipt), Ok(())) => Ok(receipt),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "the single deterministic input state machine keeps staged confirmation and detach semantics auditable"
)]
fn run_inner<B: SwarmBackend, S: TuiSession, E: ExecutionControls, A: ArtifactInspection>(
    app: &mut SwarmApplication<B>,
    session: &mut S,
    execution: &mut E,
    artifacts: &A,
    request: Option<&CreateSwarm>,
    actions: Vec<ExactSwarmAction>,
    policies: Vec<ExecutionPolicy>,
) -> Result<TuiReceipt, TuiError> {
    session.enter()?;
    let mut state = TerminalState::default();
    let mut summaries = app.list()?;
    let mut opened: Option<OpenedSwarm> = None;
    let mut created = false;
    let mut draft_creation: Option<CreationDraft> = None;
    let mut pending_creation: Option<CreateSwarm> = None;
    let mut action_plan = VecDeque::from(actions);
    let mut pending_action = None;
    let mut draft_action: Option<ActionDraft> = None;
    let mut draft_configuration: Option<ConfigurationDraft> = None;
    let mut policy_plan = VecDeque::from(policies);
    let mut pending_policy = None;
    let mut draft_policy: Option<PolicyDraft> = None;
    let mut actions_submitted = 0;
    let mut execution_commands = 0;
    let mut policies_applied = 0;
    loop {
        session.draw(
            &state,
            opened
                .as_ref()
                .map_or(Dashboard::List(&summaries), |opened| Dashboard::Swarm {
                    view: &opened.view,
                    observation: opened.observation.as_ref(),
                    execution: opened.execution.as_ref(),
                }),
        )?;
        let authoring = draft_creation.is_some()
            || draft_action.is_some()
            || draft_configuration.is_some()
            || draft_policy.is_some();
        let input = normalize_authoring_input(session.next_input()?, authoring);
        match input {
            Input::Open if pending_creation.is_some() => {
                if opened.is_some() {
                    "return to the Swarm list before creating another Swarm"
                        .clone_into(&mut state.status);
                    continue;
                }
                let pending = pending_creation
                    .take()
                    .ok_or(TuiError::MissingCreateRequest)?;
                let view = app.create(pending)?;
                execution.register(&view).map_err(TuiError::Execution)?;
                state.status = format!(
                    "Swarm {} created · execution registered stopped",
                    view.swarm_id.as_str()
                );
                opened = Some(observe_swarm(app, execution, view)?);
                created = true;
                state.creation_form = None;
                summaries = app.list()?;
            }
            Input::Open if draft_creation.is_some() => {
                let draft = draft_creation
                    .as_ref()
                    .ok_or(TuiError::MissingCreateRequest)?;
                match draft.parse() {
                    Ok(create) => {
                        state.status = creation_preview(&create);
                        state.creation_form = Some(creation_form_preview(&create));
                        pending_creation = Some(create);
                        draft_creation = None;
                    }
                    Err(message) => state.status = message,
                }
            }
            Input::Up | Input::Down => {
                let length = opened
                    .as_ref()
                    .map_or(summaries.len(), |opened| opened.view.roster.len());
                let selected = if opened.is_some() {
                    &mut state.selected_member
                } else {
                    &mut state.selected_swarm
                };
                if input == Input::Up {
                    *selected = selected.saturating_sub(1);
                } else if length > 0 {
                    *selected = (*selected + 1).min(length - 1);
                }
            }
            Input::Open if pending_action.is_some() => {
                let Some(opened_swarm) = &opened else {
                    "open a Swarm before confirming an Action".clone_into(&mut state.status);
                    continue;
                };
                let pending = pending_action.take().ok_or(TuiError::MissingAction)?;
                let receipt = app.submit(&opened_swarm.view.swarm_id, &pending)?;
                actions_submitted += 1;
                state.status = action_status(&receipt);
                opened = Some(open_swarm(app, execution, &opened_swarm.view.swarm_id)?);
            }
            Input::Open if draft_action.is_some() => {
                let Some(current) = &opened else {
                    "open a Swarm before authoring an Action".clone_into(&mut state.status);
                    continue;
                };
                let draft = draft_action.take().ok_or(TuiError::MissingAction)?;
                match author_action(current, &state, &draft) {
                    Ok(action) => {
                        state.status = action_preview(&action);
                        pending_action = Some(action);
                    }
                    Err(message) => state.status = message,
                }
            }
            Input::Open if draft_configuration.is_some() => {
                let Some(current) = &opened else {
                    "open a Swarm before authoring a configuration Action"
                        .clone_into(&mut state.status);
                    continue;
                };
                let draft = draft_configuration
                    .as_ref()
                    .ok_or(TuiError::MissingAction)?;
                match author_configuration_action(current, draft) {
                    Ok(action) => {
                        draft_configuration = None;
                        state.status = action_preview(&action);
                        pending_action = Some(action);
                    }
                    Err(message) => state.status = message,
                }
            }
            Input::Open if pending_policy.is_some() => {
                let Some(current) = &mut opened else {
                    "open a Swarm before confirming a policy".clone_into(&mut state.status);
                    continue;
                };
                let policy = pending_policy.take().ok_or(TuiError::MissingPolicy)?;
                let view = execution
                    .apply_policy(&current.view.swarm_id, &policy)
                    .map_err(TuiError::Execution)?;
                state.status = format!("execution policy applied · {}", policy_preview(&policy));
                current.execution = Some(view);
                execution_commands += 1;
                policies_applied += 1;
            }
            Input::Open if draft_policy.is_some() => {
                let draft = draft_policy.as_ref().ok_or(TuiError::MissingPolicy)?;
                match draft.parse() {
                    Ok(policy) => {
                        draft_policy = None;
                        state.status = format!(
                            "STAGED POLICY {} · Enter confirm · Esc cancel",
                            policy_preview(&policy)
                        );
                        pending_policy = Some(policy);
                    }
                    Err(message) => state.status = message,
                }
            }
            Input::Open if opened.is_none() => {
                if let Some(summary) = summaries.get(state.selected_swarm) {
                    opened = Some(open_swarm(app, execution, &summary.swarm_id)?);
                }
            }
            Input::New => {
                if opened.is_some() {
                    "return to the Swarm list before creating another Swarm"
                        .clone_into(&mut state.status);
                    continue;
                }
                if pending_action.is_some()
                    || pending_policy.is_some()
                    || pending_creation.is_some()
                    || draft_action.is_some()
                    || draft_configuration.is_some()
                    || draft_policy.is_some()
                {
                    "A mutation is already staged · Enter confirm · Esc cancel"
                        .clone_into(&mut state.status);
                    continue;
                }
                let draft = CreationDraft::new(request);
                state.status = draft.status();
                state.creation_form = Some(draft.form());
                draft_creation = Some(draft);
            }
            Input::Refresh => {
                summaries = app.list()?;
                if let Some(current) = &opened {
                    opened = Some(open_swarm(app, execution, &current.view.swarm_id)?);
                }
            }
            Input::StageNextAction => {
                if opened.is_none() {
                    "open a Swarm before staging an Action".clone_into(&mut state.status);
                    continue;
                }
                if pending_action.is_some() || pending_policy.is_some() {
                    "A mutation is already staged · Enter confirm · Esc cancel"
                        .clone_into(&mut state.status);
                    continue;
                }
                let action = action_plan.pop_front().ok_or(TuiError::MissingAction)?;
                if action.actor != SwarmActor::HumanCoordinator {
                    return Err(TuiError::WorkerActionNotAllowed);
                }
                state.status = action_preview(&action);
                pending_action = Some(action);
            }
            Input::BeginSuggestion | Input::BeginDirection => {
                if opened.is_none() {
                    "open a Swarm before authoring an Action".clone_into(&mut state.status);
                    continue;
                }
                if pending_action.is_some()
                    || pending_policy.is_some()
                    || draft_configuration.is_some()
                    || draft_policy.is_some()
                {
                    "A mutation is already staged · Enter confirm · Esc cancel"
                        .clone_into(&mut state.status);
                    continue;
                }
                let kind = if matches!(input, Input::BeginSuggestion) {
                    AuthoredActionKind::Suggestion
                } else {
                    AuthoredActionKind::Direction
                };
                let draft = ActionDraft {
                    kind,
                    text: String::new(),
                    target: ActionTarget::WholeSwarm,
                };
                state.status = draft_status(&draft);
                draft_action = Some(draft);
            }
            Input::BeginConfiguration => {
                let Some(current) = &opened else {
                    "open a Swarm before authoring a configuration Action"
                        .clone_into(&mut state.status);
                    continue;
                };
                if pending_action.is_some()
                    || pending_policy.is_some()
                    || draft_action.is_some()
                    || draft_policy.is_some()
                {
                    "A mutation is already staged · Enter confirm · Esc cancel"
                        .clone_into(&mut state.status);
                    continue;
                }
                match configuration_draft(current, &state) {
                    Ok(draft) => {
                        state.status = draft.status();
                        draft_configuration = Some(draft);
                    }
                    Err(message) => state.status = message,
                }
            }
            Input::BeginExecutionPolicy => {
                if opened.is_none() {
                    "open a Swarm before authoring an execution policy"
                        .clone_into(&mut state.status);
                    continue;
                }
                if pending_action.is_some()
                    || pending_policy.is_some()
                    || draft_action.is_some()
                    || draft_configuration.is_some()
                {
                    "A mutation is already staged · Enter confirm · Esc cancel"
                        .clone_into(&mut state.status);
                    continue;
                }
                let draft = PolicyDraft::default();
                state.status = draft.status();
                draft_policy = Some(draft);
            }
            Input::Text { text } | Input::Paste { text } if draft_action.is_some() => {
                if let Some(draft) = draft_action.as_mut() {
                    append_bounded(&mut draft.text, &text);
                    state.status = draft_status(draft);
                }
            }
            Input::Text { text } | Input::Paste { text } if draft_creation.is_some() => {
                if let Some(draft) = draft_creation.as_mut() {
                    draft.append(&text);
                    state.status = draft.status();
                    state.creation_form = Some(draft.form());
                }
            }
            Input::Text { text } | Input::Paste { text } if draft_configuration.is_some() => {
                if let Some(draft) = draft_configuration.as_mut() {
                    append_bounded(&mut draft.text, &text);
                    state.status = draft.status();
                }
            }
            Input::Text { text } | Input::Paste { text } if draft_policy.is_some() => {
                if let Some(draft) = draft_policy.as_mut() {
                    append_bounded(&mut draft.text, &text);
                    state.status = draft.status();
                }
            }
            Input::Backspace if draft_action.is_some() => {
                if let Some(draft) = draft_action.as_mut() {
                    draft.text.pop();
                    state.status = draft_status(draft);
                }
            }
            Input::Backspace if draft_creation.is_some() => {
                if let Some(draft) = draft_creation.as_mut() {
                    draft.backspace();
                    state.status = draft.status();
                    state.creation_form = Some(draft.form());
                }
            }
            Input::Backspace if draft_configuration.is_some() => {
                if let Some(draft) = draft_configuration.as_mut() {
                    draft.text.pop();
                    state.status = draft.status();
                }
            }
            Input::Backspace if draft_policy.is_some() => {
                if let Some(draft) = draft_policy.as_mut() {
                    draft.text.pop();
                    state.status = draft.status();
                }
            }
            Input::ToggleTarget if draft_action.is_some() => {
                if let Some(draft) = draft_action.as_mut() {
                    draft.target = match draft.target {
                        ActionTarget::WholeSwarm => ActionTarget::SelectedMemberWork,
                        ActionTarget::SelectedMemberWork => ActionTarget::WholeSwarm,
                    };
                    state.status = draft_status(draft);
                }
            }
            Input::ToggleTarget if draft_creation.is_some() => {
                if let Some(draft) = draft_creation.as_mut() {
                    draft.next_field();
                    state.status = draft.status();
                    state.creation_form = Some(draft.form());
                }
            }
            Input::ToggleTarget if draft_policy.is_some() => {
                if let Some(draft) = draft_policy.as_mut() {
                    draft.cycle_kind();
                    state.status = draft.status();
                }
            }
            Input::ToggleTarget if draft_configuration.is_some() => {
                if let Some(draft) = draft_configuration.as_ref() {
                    state.status = draft.status();
                }
            }
            Input::InspectArtifact => {
                let Some(observation) = opened
                    .as_ref()
                    .and_then(|current| current.observation.as_ref())
                else {
                    "artifact inspection requires a current authorized observation"
                        .clone_into(&mut state.status);
                    continue;
                };
                let references = authoritative_artifacts(observation);
                if references.is_empty() {
                    state.workflow_detail = true;
                    state.artifact_preview = None;
                    "no complete authoritative artifact reference is available"
                        .clone_into(&mut state.status);
                    continue;
                }
                if state.workflow_detail {
                    state.artifact_index = (state.artifact_index + 1) % references.len();
                } else {
                    state.workflow_detail = true;
                    state.artifact_index = 0;
                }
                state.artifact_preview = None;
                let selected = &references[state.artifact_index];
                match artifacts.inspect(observation, selected) {
                    Ok(preview) => {
                        state.status = format!(
                            "verified artifact {}/{} · {} · {} bytes · i next · Esc close",
                            state.artifact_index + 1,
                            references.len(),
                            preview.path,
                            preview.byte_length
                        );
                        state.artifact_preview = Some(preview);
                    }
                    Err(message) => {
                        state.status = format!(
                            "artifact {}/{} not displayed · {} · {message}",
                            state.artifact_index + 1,
                            references.len(),
                            bounded_display(&selected.local_path, 72)
                        );
                    }
                }
            }
            Input::StageNextPolicy => {
                if opened.is_none() {
                    "open a Swarm before staging a policy".clone_into(&mut state.status);
                    continue;
                }
                if pending_action.is_some()
                    || pending_policy.is_some()
                    || draft_action.is_some()
                    || draft_configuration.is_some()
                    || draft_policy.is_some()
                {
                    "A mutation is already staged · Enter confirm · Esc cancel"
                        .clone_into(&mut state.status);
                    continue;
                }
                let policy = policy_plan.pop_front().ok_or(TuiError::MissingPolicy)?;
                state.status = format!(
                    "STAGED POLICY {} · Enter confirm · Esc cancel",
                    policy_preview(&policy)
                );
                pending_policy = Some(policy);
            }
            Input::Home
                if draft_creation.is_some()
                    || draft_action.is_some()
                    || draft_configuration.is_some()
                    || draft_policy.is_some()
                    || pending_creation.is_some()
                    || pending_action.is_some()
                    || pending_policy.is_some() =>
            {
                draft_creation = None;
                draft_action = None;
                draft_configuration = None;
                draft_policy = None;
                pending_creation = None;
                pending_action = None;
                pending_policy = None;
                state.creation_form = None;
                "staged mutation cancelled".clone_into(&mut state.status);
            }
            Input::Home if state.workflow_detail => {
                state.workflow_detail = false;
                state.artifact_preview = None;
                "authoritative detail closed".clone_into(&mut state.status);
            }
            Input::Home => opened = None,
            Input::Pause | Input::Stop | Input::Resume => {
                let Some(current) = &mut opened else {
                    "open a Swarm before using execution controls".clone_into(&mut state.status);
                    continue;
                };
                let view = if input == Input::Pause {
                    execution.pause(&current.view.swarm_id)
                } else if input == Input::Stop {
                    execution.stop(&current.view.swarm_id)
                } else {
                    execution.resume(&current.view.swarm_id)
                }
                .map_err(TuiError::Execution)?;
                state.status = format!("execution {} · desired {}", view.phase, view.desired);
                current.execution = Some(view);
                execution_commands += 1;
            }
            Input::Quit => {
                return Ok(TuiReceipt {
                    status: "detached",
                    created,
                    swarm_id: opened
                        .as_ref()
                        .map(|opened| opened.view.swarm_id.as_str().to_owned()),
                    room_id: opened.as_ref().map(|opened| opened.view.room_id.clone()),
                    actions_submitted,
                    execution_commands,
                    policies_applied,
                });
            }
            Input::Resize { width, height } => {
                state.width = width;
                state.height = height;
                state.status = format!("terminal {width}×{height}");
            }
            Input::Open
            | Input::Text { .. }
            | Input::Paste { .. }
            | Input::Backspace
            | Input::ToggleTarget => {}
        }
    }
}

fn normalize_authoring_input(input: Input, authoring: bool) -> Input {
    if !authoring {
        return input;
    }
    let character = match input {
        Input::New => 'n',
        Input::Refresh => 'r',
        Input::StageNextAction => 'a',
        Input::BeginSuggestion => 'g',
        Input::BeginDirection => 'd',
        Input::BeginConfiguration => 'c',
        Input::BeginExecutionPolicy => 'e',
        Input::InspectArtifact => 'i',
        Input::StageNextPolicy => 'l',
        Input::Pause => 'p',
        Input::Stop => 's',
        Input::Resume => 'u',
        Input::Quit => 'q',
        other => return other,
    };
    Input::Text {
        text: character.to_string(),
    }
}

#[derive(Clone, Copy)]
enum AuthoredActionKind {
    Suggestion,
    Direction,
}

#[derive(Clone, Copy)]
enum ActionTarget {
    WholeSwarm,
    SelectedMemberWork,
}

struct ActionDraft {
    kind: AuthoredActionKind,
    text: String,
    target: ActionTarget,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum CreationField {
    #[default]
    Goal,
    Constraints,
    AcceptanceCriteria,
    WorkingArea,
    Roster,
}

impl CreationField {
    const fn label(self) -> &'static str {
        match self {
            Self::Goal => "GOAL",
            Self::Constraints => "CONSTRAINTS",
            Self::AcceptanceCriteria => "ACCEPTANCE CRITERIA",
            Self::WorkingArea => "WORKING AREA",
            Self::Roster => "ROSTER",
        }
    }

    const fn syntax(self) -> &'static str {
        match self {
            Self::Goal => "plain text",
            Self::Constraints => "items separated by |, or - for none",
            Self::AcceptanceCriteria => "one or more items separated by |",
            Self::WorkingArea => "absolute existing directory",
            Self::Roster => {
                "members separated by ; each key | label | provider | model | effort-or-- | ack-or-unack"
            }
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Goal => Self::Constraints,
            Self::Constraints => Self::AcceptanceCriteria,
            Self::AcceptanceCriteria => Self::WorkingArea,
            Self::WorkingArea => Self::Roster,
            Self::Roster => Self::Goal,
        }
    }
}

struct CreationDraft {
    field: CreationField,
    goal: String,
    constraints: String,
    acceptance_criteria: String,
    working_area: String,
    roster: String,
    progress_review_interval_seconds: u32,
    correction_failure_limit: u32,
}

impl CreationDraft {
    fn new(template: Option<&CreateSwarm>) -> Self {
        template.map_or_else(
            || Self {
                field: CreationField::Goal,
                goal: String::new(),
                constraints: String::new(),
                acceptance_criteria: String::new(),
                working_area: String::new(),
                roster: String::new(),
                progress_review_interval_seconds: DEFAULT_PROGRESS_REVIEW_INTERVAL_SECONDS,
                correction_failure_limit: DEFAULT_CORRECTION_FAILURE_LIMIT,
            },
            |request| Self {
                field: CreationField::Goal,
                goal: request.goal.clone(),
                constraints: if request.constraints.is_empty() {
                    "-".to_owned()
                } else {
                    request.constraints.join(" | ")
                },
                acceptance_criteria: request
                    .acceptance_criteria
                    .iter()
                    .map(|criterion| criterion.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" | "),
                working_area: request.working_area.display().to_string(),
                roster: request
                    .roster
                    .iter()
                    .map(|member| {
                        format!(
                            "{} | {} | {} | {} | {} | {}",
                            member.member_key,
                            member.label,
                            member.provider,
                            member.requested_model,
                            member.requested_effort.as_deref().unwrap_or("-"),
                            if member.moving_alias_acknowledged {
                                "ack"
                            } else {
                                "unack"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ; "),
                progress_review_interval_seconds: request.progress_review_interval_seconds,
                correction_failure_limit: request.correction_failure_limit,
            },
        )
    }

    fn current(&self) -> &str {
        match self.field {
            CreationField::Goal => &self.goal,
            CreationField::Constraints => &self.constraints,
            CreationField::AcceptanceCriteria => &self.acceptance_criteria,
            CreationField::WorkingArea => &self.working_area,
            CreationField::Roster => &self.roster,
        }
    }

    fn current_mut(&mut self) -> &mut String {
        match self.field {
            CreationField::Goal => &mut self.goal,
            CreationField::Constraints => &mut self.constraints,
            CreationField::AcceptanceCriteria => &mut self.acceptance_criteria,
            CreationField::WorkingArea => &mut self.working_area,
            CreationField::Roster => &mut self.roster,
        }
    }

    fn append(&mut self, text: &str) {
        append_bounded_to(self.current_mut(), text, 16 * 1024);
    }

    fn backspace(&mut self) {
        self.current_mut().pop();
    }

    fn next_field(&mut self) {
        self.field = self.field.next();
    }

    fn status(&self) -> String {
        format!(
            "AUTHORING NEW SWARM · {} · {} · {} · Tab next field · Enter stage · Esc cancel",
            self.field.label(),
            self.field.syntax(),
            bounded_display(self.current(), 88)
        )
    }

    fn form(&self) -> String {
        let value = |field, label: &str, value: &str| {
            format!(
                "{} {label}: {}",
                if self.field == field { ">" } else { " " },
                bounded_display(value, 88)
            )
        };
        [
            "NEW SWARM FORM · Tab next · Enter stage · Esc cancel".to_owned(),
            value(CreationField::Goal, "GOAL", &self.goal),
            value(CreationField::Constraints, "CONSTRAINTS", &self.constraints),
            value(
                CreationField::AcceptanceCriteria,
                "ACCEPTANCE CRITERIA",
                &self.acceptance_criteria,
            ),
            value(
                CreationField::WorkingArea,
                "WORKING AREA",
                &self.working_area,
            ),
            value(CreationField::Roster, "ROSTER", &self.roster),
        ]
        .join("\n")
    }

    fn parse(&self) -> Result<CreateSwarm, String> {
        let create = CreateSwarm {
            goal: self.goal.trim().to_owned(),
            constraints: draft_items(&self.constraints, true),
            acceptance_criteria: draft_items(&self.acceptance_criteria, false)
                .into_iter()
                .map(|text| AcceptanceCriterion { text })
                .collect(),
            working_area: PathBuf::from(self.working_area.trim()),
            roster: parse_creation_roster(&self.roster)?,
            progress_review_interval_seconds: self.progress_review_interval_seconds,
            correction_failure_limit: self.correction_failure_limit,
        };
        ValidatedCreateSwarm::try_from(create.clone())
            .map_err(|error| format!("new Swarm is invalid: {error}"))?;
        Ok(create)
    }
}

fn draft_items(value: &str, optional: bool) -> Vec<String> {
    let value = value.trim();
    if optional && (value.is_empty() || value == "-") {
        return Vec::new();
    }
    value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_creation_roster(value: &str) -> Result<Vec<MemberConfiguration>, String> {
    value
        .split([';', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let fields = entry.split('|').map(str::trim).collect::<Vec<_>>();
            let [member_key, label, provider, model, effort, alias] = fields.as_slice() else {
                return Err(
                    "each roster member requires: key | label | provider | model | effort-or-- | ack-or-unack"
                        .to_owned(),
                );
            };
            if !matches!(*provider, "codex" | "claude" | "kiro" | "controlled") {
                return Err("roster provider must be codex, claude, kiro, or controlled".to_owned());
            }
            let moving_alias_acknowledged = match *alias {
                "ack" => true,
                "unack" => false,
                _ => return Err("roster alias choice must be ack or unack".to_owned()),
            };
            Ok(MemberConfiguration {
                member_key: (*member_key).to_owned(),
                label: (*label).to_owned(),
                provider: (*provider).to_owned(),
                requested_model: (*model).to_owned(),
                requested_effort: (!effort.is_empty() && *effort != "-")
                    .then(|| (*effort).to_owned()),
                configuration_revision: 1,
                moving_alias_acknowledged,
                configuration_state: if *provider == "controlled" {
                    ProviderConfigurationState::FixtureUnavailable
                } else {
                    ProviderConfigurationState::ResolutionUnreported
                },
            })
        })
        .collect()
}

fn creation_preview(create: &CreateSwarm) -> String {
    format!(
        "STAGED NEW SWARM · {} · constraints {} · criteria {} · area {} · members {} · Enter confirm · Esc cancel",
        bounded_display(create.goal.trim(), 48),
        create.constraints.len(),
        create.acceptance_criteria.len(),
        bounded_display(&create.working_area.display().to_string(), 48),
        create.roster.len()
    )
}

fn creation_form_preview(create: &CreateSwarm) -> String {
    format!(
        "STAGED NEW SWARM · Enter confirm · Esc cancel\n  GOAL: {}\n  CONSTRAINTS: {}\n  ACCEPTANCE CRITERIA: {}\n  WORKING AREA: {}\n  ROSTER: {}",
        bounded_display(create.goal.trim(), 88),
        bounded_display(&create.constraints.join(" | "), 88),
        bounded_display(
            &create
                .acceptance_criteria
                .iter()
                .map(|criterion| criterion.text.as_str())
                .collect::<Vec<_>>()
                .join(" | "),
            88,
        ),
        bounded_display(&create.working_area.display().to_string(), 88),
        bounded_display(
            &create
                .roster
                .iter()
                .map(|member| format!("{} ({})", member.member_key, member.provider))
                .collect::<Vec<_>>()
                .join("; "),
            88,
        )
    )
}

struct ConfigurationDraft {
    member_key: String,
    expected_configuration_revision: u64,
    text: String,
}

impl ConfigurationDraft {
    fn status(&self) -> String {
        format!(
            "AUTHORING CONFIG {} r{}→r{} · provider | model | effort-or-- | ack-or-unack · {} · Enter stage · Esc cancel",
            self.member_key,
            self.expected_configuration_revision,
            self.expected_configuration_revision.saturating_add(1),
            bounded_display(&self.text, 72)
        )
    }
}

#[derive(Clone, Copy, Default)]
enum PolicyDraftKind {
    #[default]
    ProviderCap,
    Priority,
    Budget,
}

impl PolicyDraftKind {
    const fn syntax(self) -> &'static str {
        match self {
            Self::ProviderCap => "provider limit",
            Self::Priority => "priority (1..16)",
            Self::Budget => "invocation-limit-or-- active-ms-or--",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::ProviderCap => "PROVIDER CAP",
            Self::Priority => "SWARM PRIORITY",
            Self::Budget => "SWARM BUDGET",
        }
    }
}

#[derive(Default)]
struct PolicyDraft {
    kind: PolicyDraftKind,
    text: String,
}

impl PolicyDraft {
    fn status(&self) -> String {
        format!(
            "AUTHORING {} · {} · {} · Tab policy type · Enter stage · Esc cancel",
            self.kind.label(),
            self.kind.syntax(),
            bounded_display(&self.text, 72)
        )
    }

    fn cycle_kind(&mut self) {
        self.kind = match self.kind {
            PolicyDraftKind::ProviderCap => PolicyDraftKind::Priority,
            PolicyDraftKind::Priority => PolicyDraftKind::Budget,
            PolicyDraftKind::Budget => PolicyDraftKind::ProviderCap,
        };
        self.text.clear();
    }

    fn parse(&self) -> Result<ExecutionPolicy, String> {
        let fields = self
            .text
            .split(|character: char| character.is_whitespace() || character == '|')
            .filter(|field| !field.is_empty())
            .collect::<Vec<_>>();
        match self.kind {
            PolicyDraftKind::ProviderCap => {
                let [provider, limit] = fields.as_slice() else {
                    return Err("provider cap requires: provider limit".to_owned());
                };
                let limit = limit
                    .parse::<u16>()
                    .map_err(|_| "provider cap limit must be 0..65535".to_owned())?;
                Ok(ExecutionPolicy::SetProviderCap {
                    provider: (*provider).to_owned(),
                    limit,
                })
            }
            PolicyDraftKind::Priority => {
                let [priority] = fields.as_slice() else {
                    return Err("Swarm priority requires one value from 1 through 16".to_owned());
                };
                let priority = priority
                    .parse::<u8>()
                    .map_err(|_| "Swarm priority must be 1..16".to_owned())?;
                if !(1..=16).contains(&priority) {
                    return Err("Swarm priority must be 1..16".to_owned());
                }
                Ok(ExecutionPolicy::SetPriority { priority })
            }
            PolicyDraftKind::Budget => {
                let [invocations, active_ms] = fields.as_slice() else {
                    return Err(
                        "Swarm budget requires: invocation-limit-or-- active-ms-or--".to_owned(),
                    );
                };
                Ok(ExecutionPolicy::SetBudget {
                    invocation_limit: parse_optional_policy_limit(invocations, "invocation limit")?,
                    active_time_limit_ms: parse_optional_policy_limit(
                        active_ms,
                        "active-time limit",
                    )?,
                })
            }
        }
    }
}

fn parse_optional_policy_limit(value: &str, label: &str) -> Result<Option<u64>, String> {
    if matches!(value, "-" | "none" | "unlimited") {
        return Ok(None);
    }
    let limit = value
        .parse::<u64>()
        .map_err(|_| format!("{label} must be a positive integer or -"))?;
    if limit == 0 {
        return Err(format!("{label} must be positive or -"));
    }
    Ok(Some(limit))
}

fn append_bounded(target: &mut String, text: &str) {
    append_bounded_to(target, text, 4096);
}

fn append_bounded_to(target: &mut String, text: &str, maximum_chars: usize) {
    target.extend(
        text.chars()
            .take(maximum_chars.saturating_sub(target.chars().count())),
    );
}

fn draft_status(draft: &ActionDraft) -> String {
    let kind = match draft.kind {
        AuthoredActionKind::Suggestion => "Suggestion",
        AuthoredActionKind::Direction => "Direction",
    };
    let target = match draft.target {
        ActionTarget::WholeSwarm => "whole Swarm",
        ActionTarget::SelectedMemberWork => "selected member's current work",
    };
    format!(
        "AUTHORING {kind} → {target} · {} · Tab target · Enter stage · Esc cancel",
        bounded_display(&draft.text, 80)
    )
}

fn configuration_draft(
    opened: &OpenedSwarm,
    state: &TerminalState,
) -> Result<ConfigurationDraft, String> {
    let member = opened
        .view
        .roster
        .get(state.selected_member)
        .ok_or_else(|| "select a roster member before authoring configuration".to_owned())?;
    let current = authoritative_roster_member(opened, &member.member_key)?;
    let expected_configuration_revision = current
        .get("configuration_revision")
        .and_then(serde_json::Value::as_u64)
        .filter(|revision| *revision > 0)
        .ok_or_else(|| "selected member configuration revision is unavailable".to_owned())?;
    Ok(ConfigurationDraft {
        member_key: member.member_key.clone(),
        expected_configuration_revision,
        text: String::new(),
    })
}

fn authoritative_roster_member<'a>(
    opened: &'a OpenedSwarm,
    member_key: &str,
) -> Result<&'a serde_json::Value, String> {
    opened
        .observation
        .as_ref()
        .ok_or_else(|| {
            "configuration authoring requires a current authorized observation".to_owned()
        })?
        .activity
        .get("roster")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|member| field_text(member, &["member_key"]) == Some(member_key))
        .ok_or_else(|| "selected member is absent from the authoritative roster".to_owned())
}

fn author_configuration_action(
    opened: &OpenedSwarm,
    draft: &ConfigurationDraft,
) -> Result<ExactSwarmAction, String> {
    let observation = opened.observation.as_ref().ok_or_else(|| {
        "configuration authoring requires a current authorized observation".to_owned()
    })?;
    let current = authoritative_roster_member(opened, &draft.member_key)?;
    let current_revision = current
        .get("configuration_revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "selected member configuration revision is unavailable".to_owned())?;
    if current_revision != draft.expected_configuration_revision {
        return Err(format!(
            "configuration changed from r{} to r{current_revision}; refresh and author again",
            draft.expected_configuration_revision
        ));
    }
    let fields = draft.text.split('|').map(str::trim).collect::<Vec<_>>();
    let [
        provider,
        requested_model,
        requested_effort,
        alias_acknowledgement,
    ] = fields.as_slice()
    else {
        return Err(
            "configuration requires: provider | model | effort-or-- | ack-or-unack".to_owned(),
        );
    };
    if !matches!(*provider, "codex" | "claude" | "kiro" | "controlled") {
        return Err("provider must be codex, claude, kiro, or controlled".to_owned());
    }
    if requested_model.is_empty() {
        return Err("requested model cannot be empty".to_owned());
    }
    let moving_alias_acknowledged = match *alias_acknowledgement {
        "ack" => true,
        "unack" => false,
        _ => return Err("moving-alias choice must explicitly be ack or unack".to_owned()),
    };
    let offer = observation
        .action_offers
        .iter()
        .find(|offer| offer.action_type == "update_member_configuration")
        .ok_or_else(|| "update_member_configuration is not currently offered".to_owned())?;
    let mut payload = serde_json::json!({
        "expected_configuration_revision": current_revision,
        "member_key": draft.member_key,
        "moving_alias_acknowledged": moving_alias_acknowledged,
        "provider": provider,
        "requested_model": requested_model,
    });
    if !requested_effort.is_empty() && *requested_effort != "-" {
        payload["requested_effort"] = serde_json::Value::String((*requested_effort).to_owned());
    }
    Ok(ExactSwarmAction {
        actor: SwarmActor::HumanCoordinator,
        action_id: next_tui_action_id()?,
        based_on_room_seq: observation.room_seq,
        offer_id: offer.offer_id.clone(),
        action_type: "update_member_configuration".to_owned(),
        payload_schema_digest: offer.payload_schema_digest.clone(),
        payload,
    })
}

fn author_action(
    opened: &OpenedSwarm,
    state: &TerminalState,
    draft: &ActionDraft,
) -> Result<ExactSwarmAction, String> {
    if draft.text.trim().is_empty() {
        return Err("Action text cannot be empty".to_owned());
    }
    let observation = opened
        .observation
        .as_ref()
        .ok_or_else(|| "authoring requires an authorized current observation".to_owned())?;
    let action_type = match draft.kind {
        AuthoredActionKind::Suggestion => "submit_suggestion",
        AuthoredActionKind::Direction => "issue_direction",
    };
    let offer = observation
        .action_offers
        .iter()
        .find(|offer| offer.action_type == action_type)
        .ok_or_else(|| format!("{action_type} is not currently offered"))?;
    let targets = match draft.target {
        ActionTarget::WholeSwarm => Vec::new(),
        ActionTarget::SelectedMemberWork => selected_member_work(opened, state)?,
    };
    let scope = if targets.is_empty() { "goal" } else { "work" };
    let action_id = next_tui_action_id()?;
    let payload = match draft.kind {
        AuthoredActionKind::Suggestion => serde_json::json!({
            "suggestion_id": action_id,
            "summary": draft.text,
            "target_scope": scope,
            "target_work_ids": targets,
        }),
        AuthoredActionKind::Direction => serde_json::json!({
            "direction_id": action_id,
            "expected_direction_revision": current_direction_revision(observation),
            "instruction": draft.text,
            "target_scope": scope,
            "target_work_ids": targets,
        }),
    };
    Ok(ExactSwarmAction {
        actor: SwarmActor::HumanCoordinator,
        action_id,
        based_on_room_seq: observation.room_seq,
        offer_id: offer.offer_id.clone(),
        action_type: action_type.to_owned(),
        payload_schema_digest: offer.payload_schema_digest.clone(),
        payload,
    })
}

fn selected_member_work(
    opened: &OpenedSwarm,
    state: &TerminalState,
) -> Result<Vec<String>, String> {
    let member = opened
        .view
        .roster
        .get(state.selected_member)
        .ok_or_else(|| "select a member with current work".to_owned())?;
    let observation = opened
        .observation
        .as_ref()
        .ok_or_else(|| "current work requires an authorized observation".to_owned())?;
    let member_id = observation
        .activity
        .get("roster")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flat_map(|items| items.iter())
        .find(|item| field_text(item, &["member_key"]) == Some(member.member_key.as_str()))
        .and_then(|item| field_text(item, &["member_id"]))
        .ok_or_else(|| "selected member identity is absent from the current view".to_owned())?;
    let work = opened
        .observation
        .as_ref()
        .and_then(|observation| observation.activity.get("work_items"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            (field_text(item, &["owner_member_id"]) == Some(member_id)
                || field_text(item, &["assigned_member_key"]) == Some(member.member_key.as_str()))
                && !matches!(
                    field_text(item, &["status"]),
                    Some("completed" | "superseded")
                )
        })
        .filter_map(|item| field_text(item, &["work_id", "id"]).map(str::to_owned))
        .collect::<Vec<_>>();
    if work.is_empty() {
        Err("selected member has no current work".to_owned())
    } else {
        Ok(work)
    }
}

fn current_direction_revision(observation: &SwarmObservation) -> u64 {
    observation
        .activity
        .get("directions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("direction_revision")
                .and_then(serde_json::Value::as_u64)
        })
        .max()
        .unwrap_or(0)
}

fn next_tui_action_id() -> Result<String, String> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut value = action_identity_value()?;
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec())
        .map_err(|_| "could not create an Action identity".to_owned())
}

#[cfg(feature = "managed-local-runtime")]
fn action_identity_value() -> Result<u128, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "could not create an Action identity".to_owned())?;
    Ok(u128::from_be_bytes(bytes) >> 2)
}

#[cfg(not(feature = "managed-local-runtime"))]
fn action_identity_value() -> Result<u128, String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "could not create an Action identity".to_owned())?
        .as_millis();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let process_sequence = (u64::from(std::process::id()) << 32) | (sequence & 0xffff_ffff);
    Ok((time << 64) | u128::from(process_sequence))
}

fn action_preview(action: &ExactSwarmAction) -> String {
    if action.action_type == "update_member_configuration" {
        let member = action
            .payload
            .get("member_key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unidentified member");
        let revision = action
            .payload
            .get("expected_configuration_revision")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let provider = action
            .payload
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unreported provider");
        let model = action
            .payload
            .get("requested_model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unreported model");
        let effort = action
            .payload
            .get("requested_effort")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("none");
        let alias = if action
            .payload
            .get("moving_alias_acknowledged")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            "ack"
        } else {
            "unack"
        };
        return format!(
            "STAGED CONFIG {member} r{revision}→r{} · {provider}/{model} · effort {effort} · alias {alias} · Enter confirm · Esc cancel",
            revision.saturating_add(1)
        );
    }
    let scope = action
        .payload
        .get("target_scope")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("declared payload");
    let targets = action
        .payload
        .get("target_work_ids")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "whole scope".to_owned());
    format!(
        "STAGED {} → {scope}:{targets} · Enter confirm · Esc cancel",
        action.action_type
    )
}

fn policy_preview(policy: &ExecutionPolicy) -> String {
    match policy {
        ExecutionPolicy::SetProviderCap { provider, limit } => {
            format!("provider {provider} cap {limit}")
        }
        ExecutionPolicy::SetPriority { priority } => format!("Swarm priority {priority}"),
        ExecutionPolicy::SetBudget {
            invocation_limit,
            active_time_limit_ms,
        } => format!(
            "Swarm budget invocations {} · active-ms {}",
            invocation_limit.map_or_else(|| "unlimited".to_owned(), |value| value.to_string()),
            active_time_limit_ms.map_or_else(|| "unlimited".to_owned(), |value| value.to_string())
        ),
    }
}

fn action_status(receipt: &SwarmActionReceipt) -> String {
    match receipt {
        SwarmActionReceipt::Accepted {
            action_id,
            room_seq,
            duplicate,
        } => format!(
            "Action {action_id} accepted at head {room_seq}{}",
            if *duplicate {
                " (duplicate receipt)"
            } else {
                ""
            }
        ),
        SwarmActionReceipt::Rejected {
            action_id,
            code,
            current_room_seq,
            ..
        } => format!("Action {action_id} rejected: {code} · current head {current_room_seq}"),
    }
}

struct OpenedSwarm {
    view: SwarmView,
    observation: Option<SwarmObservation>,
    execution: Option<ExecutionView>,
}

fn open_swarm<B: SwarmBackend, E: ExecutionControls>(
    app: &SwarmApplication<B>,
    execution: &E,
    swarm_id: &crate::SwarmId,
) -> Result<OpenedSwarm, TuiError> {
    observe_swarm(app, execution, app.open(swarm_id)?)
}

fn observe_swarm<B: SwarmBackend, E: ExecutionControls>(
    app: &SwarmApplication<B>,
    execution: &E,
    view: SwarmView,
) -> Result<OpenedSwarm, TuiError> {
    let observation = match app.observe(&view.swarm_id, &SwarmActor::HumanCoordinator) {
        Ok(observation) => Some(observation),
        Err(ApplicationError::Backend(BackendError::Unsupported)) => None,
        Err(error) => return Err(error.into()),
    };
    let execution = execution
        .status(&view.swarm_id)
        .map_err(TuiError::Execution)?;
    Ok(OpenedSwarm {
        view,
        observation,
        execution,
    })
}

pub struct CrosstermSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    entered: bool,
}
impl CrosstermSession {
    /// # Errors
    /// Returns a terminal construction failure.
    pub fn open() -> Result<Self, TuiError> {
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            entered: false,
        })
    }
}
impl TuiSession for CrosstermSession {
    fn enter(&mut self) -> Result<(), TuiError> {
        enable_raw_mode()?;
        if let Err(error) = execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste
        ) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        self.entered = true;
        Ok(())
    }
    fn draw(&mut self, state: &TerminalState, dashboard: Dashboard<'_>) -> Result<(), TuiError> {
        self.terminal
            .draw(|frame| render_dashboard(frame, state, dashboard))?;
        Ok(())
    }
    fn next_input(&mut self) -> Result<Input, TuiError> {
        loop {
            if let Some(input) = input_from_event(&event::read()?) {
                return Ok(input);
            }
        }
    }
    fn restore(&mut self) -> Result<(), TuiError> {
        if !self.entered {
            return Ok(());
        }
        let cursor = self.terminal.show_cursor();
        let leave = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let raw = disable_raw_mode();
        self.entered = false;
        cursor.and(leave).and(raw)?;
        Ok(())
    }
}

/// Maps one native terminal event into an application input.
///
/// Key releases, unsupported keys, mouse events, and focus events are ignored.
#[must_use]
pub fn input_from_event(event: &Event) -> Option<Input> {
    match event {
        Event::Paste(text) => Some(Input::Paste { text: text.clone() }),
        Event::Resize(width, height) => Some(Input::Resize {
            width: *width,
            height: *height,
        }),
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Up => Some(Input::Up),
            KeyCode::Down => Some(Input::Down),
            KeyCode::Enter => Some(Input::Open),
            KeyCode::Esc => Some(Input::Home),
            KeyCode::Tab => Some(Input::ToggleTarget),
            KeyCode::Backspace => Some(Input::Backspace),
            KeyCode::Char('g') => Some(Input::BeginSuggestion),
            KeyCode::Char('d') => Some(Input::BeginDirection),
            KeyCode::Char('c') => Some(Input::BeginConfiguration),
            KeyCode::Char('e') => Some(Input::BeginExecutionPolicy),
            KeyCode::Char('i') => Some(Input::InspectArtifact),
            KeyCode::Char('n') => Some(Input::New),
            KeyCode::Char('r') => Some(Input::Refresh),
            KeyCode::Char('a') => Some(Input::StageNextAction),
            KeyCode::Char('l') => Some(Input::StageNextPolicy),
            KeyCode::Char('p') => Some(Input::Pause),
            KeyCode::Char('s') => Some(Input::Stop),
            KeyCode::Char('u') => Some(Input::Resume),
            KeyCode::Char('q') => Some(Input::Quit),
            KeyCode::Char(value) => Some(Input::Text {
                text: value.to_string(),
            }),
            _ => None,
        },
        _ => None,
    }
}
impl Drop for CrosstermSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub struct ScriptedSession {
    inputs: VecDeque<Input>,
    terminal: Terminal<TestBackend>,
    entered: bool,
    restored: bool,
    cursor_shown: bool,
    draws: usize,
    statuses: Vec<String>,
    frames: Vec<String>,
}
impl ScriptedSession {
    /// # Errors
    /// Propagates terminal construction failures.
    pub fn new(inputs: Vec<Input>, width: u16, height: u16) -> Result<Self, TuiError> {
        Ok(Self {
            inputs: inputs.into(),
            terminal: infallible(Terminal::new(TestBackend::new(width, height))),
            entered: false,
            restored: false,
            cursor_shown: false,
            draws: 0,
            statuses: Vec::new(),
            frames: Vec::new(),
        })
    }
    #[must_use]
    pub const fn restored(&self) -> bool {
        self.restored
    }
    #[must_use]
    pub const fn draws(&self) -> usize {
        self.draws
    }
    #[must_use]
    pub const fn cursor_shown(&self) -> bool {
        self.cursor_shown
    }
    #[must_use]
    pub fn statuses(&self) -> &[String] {
        &self.statuses
    }
    #[must_use]
    pub fn frames(&self) -> &[String] {
        &self.frames
    }
}
impl TuiSession for ScriptedSession {
    fn enter(&mut self) -> Result<(), TuiError> {
        self.entered = true;
        Ok(())
    }
    fn draw(&mut self, state: &TerminalState, dashboard: Dashboard<'_>) -> Result<(), TuiError> {
        self.statuses.push(state.status.clone());
        infallible(
            self.terminal
                .resize(Rect::new(0, 0, state.width, state.height)),
        );
        infallible(
            self.terminal
                .draw(|frame| render_dashboard(frame, state, dashboard)),
        );
        self.frames.push(
            self.terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect(),
        );
        self.draws += 1;
        Ok(())
    }
    fn next_input(&mut self) -> Result<Input, TuiError> {
        self.inputs.pop_front().ok_or(TuiError::ScriptExhausted)
    }
    fn restore(&mut self) -> Result<(), TuiError> {
        self.entered = false;
        self.cursor_shown = true;
        self.restored = true;
        Ok(())
    }
}

fn infallible<T>(result: Result<T, Infallible>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => match error {},
    }
}

fn render_dashboard(frame: &mut Frame<'_>, state: &TerminalState, dashboard: Dashboard<'_>) {
    match dashboard {
        Dashboard::List(items) => render_list(frame, state, items),
        Dashboard::Swarm {
            view,
            observation,
            execution,
        } => {
            render_swarm(frame, state, view, observation, execution);
        }
    }
}
fn render_list(frame: &mut Frame<'_>, state: &TerminalState, summaries: &[SwarmSummary]) {
    let constraints = if state.creation_form.is_some() {
        vec![
            Constraint::Min(3),
            Constraint::Length(8),
            Constraint::Length(3),
        ]
    } else {
        vec![Constraint::Min(3), Constraint::Length(3)]
    };
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());
    let rows = summaries.iter().enumerate().map(|(i, s)| {
        Row::new(vec![
            Cell::from(s.swarm_id.as_str().to_owned()),
            Cell::from(s.room_id.clone()),
            Cell::from(s.goal.clone()),
            Cell::from(s.member_count.to_string()),
        ])
        .style(if i == state.selected_swarm {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        })
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(28),
            Constraint::Length(28),
            Constraint::Min(24),
            Constraint::Length(8),
        ],
    )
    .header(Row::new(["SWARM", "ROOM", "GOAL", "MEMBERS"]))
    .block(
        Block::default()
            .title("AGENT SWARMS · n new · enter open · r refresh · q detach")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, areas[0]);
    let status_area = if let Some(form) = &state.creation_form {
        frame.render_widget(
            Paragraph::new(form.as_str())
                .wrap(Wrap { trim: true })
                .block(Block::default().title("AUTHORING").borders(Borders::ALL)),
            areas[1],
        );
        areas[2]
    } else {
        areas[1]
    };
    frame.render_widget(
        Paragraph::new(state.status.as_str())
            .wrap(Wrap { trim: true })
            .block(Block::default().title("STATUS").borders(Borders::ALL)),
        status_area,
    );
}

pub fn render(frame: &mut Frame<'_>, state: &TerminalState, swarm: &SwarmView) {
    render_swarm(frame, state, swarm, None, None);
}

/// Renders authoritative workflow facts while keeping supervised process
/// activity visually separate from Room state.
pub fn render_observed(
    frame: &mut Frame<'_>,
    state: &TerminalState,
    observation: &SwarmObservation,
) {
    render_swarm(frame, state, &observation.swarm, Some(observation), None);
}

fn render_swarm(
    frame: &mut Frame<'_>,
    state: &TerminalState,
    swarm: &SwarmView,
    observation: Option<&SwarmObservation>,
    execution: Option<&ExecutionView>,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(7),
            Constraint::Length(3),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " AGENT SWARM ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(&swarm.source_label, Style::default().fg(Color::Yellow)),
        ]))
        .block(Block::default().borders(Borders::ALL)),
        rows[0],
    );
    let overview = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(rows[1]);
    let details = format!(
        "{}\nconstraints: {}\nacceptance: {}",
        swarm.goal,
        swarm.constraints.join(" · "),
        swarm
            .acceptance_criteria
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    );
    frame.render_widget(
        panel("GOAL · CONSTRAINTS · ACCEPTANCE", &details),
        overview[0],
    );
    frame.render_widget(
        panel(
            "ROOM FACTS",
            &format!(
                "swarm  {}\nroom   {}\narea   {}\nreview every {}s · correction limit {}",
                swarm.swarm_id.as_str(),
                swarm.room_id,
                swarm.working_area.display(),
                swarm.progress_review_interval_seconds,
                swarm.correction_failure_limit
            ),
        ),
        overview[1],
    );
    if let Some(observation) = observation {
        render_observation_content(frame, rows[2], state, swarm, observation, execution);
    } else {
        render_roster(frame, rows[2], state, swarm);
    }
    frame.render_widget(
        Paragraph::new(format!(
            "↑/↓ select · g/d suggest/direct · c config · e policy · tab target/type · i detail · a/l planned · enter stage/confirm · esc cancel · q detach · {}",
            state.status
        ))
            .block(Block::default().borders(Borders::ALL)),
        rows[3],
    );
}

fn render_observation_content(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &TerminalState,
    swarm: &SwarmView,
    observation: &SwarmObservation,
    execution: Option<&ExecutionView>,
) {
    if state.workflow_detail {
        let title = if state.artifact_preview.is_some() {
            "AUTHORITATIVE ROOM DETAIL · VERIFIED ARTIFACT PREVIEW"
        } else {
            "AUTHORITATIVE ROOM DETAIL · OBSERVED REFERENCES ONLY"
        };
        frame.render_widget(
            panel(title, &workflow_detail(observation, state, swarm)),
            area,
        );
        return;
    }
    let content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(area);
    render_roster(frame, content[0], state, swarm);
    let workflow = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(content[1]);
    frame.render_widget(
        panel(
            "AUTHORITATIVE ROOM WORKFLOW",
            &workflow_summary(observation),
        ),
        workflow[0],
    );
    frame.render_widget(
        panel(
            "SUPERVISED EXECUTION · NOT ROOM FACTS",
            &execution_summary(execution),
        ),
        workflow[1],
    );
}

fn execution_summary(execution: Option<&ExecutionView>) -> String {
    execution.map_or_else(
        || {
            "execution daemon not attached\nRoom activity does not imply a worker is running"
                .to_owned()
        },
        |view| {
            let coordinator = coordinator_summary(view.coordinator.as_ref());
            format!(
                "{}\n{} · desired {} · priority {}\nactive {} · queued {} · started {}\nbudget invocations {} · active-ms {}\ncaps {} · unknown effects {}",
                coordinator,
                view.phase,
                view.desired,
                view.priority,
                view.active,
                view.queued,
                view.invocations_started,
                view.invocation_limit.map_or_else(|| "unlimited".to_owned(), |value| value.to_string()),
                view.active_time_limit_ms.map_or_else(|| "unlimited".to_owned(), |value| value.to_string()),
                view.provider_caps.iter().map(|(provider, limit)| format!("{provider}:{limit}")).collect::<Vec<_>>().join(", "),
                view.unknown_effects,
            )
        },
    )
}

fn coordinator_summary(coordinator: Option<&CoordinatorServiceView>) -> String {
    let Some(coordinator) = coordinator else {
        return "coordinator journal not attached".to_owned();
    };
    let mut summary = format!(
        "coordinator last {} · pending {} · attention {}",
        coordinator_loop_label(coordinator.loop_state),
        coordinator.pending_count,
        coordinator.manual_attention_count
    );
    if let Some(review) = coordinator.progress_reviews.first() {
        let owner = review.member_key.as_deref().map_or_else(
            || "unassigned".to_owned(),
            |member| format!("worker {member}"),
        );
        let _ = write!(
            summary,
            "\nprogress review {} · {} · {} · {}",
            review.review_id,
            progress_review_authoritative_label(review.authoritative_status),
            progress_review_execution_label(review.execution_status),
            owner
        );
    }
    if let Some(autonomy) = &coordinator.autonomy {
        let _ = write!(
            summary,
            "\nAdaptive planning: {} · {}/{} Invocations · {}",
            autonomy.phase,
            autonomy.generated_invocation_count,
            autonomy.invocation_limit,
            autonomy.reason
        );
    }
    let Some(latest) = coordinator.invocations.last() else {
        if coordinator.autonomy.is_none() {
            summary.push_str(
                "\nNo worker plans recorded. Enable adaptive planning or stage a worker plan.",
            );
        }
        return summary;
    };
    let evidence = match latest.provider_evidence.state {
        ProviderEffectiveState::Verified => format!(
            "verified model {} · effort {} · reported session {}",
            latest
                .provider_evidence
                .effective_model
                .as_deref()
                .unwrap_or("unavailable"),
            latest
                .provider_evidence
                .effective_effort
                .as_deref()
                .unwrap_or("none"),
            latest
                .provider_evidence
                .reported_session_id
                .as_deref()
                .unwrap_or("none")
        ),
        ProviderEffectiveState::AwaitingReport => {
            "effective configuration awaiting report".to_owned()
        }
        ProviderEffectiveState::Unreported => "effective configuration unreported".to_owned(),
        ProviderEffectiveState::Mismatch => "effective configuration mismatch".to_owned(),
        ProviderEffectiveState::Unsupported => "effective configuration unsupported".to_owned(),
    };
    format!(
        "{summary}\n{} · {} · {evidence}",
        latest.member_key,
        coordinator_outcome_label(&latest.coordinator_outcome)
    )
}

const fn progress_review_authoritative_label(
    status: crate::ProgressReviewAuthoritativeStatus,
) -> &'static str {
    match status {
        crate::ProgressReviewAuthoritativeStatus::Due => "due",
        crate::ProgressReviewAuthoritativeStatus::Claimed => "claimed",
        crate::ProgressReviewAuthoritativeStatus::Blocked => "blocked",
    }
}

const fn progress_review_execution_label(
    status: crate::ProgressReviewExecutionStatus,
) -> &'static str {
    match status {
        crate::ProgressReviewExecutionStatus::Due => "eligible when running",
        crate::ProgressReviewExecutionStatus::Claimed => "claimed; eligible when running",
        crate::ProgressReviewExecutionStatus::WaitingForCapacity => "waiting for capacity",
        crate::ProgressReviewExecutionStatus::Running => "running",
        crate::ProgressReviewExecutionStatus::Blocked => "blocked",
        crate::ProgressReviewExecutionStatus::NoEligibleProvider => "no eligible provider",
        crate::ProgressReviewExecutionStatus::ManualReconciliationRequired => {
            "manual reconciliation required"
        }
    }
}

const fn coordinator_loop_label(state: crate::CoordinatorLoopState) -> &'static str {
    match state {
        crate::CoordinatorLoopState::Idle => "idle",
        crate::CoordinatorLoopState::Running => "running",
        crate::CoordinatorLoopState::WaitingForCapacity => "waiting for capacity",
        crate::CoordinatorLoopState::WaitingForExplicitResume => "waiting for explicit resume",
        crate::CoordinatorLoopState::RecoveryRequired => "recovery required",
        crate::CoordinatorLoopState::ManualReconciliationRequired => {
            "manual reconciliation required"
        }
    }
}

fn coordinator_outcome_label(outcome: &CoordinatorOutcome) -> String {
    match outcome {
        CoordinatorOutcome::Pending => "pending".to_owned(),
        CoordinatorOutcome::Running => "running".to_owned(),
        CoordinatorOutcome::ProcessingResult => "processing result".to_owned(),
        CoordinatorOutcome::LaunchUncertain => "launch uncertain".to_owned(),
        CoordinatorOutcome::NeedsReevaluation => "needs reevaluation".to_owned(),
        CoordinatorOutcome::SubmissionUncertain => "submission uncertain".to_owned(),
        CoordinatorOutcome::Accepted { duplicate } => {
            format!("accepted · duplicate {duplicate}")
        }
        CoordinatorOutcome::Rejected {
            code, duplicate, ..
        } => format!("rejected {code} · duplicate {duplicate}"),
        CoordinatorOutcome::NotSubmitted { reason } => format!("not submitted {reason}"),
        CoordinatorOutcome::SettledWithoutSubmission => "settled without submission".to_owned(),
        CoordinatorOutcome::PlanningDecisionRecorded => "planning decision retained".to_owned(),
    }
}

fn workflow_detail(
    observation: &SwarmObservation,
    state: &TerminalState,
    swarm: &SwarmView,
) -> String {
    let Some(activity) = observation.activity.as_object() else {
        return "authoritative activity is unavailable".to_owned();
    };
    let mut lines = current_work_detail(activity, state, swarm);
    lines.extend(candidate_detail(activity));
    lines.extend(check_detail(activity));
    lines.push(review_detail(activity));
    lines.push(finding_detail(activity));
    lines.extend(problem_detail(activity));
    lines.truncate(14);
    if let Some(preview) = &state.artifact_preview {
        lines.push(format!(
            "verified preview: {} · {} · {} bytes · {}{}",
            preview.path,
            preview.media_type,
            preview.byte_length,
            bounded_display(&preview.digest, 32),
            if preview.truncated {
                " · truncated"
            } else {
                ""
            }
        ));
        lines.extend(
            preview
                .content
                .lines()
                .take(10)
                .map(|line| format!("│ {line}")),
        );
    }
    lines.truncate(25);
    lines.join("\n")
}

fn current_work_detail(
    activity: &serde_json::Map<String, serde_json::Value>,
    state: &TerminalState,
    swarm: &SwarmView,
) -> Vec<String> {
    let selected_key = swarm
        .roster
        .get(state.selected_member)
        .map(|member| member.member_key.as_str());
    let selected_member_id = selected_key.and_then(|key| {
        activity
            .get("roster")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .find(|member| field_text(member, &["member_key"]) == Some(key))
            .and_then(|member| field_text(member, &["member_id"]))
    });
    let work_items = activity
        .get("work_items")
        .and_then(serde_json::Value::as_array);
    let work = work_items
        .into_iter()
        .flatten()
        .filter(|work| is_current_work(work))
        .find(|work| {
            selected_member_id
                .is_some_and(|member_id| field_text(work, &["owner_member_id"]) == Some(member_id))
                || selected_key.is_some_and(|member_key| {
                    field_text(work, &["assigned_member_key"]) == Some(member_key)
                })
        })
        .or_else(|| {
            work_items
                .into_iter()
                .flatten()
                .find(|work| is_current_work(work))
        });
    if let Some(work) = work {
        vec![
            format!(
                "work {} r{} · {} · owner {} · status {}",
                bounded_display(
                    field_text(work, &["work_id", "id"]).unwrap_or("unidentified"),
                    40
                ),
                display_revision(work, "revision"),
                field_text(work, &["kind"]).unwrap_or("unreported kind"),
                bounded_display(
                    field_text(work, &["owner_member_id", "assigned_member_key"])
                        .unwrap_or("unclaimed"),
                    32
                ),
                field_text(work, &["status"]).unwrap_or("unreported")
            ),
            format!(
                "dependencies: {}",
                bounded_string_array(work.get("dependency_ids"), 6, 32)
            ),
        ]
    } else {
        vec![format!(
            "work for {}: none current",
            selected_key.unwrap_or("selected member")
        )]
    }
}

fn is_current_work(work: &serde_json::Value) -> bool {
    !matches!(
        field_text(work, &["status"]),
        Some("completed" | "superseded")
    )
}

fn candidate_detail(activity: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    if let Some(candidate) = latest_activity_item(activity, "candidates") {
        let mut lines = vec![format!(
            "latest candidate {} v{} · work {}",
            bounded_display(
                field_text(candidate, &["candidate_id", "id"]).unwrap_or("unidentified"),
                36
            ),
            display_revision(candidate, "version"),
            bounded_display(
                field_text(candidate, &["work_id"]).unwrap_or("unreported"),
                36
            )
        )];
        if let Some(artifact) = candidate.get("artifact") {
            lines.push(artifact_detail("candidate artifact", artifact));
        }
        lines
    } else {
        let mut lines = vec!["latest candidate: none".to_owned()];
        if let Some(contribution) = latest_activity_item(activity, "contributions")
            && let Some(artifact) = contribution.get("artifact")
        {
            lines.push(artifact_detail("contribution artifact", artifact));
        }
        lines
    }
}

fn check_detail(activity: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    if let Some(check) = latest_activity_item(activity, "checks") {
        let mut lines = vec![format!(
            "latest check {} r{} · {} · criterion {}",
            bounded_display(
                field_text(check, &["check_id", "id"]).unwrap_or("unidentified"),
                32
            ),
            display_revision(check, "revision"),
            field_text(check, &["status"]).unwrap_or("unreported"),
            bounded_display(
                field_text(check, &["criterion"]).unwrap_or("unreported"),
                64
            )
        )];
        if let Some(artifact) = check
            .get("evidence_refs")
            .and_then(serde_json::Value::as_array)
            .and_then(|evidence| evidence.last())
            .and_then(|evidence| evidence.get("artifact"))
        {
            lines.push(artifact_detail("check evidence", artifact));
        }
        lines
    } else {
        vec!["latest check: none".to_owned()]
    }
}

fn review_detail(activity: &serde_json::Map<String, serde_json::Value>) -> String {
    if let Some(review) = latest_activity_item(activity, "reviews") {
        format!(
            "latest review {} r{} · {} · reviewer {} · findings {}",
            bounded_display(
                field_text(review, &["review_id", "id"]).unwrap_or("unidentified"),
                32
            ),
            display_revision(review, "revision"),
            field_text(review, &["verdict", "status"]).unwrap_or("unreported"),
            bounded_display(
                field_text(review, &["reviewer_member_id"]).unwrap_or("unreported"),
                28
            ),
            bounded_string_array(review.get("finding_ids"), 4, 24)
        )
    } else {
        "latest review: none".to_owned()
    }
}

fn finding_detail(activity: &serde_json::Map<String, serde_json::Value>) -> String {
    if let Some(finding) = latest_activity_item(activity, "findings") {
        format!(
            "latest finding {} r{} · {}/{} · correction {} · {}",
            bounded_display(
                field_text(finding, &["finding_id", "id"]).unwrap_or("unidentified"),
                30
            ),
            display_revision(finding, "revision"),
            field_text(finding, &["severity"]).unwrap_or("unreported"),
            field_text(finding, &["status"]).unwrap_or("unreported"),
            bounded_display(
                field_text(finding, &["corrective_work_id"]).unwrap_or("none"),
                28
            ),
            bounded_display(
                field_text(finding, &["summary", "reason"]).unwrap_or("reason unreported"),
                72
            )
        )
    } else {
        "latest finding: none".to_owned()
    }
}

fn problem_detail(activity: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    if let Some(problem) = latest_activity_item(activity, "problems") {
        vec![
            format!(
                "latest problem {} r{} · {} · failed corrections {} · reason {}",
                bounded_display(
                    field_text(problem, &["problem_id", "id"]).unwrap_or("unidentified"),
                    30
                ),
                display_revision(problem, "revision"),
                field_text(problem, &["status"]).unwrap_or("unreported"),
                display_revision(problem, "failed_corrections"),
                bounded_display(
                    field_text(problem, &["summary", "reason"]).unwrap_or("unreported"),
                    72
                )
            ),
            format!(
                "correction attempts: {} · evidence: {}",
                bounded_string_array(problem.get("recorded_correction_attempt_ids"), 5, 24),
                bounded_string_array(problem.get("evidence_refs"), 5, 30)
            ),
        ]
    } else {
        vec!["latest problem: none".to_owned()]
    }
}

fn display_revision(value: &serde_json::Value, field: &str) -> String {
    field_u64(value, &[field]).map_or_else(|| "?".to_owned(), |revision| revision.to_string())
}

fn latest_activity_item<'a>(
    activity: &'a serde_json::Map<String, serde_json::Value>,
    collection: &str,
) -> Option<&'a serde_json::Value> {
    activity
        .get(collection)
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.last())
}

fn artifact_detail(label: &str, artifact: &serde_json::Value) -> String {
    format!(
        "{label}: {} · {} · {}",
        bounded_display(
            field_text(artifact, &["local_path", "path"]).unwrap_or("path unreported"),
            64
        ),
        bounded_display(
            field_text(artifact, &["digest", "content_digest"]).unwrap_or("digest unreported"),
            42
        ),
        bounded_display(
            field_text(artifact, &["media_type"]).unwrap_or("media type unreported"),
            56
        )
    )
}

fn bounded_string_array(
    value: Option<&serde_json::Value>,
    maximum_items: usize,
    maximum_chars: usize,
) -> String {
    let values = value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(maximum_items)
        .filter_map(serde_json::Value::as_str)
        .map(|value| bounded_display(value, maximum_chars))
        .collect::<Vec<_>>();
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(",")
    }
}

fn field_u64(value: &serde_json::Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_u64))
}

fn workflow_summary(observation: &SwarmObservation) -> String {
    let activity = observation.activity.as_object();
    let lifecycle = activity
        .and_then(|value| {
            value
                .get("phase")
                .or_else(|| value.get("lifecycle"))
                .or_else(|| value.get("status"))
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unreported");
    let count = |names: &[&str]| -> usize {
        names
            .iter()
            .filter_map(|name| activity.and_then(|value| value.get(*name)))
            .map(|value| {
                value.as_array().map_or_else(
                    || value.as_object().map_or(0, serde_json::Map::len),
                    Vec::len,
                )
            })
            .find(|count| *count > 0)
            .unwrap_or(0)
    };
    let hash = observation
        .authoritative_state_hash
        .strip_prefix("blake3:")
        .unwrap_or(&observation.authoritative_state_hash);
    let short_hash = &hash[..hash.len().min(12)];
    let progress_reviews = activity
        .and_then(|value| {
            value
                .get("outstanding_progress_review")
                .or_else(|| value.get("outstanding_progress_reviews"))
                .or_else(|| value.get("progress_review_obligations"))
                .or_else(|| value.get("progress_reviews"))
        })
        .map_or(0, |value| {
            if value.is_null() || value == &serde_json::Value::Bool(false) {
                0
            } else if let Some(items) = value.as_array() {
                items.len()
            } else {
                1
            }
        });
    let mut summary = format!(
        "{lifecycle} · head {} · {}\nwork {} · contributions {} · candidates {}\nchecks {} · reviews {} · results {}\ndirections {} · blockers {} · progress reviews {}",
        observation.room_seq,
        short_hash,
        count(&["work_items", "work"]),
        count(&["contributions"]),
        count(&["candidates"]),
        count(&["checks"]),
        count(&["reviews"]),
        count(&["results", "accepted_results"]),
        count(&["directions"]),
        count(&["blockers", "problems"]),
        progress_reviews,
    );
    for highlight in workflow_highlights(activity) {
        summary.push('\n');
        summary.push_str(&highlight);
    }
    summary
}

fn workflow_highlights(
    activity: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Vec<String> {
    let Some(activity) = activity else {
        return Vec::new();
    };

    [
        next_work_highlight(activity),
        unresolved_blocker_highlight(activity),
        unresolved_finding_highlight(activity),
        pending_suggestion_highlight(activity),
        latest_artifact_highlight(activity),
        latest_review_highlight(activity),
    ]
    .into_iter()
    .flatten()
    .take(4)
    .collect()
}

fn next_work_highlight(activity: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let work = activity
        .get("work_items")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                !matches!(
                    item.get("status").and_then(serde_json::Value::as_str),
                    Some("completed" | "superseded")
                )
            })
        })?;
    let id = field_text(work, &["work_id", "id"]);
    let status = field_text(work, &["status"]);
    let owner = field_text(work, &["owner_member_id"]).unwrap_or("unclaimed");
    Some(format!(
        "next work {} · {} · owner {}",
        bounded_display(id.unwrap_or("unidentified"), 40),
        status.unwrap_or("unreported"),
        bounded_display(owner, 32)
    ))
}

fn unresolved_blocker_highlight(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let blocker = ["blockers", "problems", "resource_conflicts"]
        .iter()
        .filter_map(|collection| {
            activity
                .get(*collection)
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
        .find(|item| {
            item.get("status")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|status| status != "resolved")
        })?;
    let summary = field_text(
        blocker,
        &[
            "summary",
            "reason",
            "problem_key",
            "problem_id",
            "conflict_id",
            "resource_id",
        ],
    )
    .unwrap_or("unresolved blocker");
    Some(format!("blocked: {}", bounded_display(summary, 72)))
}

fn unresolved_finding_highlight(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let finding = activity
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().rev().find(|item| {
                item.get("status")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|status| status != "resolved")
            })
        })?;
    Some(format!(
        "review attention: {} · {}",
        field_text(finding, &["status"]).unwrap_or("unresolved"),
        bounded_display(
            field_text(finding, &["summary", "finding_id"]).unwrap_or("review finding"),
            64
        )
    ))
}

fn pending_suggestion_highlight(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let suggestion = activity
        .get("suggestions")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().rev().find(|item| {
                item.get("disposition").and_then(serde_json::Value::as_str) == Some("pending")
            })
        })?;
    Some(format!(
        "suggestion pending: {}",
        bounded_display(
            field_text(suggestion, &["summary", "suggestion_id"])
                .unwrap_or("human disposition required"),
            72
        )
    ))
}

fn latest_artifact_highlight(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let artifact = ["results", "candidates", "contributions"]
        .iter()
        .find_map(|collection| {
            activity
                .get(*collection)
                .and_then(serde_json::Value::as_array)
                .and_then(|items| items.last())
                .and_then(|item| item.get("artifact"))
        })?;
    let path = field_text(artifact, &["local_path", "path"]);
    let digest = field_text(artifact, &["digest", "content_digest"]);
    if path.is_none() && digest.is_none() {
        return None;
    }
    Some(format!(
        "artifact: {} · {}",
        bounded_display(path.unwrap_or("path unreported"), 56),
        bounded_display(digest.unwrap_or("digest unreported"), 28)
    ))
}

fn latest_review_highlight(
    activity: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let review = activity
        .get("reviews")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.last())?;
    Some(format!(
        "latest review: {} · reviewer {}",
        field_text(review, &["verdict", "status"]).unwrap_or("unreported"),
        bounded_display(
            field_text(review, &["reviewer_member_id"]).unwrap_or("unreported"),
            32
        )
    ))
}

fn authoritative_artifacts(observation: &SwarmObservation) -> Vec<AuthoritativeArtifactRef> {
    let mut references = Vec::new();
    let activity = &observation.activity;
    for artifact in [
        activity
            .get("candidates")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.last())
            .and_then(|item| item.get("artifact")),
        activity
            .get("checks")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.last())
            .and_then(|check| check.get("evidence_refs"))
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.last())
            .and_then(|evidence| evidence.get("artifact")),
        activity
            .get("results")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.last())
            .and_then(|item| item.get("artifact")),
        activity
            .get("contributions")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.last())
            .and_then(|item| item.get("artifact")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(reference) = serde_json::from_value::<AuthoritativeArtifactRef>(artifact.clone())
            && !references.contains(&reference)
        {
            references.push(reference);
        }
    }
    references
}

fn textual_media_type(media_type: &str) -> bool {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence.starts_with("text/") || essence == "application/json" || essence.ends_with("+json")
}

fn sanitized_bounded_preview(value: &str) -> (String, bool) {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut content = String::new();
    let mut truncated = false;
    let mut lines = 1_usize;
    for (characters, character) in normalized.chars().enumerate() {
        if characters >= MAX_ARTIFACT_PREVIEW_CHARS
            || (character == '\n' && lines >= MAX_ARTIFACT_PREVIEW_LINES)
        {
            truncated = true;
            break;
        }
        match character {
            '\n' => {
                content.push('\n');
                lines += 1;
            }
            '\t' => content.push('\t'),
            character if character.is_control() => content.push('�'),
            character => content.push(character),
        }
    }
    if content.is_empty() {
        content.push_str("(empty artifact)");
    }
    (content, truncated)
}

fn field_text<'a>(value: &'a serde_json::Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_str))
}

fn bounded_display(value: &str, maximum_chars: usize) -> String {
    let mut chars = value.chars();
    let bounded = chars.by_ref().take(maximum_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}
fn panel<'a>(title: &'a str, body: &'a str) -> Paragraph<'a> {
    Paragraph::new(body)
        .wrap(Wrap { trim: true })
        .block(Block::default().title(title).borders(Borders::ALL))
}
fn render_roster(frame: &mut Frame<'_>, area: Rect, state: &TerminalState, swarm: &SwarmView) {
    let header = Row::new(["MEMBER", "PROVIDER", "REQUESTED MODEL", "EFFORT", "CONFIG"]).style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let rows = swarm.roster.iter().enumerate().map(|(i, m)| {
        Row::new([
            Cell::from(m.label.clone()),
            Cell::from(m.provider.clone()),
            Cell::from(m.requested_model.clone()),
            Cell::from(
                m.requested_effort
                    .clone()
                    .unwrap_or_else(|| "unavailable".into()),
            ),
            Cell::from(authoritative_configuration_label(m)),
        ])
        .style(if i == state.selected_member {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        })
    });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(15),
                Constraint::Length(10),
                Constraint::Length(18),
                Constraint::Length(10),
                Constraint::Min(26),
            ],
        )
        .header(header)
        .column_spacing(1)
        .block(
            Block::default()
                .title("ROSTER · REQUESTED CONFIGURATION")
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn authoritative_configuration_label(member: &crate::MemberConfiguration) -> String {
    format!(
        "{} · r{} · alias:{}",
        member.configuration_state.display_label(),
        member.configuration_revision,
        if member.moving_alias_acknowledged {
            "ack"
        } else {
            "unack"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CoordinatorInvocationStatus, CoordinatorLoopState, MemberConfiguration,
        ProgressReviewAuthoritativeStatus, ProgressReviewExecutionStatus,
        ProgressReviewOperationalView, ProviderConfigurationState, ProviderOperationalView,
        SwarmId,
    };

    #[test]
    fn execution_summary_labels_verified_configuration_as_operational() {
        let summary = execution_summary(Some(&ExecutionView {
            desired: "running".to_owned(),
            phase: "running".to_owned(),
            active: 0,
            queued: 0,
            invocations_started: 1,
            unknown_effects: 0,
            priority: 1,
            invocation_limit: None,
            active_time_limit_ms: None,
            provider_caps: BTreeMap::from([("controlled".to_owned(), 1)]),
            coordinator: Some(CoordinatorServiceView {
                swarm_id: SwarmId::new("swarm-tui".to_owned())
                    .unwrap_or_else(|error| unreachable!("valid fixture: {error}")),
                revision: 3,
                autonomy: None,
                loop_state: CoordinatorLoopState::Idle,
                pending_count: 0,
                manual_attention_count: 0,
                progress_reviews: vec![ProgressReviewOperationalView {
                    review_id: "progress-review-1-1".to_owned(),
                    review_revision: 1,
                    work_id: "progress-work-1-1".to_owned(),
                    work_revision: 1,
                    authoritative_status: ProgressReviewAuthoritativeStatus::Due,
                    execution_status: ProgressReviewExecutionStatus::WaitingForCapacity,
                    member_key: Some("worker-a".to_owned()),
                    invocation_id: Some("invocation-review-1".to_owned()),
                }],
                invocations: vec![CoordinatorInvocationStatus {
                    invocation_id: "invocation-1".to_owned(),
                    member_key: "worker-a".to_owned(),
                    provider: "controlled".to_owned(),
                    configuration_revision: 1,
                    requested_model: "fixture-v1".to_owned(),
                    requested_effort: Some("medium".to_owned()),
                    provider_evidence: ProviderOperationalView {
                        state: ProviderEffectiveState::Verified,
                        effective_model: Some("fixture-v1".to_owned()),
                        effective_effort: Some("medium".to_owned()),
                        reported_model: Some("fixture-v1".to_owned()),
                        reported_effort: Some("medium".to_owned()),
                        reported_session_id: Some("session-7".to_owned()),
                    },
                    coordinator_outcome: CoordinatorOutcome::Accepted { duplicate: false },
                }],
            }),
        }));
        assert!(summary.contains("coordinator last idle"));
        assert!(summary.contains("verified model fixture-v1"));
        assert!(summary.contains("reported session session-7"));
        assert!(summary.contains("progress review progress-review-1-1 · due"));
        assert!(summary.contains("waiting for capacity"));
        assert!(summary.contains("accepted"));
        assert!(!summary.contains("Room fact"));
    }

    #[test]
    fn empty_blockers_do_not_hide_an_unresolved_problem() {
        let activity = serde_json::json!({
            "blockers": [],
            "problems": [
                {"status": "resolved", "summary": "old"},
                {"status": "escalated", "summary": "needs human attention"}
            ]
        });
        let highlights = workflow_highlights(activity.as_object());
        assert_eq!(highlights, vec!["blocked: needs human attention"]);
    }

    #[test]
    fn unresolved_findings_and_pending_suggestions_are_visible_human_attention() {
        let activity = serde_json::json!({
            "findings": [
                {"status": "resolved", "summary": "old"},
                {"status": "disputed", "summary": "independent reviewers disagree"}
            ],
            "suggestions": [
                {"disposition": "pending", "summary": "choose the safer tradeoff"}
            ]
        });
        let highlights = workflow_highlights(activity.as_object());
        assert_eq!(
            highlights,
            vec![
                "review attention: disputed · independent reviewers disagree",
                "suggestion pending: choose the safer tradeoff",
            ]
        );
    }

    #[test]
    fn roster_configuration_label_discloses_revision_and_alias_acknowledgement() {
        let mut member = MemberConfiguration {
            member_key: "member-a".to_owned(),
            label: "Member A".to_owned(),
            provider: "codex".to_owned(),
            requested_model: "gpt-6".to_owned(),
            requested_effort: Some("high".to_owned()),
            configuration_revision: 7,
            moving_alias_acknowledged: true,
            configuration_state: ProviderConfigurationState::ResolutionUnreported,
        };
        assert_eq!(
            authoritative_configuration_label(&member),
            "resolution unreported · r7 · alias:ack"
        );
        member.moving_alias_acknowledged = false;
        assert_eq!(
            authoritative_configuration_label(&member),
            "resolution unreported · r7 · alias:unack"
        );
    }
}
