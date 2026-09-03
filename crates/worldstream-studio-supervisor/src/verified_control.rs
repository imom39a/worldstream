//! Same-connection local endpoint proof before disclosure of control authority.
//!
//! Endpoint proof is scoped to fixed loopback listeners, excluding privileged
//! network interception or a process able to read the installation owner's keys.

use std::{
    io::{self, BufRead as _, BufReader, Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::process_ownership::{OwnershipError, ProcessOwnership, ProcessRole};

const PROOF_SCHEMA: &str = "worldstream/local-process-proof/v1";
const PROOF_PATH: &str = "/api/v1/control/proof";
const MAX_BODY: usize = 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;

/// Closed protocol failures; remote bytes, credentials, and filesystem paths are absent.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ControlTransportError {
    /// No verified current generation is available.
    #[error("managed endpoint ownership is unavailable")]
    Unavailable,
    /// The response did not prove the expected endpoint and generation.
    #[error("managed endpoint proof was rejected")]
    Rejected,
    /// Request or response violates the bounded fixed protocol.
    #[error("managed control protocol is invalid or incomplete")]
    Protocol,
}

impl From<OwnershipError> for ControlTransportError {
    fn from(_: OwnershipError) -> Self {
        Self::Unavailable
    }
}

pub(crate) struct ProofIdentity {
    pub(crate) installation: String,
    pub(crate) role: ProcessRole,
    pub(crate) generation: String,
    pub(crate) pid: u32,
    pub(crate) endpoint: SocketAddr,
    pub(crate) key: Zeroizing<[u8; 32]>,
}

/// Fresh, non-secret challenge sent without any bearer authority.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofRequest {
    /// Exactly 32 random bytes encoded as lowercase hexadecimal.
    pub nonce: String,
}

/// Signed public process identity; contains no reusable authority or private key.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofDocument {
    schema: String,
    installation: String,
    role: ProcessRole,
    generation: String,
    pid: u32,
    endpoint: SocketAddr,
    nonce: String,
    mac: String,
}

struct LiveProof {
    identity: ProofIdentity,
    enabled: AtomicBool,
}

/// Cloneable proof handler tied to the owning process's retained lifetime lease.
/// The application must supply accepted-socket local address, never HTTP Host.
#[derive(Clone)]
pub struct ProofService(Arc<LiveProof>);

impl ProofService {
    pub(crate) fn new(identity: ProofIdentity) -> Self {
        Self(Arc::new(LiveProof {
            identity,
            enabled: AtomicBool::new(true),
        }))
    }

    pub(crate) fn disable(&self) {
        self.0.enabled.store(false, Ordering::SeqCst);
    }

    /// Irreversibly disables proof and derived Runtime admission for all clones.
    /// Call before releasing the listener, retaining the process lease through
    /// connection drain and storage/task teardown. This does not release the lease.
    pub fn disable_for_shutdown(&self) {
        self.disable();
    }

    /// Admits only the live Runtime generation's derived control credential.
    /// This is exclusively for managed status/stop handlers, never Host or Room routes.
    #[must_use]
    pub fn authenticate_runtime(&self, headers: &HeaderMap) -> bool {
        if self.0.identity.role != ProcessRole::Runtime
            || !self.0.enabled.load(Ordering::SeqCst)
            || self.0.identity.pid != std::process::id()
        {
            return false;
        }
        let mut supplied = headers.get_all("authorization").iter();
        let Some(header) = supplied.next() else {
            return false;
        };
        if supplied.next().is_some() {
            return false;
        }
        let Ok(text) = header.to_str() else {
            return false;
        };
        let Some(token) = text.strip_prefix("Bearer ") else {
            return false;
        };
        let Ok(candidate) = decode_hex(token) else {
            return false;
        };
        blake3::Hash::from(candidate) == runtime_mac(&self.0.identity)
    }

