//! Copy-isolated code candidates and restart-safe write-back reconciliation.
//!
//! Preparing a candidate captures each authorized target's baseline and creates
//! a separate editable copy. Sealing it creates an immutable revision bound to
//! both baseline and candidate digests. Applying a revision is deliberately a
//! two-step operation: [`CodeChangeWorkspace::stage_write_back`] durably binds a
//! stable operation identity, then [`CodeChangeWorkspace::reconcile_write_back`]
//! compares observed target bytes before attempting or recovering each effect.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::artifacts::{
    ArtifactError, ArtifactPath, ArtifactRef, ArtifactWorkspace, ContentDigest,
    read_stable_regular_file,
};

const CANDIDATE_SCHEMA: &str = "worldstream/agent-swarm-code-candidate/v1";
const CANDIDATE_REVISION_SCHEMA: &str = "worldstream/agent-swarm-code-candidate-revision/v1";
const WRITE_BACK_INTENT_SCHEMA: &str = "worldstream/agent-swarm-write-back-intent/v1";
const WRITE_BACK_RESULT_SCHEMA: &str = "worldstream/agent-swarm-write-back-result/v1";
const MAX_TARGETS: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One isolated, editable candidate and its captured baselines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeCandidateDraft {
    candidate_id: String,
    root: PathBuf,
    targets: Vec<CandidateBaseline>,
}

impl CodeCandidateDraft {
    #[must_use]
    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn targets(&self) -> &[CandidateBaseline] {
        &self.targets
    }

    /// Returns the editable candidate path for an authorized target.
    ///
    /// # Errors
    ///
    /// Rejects paths not fixed when the candidate was prepared.
    pub fn path(&self, target: &ArtifactPath) -> Result<PathBuf, CodeChangeError> {
        if !self.targets.iter().any(|entry| &entry.path == target) {
            return Err(CodeChangeError::TargetNotAuthorized);
        }
        Ok(self.root.join(target.to_path_buf()))
    }
}

/// The exact original version captured for one candidate target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateBaseline {
    path: ArtifactPath,
    artifact: Option<ArtifactRef>,
}

impl CandidateBaseline {
    #[must_use]
    pub fn path(&self) -> &ArtifactPath {
        &self.path
    }

    #[must_use]
    pub fn artifact(&self) -> Option<&ArtifactRef> {
        self.artifact.as_ref()
    }
}

/// One path in a sealed candidate revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateTargetRevision {
    path: ArtifactPath,
    baseline: Option<ArtifactRef>,
    candidate: Option<ArtifactRef>,
}

impl CandidateTargetRevision {
    #[must_use]
    pub fn path(&self) -> &ArtifactPath {
        &self.path
    }

    #[must_use]
    pub fn baseline(&self) -> Option<&ArtifactRef> {
        self.baseline.as_ref()
    }

    #[must_use]
    pub fn candidate(&self) -> Option<&ArtifactRef> {
        self.candidate.as_ref()
    }
}

/// Immutable candidate bytes and their exact write-back preconditions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeCandidateRevision {
    record: CandidateRevisionRecord,
    candidate_root: PathBuf,
}

impl CodeCandidateRevision {
    #[must_use]
    pub fn candidate_id(&self) -> &str {
        &self.record.candidate_id
    }

    #[must_use]
    pub const fn candidate_digest(&self) -> ContentDigest {
        self.record.candidate_digest
    }

    #[must_use]
    pub const fn revision_digest(&self) -> ContentDigest {
        self.record.revision_digest
    }

    #[must_use]
    pub fn targets(&self) -> &[CandidateTargetRevision] {
        &self.record.targets
    }

