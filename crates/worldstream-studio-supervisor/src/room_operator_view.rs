//! Explicit, durable operator-membership views for a selected Room.
//!
//! The Host Operator is deliberately not treated as a Room Member.  This
//! module therefore owns one opt-in, owner-only intent per Room and uses the
//! normal member Capability and Projection paths after the intent is sealed.

use std::{
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_protocol::{
    AccessMode, BearerWireV1, MemberCapabilityProvisionRequestV1,
    MemberCapabilityProvisionResponseV1, PrincipalKind, ProjectionResponse,
    SealedCapabilityBearerV1, SealedCapabilityInputV1, UlidString,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::{
    room_creation::RoomCreationSupervisorV1,
    secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
    task_setup::{DaemonTaskSetupProvisionerV1, TaskSetupAttemptErrorV1},
};

const ENABLE_SCHEMA_V1: &str = "worldstream/studio-room-operator-view-enable/v1";
const VIEW_SCHEMA_V1: &str = "worldstream/studio-room-operator-view/v1";
const OPERATION_SCHEMA_V1: &str = "worldstream/studio-room-operator-view-operation/v1";
const COUNTER_PROJECTION_SCHEMA_V1: &str = "counter/public-projection/v1";
const MAX_OPERATION_BYTES: usize = 32 * 1024;

/// The only accepted request body for explicit operator-view enablement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnableRoomOperatorViewRequestV1 {
    pub schema: String,
}

/// Browser-safe availability state. It carries no retained authority data.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomOperatorViewStateV1 {
    Available,
    Unavailable,
}

/// Closed reasons deliberately suitable for the local browser API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomOperatorViewUnavailableReasonV1 {
    OperatorMembershipRequired,
    OperatorViewUnavailable,
    UnsupportedProjection,
}

/// Minimal safe Room head signal accompanying the counter value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOperatorViewHeadV1 {
    pub room_seq: u64,
}

/// The single value exposed from the Counter public Projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOperatorCounterViewV1 {
    pub value: u8,
}

/// Strict browser DTO. It never includes member, principal, capability,
/// secret-reference, bearer, observation, Invocation, or Action-offer data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOperatorViewResponseV1 {
    pub schema: String,
    pub room_id: String,
    pub state: RoomOperatorViewStateV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<RoomOperatorViewUnavailableReasonV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counter: Option<RoomOperatorCounterViewV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_head: Option<RoomOperatorViewHeadV1>,
}

impl RoomOperatorViewResponseV1 {
    fn unavailable(room_id: String, reason: RoomOperatorViewUnavailableReasonV1) -> Self {
        Self {
            schema: VIEW_SCHEMA_V1.to_owned(),
            room_id,
            state: RoomOperatorViewStateV1::Unavailable,
            unavailable_reason: Some(reason),
            counter: None,
            room_head: None,
        }
    }

    fn available(room_id: String, value: u8, room_seq: u64) -> Self {
        Self {
            schema: VIEW_SCHEMA_V1.to_owned(),
            room_id,
            state: RoomOperatorViewStateV1::Available,
            unavailable_reason: None,
            counter: Some(RoomOperatorCounterViewV1 { value }),
            room_head: Some(RoomOperatorViewHeadV1 { room_seq }),
        }
    }
}

/// Narrow local daemon read boundary for the standard Projection route.
pub trait DaemonRoomOperatorProjectionV1: Send + Sync + 'static {
    /// Reads only the normal membership-authorized current Projection.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or rejected standard Projection result.
    fn current_projection(
        &self,
        room_id: &str,
        bearer: &SealedCapabilityBearerV1,
    ) -> Result<ProjectionResponse, RoomOperatorProjectionErrorV1>;
}

/// Closed result classes for a normal Projection request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomOperatorProjectionErrorV1 {
    Unavailable,
    Rejected,
}

/// Fixed-address implementation that reads the existing daemon Projection
/// endpoint. It supplies no Host authority and creates no projection bypass.
#[derive(Clone, Debug)]
pub struct HttpDaemonRoomOperatorProjectionV1 {
    address: SocketAddr,
    timeout: Duration,
}

