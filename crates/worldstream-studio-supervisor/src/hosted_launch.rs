//! Narrow, retained Hosted Activity launch adapter.
//!
//! The adapter accepts only a service-authenticated frozen launch. It resolves
//! compile-time reviewed artifacts, re-derives the complete Room setup, binds
//! one launch digest to one existing setup-operation identity, and delegates to
//! the ordinary retained Room setup lifecycle. It is not a generic Host proxy.

use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use ring::hmac;
use serde::{Deserialize, Serialize};
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1, HostedLaunchStageV1,
    HostedLaunchStatusV1, HouseAgentRevision, ListingRevision,
    validate_hosted_launch_evidence_request, validate_hosted_launch_request,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    room_setup_operations::{
        RoomSetupCreateRequestV1, RoomSetupOperationErrorV1, RoomSetupOperationStatusV1,
        RoomSetupOperationsV1,
    },
    room_setup_spec::RoomSetupSpecificationV1,
    task_setup::{TaskLaunchStateV1, TaskSetupErrorV1, TaskSetupSupervisorV1},
};

const BINDING_SCHEMA_V1: &str = "worldstream/hosted-launch-binding/v1";
const ACCESS_TAG_KEY: &[u8] = b"worldstream/hosted-controller-authority/v1";
const MAX_BINDING_BYTES: usize = 16 * 1024;
const MAX_BINDINGS: usize = 256;
const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// Closed failures safe to expose to the colocated Hosted Gateway.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedLaunchErrorV1 {
    Invalid,
    Conflict,
    NotFound,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedHostedLaunchBindingV1 {
    schema: String,
    host_installation_id: String,
    listing_revision_digest: String,
    launch_request_digest: String,
    launch_input_digest: String,
    frozen_roster_digest: String,
    room_setup_specification_digest: String,
    room_setup_operation_id: String,
    capacity_reservation_reference: String,
}

trait HostedRoomOperationBackendV1: Send + Sync + 'static {
    fn advance(
        &self,
        operation: &str,
        specification: RoomSetupSpecificationV1,
    ) -> Result<RoomSetupOperationStatusV1, HostedLaunchErrorV1>;

    fn inspect(&self, operation: &str) -> Result<RoomSetupOperationStatusV1, HostedLaunchErrorV1>;
}

#[derive(Clone)]
struct LiveHostedRoomOperationBackendV1 {
    rooms: RoomSetupOperationsV1,
    setup: TaskSetupSupervisorV1,
}

impl HostedRoomOperationBackendV1 for LiveHostedRoomOperationBackendV1 {
    fn advance(
        &self,
        operation: &str,
        specification: RoomSetupSpecificationV1,
    ) -> Result<RoomSetupOperationStatusV1, HostedLaunchErrorV1> {
        let status = match self.rooms.status(operation) {
            Ok(_) => self.rooms.resume(operation),
            Err(RoomSetupOperationErrorV1::NotFound) => self.rooms.create(
                operation,
                &RoomSetupCreateRequestV1 {
                    specification,
                    acknowledge_start: false,
                },
            ),
            Err(error) => return Err(map_room_error(&error)),
        }
        .map_err(|error| map_room_error(&error))?;

        if status.complete {
            match self.setup.launch(operation) {
                Ok(_) | Err(TaskSetupErrorV1::NotReady) => {}
                Err(TaskSetupErrorV1::NotFound | TaskSetupErrorV1::InvalidCreation) => {
                    return Err(HostedLaunchErrorV1::Invalid);
                }
                Err(TaskSetupErrorV1::Unavailable) => {
                    return Err(HostedLaunchErrorV1::Unavailable);
                }
            }
        }
        self.rooms
            .status(operation)
            .map_err(|error| map_room_error(&error))
    }

    fn inspect(&self, operation: &str) -> Result<RoomSetupOperationStatusV1, HostedLaunchErrorV1> {
        self.rooms
            .status(operation)
            .map_err(|error| map_room_error(&error))
    }
}