    /// Answers only for this live generation's exact actual listening address.
    ///
    /// # Errors
    /// Rejects malformed challenges, disabled proof, wrong processes, and other sockets.
    pub fn respond(
        &self,
        request: &ProofRequest,
        actual_local_addr: SocketAddr,
    ) -> Result<ProofDocument, ControlTransportError> {
        let identity = &self.0.identity;
        if !self.0.enabled.load(Ordering::SeqCst)
            || std::process::id() != identity.pid
            || actual_local_addr != identity.endpoint
        {
            return Err(ControlTransportError::Rejected);
        }
        decode_hex(&request.nonce)?;
        let document = ProofDocument {
            schema: PROOF_SCHEMA.to_owned(),
            installation: identity.installation.clone(),
            role: identity.role,
            generation: identity.generation.clone(),
            pid: identity.pid,
            endpoint: actual_local_addr,
            nonce: request.nonce.clone(),
            mac: proof_mac(identity, &request.nonce).to_hex().to_string(),
        };
        Ok(document)
    }
}

/// Bounded authenticated response. Deliberately has no raw-body Debug formatter.
pub struct ControlResponse {
    /// HTTP status from the verified peer.
    pub status: u16,
    /// At most one MiB; typed callers decide which fields may be displayed.
    pub body: Vec<u8>,
}

/// A sealed, already-proved TCP stream; never reconnects or follows redirects.
pub struct VerifiedConnection {
    stream: BufReader<DeadlineStream>,
    identity: ProofIdentity,
}

// Update the OS timeout before every underlying read/write, not merely once
// before a read_line/write_all loop. Buffered progress cannot reset the deadline.
struct DeadlineStream {
    stream: TcpStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))
    }
}

impl io::Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}

impl io::Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.flush()
    }
}

