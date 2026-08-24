//! Durable, assignment-scoped idempotency for helper-side effects.
//!
//! This module owns only safe operation intent and bounded daemon results. It deliberately does
//! not own sealed authority, Room or Membership topology, Activation input, invocation context,
//! or model-private state.

use std::{
    fmt, fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_owner_only_file,
};

const SCHEMA_V1: &str = "worldstream/studio-assignment-mcp-operation@1";
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_RECORD_BYTES: u64 = 768 * 1024;
const MAX_RECORDS: usize = 256;

/// A pathless, browser-safe operation-ledger failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentMcpOperationErrorV1 {
    /// An identifier, request, response, or persisted record is invalid.
    InvalidData,
    /// Credential-shaped material was rejected before persistence.
    CredentialData,
    /// Activation-private or topology material was rejected before persistence.
    PrivateData,
    /// The requested exact operation does not exist.
    NotFound,
    /// The same immutable identity was used with different intent or result.
    Conflict,
    /// Owner-only storage or durable publication is unavailable.
    Unavailable,
    /// The bounded ledger has reached its supported operation count.
    CapacityExceeded,
}

/// Exact assignment-scoped operation identity.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpOperationIdentityV1 {
    assignment_id: String,
    operation_id: String,
}

impl AssignmentMcpOperationIdentityV1 {
    /// Constructs a bounded exact identity.
    ///
    /// # Errors
    ///
    /// Returns [`AssignmentMcpOperationErrorV1::InvalidData`] for unsafe identifiers.
    pub fn new(
        assignment_id: impl Into<String>,
        operation_id: impl Into<String>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let identity = Self {
            assignment_id: assignment_id.into(),
            operation_id: operation_id.into(),
        };
        validate_identifier(&identity.assignment_id)?;
        validate_identifier(&identity.operation_id)?;
        Ok(identity)
    }

    /// Returns the stable assignment identifier.
    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    /// Returns the stable operation identifier.
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
}

impl fmt::Debug for AssignmentMcpOperationIdentityV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssignmentMcpOperationIdentityV1")
            .field("assignment_id", &self.assignment_id)
            .field("operation_id", &self.operation_id)
            .finish()
    }
}

/// Immutable kind-specific identity and witness fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum AssignmentMcpOperationKindV1 {
    /// One listed Action submission.
    Action {
        /// Stable Action identity.
        action_id: String,
    },
    /// One exact Activation completion under an acquired lease.
    ActivationCompletion {
        /// Stable Activation identity.
        activation_id: String,
        /// Exact acquired claim identity.
        claim_id: String,
        /// Exact acquired lease generation.
        lease_generation: u64,
        /// Cursor acknowledged when the Activation was acquired.
        acquisition_cursor: u64,
        /// Stable completion identity.
        completion_id: String,
    },
}

/// Durable local progress through a possibly ambiguous remote effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentMcpOperationPhaseV1 {
    /// Immutable intent is durable; the remote response may still be unknown.
    Prepared,
    /// A full bounded remote response is durable; local completion may be retried.
    RemoteAccepted,
    /// The exact retained response has been locally completed.
    Complete,
}

/// Whether the retained bounded daemon response accepted or rejected the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentMcpRemoteOutcomeV1 {
    /// The daemon accepted the exact remote request.
    Accepted,
    /// The daemon returned a structured safe rejection or refresh result.
    Rejected,
}

/// Exact immutable helper operation intent.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpOperationIntentV1 {
    identity: AssignmentMcpOperationIdentityV1,
    kind: AssignmentMcpOperationKindV1,
    remote_request_id: String,
    canonical_request: String,
    request_hash: String,
}

impl AssignmentMcpOperationIntentV1 {
    /// Constructs an exact Action operation intent.
    ///
    /// The canonical JSON request must bind the assignment, operation, remote request, and Action
    /// identifiers. Room, Membership, principal, authority, or credential material is forbidden.
    ///
    /// # Errors
    ///
    /// Returns a closed safe error for malformed, incoherent, private, credential-shaped, or
    /// oversized input.
    pub fn new_action(
        identity: AssignmentMcpOperationIdentityV1,
        action_id: impl Into<String>,
        remote_request_id: impl Into<String>,
        canonical_request: Vec<u8>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let action_id = action_id.into();
        let remote_request_id = remote_request_id.into();
        validate_identifier(&action_id)?;
        let value = validate_safe_json(&canonical_request, SafeJsonKindV1::ActionRequest)?;
        validate_common_request_bindings(&value, &identity, &remote_request_id)?;
        require_string(&value, "action_id", &action_id)?;
        Self::new(
            identity,
            AssignmentMcpOperationKindV1::Action { action_id },
            remote_request_id,
            canonical_request,
        )
    }

