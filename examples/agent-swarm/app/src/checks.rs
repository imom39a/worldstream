//! Exact-version check evidence and independent-review acceptance.
//!
//! A successful process exit is only one field in [`CheckEvidence`]. Evidence
//! also pins the candidate, named input versions, criteria version, executable
//! bytes, exact argv/environment, and captured stdout/stderr. The review gate
//! rejects stale or failed checks, self-certification, and every unresolved
//! blocking finding for the same material revision.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "managed-local-runtime")]
use crate::execution::{NativeProcessSpawner, OwnedCommand, ProcessError};
use crate::{
    artifacts::{ArtifactRef, ContentDigest, read_stable_regular_file},
    code_change::{CodeCandidateRevision, CodeChangeError, CodeChangeWorkspace},
};

const MAX_CHECKS: usize = 64;
const MAX_INPUTS: usize = 256;
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 256;
const MAX_TEXT_BYTES: usize = 4 * 1024;

/// A named input version used by a candidate and its checks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedInput {
    pub name: String,
    pub artifact: ArtifactRef,
}

/// Exact material versions to which checks and reviews apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBinding {
    pub candidate: ContentDigest,
    pub inputs: Vec<VersionedInput>,
    pub criteria: ArtifactRef,
}

impl EvidenceBinding {
    /// Creates a canonical, name-sorted version binding.
    ///
    /// # Errors
    ///
    /// Rejects invalid, duplicate, or excessive input names.
    pub fn new(
        revision: &CodeCandidateRevision,
        mut inputs: Vec<VersionedInput>,
        criteria: ArtifactRef,
    ) -> Result<Self, CheckError> {
        if inputs.len() > MAX_INPUTS {
            return Err(CheckError::InvalidBinding);
        }
        for input in &inputs {
            validate_label(&input.name)?;
        }
        inputs.sort_by(|left, right| left.name.cmp(&right.name));
        if inputs.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(CheckError::InvalidBinding);
        }
        Ok(Self {
            candidate: revision.candidate_digest(),
            inputs,
            criteria,
        })
    }
}

/// Exact checker process configuration. The environment is cleared before the
/// listed values are applied, so ambient PATH or model credentials cannot
/// silently alter the recorded invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckCommand {
    check_id: String,
    program: PathBuf,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
}

impl CheckCommand {
    /// Constructs a checker command. `program` must be an absolute pathname;
    /// command lookup through ambient PATH is intentionally unsupported.
    ///
    /// # Errors
    ///
    /// Rejects invalid identifiers, relative programs, or oversized argv.
    pub fn new(
        check_id: impl Into<String>,
        program: PathBuf,
        arguments: Vec<String>,
    ) -> Result<Self, CheckError> {
        let check_id = check_id.into();
        validate_identifier(&check_id)?;
        if !program.is_absolute()
            || arguments.len() > MAX_ARGUMENTS
            || arguments
                .iter()
                .any(|argument| argument.len() > MAX_TEXT_BYTES || argument.as_bytes().contains(&0))
        {
            return Err(CheckError::InvalidCommand);
        }
        Ok(Self {
            check_id,
            program,
            arguments,
            environment: BTreeMap::new(),
        })
    }

    /// Adds one explicit environment value.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized names, `=`, NUL, or oversized/NUL values.
    pub fn with_environment(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, CheckError> {
        let name = name.into();
        let value = value.into();
        if name.is_empty()
            || name.len() > MAX_TEXT_BYTES
            || name.contains(['=', '\0'])
            || value.len() > MAX_TEXT_BYTES
            || value.as_bytes().contains(&0)
        {
            return Err(CheckError::InvalidCommand);
        }
        self.environment.insert(name, value);
        Ok(self)
    }

    #[must_use]
    pub fn check_id(&self) -> &str {
        &self.check_id
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    #[must_use]
    pub const fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
}

/// Executable identity and all process inputs for one check.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckerIdentity {
    pub executable_path: String,
    pub executable: ArtifactRef,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

/// Process result. A signal/exception has no portable integer exit code.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed { exit_code: Option<i32> },
}

/// Goal-specific check evidence bound to exact material and process versions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckEvidence {
    pub check_id: String,
    pub binding: EvidenceBinding,
    pub checker: CheckerIdentity,
    pub outcome: CheckOutcome,
    pub stdout: ArtifactRef,
    pub stderr: ArtifactRef,
}

impl CheckEvidence {
    #[must_use]
    pub const fn passed(&self) -> bool {
        matches!(self.outcome, CheckOutcome::Passed)
    }
}

