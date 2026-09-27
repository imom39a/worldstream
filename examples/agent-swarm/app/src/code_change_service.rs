//! Durable reviewed-code-change workflow.
//!
//! This module is the owner-only application seam above [`CodeChangeWorkspace`].
//! It retains exact candidate bindings, guarded check evidence, independent
//! reviews, and write-back operation identities as immutable journal records.
//! Callers receive Pack-shaped check evidence and exact semantic references;
//! they never assert that a process or write-back succeeded.

use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_core::CanonicalJsonV1;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    artifacts::{ArtifactError, ArtifactPath, AuthoritativeArtifactRef, ContentDigest},
    checks::{
        CheckError, CheckEvidence, EvidenceBinding, ReviewError, ReviewEvidence, ReviewPolicy,
        ReviewVerdict, ReviewedCodeChange, ReviewerKind, VersionedInput,
    },
    code_change::{
        CodeCandidateDraft, CodeCandidateRevision, CodeChangeError, CodeChangeWorkspace,
        WriteBackOperation, WriteBackStatus, publish_protected_json, read_protected_json,
        validate_identifier,
    },
};

#[cfg(feature = "managed-local-runtime")]
use crate::artifacts::{ArtifactRef, read_stable_regular_file};
#[cfg(feature = "managed-local-runtime")]
use crate::checks::{CheckCommand, CheckRunner};

const WORKFLOW_SCHEMA: &str = "worldstream/agent-swarm-code-change-workflow/v2";
const CHECK_SCHEMA: &str = "worldstream/agent-swarm-code-change-check/v1";
#[cfg(feature = "managed-local-runtime")]
const PENDING_CHECK_SCHEMA: &str = "worldstream/agent-swarm-code-change-pending-check/v1";
const REVIEW_SCHEMA: &str = "worldstream/agent-swarm-code-change-review/v1";
const ACCEPTED_SCHEMA: &str = "worldstream/agent-swarm-code-change-accepted/v1";
const CANDIDATE_MANIFEST_SCHEMA: &str = "worldstream/agent-swarm-code-candidate-artifact/v1";
pub const CHECK_EVIDENCE_MEDIA_TYPE: &str =
    "application/vnd.worldstream.agent-swarm-check-evidence+json;version=1";
pub const CANDIDATE_MANIFEST_MEDIA_TYPE: &str =
    "application/vnd.worldstream.agent-swarm-code-candidate+json;version=1";
const MAX_IDENTITIES: usize = 64;

/// One Pack Candidate identity bound to a local sealed revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackCandidateRef {
    pub candidate_id: String,
    pub version: u64,
}

/// One exact Pack resource basis entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackVersionRef {
    pub resource_id: String,
    pub version: u64,
}

/// Pack-compatible local artifact reference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackArtifactRef {
    pub artifact_id: String,
    pub digest: String,
    pub local_path: String,
    pub media_type: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PackCandidateManifest {
    schema: String,
    candidate_id: String,
    pack_candidate: PackCandidateRef,
    candidate_digest: ContentDigest,
    revision_digest: ContentDigest,
    targets: Vec<crate::code_change::CandidateTargetRevision>,
}

/// Canonical evidence reference accepted by `record_check`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackCheckEvidenceRef {
    pub artifact: PackArtifactRef,
    pub candidate: PackCandidateRef,
    pub candidate_artifact: PackArtifactRef,
    pub check_id: String,
    pub criterion: String,
    pub criteria_revision: u64,
    pub resource_basis: Vec<PackVersionRef>,
}

/// Exact Pack `record_check` payload generated from retained evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackRecordCheckPayload {
    pub candidate: PackCandidateRef,
    pub check_id: String,
    pub evidence_refs: Vec<PackCheckEvidenceRef>,
    pub expected_criteria_revision: u64,
    pub resource_basis: Vec<PackVersionRef>,
    pub status: CheckStatus,
}

/// Pack-compatible check outcome label.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
}

/// Named authorized input captured when a revision is sealed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundInputRequest {
    pub name: String,
    pub path: ArtifactPath,
}

/// Creates or idempotently reopens one isolated editable candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareCodeChangeRequest {
    pub candidate_id: String,
    pub targets: Vec<ArtifactPath>,
}

/// Seals one candidate and binds its validation policy and Pack identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealCodeChangeRequest {
    pub acceptance_criteria: Vec<String>,
    pub authors: Vec<String>,
    pub candidate_id: String,
    pub criteria_revision: u64,
    #[serde(default)]
    pub inputs: Vec<BoundInputRequest>,
    pub pack_candidate: PackCandidateRef,
    /// Existing Room Candidate artifact to bind, for non-code or externally
    /// assembled candidates. Both artifact and path must be supplied together.
    #[serde(default)]
    pub pack_candidate_artifact: Option<PackArtifactRef>,
    #[serde(default)]
    pub pack_candidate_path: Option<ArtifactPath>,
    #[serde(default)]
    pub resource_basis: Vec<PackVersionRef>,
}

/// Executes one canonical criterion check against a sealed revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunCodeCheckRequest {
    pub arguments: Vec<String>,
    pub candidate_id: String,
    pub check_id: String,
    pub evidence_path: ArtifactPath,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub pack_check_revision: u64,
    pub program: PathBuf,
    pub revision_digest: ContentDigest,
}

