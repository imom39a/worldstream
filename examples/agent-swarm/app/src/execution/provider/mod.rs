//! Provider-specific command contracts.
//!
//! Adapters construct argv directly and never invoke a shell. Discovery is
//! read-only (`--version`/`--help`) and deliberately does not inspect auth or
//! start model work. A discovered provider is executable only when its exact
//! installed contract has separate qualification evidence for resolution,
//! delegation containment, and native cancellation.

mod claude;
mod codex;
pub(crate) mod codex_app_server;
mod controlled;
mod kiro;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;
pub use controlled::{ControlledAdapter, ControlledBehavior};
pub use kiro::KiroAdapter;

const MAX_PROBE_BYTES: usize = 1024 * 1024;
// Native CLIs may page their executable and initialize platform libraries more
// slowly while the full qualification suite is launching other processes. Keep
// discovery bounded, but leave enough room that host load is not mistaken for
// a missing capability.
const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_ID_BYTES: usize = 256;
const MAX_PROMPT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ACTION_PROPOSAL_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_PATH_BYTES: usize = 4 * 1024;
const MAX_MEDIA_TYPE_BYTES: usize = 256;

/// Exact application envelope expected in one provider's final answer.
pub const ACTION_PROPOSAL_SCHEMA: &str = "worldstream/agent-swarm-action-proposal@1";

/// Native evidence must name this transport contract before Codex admission.
/// Existing v2 qualification records cannot establish this new proof.
pub const CODEX_APP_SERVER_CONTRACT: &str = "worldstream/codex-app-server-invocation@1";

/// Provider family selected for one roster member.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Codex,
    Claude,
    Kiro,
    Controlled,
}

/// One exact model/effort combination retained from native qualification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualifiedSelection {
    pub model: String,
    pub effort: Option<String>,
}

/// Non-secret identity of the native evidence that qualified an executable.
///
/// Keeping the exact selections here prevents a capability proven for one
/// model/effort pair from becoming ambient authorization for every setting the
/// provider happens to accept.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderQualificationBinding {
    pub provider: ProviderKind,
    pub executable_digest: String,
    pub version: String,
    pub evidence_sha256: String,
    pub operating_system: String,
    pub architecture: String,
    pub resource_confinement_qualified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_contract: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_codex_profile: Option<super::local_codex::LocalCodexProfile>,
    pub selections: Vec<QualifiedSelection>,
}

impl ProviderKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Kiro => "kiro",
            Self::Controlled => "controlled",
        }
    }

    /// Resolves the exact provider vocabulary accepted in authoritative roster
    /// configuration. Unknown names are never folded into an ambient bucket.
    #[must_use]
    pub fn from_roster_name(value: &str) -> Option<Self> {
        match value {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "kiro" => Some(Self::Kiro),
            "controlled" => Some(Self::Controlled),
            _ => None,
        }
    }
}

/// Whether the provider may write within the explicitly supplied working area.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePolicy {
    ReadOnly,
    WorkspaceWrite,
}

/// Explicit provider conversation choice for one Invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum SessionSelection {
    Fresh { requested_id: Option<String> },
    Resume { session_id: String },
}

/// Requested, reported, and blocked configuration are intentionally distinct.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ConfigurationResolution {
    /// The exact qualified contract promises an Invocation-time report. Output
    /// remains non-authoritative until that report is validated.
    PendingReport,
    Verified {
        model: String,
        effort: Option<String>,
    },
    Unreported,
    Mismatch {
        reported_model: Option<String>,
        reported_effort: Option<String>,
    },
    Unsupported,
}

impl ConfigurationResolution {
    #[must_use]
    pub const fn permits_spawn(&self) -> bool {
        matches!(self, Self::PendingReport | Self::Verified { .. })
    }