/// Runs checks against one copy-isolated candidate workspace.
pub struct CheckRunner<'a> {
    changes: &'a CodeChangeWorkspace,
}

impl<'a> CheckRunner<'a> {
    #[must_use]
    pub const fn new(changes: &'a CodeChangeWorkspace) -> Self {
        Self { changes }
    }

    /// Runs a checker only while the editable copy exactly matches the sealed
    /// revision, and verifies that it still matches afterward.
    ///
    /// # Errors
    ///
    /// Rejects stale bindings, missing evidence objects, unsafe executables,
    /// process failures to start, oversized output, or checker mutation of the
    /// candidate. A nonzero exit is returned as failed evidence, not an error.
    pub fn run(
        &self,
        revision: &CodeCandidateRevision,
        binding: &EvidenceBinding,
        command: &CheckCommand,
    ) -> Result<CheckEvidence, CheckError> {
        let prepared = self.prepare(revision, binding, command)?;
        let output = Command::new(&prepared.canonical)
            .args(&command.arguments)
            .env_clear()
            .envs(&command.environment)
            .current_dir(revision.candidate_root())
            .output()
            .map_err(|_| CheckError::CheckerUnavailable)?;
        self.finish(
            revision,
            binding,
            command,
            prepared,
            output.status.success(),
            output.status.code(),
            &output.stdout,
            &output.stderr,
        )
    }

    /// Runs a checker through the native process guard so owner loss and
    /// cancellation contain the complete checker descendant tree.
    ///
    /// # Errors
    ///
    /// Applies the same exact-version validation as [`Self::run`] and fails
    /// closed if process containment or bounded output cannot be established.
    #[cfg(feature = "managed-local-runtime")]
    pub fn run_guarded(
        &self,
        revision: &CodeCandidateRevision,
        binding: &EvidenceBinding,
        command: &CheckCommand,
        process_guard: &Path,
    ) -> Result<CheckEvidence, CheckError> {
        self.run_guarded_while(revision, binding, command, process_guard, &mut || true)
    }