    pub(crate) fn candidate_root(&self) -> &Path {
        &self.candidate_root
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateRevisionRecord {
    candidate_id: String,
    targets: Vec<CandidateTargetRevision>,
    candidate_digest: ContentDigest,
    revision_digest: ContentDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateManifest {
    schema: String,
    candidate_id: String,
    targets: Vec<CandidateBaseline>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SealedCandidateRevision {
    schema: String,
    revision: CandidateRevisionRecord,
}

/// Stable identity returned after a write-back intent is durable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBackOperation {
    operation_id: String,
    revision_digest: ContentDigest,
}

impl WriteBackOperation {
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub const fn revision_digest(&self) -> ContentDigest {
        self.revision_digest
    }
}

/// Overall status of an exact write-back operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteBackDisposition {
    Applied,
    BlockedConflict,
    ReconciliationRequired,
}

/// What reconciliation observed for one authorized target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteBackTargetOutcome {
    Applied {
        observed: Option<ArtifactRef>,
    },
    Conflict {
        /// The newer bytes are retained in the artifact store. `None` records
        /// a concurrent deletion.
        current: Option<ArtifactRef>,
    },
    Unknown {
        reason: WriteBackUncertainty,
    },
}

impl WriteBackTargetOutcome {
    #[must_use]
    pub const fn is_applied(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

/// Closed reasons an effect needs another reconciliation pass.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteBackUncertainty {
    TargetCouldNotBeObserved,
    StagedFileUnavailable,
    PublicationOutcomeUnobserved,
}

/// Durable terminal result, or a nonterminal reconciliation-required view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBackStatus {
    pub operation_id: String,
    pub revision_digest: ContentDigest,
    pub disposition: WriteBackDisposition,
    pub targets: Vec<WriteBackTargetStatus>,
}

impl WriteBackStatus {
    #[must_use]
    pub const fn succeeded(&self) -> bool {
        matches!(self.disposition, WriteBackDisposition::Applied)
    }
}

/// One target's outcome in stable path order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBackTargetStatus {
    pub path: ArtifactPath,
    pub outcome: WriteBackTargetOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WriteBackIntent {
    schema: String,
    operation_id: String,
    revision: CandidateRevisionRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WriteBackResult {
    schema: String,
    status: WriteBackStatus,
}

/// Local code-change storage. All filesystem complexity sits behind candidate
/// preparation, sealing, and stable write-back reconciliation.
#[derive(Debug)]
pub struct CodeChangeWorkspace {
    artifacts: ArtifactWorkspace,
    candidates_root: PathBuf,
    operations_root: PathBuf,
    serial: Mutex<()>,
}

impl CodeChangeWorkspace {
    /// Opens protected candidate, artifact, and operation storage for one
    /// authorized working root.
    ///
    /// # Errors
    ///
    /// Fails closed when any protected storage cannot be created and verified.
    pub fn open(authorized_root: &Path, protected_root: &Path) -> Result<Self, CodeChangeError> {
        let artifacts = ArtifactWorkspace::open(authorized_root, protected_root)?;
        let candidates_root =
            prepare_data_directory(&artifacts.protected_root().join("candidates"))
                .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let operations_root =
            prepare_data_directory(&artifacts.protected_root().join("write-back"))
                .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        Ok(Self {
            artifacts,
            candidates_root,
            operations_root,
            serial: Mutex::new(()),
        })
    }

    #[must_use]
    pub const fn artifacts(&self) -> &ArtifactWorkspace {
        &self.artifacts
    }

    /// Reopens an existing editable candidate without recapturing baselines.
    ///
    /// # Errors
    ///
    /// Rejects unknown, incomplete, or corrupt candidate state.
    pub fn open_candidate(
        &self,
        candidate_id: &str,
    ) -> Result<CodeCandidateDraft, CodeChangeError> {
        validate_identifier(candidate_id)?;
        let candidate_directory = self.candidates_root.join(candidate_id);
        let manifest: CandidateManifest =
            read_protected_json(&candidate_directory.join("candidate.json"))?;
        let targets = manifest
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        validate_manifest(&manifest, candidate_id, &targets)?;
        let root = candidate_directory.join("tree");
        let metadata =
            fs::symlink_metadata(&root).map_err(|_| CodeChangeError::IncompleteCandidate)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CodeChangeError::IncompleteCandidate);
        }
        Ok(CodeCandidateDraft {
            candidate_id: candidate_id.to_owned(),
            root,
            targets: manifest.targets,
        })
    }

