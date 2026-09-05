//! Protected local secret references for the Studio Supervisor.

use std::{
    fmt, fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

const MAX_SECRET_BYTES: u64 = 64 * 1024;

/// Authority or provider boundary served by one retained secret.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKindV1 {
    /// Global host-operator authority.
    HostAuthority,
    /// One participant Membership's observation and Action authority.
    MembershipAuthority,
    /// Runner-control authority, kept separate from participant authority.
    RunnerAuthority,
    /// Future model-provider credentials resolved outside `worldstreamd`.
    ModelProvider,
}

impl SecretKindV1 {
    const fn file_label(self) -> &'static str {
        match self {
            Self::HostAuthority => "host",
            Self::MembershipAuthority => "membership",
            Self::RunnerAuthority => "runner",
            Self::ModelProvider => "model-provider",
        }
    }
}

/// Opaque, non-secret handle safe to retain in Supervisor control-plane data.
#[derive(Clone, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SecretReferenceV1(String);

impl SecretReferenceV1 {
    /// Returns the opaque reference without resolving secret material.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates and constructs one exact opaque local reference.
    ///
    /// # Errors
    ///
    /// Returns [`SecretVaultErrorV1::InvalidReference`] unless the value is
    /// exactly 64 lowercase hexadecimal characters.
    pub fn parse(value: String) -> Result<Self, SecretVaultErrorV1> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SecretVaultErrorV1::InvalidReference);
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for SecretReferenceV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Debug for SecretReferenceV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SecretReferenceV1")
            .field(&self.0)
            .finish()
    }
}

/// Secret bytes available only to an internal, kind-bound operation.
pub struct ResolvedSecretV1(Zeroizing<Vec<u8>>);

impl ResolvedSecretV1 {
    /// Borrows the secret for the narrow caller operation.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ResolvedSecretV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResolvedSecretV1([REDACTED])")
    }
}

/// Protected, immutable file vault rooted in an owner-only directory.
#[derive(Clone, Debug)]
pub struct FileSecretVaultV1 {
    root: PathBuf,
}

/// Browser-safe availability of one retained secret reference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretAvailabilityV1 {
    /// The protected backend contains usable material for this exact kind.
    Configured,
    /// No material exists for this exact kind and reference.
    Missing,
    /// Material exists but cannot be used safely.
    Unavailable,
}

/// Non-sensitive control-plane record safe to persist or return to Studio.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SecretMetadataV1 {
    /// Stable response schema identifier.
    pub schema: String,
    /// Authority boundary required by the caller.
    pub kind: SecretKindV1,
    /// Opaque lookup handle; this is not credential material.
    pub reference: SecretReferenceV1,
    /// Closed, actionable configuration state.
    pub availability: SecretAvailabilityV1,
}

/// Browser-safe configuration state for one credential authority kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SecretKindStatusV1 {
    /// Credential boundary being reported.
    pub kind: SecretKindV1,
    /// Whether any retained reference for that boundary is usable.
    pub availability: SecretAvailabilityV1,
}

/// Complete bounded configuration-state response for Studio Settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SecretKindStatusResponseV1 {
    /// Stable response schema identifier.
    pub schema: String,
    /// Exactly one row for each supported credential authority kind.
    pub credentials: Vec<SecretKindStatusV1>,
}