    /// Constructs an exact Activation-completion operation intent.
    ///
    /// # Errors
    ///
    /// Returns a closed safe error for malformed, incoherent, private, credential-shaped, or
    /// oversized input.
    #[allow(clippy::too_many_arguments)]
    pub fn new_activation_completion(
        identity: AssignmentMcpOperationIdentityV1,
        activation_id: impl Into<String>,
        claim_id: impl Into<String>,
        lease_generation: u64,
        acquisition_cursor: u64,
        completion_id: impl Into<String>,
        remote_request_id: impl Into<String>,
        canonical_request: Vec<u8>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let activation_id = activation_id.into();
        let claim_id = claim_id.into();
        let completion_id = completion_id.into();
        let remote_request_id = remote_request_id.into();
        validate_identifier(&activation_id)?;
        validate_identifier(&claim_id)?;
        validate_identifier(&completion_id)?;
        let value = validate_safe_json(
            &canonical_request,
            SafeJsonKindV1::ActivationCompletionRequest,
        )?;
        validate_common_request_bindings(&value, &identity, &remote_request_id)?;
        require_string(&value, "activation_id", &activation_id)?;
        require_string(&value, "claim_id", &claim_id)?;
        require_u64(&value, "lease_generation", lease_generation)?;
        require_u64(&value, "cursor", acquisition_cursor)?;
        require_string(&value, "completion_id", &completion_id)?;
        Self::new(
            identity,
            AssignmentMcpOperationKindV1::ActivationCompletion {
                activation_id,
                claim_id,
                lease_generation,
                acquisition_cursor,
                completion_id,
            },
            remote_request_id,
            canonical_request,
        )
    }

    fn new(
        identity: AssignmentMcpOperationIdentityV1,
        kind: AssignmentMcpOperationKindV1,
        remote_request_id: String,
        canonical_request: Vec<u8>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        validate_identifier(&remote_request_id)?;
        let canonical_request = String::from_utf8(canonical_request)
            .map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)?;
        let request_hash = digest(canonical_request.as_bytes());
        Ok(Self {
            identity,
            kind,
            remote_request_id,
            canonical_request,
            request_hash,
        })
    }

    /// Returns the exact assignment-scoped operation identity.
    #[must_use]
    pub fn identity(&self) -> &AssignmentMcpOperationIdentityV1 {
        &self.identity
    }

    /// Returns the exact operation kind and kind-specific identity.
    #[must_use]
    pub fn kind(&self) -> &AssignmentMcpOperationKindV1 {
        &self.kind
    }

    /// Returns the immutable remote request identifier.
    #[must_use]
    pub fn remote_request_id(&self) -> &str {
        &self.remote_request_id
    }

    /// Returns the exact safe canonical request bytes retained for retry.
    #[must_use]
    pub fn canonical_request(&self) -> &[u8] {
        self.canonical_request.as_bytes()
    }

    /// Returns the domain-separated digest of the exact canonical request.
    #[must_use]
    pub fn request_hash(&self) -> &str {
        &self.request_hash
    }
}

impl fmt::Debug for AssignmentMcpOperationIntentV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssignmentMcpOperationIntentV1")
            .field("identity", &self.identity)
            .field("kind", &self.kind)
            .field("remote_request_id", &self.remote_request_id)
            .field("request_hash", &self.request_hash)
            .field("canonical_request", &"<redacted>")
            .finish()
    }
}

/// Exact kind identity returned with a bounded daemon response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum AssignmentMcpRemoteKindV1 {
    Action {
        action_id: String,
    },
    ActivationCompletion {
        activation_id: String,
        completion_id: String,
    },
}

/// Full bounded safe daemon response retained across a local crash window.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpRemoteAcceptanceV1 {
    remote_request_id: String,
    kind: AssignmentMcpRemoteKindV1,
    outcome: AssignmentMcpRemoteOutcomeV1,
    canonical_response: String,
    response_hash: String,
}

