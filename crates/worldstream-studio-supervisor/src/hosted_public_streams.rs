//! Durable post-Genesis public Run bindings and one-use Runtime admission.
//!
//! The platform supplies only secret-free Genesis correspondence. This module
//! revalidates it against retained Host state, keeps Membership authority in
//! the local vault, and returns only a short-lived ticket to the colocated
//! Hosted Gateway.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Request, State},
    http::{Method, StatusCode},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedBrowserStreamTicketResponseV1, HostedPublicRelayBindReceiptV1,
    HostedPublicRelayBindRequestV1, HostedPublicStreamTicketRequestV1,
    validate_hosted_browser_stream_ticket_response, validate_hosted_public_relay_bind_receipt,
    validate_hosted_public_relay_bind_request, validate_hosted_public_stream_ticket_request,
};
use worldstream_protocol::{
    AccessMode, BearerWireV1, ClientMode, HOSTED_BROWSER_WS_TICKET_VERSION,
    HostedBrowserWebSocketTicketIssueRequest, PackReference, SealedCapabilityBearerV1,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use crate::{
    hosted_launch::{HostedLaunchAccessV1, HostedLaunchErrorV1, HostedLaunchOperationsV1},
    participant_handoff::{
        CurrentMembershipSnapshotV1, HumanSeatAuthorityV1, ParticipantConsoleGatewayErrorV1,
        ParticipantConsoleGatewayV1,
    },
    room_setup_operations::RoomSetupPublicRelayBindingV1,
    secrets::{FileSecretVaultV1, SecretAvailabilityV1, SecretKindV1},
};

const BINDING_SCHEMA_V1: &str = "worldstream/hosted-public-relay-binding/v1";
const MAX_BINDING_BYTES: usize = 32 * 1024;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_BINDINGS: usize = 256;
const PUBLIC_CLIENT_SURFACE_ID: &str = "worldstream.public-projection-relay";
const PUBLIC_CLIENT_RELEASE: &[u8] = b"worldstream/public-projection-relay/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedPublicStreamErrorV1 {
    Invalid,
    Conflict,
    NotFound,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedPublicRelayBindingV1 {
    schema: String,
    request: HostedPublicRelayBindRequestV1,
    request_digest: String,
}

trait HostedPublicRelayAuthoritySourceV1: Send + Sync + 'static {
    fn resolve(
        &self,
        request: &HostedPublicRelayBindRequestV1,
    ) -> Result<RoomSetupPublicRelayBindingV1, HostedPublicStreamErrorV1>;
}

impl HostedPublicRelayAuthoritySourceV1 for HostedLaunchOperationsV1 {
    fn resolve(
        &self,
        request: &HostedPublicRelayBindRequestV1,
    ) -> Result<RoomSetupPublicRelayBindingV1, HostedPublicStreamErrorV1> {
        self.resolve_public_relay(request).map_err(map_launch_error)
    }
}

#[derive(Clone)]
pub struct HostedPublicStreamBrokerV1 {
    root: Arc<PathBuf>,
    authority: Arc<dyn HostedPublicRelayAuthoritySourceV1>,
    vault: FileSecretVaultV1,
    gateway: Arc<dyn ParticipantConsoleGatewayV1>,
    client_origin: Arc<str>,
    mutation: Arc<Mutex<()>>,
}

impl HostedPublicStreamBrokerV1 {
    /// Opens the owner-only public correspondence store.
    ///
    /// # Errors
    /// Rejects an unsafe origin, storage, or retained inventory.
    pub fn open(
        root: &Path,
        launches: HostedLaunchOperationsV1,
        vault: FileSecretVaultV1,
        gateway: impl ParticipantConsoleGatewayV1,
        client_origin: &str,
    ) -> Result<Self, HostedPublicStreamErrorV1> {
        Self::open_with_authority(root, launches, vault, gateway, client_origin)
    }