impl FileSecretVaultV1 {
    /// Opens or creates the protected vault directory.
    ///
    /// # Errors
    ///
    /// Returns a pathless error when platform ownership or permission checks
    /// fail closed.
    pub fn open(root: &Path) -> Result<Self, SecretVaultErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        Ok(Self { root })
    }

    /// Opens a retained vault without creating or repairing any filesystem state.
    ///
    /// # Errors
    /// Rejects missing or unsafe retained vault directories.
    pub fn open_existing(root: &Path) -> Result<Self, SecretVaultErrorV1> {
        let root = worldstream_runtime::validate_data_directory(root)
            .map_err(|_| SecretVaultErrorV1::Unavailable)?;
        Ok(Self { root })
    }

    /// Creates one immutable kind-bound secret and returns only its opaque
    /// reference.
    ///
    /// # Errors
    ///
    /// Returns a pathless error for empty/oversized material, randomness
    /// failure, unsafe local storage, or an I/O failure.
    pub fn store(
        &self,
        kind: SecretKindV1,
        secret: &[u8],
    ) -> Result<SecretReferenceV1, SecretVaultErrorV1> {
        if secret.is_empty() || u64::try_from(secret.len()).unwrap_or(u64::MAX) > MAX_SECRET_BYTES {
            return Err(SecretVaultErrorV1::InvalidMaterial);
        }
        let mut id = [0_u8; 32];
        getrandom::fill(&mut id).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        let reference = SecretReferenceV1(hex(&id));
        let path = self.secret_path(kind, &reference);
        let mut file =
            create_owner_only_file(&path).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        file.write_all(secret)
            .and_then(|()| file.sync_all())
            .map_err(|_| SecretVaultErrorV1::Unavailable)?;
        Ok(reference)
    }

    /// Publishes one exact, kind-bound reference without rotating it. This is
    /// used only when a durable operation derives the reference before it
    /// creates secret material, so a crash retry resolves the same bearer.
    #[doc(hidden)]
    pub fn publish_at_reference(
        &self,
        kind: SecretKindV1,
        reference: &SecretReferenceV1,
        secret: &[u8],
    ) -> Result<(), SecretVaultErrorV1> {
        if secret.is_empty() || u64::try_from(secret.len()).unwrap_or(u64::MAX) > MAX_SECRET_BYTES {
            return Err(SecretVaultErrorV1::InvalidMaterial);
        }
        let path = self.secret_path(kind, reference);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                let existing = self.resolve(kind, reference)?;
                let matches = existing.as_bytes().len() == secret.len()
                    && existing
                        .as_bytes()
                        .iter()
                        .zip(secret)
                        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
                        == 0;
                return if matches {
                    Ok(())
                } else {
                    Err(SecretVaultErrorV1::InvalidMaterial)
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SecretVaultErrorV1::Unavailable),
        }
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        let temporary = self.root.join(format!(
            ".fixed-secret-{}.tmp",
            blake3::hash(&nonce).to_hex()
        ));
        let result = (|| {
            let mut file = worldstream_runtime::create_owner_only_renameable_file(&temporary)
                .map_err(|_| SecretVaultErrorV1::Unavailable)?;
            file.write_all(secret)
                .map_err(|_| SecretVaultErrorV1::Unavailable)?;
            crate::protected_publication::publish(
                file,
                &temporary,
                &path,
                crate::protected_publication::PublicationMode::CreateNew,
            )
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    SecretVaultErrorV1::InvalidMaterial
                } else {
                    SecretVaultErrorV1::Unavailable
                }
            })
        })();
        let _ = fs::remove_file(temporary);
        result
    }

    /// Publishes one pending provider import's chosen reference without rotation.
    pub(crate) fn publish_imported_provider(
        &self,
        reference: &SecretReferenceV1,
        secret: &[u8],
    ) -> Result<(), SecretVaultErrorV1> {
        if secret.is_empty() || u64::try_from(secret.len()).unwrap_or(u64::MAX) > MAX_SECRET_BYTES {
            return Err(SecretVaultErrorV1::InvalidMaterial);
        }
        let path = self.secret_path(SecretKindV1::ModelProvider, reference);
        let matches_existing = || -> Result<(), SecretVaultErrorV1> {
            let existing = self.resolve(SecretKindV1::ModelProvider, reference)?;
            if existing.as_bytes().len() == secret.len()
                && existing
                    .as_bytes()
                    .iter()
                    .zip(secret)
                    .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
                    == 0
            {
                Ok(())
            } else {
                Err(SecretVaultErrorV1::InvalidMaterial)
            }
        };
        match fs::symlink_metadata(&path) {
            Ok(_) => return matches_existing(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SecretVaultErrorV1::Unavailable),
        }
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        let temporary = self.root.join(format!(
            ".provider-import-{}.tmp",
            blake3::hash(&nonce).to_hex()
        ));
        let result = (|| {
            let mut file = worldstream_runtime::create_owner_only_renameable_file(&temporary)
                .map_err(|_| SecretVaultErrorV1::Unavailable)?;
            file.write_all(secret)
                .map_err(|_| SecretVaultErrorV1::Unavailable)?;
            match crate::protected_publication::publish(
                file,
                &temporary,
                &path,
                crate::protected_publication::PublicationMode::CreateNew,
            ) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    matches_existing()
                }
                Err(_) => Err(SecretVaultErrorV1::Unavailable),
            }
        })();
        let _ = fs::remove_file(temporary);
        result
    }

    /// Resolves one secret only through its original authority kind.
    ///
    /// # Errors
    ///
    /// Returns a pathless missing or unavailable error and never substitutes
    /// another authority or secret.
    pub fn resolve(
        &self,
        kind: SecretKindV1,
        reference: &SecretReferenceV1,
    ) -> Result<ResolvedSecretV1, SecretVaultErrorV1> {
        let path = self.secret_path(kind, reference);
        if !path.exists() {
            return Err(SecretVaultErrorV1::Missing);
        }
        validate_owner_only_file(&path).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        let metadata = fs::metadata(&path).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        if metadata.len() == 0 || metadata.len() > MAX_SECRET_BYTES {
            return Err(SecretVaultErrorV1::Unavailable);
        }
        let bytes = fs::read(&path).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        Ok(ResolvedSecretV1(Zeroizing::new(bytes)))
    }

    /// Inspects one kind-bound reference without resolving or returning its
    /// underlying bytes.
    #[must_use]
    pub fn inspect(&self, kind: SecretKindV1, reference: &SecretReferenceV1) -> SecretMetadataV1 {
        let availability = match self.resolve(kind, reference) {
            Ok(_) => SecretAvailabilityV1::Configured,
            Err(SecretVaultErrorV1::Missing) => SecretAvailabilityV1::Missing,
            Err(
                SecretVaultErrorV1::InvalidMaterial
                | SecretVaultErrorV1::InvalidReference
                | SecretVaultErrorV1::Unavailable,
            ) => SecretAvailabilityV1::Unavailable,
        };
        SecretMetadataV1 {
            schema: "worldstream/studio-secret-metadata/v1".to_owned(),
            kind,
            reference: reference.clone(),
            availability,
        }
    }

    /// Checks protected retained-file metadata without opening secret material.
    pub(crate) fn reference_metadata(
        &self,
        kind: SecretKindV1,
        reference: &SecretReferenceV1,
    ) -> Result<u64, SecretVaultErrorV1> {
        let path = self.secret_path(kind, reference);
        validate_owner_only_file(&path).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        let length = fs::metadata(path)
            .map_err(|_| SecretVaultErrorV1::Unavailable)?
            .len();
        if length == 0 || length > MAX_SECRET_BYTES {
            return Err(SecretVaultErrorV1::InvalidMaterial);
        }
        Ok(length)
    }

    /// Summarizes configuration without returning retained references or
    /// resolving bytes across the browser boundary.
    #[must_use]
    pub fn kind_statuses(&self) -> SecretKindStatusResponseV1 {
        const KINDS: [SecretKindV1; 4] = [
            SecretKindV1::HostAuthority,
            SecretKindV1::MembershipAuthority,
            SecretKindV1::RunnerAuthority,
            SecretKindV1::ModelProvider,
        ];
        let names = fs::read_dir(&self.root).map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect::<Vec<_>>()
        });
        let credentials = KINDS
            .into_iter()
            .map(|kind| {
                let Ok(names) = &names else {
                    return SecretKindStatusV1 {
                        kind,
                        availability: SecretAvailabilityV1::Unavailable,
                    };
                };
                let prefix = format!("{}-", kind.file_label());
                let references = names.iter().filter_map(|name| {
                    name.strip_prefix(&prefix)
                        .and_then(|value| value.strip_suffix(".secret"))
                        .and_then(|value| SecretReferenceV1::parse(value.to_owned()).ok())
                });
                let mut found = false;
                let mut configured = false;
                for reference in references {
                    found = true;
                    if self.resolve(kind, &reference).is_ok() {
                        configured = true;
                        break;
                    }
                }
                let availability = if configured {
                    SecretAvailabilityV1::Configured
                } else if found {
                    SecretAvailabilityV1::Unavailable
                } else {
                    SecretAvailabilityV1::Missing
                };
                SecretKindStatusV1 { kind, availability }
            })
            .collect();
        SecretKindStatusResponseV1 {
            schema: "worldstream/studio-secret-kind-status/v1".to_owned(),
            credentials,
        }
    }

    /// Returns every exact retained reference for one authority kind without
    /// resolving any material.
    ///
    /// This internal startup boundary uses the result to recover an interrupted
    /// first import only when exactly one retained Host authority exists.
    pub(crate) fn retained_references(
        &self,
        kind: SecretKindV1,
    ) -> Result<Vec<SecretReferenceV1>, SecretVaultErrorV1> {
        let prefix = format!("{}-", kind.file_label());
        let mut references = Vec::new();
        let entries = fs::read_dir(&self.root).map_err(|_| SecretVaultErrorV1::Unavailable)?;
        for entry in entries {
            let entry = entry.map_err(|_| SecretVaultErrorV1::Unavailable)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| SecretVaultErrorV1::Unavailable)?;
            let Some(value) = name
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix(".secret"))
            else {
                continue;
            };
            let reference = SecretReferenceV1::parse(value.to_owned())
                .map_err(|_| SecretVaultErrorV1::Unavailable)?;
            references.push(reference);
        }
        references.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(references)
    }

    fn secret_path(&self, kind: SecretKindV1, reference: &SecretReferenceV1) -> PathBuf {
        self.root.join(format!(
            "{}-{}.secret",
            kind.file_label(),
            reference.as_str()
        ))
    }
}

