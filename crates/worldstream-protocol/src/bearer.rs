//! Strict transport representation for a capability bearer.
//!
//! The protocol crate intentionally does not own the authority bearer type.
//! It owns only the versioned, bounded wire representation that the gateway
//! can validate before handing the exact bytes to Core.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use zeroize::Zeroizing;

/// Version marker for the capability bearer transport representation.
pub const BEARER_WIRE_PREFIX: &str = "wsb1:";
/// Number of secret bytes carried by a capability bearer.
pub const BEARER_BYTES: usize = 32;
/// Number of lowercase hexadecimal characters in the encoded bearer.
pub const BEARER_HEX_LENGTH: usize = BEARER_BYTES * 2;
/// Exact maximum and minimum length of a canonical bearer wire value.
pub const BEARER_WIRE_LENGTH: usize = BEARER_WIRE_PREFIX.len() + BEARER_HEX_LENGTH;

/// A canonical v1 capability bearer wire value.
///
/// The value is deliberately not serializable or displayable. Callers that
/// need to place it in an authorization header must do so explicitly through
/// [`Self::to_wire`], and diagnostics redact the secret.
pub struct BearerWireV1 {
    bytes: [u8; BEARER_BYTES],
}

impl BearerWireV1 {
    /// Constructs a canonical bearer value from exactly 32 secret bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; BEARER_BYTES]) -> Self {
        Self { bytes }
    }

    /// Parses the exact `wsb1:` lowercase-hex representation.
    ///
    /// # Errors
    ///
    /// Returns a closed error for an incorrect version, length, or alphabet.
    pub fn parse(value: &str) -> Result<Self, BearerWireError> {
        if value.len() != BEARER_WIRE_LENGTH {
            return Err(BearerWireError::InvalidLength);
        }
        if !value.starts_with(BEARER_WIRE_PREFIX) {
            return Err(BearerWireError::UnsupportedVersion);
        }

        let encoded = &value.as_bytes()[BEARER_WIRE_PREFIX.len()..];
        let mut bytes = [0_u8; BEARER_BYTES];
        for (index, pair) in encoded.chunks_exact(2).enumerate() {
            let high = decode_lower_hex(pair[0]).ok_or(BearerWireError::NonCanonicalEncoding)?;
            let low = decode_lower_hex(pair[1]).ok_or(BearerWireError::NonCanonicalEncoding)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self { bytes })
    }

    /// Returns the canonical header value for this bearer.
    #[must_use]
    pub fn to_wire(&self) -> String {
        let mut value = String::with_capacity(BEARER_WIRE_LENGTH);
        value.push_str(BEARER_WIRE_PREFIX);
        for byte in self.bytes {
            value.push(hex_digit(byte >> 4));
            value.push(hex_digit(byte & 0x0f));
        }
        value
    }

    /// Moves the exact bytes to the authority boundary.
    #[must_use]
    pub fn into_bytes(mut self) -> [u8; BEARER_BYTES] {
        std::mem::replace(&mut self.bytes, [0_u8; BEARER_BYTES])
    }
}

impl FromStr for BearerWireV1 {
    type Err = BearerWireError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for BearerWireV1 {
    type Error = BearerWireError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl fmt::Debug for BearerWireV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerWireV1(REDACTED)")
    }
}

impl Drop for BearerWireV1 {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

/// Zeroizing serde transport for one caller-sealed Capability bearer.
///
/// Serialization is deliberately supported only because this value crosses
/// the authenticated daemon request boundary. Debug and error paths always
/// redact it, and the retained allocation is zeroized on drop.
pub struct SealedCapabilityBearerV1(Zeroizing<String>);

impl SealedCapabilityBearerV1 {
    /// Seals one already validated bearer wire value.
    #[must_use]
    pub fn from_wire(bearer: &BearerWireV1) -> Self {
        Self(Zeroizing::new(bearer.to_wire()))
    }

    /// Parses and immediately wraps an owned wire allocation for zeroization.
    ///
    /// # Errors
    ///
    /// Returns a closed bearer error without retaining or printing the input.
    pub fn parse(value: String) -> Result<Self, BearerWireError> {
        let value = Zeroizing::new(value);
        BearerWireV1::parse(value.as_str())?;
        Ok(Self(value))
    }