impl VerifiedConnection {
    /// Opens the recorded literal loopback endpoint and proves it before authority.
    /// No DNS lookup, proxy, redirects, or caller-selected endpoint are supported.
    ///
    /// # Errors
    /// Rejects unavailable records/connections and any incomplete or invalid proof.
    pub fn connect(
        ownership: &ProcessOwnership,
        role: ProcessRole,
        timeout: Duration,
    ) -> Result<Self, ControlTransportError> {
        let started = Instant::now();
        let identity = ownership.proof_identity(role)?;
        if timeout.is_zero() || timeout > Duration::from_mins(5) {
            return Err(ControlTransportError::Protocol);
        }
        let deadline = started + timeout;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or(ControlTransportError::Unavailable)?;
        let stream = TcpStream::connect_timeout(&identity.endpoint, remaining)
            .map_err(|_| ControlTransportError::Unavailable)?;
        let mut connection = Self {
            stream: BufReader::new(DeadlineStream { stream, deadline }),
            identity,
        };
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce).map_err(|_| ControlTransportError::Unavailable)?;
        let request = ProofRequest {
            nonce: blake3::Hash::from(nonce).to_hex().to_string(),
        };
        let body = serde_json::to_vec(&request).map_err(|_| ControlTransportError::Protocol)?;
        connection.send("POST", PROOF_PATH, &body, None)?;
        let response = connection.receive(4096)?;
        if response.status != 200 {
            return Err(ControlTransportError::Rejected);
        }
        let proof: ProofDocument =
            serde_json::from_slice(&response.body).map_err(|_| ControlTransportError::Rejected)?;
        let expected = &connection.identity;
        if proof.schema != PROOF_SCHEMA
            || proof.installation != expected.installation
            || proof.role != expected.role
            || proof.generation != expected.generation
            || proof.pid != expected.pid
            || proof.endpoint != expected.endpoint
            || proof.nonce != request.nonce
            || blake3::Hash::from(decode_hex(&proof.mac)?) != proof_mac(expected, &request.nonce)
        {
            return Err(ControlTransportError::Rejected);
        }
        Ok(connection)
    }

    /// Sends one bounded GET/POST on the proved stream, invoking the sensitive
    /// credential supplier only after proof. A consumed connection is never retried.
    ///
    /// # Errors
    /// Rejects invalid requests, unavailable credentials, and incomplete responses.
    pub fn request<E>(
        mut self,
        method: &str,
        path: &str,
        body: &[u8],
        authorization: impl FnOnce() -> Result<HeaderValue, E>,
    ) -> Result<ControlResponse, ControlTransportError> {
        validate_request(method, path, body)?;
        let authorization = authorization().map_err(|_| ControlTransportError::Unavailable)?;
        self.send(method, path, body, Some(&authorization))?;
        self.receive(MAX_BODY)
    }

    /// Sends one request with a stricter whole-response limit, including status
    /// and header bytes. The default request contract remains unchanged.
    ///
    /// # Errors
    /// Rejects invalid limits and responses whose declared body exceeds the
    /// allowance remaining after headers, before allocating or reading that body.
    pub fn request_bounded<E>(
        mut self,
        method: &str,
        path: &str,
        body: &[u8],
        maximum_response_bytes: usize,
        authorization: impl FnOnce() -> Result<HeaderValue, E>,
    ) -> Result<ControlResponse, ControlTransportError> {
        if maximum_response_bytes == 0 || maximum_response_bytes > MAX_BODY + MAX_HEADERS {
            return Err(ControlTransportError::Protocol);
        }
        validate_request(method, path, body)?;
        let authorization = authorization().map_err(|_| ControlTransportError::Unavailable)?;
        self.send(method, path, body, Some(&authorization))?;
        self.receive_bounded(MAX_BODY, Some(maximum_response_bytes))
    }

    /// Sends only managed Runtime status, Pack facts or stop using generation
    /// authority. The private generation key and derived header are never exposed.
    ///
    /// # Errors
    /// Rejects controller identities, other routes, and incomplete requests/responses.
    pub fn request_runtime(
        self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<ControlResponse, ControlTransportError> {
        if self.identity.role != ProcessRole::Runtime
            || !matches!(
                (method, path),
                ("GET", "/api/v1/control/status" | "/api/v1/control/packs")
                    | ("POST", "/api/v1/control/stop")
            )
        {
            return Err(ControlTransportError::Rejected);
        }
        let token = Zeroizing::new(format!("Bearer {}", runtime_mac(&self.identity).to_hex()));
        let mut header =
            HeaderValue::from_str(&token).map_err(|_| ControlTransportError::Protocol)?;
        header.set_sensitive(true);
        self.request(method, path, body, || {
            Ok::<_, ControlTransportError>(header)
        })
    }

    fn send(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
        authorization: Option<&HeaderValue>,
    ) -> Result<(), ControlTransportError> {
        validate_request(method, path, body)?;
        let mut encoded = Zeroizing::new(Vec::new());
        write!(&mut *encoded, "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", self.identity.endpoint, body.len()).map_err(|_| ControlTransportError::Protocol)?;
        if let Some(header) = authorization {
            if header
                .as_bytes()
                .iter()
                .any(|byte| matches!(byte, b'\r' | b'\n'))
            {
                return Err(ControlTransportError::Protocol);
            }
            encoded.extend_from_slice(b"Authorization: ");
            encoded.extend_from_slice(header.as_bytes());
            encoded.extend_from_slice(b"\r\n");
        }
        encoded.extend_from_slice(b"\r\n");
        encoded.extend_from_slice(body);
        self.stream
            .get_mut()
            .write_all(&encoded)
            .map_err(|_| ControlTransportError::Unavailable)
    }

    fn receive(&mut self, maximum: usize) -> Result<ControlResponse, ControlTransportError> {
        self.receive_bounded(maximum, None)
    }

    fn receive_bounded(
        &mut self,
        maximum: usize,
        maximum_wire: Option<usize>,
    ) -> Result<ControlResponse, ControlTransportError> {
        let mut used = 0;
        let line = read_line(&mut self.stream, &mut used)?;
        let mut fields = line.split_whitespace();
        if fields.next() != Some("HTTP/1.1") {
            return Err(ControlTransportError::Protocol);
        }
        let status: u16 = fields
            .next()
            .ok_or(ControlTransportError::Protocol)?
            .parse()
            .map_err(|_| ControlTransportError::Protocol)?;
        if !(200..=599).contains(&status) {
            return Err(ControlTransportError::Protocol);
        }
        let mut length = None;
        loop {
            let header = read_line(&mut self.stream, &mut used)?;
            if header == "\r\n" {
                break;
            }
            let (name, value) = header
                .trim_end()
                .split_once(':')
                .ok_or(ControlTransportError::Protocol)?;
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err(ControlTransportError::Protocol);
            }
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err(ControlTransportError::Protocol);
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| ControlTransportError::Protocol)?,
                );
            }
        }
        let length = if matches!(status, 204 | 304) {
            0
        } else {
            length.ok_or(ControlTransportError::Protocol)?
        };
        let allowance = maximum_wire
            .map(|limit| {
                limit
                    .checked_sub(used)
                    .ok_or(ControlTransportError::Protocol)
            })
            .transpose()?
            .map_or(maximum, |remaining| remaining.min(maximum));
        if length > allowance {
            return Err(ControlTransportError::Protocol);
        }
        let mut body = vec![0_u8; length];
        self.stream
            .read_exact(&mut body)
            .map_err(|_| ControlTransportError::Protocol)?;
        Ok(ControlResponse { status, body })
    }
}

