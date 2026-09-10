//! Bounded Host-authorized Activity Pack catalog proxy for Studio.

use std::{
    collections::BTreeSet,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;
use worldstream_protocol::{
    ACTIVITY_PACK_CATALOG_VERSION, ActivityPackCatalogResponse,
    ActivityPackCatalogRevisionResponse, BearerWireV1, MAX_MESSAGE_BYTES,
};
use zeroize::Zeroizing;

use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const MAX_CATALOG_REVISIONS: usize = 256;
const MAX_DESCRIPTOR_ROWS: usize = 256;
const MAX_DESCRIPTOR_TEXT_BYTES: usize = 256;
const MAX_ACTIVITY_START_PAYLOAD_BYTES: usize = 16_384;

/// Closed failures safe to expose across the Studio browser boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityPackProxyErrorV1 {
    /// No usable exact Host authority reference is configured.
    AuthorityUnavailable,
    /// The configured daemon cannot be reached.
    DaemonUnavailable,
    /// The daemon response was not the expected bounded typed response.
    InvalidResponse,
    /// The requested exact digest is malformed.
    InvalidRevision,
    /// The daemon does not have the requested exact revision.
    RevisionUnavailable,
}

/// The sole capability injected into Studio's Activity Pack routes.
pub trait DaemonActivityPackSource: Send + Sync + 'static {
    /// Lists the installed bounded catalog using retained Host authority.
    ///
    /// # Errors
    ///
    /// Returns a closed proxy error without credential or daemon-body details.
    fn catalog(&self) -> Result<ActivityPackCatalogResponse, ActivityPackProxyErrorV1>;

    /// Reads one exact digest without name or version fallback.
    ///
    /// # Errors
    ///
    /// Returns a closed proxy error when authority, transport, response, or
    /// the requested exact revision is unavailable.
    fn revision(
        &self,
        digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, ActivityPackProxyErrorV1>;
}

/// Fixed-address daemon client with one exact retained Host authority reference.
#[derive(Clone, Debug)]
pub struct HttpDaemonActivityPackSource {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
    managed: Option<crate::managed_daemon_transport::ManagedDaemonTransport>,
}

impl HttpDaemonActivityPackSource {
    /// Configures the bounded client. Absence of a reference fails closed when used.
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

    /// Uses retained managed Runtime ownership and same-socket proof before
    /// resolving Host authority. The explicit foreground constructor is unchanged.
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

    fn request(&self, path: &str) -> Result<DaemonHttpResponse, ActivityPackProxyErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(ActivityPackProxyErrorV1::AuthorityUnavailable)?;
        if let Some(transport) = &self.managed {
            let mut authority_unavailable = false;
            let response = transport
                .request("GET", path, b"", MAX_MESSAGE_BYTES, || {
                    let resolved = (|| {
                        let secret = self
                            .vault
                            .resolve(SecretKindV1::HostAuthority, reference)
                            .map_err(|_| ())?;
                        let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                        let token = Zeroizing::new(format!(
                            "Bearer {}",
                            BearerWireV1::from_bytes(bytes).to_wire()
                        ));
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
                        ActivityPackProxyErrorV1::AuthorityUnavailable
                    } else {
                        ActivityPackProxyErrorV1::DaemonUnavailable
                    }
                })?;
            if response.body.len() > MAX_MESSAGE_BYTES {
                return Err(ActivityPackProxyErrorV1::InvalidResponse);
            }
            return Ok(DaemonHttpResponse {
                status: response.status,
                body: response.body,
            });
        }
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| ActivityPackProxyErrorV1::AuthorityUnavailable)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| ActivityPackProxyErrorV1::AuthorityUnavailable)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| ActivityPackProxyErrorV1::DaemonUnavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| ActivityPackProxyErrorV1::DaemonUnavailable)?;
        let request = Zeroizing::new(format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
            self.address,
            bearer.as_str()
        ));
        stream
            .write_all(request.as_bytes())
            .map_err(|_| ActivityPackProxyErrorV1::DaemonUnavailable)?;
        let mut response = Vec::new();
        let limit = u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1;
        stream
            .take(limit)
            .read_to_end(&mut response)
            .map_err(|_| ActivityPackProxyErrorV1::DaemonUnavailable)?;
        if response.len() > MAX_MESSAGE_BYTES {
            return Err(ActivityPackProxyErrorV1::InvalidResponse);
        }
        parse_http_response(&response)
    }
}

