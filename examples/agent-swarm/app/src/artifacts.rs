//! Protected, content-addressed local artifacts for one Agent Swarm workspace.
//!
//! Callers name authorized files with [`ArtifactPath`]. The module rejects
//! absolute paths, traversal, symlinks, and Windows reparse points before it
//! reads a file. Captured bytes are copied into an owner-only store and are
//! verified every time they are read; a pathname alone is never evidence of a
//! particular artifact version.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

const DIGEST_PREFIX: &str = "blake3:";
const DIGEST_HEX_BYTES: usize = 64;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RELATIVE_PATH_BYTES: usize = 4 * 1024;
static STAGED_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A portable path relative to an explicitly authorized root.
///
/// Paths use `/` separators on every platform. Empty segments, traversal,
/// Windows alternate streams, and reserved Windows device names are rejected
/// so persisted evidence has the same meaning on macOS and Windows.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ArtifactPath(String);

impl ArtifactPath {
    /// Validates a portable relative artifact path.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError::InvalidPath`] when the value is not a bounded,
    /// portable relative path.
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        validate_relative_path(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn to_path_buf(&self) -> PathBuf {
        self.0.split('/').collect()
    }
}

impl fmt::Display for ArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ArtifactPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// The BLAKE3 identity of immutable artifact bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn hex(&self) -> String {
        lower_hex(&self.0)
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{DIGEST_PREFIX}{}", self.hex())
    }
}

impl FromStr for ContentDigest {
    type Err = ArtifactError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let hex = value
            .strip_prefix(DIGEST_PREFIX)
            .ok_or(ArtifactError::InvalidDigest)?;
        if hex.len() != DIGEST_HEX_BYTES {
            return Err(ArtifactError::InvalidDigest);
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_nibble(pair[0]).ok_or(ArtifactError::InvalidDigest)?;
            let low = hex_nibble(pair[1]).ok_or(ArtifactError::InvalidDigest)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for ContentDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// A verified reference to bytes retained in the protected object store.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    digest: ContentDigest,
    byte_length: u64,
}

/// Exact artifact descriptor discovered in an authoritative participant view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeArtifactRef {
    pub artifact_id: String,
    pub digest: String,
    pub local_path: String,
    pub media_type: String,
}

impl ArtifactRef {
    #[must_use]
    pub const fn digest(&self) -> ContentDigest {
        self.digest
    }

    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }
}

/// Content-addressed storage paired with one authorized working root.
#[derive(Clone, Debug)]
pub struct ArtifactWorkspace {
    authorized: PathBuf,
    protected: PathBuf,
    objects: PathBuf,
}

impl ArtifactWorkspace {
    /// Opens one authorized root and an owner-only artifact store.
    ///
    /// The two roots must not overlap. This prevents a write-back target from
    /// naming the evidence that authorizes it.
    ///
    /// # Errors
    ///
    /// Fails closed for redirected roots, non-directories, unsafe protected
    /// storage, or overlapping roots.
    pub fn open(authorized_root: &Path, protected_root: &Path) -> Result<Self, ArtifactError> {
        let authorized_metadata = fs::symlink_metadata(authorized_root)
            .map_err(|_| ArtifactError::AuthorizedRootUnavailable)?;
        ensure_safe_type(&authorized_metadata, ExpectedType::Directory)?;
        let authorized_root = fs::canonicalize(authorized_root)
            .map_err(|_| ArtifactError::AuthorizedRootUnavailable)?;
        let protected_root = prepare_data_directory(protected_root)
            .map_err(|_| ArtifactError::ProtectedStoreUnavailable)?;
        if authorized_root.starts_with(&protected_root)
            || protected_root.starts_with(&authorized_root)
        {
            return Err(ArtifactError::OverlappingRoots);
        }
        let objects_root = prepare_data_directory(&protected_root.join("objects"))
            .map_err(|_| ArtifactError::ProtectedStoreUnavailable)?;
        Ok(Self {
            authorized: authorized_root,
            protected: protected_root,
            objects: objects_root,
        })
    }

    /// Captures one existing regular file from the authorized root.
    ///
    /// The file is read twice through one open handle and its path identity is
    /// rechecked. Concurrent mutation returns `ChangedDuringCapture` instead
    /// of publishing ambiguous evidence.
    ///
    /// # Errors
    ///
    /// Rejects missing, redirected, non-regular, oversized, or changing files.
    pub fn capture(&self, path: &ArtifactPath) -> Result<ArtifactRef, ArtifactError> {
        self.capture_if_present(path)?
            .ok_or(ArtifactError::ArtifactMissing)
    }

