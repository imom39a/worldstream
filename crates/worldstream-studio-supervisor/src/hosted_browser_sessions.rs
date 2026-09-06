//! Ephemeral hosted Browser Activity Sessions built on the existing
//! Membership-bound participant transport. Durable Run correspondence stays in
//! the platform database; Membership authority stays sealed in the Fly Host.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::de::DeserializeOwned;
use worldstream_activity_client::ExactPackReferenceV1;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{
    HostedBrowserHandoffRedeemRequestV1, HostedBrowserHandoffRedeemResponseV1,
    HostedBrowserHandoffRequestV1, HostedBrowserHandoffResponseV1, HostedBrowserSessionLogoutV1,
    HostedBrowserSessionRequestV1, HostedBrowserSessionStateV1, HostedBrowserSessionStatusV1,
    HostedBrowserStreamTicketRequestV1, HostedBrowserStreamTicketResponseV1,
    validate_hosted_browser_handoff_redeem_request, validate_hosted_browser_handoff_request,
    validate_hosted_browser_session_request, validate_hosted_browser_stream_ticket_request,
};
use worldstream_protocol::{
    AccessMode, ClientMode, HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION,
    HOSTED_BROWSER_WS_TICKET_VERSION, HostedBrowserWebSocketSessionRevokeRequest,
    HostedBrowserWebSocketTicketIssueRequest,
};
use worldstream_runtime::is_exact_loopback_origin;

use crate::{
    client_bindings::{
        ClientBindingStoreErrorV1, ClientCandidateClassV1, ClientCandidateV1,
        ClientSelectionRequestV1, ClientSelectionSourceV1, ClientSelectionV1,
    },
    hosted_launch::HostedLaunchAccessV1,
    participant_handoff::{
        CurrentMembershipSnapshotV1, HumanSeatAuthorityV1, ParticipantConsoleGatewayErrorV1,
        ParticipantConsoleGatewayV1, ParticipantConsoleSessionHealthV1,
        ParticipantHandoffAuthorityErrorV1,
    },
};

const HANDOFF_TTL_LIMIT: Duration = Duration::from_mins(5);
const DEFAULT_SESSION_TTL: Duration = Duration::from_hours(12);
const CLIENT_CONTRACT_V1: &str = "worldstream/activity-client-protocol/v1";
const TOKEN_BYTES: usize = 32;

/// Resolves only the exact immutable hosted Run Membership correspondence.
/// Implementations must keep the Membership bearer inside the Host boundary.
pub trait HostedBrowserMembershipAuthoritySourceV1: Send + Sync + 'static {
    /// Resolves and verifies one account-controlled human Membership.
    ///
    /// # Errors
    /// Returns a closed authority failure without exposing retained identity or
    /// secret-reference details.
    fn resolve_hosted_browser_membership(
        &self,
        binding: &HostedBrowserHandoffRequestV1,
    ) -> Result<HumanSeatAuthorityV1, ParticipantHandoffAuthorityErrorV1>;
}

#[derive(Clone, Eq, PartialEq)]
struct RetainedHostedTargetV1 {
    binding: HostedBrowserHandoffRequestV1,
    candidate: ClientCandidateV1,
}

struct HostedHandoffRecordV1 {
    target: RetainedHostedTargetV1,
    expires_at: Instant,
}

#[derive(Clone, Eq, PartialEq)]
struct HostedSessionRecordV1 {
    target: RetainedHostedTargetV1,
    expires_at: Instant,
}

#[derive(Default)]
struct HostedBrokerStateV1 {
    handoffs: HashMap<String, HostedHandoffRecordV1>,
    sessions: HashMap<String, HostedSessionRecordV1>,
}

struct HostedBrokerInnerV1 {
    host_installation_id: String,
    client_origin: String,
    handoff_ttl: Duration,
    session_ttl: Duration,
    maximum_retained: usize,
    authority: Arc<dyn HostedBrowserMembershipAuthoritySourceV1>,
    gateway: Arc<dyn ParticipantConsoleGatewayV1>,
    clients: Arc<dyn ClientSelectionSourceV1>,
    state: Mutex<HostedBrokerStateV1>,
}

/// In-memory one-use handoff and Browser Activity Session broker for the
/// single-authority hosted preview.
#[derive(Clone)]
pub struct HostedBrowserSessionBrokerV1 {
    inner: Arc<HostedBrokerInnerV1>,
}