impl HttpDaemonRoomOperatorProjectionV1 {
    #[must_use]
    pub const fn new(address: SocketAddr, timeout: Duration) -> Self {
        Self { address, timeout }
    }
}

impl DaemonRoomOperatorProjectionV1 for HttpDaemonRoomOperatorProjectionV1 {
    fn current_projection(
        &self,
        room_id: &str,
        bearer: &SealedCapabilityBearerV1,
    ) -> Result<ProjectionResponse, RoomOperatorProjectionErrorV1> {
        if room_id.parse::<UlidString>().is_err() {
            return Err(RoomOperatorProjectionErrorV1::Rejected);
        }
        let request = Zeroizing::new(format!(
            "GET /v1/rooms/{room_id}/projection HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            self.address,
            bearer.as_str(),
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| RoomOperatorProjectionErrorV1::Unavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| RoomOperatorProjectionErrorV1::Unavailable)?;
        stream
            .write_all(request.as_bytes())
            .map_err(|_| RoomOperatorProjectionErrorV1::Unavailable)?;
        let mut response = Vec::new();
        stream
            .take(256 * 1024 + 1)
            .read_to_end(&mut response)
            .map_err(|_| RoomOperatorProjectionErrorV1::Unavailable)?;
        if response.len() > 256 * 1024 {
            return Err(RoomOperatorProjectionErrorV1::Unavailable);
        }
        let (status, body) = parse_http_response(&response)?;
        match status {
            200 => {
                serde_json::from_slice(body).map_err(|_| RoomOperatorProjectionErrorV1::Unavailable)
            }
            400 | 401 | 403 | 404 | 409 | 422 => Err(RoomOperatorProjectionErrorV1::Rejected),
            _ => Err(RoomOperatorProjectionErrorV1::Unavailable),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OperationStateV1 {
    Provisioning,
    Retrying,
    Provisioned,
    NeedsAttention,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RoomOperatorViewOperationV1 {
    schema: String,
    room_id: String,
    principal_id: String,
    member_id: String,
    capability_id: String,
    change_id: String,
    secret_reference: SecretReferenceV1,
    state: OperationStateV1,
}

/// Closed local persistence and daemon-boundary errors.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RoomOperatorViewErrorV1 {
    #[error("operator view request is invalid")]
    InvalidRequest,
    #[error("operator view intent was not found")]
    NotFound,
    #[error("operator view persistence is unavailable")]
    Unavailable,
}

/// One exact, per-Room operator membership Capability workflow.
#[derive(Clone)]
pub struct RoomOperatorViewSupervisorV1 {
    root: Arc<PathBuf>,
    vault: FileSecretVaultV1,
    creation: RoomCreationSupervisorV1,
    provisioner: Arc<dyn DaemonTaskSetupProvisionerV1>,
    projection: Arc<dyn DaemonRoomOperatorProjectionV1>,
    mutation: Arc<Mutex<()>>,
}

impl RoomOperatorViewSupervisorV1 {
    /// Opens the protected per-Room intent store.
    ///
    /// # Errors
    ///
    /// Returns a closed error when protected persistence cannot be prepared.
    pub fn open(
        root: &Path,
        vault: FileSecretVaultV1,
        creation: RoomCreationSupervisorV1,
        provisioner: impl DaemonTaskSetupProvisionerV1,
        projection: impl DaemonRoomOperatorProjectionV1,
    ) -> Result<Self, RoomOperatorViewErrorV1> {
        let root =
            prepare_data_directory(root).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            vault,
            creation,
            provisioner: Arc::new(provisioner),
            projection: Arc::new(projection),
            mutation: Arc::new(Mutex::new(())),
        })
    }

    /// Persists the immutable owner-only intent before the first daemon call.
    ///
    /// # Errors
    ///
    /// Returns a closed request or protected-persistence error without minting
    /// replacement authority for an existing operation.
    pub fn enable(
        &self,
        room_id: &str,
        request: &EnableRoomOperatorViewRequestV1,
    ) -> Result<RoomOperatorViewResponseV1, RoomOperatorViewErrorV1> {
        if request.schema != ENABLE_SCHEMA_V1 {
            return Err(RoomOperatorViewErrorV1::InvalidRequest);
        }
        validate_room_id(room_id)?;
        let _guard = self.lock();
        let operation = match self.load_unlocked(room_id) {
            Ok(operation) => operation,
            Err(RoomOperatorViewErrorV1::InvalidRequest | RoomOperatorViewErrorV1::Unavailable) => {
                return Err(RoomOperatorViewErrorV1::Unavailable);
            }
            Err(RoomOperatorViewErrorV1::NotFound) => {
                match self.creation.reviewed_operator_membership(room_id) {
                    Ok(Some(binding)) => self.new_operation(binding)?,
                    Ok(None) => {
                        return Ok(RoomOperatorViewResponseV1::unavailable(
                            room_id.to_owned(),
                            RoomOperatorViewUnavailableReasonV1::OperatorMembershipRequired,
                        ));
                    }
                    Err(_) => return Err(RoomOperatorViewErrorV1::Unavailable),
                }
            }
        };
        let operation = self.reconcile_unlocked(operation)?;
        self.view_for_operation_unlocked(&operation)
    }

    /// Reads a current safe browser value without implicitly creating a Member.
    ///
    /// # Errors
    ///
    /// Returns a closed request or protected-persistence error.
    pub fn view(
        &self,
        room_id: &str,
    ) -> Result<RoomOperatorViewResponseV1, RoomOperatorViewErrorV1> {
        validate_room_id(room_id)?;
        let _guard = self.lock();
        match self.load_unlocked(room_id) {
            Ok(operation) => self.view_for_operation_unlocked(&operation),
            Err(RoomOperatorViewErrorV1::InvalidRequest) => {
                Err(RoomOperatorViewErrorV1::Unavailable)
            }
            Err(RoomOperatorViewErrorV1::NotFound) => Ok(RoomOperatorViewResponseV1::unavailable(
                room_id.to_owned(),
                RoomOperatorViewUnavailableReasonV1::OperatorMembershipRequired,
            )),
            Err(RoomOperatorViewErrorV1::Unavailable) => Err(RoomOperatorViewErrorV1::Unavailable),
        }
    }

    fn new_operation(
        &self,
        binding: crate::room_creation::ReviewedOperatorMembershipBindingV1,
    ) -> Result<RoomOperatorViewOperationV1, RoomOperatorViewErrorV1> {
        let mut secret = Zeroizing::new([0_u8; 32]);
        getrandom::fill(secret.as_mut()).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let secret_reference = self
            .vault
            .store(SecretKindV1::MembershipAuthority, secret.as_ref())
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let operation = RoomOperatorViewOperationV1 {
            schema: OPERATION_SCHEMA_V1.to_owned(),
            room_id: binding.room_id,
            principal_id: binding.principal_id,
            member_id: binding.member_id,
            capability_id: next_ulid()?,
            change_id: next_ulid()?,
            secret_reference,
            state: OperationStateV1::Provisioning,
        };
        self.persist_new(&operation)?;
        Ok(operation)
    }

    fn reconcile_unlocked(
        &self,
        mut operation: RoomOperatorViewOperationV1,
    ) -> Result<RoomOperatorViewOperationV1, RoomOperatorViewErrorV1> {
        if operation.state == OperationStateV1::Provisioned {
            return Ok(operation);
        }
        let secret = self
            .vault
            .resolve(
                SecretKindV1::MembershipAuthority,
                &operation.secret_reference,
            )
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let bearer = SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bytes));
        let request = MemberCapabilityProvisionRequestV1 {
            room_id: operation.room_id.clone(),
            member_id: operation.member_id.clone(),
            principal_id: operation.principal_id.clone(),
            principal_kind: PrincipalKind::Human,
            role: None,
            access_mode: AccessMode::Operator,
            scopes: vec!["room:observe_member".to_owned()],
            capability: SealedCapabilityInputV1 {
                capability_id: operation.capability_id.clone(),
                capability_idempotency_key: operation.change_id.clone(),
                bearer,
            },
            expires_at: None,
        };
        match self.provisioner.provision_member(&request) {
            Ok(receipt) if receipt_is_bound(&operation, &receipt) => {
                operation.state = OperationStateV1::Provisioned;
                self.persist(&operation)?;
            }
            Ok(_) | Err(TaskSetupAttemptErrorV1::Ambiguous) => {
                operation.state = OperationStateV1::Retrying;
                self.persist(&operation)?;
            }
            Err(
                TaskSetupAttemptErrorV1::OperatorFixRequired | TaskSetupAttemptErrorV1::Rejected,
            ) => {
                operation.state = OperationStateV1::NeedsAttention;
                self.persist(&operation)?;
            }
        }
        Ok(operation)
    }