    /// Runs the same guarded check while the caller's execution lease remains
    /// valid. Revocation drains the owned tree and publishes no passing check.
    #[cfg(feature = "managed-local-runtime")]
    pub(crate) fn run_guarded_while(
        &self,
        revision: &CodeCandidateRevision,
        binding: &EvidenceBinding,
        command: &CheckCommand,
        process_guard: &Path,
        continue_running: &mut dyn FnMut() -> bool,
    ) -> Result<CheckEvidence, CheckError> {
        let prepared = self.prepare(revision, binding, command)?;
        if !continue_running() {
            return Err(CheckError::Interrupted);
        }
        let spawner = NativeProcessSpawner::new(process_guard, MAX_OUTPUT_BYTES)
            .map_err(map_process_error)?;
        let request = OwnedCommand {
            program: prepared.canonical.clone(),
            expected_executable_digest: prepared.executable.digest().to_string(),
            arguments: command.arguments.clone(),
            working_area: revision.candidate_root().to_path_buf(),
            environment: command.environment.clone(),
            stdin: String::new(),
        };
        let mut process = spawner.spawn_command(&request).map_err(map_process_error)?;
        let output = loop {
            if !continue_running() {
                process
                    .stop(std::time::Duration::from_secs(5))
                    .map_err(map_process_error)?;
                return Err(CheckError::Interrupted);
            }
            if let Some(output) = process.poll().map_err(map_process_error)? {
                break output;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        if output.stdout_truncated || output.stderr_truncated {
            return Err(CheckError::OutputTooLarge);
        }
        self.finish(
            revision,
            binding,
            command,
            prepared,
            output.success,
            output.code,
            &output.stdout,
            &output.stderr,
        )
    }

    fn prepare(
        &self,
        revision: &CodeCandidateRevision,
        binding: &EvidenceBinding,
        command: &CheckCommand,
    ) -> Result<PreparedCheck, CheckError> {
        if binding.candidate != revision.candidate_digest() {
            return Err(CheckError::StaleBinding);
        }
        self.verify_binding(binding)?;
        self.changes.verify_candidate(revision)?;
        let canonical =
            fs::canonicalize(&command.program).map_err(|_| CheckError::CheckerUnsafe)?;
        let executable_path = canonical
            .to_str()
            .ok_or(CheckError::CheckerUnsafe)?
            .to_owned();
        let executable_bytes =
            read_stable_regular_file(&canonical).map_err(|_| CheckError::CheckerUnsafe)?;
        let executable = self.changes.artifacts().store_bytes(&executable_bytes)?;
        Ok(PreparedCheck {
            canonical,
            executable_path,
            executable,
        })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "all arguments are one closed checker observation"
    )]
    fn finish(
        &self,
        revision: &CodeCandidateRevision,
        binding: &EvidenceBinding,
        command: &CheckCommand,
        prepared: PreparedCheck,
        success: bool,
        exit_code: Option<i32>,
        stdout_bytes: &[u8],
        stderr_bytes: &[u8],
    ) -> Result<CheckEvidence, CheckError> {
        let current_executable =
            read_stable_regular_file(&prepared.canonical).map_err(|_| CheckError::CheckerUnsafe)?;
        if ContentDigest::of(&current_executable) != prepared.executable.digest() {
            return Err(CheckError::CheckerUnsafe);
        }
        self.changes.verify_candidate(revision)?;
        if stdout_bytes.len() > MAX_OUTPUT_BYTES || stderr_bytes.len() > MAX_OUTPUT_BYTES {
            return Err(CheckError::OutputTooLarge);
        }
        let stdout = self.changes.artifacts().store_bytes(stdout_bytes)?;
        let stderr = self.changes.artifacts().store_bytes(stderr_bytes)?;
        let outcome = if success {
            CheckOutcome::Passed
        } else {
            CheckOutcome::Failed { exit_code }
        };
        Ok(CheckEvidence {
            check_id: command.check_id.clone(),
            binding: binding.clone(),
            checker: CheckerIdentity {
                executable_path: prepared.executable_path,
                executable: prepared.executable,
                arguments: command.arguments.clone(),
                environment: command.environment.clone(),
            },
            outcome,
            stdout,
            stderr,
        })
    }

    fn verify_binding(&self, binding: &EvidenceBinding) -> Result<(), CheckError> {
        self.changes.artifacts().verify(&binding.criteria)?;
        let mut names = HashSet::with_capacity(binding.inputs.len());
        if binding.inputs.len() > MAX_INPUTS {
            return Err(CheckError::InvalidBinding);
        }
        for input in &binding.inputs {
            validate_label(&input.name)?;
            if !names.insert(input.name.as_str()) {
                return Err(CheckError::InvalidBinding);
            }
            self.changes.artifacts().verify(&input.artifact)?;
        }
        Ok(())
    }
}

struct PreparedCheck {
    canonical: PathBuf,
    executable_path: String,
    executable: ArtifactRef,
}

#[cfg(feature = "managed-local-runtime")]
const fn map_process_error(error: ProcessError) -> CheckError {
    match error {
        ProcessError::ContainmentUnresolved => CheckError::ContainmentUnresolved,
        ProcessError::InvalidOutput => CheckError::OutputTooLarge,
        ProcessError::InvalidRequest
        | ProcessError::SpawnUnavailable
        | ProcessError::Communication
        | ProcessError::ProviderBlocked => CheckError::CheckerUnavailable,
    }
}

/// Whether a review was performed by a roster agent or a human coordinator.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewerKind {
    Agent,
    Human,
}

/// A review verdict. Any block for the exact revision remains blocking even
/// if a separate review later reports pass.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Pass,
    Block,
}

/// Independent-review evidence for one exact material binding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewEvidence {
    pub review_id: String,
    pub reviewer_id: String,
    pub reviewer_kind: ReviewerKind,
    pub binding: EvidenceBinding,
    pub verdict: ReviewVerdict,
    pub unresolved_blocking_findings: Vec<String>,
}

impl ReviewEvidence {
    /// Creates bounded review evidence.
    ///
    /// # Errors
    ///
    /// Rejects invalid identities, excessive findings, or empty findings.
    pub fn new(
        review_id: impl Into<String>,
        reviewer_id: impl Into<String>,
        reviewer_kind: ReviewerKind,
        binding: EvidenceBinding,
        verdict: ReviewVerdict,
        unresolved_blocking_findings: Vec<String>,
    ) -> Result<Self, ReviewError> {
        let review_id = review_id.into();
        let reviewer_id = reviewer_id.into();
        validate_review_identifier(&review_id)?;
        validate_review_identifier(&reviewer_id)?;
        if unresolved_blocking_findings.len() > MAX_CHECKS
            || unresolved_blocking_findings
                .iter()
                .any(|finding| finding.trim().is_empty() || finding.len() > MAX_TEXT_BYTES)
        {
            return Err(ReviewError::InvalidEvidence);
        }
        Ok(Self {
            review_id,
            reviewer_id,
            reviewer_kind,
            binding,
            verdict,
            unresolved_blocking_findings,
        })
    }
}