impl AssignmentMcpRemoteAcceptanceV1 {
    /// Constructs a safe retained Action response.
    ///
    /// # Errors
    ///
    /// Returns a closed safe error for malformed, private, credential-shaped, or oversized input.
    pub fn new_action(
        remote_request_id: impl Into<String>,
        action_id: impl Into<String>,
        outcome: AssignmentMcpRemoteOutcomeV1,
        canonical_response: Vec<u8>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let remote_request_id = remote_request_id.into();
        let action_id = action_id.into();
        validate_identifier(&remote_request_id)?;
        validate_identifier(&action_id)?;
        let value = validate_safe_json(&canonical_response, SafeJsonKindV1::ActionResponse)?;
        require_string(&value, "request_id", &remote_request_id)?;
        require_string(&value, "action_id", &action_id)?;
        Self::new(
            remote_request_id,
            AssignmentMcpRemoteKindV1::Action { action_id },
            outcome,
            canonical_response,
        )
    }

    /// Constructs a safe retained Activation-completion response.
    ///
    /// # Errors
    ///
    /// Returns a closed safe error for malformed, private, credential-shaped, or oversized input.
    pub fn new_activation_completion(
        remote_request_id: impl Into<String>,
        activation_id: impl Into<String>,
        completion_id: impl Into<String>,
        outcome: AssignmentMcpRemoteOutcomeV1,
        canonical_response: Vec<u8>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let remote_request_id = remote_request_id.into();
        let activation_id = activation_id.into();
        let completion_id = completion_id.into();
        validate_identifier(&remote_request_id)?;
        validate_identifier(&activation_id)?;
        validate_identifier(&completion_id)?;
        let value = validate_safe_json(
            &canonical_response,
            SafeJsonKindV1::ActivationCompletionResponse,
        )?;
        require_string(&value, "request_id", &remote_request_id)?;
        require_string(&value, "activation_id", &activation_id)?;
        require_string(&value, "completion_id", &completion_id)?;
        Self::new(
            remote_request_id,
            AssignmentMcpRemoteKindV1::ActivationCompletion {
                activation_id,
                completion_id,
            },
            outcome,
            canonical_response,
        )
    }

    fn new(
        remote_request_id: String,
        kind: AssignmentMcpRemoteKindV1,
        outcome: AssignmentMcpRemoteOutcomeV1,
        canonical_response: Vec<u8>,
    ) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let canonical_response = String::from_utf8(canonical_response)
            .map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)?;
        let response_hash = digest(canonical_response.as_bytes());
        Ok(Self {
            remote_request_id,
            kind,
            outcome,
            canonical_response,
            response_hash,
        })
    }

    /// Returns the immutable remote request identifier.
    #[must_use]
    pub fn remote_request_id(&self) -> &str {
        &self.remote_request_id
    }

    /// Returns whether the bounded daemon reply accepted or rejected the request.
    #[must_use]
    pub fn outcome(&self) -> AssignmentMcpRemoteOutcomeV1 {
        self.outcome
    }

    /// Returns the exact retained safe daemon-response bytes.
    #[must_use]
    pub fn canonical_response(&self) -> &[u8] {
        self.canonical_response.as_bytes()
    }

    /// Returns the domain-separated digest of the exact retained response.
    #[must_use]
    pub fn response_hash(&self) -> &str {
        &self.response_hash
    }
}

impl fmt::Debug for AssignmentMcpRemoteAcceptanceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssignmentMcpRemoteAcceptanceV1")
            .field("remote_request_id", &self.remote_request_id)
            .field("kind", &self.kind)
            .field("outcome", &self.outcome)
            .field("response_hash", &self.response_hash)
            .field("canonical_response", &"<redacted>")
            .finish()
    }
}

/// One durable immutable operation and its monotonic local phase.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentMcpOperationRecordV1 {
    schema: String,
    intent: AssignmentMcpOperationIntentV1,
    phase: AssignmentMcpOperationPhaseV1,
    remote_acceptance: Option<AssignmentMcpRemoteAcceptanceV1>,
    integrity_hash: String,
}

impl AssignmentMcpOperationRecordV1 {
    /// Returns the exact immutable intent.
    #[must_use]
    pub fn intent(&self) -> &AssignmentMcpOperationIntentV1 {
        &self.intent
    }