    /// Lists every complete candidate in stable identifier order.
    ///
    /// # Errors
    ///
    /// Fails closed when protected candidate storage contains an unexpected
    /// entry or a corrupt candidate journal.
    pub fn list_candidates(&self) -> Result<Vec<CodeCandidateDraft>, CodeChangeError> {
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.candidates_root)
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?
        {
            let entry = entry.map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
            let file_type = entry
                .file_type()
                .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(CodeChangeError::JournalCorrupt);
            }
            let candidate_id = entry
                .file_name()
                .into_string()
                .map_err(|_| CodeChangeError::JournalCorrupt)?;
            candidates.push(self.open_candidate(&candidate_id)?);
        }
        candidates.sort_by(|left, right| left.candidate_id.cmp(&right.candidate_id));
        Ok(candidates)
    }

    /// Captures baselines and creates isolated editable copies. Reopening the
    /// same candidate ID is allowed only for the exact same target set.
    ///
    /// # Errors
    ///
    /// Rejects invalid identities, duplicate or excessive targets, unsafe
    /// paths, and an existing candidate bound to different targets.
    pub fn prepare_candidate(
        &self,
        candidate_id: &str,
        targets: &[ArtifactPath],
    ) -> Result<CodeCandidateDraft, CodeChangeError> {
        validate_identifier(candidate_id)?;
        let targets = normalized_targets(targets)?;
        let _guard = self
            .serial
            .lock()
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let candidate_directory = self.candidates_root.join(candidate_id);
        let root = candidate_directory.join("tree");
        let manifest_path = candidate_directory.join("candidate.json");
        if manifest_path.exists() {
            let manifest: CandidateManifest = read_protected_json(&manifest_path)?;
            validate_manifest(&manifest, candidate_id, &targets)?;
            return Ok(CodeCandidateDraft {
                candidate_id: candidate_id.to_owned(),
                root,
                targets: manifest.targets,
            });
        }
        if candidate_directory.exists() {
            return Err(CodeChangeError::IncompleteCandidate);
        }
        let candidate_directory = prepare_data_directory(&candidate_directory)
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let root = prepare_data_directory(&candidate_directory.join("tree"))
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let mut baselines = Vec::with_capacity(targets.len());
        for target in &targets {
            let artifact = self.artifacts.capture_if_present(target)?;
            if let Some(reference) = &artifact {
                let bytes = self.artifacts.read(reference)?;
                write_candidate_file(&root, target, &bytes)?;
                copy_executable_permission(
                    self.artifacts
                        .resolve_authorized_file_if_present(target)?
                        .as_deref(),
                    &root.join(target.to_path_buf()),
                )?;
            }
            baselines.push(CandidateBaseline {
                path: target.clone(),
                artifact,
            });
        }
        let manifest = CandidateManifest {
            schema: CANDIDATE_SCHEMA.to_owned(),
            candidate_id: candidate_id.to_owned(),
            targets: baselines.clone(),
        };
        if !publish_protected_json(&manifest_path, &manifest)? {
            return Err(CodeChangeError::CandidateConflict);
        }
        Ok(CodeCandidateDraft {
            candidate_id: candidate_id.to_owned(),
            root,
            targets: baselines,
        })
    }

    /// Captures the draft's current bytes as an immutable revision.
    ///
    /// The resulting revision remains valid if the editable draft changes
    /// later; checks can separately require the draft still matches it.
    ///
    /// # Errors
    ///
    /// Rejects forged drafts, unsafe candidate paths, or unstable bytes.
    pub fn seal_candidate(
        &self,
        draft: &CodeCandidateDraft,
    ) -> Result<CodeCandidateRevision, CodeChangeError> {
        validate_identifier(&draft.candidate_id)?;
        let expected_root = self.candidates_root.join(&draft.candidate_id).join("tree");
        if draft.root != expected_root {
            return Err(CodeChangeError::CandidateConflict);
        }
        let manifest_path = draft
            .root
            .parent()
            .ok_or(CodeChangeError::CandidateConflict)?
            .join("candidate.json");
        let manifest: CandidateManifest = read_protected_json(&manifest_path)?;
        let requested = draft
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect::<Vec<_>>();
        validate_manifest(&manifest, &draft.candidate_id, &requested)?;
        if manifest.targets != draft.targets {
            return Err(CodeChangeError::CandidateConflict);
        }
        let mut targets = Vec::with_capacity(draft.targets.len());
        for baseline in &draft.targets {
            let candidate = self
                .artifacts
                .capture_from_root_if_present(&draft.root, &baseline.path)?;
            targets.push(CandidateTargetRevision {
                path: baseline.path.clone(),
                baseline: baseline.artifact.clone(),
                candidate,
            });
        }
        let candidate_digest = candidate_content_digest(&targets);
        let revision_digest =
            candidate_revision_digest(&draft.candidate_id, &targets, candidate_digest);
        let revision = CodeCandidateRevision {
            record: CandidateRevisionRecord {
                candidate_id: draft.candidate_id.clone(),
                targets,
                candidate_digest,
                revision_digest,
            },
            candidate_root: draft.root.clone(),
        };
        self.validate_revision(&revision.record)?;
        self.persist_revision(&revision.record)?;
        Ok(revision)
    }

    /// Reopens one exact immutable candidate revision by its content-bound
    /// identity. The editable tree may since have changed; callers that need
    /// to execute checks must still call [`Self::verify_candidate`].
    ///
    /// # Errors
    ///
    /// Rejects missing, corrupt, or identity-mismatched revision state.
    pub fn open_candidate_revision(
        &self,
        candidate_id: &str,
        revision_digest: ContentDigest,
    ) -> Result<CodeCandidateRevision, CodeChangeError> {
        validate_identifier(candidate_id)?;
        let path = self.revision_path(candidate_id, revision_digest);
        let retained: SealedCandidateRevision = read_protected_json(&path)?;
        if retained.schema != CANDIDATE_REVISION_SCHEMA
            || retained.revision.candidate_id != candidate_id
            || retained.revision.revision_digest != revision_digest
        {
            return Err(CodeChangeError::CandidateConflict);
        }
        self.validate_revision(&retained.revision)?;
        Ok(CodeCandidateRevision {
            record: retained.revision,
            candidate_root: self.candidates_root.join(candidate_id).join("tree"),
        })
    }

    /// Lists exact immutable revisions for one candidate in digest order.
    ///
    /// # Errors
    ///
    /// Fails closed for unknown candidates or unexpected journal entries.
    pub fn list_candidate_revisions(
        &self,
        candidate_id: &str,
    ) -> Result<Vec<CodeCandidateRevision>, CodeChangeError> {
        self.open_candidate(candidate_id)?;
        let root = self.candidates_root.join(candidate_id).join("revisions");
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) | Err(_) => return Err(CodeChangeError::JournalCorrupt),
        }
        let mut revisions = Vec::new();
        for entry in fs::read_dir(root).map_err(|_| CodeChangeError::JournalCorrupt)? {
            let entry = entry.map_err(|_| CodeChangeError::JournalCorrupt)?;
            let file_type = entry
                .file_type()
                .map_err(|_| CodeChangeError::JournalCorrupt)?;
            if !file_type.is_file() || file_type.is_symlink() {
                return Err(CodeChangeError::JournalCorrupt);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| CodeChangeError::JournalCorrupt)?;
            let digest = name
                .strip_suffix(".json")
                .ok_or(CodeChangeError::JournalCorrupt)?;
            let digest = format!("blake3:{digest}")
                .parse()
                .map_err(|_| CodeChangeError::JournalCorrupt)?;
            revisions.push(self.open_candidate_revision(candidate_id, digest)?);
        }
        revisions.sort_by_key(CodeCandidateRevision::revision_digest);
        Ok(revisions)
    }

    /// Confirms that the editable candidate still has the sealed content.
    ///
    /// # Errors
    ///
    /// Returns `CandidateChanged` instead of allowing evidence for another
    /// material version to be attached to this revision.
    pub fn verify_candidate(
        &self,
        revision: &CodeCandidateRevision,
    ) -> Result<(), CodeChangeError> {
        self.validate_revision(&revision.record)?;
        if revision.candidate_root
            != self
                .candidates_root
                .join(&revision.record.candidate_id)
                .join("tree")
        {
            return Err(CodeChangeError::CandidateConflict);
        }
        let mut current = Vec::with_capacity(revision.record.targets.len());
        for target in &revision.record.targets {
            let artifact = self
                .artifacts
                .capture_from_root_if_present(&revision.candidate_root, &target.path)?;
            current.push(CandidateTargetRevision {
                path: target.path.clone(),
                baseline: target.baseline.clone(),
                candidate: artifact,
            });
        }
        if candidate_content_digest(&current) != revision.record.candidate_digest {
            return Err(CodeChangeError::CandidateChanged);
        }
        Ok(())
    }

    pub(crate) fn validate_sealed_revision(
        &self,
        revision: &CodeCandidateRevision,
    ) -> Result<(), CodeChangeError> {
        self.validate_revision(&revision.record)
    }

    /// Durably binds a stable operation identity to exactly one sealed
    /// revision. This method performs no write-back effect.
    ///
    /// # Errors
    ///
    /// Reusing an operation ID for another revision returns
    /// `OperationIdentityConflict`.
    pub fn stage_write_back(
        &self,
        operation_id: &str,
        revision: &CodeCandidateRevision,
    ) -> Result<WriteBackOperation, CodeChangeError> {
        validate_identifier(operation_id)?;
        self.validate_revision(&revision.record)?;
        let _guard = self.lock_operations()?;
        let operation_root = prepare_data_directory(&self.operations_root.join(operation_id))
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let intent_path = operation_root.join("intent.json");
        let expected = WriteBackIntent {
            schema: WRITE_BACK_INTENT_SCHEMA.to_owned(),
            operation_id: operation_id.to_owned(),
            revision: revision.record.clone(),
        };
        if intent_path.exists() || !publish_protected_json(&intent_path, &expected)? {
            let retained: WriteBackIntent = read_protected_json(&intent_path)?;
            if retained != expected {
                return Err(CodeChangeError::OperationIdentityConflict);
            }
        }
        Ok(WriteBackOperation {
            operation_id: operation_id.to_owned(),
            revision_digest: revision.record.revision_digest,
        })
    }

    /// Reopens one exact durable write-back intent.
    ///
    /// # Errors
    ///
    /// Rejects missing, corrupt, or identity-mismatched operation state.
    pub fn open_write_back(
        &self,
        operation_id: &str,
    ) -> Result<WriteBackOperation, CodeChangeError> {
        validate_identifier(operation_id)?;
        let intent: WriteBackIntent =
            read_protected_json(&self.operations_root.join(operation_id).join("intent.json"))?;
        if intent.schema != WRITE_BACK_INTENT_SCHEMA || intent.operation_id != operation_id {
            return Err(CodeChangeError::OperationIdentityConflict);
        }
        self.validate_revision(&intent.revision)?;
        Ok(WriteBackOperation {
            operation_id: operation_id.to_owned(),
            revision_digest: intent.revision.revision_digest,
        })
    }

    /// Lists all durable write-back intents in stable operation order.
    ///
    /// # Errors
    ///
    /// Fails closed for unexpected or corrupt operation storage.
    pub fn list_write_backs(&self) -> Result<Vec<WriteBackOperation>, CodeChangeError> {
        let mut operations = Vec::new();
        for entry in fs::read_dir(&self.operations_root)
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?
        {
            let entry = entry.map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
            let file_type = entry
                .file_type()
                .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| CodeChangeError::JournalCorrupt)?;
            if name == "operations.lock" && file_type.is_file() {
                continue;
            }
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(CodeChangeError::JournalCorrupt);
            }
            operations.push(self.open_write_back(&name)?);
        }
        operations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        Ok(operations)
    }

    /// Reads a terminal result without performing or retrying an effect.
    ///
    /// # Errors
    ///
    /// Rejects a missing intent or corrupt terminal result.
    pub fn write_back_status(
        &self,
        operation: &WriteBackOperation,
    ) -> Result<Option<WriteBackStatus>, CodeChangeError> {
        let intent: WriteBackIntent = read_protected_json(
            &self
                .operations_root
                .join(&operation.operation_id)
                .join("intent.json"),
        )?;
        if intent.schema != WRITE_BACK_INTENT_SCHEMA
            || intent.operation_id != operation.operation_id
            || intent.revision.revision_digest != operation.revision_digest
        {
            return Err(CodeChangeError::OperationIdentityConflict);
        }
        let result_path = self
            .operations_root
            .join(&operation.operation_id)
            .join("result.json");
        if !result_path.exists() {
            return Ok(None);
        }
        let result: WriteBackResult = read_protected_json(&result_path)?;
        validate_result(&result, operation, &intent.revision)?;
        Ok(Some(result.status))
    }

    /// Reconciles an already-durable operation with observed target bytes.
    ///
    /// A baseline match permits the exact candidate effect. A candidate match
    /// proves an earlier uncertain attempt succeeded. Every third value is
    /// captured and returned as a conflict without being overwritten. Results
    /// containing `Unknown` are not terminal and may be reconciled again.
    ///
    /// # Errors
    ///
    /// Rejects missing, mismatched, or corrupt operation state. Filesystem
    /// uncertainty after intent persistence is returned as a blocking status,
    /// not as successful completion.
    pub fn reconcile_write_back(
        &self,
        operation: &WriteBackOperation,
    ) -> Result<WriteBackStatus, CodeChangeError> {
        validate_identifier(&operation.operation_id)?;
        let _guard = self.lock_operations()?;
        let operation_root = self.operations_root.join(&operation.operation_id);
        let intent: WriteBackIntent = read_protected_json(&operation_root.join("intent.json"))?;
        if intent.schema != WRITE_BACK_INTENT_SCHEMA
            || intent.operation_id != operation.operation_id
            || intent.revision.revision_digest != operation.revision_digest
        {
            return Err(CodeChangeError::OperationIdentityConflict);
        }
        self.validate_revision(&intent.revision)?;
        let result_path = operation_root.join("result.json");
        if result_path.exists() {
            let result: WriteBackResult = read_protected_json(&result_path)?;
            validate_result(&result, operation, &intent.revision)?;
            return Ok(result.status);
        }

        let mut targets = Vec::with_capacity(intent.revision.targets.len());
        for (index, target) in intent.revision.targets.iter().enumerate() {
            targets.push(WriteBackTargetStatus {
                path: target.path.clone(),
                outcome: self.reconcile_target(&operation.operation_id, index, target),
            });
        }
        let disposition = if targets
            .iter()
            .any(|target| matches!(target.outcome, WriteBackTargetOutcome::Unknown { .. }))
        {
            WriteBackDisposition::ReconciliationRequired
        } else if targets
            .iter()
            .any(|target| matches!(target.outcome, WriteBackTargetOutcome::Conflict { .. }))
        {
            WriteBackDisposition::BlockedConflict
        } else {
            WriteBackDisposition::Applied
        };
        let status = WriteBackStatus {
            operation_id: operation.operation_id.clone(),
            revision_digest: operation.revision_digest,
            disposition,
            targets,
        };
        if disposition != WriteBackDisposition::ReconciliationRequired {
            let result = WriteBackResult {
                schema: WRITE_BACK_RESULT_SCHEMA.to_owned(),
                status: status.clone(),
            };
            if !publish_protected_json(&result_path, &result)? {
                let retained: WriteBackResult = read_protected_json(&result_path)?;
                validate_result(&retained, operation, &intent.revision)?;
                return Ok(retained.status);
            }
        }
        Ok(status)
    }

    fn reconcile_target(
        &self,
        operation_id: &str,
        index: usize,
        target: &CandidateTargetRevision,
    ) -> WriteBackTargetOutcome {
        let Ok(current) = self.artifacts.capture_if_present(&target.path) else {
            return WriteBackTargetOutcome::Unknown {
                reason: WriteBackUncertainty::TargetCouldNotBeObserved,
            };
        };
        if current == target.candidate {
            return WriteBackTargetOutcome::Applied { observed: current };
        }
        if current != target.baseline {
            return WriteBackTargetOutcome::Conflict { current };
        }

        let attempted = match &target.candidate {
            Some(candidate) => self.publish_candidate(operation_id, index, target, candidate),
            None => self.remove_candidate_target(target),
        };
        if let Err(reason) = attempted {
            return self.observe_after_uncertain_effect(target, reason);
        }
        self.observe_after_uncertain_effect(
            target,
            WriteBackUncertainty::PublicationOutcomeUnobserved,
        )
    }

    fn publish_candidate(
        &self,
        operation_id: &str,
        index: usize,
        target: &CandidateTargetRevision,
        candidate: &ArtifactRef,
    ) -> Result<(), WriteBackUncertainty> {
        let parent = self
            .artifacts
            .resolve_authorized_parent(&target.path)
            .map_err(|_| WriteBackUncertainty::TargetCouldNotBeObserved)?;
        let target_path = parent.join(
            target
                .path
                .to_path_buf()
                .file_name()
                .ok_or(WriteBackUncertainty::TargetCouldNotBeObserved)?,
        );
        let stage_key =
            ContentDigest::of(format!("{operation_id}\0{index}\0{}", target.path).as_bytes());
        let staged = parent.join(format!(".worldstream-{}.staged", stage_key.hex()));
        if staged == target_path {
            return Err(WriteBackUncertainty::StagedFileUnavailable);
        }
        let bytes = self
            .artifacts
            .read(candidate)
            .map_err(|_| WriteBackUncertainty::StagedFileUnavailable)?;
        match fs::symlink_metadata(&staged) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let staged_bytes = read_stable_regular_file(&staged)
                    .map_err(|_| WriteBackUncertainty::StagedFileUnavailable)?;
                if staged_bytes != bytes {
                    return Err(WriteBackUncertainty::StagedFileUnavailable);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut file = create_staged_file(&staged)
                    .map_err(|_| WriteBackUncertainty::StagedFileUnavailable)?;
                file.write_all(&bytes)
                    .and_then(|()| file.sync_all())
                    .map_err(|_| WriteBackUncertainty::StagedFileUnavailable)?;
                drop(file);
            }
            Ok(_) | Err(_) => return Err(WriteBackUncertainty::StagedFileUnavailable),
        }
        if let Some(current_path) = self
            .artifacts
            .resolve_authorized_file_if_present(&target.path)
            .map_err(|_| WriteBackUncertainty::TargetCouldNotBeObserved)?
        {
            let permissions = fs::metadata(current_path)
                .map_err(|_| WriteBackUncertainty::TargetCouldNotBeObserved)?
                .permissions();
            fs::set_permissions(&staged, permissions)
                .map_err(|_| WriteBackUncertainty::StagedFileUnavailable)?;
        }

        let rechecked = self
            .artifacts
            .capture_if_present(&target.path)
            .map_err(|_| WriteBackUncertainty::TargetCouldNotBeObserved)?;
        if rechecked != target.baseline {
            return Ok(());
        }
        match target.baseline {
            Some(_) => atomicwrites::replace_atomic(&staged, &target_path)
                .map_err(|_| WriteBackUncertainty::PublicationOutcomeUnobserved),
            None => fs::hard_link(&staged, &target_path)
                .map_err(|_| WriteBackUncertainty::PublicationOutcomeUnobserved),
        }?;
        let _cleanup = fs::remove_file(staged);
        sync_directory(&parent).map_err(|_| WriteBackUncertainty::PublicationOutcomeUnobserved)
    }

    fn remove_candidate_target(
        &self,
        target: &CandidateTargetRevision,
    ) -> Result<(), WriteBackUncertainty> {
        let rechecked = self
            .artifacts
            .capture_if_present(&target.path)
            .map_err(|_| WriteBackUncertainty::TargetCouldNotBeObserved)?;
        if rechecked != target.baseline {
            return Ok(());
        }
        let Some(path) = self
            .artifacts
            .resolve_authorized_file_if_present(&target.path)
            .map_err(|_| WriteBackUncertainty::TargetCouldNotBeObserved)?
        else {
            return Ok(());
        };
        fs::remove_file(path).map_err(|_| WriteBackUncertainty::PublicationOutcomeUnobserved)
    }

    fn observe_after_uncertain_effect(
        &self,
        target: &CandidateTargetRevision,
        uncertainty: WriteBackUncertainty,
    ) -> WriteBackTargetOutcome {
        match self.artifacts.capture_if_present(&target.path) {
            Ok(current) if current == target.candidate => {
                WriteBackTargetOutcome::Applied { observed: current }
            }
            Ok(current) if current != target.baseline => {
                WriteBackTargetOutcome::Conflict { current }
            }
            Ok(_) | Err(_) => WriteBackTargetOutcome::Unknown {
                reason: uncertainty,
            },
        }
    }

    fn validate_revision(&self, revision: &CandidateRevisionRecord) -> Result<(), CodeChangeError> {
        validate_identifier(&revision.candidate_id)?;
        if revision.targets.is_empty() || revision.targets.len() > MAX_TARGETS {
            return Err(CodeChangeError::InvalidTargets);
        }
        let mut previous: Option<&ArtifactPath> = None;
        for target in &revision.targets {
            if previous.is_some_and(|path| path >= &target.path) {
                return Err(CodeChangeError::InvalidTargets);
            }
            previous = Some(&target.path);
            if let Some(artifact) = &target.baseline {
                self.artifacts.verify(artifact)?;
            }
            if let Some(artifact) = &target.candidate {
                self.artifacts.verify(artifact)?;
            }
        }
        if candidate_content_digest(&revision.targets) != revision.candidate_digest
            || candidate_revision_digest(
                &revision.candidate_id,
                &revision.targets,
                revision.candidate_digest,
            ) != revision.revision_digest
        {
            return Err(CodeChangeError::CandidateConflict);
        }
        Ok(())
    }

    fn persist_revision(&self, revision: &CandidateRevisionRecord) -> Result<(), CodeChangeError> {
        let root = prepare_data_directory(
            &self
                .candidates_root
                .join(&revision.candidate_id)
                .join("revisions"),
        )
        .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let path = root.join(format!("{}.json", revision.revision_digest.hex()));
        let expected = SealedCandidateRevision {
            schema: CANDIDATE_REVISION_SCHEMA.to_owned(),
            revision: revision.clone(),
        };
        if !publish_protected_json(&path, &expected)? {
            let retained: SealedCandidateRevision = read_protected_json(&path)?;
            if retained != expected {
                return Err(CodeChangeError::CandidateConflict);
            }
        }
        Ok(())
    }

    fn revision_path(&self, candidate_id: &str, revision_digest: ContentDigest) -> PathBuf {
        self.candidates_root
            .join(candidate_id)
            .join("revisions")
            .join(format!("{}.json", revision_digest.hex()))
    }

    fn lock_operations(&self) -> Result<OperationGuard<'_>, CodeChangeError> {
        let process = self
            .serial
            .lock()
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        let path = self.operations_root.join("operations.lock");
        let file = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                validate_owner_only_file(&path)
                    .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => create_owner_only_file(&path)
                .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?,
            Ok(_) | Err(_) => return Err(CodeChangeError::ProtectedStateUnavailable),
        };
        file.lock()
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        Ok(OperationGuard {
            _process: process,
            _file: file,
        })
    }
}