    #[must_use]
    pub const fn permits_result(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

/// Closed reason an installed provider is not executable by Agent Swarm.
#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "blocker")]
pub enum ProviderBlocker {
    #[error("provider executable is unavailable")]
    ExecutableUnavailable,
    #[error("provider version is unsupported")]
    UnsupportedVersion,
    #[error("explicit model control is unavailable")]
    ModelControlUnavailable,
    #[error("explicit effort control is unavailable")]
    EffortControlUnavailable,
    #[error("provider does not report effective configuration")]
    ResolutionUnreported,
    #[error("provider-native delegation cannot be contained")]
    DelegationUncontained,
    #[error("native process cancellation is not qualified")]
    CancellationUnqualified,
    #[error("root-scoped filesystem and tool confinement is not qualified")]
    ResourceConfinementUnqualified,
    #[error("moving model alias was not deliberately acknowledged")]
    MovingAliasUnacknowledged,
    #[error("provider reported substituted configuration")]
    ConfigurationMismatch,
    #[error("provider contract data is invalid")]
    InvalidContract,
}

/// Exact installed executable and its discovered/qualified controls.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCapabilities {
    pub provider: ProviderKind,
    pub executable: PathBuf,
    pub executable_digest: String,
    pub version: String,
    pub explicit_model: bool,
    pub explicit_effort: bool,
    pub session_reuse: bool,
    /// True only when a retained compatibility qualification proves that every
    /// Invocation emits effective model and effort values.
    pub reports_effective_configuration: bool,
    /// True only when provider-native delegation is disabled or observably
    /// charged to the same roster/cap slot by the exact installed contract.
    pub delegation_contained: bool,
    /// True only after native owned-descendant cancellation evidence exists for
    /// this exact executable/version/platform tuple.
    pub native_cancellation_qualified: bool,
    /// True only after the exact provider selections passed native in-root,
    /// out-of-root, and disallowed-tool probes under the application launch path.
    pub resource_confinement_qualified: bool,
    /// Exact native evidence and model/effort selections. The controlled test
    /// provider is the only provider permitted to omit this external evidence.
    pub qualification: Option<ProviderQualificationBinding>,
}

impl ProviderCapabilities {
    #[must_use]
    pub fn first_blocker(&self, effort_requested: bool) -> Option<ProviderBlocker> {
        if !self.explicit_model {
            return Some(ProviderBlocker::ModelControlUnavailable);
        }
        if effort_requested && !self.explicit_effort {
            return Some(ProviderBlocker::EffortControlUnavailable);
        }
        if !self.reports_effective_configuration {
            return Some(ProviderBlocker::ResolutionUnreported);
        }
        if !self.delegation_contained {
            return Some(ProviderBlocker::DelegationUncontained);
        }
        if !self.native_cancellation_qualified {
            return Some(ProviderBlocker::CancellationUnqualified);
        }
        if !self.resource_confinement_qualified {
            return Some(ProviderBlocker::ResourceConfinementUnqualified);
        }
        if self.provider == ProviderKind::Codex
            && self
                .qualification
                .as_ref()
                .and_then(|binding| binding.invocation_contract.as_deref())
                != Some(CODEX_APP_SERVER_CONTRACT)
        {
            return Some(ProviderBlocker::InvalidContract);
        }
        None
    }
}

/// Result of bounded installed-provider inspection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderInspection {
    pub provider: ProviderKind,
    pub requested_path: PathBuf,
    pub capabilities: Option<ProviderCapabilities>,
    pub blocker: Option<ProviderBlocker>,
}

impl ProviderInspection {
    #[must_use]
    pub fn unavailable(provider: ProviderKind, path: PathBuf, blocker: ProviderBlocker) -> Self {
        Self {
            provider,
            requested_path: path,
            capabilities: None,
            blocker: Some(blocker),
        }
    }
}