    /// Returns the assignment-scoped operation identity.
    #[must_use]
    pub fn identity(&self) -> &AssignmentMcpOperationIdentityV1 {
        self.intent.identity()
    }

    /// Returns the operation kind.
    #[must_use]
    pub fn kind(&self) -> &AssignmentMcpOperationKindV1 {
        self.intent.kind()
    }

    /// Returns the immutable remote request identifier.
    #[must_use]
    pub fn remote_request_id(&self) -> &str {
        self.intent.remote_request_id()
    }

    /// Returns the exact retained canonical request bytes.
    #[must_use]
    pub fn canonical_request(&self) -> &[u8] {
        self.intent.canonical_request()
    }

    /// Returns the canonical request digest.
    #[must_use]
    pub fn request_hash(&self) -> &str {
        self.intent.request_hash()
    }

    /// Returns the monotonic durable phase.
    #[must_use]
    pub fn phase(&self) -> AssignmentMcpOperationPhaseV1 {
        self.phase
    }

    /// Returns the full retained safe daemon response after remote acceptance.
    #[must_use]
    pub fn remote_acceptance(&self) -> Option<&AssignmentMcpRemoteAcceptanceV1> {
        self.remote_acceptance.as_ref()
    }
}

impl fmt::Debug for AssignmentMcpOperationRecordV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssignmentMcpOperationRecordV1")
            .field("schema", &self.schema)
            .field("intent", &self.intent)
            .field("phase", &self.phase)
            .field("remote_acceptance", &self.remote_acceptance)
            .field("integrity_hash", &self.integrity_hash)
            .finish()
    }
}

/// Narrow durable operation seam shared by Action and Activation-completion helpers.
pub trait AssignmentMcpOperationLedgerV1: Send + Sync {
    /// Durably reserves an immutable exact operation or returns its identical prior record.
    ///
    /// # Errors
    ///
    /// Returns conflict for altered reuse and a closed safe persistence/validation error otherwise.
    fn reserve(
        &self,
        intent: &AssignmentMcpOperationIntentV1,
    ) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1>;

    /// Loads one exact assignment-scoped operation without fallback.
    ///
    /// # Errors
    ///
    /// Returns a closed safe persistence or corruption error.
    fn load(
        &self,
        identity: &AssignmentMcpOperationIdentityV1,
    ) -> Result<Option<AssignmentMcpOperationRecordV1>, AssignmentMcpOperationErrorV1>;

    /// Durably records the exact full bounded remote response.
    ///
    /// # Errors
    ///
    /// Returns not-found, conflict for altered reuse, or a closed safe persistence error.
    fn record_remote(
        &self,
        identity: &AssignmentMcpOperationIdentityV1,
        acceptance: &AssignmentMcpRemoteAcceptanceV1,
    ) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1>;

    /// Marks local completion using exactly the retained bounded remote response.
    ///
    /// # Errors
    ///
    /// Returns not-found, conflict for altered reuse, or a closed safe persistence error.
    fn complete(
        &self,
        identity: &AssignmentMcpOperationIdentityV1,
        acceptance: &AssignmentMcpRemoteAcceptanceV1,
    ) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1>;
}

/// Owner-only bounded file implementation of the shared operation ledger.
#[derive(Clone)]
pub struct FileAssignmentMcpOperationLedgerV1 {
    root: Arc<PathBuf>,
    mutation: Arc<Mutex<()>>,
    process_lock: Arc<fs::File>,
}

struct ProcessFileLockV1<'a>(&'a fs::File);