    fn view_for_operation_unlocked(
        &self,
        operation: &RoomOperatorViewOperationV1,
    ) -> Result<RoomOperatorViewResponseV1, RoomOperatorViewErrorV1> {
        if operation.state != OperationStateV1::Provisioned {
            return Ok(RoomOperatorViewResponseV1::unavailable(
                operation.room_id.clone(),
                RoomOperatorViewUnavailableReasonV1::OperatorViewUnavailable,
            ));
        }
        let secret = self
            .vault
            .resolve(
                SecretKindV1::MembershipAuthority,
                &operation.secret_reference,
            )
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let bearer = SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bytes));
        match self
            .projection
            .current_projection(&operation.room_id, &bearer)
        {
            Ok(response) => match counter_value(&operation.room_id, &response) {
                Ok((value, room_seq)) => Ok(RoomOperatorViewResponseV1::available(
                    operation.room_id.clone(),
                    value,
                    room_seq,
                )),
                Err(()) => Ok(RoomOperatorViewResponseV1::unavailable(
                    operation.room_id.clone(),
                    RoomOperatorViewUnavailableReasonV1::UnsupportedProjection,
                )),
            },
            Err(
                RoomOperatorProjectionErrorV1::Unavailable
                | RoomOperatorProjectionErrorV1::Rejected,
            ) => Ok(RoomOperatorViewResponseV1::unavailable(
                operation.room_id.clone(),
                RoomOperatorViewUnavailableReasonV1::OperatorViewUnavailable,
            )),
        }
    }

    fn load_unlocked(
        &self,
        room_id: &str,
    ) -> Result<RoomOperatorViewOperationV1, RoomOperatorViewErrorV1> {
        let path = self.operation_path(room_id);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RoomOperatorViewErrorV1::NotFound);
            }
            Err(_) => return Err(RoomOperatorViewErrorV1::Unavailable),
        }
        validate_owner_only_file(&path).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        if metadata.len() > MAX_OPERATION_BYTES as u64 {
            return Err(RoomOperatorViewErrorV1::Unavailable);
        }
        let operation = serde_json::from_slice::<RoomOperatorViewOperationV1>(
            &fs::read(path).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?,
        )
        .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        validate_operation(&operation, room_id)?;
        Ok(operation)
    }

    fn persist_new(
        &self,
        operation: &RoomOperatorViewOperationV1,
    ) -> Result<(), RoomOperatorViewErrorV1> {
        let (temporary, target) = self.write_temporary(operation)?;
        let result = fs::hard_link(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable);
        let _ = fs::remove_file(temporary);
        result
    }

    fn persist(
        &self,
        operation: &RoomOperatorViewOperationV1,
    ) -> Result<(), RoomOperatorViewErrorV1> {
        let (temporary, target) = self.write_temporary(operation)?;
        let result = fs::rename(&temporary, &target)
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| RoomOperatorViewErrorV1::Unavailable);
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn write_temporary(
        &self,
        operation: &RoomOperatorViewOperationV1,
    ) -> Result<(PathBuf, PathBuf), RoomOperatorViewErrorV1> {
        validate_operation(operation, &operation.room_id)?;
        let bytes =
            serde_json::to_vec(operation).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        if bytes.len() > MAX_OPERATION_BYTES {
            return Err(RoomOperatorViewErrorV1::Unavailable);
        }
        let temporary = self
            .root
            .join(format!(".{}.{}.tmp", operation.room_id, random_suffix()?));
        let mut file =
            create_owner_only_file(&temporary).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
        if file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(RoomOperatorViewErrorV1::Unavailable);
        }
        Ok((temporary, self.operation_path(&operation.room_id)))
    }

    fn operation_path(&self, room_id: &str) -> PathBuf {
        self.root.join(format!("{room_id}.json"))
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the explicit, strictly typed operator-view API.
pub fn room_operator_view_router(supervisor: RoomOperatorViewSupervisorV1) -> Router {
    Router::new()
        .route(
            "/api/v1/rooms/{room_id}/operator-view",
            get(operator_view).post(enable_operator_view),
        )
        .with_state(supervisor)
}

async fn operator_view(
    State(supervisor): State<RoomOperatorViewSupervisorV1>,
    AxumPath(room_id): AxumPath<String>,
) -> Result<Json<RoomOperatorViewResponseV1>, RoomOperatorViewErrorV1> {
    tokio::task::spawn_blocking(move || supervisor.view(&room_id))
        .await
        .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?
        .map(Json)
}

async fn enable_operator_view(
    State(supervisor): State<RoomOperatorViewSupervisorV1>,
    AxumPath(room_id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<RoomOperatorViewResponseV1>, RoomOperatorViewErrorV1> {
    if !is_json_content_type(&headers) {
        return Err(RoomOperatorViewErrorV1::InvalidRequest);
    }
    let request = serde_json::from_slice::<EnableRoomOperatorViewRequestV1>(&body)
        .map_err(|_| RoomOperatorViewErrorV1::InvalidRequest)?;
    tokio::task::spawn_blocking(move || supervisor.enable(&room_id, &request))
        .await
        .map_err(|_| RoomOperatorViewErrorV1::Unavailable)?
        .map(Json)
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

impl IntoResponse for RoomOperatorViewErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, retryable) = match self {
            Self::InvalidRequest => (
                StatusCode::BAD_REQUEST,
                "operator_view_request_invalid",
                false,
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "operator_view_not_found", false),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "operator_view_unavailable",
                true,
            ),
        };
        (
            status,
            Json(serde_json::json!({
                "error": { "code": code, "message": "operator view is unavailable", "retryable": retryable }
            })),
        ).into_response()
    }
}

