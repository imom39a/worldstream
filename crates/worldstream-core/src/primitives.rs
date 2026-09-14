use std::{cmp::Ordering, fmt, num::ParseIntError, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;
use worldstream_protocol::{IdParseError, UlidString};

const MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

macro_rules! typed_ulid {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(UlidString);

        impl $name {
            /// Returns the canonical upper-case ULID text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                value.parse().map(Self)
            }
        }
    };
}

typed_ulid!(RoomId, "An immutable Room identifier.");
typed_ulid!(MemberId, "An immutable Membership identifier.");
typed_ulid!(PrincipalId, "An authenticated Principal identifier.");
typed_ulid!(CapabilityId, "An immutable bearer Capability identifier.");
typed_ulid!(RunnerId, "A server-issued Runner identifier.");
typed_ulid!(
    AuthorityChangeId,
    "An idempotent operational authority-change identifier."
);
typed_ulid!(ActionId, "A participant-generated Action identity.");
typed_ulid!(
    TransitionId,
    "An operational immutable Transition identifier."
);
typed_ulid!(TimerId, "A logical host-owned Timer identifier.");
typed_ulid!(SourceId, "A predefined external-input source identifier.");
typed_ulid!(InputId, "An immutable external-input identity.");

/// A BLAKE3-256 digest rendered as `blake3:` followed by 64 lower-case hex
/// characters.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Blake3DigestV1([u8; 32]);

impl Blake3DigestV1 {
    /// Computes the frozen raw BLAKE3-256 digest of exact bytes.
    #[must_use]
    pub fn hash(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Constructs a digest from a verified 32-byte storage receipt.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Blake3DigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("blake3:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Blake3DigestV1 {
    type Err = DigestParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let hex = value
            .strip_prefix("blake3:")
            .ok_or(DigestParseError::Prefix)?;
        if hex.len() != 64 {
            return Err(DigestParseError::Length { actual: hex.len() });
        }
        if !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DigestParseError::LowerHex);
        }
        let mut bytes = [0_u8; 32];
        for (index, target) in bytes.iter_mut().enumerate() {
            let start = index * 2;
            *target = u8::from_str_radix(&hex[start..start + 2], 16)
                .map_err(DigestParseError::InvalidHex)?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for Blake3DigestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Blake3DigestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// The exact immutable Activity Pack revision digest.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PackDigestV1(Blake3DigestV1);

impl PackDigestV1 {
    /// Returns the underlying canonical digest.
    #[must_use]
    pub fn digest(&self) -> &Blake3DigestV1 {
        &self.0
    }
}

impl fmt::Display for PackDigestV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for PackDigestV1 {
    type Err = DigestParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Why a digest was not in the frozen v1 representation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DigestParseError {
    /// The digest did not start with its algorithm name.
    #[error("digest must start with `blake3:`")]
    Prefix,
    /// BLAKE3 is exactly 32 bytes.
    #[error("digest hex must contain 64 characters, found {actual}")]
    Length { actual: usize },
    /// Canonical digest hex is lower-case.
    #[error("digest must use lower-case hexadecimal")]
    LowerHex,
    /// The hexadecimal payload could not be decoded.
    #[error("invalid digest hexadecimal: {0}")]
    InvalidHex(ParseIntError),
}

/// A canonical 256-bit Room seed, rendered as `hex:` plus 64 lower-case hex
/// characters. It is recorded nondeterministic Genesis input, not a digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomSeedV1([u8; 32]);

impl RoomSeedV1 {
    /// Returns the exact deterministic seed bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for RoomSeedV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("hex:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for RoomSeedV1 {
    type Err = SeedParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let hex = value.strip_prefix("hex:").ok_or(SeedParseError::Prefix)?;
        if hex.len() != 64 {
            return Err(SeedParseError::Length { actual: hex.len() });
        }
        if !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SeedParseError::LowerHex);
        }
        let mut bytes = [0_u8; 32];
        for (index, target) in bytes.iter_mut().enumerate() {
            let start = index * 2;
            *target = u8::from_str_radix(&hex[start..start + 2], 16)
                .map_err(SeedParseError::InvalidHex)?;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for RoomSeedV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RoomSeedV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Why a Room seed was not in its frozen representation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SeedParseError {
    /// Room seeds use an explicit byte-encoding prefix.
    #[error("Room seed must start with `hex:`")]
    Prefix,
    /// A Room seed is exactly 32 bytes.
    #[error("Room seed hex must contain 64 characters, found {actual}")]
    Length { actual: usize },
    /// Canonical seed hex is lower-case.
    #[error("Room seed must use lower-case hexadecimal")]
    LowerHex,
    /// The hexadecimal payload could not be decoded.
    #[error("invalid Room seed hexadecimal: {0}")]
    InvalidHex(ParseIntError),
}

macro_rules! semantic_time {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(NormalizedUtcTimestamp);

        impl $name {
            /// Returns normalized UTC RFC 3339 text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = TimestampParseError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                value.parse().map(Self)
            }
        }
    };
}

semantic_time!(
    CreationRecordedAt,
    "The logical time recorded for Genesis creation."
);
semantic_time!(
    ActionAdmittedAt,
    "The host-recorded time of Action lane admission."
);
semantic_time!(
    TimerScheduledFor,
    "The immutable effective time of a Timer generation."
);
semantic_time!(
    CoreRecordedAt,
    "The declared semantic time of Core administration."
);
semantic_time!(
    ExternalInputRecordedAt,
    "The declared semantic time of external input."
);
semantic_time!(
    AuthorityCheckedAt,
    "Trusted operational time supplied to one authority decision."
);
semantic_time!(
    CapabilityExpiresAt,
    "The exclusive operational expiry of one Capability."
);
semantic_time!(
    CapabilityRevokedAt,
    "The operational time at which one Capability was revoked."
);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct NormalizedUtcTimestamp(String);

impl NormalizedUtcTimestamp {
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NormalizedUtcTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Ord for NormalizedUtcTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        timestamp_order_key(&self.0).cmp(&timestamp_order_key(&other.0))
    }
}