struct OperationGuard<'a> {
    _process: std::sync::MutexGuard<'a, ()>,
    _file: File,
}

/// Closed code-change failures. A returned error never means an uncertain
/// write succeeded; uncertainty after a durable intent is represented by
/// [`WriteBackDisposition::ReconciliationRequired`].
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CodeChangeError {
    #[error("candidate or operation identity is invalid")]
    InvalidIdentifier,
    #[error("candidate targets are empty, duplicated, or too numerous")]
    InvalidTargets,
    #[error("the path was not authorized when the candidate was prepared")]
    TargetNotAuthorized,
    #[error("protected code-change state is unavailable")]
    ProtectedStateUnavailable,
    #[error("an incomplete candidate requires operator inspection")]
    IncompleteCandidate,
    #[error("the candidate identity is bound to different state")]
    CandidateConflict,
    #[error("the editable candidate no longer matches the sealed revision")]
    CandidateChanged,
    #[error("the stable operation identity is bound to another revision")]
    OperationIdentityConflict,
    #[error("durable operation state is missing or corrupt")]
    JournalCorrupt,
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
}

fn normalized_targets(targets: &[ArtifactPath]) -> Result<Vec<ArtifactPath>, CodeChangeError> {
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(CodeChangeError::InvalidTargets);
    }
    let mut targets = targets.to_vec();
    targets.sort();
    if targets.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CodeChangeError::InvalidTargets);
    }
    Ok(targets)
}

