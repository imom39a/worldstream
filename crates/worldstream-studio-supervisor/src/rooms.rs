//! Privacy-bounded forwarding of host-authorized Room diagnostics to Studio.

use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize};
use worldstream_protocol::{
    BearerWireV1, OperatorActivityPhase, OperatorDataFreshness, OperatorRoomIntegrity,
    OperatorRoomInventoryPage, OperatorRoomSummary, PackReference, RoomHead,
};
use zeroize::Zeroizing;

use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const INVENTORY_SCHEMA_V1: &str = "worldstream/studio-room-inventory/v1";
const MAX_STUDIO_ROOM_PAGE_SIZE: usize = 50;
const MAX_DAEMON_RESPONSE_BYTES: u64 = 256 * 1024;

/// Setup progress remains independent from integrity and Activity Phase.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RoomSetupProgressV1 {
    /// Durable Genesis, complete Head, and exact pack lock are present.
    Complete {
        completed_steps: u8,
        total_steps: u8,
    },
    /// A future Supervisor-owned setup workflow has not crossed all barriers.
    PartiallyProvisioned {
        completed_steps: u8,
        total_steps: u8,
        reason: String,
    },
    /// No truthful setup observation is available.
    Unavailable { reason: String },
}

/// Participant readiness needs an authorized operator Membership projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ParticipantReadinessV1 {
    /// Authorized aggregate readiness with no participant-private detail.
    Available { ready: u32, total: u32 },
    /// Host diagnostics cannot inspect participant-private readiness.
    Unavailable { reason: String },
}

/// Studio-safe Room row/detail with no Membership, seat, or Invocation data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StudioRoomSummaryV1 {
    pub room_id: String,
    pub room_head: RoomHead,
    pub pack: PackReference,
    pub setup_progress: RoomSetupProgressV1,
    pub participant_readiness: ParticipantReadinessV1,
    pub integrity: OperatorRoomIntegrity,
    pub activity_phase: OperatorActivityPhase,
    pub freshness: OperatorDataFreshness,
}

impl From<OperatorRoomSummary> for StudioRoomSummaryV1 {
    fn from(room: OperatorRoomSummary) -> Self {
        Self {
            room_id: room.room_id,
            room_head: room.room_head,
            pack: room.pack,
            setup_progress: RoomSetupProgressV1::Unavailable {
                reason: "supervisor_setup_state_required".to_owned(),
            },
            participant_readiness: ParticipantReadinessV1::Unavailable {
                reason: "operator_membership_required".to_owned(),
            },
            integrity: room.integrity,
            activity_phase: room.activity_phase,
            freshness: room.freshness,
        }
    }
}

/// Bounded, stable-keyset page returned to Studio.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StudioRoomInventoryPageV1 {
    pub schema: String,
    pub rooms: Vec<StudioRoomSummaryV1>,
    pub next_after_room_id: Option<String>,
}

/// Closed proxy failures; daemon bodies never cross the browser boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomSourceErrorV1 {
    NotFound,
    Forbidden,
    InvalidResponse,
    Unavailable,
}

/// Narrow read-only capability injected into the Supervisor router.
pub trait DaemonRoomSource: Send + Sync + 'static {
    /// Returns one bounded stable-keyset Room inventory page.
    ///
    /// # Errors
    ///
    /// Returns a closed failure when authority, transport, or response
    /// validation prevents a safe page from being produced.
    fn inventory(
        &self,
        after_room_id: Option<&str>,
        limit: usize,
    ) -> Result<OperatorRoomInventoryPage, RoomSourceErrorV1>;
    /// Returns one Room by stable public identity.
    ///
    /// # Errors
    ///
    /// Returns a closed failure when the Room is absent or a safe authorized
    /// response cannot be produced.
    fn detail(&self, room_id: &str) -> Result<OperatorRoomSummary, RoomSourceErrorV1>;
}