/// One member's explicit provider request. It contains no Room or provider
/// bearer credential.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationRequest {
    pub invocation_id: String,
    pub member_id: String,
    pub configuration_revision: u64,
    pub model: String,
    pub effort: Option<String>,
    pub moving_alias_acknowledged: bool,
    pub working_area: PathBuf,
    pub resource_policy: ResourcePolicy,
    /// Exact provider tool names allowed by the application resource policy.
    /// An empty list selects the provider's qualified minimal built-in surface;
    /// adapters without exact named-tool control reject non-empty lists.
    /// Provider-native delegation tools are always rejected.
    pub allowed_tools: Vec<String>,
    pub session: SessionSelection,
    pub prompt: String,
}

impl InvocationRequest {
    pub(super) fn validate(&self) -> Result<(), ProviderError> {
        if !bounded(&self.invocation_id)
            || !bounded(&self.member_id)
            || self.configuration_revision == 0
            || !bounded(&self.model)
            || self.effort.as_deref().is_some_and(|value| !bounded(value))
            || !self.working_area.is_absolute()
            || self.allowed_tools.len() > 64
            || self.allowed_tools.iter().any(|tool| {
                !bounded(tool) || matches!(tool.to_ascii_lowercase().as_str(), "agent" | "task")
            })
            || self.prompt.is_empty()
            || self.prompt.len() > MAX_PROMPT_BYTES
        {
            return Err(ProviderError::InvalidRequest);
        }
        match &self.session {
            SessionSelection::Fresh { requested_id } => {
                if requested_id.as_deref().is_some_and(|value| !bounded(value)) {
                    return Err(ProviderError::InvalidRequest);
                }
            }
            SessionSelection::Resume { session_id } if !bounded(session_id) => {
                return Err(ProviderError::InvalidRequest);
            }
            SessionSelection::Resume { .. } => {}
        }
        Ok(())
    }
}

/// Parser selected for the exact provider output contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputContract {
    CodexJsonLines,
    CodexAppServer,
    ClaudeStreamJson,
    KiroUnsupported,
    ControlledJsonLines,
}

/// One non-secret file materialized below an owner-protected, invocation-local
/// directory immediately before process launch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EphemeralProviderFile {
    /// A single portable file name. Nested paths and replacement are forbidden.
    pub name: String,
    /// Bounded UTF-8 configuration bytes. Bearer credentials do not belong here.
    pub contents: String,
}

/// Provider configuration whose lifetime is bound to one owned process tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EphemeralProviderProfile {
    /// Environment variable set to the exact protected materialization directory.
    pub root_environment_variable: String,
    pub files: Vec<EphemeralProviderFile>,
}

/// Fully materialized process request. Arguments are passed directly to the
/// executable; none are interpreted by a shell.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedInvocation {
    pub provider: ProviderKind,
    pub invocation_id: String,
    pub member_id: String,
    pub configuration_revision: u64,
    pub program: PathBuf,
    pub executable_digest: String,
    pub qualification: Option<ProviderQualificationBinding>,
    pub arguments: Vec<String>,
    pub working_area: PathBuf,
    pub environment_remove: BTreeSet<String>,
    pub environment_set: BTreeMap<String, String>,
    /// Non-secret invocation-local configuration. The process owner creates it
    /// exclusively and removes it after the guarded tree is reaped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_profile: Option<EphemeralProviderProfile>,
    pub stdin: String,
    pub requested_model: String,
    pub requested_effort: Option<String>,
    pub requested_session: SessionSelection,
    pub resolution: ConfigurationResolution,
    pub output_contract: OutputContract,
}