/// Pathless secret-vault failure safe for diagnostics.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SecretVaultErrorV1 {
    /// The referenced kind-bound secret does not exist.
    #[error("retained secret is missing")]
    Missing,
    /// Secret material is empty or outside the bounded input contract.
    #[error("secret material is invalid")]
    InvalidMaterial,
    /// The opaque reference is not in the fixed local format.
    #[error("secret reference is invalid")]
    InvalidReference,
    /// The protected backend or retained secret cannot be used safely.
    #[error("protected secret backend is unavailable")]
    Unavailable,
}

/// Builds the read-only, browser-safe secret-status API.
pub fn secret_status_router(vault: FileSecretVaultV1) -> Router {
    Router::new()
        .route("/api/v1/secrets", get(secret_kind_statuses))
        .route("/api/v1/secrets/{kind}/{reference}", get(secret_status))
        .with_state(vault)
}

async fn secret_kind_statuses(
    State(vault): State<FileSecretVaultV1>,
) -> Json<SecretKindStatusResponseV1> {
    Json(vault.kind_statuses())
}

async fn secret_status(
    State(vault): State<FileSecretVaultV1>,
    AxumPath((kind, reference)): AxumPath<(SecretKindV1, String)>,
) -> Result<Json<SecretMetadataV1>, StatusCode> {
    let reference = SecretReferenceV1::parse(reference).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(vault.inspect(kind, &reference)))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tempfile::tempdir;
    use tower::ServiceExt as _;

    use super::{
        FileSecretVaultV1, SecretAvailabilityV1, SecretKindV1, SecretVaultErrorV1,
        secret_status_router,
    };

    #[test]
    fn retained_secret_resolves_after_supervisor_restart() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault_path = directory.path().join("vault");
        let vault = FileSecretVaultV1::open(&vault_path)
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        let reference = vault
            .store(SecretKindV1::HostAuthority, b"host-authority-sentinel")
            .unwrap_or_else(|error| unreachable!("store secret: {error}"));
        drop(vault);

        let reopened = FileSecretVaultV1::open(&vault_path)
            .unwrap_or_else(|error| unreachable!("reopen vault: {error}"));
        let resolved = reopened
            .resolve(SecretKindV1::HostAuthority, &reference)
            .unwrap_or_else(|error| unreachable!("resolve retained secret: {error}"));

        assert_eq!(resolved.as_bytes(), b"host-authority-sentinel");
    }

    #[test]
    fn authority_kinds_do_not_substitute_for_each_other() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        let reference = vault
            .store(SecretKindV1::HostAuthority, b"host-only")
            .unwrap_or_else(|error| unreachable!("store secret: {error}"));

        assert_eq!(
            vault
                .resolve(SecretKindV1::MembershipAuthority, &reference)
                .err(),
            Some(SecretVaultErrorV1::Missing)
        );
        assert_eq!(
            vault
                .inspect(SecretKindV1::MembershipAuthority, &reference)
                .availability,
            SecretAvailabilityV1::Missing
        );
    }

    #[tokio::test]
    async fn browser_status_never_discloses_secret_material() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        let reference = vault
            .store(
                SecretKindV1::RunnerAuthority,
                b"runner-bearer-must-not-cross-browser-boundary",
            )
            .unwrap_or_else(|error| unreachable!("store secret: {error}"));
        let response = secret_status_router(vault)
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/secrets/runner_authority/{}",
                        reference.as_str()
                    ))
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("status response: {error}"));
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("status body: {error}"))
            .to_bytes();

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)
                .unwrap_or_else(|error| unreachable!("status json: {error}"))["availability"],
            "configured"
        );
        assert!(
            !body
                .windows(b"bearer".len())
                .any(|window| window == b"bearer")
        );
        assert!(
            !body
                .windows(b"must-not".len())
                .any(|window| window == b"must-not")
        );
    }

    #[tokio::test]
    async fn browser_lists_only_configuration_state_for_each_authority_kind() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        vault
            .store(SecretKindV1::HostAuthority, b"host-list-sentinel")
            .unwrap_or_else(|error| unreachable!("store secret: {error}"));
        let response = secret_status_router(vault)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/secrets")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("status response: {error}"));
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("status body: {error}"))
            .to_bytes();
        let json = serde_json::from_slice::<serde_json::Value>(&body)
            .unwrap_or_else(|error| unreachable!("status json: {error}"));

        assert_eq!(json["credentials"].as_array().map(Vec::len), Some(4));
        assert_eq!(json["credentials"][0]["availability"], "configured");
        assert_eq!(json["credentials"][1]["availability"], "missing");
        assert!(
            !body
                .windows(b"sentinel".len())
                .any(|window| window == b"sentinel")
        );
        assert!(json["credentials"][0].get("reference").is_none());
    }

    #[test]
    fn diagnostics_redact_resolved_material() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
        let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("open vault: {error}"));
        let reference = vault
            .store(SecretKindV1::ModelProvider, b"provider-secret")
            .unwrap_or_else(|error| unreachable!("store secret: {error}"));
        let resolved = vault
            .resolve(SecretKindV1::ModelProvider, &reference)
            .unwrap_or_else(|error| unreachable!("resolve secret: {error}"));

        let debug = format!("{resolved:?}");
        assert_eq!(debug, "ResolvedSecretV1([REDACTED])");
        assert!(!debug.contains("provider-secret"));
    }
}
