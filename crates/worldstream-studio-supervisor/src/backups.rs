//! Durable, pathless Studio orchestration for verified live backups.

use std::{
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::get,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use worldstream_protocol::{
    BearerWireV1, MAX_MESSAGE_BYTES, OperatorBackupProfileStatus, OperatorBackupStorageHealth,
    OperatorBackupStorageProfile, OperatorBackupVerification, OperatorDataFreshness,
    OperatorLiveBackupStatus,
};
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, prepare_live_backup_root,
};
use zeroize::Zeroizing;

use crate::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const OPERATION_SCHEMA_V1: &str = "worldstream/studio-backup-operation/v1";
const MAX_OPERATION_ID_BYTES: usize = 64;
const MAX_OPERATIONS: usize = 256;

/// Storage profiles surfaced without pretending provider support exists.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupStorageProfileV1 {
    SqliteBundled,
    PostgresPrimary,
    Ephemeral,
}

/// Closed storage health independent from verification and freshness.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupStorageHealthV1 {
    Healthy,
    Unhealthy,
    Unavailable,
}

/// Backup lifecycle persisted across Supervisor restart.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupOperationPhaseV1 {
    Running,
    Retrying,
    Complete,
    Failed,
    Unsupported,
}

/// Verification of the exact published native artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupVerificationV1 {
    Pass,
    Failed,
    Unavailable,
}

/// Freshness remains separate from storage health and verification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BackupFreshnessV1 {
    Fresh { observed_at: String },
    Stale { observed_at: String, reason: String },
    Unavailable { reason: String },
}

/// Bounded destination evidence. It deliberately contains no filesystem path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackupDestinationSummaryV1 {
    pub kind: String,
    pub artifact_name: String,
    pub byte_length: u64,
    pub blake3_digest: String,
}

/// Complete browser-safe operation status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackupOperationStatusV1 {
    pub schema: String,
    pub operation_id: String,
    pub storage_profile: BackupStorageProfileV1,
    pub storage_health: BackupStorageHealthV1,
    pub phase: BackupOperationPhaseV1,
    pub native_verification: BackupVerificationV1,
    pub semantic_verification: BackupVerificationV1,
    pub semantic_verification_reason: Option<String>,
    pub freshness: BackupFreshnessV1,
    pub destination: Option<BackupDestinationSummaryV1>,
    pub failure_reason: Option<String>,
}

/// Profile capability and health independent from any individual operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BackupProfileStatusV1 {
    pub schema: &'static str,
    pub storage_profile: BackupStorageProfileV1,
    pub storage_health: BackupStorageHealthV1,
    pub live_backup_supported: bool,
    pub verification: BackupVerificationV1,
    pub freshness: BackupFreshnessV1,
}

/// Evidence returned only after exact destination bytes were reopened and
/// verified by the storage owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedBackupArtifactV1 {
    pub byte_length: u64,
    pub blake3_digest: String,
    pub observed_at: String,
    pub semantic_verification: BackupVerificationV1,
    pub semantic_verification_reason: Option<String>,
}

/// Closed execution outcomes for supported, unsupported, and failed profiles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackupExecutionErrorV1 {
    Unsupported,
    Unavailable,
    VerificationFailed,
}

/// Storage-owner capability. `destination` is constructed only by this module
/// beneath its fixed owner-only root; browser requests never supply it.
pub trait LiveBackupExecutorV1: Send + Sync + 'static {
    fn storage_profile(&self) -> BackupStorageProfileV1;

    fn storage_health(&self) -> BackupStorageHealthV1 {
        match self.storage_profile() {
            BackupStorageProfileV1::SqliteBundled
            | BackupStorageProfileV1::PostgresPrimary
            | BackupStorageProfileV1::Ephemeral => BackupStorageHealthV1::Healthy,
        }
    }

    fn profile_status(&self) -> BackupProfileStatusV1 {
        let profile = self.storage_profile();
        let health = self.storage_health();
        BackupProfileStatusV1 {
            schema: "worldstream/studio-backup-profile-status/v1",
            storage_profile: profile,
            storage_health: health,
            live_backup_supported: profile == BackupStorageProfileV1::SqliteBundled
                && health == BackupStorageHealthV1::Healthy,
            verification: BackupVerificationV1::Unavailable,
            freshness: BackupFreshnessV1::Unavailable {
                reason: "no_operation_selected".to_owned(),
            },
        }
    }

    /// Creates or reconciles the exact operation artifact idempotently.
    ///
    /// # Errors
    ///
    /// Returns a closed unsupported, unavailable, or verification failure.
    fn execute(
        &self,
        operation_id: &str,
        destination: &Path,
    ) -> Result<VerifiedBackupArtifactV1, BackupExecutionErrorV1>;
}

/// Fixed-address daemon executor using one retained Host backup authority.
#[derive(Clone)]
pub struct HttpDaemonBackupExecutorV1 {
    address: SocketAddr,
    timeout: Duration,
    profile: BackupStorageProfileV1,
    vault: FileSecretVaultV1,
    host_authority: Option<SecretReferenceV1>,
    managed: Option<crate::managed_daemon_transport::ManagedDaemonTransport>,
}

impl HttpDaemonBackupExecutorV1 {
    #[must_use]
    pub const fn new(
        address: SocketAddr,
        timeout: Duration,
        profile: BackupStorageProfileV1,
        vault: FileSecretVaultV1,
        host_authority: Option<SecretReferenceV1>,
    ) -> Self {
        Self {
            address,
            timeout,
            profile,
            vault,
            host_authority,
            managed: None,
        }
    }