impl PreparedInvocation {
    /// Refuses to launch unreported, unsupported, or mismatched configuration.
    ///
    /// # Errors
    /// Returns the exact closed blocker retained by discovery/qualification.
    pub fn ensure_spawnable(&self) -> Result<(), ProviderBlocker> {
        if !valid_blake3_digest(&self.executable_digest)
            || (self.provider != ProviderKind::Controlled
                && !self.qualification.as_ref().is_some_and(|binding| {
                    qualification_permits(
                        binding,
                        self.provider,
                        &self.executable_digest,
                        &binding.version,
                        &self.requested_model,
                        self.requested_effort.as_deref(),
                    )
                }))
        {
            return Err(ProviderBlocker::InvalidContract);
        }
        match self.resolution {
            ConfigurationResolution::PendingReport | ConfigurationResolution::Verified { .. } => {
                Ok(())
            }
            ConfigurationResolution::Unreported => Err(ProviderBlocker::ResolutionUnreported),
            ConfigurationResolution::Mismatch { .. } => Err(ProviderBlocker::ConfigurationMismatch),
            ConfigurationResolution::Unsupported => Err(ProviderBlocker::UnsupportedVersion),
        }
    }
}

/// Bounded decoded output. Text is supervised activity, not accepted Room fact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationOutput {
    pub invocation_id: String,
    pub reported_model: String,
    pub reported_effort: Option<String>,
    pub session_id: Option<String>,
    pub text: String,
}

/// One workspace artifact declared by a provider-authored Action proposal.
///
/// The path is deliberately relative. The coordinator resolves it beneath the
/// authorized working area, captures the bytes, and supplies the authoritative
/// digest and absolute local path to the Pack Action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedArtifact {
    pub artifact_id: String,
    pub local_path: String,
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<String>,
    /// Model-authored bytes published immutably by the coordinator. This is
    /// separate from giving the provider filesystem write access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_text: Option<String>,
}

/// Strict provider-authored proposal. It is untrusted data until the
/// coordinator binds it to a fresh participant observation and Action offer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionProposal {
    pub schema: String,
    pub action_type: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ProposedArtifact>,
}

/// Decodes one final provider answer as an exact bounded Action proposal.
///
/// Markdown fences, explanatory text, trailing JSON, non-object payloads, and
/// malformed artifact declarations are rejected. Provider text never becomes
/// a Room fact merely because it satisfies this transport contract.
///
/// # Errors
/// Returns [`ProviderError::InvalidOutput`] for any non-canonical envelope.
pub fn decode_action_proposal(text: &str) -> Result<ActionProposal, ProviderError> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_ACTION_PROPOSAL_BYTES {
        return Err(ProviderError::InvalidOutput);
    }
    let proposal: ActionProposal =
        serde_json::from_str(trimmed).map_err(|_| ProviderError::InvalidOutput)?;
    let payload_bytes =
        serde_json::to_vec(&proposal.payload).map_err(|_| ProviderError::InvalidOutput)?;
    if proposal.schema != ACTION_PROPOSAL_SCHEMA
        || !bounded(&proposal.action_type)
        || !proposal
            .action_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !proposal.payload.is_object()
        || payload_bytes.len() > MAX_ACTION_PROPOSAL_BYTES
        || proposal.artifact.as_ref().is_some_and(|artifact| {
            !bounded(&artifact.artifact_id)
                || !artifact
                    .artifact_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
                || artifact.local_path.trim().is_empty()
                || artifact.local_path.len() > MAX_ARTIFACT_PATH_BYTES
                || artifact.local_path.contains('\0')
                || artifact.media_type.trim().is_empty()
                || artifact.media_type.len() > MAX_MEDIA_TYPE_BYTES
                || artifact.media_type.contains('\0')
                || artifact
                    .inline_text
                    .as_ref()
                    .is_some_and(|text| text.len() > 65536)
                || artifact
                    .expected_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_blake3_digest(digest))
        })
    {
        return Err(ProviderError::InvalidOutput);
    }
    Ok(proposal)
}

impl InvocationOutput {
    /// Validates the provider report against the immutable prepared request.
    ///
    /// # Errors
    /// Refuses missing or substituted model/effort reports.
    pub fn verify(
        &self,
        prepared: &PreparedInvocation,
    ) -> Result<ConfigurationResolution, ProviderBlocker> {
        if self.invocation_id != prepared.invocation_id
            || self.reported_model != prepared.requested_model
            || self.reported_effort != prepared.requested_effort
            || !reported_session_matches_request(
                self.session_id.as_deref(),
                &prepared.requested_session,
            )
        {
            return Err(ProviderBlocker::ConfigurationMismatch);
        }
        Ok(ConfigurationResolution::Verified {
            model: self.reported_model.clone(),
            effort: self.reported_effort.clone(),
        })
    }
}