fn validate_manifest(
    manifest: &CandidateManifest,
    candidate_id: &str,
    targets: &[ArtifactPath],
) -> Result<(), CodeChangeError> {
    if manifest.schema != CANDIDATE_SCHEMA || manifest.candidate_id != candidate_id {
        return Err(CodeChangeError::CandidateConflict);
    }
    let retained = manifest
        .targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<Vec<_>>();
    if retained != targets {
        return Err(CodeChangeError::CandidateConflict);
    }
    Ok(())
}

fn candidate_content_digest(targets: &[CandidateTargetRevision]) -> ContentDigest {
    let mut bytes = Vec::new();
    push_field(&mut bytes, b"worldstream/agent-swarm-candidate-content/v1");
    for target in targets {
        push_field(&mut bytes, target.path.as_str().as_bytes());
        push_optional_artifact(&mut bytes, target.candidate.as_ref());
    }
    ContentDigest::of(&bytes)
}

fn candidate_revision_digest(
    candidate_id: &str,
    targets: &[CandidateTargetRevision],
    candidate_digest: ContentDigest,
) -> ContentDigest {
    let mut bytes = Vec::new();
    push_field(&mut bytes, b"worldstream/agent-swarm-candidate-revision/v1");
    push_field(&mut bytes, candidate_id.as_bytes());
    push_field(&mut bytes, candidate_digest.as_bytes());
    for target in targets {
        push_field(&mut bytes, target.path.as_str().as_bytes());
        push_optional_artifact(&mut bytes, target.baseline.as_ref());
        push_optional_artifact(&mut bytes, target.candidate.as_ref());
    }
    ContentDigest::of(&bytes)
}

