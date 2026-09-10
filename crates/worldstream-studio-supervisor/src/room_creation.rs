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
    HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2, HOSTED_ROOM_CREATION_SCHEMA_V2,
    HostedRoomCreationRequestV2 as RuntimeHostedRoomCreationRequestV2,
    HostedSpectatorCredentialInputV2, MAX_MESSAGE_BYTES, PrincipalKind,
    ROOM_ARCHIVE_REQUEST_SCHEMA_V1, ROOM_ARCHIVE_RESPONSE_SCHEMA_V1, RoomArchiveRequestV1,
    RoomArchiveResponseV1, SealedCapabilityBearerV1, SealedCapabilityInputV1, UlidString,
};
pub use worldstream_protocol::{HostedRoomCreationResponseV2, HostedSpectatorCredentialReceiptV2};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::room_drafts::{RoomDraftReviewV1, RoomDraftStepV1, RoomDraftStoreV1, RoomDraftV1};
use crate::room_setup_spec::{SetupSpectatorPurposeV2, SetupSpectatorV2};
use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1, SecretVaultErrorV1};

const OPERATION_SCHEMA_V1: &str = "worldstream/studio-room-creation-operation/v1";
const RESPONSE_BINDING_DOMAIN_V1: &str = "worldstream/studio-room-creation-response/v1";
const MAX_OPERATION_BYTES: usize = 256 * 1024;
const MAX_OPERATIONS: usize = 256;

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
    /// Immutable non-seat v2 setup intent. Principal identities are allocated
    /// once before the first daemon mutation and survive every retry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectators: Vec<ReviewedSpectatorV2>,
    pub request: CreateRoomRequest,
    pub state: RoomCreationStateV1,
    pub attempts: u32,
    pub room_id: Option<String>,
    pub response: Option<CreateRoomResponse>,
    /// Secret-free proof that the hosted creator provisioned every required
    /// spectator credential in the same pre-Genesis operation as the Room.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectator_credentials: Vec<HostedSpectatorCredentialReceiptV2>,
    pub response_hash: Option<String>,
    pub attention: Option<RoomCreationAttentionV1>,
    /// CLI preparation guard. None is the retained pre-CLI shape; it cannot
    /// authorize recreating a missing provisioning operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_preparation_started: Option<bool>,
}

/// Retained Host interpretation of one bounded v2 non-seat spectator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedSpectatorV2 {
    pub purpose: SetupSpectatorPurposeV2,
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    pub capability_id: String,
    pub capability_idempotency_key: String,
    pub secret_reference: SecretReferenceV1,
}

/// One immutable server-owned credential requested as part of hosted Genesis.
/// The hosted creator generates and retains the bearer; this boundary carries
/// only stable identities and the exact non-action scopes it must bind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostedSpectatorCredentialIntentV2 {
    pub purpose: SetupSpectatorPurposeV2,
    pub member_index: u16,
    pub principal_id: String,
    pub principal_kind: PrincipalKind,
    pub capability_id: String,
    pub capability_idempotency_key: String,
    pub secret_reference: SecretReferenceV1,
}

/// Narrow all-or-nothing hosted Room creation request. Implementations must
/// persist the Room, Memberships, bearer hashes, and receipts atomically.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedHostedRoomCreationIntentV2 {
    pub schema: String,
    pub room: CreateRoomRequest,
    pub spectators: Vec<HostedSpectatorCredentialIntentV2>,
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

/// Internal binding for the one reviewed Genesis Operator Membership. It is
/// never serialized into browser status and is only consumed to provision its
/// read-only Capability later.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    clippy::struct_field_names,
    reason = "retained for generic operator clients; Studio no longer renders Pack projections"
)]
pub(crate) struct ReviewedOperatorMembershipBindingV1 {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
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

    /// Commits hosted Room Genesis and every required server-owned spectator
    /// credential atomically. The default deliberately refuses the operation:
    /// hosted v2 intent must never downgrade to ordinary Room creation.
    ///
    /// # Errors
    ///
    /// Returns only ambiguous, operator-fix, or permanent rejection classes.
    fn create_hosted(
        &self,
        _request: &RetainedHostedRoomCreationIntentV2,
    ) -> Result<HostedRoomCreationResponseV2, RoomCreationAttemptErrorV1> {
        Err(RoomCreationAttemptErrorV1::Rejected)
    }