impl DaemonActivityPackSource for HttpDaemonActivityPackSource {
    fn catalog(&self) -> Result<ActivityPackCatalogResponse, ActivityPackProxyErrorV1> {
        let response = self.request("/v1/operator/activity-packs")?;
        map_common_status(response.status)?;
        let catalog: ActivityPackCatalogResponse = serde_json::from_slice(&response.body)
            .map_err(|_| ActivityPackProxyErrorV1::InvalidResponse)?;
        validate_catalog(&catalog)?;
        Ok(catalog)
    }

    fn revision(
        &self,
        digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, ActivityPackProxyErrorV1> {
        if !is_digest(digest) {
            return Err(ActivityPackProxyErrorV1::InvalidRevision);
        }
        let response = self.request(&format!("/v1/operator/activity-packs/{digest}"))?;
        if response.status == 404 {
            return Err(ActivityPackProxyErrorV1::RevisionUnavailable);
        }
        map_common_status(response.status)?;
        let detail: ActivityPackCatalogRevisionResponse = serde_json::from_slice(&response.body)
            .map_err(|_| ActivityPackProxyErrorV1::InvalidResponse)?;
        validate_detail(&detail, digest)?;
        Ok(detail)
    }
}

/// Builds the read-only Activity Pack catalog browser API.
pub fn activity_pack_router(source: impl DaemonActivityPackSource) -> Router {
    let source: Arc<dyn DaemonActivityPackSource> = Arc::new(source);
    Router::new()
        .route("/api/v1/activity-packs", get(catalog))
        .route("/api/v1/activity-packs/{digest}", get(revision))
        .with_state(source)
}

async fn catalog(
    State(source): State<Arc<dyn DaemonActivityPackSource>>,
) -> Result<Json<ActivityPackCatalogResponse>, ActivityPackProxyErrorV1> {
    tokio::task::spawn_blocking(move || source.catalog())
        .await
        .map_err(|_| ActivityPackProxyErrorV1::DaemonUnavailable)?
        .map(Json)
}

async fn revision(
    State(source): State<Arc<dyn DaemonActivityPackSource>>,
    Path(digest): Path<String>,
) -> Result<Json<ActivityPackCatalogRevisionResponse>, ActivityPackProxyErrorV1> {
    if !is_digest(&digest) {
        return Err(ActivityPackProxyErrorV1::InvalidRevision);
    }
    tokio::task::spawn_blocking(move || source.revision(&digest))
        .await
        .map_err(|_| ActivityPackProxyErrorV1::DaemonUnavailable)?
        .map(Json)
}

#[derive(Serialize)]
struct ProxyErrorEnvelope {
    error: ProxyErrorBody,
}

#[derive(Serialize)]
struct ProxyErrorBody {
    code: &'static str,
    message: &'static str,
    retryable: bool,
}

impl IntoResponse for ActivityPackProxyErrorV1 {
    fn into_response(self) -> Response {
        let (status, code, message, retryable) = match self {
            Self::AuthorityUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "host_authority_unavailable",
                "retained Host authority is unavailable",
                false,
            ),
            Self::DaemonUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon_unavailable",
                "the configured daemon is unavailable",
                true,
            ),
            Self::InvalidResponse => (
                StatusCode::BAD_GATEWAY,
                "daemon_response_invalid",
                "the configured daemon returned an invalid response",
                false,
            ),
            Self::InvalidRevision => (
                StatusCode::BAD_REQUEST,
                "activity_pack_revision_invalid",
                "the Activity Pack revision digest is invalid",
                false,
            ),
            Self::RevisionUnavailable => (
                StatusCode::NOT_FOUND,
                "activity_pack_revision_unavailable",
                "the exact Activity Pack revision is unavailable",
                false,
            ),
        };
        (
            status,
            Json(ProxyErrorEnvelope {
                error: ProxyErrorBody {
                    code,
                    message,
                    retryable,
                },
            }),
        )
            .into_response()
    }
}

