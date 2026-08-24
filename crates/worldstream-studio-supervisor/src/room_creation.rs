//! Durable exact-once Room creation orchestration for reviewed Studio drafts.

use std::{
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_protocol::{
    AccessMode, BearerWireV1, CreateMember, CreateRoomRequest, CreateRoomResponse,
    MAX_MESSAGE_BYTES,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::room_drafts::{RoomDraftReviewV1, RoomDraftStepV1, RoomDraftStoreV1, RoomDraftV1};
use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const OPERATION_SCHEMA_V1: &str = "worldstream/studio-room-creation-operation/v1";
const RESPONSE_BINDING_DOMAIN_V1: &str = "worldstream/studio-room-creation-response/v1";
const MAX_OPERATION_BYTES: usize = 256 * 1024;

/// Durable operation states visible to Studio.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomCreationStateV1 {
    Waiting,
    Retrying,
    Succeeded,
    NeedsAttention,
}

/// Closed actionable state for an operation requiring operator attention.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCreationAttentionV1 {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// Complete durable intent and receipt. The request never changes after first
/// publication, and one draft identifier can own only one such record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCreationOperationV1 {
    pub schema: String,
    pub draft_id: String,
    pub operation_id: String,
    pub idempotency_key: String,
    pub review_hash: String,
    pub intent_hash: String,
    pub review: RoomDraftReviewV1,
    pub request: CreateRoomRequest,
    pub state: RoomCreationStateV1,
    pub attempts: u32,
    pub room_id: Option<String>,
    pub response: Option<CreateRoomResponse>,
    pub response_hash: Option<String>,
    pub attention: Option<RoomCreationAttentionV1>,
}

/// Browser-safe durable status. Exact intent and daemon receipt remain inside
/// the owner-only Supervisor store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCreationStatusV1 {
    pub version: String,
    pub draft_id: String,
    pub operation_id: String,
    pub idempotency_key: String,
    pub review_hash: String,
    pub intent_hash: String,
    pub state: RoomCreationStateV1,
    pub attempts: u32,
    pub room_id: Option<String>,
    pub attention: Option<RoomCreationAttentionV1>,
}

impl From<RoomCreationOperationV1> for RoomCreationStatusV1 {
    fn from(operation: RoomCreationOperationV1) -> Self {
        Self {
            version: "studio_room_creation.v1".to_owned(),
            draft_id: operation.draft_id,
            operation_id: operation.operation_id,
            idempotency_key: operation.idempotency_key,
            review_hash: operation.review_hash,
            intent_hash: operation.intent_hash,
            state: operation.state,
            attempts: operation.attempts,
            room_id: operation.room_id,
            attention: operation.attention,
        }
    }
}

/// Attempt classification. Ambiguous results must retry the same intent;
/// operator-fix and rejected results must never mint a replacement operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomCreationAttemptErrorV1 {
    Ambiguous,
    OperatorFixRequired,
    Rejected,
}

/// Bounded daemon creation capability.
pub trait DaemonRoomCreatorV1: Send + Sync + 'static {
    /// Submits one already-persisted exact intent.
    ///
    /// # Errors
    ///
    /// Returns only ambiguous, operator-fix, or permanent rejection classes.
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1>;
}

/// Fixed-address HTTP creator using one exact retained Host authority.
#[derive(Clone, Debug)]
pub struct HttpDaemonRoomCreatorV1 {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
}

impl HttpDaemonRoomCreatorV1 {
    /// Configures the sole supported daemon Room-creation call.
    #[must_use]
    pub const fn new(
        address: SocketAddr,
        timeout: Duration,
        vault: FileSecretVaultV1,
        host_authority: Option<SecretReferenceV1>,
    ) -> Self {
        Self {
            address,
            timeout,
            vault,
            host_authority,
        }
    }
}

impl DaemonRoomCreatorV1 for HttpDaemonRoomCreatorV1 {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let body =
            serde_json::to_string(request).map_err(|_| RoomCreationAttemptErrorV1::Rejected)?;
        let http = Zeroizing::new(format!(
            "POST /v1/rooms HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.address,
            bearer.as_str(),
            body.len(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous)?;
        stream
            .write_all(http.as_bytes())
            .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut response)
            .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous)?;
        if response.len() > MAX_MESSAGE_BYTES {
            return Err(RoomCreationAttemptErrorV1::Ambiguous);
        }
        let (status, body) = parse_http_response(&response)?;
        match status {
            200 => serde_json::from_slice(body).map_err(|_| RoomCreationAttemptErrorV1::Ambiguous),
            401 | 403 => Err(RoomCreationAttemptErrorV1::OperatorFixRequired),
            400 | 404 | 409 | 422 => Err(RoomCreationAttemptErrorV1::Rejected),
            _ => Err(RoomCreationAttemptErrorV1::Ambiguous),
        }
    }
}