/// Live daemon source using the configured owner-only bootstrap authority.
#[derive(Clone)]
pub struct HttpDaemonRoomSource {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
    managed: Option<crate::managed_daemon_transport::ManagedDaemonTransport>,
}

impl HttpDaemonRoomSource {
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

    /// Uses proof-bound managed Runtime transport; foreground `new` is unchanged.
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

    fn request<T: for<'de> serde::Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, RoomSourceErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(RoomSourceErrorV1::Unavailable)?;
        if let Some(transport) = &self.managed {
            let maximum = usize::try_from(MAX_DAEMON_RESPONSE_BYTES)
                .map_err(|_| RoomSourceErrorV1::InvalidResponse)?;
            let response = transport
                .request("GET", path, b"", maximum, || {
                    let secret = self
                        .vault
                        .resolve(SecretKindV1::HostAuthority, reference)
                        .map_err(|_| ())?;
                    let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                    let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
                    let token = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
                    let mut header = axum::http::HeaderValue::from_str(&token).map_err(|_| ())?;
                    header.set_sensitive(true);
                    Ok::<_, ()>(header)
                })
                .map_err(|error| match error {
                    crate::verified_control::ControlTransportError::Protocol => {
                        RoomSourceErrorV1::InvalidResponse
                    }
                    _ => RoomSourceErrorV1::Unavailable,
                })?;
            return match response.status {
                200 => serde_json::from_slice(&response.body)
                    .map_err(|_| RoomSourceErrorV1::InvalidResponse),
                401 | 403 => Err(RoomSourceErrorV1::Forbidden),
                404 => Err(RoomSourceErrorV1::NotFound),
                500..=599 => Err(RoomSourceErrorV1::Unavailable),
                _ => Err(RoomSourceErrorV1::InvalidResponse),
            };
        }
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| RoomSourceErrorV1::Unavailable)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| RoomSourceErrorV1::Unavailable)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| RoomSourceErrorV1::Unavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| RoomSourceErrorV1::Unavailable)?;
        let request = Zeroizing::new(format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            self.address,
            bearer.as_str()
        ));
        stream
            .write_all(request.as_bytes())
            .map_err(|_| RoomSourceErrorV1::Unavailable)?;
        let mut bytes = Vec::new();
        stream
            .take(MAX_DAEMON_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| RoomSourceErrorV1::Unavailable)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_DAEMON_RESPONSE_BYTES {
            return Err(RoomSourceErrorV1::InvalidResponse);
        }
        let (status, body) = parse_http_response(&bytes)?;
        match status {
            200 => serde_json::from_slice(body).map_err(|_| RoomSourceErrorV1::InvalidResponse),
            401 | 403 => Err(RoomSourceErrorV1::Forbidden),
            404 => Err(RoomSourceErrorV1::NotFound),
            500..=599 => Err(RoomSourceErrorV1::Unavailable),
            _ => Err(RoomSourceErrorV1::InvalidResponse),
        }
    }
}