fn reported_session_matches_request(
    reported_session_id: Option<&str>,
    requested: &SessionSelection,
) -> bool {
    match requested {
        SessionSelection::Fresh {
            requested_id: Some(requested_id),
        }
        | SessionSelection::Resume {
            session_id: requested_id,
        } => reported_session_id == Some(requested_id.as_str()),
        SessionSelection::Fresh { requested_id: None } => reported_session_id.is_some_and(bounded),
    }
}

/// Read-only provider process result used during discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Injectable discovery port. Deterministic tests never launch real providers.
pub trait ProviderProbe: Send + Sync {
    /// Executes one bounded, read-only provider metadata query.
    ///
    /// # Errors
    /// Reports unavailable processes, timeouts, invalid paths, and oversized
    /// or non-UTF-8 output without starting provider model work.
    fn run(&self, executable: &Path, arguments: &[&str]) -> Result<ProbeOutput, ProviderError>;
}

/// Bounded native `--help`/`--version` probe.
#[derive(Clone, Copy, Debug)]
pub struct NativeProviderProbe {
    timeout: Duration,
}

impl Default for NativeProviderProbe {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_PROBE_TIMEOUT,
        }
    }
}

impl NativeProviderProbe {
    /// Constructs a probe with a small non-zero timeout.
    ///
    /// # Errors
    /// Rejects zero or excessive discovery timeouts.
    pub fn new(timeout: Duration) -> Result<Self, ProviderError> {
        if timeout.is_zero() || timeout > Duration::from_secs(30) {
            return Err(ProviderError::InvalidRequest);
        }
        Ok(Self { timeout })
    }
}

impl ProviderProbe for NativeProviderProbe {
    fn run(&self, executable: &Path, arguments: &[&str]) -> Result<ProbeOutput, ProviderError> {
        let executable = exact_executable(executable)?;
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        scrub_direct_api_environment(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| ProviderError::ProbeUnavailable)?;
        let stdout = child.stdout.take().ok_or(ProviderError::ProbeUnavailable)?;
        let stderr = child.stderr.take().ok_or(ProviderError::ProbeUnavailable)?;
        let stdout_reader = thread::spawn(move || bounded_probe_reader(stdout));
        let stderr_reader = thread::spawn(move || bounded_probe_reader(stderr));
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(ProviderError::ProbeUnavailable)?;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProviderError::ProbeUnavailable);
                }
            }
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| ProviderError::ProbeUnavailable)??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| ProviderError::ProbeUnavailable)??;
        Ok(ProbeOutput {
            success: status.success(),
            stdout,
            stderr,
        })
    }
}

/// Provider command adapter.
pub trait ProviderAdapter: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn inspect(&self, probe: &dyn ProviderProbe, executable: &Path) -> ProviderInspection;
    /// Materializes the exact direct process invocation.
    ///
    /// # Errors
    /// Rejects malformed requests or mismatched capability evidence.
    fn prepare(
        &self,
        request: &InvocationRequest,
        capabilities: &ProviderCapabilities,
    ) -> Result<PreparedInvocation, ProviderError>;
    /// Decodes bounded provider output under the prepared contract.
    ///
    /// # Errors
    /// Rejects malformed, missing, or unsupported provider reports.
    fn decode(
        &self,
        prepared: &PreparedInvocation,
        stdout: &[u8],
    ) -> Result<InvocationOutput, ProviderError>;
}