    /// Uses proof-bound managed Runtime transport without changing backup policy.
    #[must_use]
    pub fn new_managed(
        address: SocketAddr,
        timeout: Duration,
        profile: BackupStorageProfileV1,
        vault: FileSecretVaultV1,
        host_authority: Option<SecretReferenceV1>,
        ownership: crate::process_ownership::ProcessOwnership,
    ) -> Self {
        Self {
            address,
            timeout,
            profile,
            vault,
            host_authority,
            managed: Some(
                crate::managed_daemon_transport::ManagedDaemonTransport::new(
                    ownership, address, timeout,
                ),
            ),
        }
    }

    fn request_backup(
        &self,
        operation_id: &str,
    ) -> Result<OperatorLiveBackupStatus, BackupExecutionErrorV1> {
        let body = Zeroizing::new(
            serde_json::to_vec(&serde_json::json!({
                "operation_id": operation_id,
            }))
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?,
        );
        self.request_json("POST", "/v1/operator/backups", body.as_slice())
    }

    fn request_profile(&self) -> Result<OperatorBackupProfileStatus, BackupExecutionErrorV1> {
        self.request_json("GET", "/v1/operator/backups/health", &[])
    }

    fn request_json<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<T, BackupExecutionErrorV1> {
        let reference = self
            .host_authority
            .as_ref()
            .ok_or(BackupExecutionErrorV1::Unavailable)?;
        if let Some(transport) = &self.managed {
            let response = transport
                .request(method, path, body, MAX_MESSAGE_BYTES, || {
                    let secret = self
                        .vault
                        .resolve(SecretKindV1::HostAuthority, reference)
                        .map_err(|_| ())?;
                    let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| ())?;
                    let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
                    let token = Zeroizing::new(format!("Bearer {}", bearer.as_str()));
                    let mut header = axum::http::HeaderValue::from_str(&token).map_err(|_| ())?;
                    header.set_sensitive(true);
                    Ok::<_, ()>(header)
                })
                .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
            if response.status != 200 {
                return Err(if response.status == 501 {
                    BackupExecutionErrorV1::Unsupported
                } else {
                    BackupExecutionErrorV1::Unavailable
                });
            }
            return serde_json::from_slice(&response.body)
                .map_err(|_| BackupExecutionErrorV1::Unavailable);
        }
        let secret = self
            .vault
            .resolve(SecretKindV1::HostAuthority, reference)
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        let bytes: [u8; 32] = secret
            .as_bytes()
            .try_into()
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        let bearer = Zeroizing::new(BearerWireV1::from_bytes(bytes).to_wire());
        let mut stream = TcpStream::connect_timeout(&self.address, self.timeout)
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.timeout)))
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        let request = Zeroizing::new(format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            bearer.as_str(),
            body.len()
        ));
        stream
            .write_all(request.as_bytes())
            .and_then(|()| stream.write_all(body))
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut response)
            .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        if response.len() > MAX_MESSAGE_BYTES {
            return Err(BackupExecutionErrorV1::Unavailable);
        }
        let (status, response_body) = parse_http_response(&response)?;
        if status != 200 {
            return Err(if status == 501 {
                BackupExecutionErrorV1::Unsupported
            } else {
                BackupExecutionErrorV1::Unavailable
            });
        }
        serde_json::from_slice(response_body).map_err(|_| BackupExecutionErrorV1::Unavailable)
    }
}

impl LiveBackupExecutorV1 for HttpDaemonBackupExecutorV1 {
    fn storage_profile(&self) -> BackupStorageProfileV1 {
        self.profile
    }

    fn profile_status(&self) -> BackupProfileStatusV1 {
        self.request_profile()
            .ok()
            .and_then(|status| map_profile_status(self.profile, status))
            .unwrap_or_else(|| unavailable_profile_status(self.profile))
    }

    fn execute(
        &self,
        operation_id: &str,
        destination: &Path,
    ) -> Result<VerifiedBackupArtifactV1, BackupExecutionErrorV1> {
        if self.profile != BackupStorageProfileV1::SqliteBundled {
            return Err(BackupExecutionErrorV1::Unsupported);
        }
        let status = self.request_backup(operation_id)?;
        if status.operation_id != operation_id
            || status.storage_profile != OperatorBackupStorageProfile::SqliteBundled
            || status.storage_health != OperatorBackupStorageHealth::Healthy
            || status.native_verification != OperatorBackupVerification::Pass
            || status.semantic_verification == OperatorBackupVerification::Failed
        {
            return Err(BackupExecutionErrorV1::VerificationFailed);
        }
        let artifact = status
            .artifact
            .ok_or(BackupExecutionErrorV1::VerificationFailed)?;
        if artifact.artifact_name != "backup.sqlite3"
            || artifact.semantic_digest.len() != 64
            || !artifact
                .semantic_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || (status.semantic_verification == OperatorBackupVerification::Unavailable
                && status.unavailable_reason.as_deref()
                    != Some("full_semantic_restore_verification_not_run"))
        {
            return Err(BackupExecutionErrorV1::VerificationFailed);
        }
        let exact = fs::read(destination).map_err(|_| BackupExecutionErrorV1::Unavailable)?;
        let digest = blake3::hash(&exact).to_hex().to_string();
        if exact.len() as u64 != artifact.byte_length || digest != artifact.blake3_digest {
            return Err(BackupExecutionErrorV1::VerificationFailed);
        }
        let observed_at = match status.freshness {
            OperatorDataFreshness::Fresh { observed_at } => observed_at,
            OperatorDataFreshness::Stale { .. } | OperatorDataFreshness::Unavailable { .. } => {
                return Err(BackupExecutionErrorV1::VerificationFailed);
            }
        };
        Ok(VerifiedBackupArtifactV1 {
            byte_length: artifact.byte_length,
            blake3_digest: artifact.blake3_digest,
            observed_at,
            semantic_verification: map_verification(status.semantic_verification),
            semantic_verification_reason: match status.semantic_verification {
                OperatorBackupVerification::Pass => None,
                OperatorBackupVerification::Unavailable => status.unavailable_reason,
                OperatorBackupVerification::Failed => {
                    return Err(BackupExecutionErrorV1::VerificationFailed);
                }
            },
        })
    }
}

