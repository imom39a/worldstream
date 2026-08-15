use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The compatibility artifact compiled into every runtime-linked binary.
pub const EMBEDDED_COMPATIBILITY_JSON: &str = include_str!("../../../compatibility.json");

const MANIFEST_SCHEMA_V1: &str = "worldstream/storage-compatibility-manifest/v1";

/// Compatibility fields consumed by the process shell.
#[derive(Clone, Debug, Deserialize)]
pub struct CompatibilityManifest {
    /// Manifest schema identifier.
    pub schema: String,
    /// Monotonic manifest schema revision.
    pub manifest_revision: u32,
    /// Specification or release artifact kind.
    pub manifest_kind: String,
    /// Whether all release gates have completed.
    pub release_ready: bool,
    /// Release candidate version.
    pub release_candidate: String,
    /// Frozen product contracts.
    pub contracts: CompatibilityContracts,
    /// Pinned compiler toolchain.
    pub toolchains: CompatibilityToolchains,
    /// Supported storage selector values.
    pub storage: CompatibilityStorage,
}

/// Contract versions reported by `/version`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompatibilityContracts {
    /// Product version.
    pub product: String,
    /// Wire protocol version.
    pub wire: String,
    /// Configuration schema version.
    pub config: u32,
    /// Storage schema version.
    pub storage_schema: u32,
    /// Canonical Core schema identifier.
    pub core_schema_version: String,
    /// Canonical hash suite identifier.
    pub hash_suite: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompatibilityToolchains {
    rust: String,
    rust_edition: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompatibilityStorage {
    profiles: Vec<String>,
    default_profile: String,
    sqlite: CompatibilitySqlite,
    postgresql: CompatibilityPostgresql,
}

#[derive(Clone, Debug, Deserialize)]
struct CompatibilitySqlite {
    profile_id: String,
    version: String,
}

#[derive(Clone, Debug, Deserialize)]
struct CompatibilityPostgresql {
    profile_id: String,
    major: u32,
    minimum_patch: String,
}

/// Stable manifest details safe to expose through operator surfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompatibilitySummary {
    /// Manifest schema identifier.
    pub schema: String,
    /// Manifest revision.
    pub revision: u32,
    /// Specification/release artifact kind.
    pub kind: String,
    /// False until release evidence is complete.
    pub release_ready: bool,
    /// Canonical product and protocol contracts.
    pub contracts: CompatibilityContracts,
    /// Exact Rust toolchain pin.
    pub rust_toolchain: String,
    /// Exact Rust edition.
    pub rust_edition: String,
    /// Exact `SQLite` build version named by the authored specification.
    pub sqlite_version: String,
    /// Supported `PostgreSQL` major.
    pub postgresql_major: u32,
    /// Minimum supported `PostgreSQL` patch.
    pub postgresql_minimum_patch: String,
}

impl CompatibilityManifest {
    /// Validates the subset required before the process shell can start.
    ///
    /// # Errors
    ///
    /// Returns an error when the embedded artifact declares a schema, revision,
    /// config/toolchain pin, or storage selector the compiled shell cannot honor.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != MANIFEST_SCHEMA_V1 {
            return Err(ManifestError::UnsupportedSchema(self.schema.clone()));
        }
        if self.manifest_revision != 1 {
            return Err(ManifestError::UnsupportedRevision(self.manifest_revision));
        }
        if self.release_candidate != self.contracts.product {
            return Err(ManifestError::Inconsistent(
                "release_candidate must equal contracts.product",
            ));
        }
        if self.contracts.config != 1 {
            return Err(ManifestError::UnsupportedConfig(self.contracts.config));
        }
        if self.toolchains.rust != "1.97.1" || self.toolchains.rust_edition != "2024" {
            return Err(ManifestError::Inconsistent(
                "embedded Rust toolchain does not match the workspace pin",
            ));
        }
        if self.storage.profiles.as_slice() != ["sqlite-bundled", "postgres-primary"] {
            return Err(ManifestError::Inconsistent(
                "storage.profiles must contain only the two frozen profiles in order",
            ));
        }
        if self.storage.default_profile != "sqlite-bundled"
            || self.storage.sqlite.profile_id != "sqlite-bundled"
            || self.storage.postgresql.profile_id != "postgres-primary"
        {
            return Err(ManifestError::Inconsistent(
                "storage profile identifiers do not match the frozen selectors",
            ));
        }
        Ok(())
    }

    /// Returns whether a startup storage selector is declared by the manifest.
    #[must_use]
    pub fn supports_storage_profile(&self, profile: &str) -> bool {
        self.storage.profiles.iter().any(|item| item == profile)
    }

    /// Produces the bounded operator-facing manifest view.
    #[must_use]
    pub fn summary(&self) -> CompatibilitySummary {
        CompatibilitySummary {
            schema: self.schema.clone(),
            revision: self.manifest_revision,
            kind: self.manifest_kind.clone(),
            release_ready: self.release_ready,
            contracts: self.contracts.clone(),
            rust_toolchain: self.toolchains.rust.clone(),
            rust_edition: self.toolchains.rust_edition.clone(),
            sqlite_version: self.storage.sqlite.version.clone(),
            postgresql_major: self.storage.postgresql.major,
            postgresql_minimum_patch: self.storage.postgresql.minimum_patch.clone(),
        }
    }
}

/// Returns the exact embedded canonical JSON bytes as UTF-8 text.
#[must_use]
pub const fn embedded_manifest_json() -> &'static str {
    EMBEDDED_COMPATIBILITY_JSON
}

/// Parses and validates the build-guarded embedded compatibility artifact.
///
/// # Errors
///
/// Returns an error when the JSON is malformed or its frozen identity is invalid.
pub fn embedded_manifest() -> Result<CompatibilityManifest, ManifestError> {
    let manifest: CompatibilityManifest = serde_json::from_str(EMBEDDED_COMPATIBILITY_JSON)?;
    manifest.validate()?;
    Ok(manifest)
}

/// Fail-closed embedded manifest validation errors.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The embedded JSON is malformed or has a wrong field type.
    #[error("embedded compatibility JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The schema is not implemented by this process.
    #[error("unsupported compatibility schema: {0}")]
    UnsupportedSchema(String),
    /// The manifest revision is not implemented by this process.
    #[error("unsupported compatibility manifest revision: {0}")]
    UnsupportedRevision(u32),
    /// The configuration contract is not implemented by this process.
    #[error("unsupported configuration contract version: {0}")]
    UnsupportedConfig(u32),
    /// Related manifest values contradict one another or the compiled contract.
    #[error("inconsistent compatibility manifest: {0}")]
    Inconsistent(&'static str),
}

#[cfg(test)]
mod tests {
    use super::{embedded_manifest, embedded_manifest_json};

    #[test]
    fn embedded_manifest_is_valid_and_fail_closed() {
        let manifest = embedded_manifest();
        assert!(manifest.is_ok());
        let manifest = manifest.unwrap_or_else(|error| unreachable!("validated above: {error}"));
        assert!(!manifest.release_ready);
        assert_eq!(manifest.contracts.config, 1);
    }

    #[test]
    fn embedded_artifact_ends_with_a_single_newline() {
        let bytes = embedded_manifest_json().as_bytes();
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes.ends_with(b"\n\n"));
    }
}