/// Closed provider adapter failures. No command line or prompt is included.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProviderError {
    #[error("provider request is invalid")]
    InvalidRequest,
    #[error("provider discovery is unavailable")]
    ProbeUnavailable,
    #[error("provider contract is unsupported")]
    Unsupported,
    #[error("provider cannot enforce the requested named-tool policy")]
    ToolPolicyUnavailable,
    #[error("provider output is invalid")]
    InvalidOutput,
}

/// Fixed registry for the four application provider kinds.
#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry;

impl ProviderRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub fn inspect(
        &self,
        kind: ProviderKind,
        probe: &dyn ProviderProbe,
        executable: &Path,
    ) -> ProviderInspection {
        match kind {
            ProviderKind::Codex => CodexAdapter.inspect(probe, executable),
            ProviderKind::Claude => ClaudeAdapter.inspect(probe, executable),
            ProviderKind::Kiro => KiroAdapter.inspect(probe, executable),
            ProviderKind::Controlled => ControlledAdapter.inspect(probe, executable),
        }
    }

    /// Materializes the selected provider's exact direct invocation.
    ///
    /// # Errors
    /// Rejects malformed requests or mismatched capability evidence.
    pub fn prepare(
        &self,
        kind: ProviderKind,
        request: &InvocationRequest,
        capabilities: &ProviderCapabilities,
    ) -> Result<PreparedInvocation, ProviderError> {
        match kind {
            ProviderKind::Codex => CodexAdapter.prepare(request, capabilities),
            ProviderKind::Claude => ClaudeAdapter.prepare(request, capabilities),
            ProviderKind::Kiro => KiroAdapter.prepare(request, capabilities),
            ProviderKind::Controlled => ControlledAdapter.prepare(request, capabilities),
        }
    }

    /// Decodes provider output with the matching prepared contract.
    ///
    /// # Errors
    /// Rejects malformed, missing, or unsupported provider reports.
    pub fn decode(
        &self,
        prepared: &PreparedInvocation,
        stdout: &[u8],
    ) -> Result<InvocationOutput, ProviderError> {
        match prepared.provider {
            ProviderKind::Codex => CodexAdapter.decode(prepared, stdout),
            ProviderKind::Claude => ClaudeAdapter.decode(prepared, stdout),
            ProviderKind::Kiro => KiroAdapter.decode(prepared, stdout),
            ProviderKind::Controlled => ControlledAdapter.decode(prepared, stdout),
        }
    }
}

/// Finds distinct absolute executable candidates without invoking them.
#[must_use]
pub fn discover_candidates(names: &[&str], extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates = BTreeSet::new();
    for path in extra {
        if let Ok(canonical) = exact_executable(path) {
            candidates.insert(canonical);
        }
    }
    if let Some(path_value) = env::var_os("PATH") {
        for directory in env::split_paths(&path_value) {
            for name in names {
                let candidate = directory.join(platform_executable_name(name));
                if let Ok(canonical) = exact_executable(&candidate) {
                    candidates.insert(canonical);
                }
            }
        }
    }
    candidates.into_iter().collect()
}

pub(super) fn base_prepared(
    provider: ProviderKind,
    request: &InvocationRequest,
    capabilities: &ProviderCapabilities,
    arguments: Vec<String>,
    environment_remove: BTreeSet<String>,
    environment_set: BTreeMap<String, String>,
    output_contract: OutputContract,
) -> Result<PreparedInvocation, ProviderError> {
    request.validate()?;
    validate_capabilities(provider, request, capabilities)?;
    let resolution = capabilities.first_blocker(request.effort.is_some()).map_or(
        ConfigurationResolution::PendingReport,
        |blocker| match blocker {
            ProviderBlocker::ResolutionUnreported => ConfigurationResolution::Unreported,
            _ => ConfigurationResolution::Unsupported,
        },
    );
    Ok(PreparedInvocation {
        provider,
        invocation_id: request.invocation_id.clone(),
        member_id: request.member_id.clone(),
        configuration_revision: request.configuration_revision,
        program: capabilities.executable.clone(),
        executable_digest: capabilities.executable_digest.clone(),
        qualification: capabilities.qualification.clone(),
        arguments,
        working_area: request.working_area.clone(),
        environment_remove,
        environment_set,
        ephemeral_profile: None,
        stdin: request.prompt.clone(),
        requested_model: request.model.clone(),
        requested_effort: request.effort.clone(),
        requested_session: request.session.clone(),
        resolution,
        output_contract,
    })
}