fn validate_request(method: &str, path: &str, body: &[u8]) -> Result<(), ControlTransportError> {
    if !matches!(method, "GET" | "POST")
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.len() > 4096
        || !path.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        || path.contains('#')
        || body.len() > MAX_BODY
    {
        return Err(ControlTransportError::Protocol);
    }
    Ok(())
}

fn read_line(
    reader: &mut BufReader<DeadlineStream>,
    used: &mut usize,
) -> Result<String, ControlTransportError> {
    let remaining = MAX_HEADERS
        .checked_sub(*used)
        .ok_or(ControlTransportError::Protocol)?;
    let mut line = String::new();
    let count = reader
        .take(remaining as u64)
        .read_line(&mut line)
        .map_err(|_| ControlTransportError::Protocol)?;
    *used += count;
    if count == 0 || !line.ends_with("\r\n") {
        return Err(ControlTransportError::Protocol);
    }
    Ok(line)
}

fn proof_mac(identity: &ProofIdentity, nonce: &str) -> blake3::Hash {
    let mut mac = blake3::Hasher::new_keyed(&identity.key);
    mac.update(b"worldstream/local-process-proof/v1\0");
    for field in [
        identity.installation.as_str(),
        match identity.role {
            ProcessRole::Controller => "controller",
            ProcessRole::Runtime => "runtime",
        },
        identity.generation.as_str(),
        &identity.pid.to_string(),
        &identity.endpoint.to_string(),
        nonce,
    ] {
        mac.update(&(field.len() as u64).to_le_bytes());
        mac.update(field.as_bytes());
    }
    mac.finalize()
}

fn runtime_mac(identity: &ProofIdentity) -> blake3::Hash {
    let mut mac = blake3::Hasher::new_keyed(&identity.key);
    mac.update(b"worldstream/runtime-control/v1\0");
    for field in [
        identity.installation.as_str(),
        identity.generation.as_str(),
        &identity.pid.to_string(),
        &identity.endpoint.to_string(),
    ] {
        mac.update(&(field.len() as u64).to_le_bytes());
        mac.update(field.as_bytes());
    }
    mac.finalize()
}

fn decode_hex(value: &str) -> Result<[u8; 32], ControlTransportError> {
    if value.len() != 64 {
        return Err(ControlTransportError::Rejected);
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(ControlTransportError::Rejected),
        };
        bytes[index] = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Ok(bytes)
}