    /// Archives one exact Room under the creator's retained Host authority.
    ///
    /// # Errors
    /// Returns only ambiguous, operator-fix, or permanent rejection classes.
    fn archive(
        &self,
        _room_id: &str,
        _request: &RoomArchiveRequestV1,
    ) -> Result<RoomArchiveResponseV1, RoomCreationAttemptErrorV1> {
        Err(RoomCreationAttemptErrorV1::Rejected)
    }
}

/// Fixed-address HTTP creator using one exact retained Host authority.
#[derive(Clone, Debug)]
pub struct HttpDaemonRoomCreatorV1 {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
    managed: Option<crate::managed_daemon_transport::ManagedDaemonTransport>,
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
            managed: None,
        }
    }

    /// Keeps exact creation intent on the Runtime socket proved before Host authority.
    #[must_use]
    pub fn new_managed(
        address: SocketAddr,
        timeout: Duration,
        vault: FileSecretVaultV1,
        host_authority: Option<SecretReferenceV1>,
        ownership: crate::process_ownership::ProcessOwnership,
    ) -> Self {
        Self {
            address,
            timeout,
            vault,
            host_authority,
            managed: Some(
                crate::managed_daemon_transport::ManagedDaemonTransport::new(
                    ownership, address, timeout,
                ),
            ),
        }
    }

    fn hosted_runtime_request(
        &self,
        request: &RetainedHostedRoomCreationIntentV2,
    ) -> Result<RuntimeHostedRoomCreationRequestV2, RoomCreationAttemptErrorV1> {
        let spectators = request
            .spectators
            .iter()
            .map(|spectator| {
                let bearer = match self.vault.resolve(
                    SecretKindV1::MembershipAuthority,
                    &spectator.secret_reference,
                ) {
                    Ok(secret) => {
                        let bytes: [u8; 32] = secret
                            .as_bytes()
                            .try_into()
                            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
                        SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bytes))
                    }
                    Err(SecretVaultErrorV1::Missing) => {
                        let mut bytes = Zeroizing::new([0_u8; 32]);
                        getrandom::fill(bytes.as_mut())
                            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
                        self.vault
                            .publish_at_reference(
                                SecretKindV1::MembershipAuthority,
                                &spectator.secret_reference,
                                bytes.as_ref(),
                            )
                            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
                        SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(*bytes))
                    }
                    Err(
                        SecretVaultErrorV1::InvalidMaterial
                        | SecretVaultErrorV1::InvalidReference
                        | SecretVaultErrorV1::Unavailable,
                    ) => return Err(RoomCreationAttemptErrorV1::OperatorFixRequired),
                };
                Ok(HostedSpectatorCredentialInputV2 {
                    purpose: spectator.purpose,
                    member_index: spectator.member_index,
                    principal_id: spectator.principal_id.clone(),
                    principal_kind: spectator.principal_kind,
                    capability: SealedCapabilityInputV1 {
                        capability_id: spectator.capability_id.clone(),
                        capability_idempotency_key: spectator.capability_idempotency_key.clone(),
                        bearer,
                    },
                })
            })
            .collect::<Result<Vec<_>, RoomCreationAttemptErrorV1>>()?;
        Ok(RuntimeHostedRoomCreationRequestV2 {
            schema: HOSTED_ROOM_CREATION_SCHEMA_V2.to_owned(),
            room: request.room.clone(),
            spectators,
        })
    }

    fn create_hosted_over_http(
        &self,
        request: &RuntimeHostedRoomCreationRequestV2,
    ) -> Result<HostedRoomCreationResponseV2, RoomCreationAttemptErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let body = Zeroizing::new(
            serde_json::to_vec(request).map_err(|_| RoomCreationAttemptErrorV1::Rejected)?,
        );
        if let Some(transport) = &self.managed {
            let mut authority_unavailable = false;
            let response = transport
                .request(
                    "POST",
                    "/v1/operator/hosted-rooms",
                    body.as_ref(),
                    MAX_MESSAGE_BYTES,
                    || {
                        let resolved = (|| {
                            let secret = self
                                .vault
                                .resolve(SecretKindV1::HostAuthority, reference)
                                .map_err(|_| ())?;
                            let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                            let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
                            let token = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
                            let mut header =
                                axum::http::HeaderValue::from_str(&token).map_err(|_| ())?;
                            header.set_sensitive(true);
                            Ok::<_, ()>(header)
                        })();
                        authority_unavailable = resolved.is_err();
                        resolved
                    },
                )
                .map_err(|_| {
                    if authority_unavailable {
                        RoomCreationAttemptErrorV1::OperatorFixRequired
                    } else {
                        RoomCreationAttemptErrorV1::Ambiguous
                    }
                })?;
            return match response.status {
                200 => serde_json::from_slice(&response.body)
                    .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous),
                401 | 403 => Err(RoomCreationAttemptErrorV1::OperatorFixRequired),
                400 | 404 | 409 | 422 => Err(RoomCreationAttemptErrorV1::Rejected),
                _ => Err(RoomCreationAttemptErrorV1::Ambiguous),
            };
        }

        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let header = Zeroizing::new(format!(
            "POST /v1/operator/hosted-rooms HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(body.as_ref()))
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

    fn archive_over_http(
        &self,
        room_id: &str,
        request: &RoomArchiveRequestV1,
    ) -> Result<RoomArchiveResponseV1, RoomCreationAttemptErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let room_id = room_id
            .parse::<UlidString>()
            .map_err(|_| RoomCreationAttemptErrorV1::Rejected)?;
        let path = format!("/v1/operator/rooms/{}/archive", room_id.as_str());
        let body = Zeroizing::new(
            serde_json::to_vec(request).map_err(|_| RoomCreationAttemptErrorV1::Rejected)?,
        );
        if let Some(transport) = &self.managed {
            let mut authority_unavailable = false;
            let response = transport
                .request("POST", &path, body.as_ref(), MAX_MESSAGE_BYTES, || {
                    let resolved = (|| {
                        let secret = self
                            .vault
                            .resolve(SecretKindV1::HostAuthority, reference)
                            .map_err(|_| ())?;
                        let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
                        let token = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
                        let mut header =
                            axum::http::HeaderValue::from_str(&token).map_err(|_| ())?;
                        header.set_sensitive(true);
                        Ok::<_, ()>(header)
                    })();
                    authority_unavailable = resolved.is_err();
                    resolved
                })
                .map_err(|_| {
                    if authority_unavailable {
                        RoomCreationAttemptErrorV1::OperatorFixRequired
                    } else {
                        RoomCreationAttemptErrorV1::Ambiguous
                    }
                })?;
            return match response.status {
                200 => serde_json::from_slice(&response.body)
                    .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous),
                401 | 403 => Err(RoomCreationAttemptErrorV1::OperatorFixRequired),
                400 | 404 | 409 | 422 => Err(RoomCreationAttemptErrorV1::Rejected),
                _ => Err(RoomCreationAttemptErrorV1::Ambiguous),
            };
        }

        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let header = Zeroizing::new(format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(body.as_ref()))
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

impl DaemonRoomCreatorV1 for HttpDaemonRoomCreatorV1 {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(RoomCreationAttemptErrorV1::OperatorFixRequired)?;
        if let Some(transport) = &self.managed {
            let body = Zeroizing::new(
                serde_json::to_vec(request).map_err(|_| RoomCreationAttemptErrorV1::Rejected)?,
            );
            let mut authority_unavailable = false;
            let response = transport
                .request("POST", "/v1/rooms", &body, MAX_MESSAGE_BYTES, || {
                    let resolved = (|| {
                        let secret = self
                            .vault
                            .resolve(SecretKindV1::HostAuthority, reference)
                            .map_err(|_| ())?;
                        let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
                        let token = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
                        let mut header =
                            axum::http::HeaderValue::from_str(&token).map_err(|_| ())?;
                        header.set_sensitive(true);
                        Ok::<_, ()>(header)
                    })();
                    authority_unavailable = resolved.is_err();
                    resolved
                })
                .map_err(|_| {
                    if authority_unavailable {
                        RoomCreationAttemptErrorV1::OperatorFixRequired
                    } else {
                        RoomCreationAttemptErrorV1::Ambiguous
                    }
                })?;
            return match response.status {
                200 => serde_json::from_slice(&response.body)
                    .map_err(|_| RoomCreationAttemptErrorV1::Ambiguous),
                401 | 403 => Err(RoomCreationAttemptErrorV1::OperatorFixRequired),
                400 | 404 | 409 | 422 => Err(RoomCreationAttemptErrorV1::Rejected),
                _ => Err(RoomCreationAttemptErrorV1::Ambiguous),
            };
        }
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

    fn create_hosted(
        &self,
        request: &RetainedHostedRoomCreationIntentV2,
    ) -> Result<HostedRoomCreationResponseV2, RoomCreationAttemptErrorV1> {
        self.create_hosted_over_http(&self.hosted_runtime_request(request)?)
    }

    fn archive(
        &self,
        room_id: &str,
        request: &RoomArchiveRequestV1,
    ) -> Result<RoomArchiveResponseV1, RoomCreationAttemptErrorV1> {
        if request.schema != ROOM_ARCHIVE_REQUEST_SCHEMA_V1 || request.validate_bounds().is_err() {
            return Err(RoomCreationAttemptErrorV1::Rejected);
        }
        let response = self.archive_over_http(room_id, request)?;
        if response.schema != ROOM_ARCHIVE_RESPONSE_SCHEMA_V1
            || response.room_id != room_id
            || response.room_head.room_id != room_id
        {
            return Err(RoomCreationAttemptErrorV1::Ambiguous);
        }
        Ok(response)
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
            .load_for_operation(draft_id)
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

    /// Retains a freshly resolved CLI review without reading an editable draft
    /// or calling the daemon. Existing operation identities are never replaced.
    pub(crate) fn prepare_reviewed(&self, draft: &RoomDraftV1) -> Result<(), RoomCreationErrorV1> {
        self.prepare_reviewed_with_spectators(draft, &[])
    }

    /// Retains one reviewed v2 Host setup including bounded non-seat
    /// spectators. This is the only path that can add them to Genesis.
    pub(crate) fn prepare_reviewed_with_spectators(
        &self,
        draft: &RoomDraftV1,
        spectators: &[SetupSpectatorV2],
    ) -> Result<(), RoomCreationErrorV1> {
        crate::room_drafts::validate_draft(draft).map_err(|_| RoomCreationErrorV1::InvalidDraft)?;
        let _guard = self.lock();
        match self.load_unlocked(&draft.draft_id) {
            Ok(_) => return Err(RoomCreationErrorV1::InvalidDraft),
            Err(RoomCreationErrorV1::NotFound) => {}
            Err(error) => return Err(error),
        }
        let mut operation = prepare_operation_with_spectators(draft, spectators)?;
        operation.setup_preparation_started = Some(false);
        self.persist_new(&operation)
    }

    /// Claims the single CLI setup preparation before it can allocate authority.
    /// A retained legacy record or an already claimed preparation cannot authorize
    /// replacement intent when the setup file is missing.
    pub(crate) fn begin_setup_preparation(
        &self,
        draft_id: &str,
    ) -> Result<bool, RoomCreationErrorV1> {
        let _guard = self.lock();
        let mut operation = self.load_unlocked(draft_id)?;
        if operation.state != RoomCreationStateV1::Succeeded {
            return Err(RoomCreationErrorV1::InvalidDraft);
        }
        if operation.setup_preparation_started != Some(false) {
            return Ok(false);
        }
        operation.setup_preparation_started = Some(true);
        self.persist(&operation)?;
        Ok(true)
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

    pub(crate) fn archive_room(
        &self,
        room_id: &str,
        idempotency_key: &str,
    ) -> Result<RoomArchiveResponseV1, RoomCreationAttemptErrorV1> {
        let request = RoomArchiveRequestV1 {
            schema: ROOM_ARCHIVE_REQUEST_SCHEMA_V1.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
        };
        request
            .validate_bounds()
            .map_err(|_| RoomCreationAttemptErrorV1::Rejected)?;
        self.creator.archive(room_id, &request)
    }

    /// Finds only an explicitly reviewed Genesis Operator Membership by its
    /// committed Room identity. No Host authority is converted into membership
    /// and no late Core mutation is attempted.
    #[allow(
        dead_code,
        reason = "retained for generic operator clients; Studio no longer renders Pack projections"
    )]
    pub(crate) fn reviewed_operator_membership(
        &self,
        room_id: &str,
    ) -> Result<Option<ReviewedOperatorMembershipBindingV1>, RoomCreationErrorV1> {
        let _guard = self.lock();
        let entries =
            fs::read_dir(self.root.as_ref()).map_err(|_| RoomCreationErrorV1::Unavailable)?;
        let mut found = None;
        for entry in entries {
            let path = entry.map_err(|_| RoomCreationErrorV1::Unavailable)?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let draft_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            let operation = self.load_unlocked(draft_id)?;
            if operation.room_id.as_deref() != Some(room_id) || !operation.review.operator_view {
                continue;
            }
            let response = operation
                .response
                .as_ref()
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            let index = operation
                .request
                .members
                .iter()
                .position(|member| {
                    member.access_mode == AccessMode::Operator
                        && member.principal_kind == PrincipalKind::Human
                        && member.role.is_none()
                })
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            let member = operation
                .request
                .members
                .get(index)
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            let member_id = response
                .member_ids
                .get(index)
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            let binding = ReviewedOperatorMembershipBindingV1 {
                room_id: room_id.to_owned(),
                member_id: member_id.clone(),
                principal_id: member.principal_id.clone(),
            };
            if found.replace(binding).is_some() {
                return Err(RoomCreationErrorV1::Unavailable);
            }
        }
        Ok(found)
    }

    /// Lists every retained browser-safe creation status in stable draft order.
    ///
    /// # Errors
    ///
    /// Fails closed for malformed, unexpected, excessive, or unavailable
    /// protected operation records.
    pub fn statuses(&self) -> Result<Vec<RoomCreationStatusV1>, RoomCreationErrorV1> {
        let _guard = self.lock();
        let mut draft_ids = Vec::new();
        for entry in
            fs::read_dir(self.root.as_ref()).map_err(|_| RoomCreationErrorV1::Unavailable)?
        {
            let path = entry.map_err(|_| RoomCreationErrorV1::Unavailable)?.path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            if name.starts_with('.')
                && Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
            {
                continue;
            }
            let draft_id = name
                .strip_suffix(".json")
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            validate_draft_id(draft_id)?;
            draft_ids.push(draft_id.to_owned());
            if draft_ids.len() > MAX_OPERATIONS {
                return Err(RoomCreationErrorV1::Unavailable);
            }
        }
        draft_ids.sort();
        if draft_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(RoomCreationErrorV1::Unavailable);
        }
        draft_ids
            .into_iter()
            .map(|draft_id| {
                self.load_unlocked(&draft_id)
                    .map(RoomCreationStatusV1::from)
            })
            .collect()
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
        let attempt = if operation.spectators.is_empty() {
            self.creator
                .create(&operation.request)
                .and_then(|response| {
                    response_is_bound(&operation.request, &response)
                        .then_some((response, Vec::new()))
                        .ok_or(RoomCreationAttemptErrorV1::Ambiguous)
                })
        } else {
            let request = hosted_creation_request(&operation)?;
            self.creator.create_hosted(&request).and_then(|response| {
                hosted_response_is_bound(&request, &response)
                    .then_some((response.room, response.spectators))
                    .ok_or(RoomCreationAttemptErrorV1::Ambiguous)
            })
        };
        match attempt {
            Ok((response, spectator_credentials)) => {
                #[cfg(test)]
                if self.fail_before_receipt_once.swap(false, Ordering::SeqCst) {
                    operation.state = RoomCreationStateV1::Retrying;
                    operation.attention = Some(ambiguous_attention());
                    self.persist(&operation)?;
                    return Ok(operation);
                }
                operation.state = RoomCreationStateV1::Succeeded;
                operation.room_id = Some(response.room_id.clone());
                operation.response_hash = Some(response_hash(
                    &operation.intent_hash,
                    &response,
                    &spectator_credentials,
                )?);
                operation.response = Some(response);
                operation.spectator_credentials = spectator_credentials;
                operation.attention = None;
                self.persist(&operation)?;
                Ok(operation)
            }
            Err(RoomCreationAttemptErrorV1::Ambiguous) => {
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
            "/api/v1/room-creations/{draft_id}/start",
            post(start_creation),
        )
        .route(
            "/api/v1/room-creations/{draft_id}/retry",
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
    prepare_operation_with_spectators(draft, &[])
}

fn prepare_operation_with_spectators(
    draft: &RoomDraftV1,
    requested_spectators: &[SetupSpectatorV2],
) -> Result<RoomCreationOperationV1, RoomCreationErrorV1> {
    if draft.last_valid_step != Some(RoomDraftStepV1::Review) {
        return Err(RoomCreationErrorV1::InvalidDraft);
    }
    let pack = draft
        .pack
        .clone()
        .ok_or(RoomCreationErrorV1::InvalidDraft)?;
    let mut members = draft
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
    let spectators = requested_spectators
        .iter()
        .map(|spectator| {
            let capability_id = next_ulid()?;
            Ok(ReviewedSpectatorV2 {
                purpose: spectator.purpose,
                principal_id: next_ulid()?,
                principal_kind: spectator.principal.kind,
                secret_reference: spectator_secret_reference(&capability_id)?,
                capability_id,
                capability_idempotency_key: next_ulid()?,
            })
        })
        .collect::<Result<Vec<_>, RoomCreationErrorV1>>()?;
    members.extend(spectators.iter().map(|spectator| CreateMember {
        principal_id: spectator.principal_id.clone(),
        principal_kind: spectator.principal_kind,
        role: None,
        access_mode: AccessMode::Spectator,
    }));
    if draft.operator_view {
        members.push(CreateMember {
            principal_id: next_ulid()?,
            principal_kind: PrincipalKind::Human,
            role: None,
            access_mode: AccessMode::Operator,
        });
    }
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
        operator_view: draft.operator_view,
    };
    let intent = creation_intent_bytes(&request, &spectators)
        .map_err(|_| RoomCreationErrorV1::InvalidDraft)?;
    Ok(RoomCreationOperationV1 {
        schema: OPERATION_SCHEMA_V1.to_owned(),
        draft_id: draft.draft_id.clone(),
        operation_id,
        idempotency_key,
        review_hash: reviewed_intent_hash(&review, &spectators)?,
        intent_hash: digest(&intent),
        review,
        spectators,
        request,
        state: RoomCreationStateV1::Waiting,
        attempts: 0,
        room_id: None,
        response: None,
        spectator_credentials: Vec::new(),
        response_hash: None,
        attention: None,
        setup_preparation_started: None,
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
        || digest(&creation_intent_bytes(
            &operation.request,
            &operation.spectators,
        )?) != operation.intent_hash
        || reviewed_intent_hash(&operation.review, &operation.spectators)? != operation.review_hash
        || operation.intent_hash.len() != 71
        || !request_matches_review(&operation.request, &operation.review, &operation.spectators)
        || !operation_state_is_consistent(operation)
        || !spectators_are_valid(&operation.spectators)
        || (operation.setup_preparation_started == Some(true)
            && operation.state != RoomCreationStateV1::Succeeded)
    {
        return Err(RoomCreationErrorV1::Unavailable);
    }
    Ok(())
}

fn reviewed_intent_hash(
    review: &RoomDraftReviewV1,
    spectators: &[ReviewedSpectatorV2],
) -> Result<String, RoomCreationErrorV1> {
    let bytes = if spectators.is_empty() {
        serde_json::to_vec(review)
    } else {
        serde_json::to_vec(&(review, spectators))
    }
    .map_err(|_| RoomCreationErrorV1::Unavailable)?;
    Ok(digest(&bytes))
}

fn spectators_are_valid(spectators: &[ReviewedSpectatorV2]) -> bool {
    if spectators.len() > 3 {
        return false;
    }
    let mut purposes = std::collections::BTreeSet::new();
    let mut principals = std::collections::BTreeSet::new();
    let mut capabilities = std::collections::BTreeSet::new();
    let mut capability_changes = std::collections::BTreeSet::new();
    let all_valid = spectators.iter().all(|spectator| {
        let expected_kind = spectator.purpose.principal_kind();
        spectator.principal_kind == expected_kind
            && spectator.principal_id.parse::<UlidString>().is_ok()
            && spectator.capability_id.parse::<UlidString>().is_ok()
            && spectator
                .capability_idempotency_key
                .parse::<UlidString>()
                .is_ok()
            && spectator.capability_id != spectator.capability_idempotency_key
            && spectator_secret_reference(&spectator.capability_id)
                .is_ok_and(|expected| expected == spectator.secret_reference)
            && purposes.insert(spectator.purpose)
            && principals.insert(spectator.principal_id.as_str())
            && capabilities.insert(spectator.capability_id.as_str())
            && capability_changes.insert(spectator.capability_idempotency_key.as_str())
    });
    all_valid
        && (spectators.is_empty() || purposes.contains(&SetupSpectatorPurposeV2::ResultIndexer))
}

fn request_matches_review(
    request: &CreateRoomRequest,
    review: &RoomDraftReviewV1,
    spectators: &[ReviewedSpectatorV2],
) -> bool {
    let mut expected_members = review
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
    expected_members.extend(spectators.iter().map(|spectator| CreateMember {
        principal_id: spectator.principal_id.clone(),
        principal_kind: spectator.principal_kind,
        role: None,
        access_mode: AccessMode::Spectator,
    }));
    if review.operator_view {
        let Some(operator) = request.members.last() else {
            return false;
        };
        if operator.principal_kind != PrincipalKind::Human
            || operator.access_mode != AccessMode::Operator
            || operator.role.is_some()
            || operator.principal_id.parse::<UlidString>().is_err()
        {
            return false;
        }
        expected_members.push(operator.clone());
    }
    review.pack.as_ref() == Some(&request.pack)
        && review.configuration == request.configuration
        && expected_members == request.members
}

fn creation_intent_bytes(
    request: &CreateRoomRequest,
    spectators: &[ReviewedSpectatorV2],
) -> Result<Vec<u8>, RoomCreationErrorV1> {
    if spectators.is_empty() {
        serde_json::to_vec(request).map_err(|_| RoomCreationErrorV1::Unavailable)
    } else {
        serde_json::to_vec(&hosted_creation_request_parts(request, spectators)?)
            .map_err(|_| RoomCreationErrorV1::Unavailable)
    }
}

fn hosted_creation_request(
    operation: &RoomCreationOperationV1,
) -> Result<RetainedHostedRoomCreationIntentV2, RoomCreationErrorV1> {
    hosted_creation_request_parts(&operation.request, &operation.spectators)
}

fn hosted_creation_request_parts(
    request: &CreateRoomRequest,
    spectators: &[ReviewedSpectatorV2],
) -> Result<RetainedHostedRoomCreationIntentV2, RoomCreationErrorV1> {
    let intents = spectators
        .iter()
        .map(|spectator| {
            let member_index = request
                .members
                .iter()
                .position(|member| {
                    member.principal_id == spectator.principal_id
                        && member.principal_kind == spectator.principal_kind
                        && member.access_mode == AccessMode::Spectator
                        && member.role.is_none()
                })
                .and_then(|index| u16::try_from(index).ok())
                .ok_or(RoomCreationErrorV1::Unavailable)?;
            Ok(HostedSpectatorCredentialIntentV2 {
                purpose: spectator.purpose,
                member_index,
                principal_id: spectator.principal_id.clone(),
                principal_kind: spectator.principal_kind,
                capability_id: spectator.capability_id.clone(),
                capability_idempotency_key: spectator.capability_idempotency_key.clone(),
                secret_reference: spectator.secret_reference.clone(),
            })
        })
        .collect::<Result<Vec<_>, RoomCreationErrorV1>>()?;
    Ok(RetainedHostedRoomCreationIntentV2 {
        schema: HOSTED_ROOM_CREATION_SCHEMA_V2.to_owned(),
        room: request.clone(),
        spectators: intents,
    })
}

fn hosted_response_is_bound(
    request: &RetainedHostedRoomCreationIntentV2,
    response: &HostedRoomCreationResponseV2,
) -> bool {
    request.schema == HOSTED_ROOM_CREATION_SCHEMA_V2
        && response.schema == HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2
        && response_is_bound(&request.room, &response.room)
        && request.spectators.len() == response.spectators.len()
        && request
            .spectators
            .iter()
            .zip(&response.spectators)
            .all(|(intent, receipt)| {
                response
                    .room
                    .member_ids
                    .get(usize::from(intent.member_index))
                    == Some(&receipt.member_id)
                    && receipt.purpose == intent.purpose
                    && receipt.room_id == response.room.room_id
                    && receipt.principal_id == intent.principal_id
                    && receipt.capability_id == intent.capability_id
                    && receipt.scopes.iter().map(String::as_str).eq(intent
                        .purpose
                        .capability_scopes()
                        .iter()
                        .copied())
            })
}

fn spectator_secret_reference(
    capability_id: &str,
) -> Result<SecretReferenceV1, RoomCreationErrorV1> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/hosted-spectator-secret-reference/v2\0");
    hasher.update(capability_id.as_bytes());
    SecretReferenceV1::parse(hasher.finalize().to_hex().to_string())
        .map_err(|_| RoomCreationErrorV1::Unavailable)
}

fn hosted_receipts_match_operation(operation: &RoomCreationOperationV1) -> bool {
    if operation.spectators.is_empty() {
        return operation.spectator_credentials.is_empty();
    }
    let (Ok(request), Some(room)) = (
        hosted_creation_request(operation),
        operation.response.clone(),
    ) else {
        return false;
    };
    hosted_response_is_bound(
        &request,
        &HostedRoomCreationResponseV2 {
            schema: HOSTED_ROOM_CREATION_RESPONSE_SCHEMA_V2.to_owned(),
            room,
            spectators: operation.spectator_credentials.clone(),
        },
    )
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
                            && hosted_receipts_match_operation(operation)
                            && response_hash(
                                &operation.intent_hash,
                                response,
                                &operation.spectator_credentials,
                            )
                            .is_ok_and(|expected| &expected == stored_hash)
                    })
        }
        RoomCreationStateV1::Waiting => {
            operation.room_id.is_none()
                && operation.response.is_none()
                && operation.spectator_credentials.is_empty()
                && operation.response_hash.is_none()
                && operation.attention.is_none()
        }
        RoomCreationStateV1::Retrying => {
            operation.room_id.is_none()
                && operation.response.is_none()
                && operation.spectator_credentials.is_empty()
                && operation.response_hash.is_none()
                && operation.attention.as_ref() == Some(&ambiguous_attention())
        }
        RoomCreationStateV1::NeedsAttention => {
            operation.room_id.is_none()
                && operation.response.is_none()
                && operation.spectator_credentials.is_empty()
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
    spectator_credentials: &[HostedSpectatorCredentialReceiptV2],
) -> Result<String, RoomCreationErrorV1> {
    let binding = if spectator_credentials.is_empty() {
        serde_json::to_vec(&(RESPONSE_BINDING_DOMAIN_V1, intent_hash, response))
    } else {
        serde_json::to_vec(&(
            RESPONSE_BINDING_DOMAIN_V1,
            intent_hash,
            response,
            spectator_credentials,
        ))
    }
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

pub(crate) fn next_ulid() -> Result<String, RoomCreationErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RoomCreationErrorV1::Unavailable)?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).map_err(|_| RoomCreationErrorV1::Unavailable)
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