impl HostedBrowserSessionBrokerV1 {
    /// Constructs a production broker for one exact Host installation and one
    /// exact registered Activity Client origin.
    ///
    /// # Errors
    /// Rejects non-HTTPS remote origins, unsafe Host identities, unbounded
    /// expiry, or unbounded retained state. Exact loopback HTTP origins remain
    /// available for the local deployment proof.
    pub fn new(
        host_installation_id: &str,
        client_origin: &str,
        handoff_ttl: Duration,
        maximum_retained: usize,
        authority: impl HostedBrowserMembershipAuthoritySourceV1,
        gateway: impl ParticipantConsoleGatewayV1,
        clients: impl ClientSelectionSourceV1,
    ) -> Result<Self, HostedBrowserSessionErrorV1> {
        Self::new_with_session_ttl(
            host_installation_id,
            client_origin,
            handoff_ttl,
            DEFAULT_SESSION_TTL,
            maximum_retained,
            authority,
            gateway,
            clients,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_session_ttl(
        host_installation_id: &str,
        client_origin: &str,
        handoff_ttl: Duration,
        session_ttl: Duration,
        maximum_retained: usize,
        authority: impl HostedBrowserMembershipAuthoritySourceV1,
        gateway: impl ParticipantConsoleGatewayV1,
        clients: impl ClientSelectionSourceV1,
    ) -> Result<Self, HostedBrowserSessionErrorV1> {
        if !safe_public_reference(host_installation_id, 128)
            || !exact_client_origin(client_origin)
            || handoff_ttl.is_zero()
            || handoff_ttl > HANDOFF_TTL_LIMIT
            || session_ttl.is_zero()
            || session_ttl > DEFAULT_SESSION_TTL
            || maximum_retained == 0
            || maximum_retained > 4_096
        {
            return Err(HostedBrowserSessionErrorV1::Invalid);
        }
        Ok(Self {
            inner: Arc::new(HostedBrokerInnerV1 {
                host_installation_id: host_installation_id.to_owned(),
                client_origin: client_origin.to_owned(),
                handoff_ttl,
                session_ttl,
                maximum_retained,
                authority: Arc::new(authority),
                gateway: Arc::new(gateway),
                clients: Arc::new(clients),
                state: Mutex::new(HostedBrokerStateV1::default()),
            }),
        })
    }

    /// Issues one fresh handoff after re-resolving the exact immutable
    /// Membership correspondence and pinned Activity Client release.
    ///
    /// # Errors
    /// Fails closed for changed Membership, Role, Pack, client, origin, or
    /// retained-state capacity.
    pub fn issue(
        &self,
        request: HostedBrowserHandoffRequestV1,
    ) -> Result<HostedBrowserHandoffResponseV1, HostedBrowserSessionErrorV1> {
        validate_hosted_browser_handoff_request(&request)
            .map_err(|_| HostedBrowserSessionErrorV1::Invalid)?;
        if request.host_installation_id != self.inner.host_installation_id {
            return Err(HostedBrowserSessionErrorV1::Rejected);
        }
        let authority = self.resolve_authority(&request)?;
        let current = self.current_membership(&authority)?;
        require_exact_membership(&request, &current)?;
        let candidate = self.resolve_pinned_client(&request, &current)?;
        let launch = exact_launch_url(&self.inner.client_origin, &candidate.launch_url)?;
        let mut state = self.lock();
        prune_expired(&mut state);
        if retained_len(&state) >= self.inner.maximum_retained {
            return Err(HostedBrowserSessionErrorV1::Capacity);
        }
        let handoff = unique_token("wsh1:", &state.handoffs)?;
        state.handoffs.insert(
            handoff.clone(),
            HostedHandoffRecordV1 {
                target: RetainedHostedTargetV1 {
                    binding: request,
                    candidate,
                },
                expires_at: Instant::now() + self.inner.handoff_ttl,
            },
        );
        Ok(HostedBrowserHandoffResponseV1 {
            schema: "worldstream/hosted-browser-handoff-response/v1".to_owned(),
            client_url: format!("{launch}#handoff={handoff}"),
        })
    }

    /// Atomically consumes one handoff before any fallible revalidation and
    /// rotates the browser profile's prior session when supplied.
    ///
    /// # Errors
    /// A consumed, expired, changed, or mismatched handoff always fails closed
    /// and can never be retried.
    pub fn redeem(
        &self,
        request: &HostedBrowserHandoffRedeemRequestV1,
    ) -> Result<HostedBrowserHandoffRedeemResponseV1, HostedBrowserSessionErrorV1> {
        validate_hosted_browser_handoff_redeem_request(request)
            .map_err(|_| HostedBrowserSessionErrorV1::Invalid)?;
        let record = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state
                .handoffs
                .remove(&request.handoff)
                .ok_or(HostedBrowserSessionErrorV1::Missing)?
        };
        if record.target.binding.platform_account_id != request.platform_account_id {
            return Err(HostedBrowserSessionErrorV1::Rejected);
        }
        self.revalidate_target(&record.target)?;
        let mut state = self.lock();
        prune_expired(&mut state);
        drop(state);
        if let Some(prior) = &request.prior_session {
            self.retire_session(prior, true)?;
        }
        let mut state = self.lock();
        prune_expired(&mut state);
        if retained_len(&state) >= self.inner.maximum_retained {
            return Err(HostedBrowserSessionErrorV1::Capacity);
        }
        let session = unique_token("wss1:", &state.sessions)?;
        state.sessions.insert(
            session.clone(),
            HostedSessionRecordV1 {
                target: record.target,
                expires_at: Instant::now() + self.inner.session_ttl,
            },
        );
        Ok(HostedBrowserHandoffRedeemResponseV1 {
            schema: "worldstream/hosted-browser-handoff-redeem-response/v1".to_owned(),
            session,
        })
    }

    /// Revalidates one retained session against the current Membership and
    /// exact active client Deployment.
    ///
    /// # Errors
    /// Missing, expired, revoked, or changed sessions are removed and rejected.
    pub fn status(
        &self,
        request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionStatusV1, HostedBrowserSessionErrorV1> {
        validate_hosted_browser_session_request(request)
            .map_err(|_| HostedBrowserSessionErrorV1::Invalid)?;
        let record = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state
                .sessions
                .get(&request.session)
                .cloned()
                .ok_or(HostedBrowserSessionErrorV1::Missing)?
        };
        let authority = match self.revalidate_target(&record.target) {
            Ok(authority) => authority,
            Err(error) => {
                if error.invalidates_session() {
                    let _ = self.retire_session(&request.session, false);
                }
                return Err(error);
            }
        };
        let state = match self.inner.gateway.health(&authority, None) {
            Ok(ParticipantConsoleSessionHealthV1::Usable) => HostedBrowserSessionStateV1::Usable,
            Ok(
                ParticipantConsoleSessionHealthV1::Disconnected
                | ParticipantConsoleSessionHealthV1::Stale,
            )
            | Err(ParticipantConsoleGatewayErrorV1::Disconnected) => {
                HostedBrowserSessionStateV1::Disconnected
            }
            Ok(
                ParticipantConsoleSessionHealthV1::Missing
                | ParticipantConsoleSessionHealthV1::Invalid,
            )
            | Err(ParticipantConsoleGatewayErrorV1::Rejected) => {
                let _ = self.retire_session(&request.session, false);
                return Err(HostedBrowserSessionErrorV1::Rejected);
            }
            Err(ParticipantConsoleGatewayErrorV1::Unavailable) => {
                return Err(HostedBrowserSessionErrorV1::Unavailable);
            }
        };
        Ok(HostedBrowserSessionStatusV1 {
            schema: "worldstream/hosted-browser-session-status/v1".to_owned(),
            state,
        })
    }

    /// Idempotently retires one opaque Browser Activity Session. A later direct
    /// stream admission check observes the same missing session.
    ///
    /// # Errors
    /// Rejects malformed session requests.
    pub fn logout(
        &self,
        request: &HostedBrowserSessionRequestV1,
    ) -> Result<HostedBrowserSessionLogoutV1, HostedBrowserSessionErrorV1> {
        validate_hosted_browser_session_request(request)
            .map_err(|_| HostedBrowserSessionErrorV1::Invalid)?;
        self.retire_session(&request.session, false)?;
        Ok(HostedBrowserSessionLogoutV1 {
            schema: "worldstream/hosted-browser-session-logout/v1".to_owned(),
            logged_out: true,
        })
    }

    /// Revalidates the Browser Activity Session and exact client Deployment,
    /// then asks the Runtime to retain one digest-only, target-bound ticket.
    ///
    /// # Errors
    /// Missing, expired, revoked, changed, or capacity-limited sessions fail
    /// closed without exposing any retained target or Membership credential.
    pub fn stream_ticket(
        &self,
        request: &HostedBrowserStreamTicketRequestV1,
    ) -> Result<HostedBrowserStreamTicketResponseV1, HostedBrowserSessionErrorV1> {
        validate_hosted_browser_stream_ticket_request(request)
            .map_err(|_| HostedBrowserSessionErrorV1::Invalid)?;
        let record = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state
                .sessions
                .get(&request.session)
                .cloned()
                .ok_or(HostedBrowserSessionErrorV1::Missing)?
        };
        let authority = match self.revalidate_target(&record.target) {
            Ok(authority) => authority,
            Err(error) => {
                if error.invalidates_session() {
                    let _ = self.retire_session(&request.session, false);
                }
                return Err(error);
            }
        };
        let runtime_request = HostedBrowserWebSocketTicketIssueRequest {
            version: HOSTED_BROWSER_WS_TICKET_VERSION.to_owned(),
            room_id: authority.room_id().to_owned(),
            member_id: authority.member_id().to_owned(),
            mode: match authority.access_mode() {
                AccessMode::Participant => ClientMode::Participant,
                AccessMode::Spectator => ClientMode::Spectator,
                AccessMode::Operator => return Err(HostedBrowserSessionErrorV1::Rejected),
            },
            after_frame_seq: request.after_frame_seq,
            browser_session_digest: session_digest(&request.session),
            client_release_digest: record.target.candidate.release_digest.clone(),
            client_surface_id: record.target.candidate.surface_id.clone(),
        };
        let response = self
            .inner
            .gateway
            .issue_hosted_browser_stream_ticket(
                &authority,
                &runtime_request,
                &self.inner.client_origin,
            )
            .map_err(map_gateway_error)?;
        let still_current = {
            let mut state = self.lock();
            prune_expired(&mut state);
            state.sessions.get(&request.session) == Some(&record)
        };
        if !still_current {
            let revoke = HostedBrowserWebSocketSessionRevokeRequest {
                version: HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION.to_owned(),
                browser_session_digest: session_digest(&request.session),
            };
            let _ = self
                .inner
                .gateway
                .revoke_hosted_browser_stream_session(&authority, &revoke);
            return Err(HostedBrowserSessionErrorV1::Missing);
        }
        Ok(HostedBrowserStreamTicketResponseV1 {
            schema: "worldstream/hosted-browser-stream-ticket-response/v1".to_owned(),
            ticket: response.ticket,
            expires_in_ms: response.expires_in_ms,
        })
    }