impl Drop for ProcessFileLockV1<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl FileAssignmentMcpOperationLedgerV1 {
    /// Opens or creates an owner-only ledger and validates every retained record fail-closed.
    ///
    /// # Errors
    ///
    /// Returns unavailable for unsafe storage and invalid-data for corrupt retained state.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, AssignmentMcpOperationErrorV1> {
        let root = prepare_data_directory(root.as_ref())
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
        let lock_path = root.join(".ledger.lock");
        let process_lock = match create_owner_only_file(&lock_path) {
            Ok(file) => file,
            Err(_) if lock_path.exists() => fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?,
            Err(_) => return Err(AssignmentMcpOperationErrorV1::Unavailable),
        };
        validate_owner_only_file(&lock_path)
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
        let ledger = Self {
            root: Arc::new(root),
            mutation: Arc::new(Mutex::new(())),
            process_lock: Arc::new(process_lock),
        };
        let process_guard = ledger.lock_process()?;
        ledger.validate_existing()?;
        drop(process_guard);
        Ok(ledger)
    }

    fn lock_process(&self) -> Result<ProcessFileLockV1<'_>, AssignmentMcpOperationErrorV1> {
        self.process_lock
            .lock()
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
        Ok(ProcessFileLockV1(&self.process_lock))
    }

    fn path(&self, identity: &AssignmentMcpOperationIdentityV1) -> PathBuf {
        self.root.join(format!(
            "{}-{}.json",
            encode_component(identity.assignment_id()),
            encode_component(identity.operation_id())
        ))
    }

    fn validate_existing(&self) -> Result<(), AssignmentMcpOperationErrorV1> {
        let mut count = 0_usize;
        let entries = fs::read_dir(self.root.as_ref())
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
        for entry in entries {
            let entry = entry.map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
            let file_type = entry
                .file_type()
                .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".ledger.lock" && file_type.is_file() {
                validate_owner_only_file(&entry.path())
                    .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
                continue;
            }
            if name.starts_with('.') && name.ends_with(".tmp") {
                if file_type.is_file() {
                    validate_owner_only_file(&entry.path())
                        .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
                    fs::remove_file(entry.path())
                        .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
                    continue;
                }
                return Err(AssignmentMcpOperationErrorV1::InvalidData);
            }
            if !file_type.is_file() || !name.ends_with(".json") {
                return Err(AssignmentMcpOperationErrorV1::InvalidData);
            }
            count = count.saturating_add(1);
            if count > MAX_RECORDS {
                return Err(AssignmentMcpOperationErrorV1::CapacityExceeded);
            }
            let record = read_record(&entry.path())?;
            if self.path(record.identity()) != entry.path() {
                return Err(AssignmentMcpOperationErrorV1::InvalidData);
            }
        }
        sync_directory(self.root.as_ref()).map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)
    }

    fn persist(
        &self,
        record: &AssignmentMcpOperationRecordV1,
    ) -> Result<(), AssignmentMcpOperationErrorV1> {
        let bytes =
            serde_json::to_vec(record).map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
            return Err(AssignmentMcpOperationErrorV1::InvalidData);
        }
        let target = self.path(record.identity());
        let temporary = self.root.join(format!(
            ".{}-{}.tmp",
            encode_component(record.identity().operation_id()),
            temporary_suffix()?
        ));
        let mut file = create_owner_only_renameable_file(&temporary)
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
        let result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_file(&temporary, &target))
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable);
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn count_records(&self) -> Result<usize, AssignmentMcpOperationErrorV1> {
        fs::read_dir(self.root.as_ref())
            .map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?
            .try_fold(0_usize, |count, entry| {
                let entry = entry.map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
                Ok(count + usize::from(entry.path().extension().is_some_and(|ext| ext == "json")))
            })
    }
}

impl AssignmentMcpOperationLedgerV1 for FileAssignmentMcpOperationLedgerV1 {
    fn reserve(
        &self,
        intent: &AssignmentMcpOperationIntentV1,
    ) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1> {
        validate_intent(intent)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_lock = self.lock_process()?;
        if let Some(existing) = self.load(intent.identity())? {
            return if existing.intent == *intent {
                Ok(existing)
            } else {
                Err(AssignmentMcpOperationErrorV1::Conflict)
            };
        }
        if self.count_records()? >= MAX_RECORDS {
            return Err(AssignmentMcpOperationErrorV1::CapacityExceeded);
        }
        let mut record = AssignmentMcpOperationRecordV1 {
            schema: SCHEMA_V1.to_owned(),
            intent: intent.clone(),
            phase: AssignmentMcpOperationPhaseV1::Prepared,
            remote_acceptance: None,
            integrity_hash: String::new(),
        };
        record.integrity_hash = record_integrity_hash(&record)?;
        self.persist(&record)?;
        Ok(record)
    }