fn push_optional_artifact(bytes: &mut Vec<u8>, artifact: Option<&ArtifactRef>) {
    match artifact {
        Some(artifact) => {
            bytes.push(1);
            push_field(bytes, artifact.digest().as_bytes());
            bytes.extend_from_slice(&artifact.byte_length().to_be_bytes());
        }
        None => bytes.push(0),
    }
}

fn push_field(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    bytes.extend_from_slice(field);
}

fn write_candidate_file(
    candidate_root: &Path,
    target: &ArtifactPath,
    bytes: &[u8],
) -> Result<(), CodeChangeError> {
    let path = candidate_root.join(target.to_path_buf());
    let parent = path.parent().ok_or(CodeChangeError::InvalidTargets)?;
    prepare_data_directory(parent).map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
    let mut file =
        create_owner_only_file(&path).map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| CodeChangeError::ProtectedStateUnavailable)
}

#[cfg(unix)]
fn copy_executable_permission(source: Option<&Path>, target: &Path) -> Result<(), CodeChangeError> {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(source) = source else {
        return Ok(());
    };
    let source_mode = fs::metadata(source)
        .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?
        .permissions()
        .mode();
    let mode = 0o600 | (source_mode & 0o100);
    fs::set_permissions(target, fs::Permissions::from_mode(mode))
        .map_err(|_| CodeChangeError::ProtectedStateUnavailable)
}