pub(super) fn validate_capabilities(
    provider: ProviderKind,
    request: &InvocationRequest,
    capabilities: &ProviderCapabilities,
) -> Result<(), ProviderError> {
    if capabilities.provider != provider
        || exact_executable(&capabilities.executable).as_deref()
            != Ok(capabilities.executable.as_path())
        || capabilities.version.is_empty()
        || capabilities.executable_digest.len() != "blake3:".len() + 64
        || capabilities
            .executable_digest
            .strip_prefix("blake3:")
            .is_none_or(|digest| {
                digest
                    .bytes()
                    .any(|byte| !byte.is_ascii_digit() && !matches!(byte, b'a'..=b'f'))
            })
        || digest_executable(&capabilities.executable).as_ref()
            != Ok(&capabilities.executable_digest)
        || (provider != ProviderKind::Controlled
            && !capabilities.qualification.as_ref().is_some_and(|binding| {
                qualification_permits(
                    binding,
                    provider,
                    &capabilities.executable_digest,
                    &capabilities.version,
                    &request.model,
                    request.effort.as_deref(),
                )
            }))
        || (!request.moving_alias_acknowledged && is_declared_moving_alias(&request.model))
    {
        return Err(ProviderError::InvalidRequest);
    }
    Ok(())
}

pub(super) fn inspect_contract(
    provider: ProviderKind,
    probe: &dyn ProviderProbe,
    executable: &Path,
    version_args: &[&str],
    help_args: &[&str],
    assess: impl FnOnce(&str, &str) -> Option<(String, bool, bool, bool)>,
) -> ProviderInspection {
    let requested_path = executable.to_path_buf();
    let Ok(canonical) = exact_executable(executable) else {
        return ProviderInspection::unavailable(
            provider,
            requested_path,
            ProviderBlocker::ExecutableUnavailable,
        );
    };
    let version = match probe.run(&canonical, version_args) {
        Ok(output) if output.success => joined_probe_text(&output),
        _ => {
            return ProviderInspection::unavailable(
                provider,
                requested_path,
                ProviderBlocker::UnsupportedVersion,
            );
        }
    };
    let help = match probe.run(&canonical, help_args) {
        Ok(output) if output.success => joined_probe_text(&output),
        _ => {
            return ProviderInspection::unavailable(
                provider,
                requested_path,
                ProviderBlocker::UnsupportedVersion,
            );
        }
    };
    let Some((version, explicit_model, explicit_effort, session_reuse)) = assess(&version, &help)
    else {
        return ProviderInspection::unavailable(
            provider,
            requested_path,
            ProviderBlocker::UnsupportedVersion,
        );
    };
    let Ok(digest) = digest_executable(&canonical) else {
        return ProviderInspection::unavailable(
            provider,
            requested_path,
            ProviderBlocker::ExecutableUnavailable,
        );
    };
    ProviderInspection {
        provider,
        requested_path,
        capabilities: Some(ProviderCapabilities {
            provider,
            executable: canonical,
            executable_digest: digest,
            version,
            explicit_model,
            explicit_effort,
            session_reuse,
            // Static help proves flag presence only. Qualification must replace
            // these three fields using signed-in native evidence for the exact
            // executable/version/platform tuple.
            reports_effective_configuration: false,
            delegation_contained: false,
            native_cancellation_qualified: false,
            resource_confinement_qualified: false,
            qualification: None,
        }),
        blocker: Some(ProviderBlocker::ResolutionUnreported),
    }
}