    /// Borrows the exact wire value only for the bounded transport boundary.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Revalidates and returns the zeroizing binary bearer representation.
    ///
    /// # Errors
    ///
    /// Returns a closed error if memory corruption changed the retained wire.
    pub fn wire(&self) -> Result<BearerWireV1, BearerWireError> {
        BearerWireV1::parse(self.0.as_str())
    }
}

impl fmt::Debug for SealedCapabilityBearerV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SealedCapabilityBearerV1(REDACTED)")
    }
}

impl Serialize for SealedCapabilityBearerV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for SealedCapabilityBearerV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Closed parsing failures for the bearer wire representation.
///
/// Error values intentionally never retain or print the supplied bearer.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BearerWireError {
    /// The value is not exactly the bounded v1 length.
    #[error("bearer wire value has an invalid length")]
    InvalidLength,
    /// The value does not carry the supported version marker.
    #[error("bearer wire value has an unsupported version")]
    UnsupportedVersion,
    /// The secret is not canonical lowercase hexadecimal.
    #[error("bearer wire value is not canonical lowercase hexadecimal")]
    NonCanonicalEncoding,
}

fn decode_lower_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + value - 10),
        _ => unreachable!("hex digit is four bits"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BEARER_WIRE_LENGTH, BEARER_WIRE_PREFIX, BearerWireError, BearerWireV1,
        SealedCapabilityBearerV1,
    };

    const WIRE: &str = "wsb1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    #[test]
    fn canonical_wire_round_trips_exactly() {
        let bearer = BearerWireV1::parse(WIRE)
            .unwrap_or_else(|error| unreachable!("canonical bearer: {error}"));
        assert_eq!(bearer.to_wire(), WIRE);
        assert_eq!(
            bearer.into_bytes(),
            [
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
                0x1c, 0x1d, 0x1e, 0x1f,
            ]
        );
    }

    #[test]
    fn byte_constructor_emits_versioned_canonical_wire() {
        let bearer = BearerWireV1::from_bytes([0xab; 32]);
        assert_eq!(
            bearer.to_wire(),
            format!("{BEARER_WIRE_PREFIX}{}", "ab".repeat(32))
        );
    }

    #[test]
    fn malformed_bearers_fail_closed_without_echoing_secret() {
        let secret = "ab".repeat(32);
        let cases = [
            "",
            "wsb0:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "WSB1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "wsb1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1g",
            "wsb1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f ",
        ];
        for value in cases {
            let Err(error) = BearerWireV1::parse(value) else {
                unreachable!("malformed bearer was accepted")
            };
            assert!(!error.to_string().contains(&secret));
        }
        assert_eq!(WIRE.len(), BEARER_WIRE_LENGTH);
        assert!(matches!(
            BearerWireV1::parse(&format!("wsb1:{secret}x")),
            Err(BearerWireError::InvalidLength)
        ));
    }

    #[test]
    fn diagnostics_redact_bearer_bytes() {
        let bearer = BearerWireV1::parse(WIRE)
            .unwrap_or_else(|error| unreachable!("canonical bearer: {error}"));
        let debug = format!("{bearer:?}");
        assert_eq!(debug, "BearerWireV1(REDACTED)");
        assert!(!debug.contains("00010203"));
    }

    #[test]
    fn sealed_serde_transport_serializes_only_at_the_explicit_boundary_and_redacts_debug() {
        let sealed = SealedCapabilityBearerV1::parse(WIRE.to_owned())
            .unwrap_or_else(|error| unreachable!("sealed bearer: {error}"));
        assert_eq!(
            serde_json::to_string(&sealed)
                .unwrap_or_else(|error| unreachable!("serialize sealed bearer: {error}")),
            format!("\"{WIRE}\"")
        );
        let debug = format!("{sealed:?}");
        assert_eq!(debug, "SealedCapabilityBearerV1(REDACTED)");
        assert!(!debug.contains("00010203"));
        assert_eq!(
            sealed
                .wire()
                .unwrap_or_else(|error| unreachable!("retained bearer: {error}"))
                .to_wire(),
            WIRE
        );
    }

    #[test]
    fn sealed_serde_transport_rejects_noncanonical_input_without_echoing_it() {
        let invalid = format!("\"{}\"", WIRE.to_uppercase());
        let error = serde_json::from_str::<SealedCapabilityBearerV1>(&invalid)
            .err()
            .unwrap_or_else(|| unreachable!("invalid sealed bearer was accepted"));
        assert!(!error.to_string().contains("00010203"));
    }
}