/// Records one review and evaluates the complete retained evidence set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordCodeReviewRequest {
    pub candidate_id: String,
    #[serde(default)]
    pub findings: Vec<String>,
    pub pack_review_revision: u64,
    pub review_id: String,
    pub reviewer_id: String,
    pub reviewer_kind: ReviewerKind,
    pub revision_digest: ContentDigest,
    pub verdict: ReviewVerdict,
}

/// Stages one stable write-back identity for an accepted revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StageCodeWriteBackRequest {
    pub candidate_id: String,
    pub operation_id: String,
    pub revision_digest: ContentDigest,
}

/// Editable candidate discovery result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedCodeChange {
    pub candidate_id: String,
    pub editable_root: PathBuf,
    pub targets: Vec<ArtifactPath>,
}

/// Sealed revision and its canonical criterion check identities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedCodeChange {
    pub candidate_digest: ContentDigest,
    pub candidate_id: String,
    pub candidate_artifact: PackArtifactRef,
    pub pack_candidate: PackCandidateRef,
    pub required_checks: Vec<String>,
    pub revision_digest: ContentDigest,
}

/// Retained check outcome plus the exact Pack payload/reference to submit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedCodeCheck {
    pub action_payload: PackRecordCheckPayload,
    pub evidence: CheckEvidence,
    pub pack_reference: String,
}

/// Retained review outcome and current acceptance-gate result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedCodeReview {
    pub accepted: Option<ReviewedCodeChange>,
    pub check_references: Vec<String>,
    pub gate: ReviewGate,
    pub pack_reference: String,
    pub review: ReviewEvidence,
}

/// Stable, machine-readable acceptance state after recording a review.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewGate {
    Accepted,
    MissingCheck,
    FailedCheck,
    BlockingReview,
    IndependentReviewMissing,
}