fn map_profile_status(
    configured: BackupStorageProfileV1,
    status: OperatorBackupProfileStatus,
) -> Option<BackupProfileStatusV1> {
    let profile = map_profile(status.storage_profile);
    (profile == configured).then(|| BackupProfileStatusV1 {
        schema: "worldstream/studio-backup-profile-status/v1",
        storage_profile: profile,
        storage_health: map_health(status.storage_health),
        live_backup_supported: status.live_backup_supported,
        verification: map_verification(status.verification),
        freshness: map_freshness(status.freshness),
    })
}

const fn map_profile(profile: OperatorBackupStorageProfile) -> BackupStorageProfileV1 {
    match profile {
        OperatorBackupStorageProfile::SqliteBundled => BackupStorageProfileV1::SqliteBundled,
        OperatorBackupStorageProfile::PostgresPrimary => BackupStorageProfileV1::PostgresPrimary,
        OperatorBackupStorageProfile::Ephemeral => BackupStorageProfileV1::Ephemeral,
    }
}

const fn map_health(health: OperatorBackupStorageHealth) -> BackupStorageHealthV1 {
    match health {
        OperatorBackupStorageHealth::Healthy => BackupStorageHealthV1::Healthy,
        OperatorBackupStorageHealth::Unhealthy => BackupStorageHealthV1::Unhealthy,
        OperatorBackupStorageHealth::Unavailable => BackupStorageHealthV1::Unavailable,
    }
}

const fn map_verification(verification: OperatorBackupVerification) -> BackupVerificationV1 {
    match verification {
        OperatorBackupVerification::Pass => BackupVerificationV1::Pass,
        OperatorBackupVerification::Failed => BackupVerificationV1::Failed,
        OperatorBackupVerification::Unavailable => BackupVerificationV1::Unavailable,
    }
}

fn map_freshness(freshness: OperatorDataFreshness) -> BackupFreshnessV1 {
    match freshness {
        OperatorDataFreshness::Fresh { observed_at } => BackupFreshnessV1::Fresh { observed_at },
        OperatorDataFreshness::Stale {
            observed_at,
            reason,
        } => BackupFreshnessV1::Stale {
            observed_at,
            reason,
        },
        OperatorDataFreshness::Unavailable { reason } => BackupFreshnessV1::Unavailable { reason },
    }
}

fn unavailable_profile_status(profile: BackupStorageProfileV1) -> BackupProfileStatusV1 {
    BackupProfileStatusV1 {
        schema: "worldstream/studio-backup-profile-status/v1",
        storage_profile: profile,
        storage_health: BackupStorageHealthV1::Unavailable,
        live_backup_supported: false,
        verification: BackupVerificationV1::Unavailable,
        freshness: BackupFreshnessV1::Unavailable {
            reason: "daemon_backup_profile_unavailable".to_owned(),
        },
    }
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, &[u8]), BackupExecutionErrorV1> {
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(BackupExecutionErrorV1::Unavailable)?;
    let headers =
        std::str::from_utf8(&bytes[..split]).map_err(|_| BackupExecutionErrorV1::Unavailable)?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(BackupExecutionErrorV1::Unavailable)?;
    Ok((status, &bytes[(split + 4)..]))
}

#[derive(Clone)]
pub struct BackupOperationsV1 {
    root: PathBuf,
    executor: Arc<dyn LiveBackupExecutorV1>,
    serial: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct BackupIntentV1 {
    schema: String,
    operation_id: String,
    storage_profile: BackupStorageProfileV1,
}

/// Closed pathless persistence failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupOperationErrorV1 {
    InvalidOperationId,
    Unavailable,
    Conflict,
    ConfigurationMismatch,
}

/// Prepares and verifies the one backup root shared by worldstreamd and the
/// Studio Supervisor. Distinct canonical roots fail closed at startup.
///
/// # Errors
///
/// Returns unavailable for unsafe storage and configuration mismatch when the
/// Supervisor state directory does not name the daemon's canonical root.
pub fn prepare_shared_backup_root(
    supervisor_state: &Path,
    daemon_data: &Path,
) -> Result<PathBuf, BackupOperationErrorV1> {
    let supervisor = prepare_data_directory(&supervisor_state.join("backups"))
        .map_err(|_| BackupOperationErrorV1::Unavailable)?;
    let daemon =
        prepare_live_backup_root(daemon_data).map_err(|_| BackupOperationErrorV1::Unavailable)?;
    if supervisor != daemon {
        return Err(BackupOperationErrorV1::ConfigurationMismatch);
    }
    Ok(supervisor)
}

impl BackupOperationsV1 {
    /// Opens the fixed owner-only backup root.
    ///
    /// # Errors
    ///
    /// Returns a pathless error when the root cannot be safely prepared.
    pub fn open(
        root: &Path,
        executor: impl LiveBackupExecutorV1,
    ) -> Result<Self, BackupOperationErrorV1> {
        let root = prepare_data_directory(root).map_err(|_| BackupOperationErrorV1::Unavailable)?;
        Ok(Self {
            root,
            executor: Arc::new(executor),
            serial: Arc::new(Mutex::new(())),
        })
    }