    /// Captures a file if present, while still validating every existing path
    /// component. Only the final component may be absent.
    ///
    /// # Errors
    ///
    /// Returns a closed error for unsafe path components or unavailable I/O.
    pub fn capture_if_present(
        &self,
        path: &ArtifactPath,
    ) -> Result<Option<ArtifactRef>, ArtifactError> {
        self.capture_from_root_if_present(&self.authorized, path)
    }

    /// Stores generated bytes (for example checker output) without treating a
    /// transient pathname as their identity.
    ///
    /// # Errors
    ///
    /// Rejects oversized bytes or unavailable protected storage.
    pub fn store_bytes(&self, bytes: &[u8]) -> Result<ArtifactRef, ArtifactError> {
        let byte_length =
            u64::try_from(bytes.len()).map_err(|_| ArtifactError::ArtifactTooLarge)?;
        if byte_length > MAX_ARTIFACT_BYTES {
            return Err(ArtifactError::ArtifactTooLarge);
        }
        let artifact = ArtifactRef {
            digest: ContentDigest::of(bytes),
            byte_length,
        };
        let object = self.object_path(artifact.digest);
        match fs::symlink_metadata(&object) {
            Ok(_) => {
                self.verify(&artifact)?;
                return Ok(artifact);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(ArtifactError::ProtectedStoreUnavailable),
        }

        let staged = self.unique_staged_path(artifact.digest)?;
        let mut file = create_owner_only_file(&staged)
            .map_err(|_| ArtifactError::ProtectedStoreUnavailable)?;
        let publication = (|| {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| ArtifactError::ProtectedStoreUnavailable)?;
            drop(file);
            match fs::hard_link(&staged, &object) {
                Ok(()) => {
                    fs::remove_file(&staged)
                        .map_err(|_| ArtifactError::ProtectedStoreUnavailable)?;
                    make_read_only(&object)?;
                    sync_directory(&self.objects)?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    self.verify(&artifact)?;
                }
                Err(_) => return Err(ArtifactError::ProtectedStoreUnavailable),
            }
            Ok(())
        })();
        let _cleanup = fs::remove_file(&staged);
        publication?;
        self.verify(&artifact)?;
        Ok(artifact)
    }