/// Read-only durable workflow view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodeChangeServiceStatus {
    pub candidates: Vec<CandidateWorkflowStatus>,
    pub write_backs: Vec<WriteBackWorkflowStatus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateWorkflowStatus {
    pub candidate_id: String,
    pub editable_root: PathBuf,
    pub revisions: Vec<RevisionWorkflowStatus>,
    pub targets: Vec<ArtifactPath>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionWorkflowStatus {
    pub accepted: bool,
    pub candidate_artifact: Option<PackArtifactRef>,
    pub candidate_digest: ContentDigest,
    pub checks: Vec<String>,
    pub pack_candidate: Option<PackCandidateRef>,
    pub required_checks: Vec<String>,
    pub reviews: Vec<String>,
    pub revision_digest: ContentDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBackWorkflowStatus {
    pub operation: WriteBackOperation,
    pub status: Option<WriteBackStatus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RevisionWorkflow {
    acceptance_criteria: Vec<String>,
    authors: Vec<String>,
    binding: EvidenceBinding,
    candidate_digest: ContentDigest,
    candidate_artifact: PackArtifactRef,
    candidate_id: String,
    criteria_revision: u64,
    pack_candidate: PackCandidateRef,
    required_checks: Vec<String>,
    resource_basis: Vec<PackVersionRef>,
    revision_digest: ContentDigest,
    schema: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DurableCheck {
    action_payload: PackRecordCheckPayload,
    evidence: CheckEvidence,
    pack_reference: String,
    schema: String,
}

#[cfg(feature = "managed-local-runtime")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DurableRunCheckRequest {
    arguments: Vec<String>,
    candidate_id: String,
    check_id: String,
    checker_executable: ArtifactRef,
    checker_path: String,
    environment: BTreeMap<String, String>,
    evidence_path: ArtifactPath,
    pack_check_revision: u64,
    revision_digest: ContentDigest,
}

#[cfg(feature = "managed-local-runtime")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PendingCheck {
    check: DurableCheck,
    request: DurableRunCheckRequest,
    schema: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DurableReview {
    pack_reference: String,
    review: ReviewEvidence,
    schema: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DurableAcceptance {
    reviewed: ReviewedCodeChange,
    schema: String,
}

/// Owner-only reviewed-code-change module. Filesystem, evidence, process, and
/// reconciliation complexity stay behind this interface.
#[derive(Debug)]
pub struct CodeChangeService {
    workspace: CodeChangeWorkspace,
    journal_root: PathBuf,
    serial: Mutex<()>,
}

impl CodeChangeService {
    /// Opens one workflow store paired with an explicitly authorized root.
    ///
    /// # Errors
    ///
    /// Fails closed when protected state cannot be created or verified.
    pub fn open(
        authorized_root: &Path,
        protected_root: &Path,
    ) -> Result<Self, CodeChangeServiceError> {
        let workspace = CodeChangeWorkspace::open(authorized_root, protected_root)?;
        let journal_root = prepare_data_directory(
            &workspace
                .artifacts()
                .protected_root()
                .join("reviewed-code-change"),
        )
        .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        prepare_data_directory(&journal_root.join("candidates"))
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        Ok(Self {
            workspace,
            journal_root,
            serial: Mutex::new(()),
        })
    }

    #[must_use]
    pub const fn workspace(&self) -> &CodeChangeWorkspace {
        &self.workspace
    }

    /// Creates or idempotently reopens one isolated candidate.
    ///
    /// # Errors
    ///
    /// Rejects invalid identities, unsafe targets, or target-set rebinding.
    pub fn prepare(
        &self,
        request: &PrepareCodeChangeRequest,
    ) -> Result<PreparedCodeChange, CodeChangeServiceError> {
        let draft = self
            .workspace
            .prepare_candidate(&request.candidate_id, &request.targets)?;
        Ok(prepared_view(&draft))
    }

    /// Seals the current candidate bytes and durably binds validation policy.
    /// Criterion checks are always `criterion-1..N` in supplied acceptance
    /// order, matching the Agent Swarm Pack contract.
    ///
    /// # Errors
    ///
    /// Rejects invalid Pack identities, duplicate authors/inputs/resources,
    /// empty criteria, or any attempt to rebind an existing revision.
    pub fn seal(
        &self,
        request: &SealCodeChangeRequest,
    ) -> Result<SealedCodeChange, CodeChangeServiceError> {
        validate_seal_request(request)?;
        let _guard = self.lock()?;
        let draft = self.workspace.open_candidate(&request.candidate_id)?;
        let revision = self.workspace.seal_candidate(&draft)?;
        let criteria_bytes = canonical_bytes(&request.acceptance_criteria)?;
        let criteria = self.workspace.artifacts().store_bytes(&criteria_bytes)?;
        let inputs = request
            .inputs
            .iter()
            .map(|input| {
                Ok(VersionedInput {
                    name: input.name.clone(),
                    artifact: self.workspace.artifacts().capture(&input.path)?,
                })
            })
            .collect::<Result<Vec<_>, ArtifactError>>()?;
        let binding = EvidenceBinding::new(&revision, inputs, criteria)?;
        let required_checks = required_check_ids(request.acceptance_criteria.len());
        ReviewPolicy::new(required_checks.clone())?;
        let candidate_artifact = self.pack_candidate_artifact(request, &revision)?;
        let workflow = RevisionWorkflow {
            acceptance_criteria: request.acceptance_criteria.clone(),
            authors: sorted_unique_identities(&request.authors)?,
            binding,
            candidate_digest: revision.candidate_digest(),
            candidate_artifact,
            candidate_id: request.candidate_id.clone(),
            criteria_revision: request.criteria_revision,
            pack_candidate: request.pack_candidate.clone(),
            required_checks: required_checks.clone(),
            resource_basis: request.resource_basis.clone(),
            revision_digest: revision.revision_digest(),
            schema: WORKFLOW_SCHEMA.to_owned(),
        };
        let root = self.prepare_revision_root(&workflow)?;
        let path = root.join("workflow.json");
        if !publish_protected_json(&path, &workflow)? {
            let retained: RevisionWorkflow = read_protected_json(&path)?;
            if retained != workflow {
                return Err(CodeChangeServiceError::RevisionRebound);
            }
        }
        Ok(sealed_view(&revision, &workflow))
    }

    fn pack_candidate_artifact(
        &self,
        request: &SealCodeChangeRequest,
        revision: &CodeCandidateRevision,
    ) -> Result<PackArtifactRef, CodeChangeServiceError> {
        match (
            request.pack_candidate_artifact.as_ref(),
            request.pack_candidate_path.as_ref(),
        ) {
            (Some(authoritative), Some(path)) => {
                let expected_path = self
                    .workspace
                    .artifacts()
                    .authorized_root()
                    .join(path.to_path_buf());
                if Path::new(&authoritative.local_path) != expected_path {
                    return Err(CodeChangeServiceError::InvalidRequest);
                }
                self.workspace.artifacts().capture_authoritative(
                    path,
                    &AuthoritativeArtifactRef {
                        artifact_id: authoritative.artifact_id.clone(),
                        digest: authoritative.digest.clone(),
                        local_path: authoritative.local_path.clone(),
                        media_type: authoritative.media_type.clone(),
                    },
                )?;
                Ok(authoritative.clone())
            }
            (None, None) => {
                let manifest = PackCandidateManifest {
                    candidate_digest: revision.candidate_digest(),
                    candidate_id: request.candidate_id.clone(),
                    pack_candidate: request.pack_candidate.clone(),
                    revision_digest: revision.revision_digest(),
                    schema: CANDIDATE_MANIFEST_SCHEMA.to_owned(),
                    targets: revision.targets().to_vec(),
                };
                let bytes = canonical_bytes(&manifest)?;
                let path = ArtifactPath::new(format!(
                    "agent-swarm-candidate-{}-{}.json",
                    request.pack_candidate.candidate_id, request.pack_candidate.version
                ))?;
                let artifact = self
                    .workspace
                    .artifacts()
                    .publish_generated(&path, &bytes)?;
                let local_path = self
                    .workspace
                    .artifacts()
                    .authorized_root()
                    .join(path.to_path_buf())
                    .to_str()
                    .ok_or(CodeChangeServiceError::InvalidRequest)?
                    .to_owned();
                Ok(PackArtifactRef {
                    artifact_id: format!(
                        "code-candidate:{}:{}",
                        request.pack_candidate.candidate_id, request.pack_candidate.version
                    ),
                    digest: artifact.digest().to_string(),
                    local_path,
                    media_type: CANDIDATE_MANIFEST_MEDIA_TYPE.to_owned(),
                })
            }
            _ => Err(CodeChangeServiceError::InvalidRequest),
        }
    }

    /// Runs and persists one required criterion check through the owned native
    /// process guard, then publishes canonical JSON evidence inside the
    /// authorized working root.
    ///
    /// # Errors
    ///
    /// Rejects duplicate outcomes, non-required check IDs, stale revisions,
    /// unsafe evidence paths, or uncontained checker execution.
    #[cfg(feature = "managed-local-runtime")]
    pub fn run_check(
        &self,
        request: &RunCodeCheckRequest,
        process_guard: &Path,
    ) -> Result<RecordedCodeCheck, CodeChangeServiceError> {
        self.run_check_while(request, process_guard, &mut || true)
    }

    /// Caller-controlled version used by autonomous delivery for Stop, scope
    /// invalidation and check deadlines. Interrupted checks are not accepted.
    #[cfg(feature = "managed-local-runtime")]
    pub(crate) fn run_check_while(
        &self,
        request: &RunCodeCheckRequest,
        process_guard: &Path,
        continue_running: &mut dyn FnMut() -> bool,
    ) -> Result<RecordedCodeCheck, CodeChangeServiceError> {
        if request.pack_check_revision == 0 {
            return Err(CodeChangeServiceError::InvalidRequest);
        }
        let workflow = self.read_workflow(&request.candidate_id, request.revision_digest)?;
        let _guard = self.lock()?;
        if !workflow
            .required_checks
            .iter()
            .any(|check| check == &request.check_id)
        {
            return Err(CodeChangeServiceError::CheckNotRequired);
        }
        let revision_root = self.revision_root(&request.candidate_id, request.revision_digest);
        let check_path = revision_root
            .join("checks")
            .join(format!("{}.json", request.check_id));
        let pending_path = revision_root
            .join("pending-checks")
            .join(format!("{}.json", request.check_id));
        let durable_request = self.durable_run_check_request(request)?;
        if pending_path.exists() {
            let pending: PendingCheck = read_protected_json(&pending_path)?;
            return self.finalize_pending_check(
                request,
                &workflow,
                &durable_request,
                &pending,
                &check_path,
            );
        }
        if check_path.exists() {
            return Err(CodeChangeServiceError::CheckAlreadyRecorded);
        }
        if self
            .workspace
            .artifacts()
            .capture_if_present(&request.evidence_path)?
            .is_some()
        {
            return Err(CodeChangeServiceError::EvidencePathOccupied);
        }
        let revision = self
            .workspace
            .open_candidate_revision(&request.candidate_id, request.revision_digest)?;
        let mut command = CheckCommand::new(
            request.check_id.clone(),
            request.program.clone(),
            request.arguments.clone(),
        )?;
        for (name, value) in &request.environment {
            command = command.with_environment(name.clone(), value.clone())?;
        }
        let evidence = CheckRunner::new(&self.workspace).run_guarded_while(
            &revision,
            &workflow.binding,
            &command,
            process_guard,
            continue_running,
        )?;
        let evidence_bytes = canonical_bytes(&evidence)?;
        let artifact = self.workspace.artifacts().store_bytes(&evidence_bytes)?;
        let durable =
            self.build_durable_check(&workflow, request, &durable_request, evidence, &artifact)?;
        let pending = PendingCheck {
            check: durable,
            request: durable_request.clone(),
            schema: PENDING_CHECK_SCHEMA.to_owned(),
        };
        if !publish_protected_json(&pending_path, &pending)? {
            let retained: PendingCheck = read_protected_json(&pending_path)?;
            if retained != pending {
                return Err(CodeChangeServiceError::CheckAlreadyRecorded);
            }
        }
        self.finalize_pending_check(request, &workflow, &durable_request, &pending, &check_path)
    }

    /// Records an immutable review, then evaluates all exact checks and all
    /// reviews for the revision. An incomplete gate remains a durable,
    /// inspectable state rather than a success claim.
    ///
    /// # Errors
    ///
    /// Rejects stale/rebound identities, duplicate review IDs, or recording
    /// any new review after acceptance.
    pub fn record_review(
        &self,
        request: &RecordCodeReviewRequest,
    ) -> Result<RecordedCodeReview, CodeChangeServiceError> {
        if request.pack_review_revision == 0 {
            return Err(CodeChangeServiceError::InvalidRequest);
        }
        let workflow = self.read_workflow(&request.candidate_id, request.revision_digest)?;
        let _guard = self.lock()?;
        if self
            .read_acceptance(&request.candidate_id, request.revision_digest)?
            .is_some()
        {
            return Err(CodeChangeServiceError::RevisionClosed);
        }
        let review = ReviewEvidence::new(
            request.review_id.clone(),
            request.reviewer_id.clone(),
            request.reviewer_kind,
            workflow.binding.clone(),
            request.verdict,
            request.findings.clone(),
        )?;
        let pack_reference = format!(
            "review:{}:{}",
            request.review_id, request.pack_review_revision
        );
        let durable = DurableReview {
            pack_reference: pack_reference.clone(),
            review: review.clone(),
            schema: REVIEW_SCHEMA.to_owned(),
        };
        let path = self
            .revision_root(&request.candidate_id, request.revision_digest)
            .join("reviews")
            .join(format!("{}.json", request.review_id));
        if !publish_protected_json(&path, &durable)? {
            let retained: DurableReview = read_protected_json(&path)?;
            if retained != durable {
                return Err(CodeChangeServiceError::ReviewAlreadyRecorded);
            }
        }
        let (gate, accepted, check_references) = self.evaluate(&workflow)?;
        Ok(RecordedCodeReview {
            accepted,
            check_references,
            gate,
            pack_reference,
            review,
        })
    }

    /// Stages an accepted exact revision for restart-safe write-back.
    ///
    /// # Errors
    ///
    /// Rejects unreviewed revisions or operation identity rebinding.
    pub fn stage_write_back(
        &self,
        request: &StageCodeWriteBackRequest,
    ) -> Result<WriteBackOperation, CodeChangeServiceError> {
        self.read_workflow(&request.candidate_id, request.revision_digest)?;
        if self
            .read_acceptance(&request.candidate_id, request.revision_digest)?
            .is_none()
        {
            return Err(CodeChangeServiceError::RevisionNotAccepted);
        }
        let revision = self
            .workspace
            .open_candidate_revision(&request.candidate_id, request.revision_digest)?;
        self.workspace
            .stage_write_back(&request.operation_id, &revision)
            .map_err(Into::into)
    }

    /// Reconciles one retained operation, including lost-reply recovery.
    ///
    /// # Errors
    ///
    /// Rejects missing/corrupt operation state; uncertainty is returned as a
    /// non-success status.
    pub fn reconcile_write_back(
        &self,
        operation_id: &str,
    ) -> Result<WriteBackStatus, CodeChangeServiceError> {
        let operation = self.workspace.open_write_back(operation_id)?;
        self.workspace
            .reconcile_write_back(&operation)
            .map_err(Into::into)
    }

    /// Returns all candidates, immutable revisions, evidence references, and
    /// write-back state without executing a checker or applying an effect.
    ///
    /// # Errors
    ///
    /// Fails closed for corrupt protected state.
    pub fn status(&self) -> Result<CodeChangeServiceStatus, CodeChangeServiceError> {
        let mut candidates = Vec::new();
        for draft in self.workspace.list_candidates()? {
            let mut revisions = Vec::new();
            for revision in self
                .workspace
                .list_candidate_revisions(draft.candidate_id())?
            {
                let workflow =
                    self.read_workflow_optional(draft.candidate_id(), revision.revision_digest())?;
                let checks =
                    self.read_checks_optional(draft.candidate_id(), revision.revision_digest())?;
                let reviews =
                    self.read_reviews_optional(draft.candidate_id(), revision.revision_digest())?;
                let accepted = self
                    .read_acceptance(draft.candidate_id(), revision.revision_digest())?
                    .is_some();
                revisions.push(RevisionWorkflowStatus {
                    accepted,
                    candidate_artifact: workflow
                        .as_ref()
                        .map(|item| item.candidate_artifact.clone()),
                    candidate_digest: revision.candidate_digest(),
                    checks: checks
                        .into_iter()
                        .map(|check| check.pack_reference)
                        .collect(),
                    pack_candidate: workflow.as_ref().map(|item| item.pack_candidate.clone()),
                    required_checks: workflow.map_or_else(Vec::new, |item| item.required_checks),
                    reviews: reviews
                        .into_iter()
                        .map(|review| review.pack_reference)
                        .collect(),
                    revision_digest: revision.revision_digest(),
                });
            }
            candidates.push(CandidateWorkflowStatus {
                candidate_id: draft.candidate_id().to_owned(),
                editable_root: draft.root().to_path_buf(),
                revisions,
                targets: draft
                    .targets()
                    .iter()
                    .map(|target| target.path().clone())
                    .collect(),
            });
        }
        let write_backs = self
            .workspace
            .list_write_backs()?
            .into_iter()
            .map(|operation| {
                let status = self.workspace.write_back_status(&operation)?;
                Ok(WriteBackWorkflowStatus { operation, status })
            })
            .collect::<Result<Vec<_>, CodeChangeError>>()?;
        Ok(CodeChangeServiceStatus {
            candidates,
            write_backs,
        })
    }

    #[cfg(feature = "managed-local-runtime")]
    fn durable_run_check_request(
        &self,
        request: &RunCodeCheckRequest,
    ) -> Result<DurableRunCheckRequest, CodeChangeServiceError> {
        let checker = fs::canonicalize(&request.program)
            .map_err(|_| CodeChangeServiceError::InvalidRequest)?;
        let checker_path = checker
            .to_str()
            .ok_or(CodeChangeServiceError::InvalidRequest)?
            .to_owned();
        let checker_executable = self
            .workspace
            .artifacts()
            .store_bytes(&read_stable_regular_file(&checker)?)?;
        Ok(DurableRunCheckRequest {
            arguments: request.arguments.clone(),
            candidate_id: request.candidate_id.clone(),
            check_id: request.check_id.clone(),
            checker_executable,
            checker_path,
            environment: request.environment.clone(),
            evidence_path: request.evidence_path.clone(),
            pack_check_revision: request.pack_check_revision,
            revision_digest: request.revision_digest,
        })
    }

    #[cfg(feature = "managed-local-runtime")]
    fn build_durable_check(
        &self,
        workflow: &RevisionWorkflow,
        request: &RunCodeCheckRequest,
        durable_request: &DurableRunCheckRequest,
        evidence: CheckEvidence,
        artifact: &ArtifactRef,
    ) -> Result<DurableCheck, CodeChangeServiceError> {
        if evidence.check_id != request.check_id
            || evidence.binding != workflow.binding
            || evidence.checker.executable_path != durable_request.checker_path
            || evidence.checker.executable != durable_request.checker_executable
            || evidence.checker.arguments != durable_request.arguments
            || evidence.checker.environment != durable_request.environment
        {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        self.verify_check_evidence(&evidence)?;
        let local_path = self
            .workspace
            .artifacts()
            .authorized_root()
            .join(request.evidence_path.to_path_buf())
            .to_str()
            .ok_or(CodeChangeServiceError::InvalidRequest)?
            .to_owned();
        let criterion_index = workflow
            .required_checks
            .iter()
            .position(|check_id| check_id == &request.check_id)
            .ok_or(CodeChangeServiceError::JournalCorrupt)?;
        let criterion = workflow
            .acceptance_criteria
            .get(criterion_index)
            .cloned()
            .ok_or(CodeChangeServiceError::JournalCorrupt)?;
        let evidence_ref = PackCheckEvidenceRef {
            artifact: PackArtifactRef {
                artifact_id: format!(
                    "check-evidence:{}:{}:{}",
                    workflow.pack_candidate.candidate_id,
                    workflow.pack_candidate.version,
                    request.check_id
                ),
                digest: artifact.digest().to_string(),
                local_path,
                media_type: CHECK_EVIDENCE_MEDIA_TYPE.to_owned(),
            },
            candidate: workflow.pack_candidate.clone(),
            candidate_artifact: workflow.candidate_artifact.clone(),
            check_id: request.check_id.clone(),
            criterion,
            criteria_revision: workflow.criteria_revision,
            resource_basis: workflow.resource_basis.clone(),
        };
        let status = if evidence.passed() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        };
        Ok(DurableCheck {
            action_payload: PackRecordCheckPayload {
                candidate: workflow.pack_candidate.clone(),
                check_id: request.check_id.clone(),
                evidence_refs: vec![evidence_ref],
                expected_criteria_revision: workflow.criteria_revision,
                resource_basis: workflow.resource_basis.clone(),
                status,
            },
            evidence,
            pack_reference: format!("check:{}:{}", request.check_id, request.pack_check_revision),
            schema: CHECK_SCHEMA.to_owned(),
        })
    }

    #[cfg(feature = "managed-local-runtime")]
    fn finalize_pending_check(
        &self,
        request: &RunCodeCheckRequest,
        workflow: &RevisionWorkflow,
        durable_request: &DurableRunCheckRequest,
        pending: &PendingCheck,
        check_path: &Path,
    ) -> Result<RecordedCodeCheck, CodeChangeServiceError> {
        if pending.schema != PENDING_CHECK_SCHEMA {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        if pending.request != *durable_request {
            return Err(CodeChangeServiceError::CheckAlreadyRecorded);
        }
        let evidence_bytes = canonical_bytes(&pending.check.evidence)?;
        let artifact = self.workspace.artifacts().store_bytes(&evidence_bytes)?;
        let expected = self.build_durable_check(
            workflow,
            request,
            durable_request,
            pending.check.evidence.clone(),
            &artifact,
        )?;
        if pending.check != expected {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        let published = self
            .workspace
            .artifacts()
            .publish_generated(&request.evidence_path, &evidence_bytes)?;
        if published != artifact {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        if !publish_protected_json(check_path, &pending.check)? {
            let retained: DurableCheck = read_protected_json(check_path)?;
            if retained != pending.check {
                return Err(CodeChangeServiceError::CheckAlreadyRecorded);
            }
        }
        Ok(recorded_check(&pending.check))
    }

    #[cfg(feature = "managed-local-runtime")]
    fn verify_check_evidence(
        &self,
        evidence: &CheckEvidence,
    ) -> Result<(), CodeChangeServiceError> {
        let artifacts = self.workspace.artifacts();
        artifacts.verify(&evidence.binding.criteria)?;
        for input in &evidence.binding.inputs {
            artifacts.verify(&input.artifact)?;
        }
        artifacts.verify(&evidence.checker.executable)?;
        artifacts.verify(&evidence.stdout)?;
        artifacts.verify(&evidence.stderr)?;
        Ok(())
    }

    fn evaluate(
        &self,
        workflow: &RevisionWorkflow,
    ) -> Result<(ReviewGate, Option<ReviewedCodeChange>, Vec<String>), CodeChangeServiceError> {
        let revision = self
            .workspace
            .open_candidate_revision(&workflow.candidate_id, workflow.revision_digest)?;
        let checks = self.read_checks(&workflow.candidate_id, workflow.revision_digest)?;
        let reviews = self.read_reviews(&workflow.candidate_id, workflow.revision_digest)?;
        let check_references = workflow
            .required_checks
            .iter()
            .filter_map(|required| {
                checks
                    .iter()
                    .find(|check| check.evidence.check_id == *required)
                    .map(|check| check.pack_reference.clone())
            })
            .collect::<Vec<_>>();
        let policy = ReviewPolicy::new(workflow.required_checks.clone())?;
        match policy.accept(
            &self.workspace,
            &revision,
            &workflow.binding,
            &workflow.authors,
            &checks
                .into_iter()
                .map(|check| check.evidence)
                .collect::<Vec<_>>(),
            &reviews
                .into_iter()
                .map(|review| review.review)
                .collect::<Vec<_>>(),
        ) {
            Ok(reviewed) => {
                let durable = DurableAcceptance {
                    reviewed: reviewed.clone(),
                    schema: ACCEPTED_SCHEMA.to_owned(),
                };
                let path = self
                    .revision_root(&workflow.candidate_id, workflow.revision_digest)
                    .join("accepted.json");
                if !publish_protected_json(&path, &durable)? {
                    let retained: DurableAcceptance = read_protected_json(&path)?;
                    if retained != durable {
                        return Err(CodeChangeServiceError::RevisionRebound);
                    }
                }
                Ok((ReviewGate::Accepted, Some(reviewed), check_references))
            }
            Err(ReviewError::MissingCheck) => {
                Ok((ReviewGate::MissingCheck, None, check_references))
            }
            Err(ReviewError::FailedCheck) => Ok((ReviewGate::FailedCheck, None, check_references)),
            Err(ReviewError::BlockingReview) => {
                Ok((ReviewGate::BlockingReview, None, check_references))
            }
            Err(ReviewError::IndependentReviewMissing) => {
                Ok((ReviewGate::IndependentReviewMissing, None, check_references))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn prepare_revision_root(
        &self,
        workflow: &RevisionWorkflow,
    ) -> Result<PathBuf, CodeChangeServiceError> {
        let root = self.revision_root(&workflow.candidate_id, workflow.revision_digest);
        prepare_data_directory(&root).map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        prepare_data_directory(&root.join("checks"))
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        prepare_data_directory(&root.join("pending-checks"))
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        prepare_data_directory(&root.join("reviews"))
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        Ok(root)
    }

    fn revision_root(&self, candidate_id: &str, revision_digest: ContentDigest) -> PathBuf {
        self.journal_root
            .join("candidates")
            .join(candidate_id)
            .join(revision_digest.hex())
    }

    fn read_workflow(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<RevisionWorkflow, CodeChangeServiceError> {
        self.read_workflow_optional(candidate_id, revision_digest)?
            .ok_or(CodeChangeServiceError::WorkflowMissing)
    }

    fn read_workflow_optional(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<Option<RevisionWorkflow>, CodeChangeServiceError> {
        validate_identifier(candidate_id)?;
        let path = self
            .revision_root(candidate_id, revision_digest)
            .join("workflow.json");
        if !path.exists() {
            return Ok(None);
        }
        let workflow: RevisionWorkflow = read_protected_json(&path)?;
        if workflow.schema != WORKFLOW_SCHEMA
            || workflow.candidate_id != candidate_id
            || workflow.revision_digest != revision_digest
        {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        Ok(Some(workflow))
    }

    fn read_checks(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<Vec<DurableCheck>, CodeChangeServiceError> {
        self.read_checks_optional(candidate_id, revision_digest)
    }

    fn read_checks_optional(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<Vec<DurableCheck>, CodeChangeServiceError> {
        read_records(
            &self
                .revision_root(candidate_id, revision_digest)
                .join("checks"),
            |record: &DurableCheck| record.schema == CHECK_SCHEMA,
        )
    }

    fn read_reviews(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<Vec<DurableReview>, CodeChangeServiceError> {
        self.read_reviews_optional(candidate_id, revision_digest)
    }

    fn read_reviews_optional(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<Vec<DurableReview>, CodeChangeServiceError> {
        read_records(
            &self
                .revision_root(candidate_id, revision_digest)
                .join("reviews"),
            |record: &DurableReview| record.schema == REVIEW_SCHEMA,
        )
    }

    fn read_acceptance(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<Option<DurableAcceptance>, CodeChangeServiceError> {
        let path = self
            .revision_root(candidate_id, revision_digest)
            .join("accepted.json");
        if !path.exists() {
            return Ok(None);
        }
        let accepted: DurableAcceptance = read_protected_json(&path)?;
        if accepted.schema != ACCEPTED_SCHEMA
            || accepted.reviewed.revision_digest != revision_digest
        {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        Ok(Some(accepted))
    }

    fn lock(&self) -> Result<ServiceGuard<'_>, CodeChangeServiceError> {
        let process = self
            .serial
            .lock()
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        let path = self.journal_root.join("workflow.lock");
        let file = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                validate_owner_only_file(&path)
                    .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|_| CodeChangeServiceError::JournalUnavailable)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => create_owner_only_file(&path)
                .map_err(|_| CodeChangeServiceError::JournalUnavailable)?,
            Ok(_) | Err(_) => return Err(CodeChangeServiceError::JournalUnavailable),
        };
        file.lock()
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        Ok(ServiceGuard {
            _process: process,
            _file: file,
        })
    }
}

struct ServiceGuard<'a> {
    _process: std::sync::MutexGuard<'a, ()>,
    _file: File,
}

/// Closed workflow failures. No error variant claims a checker or write-back
/// succeeded; uncertain effects are returned only as `WriteBackStatus`.
#[derive(Debug, Error)]
pub enum CodeChangeServiceError {
    #[error("code-change request is invalid")]
    InvalidRequest,
    #[error("protected code-change journal is unavailable")]
    JournalUnavailable,
    #[error("protected code-change journal is corrupt")]
    JournalCorrupt,
    #[error("sealed revision has no retained workflow")]
    WorkflowMissing,
    #[error("sealed revision identity is already bound to different workflow state")]
    RevisionRebound,
    #[error("check is not a canonical criterion required by this revision")]
    CheckNotRequired,
    #[error("this Candidate and criterion already have an immutable Check outcome")]
    CheckAlreadyRecorded,
    #[error("the requested evidence path is already occupied")]
    EvidencePathOccupied,
    #[error("review identity is already bound to different evidence")]
    ReviewAlreadyRecorded,
    #[error("accepted revision is closed to additional review evidence")]
    RevisionClosed,
    #[error("write-back requires an accepted reviewed revision")]
    RevisionNotAccepted,
    #[error("canonical evidence encoding failed")]
    CanonicalEvidence,
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    #[error(transparent)]
    Check(#[from] CheckError),
    #[error(transparent)]
    CodeChange(#[from] CodeChangeError),
    #[error(transparent)]
    Review(#[from] ReviewError),
}

fn validate_seal_request(request: &SealCodeChangeRequest) -> Result<(), CodeChangeServiceError> {
    validate_identifier(&request.candidate_id)?;
    validate_identifier(&request.pack_candidate.candidate_id)?;
    if request.pack_candidate.candidate_id != request.candidate_id
        || request.pack_candidate.version == 0
        || request.criteria_revision == 0
        || request.acceptance_criteria.is_empty()
        || request.acceptance_criteria.len() > MAX_IDENTITIES
        || request
            .acceptance_criteria
            .iter()
            .any(|criterion| criterion.trim().is_empty() || criterion.len() > 4096)
        || request.inputs.len() > 256
        || request.resource_basis.len() > 256
        || request.pack_candidate_artifact.is_some() != request.pack_candidate_path.is_some()
        || request
            .pack_candidate_artifact
            .as_ref()
            .is_some_and(|artifact| {
                artifact.artifact_id.trim().is_empty()
                    || artifact.digest.trim().is_empty()
                    || artifact.local_path.trim().is_empty()
                    || artifact.media_type.trim().is_empty()
            })
    {
        return Err(CodeChangeServiceError::InvalidRequest);
    }
    let mut input_names = HashSet::new();
    if request
        .inputs
        .iter()
        .any(|input| !input_names.insert(input.name.as_str()))
    {
        return Err(CodeChangeServiceError::InvalidRequest);
    }
    let mut resource_ids = HashSet::new();
    for resource in &request.resource_basis {
        validate_identifier(&resource.resource_id)?;
        if resource.version == 0 || !resource_ids.insert(resource.resource_id.as_str()) {
            return Err(CodeChangeServiceError::InvalidRequest);
        }
    }
    Ok(())
}

fn sorted_unique_identities(values: &[String]) -> Result<Vec<String>, CodeChangeServiceError> {
    if values.is_empty() || values.len() > MAX_IDENTITIES {
        return Err(CodeChangeServiceError::InvalidRequest);
    }
    let mut values = values.to_vec();
    for value in &values {
        validate_identifier(value)?;
    }
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CodeChangeServiceError::InvalidRequest);
    }
    Ok(values)
}

fn required_check_ids(count: usize) -> Vec<String> {
    (1..=count)
        .map(|index| format!("criterion-{index}"))
        .collect()
}

fn prepared_view(draft: &CodeCandidateDraft) -> PreparedCodeChange {
    PreparedCodeChange {
        candidate_id: draft.candidate_id().to_owned(),
        editable_root: draft.root().to_path_buf(),
        targets: draft
            .targets()
            .iter()
            .map(|target| target.path().clone())
            .collect(),
    }
}

fn sealed_view(revision: &CodeCandidateRevision, workflow: &RevisionWorkflow) -> SealedCodeChange {
    SealedCodeChange {
        candidate_digest: revision.candidate_digest(),
        candidate_id: revision.candidate_id().to_owned(),
        candidate_artifact: workflow.candidate_artifact.clone(),
        pack_candidate: workflow.pack_candidate.clone(),
        required_checks: workflow.required_checks.clone(),
        revision_digest: revision.revision_digest(),
    }
}

#[cfg(feature = "managed-local-runtime")]
fn recorded_check(check: &DurableCheck) -> RecordedCodeCheck {
    RecordedCodeCheck {
        action_payload: check.action_payload.clone(),
        evidence: check.evidence.clone(),
        pack_reference: check.pack_reference.clone(),
    }
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CodeChangeServiceError> {
    let encoded =
        serde_json::to_vec(value).map_err(|_| CodeChangeServiceError::CanonicalEvidence)?;
    CanonicalJsonV1::parse(&encoded)
        .and_then(|canonical| canonical.to_bytes())
        .map_err(|_| CodeChangeServiceError::CanonicalEvidence)
}

fn read_records<T>(
    root: &Path,
    valid: impl Fn(&T) -> bool,
) -> Result<Vec<T>, CodeChangeServiceError>
where
    T: serde::de::DeserializeOwned,
{
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) | Err(_) => return Err(CodeChangeServiceError::JournalCorrupt),
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| CodeChangeServiceError::JournalUnavailable)? {
        let entry = entry.map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        let file_type = entry
            .file_type()
            .map_err(|_| CodeChangeServiceError::JournalUnavailable)?;
        if !file_type.is_file()
            || file_type.is_symlink()
            || entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
        {
            return Err(CodeChangeServiceError::JournalCorrupt);
        }
        paths.push(entry.path());
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let record = read_protected_json(&path)?;
            if !valid(&record) {
                return Err(CodeChangeServiceError::JournalCorrupt);
            }
            Ok(record)
        })
        .collect()
}