    /// Starts or reconciles one stable operation. Repeating an operation ID
    /// returns its exact terminal record without executing another backup.
    ///
    /// # Errors
    ///
    /// Returns a closed error for invalid identity, durable-state conflict, or
    /// unavailable owner-only persistence.
    pub fn run(
        &self,
        operation_id: &str,
    ) -> Result<BackupOperationStatusV1, BackupOperationErrorV1> {
        validate_operation_id(operation_id)?;
        let _guard = self
            .serial
            .lock()
            .map_err(|_| BackupOperationErrorV1::Unavailable)?;
        let directory = self.operation_directory(operation_id);
        let intent_path = directory.join("intent.json");
        let result_path = directory.join("result.json");
        if let Some(result) = read_json_if_present(&result_path)? {
            return Ok(result);
        }
        let profile = self.executor.storage_profile();
        if let Some(intent) = read_json_if_present::<BackupIntentV1>(&intent_path)? {
            if intent.operation_id != operation_id || intent.storage_profile != profile {
                return Err(BackupOperationErrorV1::Conflict);
            }
        } else {
            prepare_data_directory(&directory).map_err(|_| BackupOperationErrorV1::Unavailable)?;
            publish_json(
                &intent_path,
                &BackupIntentV1 {
                    schema: OPERATION_SCHEMA_V1.to_owned(),
                    operation_id: operation_id.to_owned(),
                    storage_profile: profile,
                },
            )?;
        }
        let destination = directory.join("backup.sqlite3");
        let status = match self.executor.execute(operation_id, &destination) {
            Ok(artifact) => BackupOperationStatusV1 {
                schema: OPERATION_SCHEMA_V1.to_owned(),
                operation_id: operation_id.to_owned(),
                storage_profile: profile,
                storage_health: BackupStorageHealthV1::Healthy,
                phase: BackupOperationPhaseV1::Complete,
                native_verification: BackupVerificationV1::Pass,
                semantic_verification: artifact.semantic_verification,
                semantic_verification_reason: artifact.semantic_verification_reason,
                freshness: BackupFreshnessV1::Fresh {
                    observed_at: artifact.observed_at,
                },
                destination: Some(BackupDestinationSummaryV1 {
                    kind: "studio_managed_local".to_owned(),
                    artifact_name: "backup.sqlite3".to_owned(),
                    byte_length: artifact.byte_length,
                    blake3_digest: artifact.blake3_digest,
                }),
                failure_reason: None,
            },
            Err(BackupExecutionErrorV1::Unsupported) => unsupported_status(operation_id, profile),
            Err(BackupExecutionErrorV1::Unavailable) => {
                return Ok(retrying_status(operation_id, profile));
            }
            Err(BackupExecutionErrorV1::VerificationFailed) => failed_status(
                operation_id,
                profile,
                BackupStorageHealthV1::Healthy,
                BackupVerificationV1::Failed,
                "verification_failed",
            ),
        };
        publish_json(&result_path, &status)?;
        Ok(status)
    }

    /// Reads a terminal status or an explicit running record after restart.
    ///
    /// # Errors
    ///
    /// Returns a closed error for invalid identity or unreadable durable state.
    pub fn status(
        &self,
        operation_id: &str,
    ) -> Result<Option<BackupOperationStatusV1>, BackupOperationErrorV1> {
        let _guard = self
            .serial
            .lock()
            .map_err(|_| BackupOperationErrorV1::Unavailable)?;
        self.status_unlocked(operation_id)
    }

    /// Lists every retained browser-safe backup operation in stable order.
    ///
    /// # Errors
    ///
    /// Fails closed for malformed, unexpected, excessive, or unavailable
    /// protected operation records.
    pub fn statuses(&self) -> Result<Vec<BackupOperationStatusV1>, BackupOperationErrorV1> {
        let _guard = self
            .serial
            .lock()
            .map_err(|_| BackupOperationErrorV1::Unavailable)?;
        let mut operation_ids = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|_| BackupOperationErrorV1::Unavailable)? {
            let entry = entry.map_err(|_| BackupOperationErrorV1::Unavailable)?;
            if !entry
                .file_type()
                .map_err(|_| BackupOperationErrorV1::Unavailable)?
                .is_dir()
            {
                return Err(BackupOperationErrorV1::Unavailable);
            }
            let operation_id = entry
                .file_name()
                .into_string()
                .map_err(|_| BackupOperationErrorV1::Unavailable)?;
            validate_operation_id(&operation_id)?;
            operation_ids.push(operation_id);
            if operation_ids.len() > MAX_OPERATIONS {
                return Err(BackupOperationErrorV1::Unavailable);
            }
        }
        operation_ids.sort();
        if operation_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(BackupOperationErrorV1::Unavailable);
        }
        operation_ids
            .into_iter()
            .map(|operation_id| {
                self.status_unlocked(&operation_id)?
                    .ok_or(BackupOperationErrorV1::Unavailable)
            })
            .collect()
    }

    fn status_unlocked(
        &self,
        operation_id: &str,
    ) -> Result<Option<BackupOperationStatusV1>, BackupOperationErrorV1> {
        validate_operation_id(operation_id)?;
        let directory = self.operation_directory(operation_id);
        if let Some(result) = read_json_if_present(&directory.join("result.json"))? {
            return Ok(Some(result));
        }
        let Some(intent) = read_json_if_present::<BackupIntentV1>(&directory.join("intent.json"))?
        else {
            return Ok(None);
        };
        let profile_status = self.executor.profile_status();
        Ok(Some(BackupOperationStatusV1 {
            schema: OPERATION_SCHEMA_V1.to_owned(),
            operation_id: intent.operation_id,
            storage_profile: intent.storage_profile,
            storage_health: profile_status.storage_health,
            phase: BackupOperationPhaseV1::Retrying,
            native_verification: BackupVerificationV1::Unavailable,
            semantic_verification: BackupVerificationV1::Unavailable,
            semantic_verification_reason: Some("operation_reconciliation_required".to_owned()),
            freshness: BackupFreshnessV1::Unavailable {
                reason: "operation_reconciliation_required".to_owned(),
            },
            destination: None,
            failure_reason: Some("operation_reconciliation_required".to_owned()),
        }))
    }

    #[must_use]
    pub fn profile_status(&self) -> BackupProfileStatusV1 {
        self.executor.profile_status()
    }

    fn operation_directory(&self, operation_id: &str) -> PathBuf {
        self.root.join(operation_id)
    }
}