fn receipt_is_bound(
    operation: &RoomOperatorViewOperationV1,
    receipt: &MemberCapabilityProvisionResponseV1,
) -> bool {
    receipt.capability_id == operation.capability_id
        && receipt.room_id == operation.room_id
        && receipt.member_id == operation.member_id
        && receipt.principal_id == operation.principal_id
        && receipt.scopes == ["room:observe_member"]
}

fn counter_value(room_id: &str, response: &ProjectionResponse) -> Result<(u8, u64), ()> {
    if response.room_id != room_id
        || response.room_head.room_id != room_id
        || response.projection_schema != COUNTER_PROJECTION_SCHEMA_V1
        || !response.projection.action_offers.is_empty()
    {
        return Err(());
    }
    let activity = response.projection.activity.as_object().ok_or(())?;
    if activity.len() != 1 {
        return Err(());
    }
    let value = activity
        .get("value")
        .and_then(serde_json::Value::as_u64)
        .ok_or(())?;
    let value = u8::try_from(value).map_err(|_| ())?;
    if value > 16 {
        return Err(());
    }
    Ok((value, response.room_head.room_seq))
}

fn validate_operation(
    operation: &RoomOperatorViewOperationV1,
    room_id: &str,
) -> Result<(), RoomOperatorViewErrorV1> {
    if operation.schema != OPERATION_SCHEMA_V1
        || operation.room_id != room_id
        || operation.room_id.parse::<UlidString>().is_err()
        || operation.principal_id.parse::<UlidString>().is_err()
        || operation.member_id.parse::<UlidString>().is_err()
        || operation.capability_id.parse::<UlidString>().is_err()
        || operation.change_id.parse::<UlidString>().is_err()
        || operation.secret_reference.as_str().is_empty()
    {
        return Err(RoomOperatorViewErrorV1::Unavailable);
    }
    Ok(())
}

