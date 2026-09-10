//! Fixed result-indexer transport for hosted terminal and Replay evidence.
//!
//! The transport can read only the current authorized Projection and Replay
//! through one retained result-indexer bearer. It is not a generic Host proxy.

use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use axum::http::HeaderValue;
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use worldstream_core::{CanonicalJsonV1, projection_hash_for_canonical_bytes};
use worldstream_hosted_contract::{
    HostedAuthorizedPublicProjectionV1, HostedResultIntegrityStatusV1,
    HostedResultReplayEvidenceV1, HostedResultSourceHeadV1,
};
use worldstream_protocol::{
    BearerWireV1, MAX_MESSAGE_BYTES, PackReference, Projection, ProjectionResponse, ReplayResponse,
    RoomHead,
};
use zeroize::Zeroizing;

use crate::{
    managed_daemon_transport::ManagedDaemonTransport,
    process_ownership::ProcessOwnership,
    room_setup_operations::RoomSetupResultIndexerBindingV1,
    secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
};

const MAX_RESULT_SOURCE_BYTES: usize = 512 * 1024;
const MAX_RESULT_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounded hosted evidence-read budget, separate from lightweight health probes.
pub const HOSTED_RESULT_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);

/// Closed Host-side result-source failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HostedResultSourceErrorV1 {
    #[error("result-source evidence is invalid")]
    Invalid,
    #[error("result-indexer authority is unavailable")]
    AuthorityUnavailable,
    #[error("result-source transport is unavailable")]
    Unavailable,
}

/// Projection and optional Replay facts derived through one exact Membership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostedResultObservationV1 {
    pub room_id: String,
    pub member_id: String,
    pub pack: PackReference,
    pub source_head: HostedResultSourceHeadV1,
    pub integrity_status: HostedResultIntegrityStatusV1,
    pub integrity_generation: u64,
    pub projection_schema: String,
    pub public_projection: HostedAuthorizedPublicProjectionV1,
    pub projection_hash: String,
    pub replay: Option<HostedResultReplayEvidenceV1>,
}

/// Literal loopback Runtime client for result-indexer evidence.
#[derive(Clone, Debug)]
pub struct HttpHostedResultSourceV1 {
    address: SocketAddr,
    timeout: Duration,
    vault: FileSecretVaultV1,
    managed: Option<ManagedDaemonTransport>,
}

impl HttpHostedResultSourceV1 {
    /// Configures one fixed local Runtime endpoint.
    ///
    /// # Errors
    /// Rejects non-loopback endpoints and unsafe timeout values.
    pub fn new(
        address: SocketAddr,
        timeout: Duration,
        vault: FileSecretVaultV1,
    ) -> Result<Self, HostedResultSourceErrorV1> {
        validate_configuration(address, timeout)?;
        Ok(Self {
            address,
            timeout,
            vault,
            managed: None,
        })
    }

    /// Adds same-stream managed Runtime ownership proof.
    ///
    /// # Errors
    /// Rejects non-loopback endpoints and unsafe timeout values.
    pub fn new_managed(
        address: SocketAddr,
        timeout: Duration,
        vault: FileSecretVaultV1,
        ownership: ProcessOwnership,
    ) -> Result<Self, HostedResultSourceErrorV1> {
        let mut source = Self::new(address, timeout, vault)?;
        source.managed = Some(ManagedDaemonTransport::new(ownership, address, timeout));
        Ok(source)
    }