pub(super) fn direct_api_environment() -> BTreeSet<String> {
    [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_FOUNDRY_API_KEY",
        "ANTHROPIC_FOUNDRY_BASE_URL",
        "ANTHROPIC_FOUNDRY_RESOURCE",
        "ANTHROPIC_VERTEX_BASE_URL",
        "AWS_ACCESS_KEY_ID",
        "AWS_BEARER_TOKEN_BEDROCK",
        "AWS_PROFILE",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AZURE_OPENAI_API_KEY",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_VERTEX",
        "CODEX_API_KEY",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "KIRO_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn scrub_direct_api_environment(command: &mut Command) {
    for variable in direct_api_environment() {
        command.env_remove(variable);
    }
}

fn exact_executable(path: &Path) -> Result<PathBuf, ProviderError> {
    if !path.is_absolute() {
        return Err(ProviderError::InvalidRequest);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ProviderError::ProbeUnavailable)?;
    let metadata = fs::metadata(&canonical).map_err(|_| ProviderError::ProbeUnavailable)?;
    if !metadata.is_file() {
        return Err(ProviderError::ProbeUnavailable);
    }
    Ok(canonical)
}

pub(super) fn digest_executable(path: &Path) -> Result<String, ProviderError> {
    let mut file = fs::File::open(path).map_err(|_| ProviderError::ProbeUnavailable)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| ProviderError::ProbeUnavailable)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
}

fn qualification_permits(
    binding: &ProviderQualificationBinding,
    provider: ProviderKind,
    executable_digest: &str,
    version: &str,
    model: &str,
    effort: Option<&str>,
) -> bool {
    binding.provider == provider
        && (provider != ProviderKind::Codex
            || binding.invocation_contract.as_deref() == Some(CODEX_APP_SERVER_CONTRACT))
        && binding.executable_digest == executable_digest
        && binding.version == version
        && valid_blake3_digest(&binding.executable_digest)
        && bounded(&binding.version)
        && valid_sha256(&binding.evidence_sha256)
        && binding.resource_confinement_qualified
        && binding.operating_system == std::env::consts::OS
        && architecture_matches(&binding.architecture, std::env::consts::ARCH)
        && !binding.selections.is_empty()
        && binding
            .selections
            .iter()
            .any(|selection| selection.model == model && selection.effort.as_deref() == effort)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn architecture_matches(evidence: &str, native: &str) -> bool {
    evidence == native
        || matches!(
            (evidence, native),
            ("arm64", "aarch64") | ("amd64", "x86_64")
        )
}

fn bounded_probe_reader(mut reader: impl Read) -> Result<String, ProviderError> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(u64::try_from(MAX_PROBE_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| ProviderError::ProbeUnavailable)?;
    if bytes.len() > MAX_PROBE_BYTES {
        return Err(ProviderError::ProbeUnavailable);
    }
    String::from_utf8(bytes).map_err(|_| ProviderError::ProbeUnavailable)
}

fn joined_probe_text(output: &ProbeOutput) -> String {
    match (output.stdout.trim(), output.stderr.trim()) {
        ("", stderr) => stderr.to_owned(),
        (stdout, "") => stdout.to_owned(),
        (stdout, stderr) => format!("{stdout}\n{stderr}"),
    }
}

fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES && !value.contains('\0')
}

fn valid_blake3_digest(value: &str) -> bool {
    value.strip_prefix("blake3:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn is_declared_moving_alias(model: &str) -> bool {
    let normalized = model.to_ascii_lowercase();
    let base = normalized
        .split_once('[')
        .map_or(normalized.as_str(), |(base, _)| base);
    matches!(
        base,
        "auto" | "default" | "latest" | "haiku" | "opus" | "opusplan" | "sonnet"
    ) || base.ends_with("-latest")
}

#[cfg(windows)]
fn platform_executable_name(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_owned()
    } else {
        format!("{name}.exe")
    }
}

#[cfg(not(windows))]
fn platform_executable_name(name: &str) -> String {
    name.to_owned()
}