/// Closed local orchestration failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RoomCreationErrorV1 {
    #[error("reviewed Room draft is invalid")]
    InvalidDraft,
    #[error("Room creation operation was not found")]
    NotFound,
    #[error("Room creation operation storage is unavailable")]
    Unavailable,
}

/// Durable creation supervisor keyed permanently by draft identifier.
#[derive(Clone)]
pub struct RoomCreationSupervisorV1 {
    root: Arc<PathBuf>,
    drafts: RoomDraftStoreV1,
    creator: Arc<dyn DaemonRoomCreatorV1>,
    mutation: Arc<Mutex<()>>,
    #[cfg(test)]
    fail_before_receipt_once: Arc<AtomicBool>,
    #[cfg(test)]
    fail_after_intent_once: Arc<AtomicBool>,
}

impl RoomCreationSupervisorV1 {
    /// Opens the owner-only operation directory.
    ///
    /// # Errors
    ///
    /// Fails closed when protected persistence is unsafe or unavailable.
    pub fn open(
        root: &Path,
        drafts: RoomDraftStoreV1,
        creator: impl DaemonRoomCreatorV1,
    ) -> Result<Self, RoomCreationErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            drafts,
            creator: Arc::new(creator),
            mutation: Arc::new(Mutex::new(())),
            #[cfg(test)]
            fail_before_receipt_once: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fail_after_intent_once: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Creates and persists one exact operation before the first daemon call,
    /// or returns/reconciles the operation already bound to this draft.
    ///
    /// # Errors
    ///
    /// Returns a closed invalid-draft or persistence error without calling the
    /// daemon under a newly generated identity.
    pub fn start(&self, draft_id: &str) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
        let _guard = self.lock();
        match self.load_unlocked(draft_id) {
            Ok(existing) => {
                return if operation_is_terminal(&existing) {
                    Ok(existing)
                } else {
                    self.reconcile_unlocked(existing)
                };
            }
            Err(RoomCreationErrorV1::NotFound) => {}
            Err(error) => return Err(error),
        }
        let draft = self
            .drafts
            .load(draft_id)
            .map_err(|_| RoomCreationErrorV1::InvalidDraft)?;
        let mut operation = prepare_operation(&draft)?;
        self.persist_new(&operation)?;
        #[cfg(test)]
        if self.fail_after_intent_once.swap(false, Ordering::SeqCst) {
            return Err(RoomCreationErrorV1::Unavailable);
        }
        operation = self.reconcile_unlocked(operation)?;
        Ok(operation)
    }

    /// Reissues the identical persisted request for one nonterminal operation.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence unavailable; it never creates a new
    /// operation identity during reconciliation.
    pub fn reconcile(
        &self,
        draft_id: &str,
    ) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
        let _guard = self.lock();
        let operation = self.load_unlocked(draft_id)?;
        if operation_is_terminal(&operation) {
            Ok(operation)
        } else {
            self.reconcile_unlocked(operation)
        }
    }

    /// Loads durable visible state without issuing a daemon call.
    ///
    /// # Errors
    ///
    /// Returns exact not-found or storage unavailable.
    pub fn status(&self, draft_id: &str) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
        let _guard = self.lock();
        self.load_unlocked(draft_id)
    }

    #[cfg(test)]
    pub fn fail_before_receipt_once_for_test(&self) {
        self.fail_before_receipt_once.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn fail_after_intent_once_for_test(&self) {
        self.fail_after_intent_once.store(true, Ordering::SeqCst);
    }

    fn reconcile_unlocked(
        &self,
        mut operation: RoomCreationOperationV1,
    ) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
        operation.attempts = operation.attempts.saturating_add(1);
        operation.state = RoomCreationStateV1::Waiting;
        operation.attention = None;
        self.persist(&operation)?;
        match self.creator.create(&operation.request) {
            Ok(response) if response_is_bound(&operation.request, &response) => {
                #[cfg(test)]
                if self.fail_before_receipt_once.swap(false, Ordering::SeqCst) {
                    operation.state = RoomCreationStateV1::Retrying;
                    operation.attention = Some(ambiguous_attention());
                    self.persist(&operation)?;
                    return Ok(operation);
                }
                operation.state = RoomCreationStateV1::Succeeded;
                operation.room_id = Some(response.room_id.clone());
                operation.response_hash = Some(response_hash(&operation.intent_hash, &response)?);
                operation.response = Some(response);
                operation.attention = None;
                self.persist(&operation)?;
                Ok(operation)
            }
            Ok(_) | Err(RoomCreationAttemptErrorV1::Ambiguous) => {
                operation.state = RoomCreationStateV1::Retrying;
                operation.attention = Some(ambiguous_attention());
                self.persist(&operation)?;
                Ok(operation)
            }
            Err(RoomCreationAttemptErrorV1::OperatorFixRequired) => {
                operation.state = RoomCreationStateV1::NeedsAttention;
                operation.attention = Some(operator_fix_attention());
                self.persist(&operation)?;
                Ok(operation)
            }
            Err(RoomCreationAttemptErrorV1::Rejected) => {
                operation.state = RoomCreationStateV1::NeedsAttention;
                operation.attention = Some(rejected_attention());
                self.persist(&operation)?;
                Ok(operation)
            }
        }
    }

    fn load_unlocked(
        &self,
        draft_id: &str,
    ) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
        validate_draft_id(draft_id)?;
        let path = self.operation_path(draft_id);
        if !path.exists() {
            return Err(RoomCreationErrorV1::NotFound);
        }
        validate_owner_only_file(&path).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        if metadata.len() > u64::try_from(MAX_OPERATION_BYTES).unwrap_or(u64::MAX) {
            return Err(RoomCreationErrorV1::Unavailable);
        }
        let bytes = fs::read(path).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        let operation: RoomCreationOperationV1 =
            serde_json::from_slice(&bytes).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        validate_operation(&operation, draft_id)?;
        Ok(operation)
    }

    fn persist(&self, operation: &RoomCreationOperationV1) -> Result<(), RoomCreationErrorV1> {
        let (temporary, target) = self.write_temporary(operation)?;
        let published = fs::rename(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| RoomCreationErrorV1::Unavailable);
        if published.is_err() {
            let _ = fs::remove_file(temporary);
        }
        published
    }

    fn persist_new(&self, operation: &RoomCreationOperationV1) -> Result<(), RoomCreationErrorV1> {
        let (temporary, target) = self.write_temporary(operation)?;
        let published = fs::hard_link(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| RoomCreationErrorV1::Unavailable);
        let _ = fs::remove_file(temporary);
        published
    }

    fn write_temporary(
        &self,
        operation: &RoomCreationOperationV1,
    ) -> Result<(PathBuf, PathBuf), RoomCreationErrorV1> {
        validate_operation(operation, &operation.draft_id)?;
        let bytes =
            serde_json::to_vec_pretty(operation).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        if bytes.len() > MAX_OPERATION_BYTES {
            return Err(RoomCreationErrorV1::Unavailable);
        }
        let target = self.operation_path(&operation.draft_id);
        let temporary = self
            .root
            .join(format!(".{}.{}.tmp", operation.draft_id, random_suffix()?));
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        let written = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| RoomCreationErrorV1::Unavailable);
        drop(file);
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
            return written.map(|()| (temporary, target));
        }
        Ok((temporary, target))
    }

    fn operation_path(&self, draft_id: &str) -> PathBuf {
        self.root.join(format!("{draft_id}.json"))
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the bounded creation-operation API. It exposes no arbitrary request
/// body and derives intent only from the persisted reviewed draft.
pub fn room_creation_router(supervisor: RoomCreationSupervisorV1) -> Router {
    Router::new()
        .route("/api/v1/room-creations/{draft_id}", get(creation_status))
        .route(
            "/api/v1/room-creations/{draft_id}:start",
            post(start_creation),
        )
        .route(
            "/api/v1/room-creations/{draft_id}:retry",
            post(retry_creation),
        )
        .with_state(supervisor)
}

async fn creation_status(
    State(supervisor): State<RoomCreationSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<RoomCreationStatusV1>, RoomCreationErrorV1> {
    tokio::task::spawn_blocking(move || supervisor.status(&draft_id))
        .await
        .map_err(|_| RoomCreationErrorV1::Unavailable)?
        .map(RoomCreationStatusV1::from)
        .map(Json)
}

async fn start_creation(
    State(supervisor): State<RoomCreationSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<RoomCreationStatusV1>, RoomCreationErrorV1> {
    tokio::task::spawn_blocking(move || supervisor.start(&draft_id))
        .await
        .map_err(|_| RoomCreationErrorV1::Unavailable)?
        .map(RoomCreationStatusV1::from)
        .map(Json)
}

async fn retry_creation(
    State(supervisor): State<RoomCreationSupervisorV1>,
    AxumPath(draft_id): AxumPath<String>,
) -> Result<Json<RoomCreationStatusV1>, RoomCreationErrorV1> {
    tokio::task::spawn_blocking(move || supervisor.reconcile(&draft_id))
        .await
        .map_err(|_| RoomCreationErrorV1::Unavailable)?
        .map(RoomCreationStatusV1::from)
        .map(Json)
}

impl IntoResponse for RoomCreationErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, message, retryable) = match self {
            Self::InvalidDraft => (
                StatusCode::BAD_REQUEST,
                "reviewed_room_draft_invalid",
                "the reviewed Room draft is invalid",
                false,
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "room_creation_not_found",
                "the Room creation operation is unavailable",
                false,
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "room_creation_store_unavailable",
                "the protected Room creation operation store is unavailable",
                true,
            ),
        };
        (
            status,
            Json(serde_json::json!({
                "error": { "code": code, "message": message, "retryable": retryable }
            })),
        )
            .into_response()
    }
}