    pub(crate) fn read(
        &self,
        binding: &RoomSetupResultIndexerBindingV1,
        include_replay: bool,
    ) -> Result<HostedResultObservationV1, HostedResultSourceErrorV1> {
        let projection_path = format!("/v1/rooms/{}/projection", binding.room_id);
        let projection =
            self.required_json::<ProjectionResponse>(&projection_path, &binding.secret_reference)?;
        validate_projection_response(binding, &projection)?;
        let current_public_projection = authorized_projection(&projection)?;
        // The Runtime freezes the hash over the complete authorized Pack view.
        // The hosted wrapper reconstructs that exact shape from the protocol
        // Projection by restoring its projection schema and wire field names.
        let projection_bytes = canonical_bytes(&current_public_projection)?;
        let current_projection_hash = projection_hash_for_canonical_bytes(&projection_bytes)
            .map_err(|_| HostedResultSourceErrorV1::Invalid)?
            .to_string();
        if current_projection_hash != projection.projection_hash {
            return Err(HostedResultSourceErrorV1::Invalid);
        }
        let integrity_status = integrity_status(&projection.room_health)?;
        let source_head = hosted_head(&projection.room_head);
        let replay_path = format!(
            "/v1/rooms/{}/replay?at_room_seq={}",
            binding.room_id, projection.room_head.room_seq
        );
        // Private terminal disposition needs current authorized public evidence.
        // Replay is a separate publication proof and can use a different
        // historical schema; do not fetch or reconstruct it for private Runs.
        let replay = if include_replay {
            self.optional_replay(&replay_path, &binding.secret_reference)?
        } else {
            None
        };
        let (public_projection, projection_hash, replay) = if let Some(replay) = replay {
            validate_replay_response(binding, &projection, &replay)?;
            let replay_public_projection =
                authorized_projection_parts(&projection.projection_schema, &replay.projection)?;
            let replay_projection_bytes = canonical_bytes(&replay_public_projection)?;
            let replay_projection_hash =
                projection_hash_for_canonical_bytes(&replay_projection_bytes)
                    .map_err(|_| HostedResultSourceErrorV1::Invalid)?
                    .to_string();
            if replay_projection_hash != replay.projection_hash {
                return Err(HostedResultSourceErrorV1::Invalid);
            }
            let receipt = tagged_sha256(&canonical_bytes(&replay)?);
            let replay_evidence = HostedResultReplayEvidenceV1 {
                verifier_revision: "worldstream.authorized-replay/v1".to_owned(),
                verified_head: hosted_head(&replay.room_head),
                projection_hash: replay.projection_hash,
                verification_receipt_digest: receipt,
            };
            (
                replay_public_projection,
                replay_projection_hash,
                Some(replay_evidence),
            )
        } else {
            (current_public_projection, current_projection_hash, None)
        };
        Ok(HostedResultObservationV1 {
            room_id: binding.room_id.clone(),
            member_id: binding.member_id.clone(),
            pack: binding.pack.clone(),
            source_head,
            integrity_status,
            integrity_generation: projection.integrity_generation,
            projection_schema: projection.projection_schema,
            public_projection,
            projection_hash,
            replay,
        })
    }

    fn required_json<T: DeserializeOwned>(
        &self,
        path: &str,
        reference: &SecretReferenceV1,
    ) -> Result<T, HostedResultSourceErrorV1> {
        let response = self.request(path, reference)?;
        match response.status {
            200 => serde_json::from_slice(&response.body)
                .map_err(|_| HostedResultSourceErrorV1::Invalid),
            401 | 403 => Err(HostedResultSourceErrorV1::AuthorityUnavailable),
            400 | 404 | 409 | 422 => Err(HostedResultSourceErrorV1::Invalid),
            _ => Err(HostedResultSourceErrorV1::Unavailable),
        }
    }

    fn optional_replay(
        &self,
        path: &str,
        reference: &SecretReferenceV1,
    ) -> Result<Option<ReplayResponse>, HostedResultSourceErrorV1> {
        let response = self.request(path, reference)?;
        match response.status {
            200 => serde_json::from_slice(&response.body)
                .map(Some)
                .map_err(|_| HostedResultSourceErrorV1::Invalid),
            401 | 403 => Err(HostedResultSourceErrorV1::AuthorityUnavailable),
            400 | 404 | 422 => Err(HostedResultSourceErrorV1::Invalid),
            409 | 423 | 425 | 429 | 500 | 502 | 503 | 504 => Ok(None),
            _ => Err(HostedResultSourceErrorV1::Unavailable),
        }
    }

