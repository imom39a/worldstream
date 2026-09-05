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
    BearerWireV1, MAX_MESSAGE_BYTES, PackReference, ProjectionResponse, ReplayResponse, RoomHead,
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
    ) -> Result<HostedResultObservationV1, HostedResultSourceErrorV1> {
        let projection_path = format!("/v1/rooms/{}/projection", binding.room_id);
        let projection =
            self.required_json::<ProjectionResponse>(&projection_path, &binding.secret_reference)?;
        validate_projection_response(binding, &projection)?;
        let public_projection = authorized_projection(&projection)?;
        let projection_bytes = canonical_bytes(&public_projection)?;
        let projection_hash = projection_hash_for_canonical_bytes(&projection_bytes)
            .map_err(|_| HostedResultSourceErrorV1::Invalid)?
            .to_string();
        if projection_hash != projection.projection_hash {
            return Err(HostedResultSourceErrorV1::Invalid);
        }
        let integrity_status = integrity_status(&projection.room_health)?;
        let source_head = hosted_head(&projection.room_head);
        let replay_path = format!(
            "/v1/rooms/{}/replay?at_room_seq={}",
            binding.room_id, projection.room_head.room_seq
        );
        let replay = self
            .optional_replay(&replay_path, &binding.secret_reference)?
            .map(|replay| {
                validate_replay_response(binding, &projection, &replay, &projection_hash)?;
                let receipt = tagged_sha256(&canonical_bytes(&replay)?);
                Ok(HostedResultReplayEvidenceV1 {
                    verifier_revision: "worldstream.authorized-replay/v1".to_owned(),
                    verified_head: hosted_head(&replay.room_head),
                    projection_hash: replay.projection_hash,
                    verification_receipt_digest: receipt,
                })
            })
            .transpose()?;
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
    projection_hash: &str,
) -> Result<(), HostedResultSourceErrorV1> {
    if replay.room_id != binding.room_id
        || replay.pack != binding.pack
        || replay.requested_room_seq != projection.room_head.room_seq
        || replay.room_head != projection.room_head
        || replay.projection != projection.projection
        || replay.projection_hash != projection_hash
        || replay.verification != "verified"
        || replay.room_health != projection.room_health
        || replay.integrity_generation != projection.integrity_generation
    {
        return Err(HostedResultSourceErrorV1::Invalid);
    }
    Ok(())
}

fn authorized_projection(
    response: &ProjectionResponse,
) -> Result<HostedAuthorizedPublicProjectionV1, HostedResultSourceErrorV1> {
    let action_offers = response
        .projection
        .action_offers
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| HostedResultSourceErrorV1::Invalid)?;
    Ok(HostedAuthorizedPublicProjectionV1 {
        projection_schema: response.projection_schema.clone(),
        authorized_core: response.projection.core.clone(),
        projection: response.projection.activity.clone(),
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