fn prepare_operation(draft: &RoomDraftV1) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
    if draft.last_valid_step != Some(RoomDraftStepV1::Review) {
        return Err(RoomCreationErrorV1::InvalidDraft);
    }
    let pack = draft
        .pack
        .clone()
        .ok_or(RoomCreationErrorV1::InvalidDraft)?;
    let members = draft
        .seats
        .iter()
        .filter_map(|seat| {
            seat.principal_id.as_ref().map(|principal_id| {
                let principal_kind = seat
                    .principal_kind
                    .ok_or(RoomCreationErrorV1::InvalidDraft)?;
                Ok(CreateMember {
                    principal_id: principal_id.clone(),
                    principal_kind,
                    role: Some(seat.role.clone()),
                    access_mode: AccessMode::Participant,
                })
            })
        })
        .collect::<Result<Vec<_>, RoomCreationErrorV1>>()?;
    if members.is_empty() {
        return Err(RoomCreationErrorV1::InvalidDraft);
    }
    let operation_id = random_identity("room-create-operation")?;
    let idempotency_key = random_identity("studio-room-create")?;
    let request = CreateRoomRequest {
        pack,
        configuration: draft.configuration.clone(),
        members,
        idempotency_key: idempotency_key.clone(),
    };
    let review = RoomDraftReviewV1 {
        pack: draft.pack.clone(),
        configuration: draft.configuration.clone(),
        seats: draft.seats.clone(),
        readiness: draft.readiness.clone(),
    };
    let review_bytes =
        serde_json::to_vec(&review).map_err(|_| RoomCreationErrorV1::InvalidDraft)?;
    let intent = serde_json::to_vec(&request).map_err(|_| RoomCreationErrorV1::InvalidDraft)?;
    Ok(RoomCreationOperationV1 {
        schema: OPERATION_SCHEMA_V1.to_owned(),
        draft_id: draft.draft_id.clone(),
        operation_id,
        idempotency_key,
        review_hash: digest(&review_bytes),
        intent_hash: digest(&intent),
        review,
        request,
        state: RoomCreationStateV1::Waiting,
        attempts: 0,
        room_id: None,
        response: None,
        response_hash: None,
        attention: None,
    })
}