    fn request(
        &self,
        path: &str,
        reference: &SecretReferenceV1,
    ) -> Result<BoundedResponse, HostedResultSourceErrorV1> {
        if let Some(managed) = &self.managed {
            let response = managed
                .request("GET", path, &[], MAX_RESULT_SOURCE_BYTES, || {
                    self.authorization(reference)
                })
                .map_err(|_| HostedResultSourceErrorV1::Unavailable)?;
            return Ok(BoundedResponse {
                status: response.status,
                body: response.body,
            });
        }
        let authorization = self.authorization(reference)?;
        let authorization = authorization
            .to_str()
            .map_err(|_| HostedResultSourceErrorV1::AuthorityUnavailable)?;
        let request = Zeroizing::new(format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nAuthorization: {authorization}\r\nConnection: close\r\n\r\n",
            self.address
        ));
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| HostedResultSourceErrorV1::Unavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .and_then(|()| stream.write_all(request.as_bytes()))
            .map_err(|_| HostedResultSourceErrorV1::Unavailable)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_RESULT_SOURCE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut response)
            .map_err(|_| HostedResultSourceErrorV1::Unavailable)?;
        if response.len() > MAX_RESULT_SOURCE_BYTES {
            return Err(HostedResultSourceErrorV1::Invalid);
        }
        parse_http_response(&response)
    }

    fn authorization(
        &self,
        reference: &SecretReferenceV1,
    ) -> Result<HeaderValue, HostedResultSourceErrorV1> {
        let secret = self
            .vault
            .resolve(SecretKindV1::MembershipAuthority, reference)
            .map_err(|_| HostedResultSourceErrorV1::AuthorityUnavailable)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| HostedResultSourceErrorV1::AuthorityUnavailable)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let value = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
        let mut header = HeaderValue::from_str(&value)
            .map_err(|_| HostedResultSourceErrorV1::AuthorityUnavailable)?;
        header.set_sensitive(true);
        Ok(header)
    }
}

#[derive(Debug)]
struct BoundedResponse {
    status: u16,
    body: Vec<u8>,
}

fn validate_configuration(
    address: SocketAddr,
    timeout: Duration,
) -> Result<(), HostedResultSourceErrorV1> {
    if !address.ip().is_loopback() || timeout.is_zero() || timeout > MAX_RESULT_SOURCE_TIMEOUT {
        return Err(HostedResultSourceErrorV1::Invalid);
    }
    Ok(())
}

fn validate_projection_response(
    binding: &RoomSetupResultIndexerBindingV1,
    response: &ProjectionResponse,
) -> Result<(), HostedResultSourceErrorV1> {
    if response.room_id != binding.room_id
        || response.room_head.room_id != binding.room_id
        || response.room_head.pack_digest != binding.pack.digest
        || response.projection_schema.is_empty()
        || response.projection_schema.len() > 128
        || response.integrity_generation > 9_007_199_254_740_991
    {
        return Err(HostedResultSourceErrorV1::Invalid);
    }
    let _ = integrity_status(&response.room_health)?;
    Ok(())
}

fn validate_replay_response(
    binding: &RoomSetupResultIndexerBindingV1,
    projection: &ProjectionResponse,
    replay: &ReplayResponse,
) -> Result<(), HostedResultSourceErrorV1> {
    if replay.room_id != binding.room_id
        || replay.pack != binding.pack
        || replay.requested_room_seq != projection.room_head.room_seq
        || replay.room_head != projection.room_head
        || !replay_core_corresponds(&projection.projection.core, &replay.projection.core)
        || replay.projection.activity != projection.projection.activity
        || replay.projection.action_offers != projection.projection.action_offers
        || replay.verification != "verified"
        || replay.room_health != projection.room_health
        || replay.integrity_generation != projection.integrity_generation
    {
        return Err(HostedResultSourceErrorV1::Invalid);
    }
    Ok(())
}

fn replay_core_corresponds(current: &serde_json::Value, replay: &serde_json::Value) -> bool {
    const SHARED_FIELDS: [&str; 4] = ["access_mode", "role", "room_status", "standing"];
    let (Some(current), Some(replay)) = (current.as_object(), replay.as_object()) else {
        return false;
    };
    current.len() == 5
        && replay.len() == 5
        && current
            .get("viewer_class")
            .and_then(serde_json::Value::as_str)
            == Some("public")
        && replay
            .get("viewer_class")
            .and_then(serde_json::Value::as_str)
            == Some("historical")
        && SHARED_FIELDS
            .iter()
            .all(|field| current.contains_key(*field) && current.get(*field) == replay.get(*field))
}

fn authorized_projection(
    response: &ProjectionResponse,
) -> Result<HostedAuthorizedPublicProjectionV1, HostedResultSourceErrorV1> {
    authorized_projection_parts(&response.projection_schema, &response.projection)
}