struct DaemonHttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn parse_http_response(bytes: &[u8]) -> Result<DaemonHttpResponse, ActivityPackProxyErrorV1> {
    let separator = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(ActivityPackProxyErrorV1::InvalidResponse)?;
    let headers = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| ActivityPackProxyErrorV1::InvalidResponse)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(ActivityPackProxyErrorV1::InvalidResponse)?;
    Ok(DaemonHttpResponse {
        status,
        body: bytes[(separator + 4)..].to_vec(),
    })
}

fn map_common_status(status: u16) -> Result<(), ActivityPackProxyErrorV1> {
    match status {
        200 => Ok(()),
        401 | 403 => Err(ActivityPackProxyErrorV1::AuthorityUnavailable),
        _ => Err(ActivityPackProxyErrorV1::InvalidResponse),
    }
}

fn validate_catalog(catalog: &ActivityPackCatalogResponse) -> Result<(), ActivityPackProxyErrorV1> {
    if catalog.version != ACTIVITY_PACK_CATALOG_VERSION
        || catalog.revisions.len() > MAX_CATALOG_REVISIONS
    {
        return Err(ActivityPackProxyErrorV1::InvalidResponse);
    }
    let mut digests = BTreeSet::new();
    for revision in &catalog.revisions {
        if !is_bounded_text(&revision.pack.id)
            || !is_bounded_text(&revision.pack.version)
            || !is_bounded_text(&revision.name)
            || !is_digest(&revision.pack.digest)
            || !digests.insert(revision.pack.digest.as_str())
        {
            return Err(ActivityPackProxyErrorV1::InvalidResponse);
        }
    }
    Ok(())
}

fn validate_detail(
    detail: &ActivityPackCatalogRevisionResponse,
    requested_digest: &str,
) -> Result<(), ActivityPackProxyErrorV1> {
    let revision = &detail.revision;
    if detail.version != ACTIVITY_PACK_CATALOG_VERSION
        || revision.summary.pack.digest != requested_digest
        || (revision.lobby_compatibility.is_some()
            && revision.activity_start_compatibility.is_some())
        || revision.roles.len() > MAX_DESCRIPTOR_ROWS
        || revision.actions.len() > MAX_DESCRIPTOR_ROWS
        || !is_bounded_text(&revision.summary.pack.id)
        || !is_bounded_text(&revision.summary.pack.version)
        || !is_bounded_text(&revision.summary.name)
        || !is_schema(
            &revision.configuration_schema.schema_id,
            &revision.configuration_schema.schema_digest,
        )
        || revision
            .roles
            .iter()
            .any(|role| !is_bounded_text(&role.role) || role.minimum > role.maximum)
        || revision.actions.iter().any(|action| {
            !is_bounded_text(&action.action_type)
                || !is_schema(
                    &action.payload_schema.schema_id,
                    &action.payload_schema.schema_digest,
                )
        })
        || revision.lobby_compatibility.as_ref().is_some_and(|lobby| {
            !is_bounded_text(&lobby.contract)
                || !is_schema(
                    &lobby.configuration_schema.schema_id,
                    &lobby.configuration_schema.schema_digest,
                )
        })
        || revision
            .activity_start_compatibility
            .as_ref()
            .is_some_and(|start| {
                start.contract != "worldstream/activity-start/v1"
                    || !is_bounded_text(&start.contract)
                    || !is_bounded_text(&start.pre_start_phase)
                    || !is_bounded_text(&start.input_type)
                    || !is_schema(
                        &start.input_schema.schema_id,
                        &start.input_schema.schema_digest,
                    )
                    || match serde_json::to_vec(&start.canonical_payload) {
                        Ok(payload) => payload.len() > MAX_ACTIVITY_START_PAYLOAD_BYTES,
                        Err(_) => true,
                    }
            })
    {
        return Err(ActivityPackProxyErrorV1::InvalidResponse);
    }
    Ok(())
}