fn validate_operation(
    operation: &RoomCreationOperationV1,
    draft_id: &str,
) -> Result<(), RoomCreationErrorV1> {
    if operation.schema != OPERATION_SCHEMA_V1
        || operation.draft_id != draft_id
        || operation.operation_id.is_empty()
        || operation.idempotency_key.is_empty()
        || operation.request.idempotency_key != operation.idempotency_key
        || digest(
            &serde_json::to_vec(&operation.request)
                .map_err(|_| RoomCreationErrorV1::Unavailable)?,
        ) != operation.intent_hash
        || digest(
            &serde_json::to_vec(&operation.review).map_err(|_| RoomCreationErrorV1::Unavailable)?,
        ) != operation.review_hash
        || operation.intent_hash.len() != 71
        || !request_matches_review(&operation.request, &operation.review)
        || !operation_state_is_consistent(operation)
    {
        return Err(RoomCreationErrorV1::Unavailable);
    }
    Ok(())
}

fn request_matches_review(request: &CreateRoomRequest, review: &RoomDraftReviewV1) -> bool {
    let expected_members = review
        .seats
        .iter()
        .filter_map(|seat| {
            seat.principal_id.as_ref().and_then(|principal_id| {
                seat.principal_kind.map(|principal_kind| CreateMember {
                    principal_id: principal_id.clone(),
                    principal_kind,
                    role: Some(seat.role.clone()),
                    access_mode: AccessMode::Participant,
                })
            })
        })
        .collect::<Vec<_>>();
    review.pack.as_ref() == Some(&request.pack)
        && review.configuration == request.configuration
        && expected_members == request.members
}