fn authorized_projection_parts(
    projection_schema: &str,
    projection: &Projection,
) -> Result<HostedAuthorizedPublicProjectionV1, HostedResultSourceErrorV1> {
    let action_offers = projection
        .action_offers
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| HostedResultSourceErrorV1::Invalid)?;
    Ok(HostedAuthorizedPublicProjectionV1 {
        projection_schema: projection_schema.to_owned(),
        authorized_core: projection.core.clone(),
        projection: projection.activity.clone(),
        action_offers,
    })
}

fn hosted_head(head: &RoomHead) -> HostedResultSourceHeadV1 {
    HostedResultSourceHeadV1 {
        room_id: head.room_id.clone(),
        room_seq: head.room_seq,
        genesis_or_transition_hash: head.genesis_or_transition_hash.clone(),
        core_schema_version: head.core_schema_version.clone(),
        pack_digest: head.pack_digest.clone(),
        core_state_hash: head.core_state_hash.clone(),
        activity_state_hash: head.activity_state_hash.clone(),
        authoritative_state_hash: head.authoritative_state_hash.clone(),
    }
}

fn integrity_status(
    status: &str,
) -> Result<HostedResultIntegrityStatusV1, HostedResultSourceErrorV1> {
    match status {
        "healthy" => Ok(HostedResultIntegrityStatusV1::Healthy),
        "faulted" => Ok(HostedResultIntegrityStatusV1::Faulted),
        "quarantined" => Ok(HostedResultIntegrityStatusV1::Quarantined),
        _ => Err(HostedResultSourceErrorV1::Invalid),
    }
}

fn canonical_bytes(value: &impl serde::Serialize) -> Result<Vec<u8>, HostedResultSourceErrorV1> {
    let bytes = serde_json::to_vec(value).map_err(|_| HostedResultSourceErrorV1::Invalid)?;
    let canonical =
        CanonicalJsonV1::parse(&bytes).map_err(|_| HostedResultSourceErrorV1::Invalid)?;
    let bytes = canonical
        .to_bytes()
        .map_err(|_| HostedResultSourceErrorV1::Invalid)?;
    if bytes.len() > MAX_RESULT_SOURCE_BYTES {
        return Err(HostedResultSourceErrorV1::Invalid);
    }
    Ok(bytes)
}