impl DaemonRoomSource for HttpDaemonRoomSource {
    fn inventory(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<OperatorRoomInventoryPage, RoomSourceErrorV1> {
        let path = after.map_or_else(
            || format!("/v1/operator/rooms?limit={limit}"),
            |cursor| format!("/v1/operator/rooms?after_room_id={cursor}&limit={limit}"),
        );
        self.request(&path)
    }
    fn detail(&self, room_id: &str) -> Result<OperatorRoomSummary, RoomSourceErrorV1> {
        self.request(&format!("/v1/operator/rooms/{room_id}"))
    }
}

/// Builds the read-only Studio Room inventory/detail surface.
pub fn room_router(source: impl DaemonRoomSource) -> Router {
    Router::new()
        .route("/api/v1/rooms", get(room_inventory))
        .route("/api/v1/rooms/{room_id}", get(room_detail))
        .with_state(Arc::new(source) as Arc<dyn DaemonRoomSource>)
}

async fn room_inventory(
    State(source): State<Arc<dyn DaemonRoomSource>>,
    RawQuery(raw): RawQuery,
) -> Result<Json<StudioRoomInventoryPageV1>, StatusCode> {
    let (after, limit) = parse_inventory_query(raw.as_deref())?;
    let page = tokio::task::spawn_blocking(move || source.inventory(after.as_deref(), limit))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map_err(status_for_error)?;
    Ok(Json(StudioRoomInventoryPageV1 {
        schema: INVENTORY_SCHEMA_V1.to_owned(),
        rooms: page.rooms.into_iter().map(Into::into).collect(),
        next_after_room_id: page.next_after_room_id,
    }))
}

async fn room_detail(
    State(source): State<Arc<dyn DaemonRoomSource>>,
    Path(room_id): Path<String>,
) -> Result<Json<StudioRoomSummaryV1>, StatusCode> {
    if !is_stable_room_id(&room_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let room = tokio::task::spawn_blocking(move || source.detail(&room_id))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map_err(status_for_error)?;
    Ok(Json(room.into()))
}

fn parse_inventory_query(raw: Option<&str>) -> Result<(Option<String>, usize), StatusCode> {
    let (mut cursor, mut limit) = (None, None);
    for part in raw
        .unwrap_or_default()
        .split('&')
        .filter(|part| !part.is_empty())
    {
        let (name, value) = part.split_once('=').ok_or(StatusCode::BAD_REQUEST)?;
        match name {
            "after_room_id" if cursor.is_none() && is_stable_room_id(value) => {
                cursor = Some(value.to_owned());
            }
            "limit" if limit.is_none() => {
                let value = value
                    .parse::<usize>()
                    .map_err(|_| StatusCode::BAD_REQUEST)?;
                if value == 0 || value > MAX_STUDIO_ROOM_PAGE_SIZE {
                    return Err(StatusCode::BAD_REQUEST);
                }
                limit = Some(value);
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    }
    Ok((cursor, limit.unwrap_or(MAX_STUDIO_ROOM_PAGE_SIZE)))
}

fn status_for_error(error: RoomSourceErrorV1) -> StatusCode {
    match error {
        RoomSourceErrorV1::NotFound => StatusCode::NOT_FOUND,
        RoomSourceErrorV1::Forbidden => StatusCode::FORBIDDEN,
        RoomSourceErrorV1::InvalidResponse => StatusCode::BAD_GATEWAY,
        RoomSourceErrorV1::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
fn is_stable_room_id(value: &str) -> bool {
    value.len() == 26 && value.bytes().all(|byte| {
        byte.is_ascii_digit()
            || matches!(byte, b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z')
    })
}
fn parse_http_response(bytes: &[u8]) -> Result<(u16, &[u8]), RoomSourceErrorV1> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(RoomSourceErrorV1::InvalidResponse)?;
    let headers =
        std::str::from_utf8(&bytes[..split]).map_err(|_| RoomSourceErrorV1::InvalidResponse)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .ok_or(RoomSourceErrorV1::InvalidResponse)?;
    Ok((status, &bytes[(split + 4)..]))
}

#[cfg(test)]
mod tests {
    use super::{DaemonRoomSource, RoomSourceErrorV1, room_router};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    use worldstream_protocol::{
        OperatorActivityPhase, OperatorDataFreshness, OperatorRoomIntegrity,
        OperatorRoomIntegrityStatus, OperatorRoomInventoryPage, OperatorRoomSummary, PackReference,
        RoomHead,
    };

    #[derive(Clone)]
    struct FixedRooms;
    impl DaemonRoomSource for FixedRooms {
        fn inventory(
            &self,
            after: Option<&str>,
            limit: usize,
        ) -> Result<OperatorRoomInventoryPage, RoomSourceErrorV1> {
            assert_eq!(after, None);
            assert_eq!(limit, 2);
            Ok(OperatorRoomInventoryPage {
                rooms: vec![room("01ARZ3NDEKTSV4RRFFQ69G5FQ0")],
                next_after_room_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FQ0".to_owned()),
            })
        }
        fn detail(&self, room_id: &str) -> Result<OperatorRoomSummary, RoomSourceErrorV1> {
            Ok(room(room_id))
        }
    }

    #[derive(Clone)]
    struct UnavailableRooms;
    impl DaemonRoomSource for UnavailableRooms {
        fn inventory(
            &self,
            _after: Option<&str>,
            _limit: usize,
        ) -> Result<OperatorRoomInventoryPage, RoomSourceErrorV1> {
            Err(RoomSourceErrorV1::Unavailable)
        }

        fn detail(&self, _room_id: &str) -> Result<OperatorRoomSummary, RoomSourceErrorV1> {
            Err(RoomSourceErrorV1::Unavailable)
        }
    }
    fn room(id: &str) -> OperatorRoomSummary {
        OperatorRoomSummary {
            room_id: id.to_owned(),
            room_head: RoomHead {
                room_id: id.to_owned(),
                room_seq: 3,
                genesis_or_transition_hash: "transition".to_owned(),
                core_schema_version: "core.v1".to_owned(),
                pack_digest: "pack-digest".to_owned(),
                core_state_hash: "core".to_owned(),
                activity_state_hash: "activity".to_owned(),
                authoritative_state_hash: "authoritative".to_owned(),
            },
            pack: PackReference {
                id: "counter".to_owned(),
                version: "1.0.0".to_owned(),
                digest: "pack-digest".to_owned(),
            },
            integrity: OperatorRoomIntegrity {
                status: OperatorRoomIntegrityStatus::Healthy,
                generation: 2,
            },
            activity_phase: OperatorActivityPhase::Unavailable {
                reason: "operator_membership_required".to_owned(),
            },
            freshness: OperatorDataFreshness::Fresh {
                observed_at: "2026-08-23T12:00:00Z".to_owned(),
            },
        }
    }

    #[tokio::test]
    async fn forwards_bounded_inventory_with_independent_non_private_axes() {
        let response = room_router(FixedRooms)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rooms?limit=2")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("inventory request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("inventory response: {error}"));
        assert_eq!(response.status(), 200);
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("inventory body: {error}"))
            .to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| unreachable!("inventory json: {error}"));
        assert_eq!(json["rooms"][0]["setup_progress"]["status"], "unavailable");
        assert_eq!(
            json["rooms"][0]["setup_progress"]["reason"],
            "supervisor_setup_state_required"
        );
        assert_eq!(
            json["rooms"][0]["participant_readiness"]["status"],
            "unavailable"
        );
        assert_eq!(json["rooms"][0]["activity_phase"]["status"], "unavailable");
        assert_eq!(json["rooms"][0]["integrity"]["generation"], 2);
        assert_eq!(json["rooms"][0]["freshness"]["status"], "fresh");
        assert!(json["rooms"][0].get("members").is_none());
        assert!(json["rooms"][0].get("invocation").is_none());
    }

    #[tokio::test]
    async fn detail_uses_stable_identity_and_rejects_ambiguous_queries() {
        let detail = room_router(FixedRooms)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rooms/01ARZ3NDEKTSV4RRFFQ69G5FQ0")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("detail request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("detail response: {error}"));
        assert_eq!(detail.status(), 200);
        let invalid = room_router(FixedRooms)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rooms?limit=2&limit=3")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("invalid request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("invalid response: {error}"));
        assert_eq!(invalid.status(), 400);
    }

    #[tokio::test]
    async fn unavailable_daemon_does_not_synthesize_room_state() {
        let response = room_router(UnavailableRooms)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rooms?limit=2")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(response.status(), 503);
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        assert!(bytes.is_empty());
    }
}