fn validate_room_id(room_id: &str) -> Result<(), RoomOperatorViewErrorV1> {
    room_id
        .parse::<UlidString>()
        .map(|_| ())
        .map_err(|_| RoomOperatorViewErrorV1::InvalidRequest)
}

fn next_ulid() -> Result<String, RoomOperatorViewErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).map_err(|_| RoomOperatorViewErrorV1::Unavailable)
}

fn random_suffix() -> Result<String, RoomOperatorViewErrorV1> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| RoomOperatorViewErrorV1::Unavailable)?;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, &[u8]), RoomOperatorProjectionErrorV1> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(RoomOperatorProjectionErrorV1::Unavailable)?;
    let headers = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| RoomOperatorProjectionErrorV1::Unavailable)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(RoomOperatorProjectionErrorV1::Unavailable)?;
    Ok((status, &bytes[(separator + 4)..]))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::CONTENT_TYPE};
    use serde_json::json;
    use worldstream_protocol::{ActionOffer, Projection, RoomHead};

    use super::*;

    fn projection(activity: serde_json::Value) -> ProjectionResponse {
        ProjectionResponse {
            room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            room_head: RoomHead {
                room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
                room_seq: 7,
                genesis_or_transition_hash: "h".to_owned(),
                core_schema_version: "core.v1".to_owned(),
                pack_digest: "p".to_owned(),
                core_state_hash: "c".to_owned(),
                activity_state_hash: "a".to_owned(),
                authoritative_state_hash: "s".to_owned(),
            },
            room_health: "healthy".to_owned(),
            integrity_generation: 1,
            projection_schema: COUNTER_PROJECTION_SCHEMA_V1.to_owned(),
            projection: Projection {
                core: json!({"private_member_information_must_not_escape": true}),
                activity,
                action_offers: Vec::new(),
            },
            projection_hash: "hash".to_owned(),
        }
    }

    #[test]
    fn counter_view_releases_only_the_bounded_public_value() {
        let response = projection(json!({"value": 16}));
        assert_eq!(
            counter_value("01ARZ3NDEKTSV4RRFFQ69G5FAW", &response),
            Ok((16, 7))
        );
        let browser = serde_json::to_string(&RoomOperatorViewResponseV1::available(
            "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            16,
            7,
        ))
        .unwrap_or_else(|error| unreachable!("operator DTO JSON: {error}"));
        for forbidden in [
            "private_member_information",
            "action_offers",
            "member_id",
            "principal_id",
            "secret_reference",
            "bearer",
        ] {
            assert!(!browser.contains(forbidden));
        }
    }

    #[test]
    fn rejects_non_counter_or_member_projection_shapes() {
        let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
        let mut extra = projection(json!({"value": 2, "private": 3}));
        assert_eq!(counter_value(room_id, &extra), Err(()));

        extra = projection(json!({"value": 2}));
        extra.projection_schema = "agent-heist/projection/v1".to_owned();
        assert_eq!(counter_value(room_id, &extra), Err(()));

        extra = projection(json!({"value": 2}));
        extra.projection.action_offers.push(ActionOffer {
            domain: "worldstream/action-offer/v1".to_owned(),
            action_type: "act".to_owned(),
            payload_schema_digest: "private".to_owned(),
            eligibility_window: None,
        });
        assert_eq!(counter_value(room_id, &extra), Err(()));
    }

    #[test]
    fn receipt_must_be_bound_and_read_only() {
        let operation = RoomOperatorViewOperationV1 {
            schema: OPERATION_SCHEMA_V1.to_owned(),
            room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
            member_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
            capability_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
            change_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned(),
            secret_reference: SecretReferenceV1::parse("11".repeat(32))
                .unwrap_or_else(|error| unreachable!("reference: {error:?}")),
            state: OperationStateV1::Provisioning,
        };
        let receipt = MemberCapabilityProvisionResponseV1 {
            capability_id: operation.capability_id.clone(),
            room_id: operation.room_id.clone(),
            member_id: operation.member_id.clone(),
            principal_id: operation.principal_id.clone(),
            scopes: vec!["room:observe_member".to_owned()],
        };
        assert!(receipt_is_bound(&operation, &receipt));
        let mut overbroad = receipt;
        overbroad.scopes.push("room:act".to_owned());
        assert!(!receipt_is_bound(&operation, &overbroad));
    }

    #[test]
    fn enablement_requires_json_content_type() {
        let mut json = HeaderMap::new();
        json.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(is_json_content_type(&json));
        let mut text = HeaderMap::new();
        text.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(!is_json_content_type(&text));
    }
}