fn map_room_error(error: &RoomSetupOperationErrorV1) -> HostedLaunchErrorV1 {
    match error {
        RoomSetupOperationErrorV1::Conflict => HostedLaunchErrorV1::Conflict,
        RoomSetupOperationErrorV1::NotFound => HostedLaunchErrorV1::NotFound,
        RoomSetupOperationErrorV1::Unavailable => HostedLaunchErrorV1::Unavailable,
        RoomSetupOperationErrorV1::Specification(_)
        | RoomSetupOperationErrorV1::Invalid
        | RoomSetupOperationErrorV1::AcknowledgementRequired => HostedLaunchErrorV1::Invalid,
    }
}

/// Retained single-Host mapping from a frozen platform launch to one Room setup operation.
#[derive(Clone)]
pub struct HostedLaunchOperationsV1 {
    root: Arc<PathBuf>,
    host_installation_id: Arc<str>,
    listings: Arc<BTreeMap<String, ListingRevision>>,
    house_agents: Arc<Vec<HouseAgentRevision>>,
    backend: Arc<dyn HostedRoomOperationBackendV1>,
    mutation: Arc<Mutex<()>>,
}

impl HostedLaunchOperationsV1 {
    /// Opens the protected binding store with an exact reviewed artifact registry.
    ///
    /// # Errors
    /// Rejects duplicate artifacts, unsafe storage, or an invalid Host identity.
    pub fn open(
        root: &Path,
        host_installation_id: &str,
        listings: Vec<ListingRevision>,
        house_agents: Vec<HouseAgentRevision>,
        rooms: RoomSetupOperationsV1,
        setup: TaskSetupSupervisorV1,
    ) -> Result<Self, HostedLaunchErrorV1> {
        Self::open_with_backend(
            root,
            host_installation_id,
            listings,
            house_agents,
            LiveHostedRoomOperationBackendV1 { rooms, setup },
        )
    }