/// Builds the bounded, pathless profile and operation API.
pub fn backup_router(operations: BackupOperationsV1) -> Router {
    Router::new()
        .route("/api/v1/backups/health", get(backup_health))
        .route(
            "/api/v1/backups/{operation_id}",
            get(backup_status).post(run_backup),
        )
        .with_state(operations)
}

async fn backup_health(
    State(operations): State<BackupOperationsV1>,
) -> Result<Json<BackupProfileStatusV1>, StatusCode> {
    tokio::task::spawn_blocking(move || operations.profile_status())
        .await
        .map(Json)
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

async fn backup_status(
    State(operations): State<BackupOperationsV1>,
    AxumPath(operation_id): AxumPath<String>,
) -> Result<Json<Option<BackupOperationStatusV1>>, StatusCode> {
    tokio::task::spawn_blocking(move || operations.status(&operation_id))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map(Json)
        .map_err(status_for_operation_error)
}

async fn run_backup(
    State(operations): State<BackupOperationsV1>,
    AxumPath(operation_id): AxumPath<String>,
) -> Result<Json<BackupOperationStatusV1>, StatusCode> {
    tokio::task::spawn_blocking(move || operations.run(&operation_id))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map(Json)
        .map_err(status_for_operation_error)
}

fn status_for_operation_error(error: BackupOperationErrorV1) -> StatusCode {
    match error {
        BackupOperationErrorV1::InvalidOperationId => StatusCode::BAD_REQUEST,
        BackupOperationErrorV1::Conflict => StatusCode::CONFLICT,
        BackupOperationErrorV1::ConfigurationMismatch | BackupOperationErrorV1::Unavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

fn unsupported_status(
    operation_id: &str,
    profile: BackupStorageProfileV1,
) -> BackupOperationStatusV1 {
    BackupOperationStatusV1 {
        schema: OPERATION_SCHEMA_V1.to_owned(),
        operation_id: operation_id.to_owned(),
        storage_profile: profile,
        storage_health: BackupStorageHealthV1::Healthy,
        phase: BackupOperationPhaseV1::Unsupported,
        native_verification: BackupVerificationV1::Unavailable,
        semantic_verification: BackupVerificationV1::Unavailable,
        semantic_verification_reason: Some("profile_managed_backup_required".to_owned()),
        freshness: BackupFreshnessV1::Unavailable {
            reason: "profile_managed_backup_required".to_owned(),
        },
        destination: None,
        failure_reason: None,
    }
}

fn failed_status(
    operation_id: &str,
    profile: BackupStorageProfileV1,
    storage_health: BackupStorageHealthV1,
    native_verification: BackupVerificationV1,
    reason: &str,
) -> BackupOperationStatusV1 {
    BackupOperationStatusV1 {
        schema: OPERATION_SCHEMA_V1.to_owned(),
        operation_id: operation_id.to_owned(),
        storage_profile: profile,
        storage_health,
        phase: BackupOperationPhaseV1::Failed,
        native_verification,
        semantic_verification: BackupVerificationV1::Unavailable,
        semantic_verification_reason: Some(reason.to_owned()),
        freshness: BackupFreshnessV1::Unavailable {
            reason: reason.to_owned(),
        },
        destination: None,
        failure_reason: Some(reason.to_owned()),
    }
}

fn retrying_status(operation_id: &str, profile: BackupStorageProfileV1) -> BackupOperationStatusV1 {
    BackupOperationStatusV1 {
        schema: OPERATION_SCHEMA_V1.to_owned(),
        operation_id: operation_id.to_owned(),
        storage_profile: profile,
        storage_health: BackupStorageHealthV1::Unavailable,
        phase: BackupOperationPhaseV1::Retrying,
        native_verification: BackupVerificationV1::Unavailable,
        semantic_verification: BackupVerificationV1::Unavailable,
        semantic_verification_reason: Some("operation_reconciliation_required".to_owned()),
        freshness: BackupFreshnessV1::Unavailable {
            reason: "storage_unavailable".to_owned(),
        },
        destination: None,
        failure_reason: Some("storage_unavailable".to_owned()),
    }
}

fn validate_operation_id(value: &str) -> Result<(), BackupOperationErrorV1> {
    if value.is_empty()
        || value.len() > MAX_OPERATION_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        return Err(BackupOperationErrorV1::InvalidOperationId);
    }
    Ok(())
}

fn publish_json(path: &Path, value: &impl Serialize) -> Result<(), BackupOperationErrorV1> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackupOperationErrorV1::Unavailable)?;
    let parent = path.parent().ok_or(BackupOperationErrorV1::Unavailable)?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| BackupOperationErrorV1::Unavailable)?;
    let temporary = parent.join(format!(".backup-state-{}.tmp", lower_hex(&nonce)));
    let mut file =
        create_owner_only_file(&temporary).map_err(|_| BackupOperationErrorV1::Unavailable)?;
    let write = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| BackupOperationErrorV1::Unavailable);
    drop(file);
    if write.is_err() {
        let _ = fs::remove_file(&temporary);
        return write;
    }
    let published = fs::hard_link(&temporary, path).map_err(|_| {
        if path.exists() {
            BackupOperationErrorV1::Conflict
        } else {
            BackupOperationErrorV1::Unavailable
        }
    });
    let _ = fs::remove_file(&temporary);
    published?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| BackupOperationErrorV1::Unavailable)
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