#[cfg(not(unix))]
const fn copy_executable_permission(
    _source: Option<&Path>,
    _target: &Path,
) -> Result<(), CodeChangeError> {
    Ok(())
}

fn create_staged_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn publish_protected_json<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<bool, CodeChangeError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| CodeChangeError::JournalCorrupt)?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > MAX_JOURNAL_BYTES) {
        return Err(CodeChangeError::JournalCorrupt);
    }
    let parent = path.parent().ok_or(CodeChangeError::JournalCorrupt)?;
    let sequence = JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staged = parent.join(format!(".{}.{}.staged", std::process::id(), sequence));
    let mut file =
        create_owner_only_file(&staged).map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
    let publication = (|| {
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
        drop(file);
        match fs::hard_link(&staged, path) {
            Ok(()) => {
                fs::remove_file(&staged).map_err(|_| CodeChangeError::ProtectedStateUnavailable)?;
                make_read_only(path)?;
                sync_directory(parent)?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(_) => Err(CodeChangeError::ProtectedStateUnavailable),
        }
    })();
    let _cleanup = fs::remove_file(staged);
    publication
}

pub(crate) fn read_protected_json<T: DeserializeOwned>(path: &Path) -> Result<T, CodeChangeError> {
    validate_owner_only_file(path).map_err(|_| CodeChangeError::JournalCorrupt)?;
    let metadata = fs::metadata(path).map_err(|_| CodeChangeError::JournalCorrupt)?;
    if metadata.len() > MAX_JOURNAL_BYTES {
        return Err(CodeChangeError::JournalCorrupt);
    }
    let bytes = fs::read(path).map_err(|_| CodeChangeError::JournalCorrupt)?;
    serde_json::from_slice(&bytes).map_err(|_| CodeChangeError::JournalCorrupt)
}