    fn open_with_backend(
        root: &Path,
        host_installation_id: &str,
        listings: Vec<ListingRevision>,
        house_agents: Vec<HouseAgentRevision>,
        backend: impl HostedRoomOperationBackendV1,
    ) -> Result<Self, HostedLaunchErrorV1> {
        if !safe_public_reference(host_installation_id, 128)
            || listings.is_empty()
            || listings.len() > 64
            || house_agents.len() > 32
        {
            return Err(HostedLaunchErrorV1::Invalid);
        }
        let mut indexed = BTreeMap::new();
        for listing in listings {
            if indexed
                .insert(listing.digest().to_owned(), listing)
                .is_some()
            {
                return Err(HostedLaunchErrorV1::Invalid);
            }
        }
        let mut house_digests = BTreeMap::new();
        for house_agent in &house_agents {
            if house_digests
                .insert(house_agent.digest(), house_agent.house_agent_id())
                .is_some()
            {
                return Err(HostedLaunchErrorV1::Invalid);
            }
        }
        let root = prepare_data_directory(root).map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            host_installation_id: Arc::from(host_installation_id),
            listings: Arc::new(indexed),
            house_agents: Arc::new(house_agents),
            backend: Arc::new(backend),
            mutation: Arc::new(Mutex::new(())),
        })
    }

    /// Retains or resumes one exact launch before delegating any Room mutation.
    ///
    /// # Errors
    /// Fails closed for changed identities, an unreviewed artifact, ambiguous
    /// protected storage, or an unavailable retained Room operation.
    pub fn submit(
        &self,
        request: &HostedLaunchRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedLaunchErrorV1> {
        let listing = self
            .listings
            .get(&request.listing_revision_digest)
            .ok_or(HostedLaunchErrorV1::Invalid)?;
        let setup = validate_hosted_launch_request(
            request,
            &self.host_installation_id,
            listing,
            &self.house_agents,
        )
        .map_err(|_| HostedLaunchErrorV1::Invalid)?;
        let specification = serde_json::from_slice::<RoomSetupSpecificationV1>(
            &setup
                .canonical_bytes()
                .map_err(|_| HostedLaunchErrorV1::Invalid)?,
        )
        .map_err(|_| HostedLaunchErrorV1::Invalid)?;
        let binding = RetainedHostedLaunchBindingV1 {
            schema: BINDING_SCHEMA_V1.to_owned(),
            host_installation_id: self.host_installation_id.to_string(),
            listing_revision_digest: request.listing_revision_digest.clone(),
            launch_request_digest: request.launch_request_digest.clone(),
            launch_input_digest: request.launch_input_digest.clone(),
            frozen_roster_digest: request.frozen_roster_digest.clone(),
            room_setup_specification_digest: request.room_setup_specification_digest.clone(),
            room_setup_operation_id: request.room_setup_operation_id.clone(),
            capacity_reservation_reference: request
                .capacity_authorization
                .reservation_reference
                .clone(),
        };
        self.bind(&binding)?;
        let status = self
            .backend
            .advance(&binding.room_setup_operation_id, specification)?;
        Ok(public_status(&binding, Some(status)))
    }

    /// Reads one exact retained launch without creating or replacing intent.
    ///
    /// # Errors
    /// Returns not-found for an absent exact binding and conflict for changed identity.
    pub fn read(
        &self,
        request: &HostedLaunchEvidenceRequestV1,
    ) -> Result<HostedLaunchStatusV1, HostedLaunchErrorV1> {
        validate_hosted_launch_evidence_request(request)
            .map_err(|_| HostedLaunchErrorV1::Invalid)?;
        let _guard = self.lock();
        let binding = self.load_unlocked(&request.room_setup_operation_id)?;
        if binding.listing_revision_digest != request.listing_revision_digest
            || binding.launch_request_digest != request.launch_request_digest
        {
            return Err(HostedLaunchErrorV1::Conflict);
        }
        let room = match self.backend.inspect(&binding.room_setup_operation_id) {
            Ok(status) => Some(status),
            Err(HostedLaunchErrorV1::NotFound) => None,
            Err(error) => return Err(error),
        };
        Ok(public_status(&binding, room))
    }

    fn bind(&self, requested: &RetainedHostedLaunchBindingV1) -> Result<(), HostedLaunchErrorV1> {
        let _guard = self.lock();
        match self.load_unlocked(&requested.room_setup_operation_id) {
            Ok(existing) if existing == *requested => return Ok(()),
            Ok(_) => return Err(HostedLaunchErrorV1::Conflict),
            Err(HostedLaunchErrorV1::NotFound) => {}
            Err(error) => return Err(error),
        }
        for binding in self.bindings_unlocked()? {
            if binding.launch_request_digest == requested.launch_request_digest
                || binding.capacity_reservation_reference
                    == requested.capacity_reservation_reference
            {
                return Err(HostedLaunchErrorV1::Conflict);
            }
        }
        match self.backend.inspect(&requested.room_setup_operation_id) {
            Err(HostedLaunchErrorV1::NotFound) => {}
            Ok(_) => return Err(HostedLaunchErrorV1::Conflict),
            Err(error) => return Err(error),
        }
        self.persist_new(requested)
    }

    fn bindings_unlocked(&self) -> Result<Vec<RetainedHostedLaunchBindingV1>, HostedLaunchErrorV1> {
        let entries =
            fs::read_dir(self.root.as_ref()).map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        let mut bindings = Vec::new();
        for entry in entries {
            let path = entry.map_err(|_| HostedLaunchErrorV1::Unavailable)?.path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or(HostedLaunchErrorV1::Unavailable)?;
            if name.starts_with('.')
                && Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
            {
                continue;
            }
            let operation = name
                .strip_suffix(".json")
                .ok_or(HostedLaunchErrorV1::Unavailable)?;
            bindings.push(self.load_unlocked(operation)?);
            if bindings.len() > MAX_BINDINGS {
                return Err(HostedLaunchErrorV1::Unavailable);
            }
        }
        Ok(bindings)
    }

    fn load_unlocked(
        &self,
        operation: &str,
    ) -> Result<RetainedHostedLaunchBindingV1, HostedLaunchErrorV1> {
        if !safe_operation(operation) {
            return Err(HostedLaunchErrorV1::Invalid);
        }
        let path = self.root.join(format!("{operation}.json"));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(HostedLaunchErrorV1::NotFound);
            }
            Err(_) => return Err(HostedLaunchErrorV1::Unavailable),
            Ok(_) => {}
        }
        validate_owner_only_file(&path).map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        let bytes = fs::read(&path).map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        if bytes.len() > MAX_BINDING_BYTES {
            return Err(HostedLaunchErrorV1::Unavailable);
        }
        let binding = serde_json::from_slice::<RetainedHostedLaunchBindingV1>(&bytes)
            .map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        if !valid_binding(&binding, operation, &self.host_installation_id) {
            return Err(HostedLaunchErrorV1::Unavailable);
        }
        Ok(binding)
    }

    fn persist_new(
        &self,
        binding: &RetainedHostedLaunchBindingV1,
    ) -> Result<(), HostedLaunchErrorV1> {
        if !valid_binding(
            binding,
            &binding.room_setup_operation_id,
            &self.host_installation_id,
        ) {
            return Err(HostedLaunchErrorV1::Invalid);
        }
        let bytes =
            serde_json::to_vec_pretty(binding).map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        if bytes.len() > MAX_BINDING_BYTES {
            return Err(HostedLaunchErrorV1::Unavailable);
        }
        let nonce = blake3::hash(
            &getrandom::u64()
                .map_err(|_| HostedLaunchErrorV1::Unavailable)?
                .to_le_bytes(),
        );
        let temporary = self.root.join(format!(
            ".{}.{}.tmp",
            binding.room_setup_operation_id,
            nonce.to_hex()
        ));
        let target = self
            .root
            .join(format!("{}.json", binding.room_setup_operation_id));
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| HostedLaunchErrorV1::Unavailable)?;
        if file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(&temporary);
            return Err(HostedLaunchErrorV1::Unavailable);
        }
        drop(file);
        let published = fs::hard_link(&temporary, target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| HostedLaunchErrorV1::Unavailable);
        let _ = fs::remove_file(temporary);
        published
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn public_status(
    binding: &RetainedHostedLaunchBindingV1,
    status: Option<RoomSetupOperationStatusV1>,
) -> HostedLaunchStatusV1 {
    let Some(status) = status else {
        return HostedLaunchStatusV1 {
            schema: "worldstream/hosted-launch-status/v1".to_owned(),
            listing_revision_digest: binding.listing_revision_digest.clone(),
            launch_request_digest: binding.launch_request_digest.clone(),
            room_setup_operation_id: binding.room_setup_operation_id.clone(),
            room_id: None,
            stage: HostedLaunchStageV1::Bound,
            room_setup_complete: false,
            lobby_launch_committed: false,
            retryable: true,
            terminal_before_genesis: false,
        };
    };
    let launch = status
        .assessment
        .as_ref()
        .and_then(|assessment| assessment.launch.as_ref());
    let lobby_launch_committed =
        launch.is_some_and(|item| item.state == TaskLaunchStateV1::Launched);
    let attention_retryable = launch
        .and_then(|item| item.attention.as_ref())
        .map(|attention| attention.retryable);
    let needs_attention = status.next_action == "inspect_operation"
        || launch.is_some_and(|item| item.state == TaskLaunchStateV1::NeedsAttention);
    let stage = if lobby_launch_committed {
        HostedLaunchStageV1::Launched
    } else if needs_attention {
        HostedLaunchStageV1::NeedsAttention
    } else if launch.is_some() {
        HostedLaunchStageV1::Launching
    } else if status.complete {
        HostedLaunchStageV1::WaitingForReadiness
    } else if status.room_id.is_some() {
        HostedLaunchStageV1::Provisioning
    } else {
        HostedLaunchStageV1::CreatingRoom
    };
    let retryable = attention_retryable.unwrap_or(!needs_attention);
    HostedLaunchStatusV1 {
        schema: "worldstream/hosted-launch-status/v1".to_owned(),
        listing_revision_digest: binding.listing_revision_digest.clone(),
        launch_request_digest: binding.launch_request_digest.clone(),
        room_setup_operation_id: binding.room_setup_operation_id.clone(),
        room_id: status.room_id.clone(),
        stage,
        room_setup_complete: status.complete,
        lobby_launch_committed,
        retryable,
        terminal_before_genesis: !lobby_launch_committed && needs_attention && !retryable,
    }
}