    fn open_with_authority(
        root: &Path,
        authority: impl HostedPublicRelayAuthoritySourceV1,
        vault: FileSecretVaultV1,
        gateway: impl ParticipantConsoleGatewayV1,
        client_origin: &str,
    ) -> Result<Self, HostedPublicStreamErrorV1> {
        if !valid_client_origin(client_origin) {
            return Err(HostedPublicStreamErrorV1::Invalid);
        }
        let broker = Self {
            root: Arc::new(
                prepare_data_directory(root).map_err(|_| HostedPublicStreamErrorV1::Unavailable)?,
            ),
            authority: Arc::new(authority),
            vault,
            gateway: Arc::new(gateway),
            client_origin: Arc::from(client_origin),
            mutation: Arc::new(Mutex::new(())),
        };
        let guard = broker.lock();
        broker.bindings_unlocked()?;
        drop(guard);
        Ok(broker)
    }

    /// Persists an exact, revalidated public Run binding. Repeating the same
    /// canonical request is idempotent; any changed correspondence conflicts.
    ///
    /// # Errors
    /// Rejects invalid or changed correspondence and unavailable retained state.
    pub fn bind(
        &self,
        request: &HostedPublicRelayBindRequestV1,
    ) -> Result<HostedPublicRelayBindReceiptV1, HostedPublicStreamErrorV1> {
        validate_hosted_public_relay_bind_request(request)
            .map_err(|_| HostedPublicStreamErrorV1::Invalid)?;
        let relay = self.authority.resolve(request)?;
        if self
            .vault
            .inspect(SecretKindV1::MembershipAuthority, &relay.secret_reference)
            .availability
            != SecretAvailabilityV1::Configured
        {
            return Err(HostedPublicStreamErrorV1::Unavailable);
        }
        let canonical = canonical_bytes(request)?;
        let request_digest = tagged_sha256(&canonical);
        let retained = RetainedPublicRelayBindingV1 {
            schema: BINDING_SCHEMA_V1.to_owned(),
            request: request.clone(),
            request_digest: request_digest.clone(),
        };
        let _guard = self.lock();
        match self.load_unlocked(&request.public_run_id) {
            Ok(existing) if existing == retained => return receipt(request, request_digest),
            Ok(_) => return Err(HostedPublicStreamErrorV1::Conflict),
            Err(HostedPublicStreamErrorV1::NotFound) => {}
            Err(error) => return Err(error),
        }
        for existing in self.bindings_unlocked()? {
            if existing.request.activity_run_id == request.activity_run_id
                || existing.request.room_id == request.room_id
                || existing.request.relay_membership_id == request.relay_membership_id
            {
                return Err(HostedPublicStreamErrorV1::Conflict);
            }
        }
        self.persist_new(&retained)?;
        receipt(request, request_digest)
    }