    fn load(
        &self,
        identity: &AssignmentMcpOperationIdentityV1,
    ) -> Result<Option<AssignmentMcpOperationRecordV1>, AssignmentMcpOperationErrorV1> {
        validate_identity(identity)?;
        let path = self.path(identity);
        if !path.exists() {
            return Ok(None);
        }
        let record = read_record(&path)?;
        if record.identity() != identity {
            return Err(AssignmentMcpOperationErrorV1::InvalidData);
        }
        Ok(Some(record))
    }

    fn record_remote(
        &self,
        identity: &AssignmentMcpOperationIdentityV1,
        acceptance: &AssignmentMcpRemoteAcceptanceV1,
    ) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_lock = self.lock_process()?;
        let mut record = self
            .load(identity)?
            .ok_or(AssignmentMcpOperationErrorV1::NotFound)?;
        validate_acceptance_for_intent(&record.intent, acceptance)?;
        if let Some(existing) = &record.remote_acceptance {
            return if existing == acceptance {
                Ok(record)
            } else {
                Err(AssignmentMcpOperationErrorV1::Conflict)
            };
        }
        if record.phase != AssignmentMcpOperationPhaseV1::Prepared {
            return Err(AssignmentMcpOperationErrorV1::InvalidData);
        }
        record.phase = AssignmentMcpOperationPhaseV1::RemoteAccepted;
        record.remote_acceptance = Some(acceptance.clone());
        record.integrity_hash = record_integrity_hash(&record)?;
        self.persist(&record)?;
        Ok(record)
    }

    fn complete(
        &self,
        identity: &AssignmentMcpOperationIdentityV1,
        acceptance: &AssignmentMcpRemoteAcceptanceV1,
    ) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_lock = self.lock_process()?;
        let mut record = self
            .load(identity)?
            .ok_or(AssignmentMcpOperationErrorV1::NotFound)?;
        validate_acceptance_for_intent(&record.intent, acceptance)?;
        if let Some(existing) = &record.remote_acceptance
            && existing != acceptance
        {
            return Err(AssignmentMcpOperationErrorV1::Conflict);
        }
        if record.phase == AssignmentMcpOperationPhaseV1::Complete {
            return Ok(record);
        }
        record.phase = AssignmentMcpOperationPhaseV1::Complete;
        record.remote_acceptance = Some(acceptance.clone());
        record.integrity_hash = record_integrity_hash(&record)?;
        self.persist(&record)?;
        Ok(record)
    }
}

fn validate_identity(
    identity: &AssignmentMcpOperationIdentityV1,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    validate_identifier(identity.assignment_id())?;
    validate_identifier(identity.operation_id())
}