    /// Publishes generated evidence at an authorized relative path without
    /// replacing any existing bytes. Repeating the exact publication is
    /// idempotent; a different existing value fails closed.
    ///
    /// # Errors
    ///
    /// Rejects unsafe paths, missing parent directories, oversized bytes, or
    /// an existing path whose content differs from `bytes`.
    pub fn publish_generated(
        &self,
        path: &ArtifactPath,
        bytes: &[u8],
    ) -> Result<ArtifactRef, ArtifactError> {
        let artifact = self.store_bytes(bytes)?;
        if let Some(existing) = self.capture_if_present(path)? {
            return if existing == artifact {
                Ok(existing)
            } else {
                Err(ArtifactError::ChangedDuringCapture)
            };
        }
        let parent = self.resolve_authorized_parent(path)?;
        let target = parent.join(
            path.to_path_buf()
                .file_name()
                .ok_or(ArtifactError::InvalidPath)?,
        );
        let sequence = STAGED_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staged = parent.join(format!(
            ".worldstream-evidence-{}.{}.{}.staged",
            artifact.digest.hex(),
            std::process::id(),
            sequence
        ));
        let mut file =
            create_authorized_staged_file(&staged).map_err(|_| ArtifactError::Unavailable)?;
        let publication = (|| {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| ArtifactError::Unavailable)?;
            drop(file);
            match fs::hard_link(&staged, &target) {
                Ok(()) => {
                    fs::remove_file(&staged).map_err(|_| ArtifactError::Unavailable)?;
                    make_read_only(&target)?;
                    sync_directory(&parent)?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let existing = self.capture(path)?;
                    if existing != artifact {
                        return Err(ArtifactError::ChangedDuringCapture);
                    }
                }
                Err(_) => return Err(ArtifactError::Unavailable),
            }
            self.capture(path).and_then(|published| {
                if published == artifact {
                    Ok(published)
                } else {
                    Err(ArtifactError::ChangedDuringCapture)
                }
            })
        })();
        let _cleanup = fs::remove_file(staged);
        publication
    }

    /// Reads and re-verifies immutable artifact bytes.
    ///
    /// # Errors
    ///
    /// Returns `CorruptObject` when retained bytes no longer match the exact
    /// digest and length in the reference.
    pub fn read(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactError> {
        let object = self.object_path(artifact.digest);
        validate_owner_only_file(&object).map_err(|_| ArtifactError::CorruptObject)?;
        let bytes = read_stable_regular_file(&object)?;
        if u64::try_from(bytes.len()).ok() != Some(artifact.byte_length)
            || ContentDigest::of(&bytes) != artifact.digest
        {
            return Err(ArtifactError::CorruptObject);
        }
        Ok(bytes)
    }

    /// Verifies that a retained reference still names its exact bytes.
    ///
    /// # Errors
    ///
    /// Returns `CorruptObject` or an availability error for invalid evidence.
    pub fn verify(&self, artifact: &ArtifactRef) -> Result<(), ArtifactError> {
        self.read(artifact).map(|_| ())
    }

    /// Captures a path only after the caller has selected its unique
    /// authoritative Room reference, then verifies that reference's digest.
    /// Supported authoritative digests are lowercase BLAKE3 and SHA-256.
    ///
    /// # Errors
    ///
    /// Rejects mismatched or unsupported digest claims.
    pub fn capture_authoritative(
        &self,
        path: &ArtifactPath,
        authoritative: &AuthoritativeArtifactRef,
    ) -> Result<ArtifactRef, ArtifactError> {
        let artifact = self.capture(path)?;
        self.verify_authoritative_digest(&artifact, authoritative)?;
        Ok(artifact)
    }

    /// Resolves one selected descriptor back through the authenticated Room
    /// projection, then captures at most `maximum_bytes` from its authorized
    /// working-area path and verifies the exact digest. This is the narrow
    /// inspection seam for callers that must never turn a displayed pathname
    /// into ambient filesystem authority.
    ///
    /// # Errors
    ///
    /// Rejects zero/oversized limits, paths outside the authorized root,
    /// descriptors absent from `activity`, conflicting content claims at the
    /// selected path, unsafe files, and digest mismatches. Distinct semantic
    /// IDs may share immutable content when their digest and media type agree.
    pub fn capture_authoritative_bounded(
        &self,
        activity: &serde_json::Value,
        selected: &AuthoritativeArtifactRef,
        maximum_bytes: u64,
    ) -> Result<(ArtifactPath, ArtifactRef), ArtifactError> {
        if maximum_bytes == 0 || maximum_bytes > MAX_ARTIFACT_BYTES {
            return Err(ArtifactError::ArtifactTooLarge);
        }
        let root = portable_absolute(&self.authorized)?;
        let relative = relative_authoritative_path(&root, &selected.local_path)
            .ok_or(ArtifactError::InvalidReference)?;
        let path = ArtifactPath::new(relative)?;
        let references = authoritative_artifacts_for_path(activity, &self.authorized, &path)?;
        if references.is_empty() {
            return Err(ArtifactError::UnreferencedArtifact);
        }
        if !references.contains(selected) {
            return Err(ArtifactError::InvalidReference);
        }
        if references.iter().any(|reference| {
            reference.digest != selected.digest || reference.media_type != selected.media_type
        }) {
            return Err(ArtifactError::AmbiguousArtifact);
        }
        let file = resolve_contained_file_if_present(&self.authorized, &path)?
            .ok_or(ArtifactError::ArtifactMissing)?;
        let bytes = read_stable_regular_file_bounded(&file, maximum_bytes)?;
        let artifact = self.store_bytes(&bytes)?;
        self.verify_authoritative_digest(&artifact, selected)?;
        Ok((path, artifact))
    }

    fn verify_authoritative_digest(
        &self,
        artifact: &ArtifactRef,
        authoritative: &AuthoritativeArtifactRef,
    ) -> Result<(), ArtifactError> {
        let matches = if let Some(expected) = authoritative.digest.strip_prefix("blake3:") {
            expected.len() == DIGEST_HEX_BYTES
                && expected.bytes().all(is_lower_hex)
                && expected == artifact.digest().hex()
        } else if let Some(expected) = authoritative.digest.strip_prefix("sha256:") {
            let digest = Sha256::digest(self.read(artifact)?);
            expected.len() == DIGEST_HEX_BYTES
                && expected.bytes().all(is_lower_hex)
                && expected == lower_hex(&digest)
        } else {
            return Err(ArtifactError::UnsupportedDigest);
        };
        if !matches {
            return Err(ArtifactError::DigestMismatch);
        }
        Ok(())
    }

    #[must_use]
    pub fn authorized_root(&self) -> &Path {
        &self.authorized
    }

    pub(crate) fn protected_root(&self) -> &Path {
        &self.protected
    }

    pub(crate) fn capture_from_root_if_present(
        &self,
        root: &Path,
        path: &ArtifactPath,
    ) -> Result<Option<ArtifactRef>, ArtifactError> {
        let Some(file) = resolve_contained_file_if_present(root, path)? else {
            return Ok(None);
        };
        let bytes = read_stable_regular_file(&file)?;
        self.store_bytes(&bytes).map(Some)
    }

    pub(crate) fn resolve_authorized_parent(
        &self,
        path: &ArtifactPath,
    ) -> Result<PathBuf, ArtifactError> {
        resolve_contained_parent(&self.authorized, path)
    }

    pub(crate) fn resolve_authorized_file_if_present(
        &self,
        path: &ArtifactPath,
    ) -> Result<Option<PathBuf>, ArtifactError> {
        resolve_contained_file_if_present(&self.authorized, path)
    }

    fn object_path(&self, digest: ContentDigest) -> PathBuf {
        self.objects.join(digest.hex())
    }

    fn unique_staged_path(&self, digest: ContentDigest) -> Result<PathBuf, ArtifactError> {
        for _attempt in 0..32 {
            let sequence = STAGED_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = self.objects.join(format!(
                ".{}.{}.{}.staged",
                digest.hex(),
                std::process::id(),
                sequence
            ));
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
                Ok(_) => {}
                Err(_) => return Err(ArtifactError::ProtectedStoreUnavailable),
            }
        }
        Err(ArtifactError::ProtectedStoreUnavailable)
    }
}