    /// Revalidates one public binding and issues a fresh target-bound Runtime
    /// ticket. Membership authority and routing identities stay in this process.
    ///
    /// # Errors
    /// Rejects missing or stale bindings and unavailable Runtime admission.
    pub fn stream_ticket(
        &self,
        request: &HostedPublicStreamTicketRequestV1,
    ) -> Result<HostedBrowserStreamTicketResponseV1, HostedPublicStreamErrorV1> {
        validate_hosted_public_stream_ticket_request(request)
            .map_err(|_| HostedPublicStreamErrorV1::Invalid)?;
        let retained = {
            let _guard = self.lock();
            self.load_unlocked(&request.public_run_id)?
        };
        let relay = self.authority.resolve(&retained.request)?;
        let secret = self
            .vault
            .resolve(SecretKindV1::MembershipAuthority, &relay.secret_reference)
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        let bearer_bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        let authority = HumanSeatAuthorityV1::new(
            &relay.room_id,
            &relay.member_id,
            relay.pack.clone(),
            AccessMode::Spectator,
            None,
            SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes(bearer_bytes)),
        )
        .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        let current = self
            .gateway
            .current_membership(&authority, None)
            .map_err(map_gateway_error)?;
        require_public_relay_membership(&relay.pack, &current)?;
        let runtime_request = HostedBrowserWebSocketTicketIssueRequest {
            version: HOSTED_BROWSER_WS_TICKET_VERSION.to_owned(),
            room_id: relay.room_id,
            member_id: relay.member_id,
            mode: ClientMode::Spectator,
            after_frame_seq: None,
            browser_session_digest: random_digest()?,
            client_release_digest: format!(
                "blake3:{}",
                blake3::hash(PUBLIC_CLIENT_RELEASE).to_hex()
            ),
            client_surface_id: PUBLIC_CLIENT_SURFACE_ID.to_owned(),
        };
        let response = self
            .gateway
            .issue_hosted_browser_stream_ticket(&authority, &runtime_request, &self.client_origin)
            .map_err(map_gateway_error)?;
        let response = HostedBrowserStreamTicketResponseV1 {
            schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
            ticket: response.ticket,
            expires_in_ms: response.expires_in_ms,
        };
        validate_hosted_browser_stream_ticket_response(&response)
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        Ok(response)
    }

    fn bindings_unlocked(
        &self,
    ) -> Result<Vec<RetainedPublicRelayBindingV1>, HostedPublicStreamErrorV1> {
        let mut bindings = Vec::new();
        for entry in
            fs::read_dir(self.root.as_ref()).map_err(|_| HostedPublicStreamErrorV1::Unavailable)?
        {
            let path = entry
                .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?
                .path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                return Err(HostedPublicStreamErrorV1::Unavailable);
            };
            if name.starts_with('.')
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
            {
                continue;
            }
            let public_run_id = name
                .strip_suffix(".json")
                .ok_or(HostedPublicStreamErrorV1::Unavailable)?;
            bindings.push(self.load_unlocked(public_run_id)?);
            if bindings.len() > MAX_BINDINGS {
                return Err(HostedPublicStreamErrorV1::Unavailable);
            }
        }
        Ok(bindings)
    }

    fn load_unlocked(
        &self,
        public_run_id: &str,
    ) -> Result<RetainedPublicRelayBindingV1, HostedPublicStreamErrorV1> {
        if !valid_public_run_id(public_run_id) {
            return Err(HostedPublicStreamErrorV1::Invalid);
        }
        let path = self.root.join(format!("{public_run_id}.json"));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(HostedPublicStreamErrorV1::NotFound);
            }
            Err(_) => return Err(HostedPublicStreamErrorV1::Unavailable),
            Ok(_) => {}
        }
        validate_owner_only_file(&path).map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        let bytes = fs::read(path).map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        if bytes.len() > MAX_BINDING_BYTES {
            return Err(HostedPublicStreamErrorV1::Unavailable);
        }
        let retained = serde_json::from_slice::<RetainedPublicRelayBindingV1>(&bytes)
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        validate_retained(&retained, public_run_id)?;
        Ok(retained)
    }

    fn persist_new(
        &self,
        retained: &RetainedPublicRelayBindingV1,
    ) -> Result<(), HostedPublicStreamErrorV1> {
        validate_retained(retained, &retained.request.public_run_id)?;
        let bytes = serde_json::to_vec_pretty(retained)
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        if bytes.len() > MAX_BINDING_BYTES {
            return Err(HostedPublicStreamErrorV1::Unavailable);
        }
        let nonce = random_hex()?;
        let temporary = self
            .root
            .join(format!(".{}.{}.tmp", retained.request.public_run_id, nonce));
        let target = self
            .root
            .join(format!("{}.json", retained.request.public_run_id));
        let mut file = create_owner_only_file(&temporary)
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
        if file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(&temporary);
            return Err(HostedPublicStreamErrorV1::Unavailable);
        }
        drop(file);
        let result = fs::hard_link(&temporary, target)
            .and_then(|()| fs::File::open(self.root.as_ref())?.sync_all())
            .map_err(|_| HostedPublicStreamErrorV1::Unavailable);
        let _ = fs::remove_file(temporary);
        result
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.mutation.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the service-authenticated Controller routes for binding and ticket lookup.
pub fn hosted_public_stream_router(
    broker: HostedPublicStreamBrokerV1,
    access: HostedLaunchAccessV1,
) -> Router {
    Router::new()
        .route("/api/v1/hosted-public-relays:bind", post(bind_public_relay))
        .route(
            "/api/v1/hosted-public-streams:ticket",
            post(issue_public_stream_ticket),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(from_fn_with_state(Arc::new(access), admit_service))
        .with_state(broker)
}

async fn bind_public_relay(
    State(broker): State<HostedPublicStreamBrokerV1>,
    body: Bytes,
) -> Result<Response, HostedPublicStreamErrorV1> {
    let request = decode::<HostedPublicRelayBindRequestV1>(&body)?;
    let receipt = tokio::task::spawn_blocking(move || broker.bind(&request))
        .await
        .map_err(|_| HostedPublicStreamErrorV1::Unavailable)??;
    Ok((StatusCode::CREATED, Json(receipt)).into_response())
}

async fn issue_public_stream_ticket(
    State(broker): State<HostedPublicStreamBrokerV1>,
    body: Bytes,
) -> Result<Response, HostedPublicStreamErrorV1> {
    let request = decode::<HostedPublicStreamTicketRequestV1>(&body)?;
    let ticket = tokio::task::spawn_blocking(move || broker.stream_ticket(&request))
        .await
        .map_err(|_| HostedPublicStreamErrorV1::Unavailable)??;
    Ok((StatusCode::CREATED, Json(ticket)).into_response())
}

async fn admit_service(
    State(access): State<Arc<HostedLaunchAccessV1>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::POST || !access.authenticate(request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, HostedPublicStreamErrorV1> {
    if body.is_empty() || body.len() > MAX_REQUEST_BYTES {
        return Err(HostedPublicStreamErrorV1::Invalid);
    }
    let canonical = CanonicalJsonV1::parse(body).map_err(|_| HostedPublicStreamErrorV1::Invalid)?;
    let bytes = canonical
        .to_bytes()
        .map_err(|_| HostedPublicStreamErrorV1::Invalid)?;
    if bytes != body {
        return Err(HostedPublicStreamErrorV1::Invalid);
    }
    serde_json::from_slice(body).map_err(|_| HostedPublicStreamErrorV1::Invalid)
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, HostedPublicStreamErrorV1> {
    let encoded = serde_json::to_vec(value).map_err(|_| HostedPublicStreamErrorV1::Invalid)?;
    CanonicalJsonV1::parse(&encoded)
        .and_then(|canonical| canonical.to_bytes())
        .map_err(|_| HostedPublicStreamErrorV1::Invalid)
}

fn validate_retained(
    retained: &RetainedPublicRelayBindingV1,
    public_run_id: &str,
) -> Result<(), HostedPublicStreamErrorV1> {
    if retained.schema != BINDING_SCHEMA_V1
        || retained.request.public_run_id != public_run_id
        || validate_hosted_public_relay_bind_request(&retained.request).is_err()
        || retained.request_digest != tagged_sha256(&canonical_bytes(&retained.request)?)
    {
        return Err(HostedPublicStreamErrorV1::Unavailable);
    }
    Ok(())
}

fn receipt(
    request: &HostedPublicRelayBindRequestV1,
    request_digest: String,
) -> Result<HostedPublicRelayBindReceiptV1, HostedPublicStreamErrorV1> {
    let receipt = HostedPublicRelayBindReceiptV1 {
        schema: "worldstream/hosted-public-relay-bind-receipt/v1".to_owned(),
        public_run_id: request.public_run_id.clone(),
        activity_run_id: request.activity_run_id.clone(),
        binding_request_digest: request_digest,
        bound: true,
    };
    validate_hosted_public_relay_bind_receipt(&receipt)
        .map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
    Ok(receipt)
}

fn require_public_relay_membership(
    expected_pack: &PackReference,
    current: &CurrentMembershipSnapshotV1,
) -> Result<(), HostedPublicStreamErrorV1> {
    if current.pack != *expected_pack
        || current.access_mode != AccessMode::Spectator
        || current.role.is_some()
    {
        return Err(HostedPublicStreamErrorV1::Conflict);
    }
    Ok(())
}

fn tagged_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", lower_hex(&Sha256::digest(bytes)))
}

fn random_digest() -> Result<String, HostedPublicStreamErrorV1> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn random_hex() -> Result<String, HostedPublicStreamErrorV1> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| HostedPublicStreamErrorV1::Unavailable)?;
    Ok(lower_hex(&bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

fn valid_public_run_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_client_origin(value: &str) -> bool {
    if value.len() > 512 || value.contains(['?', '#']) || value.ends_with('/') {
        return false;
    }
    if let Some(authority) = value.strip_prefix("https://") {
        return !authority.is_empty() && !authority.contains('/');
    }
    value
        .strip_prefix("http://")
        .and_then(|authority| authority.rsplit_once(':'))
        .is_some_and(|(host, port)| {
            matches!(host, "127.0.0.1" | "localhost" | "[::1]")
                && port.parse::<u16>().is_ok_and(|port| port != 0)
        })
}

const fn map_launch_error(error: HostedLaunchErrorV1) -> HostedPublicStreamErrorV1 {
    match error {
        HostedLaunchErrorV1::Invalid => HostedPublicStreamErrorV1::Invalid,
        HostedLaunchErrorV1::Conflict => HostedPublicStreamErrorV1::Conflict,
        HostedLaunchErrorV1::NotFound => HostedPublicStreamErrorV1::NotFound,
        HostedLaunchErrorV1::Unavailable => HostedPublicStreamErrorV1::Unavailable,
    }
}

const fn map_gateway_error(error: ParticipantConsoleGatewayErrorV1) -> HostedPublicStreamErrorV1 {
    match error {
        ParticipantConsoleGatewayErrorV1::Rejected => HostedPublicStreamErrorV1::Conflict,
        ParticipantConsoleGatewayErrorV1::Disconnected
        | ParticipantConsoleGatewayErrorV1::Unavailable => HostedPublicStreamErrorV1::Unavailable,
    }
}

impl IntoResponse for HostedPublicStreamErrorV1 {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Conflict => StatusCode::CONFLICT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        };
        status.into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;
    use tempfile::tempdir;
    use worldstream_hosted_contract::PackReference as HostedPackReference;
    use worldstream_protocol::{BROWSER_WS_TICKET_VERSION, BrowserWebSocketTicketIssueResponse};

    use super::*;
    use crate::participant_handoff::{ParticipantActionRequestV1, ParticipantConsoleObservationV1};

    #[derive(Clone)]
    struct FakeAuthority {
        request: HostedPublicRelayBindRequestV1,
        binding: RoomSetupPublicRelayBindingV1,
    }

    impl HostedPublicRelayAuthoritySourceV1 for FakeAuthority {
        fn resolve(
            &self,
            request: &HostedPublicRelayBindRequestV1,
        ) -> Result<RoomSetupPublicRelayBindingV1, HostedPublicStreamErrorV1> {
            if request != &self.request {
                return Err(HostedPublicStreamErrorV1::Conflict);
            }
            Ok(self.binding.clone())
        }
    }

    #[derive(Clone)]
    struct FakeGateway {
        pack: PackReference,
        requests: Arc<Mutex<Vec<HostedBrowserWebSocketTicketIssueRequest>>>,
    }

    impl ParticipantConsoleGatewayV1 for FakeGateway {
        fn current_membership(
            &self,
            _authority: &HumanSeatAuthorityV1,
            _durable_cursor: Option<u64>,
        ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
            Ok(CurrentMembershipSnapshotV1 {
                pack: self.pack.clone(),
                access_mode: AccessMode::Spectator,
                role: None,
            })
        }

        fn observe(
            &self,
            _authority: &HumanSeatAuthorityV1,
            _durable_cursor: Option<u64>,
        ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
            Err(ParticipantConsoleGatewayErrorV1::Unavailable)
        }

        fn act(
            &self,
            _authority: &HumanSeatAuthorityV1,
            _durable_cursor: Option<u64>,
            _request: &ParticipantActionRequestV1,
        ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
            Err(ParticipantConsoleGatewayErrorV1::Rejected)
        }

        fn issue_hosted_browser_stream_ticket(
            &self,
            _authority: &HumanSeatAuthorityV1,
            request: &HostedBrowserWebSocketTicketIssueRequest,
            origin: &str,
        ) -> Result<BrowserWebSocketTicketIssueResponse, ParticipantConsoleGatewayErrorV1> {
            assert_eq!(origin, "https://arena.example");
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.clone());
            Ok(BrowserWebSocketTicketIssueResponse {
                version: BROWSER_WS_TICKET_VERSION.to_owned(),
                ticket: format!("wst1:{}", "a".repeat(64)),
                expires_in_ms: 15_000,
            })
        }
    }

    fn request(pack: &PackReference) -> HostedPublicRelayBindRequestV1 {
        HostedPublicRelayBindRequestV1 {
            schema: "worldstream/hosted-public-relay-bind-request/v1".to_owned(),
            public_run_id: "0123456789abcdef0123456789abcdef".to_owned(),
            activity_run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
            host_installation_id: "hosted-preview-1".to_owned(),
            launch_request_id: "10000000-0000-4000-8000-000000000001".to_owned(),
            listing_revision_digest: format!("blake3:{}", "1".repeat(64)),
            launch_request_digest: format!("blake3:{}", "2".repeat(64)),
            room_setup_operation_id: "hosted-launch-01".to_owned(),
            room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            pack: HostedPackReference {
                id: pack.id.clone(),
                version: pack.version.clone(),
                digest: pack.digest.clone(),
            },
            relay_principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW".to_owned(),
            relay_membership_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX".to_owned(),
        }
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn binding_is_durable_secret_free_and_ticket_is_fresh_and_spectator_only() {
        let root = tempdir().expect("temporary root");
        let vault = FileSecretVaultV1::open(&root.path().join("vault")).expect("vault");
        let secret_reference = vault
            .store(SecretKindV1::MembershipAuthority, &[7_u8; 32])
            .expect("membership bearer");
        let pack = PackReference {
            id: "worldstream.agent-heist".to_owned(),
            version: "0.2.0".to_owned(),
            digest: format!("blake3:{}", "3".repeat(64)),
        };
        let request = request(&pack);
        let authority = FakeAuthority {
            request: request.clone(),
            binding: RoomSetupPublicRelayBindingV1 {
                room_id: request.room_id.clone(),
                member_id: request.relay_membership_id.clone(),
                principal_id: request.relay_principal_id.clone(),
                pack: pack.clone(),
                secret_reference,
            },
        };
        let issued = Arc::new(Mutex::new(Vec::new()));
        let broker = HostedPublicStreamBrokerV1::open_with_authority(
            &root.path().join("bindings"),
            authority,
            vault,
            FakeGateway {
                pack,
                requests: Arc::clone(&issued),
            },
            "https://arena.example",
        )
        .expect("broker");

        let first = broker.bind(&request).expect("first binding");
        let second = broker.bind(&request).expect("idempotent binding");
        assert_eq!(first, second);
        let retained = fs::read_to_string(
            root.path()
                .join("bindings/0123456789abcdef0123456789abcdef.json"),
        )
        .expect("retained binding");
        assert!(!retained.contains("bearer"));
        assert!(!retained.contains("secret_reference"));

        let lookup = HostedPublicStreamTicketRequestV1 {
            schema: "worldstream/hosted-public-stream-ticket-request/v1".to_owned(),
            public_run_id: request.public_run_id,
        };
        broker.stream_ticket(&lookup).expect("first ticket");
        broker.stream_ticket(&lookup).expect("second ticket");
        let issued = issued.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(issued.len(), 2);
        assert_eq!(issued[0].mode, ClientMode::Spectator);
        assert_eq!(issued[0].after_frame_seq, None);
        assert_ne!(
            issued[0].browser_session_digest,
            issued[1].browser_session_digest
        );
    }
}