fn validate_intent(
    intent: &AssignmentMcpOperationIntentV1,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    validate_identity(&intent.identity)?;
    validate_identifier(&intent.remote_request_id)?;
    let expected_hash = digest(intent.canonical_request.as_bytes());
    if intent.request_hash != expected_hash {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    let value = match &intent.kind {
        AssignmentMcpOperationKindV1::Action { action_id } => {
            validate_identifier(action_id)?;
            let value = validate_safe_json(
                intent.canonical_request.as_bytes(),
                SafeJsonKindV1::ActionRequest,
            )?;
            require_string(&value, "action_id", action_id)?;
            value
        }
        AssignmentMcpOperationKindV1::ActivationCompletion {
            activation_id,
            claim_id,
            lease_generation,
            acquisition_cursor,
            completion_id,
        } => {
            validate_identifier(activation_id)?;
            validate_identifier(claim_id)?;
            validate_identifier(completion_id)?;
            let value = validate_safe_json(
                intent.canonical_request.as_bytes(),
                SafeJsonKindV1::ActivationCompletionRequest,
            )?;
            require_string(&value, "activation_id", activation_id)?;
            require_string(&value, "claim_id", claim_id)?;
            require_u64(&value, "lease_generation", *lease_generation)?;
            require_u64(&value, "cursor", *acquisition_cursor)?;
            require_string(&value, "completion_id", completion_id)?;
            value
        }
    };
    validate_common_request_bindings(&value, &intent.identity, &intent.remote_request_id)
}

fn validate_acceptance_for_intent(
    intent: &AssignmentMcpOperationIntentV1,
    acceptance: &AssignmentMcpRemoteAcceptanceV1,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    if acceptance.remote_request_id != intent.remote_request_id {
        return Err(AssignmentMcpOperationErrorV1::Conflict);
    }
    let kinds_match = match (&intent.kind, &acceptance.kind) {
        (
            AssignmentMcpOperationKindV1::Action {
                action_id: expected,
            },
            AssignmentMcpRemoteKindV1::Action { action_id: actual },
        ) => expected == actual,
        (
            AssignmentMcpOperationKindV1::ActivationCompletion {
                activation_id: expected_activation,
                completion_id: expected_completion,
                ..
            },
            AssignmentMcpRemoteKindV1::ActivationCompletion {
                activation_id: actual_activation,
                completion_id: actual_completion,
            },
        ) => expected_activation == actual_activation && expected_completion == actual_completion,
        _ => false,
    };
    if !kinds_match {
        return Err(AssignmentMcpOperationErrorV1::Conflict);
    }
    validate_acceptance(acceptance)
}

fn validate_acceptance(
    acceptance: &AssignmentMcpRemoteAcceptanceV1,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    validate_identifier(&acceptance.remote_request_id)?;
    let kind = match &acceptance.kind {
        AssignmentMcpRemoteKindV1::Action { action_id } => {
            validate_identifier(action_id)?;
            SafeJsonKindV1::ActionResponse
        }
        AssignmentMcpRemoteKindV1::ActivationCompletion {
            activation_id,
            completion_id,
        } => {
            validate_identifier(activation_id)?;
            validate_identifier(completion_id)?;
            SafeJsonKindV1::ActivationCompletionResponse
        }
    };
    let value = validate_safe_json(acceptance.canonical_response.as_bytes(), kind)?;
    require_string(&value, "request_id", &acceptance.remote_request_id)?;
    match &acceptance.kind {
        AssignmentMcpRemoteKindV1::Action { action_id } => {
            require_string(&value, "action_id", action_id)?;
        }
        AssignmentMcpRemoteKindV1::ActivationCompletion {
            activation_id,
            completion_id,
        } => {
            require_string(&value, "activation_id", activation_id)?;
            require_string(&value, "completion_id", completion_id)?;
        }
    }
    if acceptance.response_hash != digest(acceptance.canonical_response.as_bytes()) {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    Ok(())
}

fn validate_record(
    record: &AssignmentMcpOperationRecordV1,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    if record.schema != SCHEMA_V1 {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    validate_intent(&record.intent)?;
    match (record.phase, &record.remote_acceptance) {
        (AssignmentMcpOperationPhaseV1::Prepared, None) => {}
        (
            AssignmentMcpOperationPhaseV1::RemoteAccepted | AssignmentMcpOperationPhaseV1::Complete,
            Some(acceptance),
        ) => validate_acceptance_for_intent(&record.intent, acceptance)?,
        _ => return Err(AssignmentMcpOperationErrorV1::InvalidData),
    }
    if record.integrity_hash != record_integrity_hash(record)? {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    Ok(())
}

#[derive(Serialize)]
struct RecordIntegrityWitnessV1<'a> {
    schema: &'a str,
    intent: &'a AssignmentMcpOperationIntentV1,
    phase: AssignmentMcpOperationPhaseV1,
    remote_acceptance: &'a Option<AssignmentMcpRemoteAcceptanceV1>,
}

fn record_integrity_hash(
    record: &AssignmentMcpOperationRecordV1,
) -> Result<String, AssignmentMcpOperationErrorV1> {
    let witness = RecordIntegrityWitnessV1 {
        schema: &record.schema,
        intent: &record.intent,
        phase: record.phase,
        remote_acceptance: &record.remote_acceptance,
    };
    let bytes =
        serde_json::to_vec(&witness).map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)?;
    Ok(digest(&bytes))
}

fn read_record(
    path: &Path,
) -> Result<AssignmentMcpOperationRecordV1, AssignmentMcpOperationErrorV1> {
    validate_owner_only_file(path).map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
    let metadata = fs::metadata(path).map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    let bytes = fs::read(path).map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
    let record: AssignmentMcpOperationRecordV1 =
        serde_json::from_slice(&bytes).map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)?;
    validate_record(&record)?;
    Ok(record)
}

#[derive(Clone, Copy)]
enum SafeJsonKindV1 {
    ActionRequest,
    ActionResponse,
    ActivationCompletionRequest,
    ActivationCompletionResponse,
}

fn validate_safe_json(
    bytes: &[u8],
    kind: SafeJsonKindV1,
) -> Result<Value, AssignmentMcpOperationErrorV1> {
    let maximum = match kind {
        SafeJsonKindV1::ActionRequest | SafeJsonKindV1::ActivationCompletionRequest => {
            MAX_REQUEST_BYTES
        }
        SafeJsonKindV1::ActionResponse | SafeJsonKindV1::ActivationCompletionResponse => {
            MAX_RESPONSE_BYTES
        }
    };
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)?;
    if !value.is_object() {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    inspect_safe_value(&value, kind)?;
    if serde_json::to_vec(&value).map_err(|_| AssignmentMcpOperationErrorV1::InvalidData)? != bytes
    {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    Ok(value)
}

fn inspect_safe_value(
    value: &Value,
    kind: SafeJsonKindV1,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let normalized = key.replace('-', "_").to_ascii_lowercase();
                if is_credential_key(&normalized) {
                    return Err(AssignmentMcpOperationErrorV1::CredentialData);
                }
                if is_topology_key(&normalized) {
                    return Err(AssignmentMcpOperationErrorV1::PrivateData);
                }
                if matches!(
                    kind,
                    SafeJsonKindV1::ActivationCompletionRequest
                        | SafeJsonKindV1::ActivationCompletionResponse
                ) && is_activation_private_key(&normalized)
                {
                    return Err(AssignmentMcpOperationErrorV1::PrivateData);
                }
                inspect_safe_value(value, kind)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                inspect_safe_value(value, kind)?;
            }
        }
        Value::String(value) if looks_like_credential_value(value) => {
            return Err(AssignmentMcpOperationErrorV1::CredentialData);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn is_credential_key(key: &str) -> bool {
    [
        "api_key",
        "apikey",
        "authorization",
        "bearer",
        "credential",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|candidate| key.contains(candidate))
}

fn is_topology_key(key: &str) -> bool {
    [
        "room_id",
        "member_id",
        "membership_id",
        "principal_id",
        "sealed_authority",
        "authority_reference",
    ]
    .contains(&key)
}

fn is_activation_private_key(key: &str) -> bool {
    matches!(
        key,
        "activation_input"
            | "activation_payload"
            | "artifact_references"
            | "context"
            | "delivery"
            | "frames"
            | "invocation"
            | "invocation_context"
            | "memory"
            | "private_memory"
            | "projection"
            | "projection_schema"
            | "runner_budget"
            | "runner_limits"
    ) || key.starts_with("private_")
}

fn looks_like_credential_value(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xoxb-")
        || lower.starts_with("xoxp-")
        || value.starts_with("AKIA")
        || (value.starts_with("eyJ") && value.matches('.').count() == 2)
}

fn validate_common_request_bindings(
    value: &Value,
    identity: &AssignmentMcpOperationIdentityV1,
    remote_request_id: &str,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    require_string(value, "assignment_id", identity.assignment_id())?;
    require_string(value, "operation_id", identity.operation_id())?;
    require_string(value, "request_id", remote_request_id)
}

fn require_string(
    value: &Value,
    key: &str,
    expected: &str,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    if value.get(key).and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(AssignmentMcpOperationErrorV1::InvalidData)
    }
}

fn require_u64(
    value: &Value,
    key: &str,
    expected: u64,
) -> Result<(), AssignmentMcpOperationErrorV1> {
    if value.get(key).and_then(Value::as_u64) == Some(expected) {
        Ok(())
    } else {
        Err(AssignmentMcpOperationErrorV1::InvalidData)
    }
}

fn validate_identifier(value: &str) -> Result<(), AssignmentMcpOperationErrorV1> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(AssignmentMcpOperationErrorV1::InvalidData);
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/studio-assignment-mcp-operation-field@1\0");
    hasher.update(bytes);
    format!("blake3:{}", hasher.finalize().to_hex())
}

fn encode_component(value: &str) -> String {
    encode_bytes(value.as_bytes())
}

fn encode_bytes(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len().saturating_mul(2));
    for &byte in value {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn temporary_suffix() -> Result<String, AssignmentMcpOperationErrorV1> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| AssignmentMcpOperationErrorV1::Unavailable)?;
    Ok(encode_bytes(&random))
}

#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path is not Unicode",
        )
    })?;
    let target = target.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path is not Unicode",
        )
    })?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}