impl PartialOrd for NormalizedUtcTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for NormalizedUtcTimestamp {
    type Err = TimestampParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        validate_timestamp(value)?;
        Ok(Self(value.to_owned()))
    }
}

impl Serialize for NormalizedUtcTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NormalizedUtcTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

fn validate_timestamp(value: &str) -> Result<(), TimestampParseError> {
    let bytes = value.as_bytes();
    if !(bytes.len() == 20 || (22..=30).contains(&bytes.len())) {
        return Err(TimestampParseError);
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return Err(TimestampParseError);
        }
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return Err(TimestampParseError);
    }
    if bytes.len() == 20 {
        if bytes[19] != b'Z' {
            return Err(TimestampParseError);
        }
    } else {
        if bytes[19] != b'.' || bytes[bytes.len() - 1] != b'Z' {
            return Err(TimestampParseError);
        }
        let fraction = &bytes[20..bytes.len() - 1];
        if fraction.is_empty()
            || !fraction.iter().all(u8::is_ascii_digit)
            || fraction.last() == Some(&b'0')
        {
            return Err(TimestampParseError);
        }
    }

    let year = parse_decimal(&bytes[0..4]);
    let month = parse_decimal(&bytes[5..7]);
    let day = parse_decimal(&bytes[8..10]);
    let hour = parse_decimal(&bytes[11..13]);
    let minute = parse_decimal(&bytes[14..16]);
    let second = parse_decimal(&bytes[17..19]);
    if year == 0
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(TimestampParseError);
    }
    Ok(())
}

fn timestamp_order_key(value: &str) -> (u32, u32, u32, u32, u32, u32, u32) {
    let bytes = value.as_bytes();
    let fractional_nanos = if bytes.len() == 20 {
        0
    } else {
        let fraction = &bytes[20..bytes.len() - 1];
        let fraction_len = u32::try_from(fraction.len()).unwrap_or(9);
        parse_decimal(fraction) * 10_u32.pow(9_u32 - fraction_len)
    };
    (
        parse_decimal(&bytes[0..4]),
        parse_decimal(&bytes[5..7]),
        parse_decimal(&bytes[8..10]),
        parse_decimal(&bytes[11..13]),
        parse_decimal(&bytes[14..16]),
        parse_decimal(&bytes[17..19]),
        fractional_nanos,
    )
}

pub(crate) fn compare_timestamp_text(left: &str, right: &str) -> Ordering {
    timestamp_order_key(left).cmp(&timestamp_order_key(right))
}

fn parse_decimal(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0, |value, byte| value * 10 + u32::from(byte - b'0'))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The timestamp is not the unique UTC representation accepted by Core v1.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error(
    "timestamp must be a valid UTC RFC 3339 value with seconds and minimal 0-9 digit fractional precision"
)]
pub struct TimestampParseError;

macro_rules! safe_counter {
    ($name:ident, $minimum:expr, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Constructs a safe canonical JSON counter.
            ///
            /// # Errors
            ///
            /// Returns an error outside this counter's canonical safe range.
            pub fn new(value: u64) -> Result<Self, SafeCounterError> {
                if !($minimum..=MAX_SAFE_INTEGER_U64).contains(&value) {
                    Err(SafeCounterError)
                } else {
                    Ok(Self(value))
                }
            }

            /// Returns the numeric counter.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }

            /// Returns the next safe value without wrapping.
            ///
            /// # Errors
            ///
            /// Returns an error at the largest canonical safe integer.
            pub fn checked_successor(self) -> Result<Self, SafeCounterError> {
                self.0
                    .checked_add(1)
                    .ok_or(SafeCounterError)
                    .and_then(Self::new)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = u64::deserialize(deserializer)?;
                Self::new(value).map_err(de::Error::custom)
            }
        }
    };
}

safe_counter!(
    RoomSequenceV1,
    0,
    "A canonical Room sequence in the safe JSON range."
);
safe_counter!(
    TimerGenerationV1,
    1,
    "A never-zero, never-reused Timer generation."
);
safe_counter!(
    IntegrityGenerationV1,
    1,
    "An operational integrity generation."
);
safe_counter!(
    AuthorityGenerationV1,
    1,
    "A monotonic Capability scope, expiry, and revocation generation."
);
safe_counter!(
    PrincipalGenerationV1,
    1,
    "A monotonic operational Principal-status generation."
);
safe_counter!(
    MembershipGenerationV1,
    1,
    "A monotonic operational Membership authority generation."
);
safe_counter!(
    RunnerGenerationV1,
    1,
    "A monotonic operational Runner-status generation."
);

/// A counter cannot be represented in canonical JSON or violates its nonzero
/// requirement.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("counter is outside its canonical safe range")]
pub struct SafeCounterError;