fn read_json_if_present<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<Option<T>, BackupOperationErrorV1> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|_| BackupOperationErrorV1::Unavailable)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| BackupOperationErrorV1::Conflict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use std::{
        fs,
        net::TcpListener,
        sync::{
            Arc, PoisonError,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
    };
    use tempfile::tempdir;
    use tower::ServiceExt as _;

    fn http_executor_with_response(
        vault_root: &Path,
        response_body: Vec<u8>,
    ) -> (HttpDaemonBackupExecutorV1, thread::JoinHandle<()>) {
        let vault = FileSecretVaultV1::open(vault_root)
            .unwrap_or_else(|error| unreachable!("vault: {error}"));
        let reference = vault
            .store(SecretKindV1::HostAuthority, &[0xab; 32])
            .unwrap_or_else(|error| unreachable!("authority: {error}"));
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| unreachable!("listener: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("address: {error}"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .unwrap_or_else(|error| unreachable!("accept: {error}"));
            let mut request = vec![0_u8; 4096];
            let _ = stream
                .read(&mut request)
                .unwrap_or_else(|error| unreachable!("read: {error}"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .and_then(|()| stream.write_all(&response_body))
            .unwrap_or_else(|error| unreachable!("write: {error}"));
        });
        (
            HttpDaemonBackupExecutorV1::new(
                address,
                Duration::from_secs(2),
                BackupStorageProfileV1::SqliteBundled,
                vault,
                Some(reference),
            ),
            server,
        )
    }

    #[derive(Clone)]
    struct FixedExecutor {
        profile: BackupStorageProfileV1,
        calls: Arc<AtomicUsize>,
    }
    impl LiveBackupExecutorV1 for FixedExecutor {
        fn storage_profile(&self) -> BackupStorageProfileV1 {
            self.profile
        }
        fn execute(
            &self,
            _operation_id: &str,
            destination: &Path,
        ) -> Result<VerifiedBackupArtifactV1, BackupExecutionErrorV1> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.profile != BackupStorageProfileV1::SqliteBundled {
                return Err(BackupExecutionErrorV1::Unsupported);
            }
            if !destination.exists() {
                fs::write(destination, b"verified-backup")
                    .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
            }
            let bytes = fs::read(destination).map_err(|_| BackupExecutionErrorV1::Unavailable)?;
            Ok(VerifiedBackupArtifactV1 {
                byte_length: bytes.len() as u64,
                blake3_digest: blake3::hash(&bytes).to_hex().to_string(),
                observed_at: "2026-08-23T12:00:00Z".to_owned(),
                semantic_verification: BackupVerificationV1::Unavailable,
                semantic_verification_reason: Some(
                    "full_semantic_restore_verification_not_run".to_owned(),
                ),
            })
        }
    }

    #[test]
    fn stable_operation_is_exactly_once_and_pathless_after_restart() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = FixedExecutor {
            profile: BackupStorageProfileV1::SqliteBundled,
            calls: Arc::clone(&calls),
        };
        let operations = BackupOperationsV1::open(&temp.path().join("backups"), executor.clone())
            .unwrap_or_else(|error| unreachable!("open: {error:?}"));
        let first = operations
            .run("backup-2026-08-23")
            .unwrap_or_else(|error| unreachable!("run: {error:?}"));
        drop(operations);
        let reopened = BackupOperationsV1::open(&temp.path().join("backups"), executor)
            .unwrap_or_else(|error| unreachable!("reopen: {error:?}"));
        let second = reopened
            .run("backup-2026-08-23")
            .unwrap_or_else(|error| unreachable!("rerun: {error:?}"));
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(second.phase, BackupOperationPhaseV1::Complete);
        let json =
            serde_json::to_string(&second).unwrap_or_else(|error| unreachable!("json: {error}"));
        assert!(!json.contains(temp.path().to_string_lossy().as_ref()));
        assert!(!json.contains("\"path\""));
    }

    #[test]
    fn startup_accepts_only_the_daemon_canonical_backup_root() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let data = temp.path().join("data");
        let expected = fs::canonicalize(temp.path())
            .unwrap_or_else(|error| unreachable!("canonical temp: {error}"))
            .join("studio/backups");
        assert_eq!(
            prepare_shared_backup_root(&temp.path().join("studio"), &data)
                .unwrap_or_else(|error| unreachable!("shared root: {error:?}")),
            expected
        );
        assert_eq!(
            prepare_shared_backup_root(&temp.path().join("different-studio"), &data),
            Err(BackupOperationErrorV1::ConfigurationMismatch)
        );
    }

    #[test]
    fn unsupported_provider_is_truthful_and_creates_no_artifact() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let operations = BackupOperationsV1::open(
            &temp.path().join("backups"),
            FixedExecutor {
                profile: BackupStorageProfileV1::PostgresPrimary,
                calls: Arc::new(AtomicUsize::new(0)),
            },
        )
        .unwrap_or_else(|error| unreachable!("open: {error:?}"));
        let status = operations
            .run("provider-backup")
            .unwrap_or_else(|error| unreachable!("run: {error:?}"));
        assert_eq!(status.phase, BackupOperationPhaseV1::Unsupported);
        assert!(status.destination.is_none());
    }

    #[test]
    fn restart_reconciles_an_artifact_published_before_terminal_result() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let root = temp.path().join("backups");
        let operation = root.join("crash-window");
        prepare_data_directory(&operation)
            .unwrap_or_else(|error| unreachable!("operation directory: {error}"));
        publish_json(
            &operation.join("intent.json"),
            &BackupIntentV1 {
                schema: OPERATION_SCHEMA_V1.to_owned(),
                operation_id: "crash-window".to_owned(),
                storage_profile: BackupStorageProfileV1::SqliteBundled,
            },
        )
        .unwrap_or_else(|error| unreachable!("intent: {error:?}"));
        fs::write(operation.join("backup.sqlite3"), b"verified-backup")
            .unwrap_or_else(|error| unreachable!("artifact: {error}"));
        let calls = Arc::new(AtomicUsize::new(0));
        let operations = BackupOperationsV1::open(
            &root,
            FixedExecutor {
                profile: BackupStorageProfileV1::SqliteBundled,
                calls: Arc::clone(&calls),
            },
        )
        .unwrap_or_else(|error| unreachable!("reopen: {error:?}"));

        assert_eq!(
            operations
                .status("crash-window")
                .unwrap_or_else(|error| unreachable!("status: {error:?}"))
                .map(|status| status.phase),
            Some(BackupOperationPhaseV1::Retrying)
        );
        let reconciled = operations
            .run("crash-window")
            .unwrap_or_else(|error| unreachable!("reconcile: {error:?}"));
        assert_eq!(reconciled.phase, BackupOperationPhaseV1::Complete);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[derive(Clone)]
    struct TemporarilyUnavailableExecutor {
        attempts: Arc<AtomicUsize>,
    }

    impl LiveBackupExecutorV1 for TemporarilyUnavailableExecutor {
        fn storage_profile(&self) -> BackupStorageProfileV1 {
            BackupStorageProfileV1::SqliteBundled
        }

        fn execute(
            &self,
            _operation_id: &str,
            destination: &Path,
        ) -> Result<VerifiedBackupArtifactV1, BackupExecutionErrorV1> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                fs::write(destination, b"possibly-published")
                    .map_err(|_| BackupExecutionErrorV1::Unavailable)?;
                return Err(BackupExecutionErrorV1::Unavailable);
            }
            let bytes = fs::read(destination).map_err(|_| BackupExecutionErrorV1::Unavailable)?;
            Ok(VerifiedBackupArtifactV1 {
                byte_length: bytes.len() as u64,
                blake3_digest: blake3::hash(&bytes).to_hex().to_string(),
                observed_at: "2026-08-23T12:00:00Z".to_owned(),
                semantic_verification: BackupVerificationV1::Unavailable,
                semantic_verification_reason: Some(
                    "full_semantic_restore_verification_not_run".to_owned(),
                ),
            })
        }
    }

    #[test]
    fn unavailable_transport_remains_retryable_and_reconciles_the_original_operation() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let attempts = Arc::new(AtomicUsize::new(0));
        let root = temp.path().join("backups");
        let first = BackupOperationsV1::open(
            &root,
            TemporarilyUnavailableExecutor {
                attempts: Arc::clone(&attempts),
            },
        )
        .unwrap_or_else(|error| unreachable!("open: {error:?}"))
        .run("retry-original")
        .unwrap_or_else(|error| unreachable!("first: {error:?}"));
        assert_eq!(first.phase, BackupOperationPhaseV1::Retrying);
        assert!(!root.join("retry-original/result.json").exists());

        let reopened = BackupOperationsV1::open(
            &root,
            TemporarilyUnavailableExecutor {
                attempts: Arc::clone(&attempts),
            },
        )
        .unwrap_or_else(|error| unreachable!("reopen: {error:?}"));
        assert_eq!(
            reopened
                .status("retry-original")
                .unwrap_or_else(|error| unreachable!("status: {error:?}"))
                .map(|status| status.phase),
            Some(BackupOperationPhaseV1::Retrying)
        );
        let statuses = reopened
            .statuses()
            .unwrap_or_else(|error| unreachable!("backup statuses: {error:?}"));
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].operation_id, "retry-original");
        assert_eq!(statuses[0].phase, BackupOperationPhaseV1::Retrying);
        let complete = reopened
            .run("retry-original")
            .unwrap_or_else(|error| unreachable!("retry: {error:?}"));
        assert_eq!(complete.phase, BackupOperationPhaseV1::Complete);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalid_success_response_remains_nonterminal_for_same_operation() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let (executor, server) =
            http_executor_with_response(&temp.path().join("vault"), b"not-json".to_vec());
        let root = temp.path().join("backups");
        let status = BackupOperationsV1::open(&root, executor)
            .unwrap_or_else(|error| unreachable!("open: {error:?}"))
            .run("invalid-response")
            .unwrap_or_else(|error| unreachable!("run: {error:?}"));
        server
            .join()
            .unwrap_or_else(|_| unreachable!("server thread"));
        assert_eq!(status.phase, BackupOperationPhaseV1::Retrying);
        assert!(!root.join("invalid-response/result.json").exists());
        assert_eq!(
            parse_http_response(b"not an HTTP response"),
            Err(BackupExecutionErrorV1::Unavailable)
        );
    }

    #[test]
    fn unreadable_published_artifact_remains_nonterminal_for_same_operation() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let body = serde_json::to_vec(&OperatorLiveBackupStatus {
            operation_id: "missing-local-artifact".to_owned(),
            storage_profile: OperatorBackupStorageProfile::SqliteBundled,
            storage_health: OperatorBackupStorageHealth::Healthy,
            native_verification: OperatorBackupVerification::Pass,
            semantic_verification: OperatorBackupVerification::Unavailable,
            freshness: OperatorDataFreshness::Fresh {
                observed_at: "2026-08-24T00:00:00Z".to_owned(),
            },
            artifact: Some(worldstream_protocol::OperatorLiveBackupArtifactSummary {
                artifact_name: "backup.sqlite3".to_owned(),
                byte_length: 1,
                blake3_digest: "a".repeat(64),
                semantic_digest: "b".repeat(64),
            }),
            unavailable_reason: Some("full_semantic_restore_verification_not_run".to_owned()),
        })
        .unwrap_or_else(|error| unreachable!("response: {error}"));
        let (executor, server) = http_executor_with_response(&temp.path().join("vault"), body);
        let root = temp.path().join("backups");
        let status = BackupOperationsV1::open(&root, executor)
            .unwrap_or_else(|error| unreachable!("open: {error:?}"))
            .run("missing-local-artifact")
            .unwrap_or_else(|error| unreachable!("run: {error:?}"));
        server
            .join()
            .unwrap_or_else(|_| unreachable!("server thread"));
        assert_eq!(status.phase, BackupOperationPhaseV1::Retrying);
        assert!(!root.join("missing-local-artifact/result.json").exists());
    }

    #[derive(Clone)]
    struct ReportedUnhealthyExecutor;

    impl LiveBackupExecutorV1 for ReportedUnhealthyExecutor {
        fn storage_profile(&self) -> BackupStorageProfileV1 {
            BackupStorageProfileV1::SqliteBundled
        }

        fn profile_status(&self) -> BackupProfileStatusV1 {
            BackupProfileStatusV1 {
                schema: "worldstream/studio-backup-profile-status/v1",
                storage_profile: BackupStorageProfileV1::SqliteBundled,
                storage_health: BackupStorageHealthV1::Unhealthy,
                live_backup_supported: false,
                verification: BackupVerificationV1::Failed,
                freshness: BackupFreshnessV1::Stale {
                    observed_at: "2026-08-23T11:00:00Z".to_owned(),
                    reason: "verification_expired".to_owned(),
                },
            }
        }

        fn execute(
            &self,
            _operation_id: &str,
            _destination: &Path,
        ) -> Result<VerifiedBackupArtifactV1, BackupExecutionErrorV1> {
            Err(BackupExecutionErrorV1::Unavailable)
        }
    }

    #[test]
    fn profile_status_preserves_daemon_reported_health_verification_and_freshness() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let status =
            BackupOperationsV1::open(&temp.path().join("backups"), ReportedUnhealthyExecutor)
                .unwrap_or_else(|error| unreachable!("open: {error:?}"))
                .profile_status();
        assert_eq!(status.storage_health, BackupStorageHealthV1::Unhealthy);
        assert_eq!(status.verification, BackupVerificationV1::Failed);
        assert!(matches!(status.freshness, BackupFreshnessV1::Stale { .. }));
        assert!(!status.live_backup_supported);
    }

    #[test]
    fn http_profile_status_is_loaded_from_the_daemon_health_endpoint() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let vault = FileSecretVaultV1::open(&temp.path().join("vault"))
            .unwrap_or_else(|error| unreachable!("vault: {error}"));
        let reference = vault
            .store(SecretKindV1::HostAuthority, &[0xab; 32])
            .unwrap_or_else(|error| unreachable!("authority: {error}"));
        let response = serde_json::to_vec(&OperatorBackupProfileStatus {
            storage_profile: OperatorBackupStorageProfile::SqliteBundled,
            storage_health: OperatorBackupStorageHealth::Unhealthy,
            live_backup_supported: false,
            verification: OperatorBackupVerification::Failed,
            freshness: OperatorDataFreshness::Stale {
                observed_at: "2026-08-23T11:00:00Z".to_owned(),
                reason: "verification_expired".to_owned(),
            },
        })
        .unwrap_or_else(|error| unreachable!("response: {error}"));
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|error| unreachable!("listener: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("address: {error}"));
        let observed = Arc::new(Mutex::new(Vec::new()));
        let server_observed = Arc::clone(&observed);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .unwrap_or_else(|error| unreachable!("accept: {error}"));
            let mut request = vec![0_u8; 4096];
            let count = stream
                .read(&mut request)
                .unwrap_or_else(|error| unreachable!("read: {error}"));
            request.truncate(count);
            *server_observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = request;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )
            .and_then(|()| stream.write_all(&response))
            .unwrap_or_else(|error| unreachable!("write: {error}"));
        });
        let operations = BackupOperationsV1::open(
            &temp.path().join("backups"),
            HttpDaemonBackupExecutorV1::new(
                address,
                Duration::from_secs(2),
                BackupStorageProfileV1::SqliteBundled,
                vault,
                Some(reference),
            ),
        )
        .unwrap_or_else(|error| unreachable!("operations: {error:?}"));

        let profile = operations.profile_status();
        server
            .join()
            .unwrap_or_else(|_| unreachable!("server thread"));
        let request = String::from_utf8(
            observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap_or_else(|error| unreachable!("request: {error}"));
        assert!(request.starts_with("GET /v1/operator/backups/health HTTP/1.1"));
        assert_eq!(profile.storage_health, BackupStorageHealthV1::Unhealthy);
        assert_eq!(profile.verification, BackupVerificationV1::Failed);
        assert!(matches!(profile.freshness, BackupFreshnessV1::Stale { .. }));
    }

    #[tokio::test]
    async fn router_reports_profile_health_and_runs_only_stable_identifiers() {
        let temp = tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
        let operations = BackupOperationsV1::open(
            &temp.path().join("backups"),
            FixedExecutor {
                profile: BackupStorageProfileV1::SqliteBundled,
                calls: Arc::new(AtomicUsize::new(0)),
            },
        )
        .unwrap_or_else(|error| unreachable!("open: {error:?}"));
        let health = backup_router(operations.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/backups/health")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("health request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("health response: {error}"));
        assert_eq!(health.status(), 200);
        let health_bytes = health
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| unreachable!("health body: {error}"))
            .to_bytes();
        let health_json: serde_json::Value = serde_json::from_slice(&health_bytes)
            .unwrap_or_else(|error| unreachable!("health JSON: {error}"));
        assert_eq!(health_json["storage_health"], "healthy");
        assert_eq!(health_json["verification"], "unavailable");

        let run = backup_router(operations.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/backups/stable-operation")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("run request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("run response: {error}"));
        assert_eq!(run.status(), 200);

        let invalid = backup_router(operations)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/backups/not%2Fa%2Fpath")
                    .body(Body::empty())
                    .unwrap_or_else(|error| unreachable!("invalid request: {error}")),
            )
            .await
            .unwrap_or_else(|error| unreachable!("invalid response: {error}"));
        assert_ne!(invalid.status(), 200);
    }
}