    fn revalidate_target(
        &self,
        target: &RetainedHostedTargetV1,
    ) -> Result<HumanSeatAuthorityV1, HostedBrowserSessionErrorV1> {
        let authority = self.resolve_authority(&target.binding)?;
        let current = self.current_membership(&authority)?;
        require_exact_membership(&target.binding, &current)?;
        let candidate = self.resolve_active_client(&target.binding, &current, &target.candidate)?;
        if candidate != target.candidate
            || exact_launch_url(&self.inner.client_origin, &candidate.launch_url).is_err()
        {
            return Err(HostedBrowserSessionErrorV1::Rejected);
        }
        Ok(authority)
    }

    fn resolve_authority(
        &self,
        binding: &HostedBrowserHandoffRequestV1,
    ) -> Result<HumanSeatAuthorityV1, HostedBrowserSessionErrorV1> {
        self.inner
            .authority
            .resolve_hosted_browser_membership(binding)
            .map_err(map_authority_error)
    }

    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
    ) -> Result<CurrentMembershipSnapshotV1, HostedBrowserSessionErrorV1> {
        self.inner
            .gateway
            .current_membership(authority, None)
            .map_err(map_gateway_error)
    }

    fn resolve_pinned_client(
        &self,
        binding: &HostedBrowserHandoffRequestV1,
        current: &CurrentMembershipSnapshotV1,
    ) -> Result<ClientCandidateV1, HostedBrowserSessionErrorV1> {
        let request = client_request(current);
        let selection = self
            .inner
            .clients
            .select_client(&request, None)
            .map_err(map_client_error)?;
        let candidates = match selection {
            ClientSelectionV1::Selected { candidate } => vec![candidate],
            ClientSelectionV1::SelectionRequired { candidates } => candidates,
            ClientSelectionV1::InspectorFallback { .. } => {
                return Err(HostedBrowserSessionErrorV1::Rejected);
            }
        };
        let mut matching = candidates.into_iter().filter(|candidate| {
            candidate.release_digest == binding.client_release_digest
                && candidate.surface_id == binding.client_surface_id
        });
        let candidate = matching
            .next()
            .ok_or(HostedBrowserSessionErrorV1::Rejected)?;
        if matching.next().is_some() {
            return Err(HostedBrowserSessionErrorV1::Rejected);
        }
        self.resolve_active_client(binding, current, &candidate)
    }

    fn resolve_active_client(
        &self,
        binding: &HostedBrowserHandoffRequestV1,
        current: &CurrentMembershipSnapshotV1,
        retained: &ClientCandidateV1,
    ) -> Result<ClientCandidateV1, HostedBrowserSessionErrorV1> {
        let candidate = self
            .inner
            .clients
            .resolve_active_client(
                &client_request(current),
                ClientCandidateClassV1::Binding,
                &retained.candidate_id,
            )
            .map_err(map_client_error)?;
        if candidate.release_digest != binding.client_release_digest
            || candidate.surface_id != binding.client_surface_id
        {
            return Err(HostedBrowserSessionErrorV1::Rejected);
        }
        Ok(candidate)
    }

    fn retire_session(
        &self,
        token: &str,
        require_runtime_revoke: bool,
    ) -> Result<(), HostedBrowserSessionErrorV1> {
        let record = self.lock().sessions.remove(token);
        let Some(record) = record else {
            return Ok(());
        };
        let authority = match self.resolve_authority(&record.target.binding) {
            Ok(authority) => authority,
            Err(_) if !require_runtime_revoke => return Ok(()),
            Err(error) => return Err(error),
        };
        let request = HostedBrowserWebSocketSessionRevokeRequest {
            version: HOSTED_BROWSER_WS_SESSION_REVOKE_VERSION.to_owned(),
            browser_session_digest: session_digest(token),
        };
        match self
            .inner
            .gateway
            .revoke_hosted_browser_stream_session(&authority, &request)
        {
            Ok(()) => Ok(()),
            Err(_) if !require_runtime_revoke => Ok(()),
            Err(error) => Err(map_gateway_error(error)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HostedBrokerStateV1> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Closed hosted browser-session failures safe for the service adapters to map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedBrowserSessionErrorV1 {
    Invalid,
    Rejected,
    Missing,
    Capacity,
    Unavailable,
}

/// Builds the exact service-authenticated Controller surface used only by the
/// colocated Hosted Gateway.
pub fn hosted_browser_session_router(
    broker: HostedBrowserSessionBrokerV1,
    access: HostedLaunchAccessV1,
) -> Router {
    Router::new()
        .route("/api/v1/hosted-browser-handoffs:issue", post(hosted_issue))
        .route(
            "/api/v1/hosted-browser-handoffs:redeem",
            post(hosted_redeem),
        )
        .route(
            "/api/v1/hosted-browser-sessions:status",
            post(hosted_status),
        )
        .route(
            "/api/v1/hosted-browser-sessions:logout",
            post(hosted_logout),
        )
        .route(
            "/api/v1/hosted-browser-sessions:stream-ticket",
            post(hosted_stream_ticket),
        )
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(from_fn_with_state(
            Arc::new(access),
            admit_hosted_browser_session,
        ))
        .with_state(broker)
}

async fn hosted_issue(
    State(broker): State<HostedBrowserSessionBrokerV1>,
    body: Bytes,
) -> Result<(StatusCode, Json<HostedBrowserHandoffResponseV1>), HostedBrowserSessionErrorV1> {
    let request = decode_request::<HostedBrowserHandoffRequestV1>(&body)?;
    tokio::task::spawn_blocking(move || broker.issue(request))
        .await
        .map_err(|_| HostedBrowserSessionErrorV1::Unavailable)?
        .map(|response| (StatusCode::CREATED, Json(response)))
}

async fn hosted_redeem(
    State(broker): State<HostedBrowserSessionBrokerV1>,
    body: Bytes,
) -> Result<(StatusCode, Json<HostedBrowserHandoffRedeemResponseV1>), HostedBrowserSessionErrorV1> {
    let request = decode_request::<HostedBrowserHandoffRedeemRequestV1>(&body)?;
    tokio::task::spawn_blocking(move || broker.redeem(&request))
        .await
        .map_err(|_| HostedBrowserSessionErrorV1::Unavailable)?
        .map(|response| (StatusCode::CREATED, Json(response)))
}

async fn hosted_status(
    State(broker): State<HostedBrowserSessionBrokerV1>,
    body: Bytes,
) -> Result<Json<HostedBrowserSessionStatusV1>, HostedBrowserSessionErrorV1> {
    let request = decode_request::<HostedBrowserSessionRequestV1>(&body)?;
    tokio::task::spawn_blocking(move || broker.status(&request))
        .await
        .map_err(|_| HostedBrowserSessionErrorV1::Unavailable)?
        .map(Json)
}

async fn hosted_logout(
    State(broker): State<HostedBrowserSessionBrokerV1>,
    body: Bytes,
) -> Result<Json<HostedBrowserSessionLogoutV1>, HostedBrowserSessionErrorV1> {
    let request = decode_request::<HostedBrowserSessionRequestV1>(&body)?;
    tokio::task::spawn_blocking(move || broker.logout(&request))
        .await
        .map_err(|_| HostedBrowserSessionErrorV1::Unavailable)?
        .map(Json)
}

async fn hosted_stream_ticket(
    State(broker): State<HostedBrowserSessionBrokerV1>,
    body: Bytes,
) -> Result<(StatusCode, Json<HostedBrowserStreamTicketResponseV1>), HostedBrowserSessionErrorV1> {
    let request = decode_request::<HostedBrowserStreamTicketRequestV1>(&body)?;
    tokio::task::spawn_blocking(move || broker.stream_ticket(&request))
        .await
        .map_err(|_| HostedBrowserSessionErrorV1::Unavailable)?
        .map(|response| (StatusCode::CREATED, Json(response)))
}

fn decode_request<T: DeserializeOwned>(body: &[u8]) -> Result<T, HostedBrowserSessionErrorV1> {
    CanonicalJsonV1::from_canonical_bytes(body)
        .map_err(|_| HostedBrowserSessionErrorV1::Invalid)?;
    serde_json::from_slice(body).map_err(|_| HostedBrowserSessionErrorV1::Invalid)
}

async fn admit_hosted_browser_session(
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

impl IntoResponse for HostedBrowserSessionErrorV1 {
    fn into_response(self) -> Response {
        match self {
            Self::Invalid => hosted_error(StatusCode::BAD_REQUEST, "hosted_browser_invalid"),
            Self::Rejected => hosted_error(StatusCode::FORBIDDEN, "hosted_browser_rejected"),
            Self::Missing => hosted_error(StatusCode::UNAUTHORIZED, "hosted_browser_missing"),
            Self::Capacity => {
                hosted_error(StatusCode::SERVICE_UNAVAILABLE, "hosted_browser_capacity")
            }
            Self::Unavailable => hosted_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "hosted_browser_unavailable",
            ),
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

impl HostedBrowserSessionErrorV1 {
    const fn invalidates_session(self) -> bool {
        matches!(self, Self::Rejected | Self::Missing)
    }
}

fn require_exact_membership(
    binding: &HostedBrowserHandoffRequestV1,
    current: &CurrentMembershipSnapshotV1,
) -> Result<(), HostedBrowserSessionErrorV1> {
    let expected_mode = match binding.access_mode {
        worldstream_hosted_contract::HostedGenesisAccessModeV1::Participant => {
            AccessMode::Participant
        }
        worldstream_hosted_contract::HostedGenesisAccessModeV1::Spectator => AccessMode::Spectator,
    };
    if current.pack.id != binding.pack.id
        || current.pack.version != binding.pack.version
        || current.pack.digest != binding.pack.digest
        || current.access_mode != expected_mode
        || current.role != binding.role
    {
        return Err(HostedBrowserSessionErrorV1::Rejected);
    }
    Ok(())
}

fn client_request(current: &CurrentMembershipSnapshotV1) -> ClientSelectionRequestV1 {
    ClientSelectionRequestV1 {
        pack: ExactPackReferenceV1 {
            id: current.pack.id.clone(),
            version: current.pack.version.clone(),
            digest: current.pack.digest.clone(),
        },
        client_contract: CLIENT_CONTRACT_V1.to_owned(),
        access_mode: current.access_mode,
        role: current.role.clone(),
    }
}

fn map_authority_error(error: ParticipantHandoffAuthorityErrorV1) -> HostedBrowserSessionErrorV1 {
    match error {
        ParticipantHandoffAuthorityErrorV1::Unavailable => HostedBrowserSessionErrorV1::Unavailable,
        ParticipantHandoffAuthorityErrorV1::SeatNotFound
        | ParticipantHandoffAuthorityErrorV1::NotHuman
        | ParticipantHandoffAuthorityErrorV1::NotProvisioned
        | ParticipantHandoffAuthorityErrorV1::AuthorityInvalid => {
            HostedBrowserSessionErrorV1::Rejected
        }
    }
}

fn map_gateway_error(error: ParticipantConsoleGatewayErrorV1) -> HostedBrowserSessionErrorV1 {
    match error {
        ParticipantConsoleGatewayErrorV1::Rejected => HostedBrowserSessionErrorV1::Rejected,
        ParticipantConsoleGatewayErrorV1::Disconnected
        | ParticipantConsoleGatewayErrorV1::Unavailable => HostedBrowserSessionErrorV1::Unavailable,
    }
}

fn map_client_error(error: ClientBindingStoreErrorV1) -> HostedBrowserSessionErrorV1 {
    match error {
        ClientBindingStoreErrorV1::InvalidChoice => HostedBrowserSessionErrorV1::Rejected,
        ClientBindingStoreErrorV1::Invalid | ClientBindingStoreErrorV1::Unavailable => {
            HostedBrowserSessionErrorV1::Unavailable
        }
    }
}

fn exact_client_origin(value: &str) -> bool {
    if is_exact_loopback_origin(value) {
        return true;
    }
    let Ok(uri) = value.parse::<axum::http::Uri>() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    if uri.scheme_str() != Some("https")
        || uri.path() != "/"
        || uri.query().is_some()
        || host.len() > 253
        || !host.contains('.')
        || host.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return false;
    }
    let canonical = match uri.port_u16() {
        Some(443) | None => format!("https://{host}"),
        Some(port) => format!("https://{host}:{port}"),
    };
    value == canonical
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn exact_launch_url(
    client_origin: &str,
    launch_url: &str,
) -> Result<String, HostedBrowserSessionErrorV1> {
    let Some(path) = launch_url.strip_prefix(client_origin) else {
        return Err(HostedBrowserSessionErrorV1::Rejected);
    };
    if !path.starts_with('/')
        || !path.ends_with('/')
        || path.contains(['?', '#', '\\', '\r', '\n'])
        || path.split('/').any(|segment| segment == "..")
    {
        return Err(HostedBrowserSessionErrorV1::Rejected);
    }
    Ok(launch_url.to_owned())
}

fn safe_public_reference(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn retained_len(state: &HostedBrokerStateV1) -> usize {
    state.handoffs.len().saturating_add(state.sessions.len())
}

fn prune_expired(state: &mut HostedBrokerStateV1) {
    let now = Instant::now();
    state.handoffs.retain(|_, record| record.expires_at > now);
    state.sessions.retain(|_, record| record.expires_at > now);
}

fn unique_token<T>(
    prefix: &str,
    retained: &HashMap<String, T>,
) -> Result<String, HostedBrowserSessionErrorV1> {
    for _ in 0..4 {
        let mut bytes = [0_u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| HostedBrowserSessionErrorV1::Unavailable)?;
        let token = format!("{prefix}{}", encode_hex(&bytes));
        bytes.fill(0);
        if !retained.contains_key(&token) {
            return Ok(token);
        }
    }
    Err(HostedBrowserSessionErrorV1::Unavailable)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn session_digest(session: &str) -> String {
    format!("blake3:{}", blake3::hash(session.as_bytes()).to_hex())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::Value;
    use worldstream_protocol::{BearerWireV1, PackReference, SealedCapabilityBearerV1};

    use crate::{
        client_bindings::DeploymentTrustLevelV1,
        participant_handoff::{ParticipantActionRequestV1, ParticipantConsoleObservationV1},
    };

    const ACCOUNT: &str = "10000000-0000-4000-8000-000000000001";
    const OTHER_ACCOUNT: &str = "10000000-0000-4000-8000-000000000002";
    const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
    const CLIENT_ORIGIN: &str = "https://arena.example";

    #[derive(Clone)]
    struct FakeAuthority {
        expected: Arc<Mutex<Option<HostedBrowserHandoffRequestV1>>>,
    }

    impl HostedBrowserMembershipAuthoritySourceV1 for FakeAuthority {
        fn resolve_hosted_browser_membership(
            &self,
            binding: &HostedBrowserHandoffRequestV1,
        ) -> Result<HumanSeatAuthorityV1, ParticipantHandoffAuthorityErrorV1> {
            if self
                .expected
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                != Some(binding)
            {
                return Err(ParticipantHandoffAuthorityErrorV1::AuthorityInvalid);
            }
            HumanSeatAuthorityV1::new(
                &binding.room_id,
                &binding.membership_id,
                PackReference {
                    id: binding.pack.id.clone(),
                    version: binding.pack.version.clone(),
                    digest: binding.pack.digest.clone(),
                },
                match binding.access_mode {
                    worldstream_hosted_contract::HostedGenesisAccessModeV1::Participant => {
                        AccessMode::Participant
                    }
                    worldstream_hosted_contract::HostedGenesisAccessModeV1::Spectator => {
                        AccessMode::Spectator
                    }
                },
                binding.role.clone(),
                SealedCapabilityBearerV1::from_wire(&BearerWireV1::from_bytes([7; 32])),
            )
        }
    }

    #[derive(Clone)]
    struct FakeGateway {
        current: Arc<Mutex<Option<CurrentMembershipSnapshotV1>>>,
        health: Arc<Mutex<ParticipantConsoleSessionHealthV1>>,
        stream_requests: Arc<Mutex<Vec<HostedBrowserWebSocketTicketIssueRequest>>>,
        revoked_sessions: Arc<Mutex<Vec<String>>>,
    }

    impl ParticipantConsoleGatewayV1 for FakeGateway {
        fn current_membership(
            &self,
            _authority: &HumanSeatAuthorityV1,
            _durable_cursor: Option<u64>,
        ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
            self.current
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or(ParticipantConsoleGatewayErrorV1::Rejected)
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
            Err(ParticipantConsoleGatewayErrorV1::Unavailable)
        }

        fn health(
            &self,
            _authority: &HumanSeatAuthorityV1,
            _durable_cursor: Option<u64>,
        ) -> Result<ParticipantConsoleSessionHealthV1, ParticipantConsoleGatewayErrorV1> {
            Ok(*self.health.lock().unwrap_or_else(PoisonError::into_inner))
        }

        fn issue_hosted_browser_stream_ticket(
            &self,
            _authority: &HumanSeatAuthorityV1,
            request: &HostedBrowserWebSocketTicketIssueRequest,
            origin: &str,
        ) -> Result<
            worldstream_protocol::BrowserWebSocketTicketIssueResponse,
            ParticipantConsoleGatewayErrorV1,
        > {
            if origin != CLIENT_ORIGIN {
                return Err(ParticipantConsoleGatewayErrorV1::Rejected);
            }
            self.stream_requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.clone());
            Ok(worldstream_protocol::BrowserWebSocketTicketIssueResponse {
                version: worldstream_protocol::BROWSER_WS_TICKET_VERSION.to_owned(),
                ticket: format!("wst1:{}", "c".repeat(64)),
                expires_in_ms: 15_000,
            })
        }

        fn revoke_hosted_browser_stream_session(
            &self,
            _authority: &HumanSeatAuthorityV1,
            request: &HostedBrowserWebSocketSessionRevokeRequest,
        ) -> Result<(), ParticipantConsoleGatewayErrorV1> {
            self.revoked_sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.browser_session_digest.clone());
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeClients {
        candidate: Arc<Mutex<Option<ClientCandidateV1>>>,
    }

    impl ClientSelectionSourceV1 for FakeClients {
        fn select_client(
            &self,
            _request: &ClientSelectionRequestV1,
            _preferred_candidate_id: Option<&str>,
        ) -> Result<ClientSelectionV1, ClientBindingStoreErrorV1> {
            self.candidate
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .map(|candidate| ClientSelectionV1::Selected { candidate })
                .ok_or(ClientBindingStoreErrorV1::InvalidChoice)
        }

        fn resolve_active_client(
            &self,
            _request: &ClientSelectionRequestV1,
            class: ClientCandidateClassV1,
            candidate_id: &str,
        ) -> Result<ClientCandidateV1, ClientBindingStoreErrorV1> {
            let candidate = self
                .candidate
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .ok_or(ClientBindingStoreErrorV1::InvalidChoice)?;
            if class != ClientCandidateClassV1::Binding || candidate.candidate_id != candidate_id {
                return Err(ClientBindingStoreErrorV1::InvalidChoice);
            }
            Ok(candidate)
        }
    }

    #[derive(Clone)]
    struct Fixture {
        broker: HostedBrowserSessionBrokerV1,
        authority: FakeAuthority,
        gateway: FakeGateway,
        clients: FakeClients,
    }

    fn request() -> HostedBrowserHandoffRequestV1 {
        HostedBrowserHandoffRequestV1 {
            schema: "worldstream/hosted-browser-handoff-request/v1".to_owned(),
            platform_account_id: ACCOUNT.to_owned(),
            run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
            listing_revision_digest: format!("blake3:{}", "1".repeat(64)),
            host_installation_id: "hosted-preview-1".to_owned(),
            room_setup_operation_id: "hosted-launch-01".to_owned(),
            room_id: ROOM.to_owned(),
            pack: worldstream_hosted_contract::PackReference {
                id: "worldstream.agent-heist".to_owned(),
                version: "0.2.0".to_owned(),
                digest: format!("blake3:{}", "2".repeat(64)),
            },
            client_release_digest: format!("blake3:{}", "3".repeat(64)),
            client_surface_id: "participant".to_owned(),
            access_mode: worldstream_hosted_contract::HostedGenesisAccessModeV1::Participant,
            purpose: worldstream_hosted_contract::HostedGenesisMembershipPurposeV1::Participant,
            seat_id: Some("navigator".to_owned()),
            role: Some("navigator".to_owned()),
            principal_kind: worldstream_hosted_contract::HostedGenesisPrincipalKindV1::Human,
            principal_id: PRINCIPAL.to_owned(),
            membership_id: MEMBER.to_owned(),
        }
    }

    fn fixture(handoff_ttl: Duration, session_ttl: Duration) -> Fixture {
        let binding = request();
        let authority = FakeAuthority {
            expected: Arc::new(Mutex::new(Some(binding.clone()))),
        };
        let gateway = FakeGateway {
            current: Arc::new(Mutex::new(Some(CurrentMembershipSnapshotV1 {
                pack: PackReference {
                    id: binding.pack.id.clone(),
                    version: binding.pack.version.clone(),
                    digest: binding.pack.digest.clone(),
                },
                access_mode: AccessMode::Participant,
                role: binding.role.clone(),
            }))),
            health: Arc::new(Mutex::new(ParticipantConsoleSessionHealthV1::Usable)),
            stream_requests: Arc::new(Mutex::new(Vec::new())),
            revoked_sessions: Arc::new(Mutex::new(Vec::new())),
        };
        let clients = FakeClients {
            candidate: Arc::new(Mutex::new(Some(ClientCandidateV1 {
                candidate_id: "binding-heist-participant".to_owned(),
                deployment_id: "deployment-heist-web".to_owned(),
                client_id: "worldstream.agent-heist.web".to_owned(),
                release_digest: binding.client_release_digest.clone(),
                surface_id: binding.client_surface_id.clone(),
                trust_level: DeploymentTrustLevelV1::Verified,
                launch_url: format!("{CLIENT_ORIGIN}/clients/agent-heist/"),
            }))),
        };
        let broker = HostedBrowserSessionBrokerV1::new_with_session_ttl(
            &binding.host_installation_id,
            CLIENT_ORIGIN,
            handoff_ttl,
            session_ttl,
            32,
            authority.clone(),
            gateway.clone(),
            clients.clone(),
        )
        .expect("broker fixture");
        Fixture {
            broker,
            authority,
            gateway,
            clients,
        }
    }

    fn issue(broker: &HostedBrowserSessionBrokerV1) -> String {
        let response = broker.issue(request()).expect("issue handoff");
        response
            .client_url
            .rsplit_once("#handoff=")
            .expect("fragment-only handoff")
            .1
            .to_owned()
    }

    fn redeem(
        broker: &HostedBrowserSessionBrokerV1,
        handoff: &str,
        account: &str,
        prior_session: Option<String>,
    ) -> Result<String, HostedBrowserSessionErrorV1> {
        broker
            .redeem(&HostedBrowserHandoffRedeemRequestV1 {
                schema: "worldstream/hosted-browser-handoff-redeem-request/v1".to_owned(),
                platform_account_id: account.to_owned(),
                handoff: handoff.to_owned(),
                prior_session,
            })
            .map(|response| response.session)
    }

    fn session_request(session: &str) -> HostedBrowserSessionRequestV1 {
        HostedBrowserSessionRequestV1 {
            schema: "worldstream/hosted-browser-session-request/v1".to_owned(),
            session: session.to_owned(),
        }
    }

    #[test]
    fn one_use_handoff_is_consumed_before_identity_revalidation() {
        let fixture = fixture(Duration::from_mins(1), Duration::from_mins(1));
        let handoff = issue(&fixture.broker);
        assert_eq!(
            redeem(&fixture.broker, &handoff, OTHER_ACCOUNT, None),
            Err(HostedBrowserSessionErrorV1::Rejected)
        );
        assert_eq!(
            redeem(&fixture.broker, &handoff, ACCOUNT, None),
            Err(HostedBrowserSessionErrorV1::Missing)
        );

        let handoff = issue(&fixture.broker);
        let session = redeem(&fixture.broker, &handoff, ACCOUNT, None).expect("redeem handoff");
        assert_eq!(
            redeem(&fixture.broker, &handoff, ACCOUNT, None),
            Err(HostedBrowserSessionErrorV1::Missing)
        );
        assert_eq!(
            fixture.broker.status(&session_request(&session)),
            Ok(HostedBrowserSessionStatusV1 {
                schema: "worldstream/hosted-browser-session-status/v1".to_owned(),
                state: HostedBrowserSessionStateV1::Usable,
            })
        );
    }

    #[test]
    fn new_redemption_rotates_the_prior_session_and_logout_is_idempotent() {
        let fixture = fixture(Duration::from_mins(1), Duration::from_mins(1));
        let first =
            redeem(&fixture.broker, &issue(&fixture.broker), ACCOUNT, None).expect("first session");
        let second = redeem(
            &fixture.broker,
            &issue(&fixture.broker),
            ACCOUNT,
            Some(first.clone()),
        )
        .expect("rotated session");
        assert_ne!(first, second);
        assert_eq!(
            fixture.broker.status(&session_request(&first)),
            Err(HostedBrowserSessionErrorV1::Missing)
        );
        assert!(fixture.broker.logout(&session_request(&second)).is_ok());
        assert!(fixture.broker.logout(&session_request(&second)).is_ok());
        assert_eq!(
            fixture.broker.status(&session_request(&second)),
            Err(HostedBrowserSessionErrorV1::Missing)
        );
        assert_eq!(
            fixture
                .gateway
                .revoked_sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            &[session_digest(&first), session_digest(&second)]
        );
    }

    #[test]
    fn current_membership_and_exact_client_are_revalidated_for_every_session() {
        let fixture = fixture(Duration::from_mins(1), Duration::from_mins(1));
        let session =
            redeem(&fixture.broker, &issue(&fixture.broker), ACCOUNT, None).expect("session");
        fixture
            .gateway
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .expect("current membership")
            .role = Some("planner".to_owned());
        assert_eq!(
            fixture.broker.status(&session_request(&session)),
            Err(HostedBrowserSessionErrorV1::Rejected)
        );
        assert_eq!(
            fixture.broker.status(&session_request(&session)),
            Err(HostedBrowserSessionErrorV1::Missing)
        );

        *fixture
            .gateway
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(CurrentMembershipSnapshotV1 {
            pack: PackReference {
                id: request().pack.id,
                version: request().pack.version,
                digest: request().pack.digest,
            },
            access_mode: AccessMode::Participant,
            role: Some("navigator".to_owned()),
        });
        let session = redeem(&fixture.broker, &issue(&fixture.broker), ACCOUNT, None)
            .expect("replacement session");
        fixture
            .clients
            .candidate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .expect("client candidate")
            .launch_url = "https://other.example/clients/agent-heist/".to_owned();
        assert_eq!(
            fixture.broker.status(&session_request(&session)),
            Err(HostedBrowserSessionErrorV1::Rejected)
        );

        *fixture
            .authority
            .expected
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        assert_eq!(
            fixture.broker.issue(request()),
            Err(HostedBrowserSessionErrorV1::Rejected)
        );
    }

    #[test]
    fn stream_ticket_is_bound_to_the_retained_session_membership_and_client() {
        let fixture = fixture(Duration::from_mins(1), Duration::from_mins(1));
        let session =
            redeem(&fixture.broker, &issue(&fixture.broker), ACCOUNT, None).expect("session");
        let expected_session_digest = session_digest(&session);
        let response = fixture
            .broker
            .stream_ticket(&HostedBrowserStreamTicketRequestV1 {
                schema: "worldstream/hosted-browser-stream-ticket-request/v1".to_owned(),
                session: session.clone(),
                after_frame_seq: Some(19),
            })
            .expect("stream ticket");
        assert_eq!(response.ticket, format!("wst1:{}", "c".repeat(64)));
        assert!(!format!("{response:?}").contains(&response.ticket));
        let requests = fixture
            .gateway
            .stream_requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].room_id, ROOM);
        assert_eq!(requests[0].member_id, MEMBER);
        assert_eq!(requests[0].mode, ClientMode::Participant);
        assert_eq!(requests[0].after_frame_seq, Some(19));
        assert_eq!(requests[0].browser_session_digest, expected_session_digest);
        assert_eq!(
            requests[0].client_release_digest,
            request().client_release_digest
        );
        assert_eq!(requests[0].client_surface_id, "participant");
        drop(requests);

        fixture
            .gateway
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            .expect("current membership")
            .access_mode = AccessMode::Spectator;
        assert_eq!(
            fixture
                .broker
                .stream_ticket(&HostedBrowserStreamTicketRequestV1 {
                    schema: "worldstream/hosted-browser-stream-ticket-request/v1".to_owned(),
                    session: session.clone(),
                    after_frame_seq: None,
                }),
            Err(HostedBrowserSessionErrorV1::Rejected)
        );
        assert_eq!(
            fixture
                .broker
                .stream_ticket(&HostedBrowserStreamTicketRequestV1 {
                    schema: "worldstream/hosted-browser-stream-ticket-request/v1".to_owned(),
                    session,
                    after_frame_seq: None,
                }),
            Err(HostedBrowserSessionErrorV1::Missing)
        );
        assert_eq!(
            fixture
                .gateway
                .stream_requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );
        assert_eq!(
            fixture
                .gateway
                .revoked_sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            &[expected_session_digest]
        );
    }

    #[test]
    fn ephemeral_tokens_expire_and_do_not_survive_broker_restart() {
        let expiring = fixture(Duration::from_millis(1), Duration::from_millis(1));
        let handoff = issue(&expiring.broker);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            redeem(&expiring.broker, &handoff, ACCOUNT, None),
            Err(HostedBrowserSessionErrorV1::Missing)
        );
        let session = redeem(&expiring.broker, &issue(&expiring.broker), ACCOUNT, None)
            .expect("short session");
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(
            expiring.broker.status(&session_request(&session)),
            Err(HostedBrowserSessionErrorV1::Missing)
        );

        let durable = fixture(Duration::from_mins(1), Duration::from_mins(1));
        let old_session = redeem(&durable.broker, &issue(&durable.broker), ACCOUNT, None)
            .expect("pre-restart session");
        let restarted = HostedBrowserSessionBrokerV1::new(
            "hosted-preview-1",
            CLIENT_ORIGIN,
            Duration::from_mins(1),
            32,
            durable.authority,
            durable.gateway,
            durable.clients,
        )
        .expect("restarted broker");
        assert_eq!(
            restarted.status(&session_request(&old_session)),
            Err(HostedBrowserSessionErrorV1::Missing)
        );
        let replacement = redeem(&restarted, &issue(&restarted), ACCOUNT, None)
            .expect("fresh post-restart entry");
        assert!(restarted.status(&session_request(&replacement)).is_ok());
    }

    #[test]
    fn hosted_client_origin_and_launch_path_are_exact() {
        assert!(exact_client_origin("https://arena.example"));
        assert!(exact_client_origin("https://arena.example:8443"));
        for invalid in [
            "http://arena.example",
            "https://arena.example/clients",
            "https://arena.example:bad",
            "https://-arena.example",
            "https://arena..example",
            "https://arena.example:443",
        ] {
            assert!(!exact_client_origin(invalid), "{invalid}");
        }
        assert!(
            exact_launch_url(
                "https://arena.example",
                "https://arena.example/clients/heist/"
            )
            .is_ok()
        );
        for invalid in [
            "https://other.example/clients/heist/",
            "https://arena.example.evil/clients/heist/",
            "https://arena.example/clients/../admin/",
            "https://arena.example/clients/heist/?room=private",
        ] {
            assert!(
                exact_launch_url("https://arena.example", invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