fn validate_result(
    result: &WriteBackResult,
    operation: &WriteBackOperation,
    revision: &CandidateRevisionRecord,
) -> Result<(), CodeChangeError> {
    if result.schema != WRITE_BACK_RESULT_SCHEMA
        || result.status.operation_id != operation.operation_id
        || result.status.revision_digest != operation.revision_digest
        || result.status.targets.len() != revision.targets.len()
        || result
            .status
            .targets
            .iter()
            .zip(&revision.targets)
            .any(|(status, target)| status.path != target.path)
        || result.status.disposition == WriteBackDisposition::ReconciliationRequired
        || (result.status.disposition == WriteBackDisposition::Applied
            && !result
                .status
                .targets
                .iter()
                .all(|target| target.outcome.is_applied()))
    {
        return Err(CodeChangeError::JournalCorrupt);
    }
    Ok(())
}

pub(crate) fn validate_identifier(value: &str) -> Result<(), CodeChangeError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(CodeChangeError::InvalidIdentifier);
    }
    Ok(())
}

fn make_read_only(path: &Path) -> Result<(), CodeChangeError> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| CodeChangeError::ProtectedStateUnavailable)?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|_| CodeChangeError::ProtectedStateUnavailable)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), CodeChangeError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| CodeChangeError::ProtectedStateUnavailable)
}

#[cfg(not(unix))]
const fn sync_directory(_path: &Path) -> Result<(), CodeChangeError> {
    Ok(())
}