fn valid_binding(
    binding: &RetainedHostedLaunchBindingV1,
    operation: &str,
    host_installation_id: &str,
) -> bool {
    binding.schema == BINDING_SCHEMA_V1
        && binding.host_installation_id == host_installation_id
        && binding.room_setup_operation_id == operation
        && safe_operation(operation)
        && tagged_digest(&binding.listing_revision_digest, "blake3")
        && tagged_digest(&binding.launch_request_digest, "blake3")
        && tagged_digest(&binding.launch_input_digest, "sha256")
        && tagged_digest(&binding.frozen_roster_digest, "sha256")
        && tagged_digest(&binding.room_setup_specification_digest, "blake3")
        && uuid_reference(&binding.capacity_reservation_reference)
}

fn safe_operation(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn safe_public_reference(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn tagged_digest(value: &str, algorithm: &str) -> bool {
    value
        .strip_prefix(&format!("{algorithm}:"))
        .is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn uuid_reference(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

/// Dedicated least-privilege bearer verifier for the colocated Hosted Gateway.
#[derive(Clone)]
pub struct HostedLaunchAccessV1 {
    authority_tag: [u8; 32],
}

impl HostedLaunchAccessV1 {
    /// Creates an in-memory verifier without retaining the raw shared authority.
    ///
    /// # Errors
    /// Rejects weak, unbounded, or non-graphic authority material.
    pub fn new(authority: &str) -> Result<Self, HostedLaunchErrorV1> {
        if authority.len() < 32
            || authority.len() > 512
            || !authority.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(HostedLaunchErrorV1::Invalid);
        }
        let key = hmac::Key::new(hmac::HMAC_SHA256, ACCESS_TAG_KEY);
        let tag = hmac::sign(&key, authority.as_bytes());
        Ok(Self {
            authority_tag: tag
                .as_ref()
                .try_into()
                .map_err(|_| HostedLaunchErrorV1::Invalid)?,
        })
    }

    fn authenticate(&self, headers: &HeaderMap) -> bool {
        let mut values = headers.get_all(header::AUTHORIZATION).iter();
        let Some(value) = values.next() else {
            return false;
        };
        if values.next().is_some() {
            return false;
        }
        let Some(token) = value.as_bytes().strip_prefix(b"Bearer ") else {
            return false;
        };
        let key = hmac::Key::new(hmac::HMAC_SHA256, ACCESS_TAG_KEY);
        hmac::verify(&key, token, &self.authority_tag).is_ok()
    }
}

/// The only Controller routes which may bypass installation-owner admission.
#[must_use]
pub fn is_hosted_launch_route(method: &Method, path: &str) -> bool {
    matches!(
        (method.as_str(), path),
        ("GET", "/api/v1/hosted-launches/ready")
            | (
                "POST",
                "/api/v1/hosted-launches:submit" | "/api/v1/hosted-launches:read"
            )
    )
}

/// Builds the exact service-only Controller surface for hosted launch and evidence.
pub fn hosted_launch_router(
    operations: HostedLaunchOperationsV1,
    access: HostedLaunchAccessV1,
) -> Router {
    Router::new()
        .route("/api/v1/hosted-launches/ready", get(hosted_ready))
        .route("/api/v1/hosted-launches:submit", post(hosted_submit))
        .route("/api/v1/hosted-launches:read", post(hosted_read))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(from_fn_with_state(Arc::new(access), admit_hosted_launch))
        .with_state(operations)
}

async fn hosted_ready() -> Json<serde_json::Value> {
    Json(serde_json::json!({"schema":"worldstream/hosted-launch-readiness/v1","ready":true}))
}

async fn hosted_submit(
    State(operations): State<HostedLaunchOperationsV1>,
    body: Bytes,
) -> Result<Response, HostedLaunchErrorV1> {
    let request = decode_request::<HostedLaunchRequestV1>(&body)?;
    let status = tokio::task::spawn_blocking(move || operations.submit(&request))
        .await
        .map_err(|_| HostedLaunchErrorV1::Unavailable)??;
    Ok((
        if status.lobby_launch_committed {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(status),
    )
        .into_response())
}

async fn hosted_read(
    State(operations): State<HostedLaunchOperationsV1>,
    body: Bytes,
) -> Result<Json<HostedLaunchStatusV1>, HostedLaunchErrorV1> {
    let request = decode_request::<HostedLaunchEvidenceRequestV1>(&body)?;
    tokio::task::spawn_blocking(move || operations.read(&request))
        .await
        .map_err(|_| HostedLaunchErrorV1::Unavailable)?
        .map(Json)
}

fn decode_request<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, HostedLaunchErrorV1> {
    CanonicalJsonV1::from_canonical_bytes(body).map_err(|_| HostedLaunchErrorV1::Invalid)?;
    serde_json::from_slice(body).map_err(|_| HostedLaunchErrorV1::Invalid)
}

async fn admit_hosted_launch(
    State(access): State<Arc<HostedLaunchAccessV1>>,
    mut request: Request,
    next: Next,
) -> Response {
    if !access.authenticate(request.headers()) {
        return hosted_error(StatusCode::UNAUTHORIZED, "hosted_authority_required");
    }
    request.headers_mut().remove(header::AUTHORIZATION);
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    response
}

impl IntoResponse for HostedLaunchErrorV1 {
    fn into_response(self) -> Response {
        match self {
            Self::Invalid => hosted_error(StatusCode::BAD_REQUEST, "hosted_launch_invalid"),
            Self::Conflict => hosted_error(StatusCode::CONFLICT, "hosted_launch_conflict"),
            Self::NotFound => hosted_error(StatusCode::NOT_FOUND, "hosted_launch_not_found"),
            Self::Unavailable => {
                hosted_error(StatusCode::SERVICE_UNAVAILABLE, "hosted_launch_unavailable")
            }
        }
    }
}

fn hosted_error(status: StatusCode, code: &'static str) -> Response {
    let mut response = (status, Json(serde_json::json!({"error":{"code":code}}))).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, max-age=0"),
    );
    response
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use ring::digest;
    use serde_json::Value;
    use tempfile::tempdir;
    use tower::ServiceExt as _;
    use worldstream_hosted_contract::{
        HostedCapacityAuthorizationV1, HostedLaunchEvidenceRequestV1, HostedLaunchRequestV1,
    };

    use super::*;
    use crate::room_setup_operations::RoomSetupOperationStageV1;

    const LISTING: &[u8] = include_bytes!("../../../config/hosted/listings/agent-heist-0.2.0.json");
    const LAUNCH: &[u8] =
        include_bytes!("../../../fixtures/hosted-contract/valid/agent-heist-launch-request.json");
    const ROSTER: &[u8] =
        include_bytes!("../../../fixtures/hosted-contract/valid/agent-heist-frozen-roster.json");
    const SETUP: &[u8] =
        include_bytes!("../../../fixtures/hosted-contract/expected/agent-heist-room-setup.json");

    #[derive(Clone, Default)]
    struct FakeBackend {
        statuses: Arc<Mutex<BTreeMap<String, RoomSetupOperationStatusV1>>>,
        advances: Arc<Mutex<Vec<String>>>,
    }

    impl HostedRoomOperationBackendV1 for FakeBackend {
        fn advance(
            &self,
            operation: &str,
            _specification: RoomSetupSpecificationV1,
        ) -> Result<RoomSetupOperationStatusV1, HostedLaunchErrorV1> {
            self.advances
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(operation.to_owned());
            let status = room_status(operation);
            self.statuses
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(operation.to_owned(), status.clone());
            Ok(status)
        }

        fn inspect(
            &self,
            operation: &str,
        ) -> Result<RoomSetupOperationStatusV1, HostedLaunchErrorV1> {
            self.statuses
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(operation)
                .cloned()
                .ok_or(HostedLaunchErrorV1::NotFound)
        }
    }

    fn room_status(operation: &str) -> RoomSetupOperationStatusV1 {
        RoomSetupOperationStatusV1 {
            version: "room_setup_operation.v1".to_owned(),
            operation: operation.to_owned(),
            room_id: None,
            complete: false,
            stage: RoomSetupOperationStageV1::Creation,
            active_stage: None,
            next_action: "resume".to_owned(),
            assessment: None,
        }
    }

    fn canonical(source: &[u8]) -> Vec<u8> {
        CanonicalJsonV1::parse(source)
            .and_then(|value| value.to_bytes())
            .unwrap_or_else(|error| unreachable!("valid fixture: {error}"))
    }

    fn value(source: &[u8]) -> Value {
        serde_json::from_slice(source)
            .unwrap_or_else(|error| unreachable!("valid fixture: {error}"))
    }

    fn sha256(bytes: &[u8]) -> String {
        let mut value = String::from("sha256:");
        for byte in digest::digest(&digest::SHA256, bytes).as_ref() {
            value.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
            value.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
        }
        value
    }

    fn request(operation: &str) -> HostedLaunchRequestV1 {
        let launch = canonical(LAUNCH);
        let roster = canonical(ROSTER);
        let setup = canonical(SETUP);
        HostedLaunchRequestV1 {
            schema: "worldstream/hosted-launch-request/v1".to_owned(),
            listing_revision_digest:
                "blake3:fdb9f9a4b72e83aefde1a98aca89a3c75a100fcddb7dcb29c9896228f6d28c1b".to_owned(),
            launch_request_digest: format!("blake3:{}", blake3::hash(&launch).to_hex()),
            launch_input_digest: sha256(b"{}"),
            frozen_roster_digest: sha256(&roster),
            room_setup_specification_digest: format!("blake3:{}", blake3::hash(&setup).to_hex()),
            room_setup_operation_id: operation.to_owned(),
            capacity_authorization: HostedCapacityAuthorizationV1 {
                schema: "worldstream/platform-capacity-authorization/v1".to_owned(),
                host_installation_id: "hosted-test".to_owned(),
                reservation_reference: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
            },
            frozen_launch_request: value(&launch),
            frozen_roster: value(&roster),
            frozen_room_setup_specification: value(&setup),
        }
    }

    fn listing() -> ListingRevision {
        ListingRevision::from_canonical_bytes(&canonical(LISTING))
            .unwrap_or_else(|error| unreachable!("valid fixture: {error}"))
    }

    #[test]
    fn exact_retry_and_restart_reuse_one_retained_binding() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
        let root = directory.path().join("hosted");
        let backend = FakeBackend::default();
        let operations = HostedLaunchOperationsV1::open_with_backend(
            &root,
            "hosted-test",
            vec![listing()],
            Vec::new(),
            backend.clone(),
        )
        .unwrap_or_else(|error| unreachable!("valid operations: {error:?}"));
        let request = request("hosted-launch-01");
        let first = operations
            .submit(&request)
            .unwrap_or_else(|error| unreachable!("first submit: {error:?}"));
        let second = operations
            .submit(&request)
            .unwrap_or_else(|error| unreachable!("retry submit: {error:?}"));
        assert_eq!(first, second);
        assert_eq!(
            backend
                .advances
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            ["hosted-launch-01", "hosted-launch-01"]
        );

        let reopened = HostedLaunchOperationsV1::open_with_backend(
            &root,
            "hosted-test",
            vec![listing()],
            Vec::new(),
            backend,
        )
        .unwrap_or_else(|error| unreachable!("reopened operations: {error:?}"));
        let evidence = HostedLaunchEvidenceRequestV1 {
            schema: "worldstream/hosted-launch-evidence-request/v1".to_owned(),
            listing_revision_digest: request.listing_revision_digest.clone(),
            launch_request_digest: request.launch_request_digest.clone(),
            room_setup_operation_id: request.room_setup_operation_id.clone(),
        };
        assert_eq!(
            reopened
                .read(&evidence)
                .unwrap_or_else(|error| unreachable!("retained read: {error:?}")),
            first
        );
    }

    #[test]
    fn changed_operation_digest_or_existing_generic_operation_fails_closed() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
        let root = directory.path().join("hosted");
        let backend = FakeBackend::default();
        let operations = HostedLaunchOperationsV1::open_with_backend(
            &root,
            "hosted-test",
            vec![listing()],
            Vec::new(),
            backend.clone(),
        )
        .unwrap_or_else(|error| unreachable!("valid operations: {error:?}"));
        let original = request("hosted-launch-01");
        assert!(operations.submit(&original).is_ok());

        let mut changed_operation = original.clone();
        changed_operation.room_setup_operation_id = "hosted-launch-02".to_owned();
        assert_eq!(
            operations.submit(&changed_operation),
            Err(HostedLaunchErrorV1::Conflict)
        );
        let mut changed_setup = original;
        changed_setup.frozen_room_setup_specification["operator_view"] = serde_json::json!(true);
        assert_eq!(
            operations.submit(&changed_setup),
            Err(HostedLaunchErrorV1::Invalid)
        );

        backend
            .statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                "unbound-operation".to_owned(),
                room_status("unbound-operation"),
            );
        assert_eq!(
            operations.submit(&request("unbound-operation")),
            Err(HostedLaunchErrorV1::Conflict)
        );
    }

    #[test]
    fn non_retryable_pre_genesis_failure_is_terminal_after_room_allocation() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
        let operations = HostedLaunchOperationsV1::open_with_backend(
            &directory.path().join("hosted"),
            "hosted-test",
            vec![listing()],
            Vec::new(),
            FakeBackend::default(),
        )
        .unwrap_or_else(|error| unreachable!("valid operations: {error:?}"));
        let launch = request("hosted-terminal-01");
        assert!(operations.submit(&launch).is_ok());
        let binding = operations
            .load_unlocked(&launch.room_setup_operation_id)
            .unwrap_or_else(|error| unreachable!("retained binding: {error:?}"));
        let mut status = room_status(&launch.room_setup_operation_id);
        status.room_id = Some("01JY0000000000000000000000".to_owned());
        status.next_action = "inspect_operation".to_owned();

        let result = public_status(&binding, Some(status));
        assert_eq!(result.stage, HostedLaunchStageV1::NeedsAttention);
        assert_eq!(
            result.room_id.as_deref(),
            Some("01JY0000000000000000000000")
        );
        assert!(!result.retryable);
        assert!(result.terminal_before_genesis);
        assert!(!result.lobby_launch_committed);
    }

    #[tokio::test]
    async fn dedicated_routes_require_their_separate_authority_before_mutation() {
        const AUTHORITY: &str = "hosted-controller-test-authority-0000000000";
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary store: {error}"));
        let backend = FakeBackend::default();
        let operations = HostedLaunchOperationsV1::open_with_backend(
            &directory.path().join("hosted"),
            "hosted-test",
            vec![listing()],
            Vec::new(),
            backend.clone(),
        )
        .unwrap_or_else(|error| unreachable!("valid operations: {error:?}"));
        let access = HostedLaunchAccessV1::new(AUTHORITY)
            .unwrap_or_else(|error| unreachable!("valid authority: {error:?}"));
        let router = hosted_launch_router(operations, access);

        for authority in [None, Some("Bearer wrong-authority-value-000000000000")] {
            let mut builder = Request::builder()
                .method("GET")
                .uri("/api/v1/hosted-launches/ready");
            if let Some(authority) = authority {
                builder = builder.header(header::AUTHORIZATION, authority);
            }
            let response = router
                .clone()
                .oneshot(
                    builder
                        .body(Body::empty())
                        .unwrap_or_else(|error| unreachable!("request: {error}")),
                )
                .await
                .unwrap_or_else(|error| unreachable!("response: {error}"));
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let launch = request("hosted-launch-route-01");
        let encoded = serde_json::to_vec(&launch)
            .ok()
            .and_then(|source| CanonicalJsonV1::parse(&source).ok())
            .and_then(|value| value.to_bytes().ok())
            .unwrap_or_else(|| unreachable!("canonical launch request"));
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/hosted-launches:submit")
                    .header(header::AUTHORIZATION, format!("Bearer {AUTHORITY}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(encoded))
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let response_body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("response body: {error}"))
            .to_bytes();
        assert!(serde_json::from_slice::<HostedLaunchStatusV1>(&response_body).is_ok());
        assert_eq!(
            backend
                .advances
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            ["hosted-launch-route-01"]
        );
    }
}