/// Closed artifact failures. Paths and file contents are deliberately omitted
/// from display text so errors are safe to surface in the TUI.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ArtifactError {
    #[error("artifact path is not a portable relative path")]
    InvalidPath,
    #[error("content digest is invalid")]
    InvalidDigest,
    #[error("the authorized working root is unavailable or unsafe")]
    AuthorizedRootUnavailable,
    #[error("the protected artifact store is unavailable or unsafe")]
    ProtectedStoreUnavailable,
    #[error("authorized and protected roots must not overlap")]
    OverlappingRoots,
    #[error("an artifact path contains a symlink or reparse point")]
    RedirectedPath,
    #[error("an artifact path component has the wrong type")]
    WrongFileType,
    #[error("the artifact is missing")]
    ArtifactMissing,
    #[error("the artifact exceeds the local capture limit")]
    ArtifactTooLarge,
    #[error("the artifact changed while it was being captured")]
    ChangedDuringCapture,
    #[error("the retained artifact does not match its content reference")]
    CorruptObject,
    #[error("the requested path has no authoritative Room artifact reference")]
    UnreferencedArtifact,
    #[error("the requested path has ambiguous authoritative Room artifact references")]
    AmbiguousArtifact,
    #[error("an authoritative Room artifact reference is malformed")]
    InvalidReference,
    #[error("the authoritative artifact digest uses an unsupported algorithm")]
    UnsupportedDigest,
    #[error("artifact bytes do not match the authoritative Room digest")]
    DigestMismatch,
    #[error("artifact I/O is unavailable")]
    Unavailable,
}

/// Finds exactly one artifact descriptor whose canonical path names the
/// requested authorized file. This scans only the authenticated Pack
/// projection supplied by the caller; filesystem reachability grants no
/// authority.
///
/// # Errors
///
/// Rejects missing, malformed, or multiple matching references.
pub fn authoritative_artifact_for_path(
    activity: &serde_json::Value,
    authorized_root: &Path,
    requested: &ArtifactPath,
) -> Result<AuthoritativeArtifactRef, ArtifactError> {
    let mut matches = authoritative_artifacts_for_path(activity, authorized_root, requested)?;
    match matches.len() {
        0 => Err(ArtifactError::UnreferencedArtifact),
        1 => matches.pop().ok_or(ArtifactError::UnreferencedArtifact),
        _ => Err(ArtifactError::AmbiguousArtifact),
    }
}