/// Acceptance policy naming every goal-specific check that must pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewPolicy {
    required_checks: Vec<String>,
}

impl ReviewPolicy {
    /// Creates a policy with a nonempty, unique set of required check IDs.
    ///
    /// # Errors
    ///
    /// Rejects invalid, duplicate, empty, or excessive requirements.
    pub fn new(required_checks: Vec<String>) -> Result<Self, ReviewError> {
        if required_checks.is_empty() || required_checks.len() > MAX_CHECKS {
            return Err(ReviewError::InvalidPolicy);
        }
        let mut seen = HashSet::with_capacity(required_checks.len());
        for check in &required_checks {
            validate_review_identifier(check)?;
            if !seen.insert(check.as_str()) {
                return Err(ReviewError::InvalidPolicy);
            }
        }
        Ok(Self { required_checks })
    }

    /// Accepts only exact, passing checks plus at least one passing reviewer
    /// who is independent of all attributed agent authors.
    ///
    /// # Errors
    ///
    /// Stale evidence, failed/missing checks, self-review, and unresolved
    /// blocking findings all fail closed.
    pub fn accept(
        &self,
        changes: &CodeChangeWorkspace,
        revision: &CodeCandidateRevision,
        binding: &EvidenceBinding,
        authors: &[String],
        checks: &[CheckEvidence],
        reviews: &[ReviewEvidence],
    ) -> Result<ReviewedCodeChange, ReviewError> {
        changes.validate_sealed_revision(revision)?;
        if binding.candidate != revision.candidate_digest() {
            return Err(ReviewError::StaleEvidence);
        }
        verify_binding_artifacts(changes, binding)?;
        let authors = validate_authors(authors)?;

        let mut seen_checks = HashSet::with_capacity(checks.len());
        for check in checks {
            validate_review_identifier(&check.check_id)?;
            if !seen_checks.insert(check.check_id.as_str()) {
                return Err(ReviewError::InvalidEvidence);
            }
            if &check.binding != binding {
                return Err(ReviewError::StaleEvidence);
            }
            verify_check_artifacts(changes, check)?;
            if !check.passed() {
                return Err(ReviewError::FailedCheck);
            }
        }
        if self
            .required_checks
            .iter()
            .any(|required| !seen_checks.contains(required.as_str()))
        {
            return Err(ReviewError::MissingCheck);
        }

        let mut seen_reviews = HashSet::with_capacity(reviews.len());
        let mut independent_reviewers = Vec::new();
        for review in reviews {
            validate_review_identifier(&review.review_id)?;
            validate_review_identifier(&review.reviewer_id)?;
            if !seen_reviews.insert(review.review_id.as_str()) {
                return Err(ReviewError::InvalidEvidence);
            }
            if &review.binding != binding {
                return Err(ReviewError::StaleEvidence);
            }
            if review.verdict == ReviewVerdict::Block
                || !review.unresolved_blocking_findings.is_empty()
            {
                return Err(ReviewError::BlockingReview);
            }
            let independent = review.reviewer_kind == ReviewerKind::Human
                || !authors.contains(review.reviewer_id.as_str());
            if review.verdict == ReviewVerdict::Pass && independent {
                independent_reviewers.push(review.reviewer_id.clone());
            }
        }
        if independent_reviewers.is_empty() {
            return Err(ReviewError::IndependentReviewMissing);
        }
        independent_reviewers.sort();
        independent_reviewers.dedup();

        let check_ids = self.required_checks.clone();
        let evidence_digest = reviewed_evidence_digest(
            revision.revision_digest(),
            binding,
            &check_ids,
            &independent_reviewers,
        )?;
        Ok(ReviewedCodeChange {
            candidate_digest: revision.candidate_digest(),
            revision_digest: revision.revision_digest(),
            binding: binding.clone(),
            check_ids,
            independent_reviewers,
            evidence_digest,
        })
    }
}

/// The exact evidence set that passed the local reviewed-change gate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedCodeChange {
    pub candidate_digest: ContentDigest,
    pub revision_digest: ContentDigest,
    pub binding: EvidenceBinding,
    pub check_ids: Vec<String>,
    pub independent_reviewers: Vec<String>,
    pub evidence_digest: ContentDigest,
}