fn is_schema(id: &str, digest: &str) -> bool {
    is_bounded_text(id) && is_digest(digest)
}

fn is_bounded_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_DESCRIPTOR_TEXT_BYTES
}

fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("blake3:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        sync::{Arc, Mutex, PoisonError},
        thread,
        time::Duration,
    };

    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tempfile::tempdir;
    use tower::ServiceExt as _;
    use worldstream_protocol::{ActivityPackCatalogResponse, ActivityPackCatalogRevisionResponse};

    use super::{
        ActivityPackProxyErrorV1, DaemonActivityPackSource, HttpDaemonActivityPackSource,
        activity_pack_router, validate_detail,
    };
    use crate::secrets::{FileSecretVaultV1, SecretKindV1};

    const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[derive(Clone)]
    struct FixedSource {
        catalog: ActivityPackCatalogResponse,
        detail: ActivityPackCatalogRevisionResponse,
        requested: Arc<Mutex<Vec<String>>>,
    }

    impl DaemonActivityPackSource for FixedSource {
        fn catalog(&self) -> Result<ActivityPackCatalogResponse, ActivityPackProxyErrorV1> {
            Ok(self.catalog.clone())
        }

        fn revision(
            &self,
            digest: &str,
        ) -> Result<ActivityPackCatalogRevisionResponse, ActivityPackProxyErrorV1> {
            self.requested
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(digest.to_owned());
            if digest == DIGEST {
                Ok(self.detail.clone())
            } else {
                Err(ActivityPackProxyErrorV1::RevisionUnavailable)
            }
        }
    }

    fn fixture() -> FixedSource {
        let catalog: ActivityPackCatalogResponse = serde_json::from_value(serde_json::json!({
            "version": "activity_pack_catalog.v1",
            "revisions": [{
                "pack": { "id": "counter", "version": "2.0.0", "digest": DIGEST },
                "name": "Counter",
                "selectable_for_new_rooms": true,
                "runnable_for_retained_rooms": true
            }]
        }))
        .unwrap_or_else(|error| unreachable!("catalog fixture: {error}"));
        let detail: ActivityPackCatalogRevisionResponse =
            serde_json::from_value(serde_json::json!({
                "version": "activity_pack_catalog.v1",
                "revision": {
                    "summary": catalog.revisions[0],
                    "roles": [{ "role": "player", "minimum": 1, "maximum": 4 }],
                    "configuration_schema": {
                        "schema_id": "counter.config.v2",
                        "schema_digest": format!("blake3:{}", "b".repeat(64)),
                        "schema": { "type": "object" }
                    },
                    "actions": []
                }
            }))
            .unwrap_or_else(|error| unreachable!("detail fixture: {error}"));
        FixedSource {
            catalog,
            detail,
            requested: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[tokio::test]
    async fn studio_catalog_proxy_returns_only_the_typed_daemon_catalog() {
        let source = fixture();
        let expected = source.catalog.clone();
        let response = activity_pack_router(source)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/activity-packs")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));

        assert_eq!(response.status(), 200);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("body: {error}"))
            .to_bytes();
        let actual: ActivityPackCatalogResponse = serde_json::from_slice(&body)
            .unwrap_or_else(|error| unreachable!("catalog JSON: {error}"));
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn studio_detail_proxy_preserves_the_exact_digest_and_never_substitutes() {
        let source = fixture();
        let requested = Arc::clone(&source.requested);
        let response = activity_pack_router(source)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/activity-packs/{DIGEST}"))
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));

        assert_eq!(response.status(), 200);
        assert_eq!(
            requested
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            [DIGEST]
        );
    }

    #[tokio::test]
    async fn malformed_or_unknown_exact_revisions_fail_closed() {
        let source = fixture();
        let requested = Arc::clone(&source.requested);
        let malformed = activity_pack_router(source.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/activity-packs/counter@2")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(malformed.status(), 400);
        assert!(
            requested
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        );

        let unknown = activity_pack_router(source)
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/activity-packs/blake3:{}", "f".repeat(64)))
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("response: {error}"));
        assert_eq!(unknown.status(), 404);
    }

    #[test]
    fn activity_start_payload_uses_the_core_declaration_bound() {
        let mut detail = fixture().detail;
        detail.revision.activity_start_compatibility = Some(
            serde_json::from_value(serde_json::json!({
                "contract": "worldstream/activity-start/v1",
                "pre_start_phase": "briefing",
                "input_type": "fixture/start/v1",
                "canonical_payload": "x".repeat(16_385),
                "input_schema": {
                    "schema_id": "fixture.start.v1",
                    "schema_digest": format!("blake3:{}", "c".repeat(64)),
                    "schema": { "type": "string" }
                }
            }))
            .unwrap_or_else(|error| unreachable!("Activity Start fixture: {error}")),
        );
        assert_eq!(
            validate_detail(&detail, DIGEST),
            Err(ActivityPackProxyErrorV1::InvalidResponse)
        );
    }

    #[test]
    fn live_source_resolves_retained_host_authority_only_for_the_daemon_request() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        let reference = vault
            .store(SecretKindV1::HostAuthority, &[0xab; 32])
            .unwrap_or_else(|error| unreachable!("store Host authority: {error}"));
        let expected = fixture().catalog;
        let response_body = serde_json::to_vec(&expected)
            .unwrap_or_else(|error| unreachable!("catalog fixture JSON: {error}"));
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| unreachable!("loopback listener: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("loopback address: {error}"));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let server_observed = Arc::clone(&observed);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .unwrap_or_else(|error| unreachable!("accept proxy request: {error}"));
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap_or_else(|error| unreachable!("request timeout: {error}"));
            let mut bytes = vec![0_u8; 4096];
            let count = stream
                .read(&mut bytes)
                .unwrap_or_else(|error| unreachable!("read proxy request: {error}"));
            bytes.truncate(count);
            *server_observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = bytes;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .and_then(|()| stream.write_all(&response_body))
            .unwrap_or_else(|error| unreachable!("write daemon response: {error}"));
        });
        let source = HttpDaemonActivityPackSource::new(
            address,
            Duration::from_secs(2),
            vault,
            Some(reference),
        );

        let actual = source
            .catalog()
            .unwrap_or_else(|error| unreachable!("proxy catalog: {error:?}"));
        server
            .join()
            .unwrap_or_else(|_| unreachable!("join daemon fixture"));
        let request = String::from_utf8(
            observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap_or_else(|error| unreachable!("request text: {error}"));

        assert_eq!(actual, expected);
        assert!(request.starts_with("GET /v1/operator/activity-packs HTTP/1.1\r\n"));
        assert!(request.contains(&format!("Authorization: Bearer wsb1:{}", "ab".repeat(32))));
        let browser_json = serde_json::to_vec(&actual)
            .unwrap_or_else(|error| unreachable!("browser response: {error}"));
        assert!(!browser_json.windows(6).any(|window| window == b"Bearer"));
        assert!(!browser_json.windows(5).any(|window| window == b"wsb1:"));
    }

    #[test]
    fn live_source_fails_closed_before_connecting_without_a_retained_reference() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        let address = "127.0.0.1:9"
            .parse()
            .unwrap_or_else(|error| unreachable!("fixed address: {error}"));
        let source =
            HttpDaemonActivityPackSource::new(address, Duration::from_millis(1), vault, None);

        assert_eq!(
            source.catalog(),
            Err(ActivityPackProxyErrorV1::AuthorityUnavailable)
        );
    }
}