fn authoritative_artifacts_for_path(
    activity: &serde_json::Value,
    authorized_root: &Path,
    requested: &ArtifactPath,
) -> Result<Vec<AuthoritativeArtifactRef>, ArtifactError> {
    let root = portable_absolute(authorized_root)?;
    let mut matches = Vec::new();
    visit_artifact_refs(activity, &mut |value| {
        let Some(local_path) = value.get("local_path").and_then(serde_json::Value::as_str) else {
            return Ok(());
        };
        if relative_authoritative_path(&root, local_path).as_deref() != Some(requested.as_str()) {
            return Ok(());
        }
        let reference: AuthoritativeArtifactRef =
            serde_json::from_value(serde_json::Value::Object(value.clone()))
                .map_err(|_| ArtifactError::InvalidReference)?;
        if !matches.contains(&reference) {
            matches.push(reference);
        }
        Ok(())
    })?;
    Ok(matches)
}

fn visit_artifact_refs(
    value: &serde_json::Value,
    visit: &mut impl FnMut(&serde_json::Map<String, serde_json::Value>) -> Result<(), ArtifactError>,
) -> Result<(), ArtifactError> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                visit_artifact_refs(value, visit)?;
            }
        }
        serde_json::Value::Object(object) => {
            let is_artifact = ["artifact_id", "digest", "local_path", "media_type"]
                .iter()
                .all(|field| object.contains_key(*field));
            if is_artifact {
                visit(object)?;
            }
            for value in object.values() {
                visit_artifact_refs(value, visit)?;
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
    Ok(())
}

fn portable_absolute(path: &Path) -> Result<String, ArtifactError> {
    let value = path.to_str().ok_or(ArtifactError::InvalidReference)?;
    Ok(value.replace('\\', "/").trim_end_matches('/').to_owned())
}

fn relative_authoritative_path(root: &str, path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let relative = normalized.strip_prefix(root)?.strip_prefix('/')?;
    ArtifactPath::new(relative.to_owned())
        .ok()
        .map(|path| path.0)
}

const fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

fn create_authorized_staged_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn read_stable_regular_file(path: &Path) -> Result<Vec<u8>, ArtifactError> {
    read_stable_regular_file_bounded(path, MAX_ARTIFACT_BYTES)
}

fn read_stable_regular_file_bounded(
    path: &Path,
    maximum_bytes: u64,
) -> Result<Vec<u8>, ArtifactError> {
    let path_metadata = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ArtifactError::ArtifactMissing,
        _ => ArtifactError::Unavailable,
    })?;
    ensure_safe_type(&path_metadata, ExpectedType::File)?;
    if path_metadata.len() > maximum_bytes {
        return Err(ArtifactError::ArtifactTooLarge);
    }

    let mut file = File::open(path).map_err(|_| ArtifactError::Unavailable)?;
    let first_identity = file_identity(&file)?;
    let handle_metadata_before = file.metadata().map_err(|_| ArtifactError::Unavailable)?;
    ensure_safe_type(&handle_metadata_before, ExpectedType::File)?;
    let first = read_bounded(&mut file, maximum_bytes)?;
    file.rewind().map_err(|_| ArtifactError::Unavailable)?;
    let second = read_bounded(&mut file, maximum_bytes)?;
    let handle_metadata_after = file.metadata().map_err(|_| ArtifactError::Unavailable)?;
    if first != second || !same_observed_metadata(&handle_metadata_before, &handle_metadata_after) {
        return Err(ArtifactError::ChangedDuringCapture);
    }

    let reopened = File::open(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ArtifactError::ChangedDuringCapture,
        _ => ArtifactError::Unavailable,
    })?;
    let second_identity = file_identity(&reopened)?;
    let path_metadata_after =
        fs::symlink_metadata(path).map_err(|_| ArtifactError::ChangedDuringCapture)?;
    ensure_safe_type(&path_metadata_after, ExpectedType::File)?;
    if first_identity != second_identity
        || !same_observed_metadata(&handle_metadata_after, &path_metadata_after)
    {
        return Err(ArtifactError::ChangedDuringCapture);
    }
    Ok(first)
}