fn operation_state_is_consistent(operation: &RoomCreationOperationV1) -> bool {
    match operation.state {
        RoomCreationStateV1::Succeeded => {
            operation.attention.is_none()
                && operation
                    .room_id
                    .as_ref()
                    .zip(operation.response.as_ref())
                    .zip(operation.response_hash.as_ref())
                    .is_some_and(|((room_id, response), stored_hash)| {
                        room_id == &response.room_id
                            && response_is_bound(&operation.request, response)
                            && response_hash(&operation.intent_hash, response)
                                .is_ok_and(|expected| &expected == stored_hash)
                    })
        }
        RoomCreationStateV1::Waiting => {
            operation.room_id.is_none()
                && operation.response.is_none()
                && operation.response_hash.is_none()
                && operation.attention.is_none()
        }
        RoomCreationStateV1::Retrying => {
            operation.room_id.is_none()
                && operation.response.is_none()
                && operation.response_hash.is_none()
                && operation.attention.as_ref() == Some(&ambiguous_attention())
        }
        RoomCreationStateV1::NeedsAttention => {
            operation.room_id.is_none()
                && operation.response.is_none()
                && operation.response_hash.is_none()
                && operation.attention.as_ref().is_some_and(|attention| {
                    attention == &operator_fix_attention() || attention == &rejected_attention()
                })
        }
    }
}

fn response_is_bound(request: &CreateRoomRequest, response: &CreateRoomResponse) -> bool {
    response.room_id == response.room_head.room_id
        && response.room_head.pack_digest == request.pack.digest
        && response.member_ids.len() == request.members.len()
}

fn response_hash(
    intent_hash: &str,
    response: &CreateRoomResponse,
) -> Result<String, RoomCreationErrorV1> {
    let binding = serde_json::to_vec(&(RESPONSE_BINDING_DOMAIN_V1, intent_hash, response))
        .map_err(|_| RoomCreationErrorV1::Unavailable)?;
    Ok(digest(&binding))
}

fn validate_draft_id(value: &str) -> Result<(), RoomCreationErrorV1> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(RoomCreationErrorV1::InvalidDraft);
    }
    Ok(())
}

fn random_identity(prefix: &str) -> Result<String, RoomCreationErrorV1> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RoomCreationErrorV1::Unavailable)?;
    Ok(format!("{prefix}-{}", lower_hex(&bytes)))
}

fn random_suffix() -> Result<String, RoomCreationErrorV1> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RoomCreationErrorV1::Unavailable)?;
    Ok(lower_hex(&bytes))
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

fn digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn operation_is_terminal(operation: &RoomCreationOperationV1) -> bool {
    operation.state == RoomCreationStateV1::Succeeded
        || (operation.state == RoomCreationStateV1::NeedsAttention
            && operation
                .attention
                .as_ref()
                .is_some_and(|attention| !attention.retryable))
}

fn ambiguous_attention() -> RoomCreationAttentionV1 {
    RoomCreationAttentionV1 {
        code: "daemon_result_ambiguous".to_owned(),
        message: "The original Room creation result is not resolved yet; retrying will reuse the exact persisted intent.".to_owned(),
        retryable: true,
    }
}

fn operator_fix_attention() -> RoomCreationAttentionV1 {
    RoomCreationAttentionV1 {
        code: "daemon_authorization_unavailable".to_owned(),
        message: "Repair the configured daemon authorization, then retry the original Room creation operation.".to_owned(),
        retryable: true,
    }
}

fn rejected_attention() -> RoomCreationAttentionV1 {
    RoomCreationAttentionV1 {
        code: "reviewed_intent_rejected".to_owned(),
        message: "The reviewed Room creation intent was rejected and cannot be retried.".to_owned(),
        retryable: false,
    }
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, &[u8]), RoomCreationAttemptErrorV1> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(RoomCreationAttemptErrorV1::Ambiguous)?;
    let headers = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(RoomCreationAttemptErrorV1::Ambiguous)?;
    Ok((status, &bytes[(separator + 4)..]))
}