#[derive(Debug, Error)]
pub enum CheckError {
    #[error("check execution was interrupted or exceeded its authorized duration")]
    Interrupted,
    #[error("check command is invalid")]
    InvalidCommand,
    #[error("check or input identity is invalid")]
    InvalidIdentifier,
    #[error("the material version binding is invalid")]
    InvalidBinding,
    #[error("the check binding does not match the candidate")]
    StaleBinding,
    #[error("the checker executable is unsafe or unreadable")]
    CheckerUnsafe,
    #[error("the checker process could not be started")]
    CheckerUnavailable,
    #[error("the checker process tree could not be proven contained")]
    ContainmentUnresolved,
    #[error("checker output exceeds the evidence limit")]
    OutputTooLarge,
    #[error(transparent)]
    CodeChange(#[from] CodeChangeError),
    #[error(transparent)]
    Artifact(#[from] crate::artifacts::ArtifactError),
}

#[derive(Debug, Error)]
pub enum ReviewError {
    #[error("review policy is invalid")]
    InvalidPolicy,
    #[error("review evidence is invalid or duplicated")]
    InvalidEvidence,
    #[error("evidence applies to another material version")]
    StaleEvidence,
    #[error("a required check is missing")]
    MissingCheck,
    #[error("a goal-specific check failed")]
    FailedCheck,
    #[error("an unresolved blocking review prevents acceptance")]
    BlockingReview,
    #[error("no independent passing reviewer is present")]
    IndependentReviewMissing,
    #[error("evidence storage is unavailable or corrupt")]
    EvidenceUnavailable,
}

impl From<CodeChangeError> for ReviewError {
    fn from(_error: CodeChangeError) -> Self {
        Self::EvidenceUnavailable
    }
}

fn verify_binding_artifacts(
    changes: &CodeChangeWorkspace,
    binding: &EvidenceBinding,
) -> Result<(), ReviewError> {
    changes
        .artifacts()
        .verify(&binding.criteria)
        .map_err(|_| ReviewError::EvidenceUnavailable)?;
    if binding.inputs.len() > MAX_INPUTS {
        return Err(ReviewError::InvalidEvidence);
    }
    let mut names = HashSet::with_capacity(binding.inputs.len());
    for input in &binding.inputs {
        validate_review_label(&input.name)?;
        if !names.insert(input.name.as_str()) {
            return Err(ReviewError::InvalidEvidence);
        }
        changes
            .artifacts()
            .verify(&input.artifact)
            .map_err(|_| ReviewError::EvidenceUnavailable)?;
    }
    Ok(())
}

fn verify_check_artifacts(
    changes: &CodeChangeWorkspace,
    check: &CheckEvidence,
) -> Result<(), ReviewError> {
    for artifact in [&check.checker.executable, &check.stdout, &check.stderr] {
        changes
            .artifacts()
            .verify(artifact)
            .map_err(|_| ReviewError::EvidenceUnavailable)?;
    }
    Ok(())
}

fn validate_authors(authors: &[String]) -> Result<HashSet<&str>, ReviewError> {
    if authors.is_empty() || authors.len() > MAX_CHECKS {
        return Err(ReviewError::InvalidEvidence);
    }
    let mut unique = HashSet::with_capacity(authors.len());
    for author in authors {
        validate_review_identifier(author)?;
        if !unique.insert(author.as_str()) {
            return Err(ReviewError::InvalidEvidence);
        }
    }
    Ok(unique)
}

fn reviewed_evidence_digest(
    revision: ContentDigest,
    binding: &EvidenceBinding,
    checks: &[String],
    reviewers: &[String],
) -> Result<ContentDigest, ReviewError> {
    #[derive(Serialize)]
    struct DigestInput<'a> {
        schema: &'static str,
        revision: ContentDigest,
        binding: &'a EvidenceBinding,
        checks: &'a [String],
        reviewers: &'a [String],
    }
    let bytes = serde_json::to_vec(&DigestInput {
        schema: "worldstream/agent-swarm-reviewed-code-change/v1",
        revision,
        binding,
        checks,
        reviewers,
    })
    .map_err(|_| ReviewError::EvidenceUnavailable)?;
    Ok(ContentDigest::of(&bytes))
}

fn validate_identifier(value: &str) -> Result<(), CheckError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(CheckError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_label(value: &str) -> Result<(), CheckError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES || value.as_bytes().contains(&0) {
        return Err(CheckError::InvalidBinding);
    }
    Ok(())
}

fn validate_review_identifier(value: &str) -> Result<(), ReviewError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(ReviewError::InvalidEvidence);
    }
    Ok(())
}

fn validate_review_label(value: &str) -> Result<(), ReviewError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES || value.as_bytes().contains(&0) {
        return Err(ReviewError::InvalidEvidence);
    }
    Ok(())
}

#[allow(dead_code)]
fn _portable_program_path(path: &Path) -> Option<&str> {
    path.to_str()
}