fn tagged_sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_lower(&Sha256::digest(bytes)))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn parse_http_response(response: &[u8]) -> Result<BoundedResponse, HostedResultSourceErrorV1> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(HostedResultSourceErrorV1::Invalid)?;
    let head =
        std::str::from_utf8(&response[..split]).map_err(|_| HostedResultSourceErrorV1::Invalid)?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(HostedResultSourceErrorV1::Invalid)?;
    let body = response
        .get(split + 4..)
        .ok_or(HostedResultSourceErrorV1::Invalid)?
        .to_vec();
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(HostedResultSourceErrorV1::Invalid);
    }
    Ok(BoundedResponse { status, body })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use super::{HOSTED_RESULT_SOURCE_TIMEOUT, HttpHostedResultSourceV1, replay_core_corresponds};
    use crate::secrets::{FileSecretVaultV1, SecretKindV1};

    #[test]
    fn private_current_archive_evidence_uses_the_exact_listing_projection() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))?;
        let reference = vault.store(SecretKindV1::MembershipAuthority, &[0xab; 32])?;
        let room_id = "01JY0000000000000000000000";
        let listing_bytes = worldstream_core::CanonicalJsonV1::parse(include_bytes!(
            "../../../config/hosted/listings/midnight-archive-0.2.0.json"
        ))?
        .to_bytes()?;
        let listing =
            worldstream_hosted_contract::ListingRevision::from_canonical_bytes(&listing_bytes)?;
        let pack = listing.pack();
        let digest = pack.digest.clone();
        let projection_schema = listing.public_projection_schema().to_owned();
        let authorized = json!({
            "projection_schema": projection_schema,
            "authorized_core": {"access_mode":"spectator","role":null,"room_status":"active","standing":"enabled","viewer_class":"public"},
            "projection": {"phase":"expired","lifecycle":"terminal","session_deadline":"2026-09-10T12:00:00Z",
                "location":"records","turns_used":2,"outcome":null}, "action_offers": []
        });
        let hash = worldstream_core::projection_hash_for_canonical_bytes(&super::canonical_bytes(
            &authorized,
        )?)?;
        let response = serde_json::to_vec(&json!({
            "room_id":room_id,
            "room_head":{"room_id":room_id,"room_seq":0,"genesis_or_transition_hash":digest,"core_schema_version":"worldstream.core-room-state.v1","pack_digest":digest,"core_state_hash":digest,"activity_state_hash":digest,"authoritative_state_hash":digest},
            "room_health":"healthy","integrity_generation":1,
            "projection_schema":authorized["projection_schema"],
            "projection":{"core":authorized["authorized_core"],"activity":authorized["projection"],"action_offers":[]},
            "projection_hash":hash.to_string()
        }))?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let worker = thread::spawn(move || -> std::io::Result<()> {
            let (mut socket, _) = listener.accept()?;
            socket.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte)?;
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            assert!(
                String::from_utf8_lossy(&request)
                    .starts_with(&format!("GET /v1/rooms/{room_id}/projection "))
            );
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )?;
            socket.write_all(&response)
            // The listener closes here. Any unnecessary Replay request fails.
        });
        let source = HttpHostedResultSourceV1::new(address, HOSTED_RESULT_SOURCE_TIMEOUT, vault)?;
        let observed = source.read(
            &crate::room_setup_operations::RoomSetupResultIndexerBindingV1 {
                room_id: room_id.to_owned(),
                member_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned(),
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FB2".to_owned(),
                pack: worldstream_protocol::PackReference {
                    id: pack.id.clone(),
                    version: pack.version.clone(),
                    digest,
                },
                secret_reference: reference,
            },
            false,
        )?;
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("mock transport panicked"))??;
        assert!(observed.replay.is_none());
        assert_eq!(observed.projection_schema, projection_schema);
        assert_eq!(
            observed.public_projection.projection_schema,
            listing.public_projection_schema()
        );
        assert_eq!(observed.projection_hash, hash.to_string());
        Ok(())
    }

    #[test]
    fn hosted_replay_can_finish_after_the_old_five_second_deadline() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("fixture: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| panic!("vault: {error}"));
        let reference = vault
            .store(SecretKindV1::MembershipAuthority, &[0xab; 32])
            .unwrap_or_else(|error| panic!("fixture authority: {error}"));
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("listener: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("address: {error}"));
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener
                .accept()
                .unwrap_or_else(|error| panic!("accept: {error}"));
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap_or_else(|error| panic!("timeout: {error}"));
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket
                    .read_exact(&mut byte)
                    .unwrap_or_else(|error| panic!("request: {error}"));
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            thread::sleep(Duration::from_secs(6));
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
        });
        let source = HttpHostedResultSourceV1::new(address, HOSTED_RESULT_SOURCE_TIMEOUT, vault)
            .unwrap_or_else(|error| panic!("source: {error}"));
        let result = source.request("/v1/rooms/fixture/replay?at_room_seq=14", &reference);
        worker
            .join()
            .unwrap_or_else(|_| panic!("fixture server panicked"));
        assert_eq!(
            result
                .unwrap_or_else(|error| panic!("Replay exceeded only the health deadline: {error}"))
                .status,
            200
        );
    }

    #[test]
    fn replay_core_requires_exact_public_to_historical_correspondence() {
        let current = json!({
            "access_mode": "spectator",
            "role": null,
            "room_status": "active",
            "standing": "enabled",
            "viewer_class": "public",
        });
        let replay = json!({
            "access_mode": "spectator",
            "role": null,
            "room_status": "active",
            "standing": "enabled",
            "viewer_class": "historical",
        });
        assert!(replay_core_corresponds(&current, &replay));

        let mut changed_standing = replay.clone();
        changed_standing["standing"] = json!("disabled");
        assert!(!replay_core_corresponds(&current, &changed_standing));

        let mut widened = replay.clone();
        widened["private"] = json!(true);
        assert!(!replay_core_corresponds(&current, &widened));

        let mut missing_current = current.clone();
        let mut missing_replay = replay.clone();
        for value in [&mut missing_current, &mut missing_replay] {
            value
                .as_object_mut()
                .unwrap_or_else(|| panic!("object fixture"))
                .remove("role");
            value["unexpected"] = json!(true);
        }
        assert!(!replay_core_corresponds(&missing_current, &missing_replay));

        let mut wrong_viewer = replay;
        wrong_viewer["viewer_class"] = json!("public");
        assert!(!replay_core_corresponds(&current, &wrong_viewer));
    }
}
