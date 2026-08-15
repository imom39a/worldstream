use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

const ULID_LENGTH: usize = 26;

/// A validated, canonical upper-case ULID string.
///
/// This type validates externally supplied identifiers but deliberately does
/// not generate them. Generation belongs to the runtime boundary that records
/// its nondeterministic input.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct UlidString(String);

impl UlidString {
    /// Returns the canonical identifier bytes as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(value: &str) -> Result<(), IdParseError> {
        if value.len() != ULID_LENGTH {
            return Err(IdParseError::Length {
                actual: value.len(),
            });
        }

        let mut bytes = value.bytes();
        let first = bytes.next().ok_or(IdParseError::Length { actual: 0 })?;
        if !(b'0'..=b'7').contains(&first) {
            return Err(IdParseError::Overflow);
        }

        if bytes.all(is_crockford_base32) {
            Ok(())
        } else {
            Err(IdParseError::Alphabet)
        }
    }
}

fn is_crockford_base32(byte: u8) -> bool {
    byte.is_ascii_digit()
        || matches!(
            byte,
            b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'
        )
}

impl AsRef<str> for UlidString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for UlidString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for UlidString {
    type Err = IdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::validate(value)?;
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for UlidString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// A transport request identifier. It is not a semantic operation identity.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct RequestId(UlidString);

impl RequestId {
    /// Returns the canonical identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for RequestId {
    type Err = IdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Why an external identifier is not a canonical ULID string.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum IdParseError {
    /// ULIDs encode to exactly 26 ASCII characters.
    #[error("ULID must contain exactly 26 ASCII characters, found {actual}")]
    Length { actual: usize },
    /// A 128-bit ULID cannot begin above `7`.
    #[error("ULID exceeds the 128-bit canonical range")]
    Overflow,
    /// Lower-case, ambiguous, or non-Crockford characters are forbidden.
    #[error("ULID must use the canonical upper-case Crockford Base32 alphabet")]
    Alphabet,
}

#[cfg(test)]
mod tests {
    use super::{IdParseError, UlidString};

    #[test]
    fn accepts_canonical_ulid() {
        let parsed = "01ARZ3NDEKTSV4RRFFQ69G5FAV".parse::<UlidString>();
        assert!(parsed.is_ok());
    }

    #[test]
    fn rejects_lower_case_and_ambiguous_characters() {
        for value in [
            "01arz3ndektsv4rrffq69g5fav",
            "01ARZ3NDEKTSV4RRFFQ69G5FAI",
            "01ARZ3NDEKTSV4RRFFQ69G5FAO",
        ] {
            assert_eq!(value.parse::<UlidString>(), Err(IdParseError::Alphabet));
        }
    }

    #[test]
    fn rejects_overflowing_first_character() {
        assert_eq!(
            "81ARZ3NDEKTSV4RRFFQ69G5FAV".parse::<UlidString>(),
            Err(IdParseError::Overflow)
        );
    }
}
