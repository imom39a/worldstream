use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::{
    canonical::{CanonicalError, canonical_bytes, canonical_content_bytes},
    model::{AdapterError, DetachedEs256SignatureV1, UnsignedResolverEvidenceV1},
};

#[must_use]
pub fn exact_blake3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// Compute the lowercase SHA-256 A202 content hash.
///
/// # Errors
///
/// Returns an error when the content cannot be encoded by the pinned canonical
/// JSON subset.
pub fn a202_content_hash(value: &Value) -> Result<String, CanonicalError> {
    let bytes = canonical_content_bytes(value)?;
    Ok(hex_lower(&Sha256::digest(bytes)))
}

/// Construct the exact message covered by an A202 detached signature.
///
/// # Errors
///
/// Returns an error when content or protected metadata cannot be encoded by the
/// pinned canonical JSON subset.
pub fn a202_signature_message(
    value: &Value,
    key_id: &str,
    algorithm: &str,
    purpose: &str,
    signed_at: &str,
) -> Result<Vec<u8>, CanonicalError> {
    let mut message = canonical_content_bytes(value)?;
    message.push(b'.');
    message.extend(canonical_bytes(&json!({
        "algorithm": algorithm,
        "key_id": key_id,
        "purpose": purpose,
        "signed_at": signed_at,
    }))?);
    Ok(message)
}

/// Construct the domain-separated resolver-attestation message.
///
/// # Errors
///
/// Returns an error when the evidence or protected signature metadata cannot be
/// canonically encoded.
pub fn resolver_attestation_message(
    evidence: &UnsignedResolverEvidenceV1,
    signature: &DetachedEs256SignatureV1,
) -> Result<Vec<u8>, AdapterError> {
    let value = serde_json::to_value(evidence)
        .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?;
    let mut message = b"worldstream/resolver-evidence-authentication/v1\0".to_vec();
    message.extend(
        canonical_bytes(&value)
            .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?,
    );
    message.push(b'.');
    message.extend(
        canonical_bytes(&json!({
            "algorithm": signature.algorithm,
            "key_id": signature.key_id,
            "purpose": signature.purpose,
            "signed_at": signature.signed_at,
        }))
        .map_err(|error| AdapterError::InvalidCanonicalJson(error.to_string()))?,
    );
    Ok(message)
}

/// Verify a fixed-width ES256 signature against an SEC1 public key.
///
/// # Errors
///
/// Returns [`AdapterError::InvalidSignature`] when the public key, signature,
/// or covered message does not verify.
pub fn verify_es256(
    public_key_sec1: &[u8],
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<(), AdapterError> {
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, public_key_sec1)
        .verify(message, signature_bytes)
        .map_err(|_| AdapterError::InvalidSignature)
}

#[must_use]
pub fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode unpadded URL-safe Base64 material.
///
/// # Errors
///
/// Returns [`AdapterError::InvalidBase64`] when the input is malformed.
pub fn decode_base64url(value: &str) -> Result<Vec<u8>, AdapterError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AdapterError::InvalidBase64)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