fn read_bounded(file: &mut File, maximum_bytes: u64) -> Result<Vec<u8>, ArtifactError> {
    let mut bytes = Vec::new();
    file.take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ArtifactError::Unavailable)?;
    if u64::try_from(bytes.len()).map_or(true, |length| length > maximum_bytes) {
        return Err(ArtifactError::ArtifactTooLarge);
    }
    Ok(bytes)
}

fn resolve_contained_file_if_present(
    root: &Path,
    relative: &ArtifactPath,
) -> Result<Option<PathBuf>, ArtifactError> {
    let components = relative.0.split('/').collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                let expected = if index + 1 == components.len() {
                    ExpectedType::File
                } else {
                    ExpectedType::Directory
                };
                ensure_safe_type(&metadata, expected)?;
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && index + 1 == components.len() =>
            {
                return Ok(None);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ArtifactError::ArtifactMissing);
            }
            Err(_) => return Err(ArtifactError::Unavailable),
        }
    }
    let canonical = fs::canonicalize(&current).map_err(|_| ArtifactError::Unavailable)?;
    if !canonical.starts_with(root) {
        return Err(ArtifactError::RedirectedPath);
    }
    Ok(Some(current))
}

fn resolve_contained_parent(
    root: &Path,
    relative: &ArtifactPath,
) -> Result<PathBuf, ArtifactError> {
    let components = relative.0.split('/').collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => ArtifactError::ArtifactMissing,
            _ => ArtifactError::Unavailable,
        })?;
        ensure_safe_type(&metadata, ExpectedType::Directory)?;
    }
    let canonical = fs::canonicalize(&current).map_err(|_| ArtifactError::Unavailable)?;
    if !canonical.starts_with(root) {
        return Err(ArtifactError::RedirectedPath);
    }
    Ok(current)
}

#[derive(Clone, Copy)]
enum ExpectedType {
    File,
    Directory,
}

fn ensure_safe_type(metadata: &fs::Metadata, expected: ExpectedType) -> Result<(), ArtifactError> {
    if metadata.file_type().is_symlink() || is_windows_reparse_point(metadata) {
        return Err(ArtifactError::RedirectedPath);
    }
    let matches = match expected {
        ExpectedType::File => metadata.is_file(),
        ExpectedType::Directory => metadata.is_dir(),
    };
    if !matches {
        return Err(ArtifactError::WrongFileType);
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn same_observed_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.file_type() == right.file_type()
        && left.modified().ok() == right.modified().ok()
}

#[cfg(unix)]
fn file_identity(file: &File) -> Result<(u64, u64), ArtifactError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = file.metadata().map_err(|_| ArtifactError::Unavailable)?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(file: &File) -> Result<fs_id::FileID, ArtifactError> {
    fs_id::FileID::new(file).map_err(|_| ArtifactError::Unavailable)
}

#[cfg(not(any(unix, windows)))]
fn file_identity(file: &File) -> Result<(u64, Option<std::time::SystemTime>), ArtifactError> {
    let metadata = file.metadata().map_err(|_| ArtifactError::Unavailable)?;
    Ok((metadata.len(), metadata.modified().ok()))
}

fn validate_relative_path(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > MAX_RELATIVE_PATH_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains(['\\', '\0'])
    {
        return Err(ArtifactError::InvalidPath);
    }
    for segment in value.split('/') {
        if segment.is_empty()
            || matches!(segment, "." | "..")
            || segment.contains(':')
            || segment.ends_with([' ', '.'])
            || windows_reserved_name(segment)
        {
            return Err(ArtifactError::InvalidPath);
        }
    }
    Ok(())
}

fn windows_reserved_name(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment);
    let uppercase = stem.to_ascii_uppercase();
    matches!(uppercase.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || uppercase
            .strip_prefix("COM")
            .or_else(|| uppercase.strip_prefix("LPT"))
            .is_some_and(|number| number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9'))
}

fn make_read_only(path: &Path) -> Result<(), ArtifactError> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| ArtifactError::ProtectedStoreUnavailable)?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|_| ArtifactError::ProtectedStoreUnavailable)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), ArtifactError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ArtifactError::ProtectedStoreUnavailable)
}

#[cfg(not(unix))]
const fn sync_directory(_path: &Path) -> Result<(), ArtifactError> {
    Ok(())
}

fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
