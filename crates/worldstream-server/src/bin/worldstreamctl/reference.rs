//! Bounded public identities shared by CLI arguments and operational reports.

use serde::Serialize;

/// A validated public identity, never a credential or arbitrary diagnostic.
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct PublicReference(String);

impl PublicReference {
    /// Validate the common public identity grammar without echoing rejected input.
    ///
    /// # Errors
    /// Returns a fixed diagnostic for an empty, oversized, or invalid identity.
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if value.len() > 128
            || !value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err("expected a bounded public identifier");
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
