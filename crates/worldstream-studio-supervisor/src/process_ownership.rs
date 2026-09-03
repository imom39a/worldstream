//! Protected launch generations and lifetime leases for the two managed processes.
//!
//! A PID is diagnostic data, never authority to signal or adopt a process.

use std::{
    fs,
    io::{self, Write as _},
    net::SocketAddr,
    path::{Path, PathBuf},
};

use thiserror::Error;
use worldstream_runtime::{
    SecretSource, create_owner_only_file, validate_data_directory, validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::protected_publication::{PublicationMode, publish};
use crate::verified_control::{ProofIdentity, ProofService};

const MAGIC: &[u8; 8] = b"WSPROC01";
const RECORD_BYTES: usize = 174;

/// The only process roles managed by the local installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    /// Persistent local operator controller.
    Controller,
    /// Configured `WorldStream` Runtime.
    Runtime,
}

/// Retained process phase, not evidence of present process liveness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessPhase {
    /// A launch is pending or the child is initializing.
    Starting,
    /// The process published its managed endpoint; connection proof is still required.
    Ready,
    /// The generation explicitly completed a clean stop.
    Stopped,
    /// The generation failed or was explicitly fenced during recovery.
    Failed,
}

/// Terminal outcome explicitly published by the process holding its lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTermination {
    /// The owned process completed its shutdown work.
    Stopped,
    /// The owned process failed safely.
    Failed,
}

/// Non-secret retained metadata. Callers must verify an endpoint before control;
/// neither this record nor its diagnostic PID proves a live process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSnapshot {
    /// Non-secret launch generation.
    pub generation: String,
    /// Retained lifecycle phase.
    pub phase: ProcessPhase,
    /// Diagnostic PID recorded when the child claimed the launch.
    pub pid: Option<u32>,
    /// Published literal endpoint, never authority without a fresh connection proof.
    pub endpoint: Option<SocketAddr>,
}

impl ProcessRole {
    fn name(self) -> &'static str {
        match self {
            Self::Controller => "controller",
            Self::Runtime => "runtime",
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Controller => 1,
            Self::Runtime => 2,
        }
    }
}

/// Closed ownership failures, containing no paths, process arguments, or keys.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum OwnershipError {
    /// A record or its protected filesystem location cannot be trusted.
    #[error("managed process ownership is invalid or unsafe")]
    Invalid,
    /// Another process holds the lease, or an uncertain generation remains.
    #[error("managed process ownership is unavailable or unresolved")]
    Unavailable,
    /// The child no longer owns the current pending launch generation.
    #[error("managed process launch generation is stale")]
    StaleGeneration,
    /// Publication may have changed the record; callers must inspect it.
    #[error("managed process ownership publication is uncertain")]
    PublicationUncertain,
}

/// Public launch reference; never contains the private generation key.
pub struct LaunchClaim {
    generation: String,
}

impl LaunchClaim {
    /// Opaque, non-secret reference passed to the intended child.
    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }
}

/// Access to the protected ownership records in one existing installation.
#[derive(Clone)]
pub struct ProcessOwnership {
    state: PathBuf,
    installation: [u8; 32],
}

/// Retained OS lock for one claimed generation. Dropping it does not assert a
/// successful stop: the retained record remains unresolved until reconciled.
pub struct ProcessLease {
    _lock: fs::File,
    ownership: ProcessOwnership,
    role: ProcessRole,
    generation: String,
    proof: Option<ProofService>,
}

impl ProcessLease {
    /// Publishes the actual exclusively held listening address and enables proof.
    /// The caller must retain that listener until proof/control connections drain,
    /// disabling proof before releasing the listener. No address reuse is permitted.
    ///
    /// # Errors
    /// Rejects non-loopback/zero endpoints, repeated publication, or changed ownership.
    pub fn publish_endpoint(
        &mut self,
        endpoint: SocketAddr,
    ) -> Result<ProofService, OwnershipError> {
        if !endpoint.ip().is_loopback() || endpoint.port() == 0 || self.proof.is_some() {
            return Err(OwnershipError::Invalid);
        }
        let mut record = self
            .ownership
            .read_record(self.role)?
            .ok_or(OwnershipError::Invalid)?;
        if hex(&record[9..41]) != self.generation || record[109] != 1 {
            return Err(OwnershipError::StaleGeneration);
        }
        let address = endpoint.to_string();
        record[110..].fill(0);
        record[110..110 + address.len()].copy_from_slice(address.as_bytes());
        record[109] = 2;
        self.ownership
            .publish_record(self.role, &record[..], PublicationMode::Replace)?;
        let proof = ProofService::new(self.ownership.proof_identity(self.role)?);
        self.proof = Some(proof.clone());
        Ok(proof)
    }

    /// Disables proof and derived control admission before releasing a listener.
    pub fn stop_proving(&self) {
        if let Some(proof) = &self.proof {
            proof.disable();
        }
    }

    /// Publishes a terminal outcome while still retaining the lifetime lock.
    ///
    /// # Errors
    /// Fails closed on changed ownership or uncertain publication; never asserts rollback.
    pub fn finish(self, outcome: ProcessTermination) -> Result<(), OwnershipError> {
        self.stop_proving();
        let mut record = self
            .ownership
            .read_record(self.role)?
            .ok_or(OwnershipError::Invalid)?;
        if hex(&record[9..41]) != self.generation {
            return Err(OwnershipError::StaleGeneration);
        }
        record[109] = match outcome {
            ProcessTermination::Stopped => 3,
            ProcessTermination::Failed => 4,
        };
        self.ownership
            .publish_record(self.role, &record[..], PublicationMode::Replace)
    }
}

impl Drop for ProcessLease {
    fn drop(&mut self) {
        self.stop_proving();
    }
}

impl ProcessOwnership {
    /// Opens existing protected state without creating or repairing anything.
    ///
    /// # Errors
    /// Rejects missing or unsafe state directories.
    pub fn open(state: &Path) -> Result<Self, OwnershipError> {
        let state = validate_data_directory(state).map_err(|_| OwnershipError::Invalid)?;
        let installation = *blake3::hash(state.as_os_str().as_encoded_bytes()).as_bytes();
        Ok(Self {
            state,
            installation,
        })
    }

    /// Reads retained non-secret metadata without creating files or probing processes.
    ///
    /// # Errors
    /// Rejects incomplete, unsafe, or wrong-installation records.
    pub fn snapshot(&self, role: ProcessRole) -> Result<Option<ProcessSnapshot>, OwnershipError> {
        self.read_record(role)?
            .map(|record| {
                let pid = u32::from_le_bytes(
                    record[105..109]
                        .try_into()
                        .map_err(|_| OwnershipError::Invalid)?,
                );
                Ok(ProcessSnapshot {
                    generation: hex(&record[9..41]),
                    phase: match record[109] {
                        0 | 1 => ProcessPhase::Starting,
                        2 => ProcessPhase::Ready,
                        3 => ProcessPhase::Stopped,
                        4 => ProcessPhase::Failed,
                        _ => return Err(OwnershipError::Invalid),
                    },
                    pid: (pid != 0).then_some(pid),
                    endpoint: record_endpoint(&record)?,
                })
            })
            .transpose()
    }

    /// Observes the existing lifetime lock without creating files or interpreting
    /// retained process phase as liveness. A released unresolved record is not adoptable.
    ///
    /// # Errors
    /// Rejects unsafe/incomplete records or lock files and non-contention OS errors.
    pub fn is_leased(&self, role: ProcessRole) -> Result<bool, OwnershipError> {
        let has_record = self.read_record(role)?.is_some();
        let path = self.state.join(format!("managed-{}.lock", role.name()));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return if has_record {
                    Err(OwnershipError::Invalid)
                } else {
                    Ok(false)
                };
            }
            Err(_) => return Err(OwnershipError::Unavailable),
            Ok(_) => {}
        }
        if !has_record {
            return Err(OwnershipError::Invalid);
        }
        validate_owner_only_file(&path).map_err(|_| OwnershipError::Invalid)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            options.share_mode(0x0000_0001 | 0x0000_0002);
        }
        let lock = options
            .open(&path)
            .map_err(|_| OwnershipError::Unavailable)?;
        validate_owner_only_file(&path).map_err(|_| OwnershipError::Invalid)?;
        match lock.try_lock() {
            Ok(()) => Ok(false),
            Err(fs::TryLockError::WouldBlock) => Ok(true),
            Err(fs::TryLockError::Error(_)) => Err(OwnershipError::Unavailable),
        }
    }

    /// Reserves a fresh launch while holding the same lock the child must claim.
    /// Existing uncertain records never authorize replacement or a duplicate.
    ///
    /// # Errors
    /// Rejects contention, retained generations, unsafe storage, and uncertain publication.
    pub fn reserve(&self, role: ProcessRole) -> Result<LaunchClaim, OwnershipError> {
        let (_lock, created) = self.lock(role, true)?;
        let mode = match self.read_record(role)? {
            None if created => PublicationMode::CreateNew,
            None => return Err(OwnershipError::Invalid),
            Some(record) if matches!(record[109], 3 | 4) => PublicationMode::Replace,
            Some(_) => return Err(OwnershipError::Unavailable),
        };
        let mut record = Zeroizing::new([0_u8; RECORD_BYTES]);
        record[..8].copy_from_slice(MAGIC);
        record[8] = role.byte();
        getrandom::fill(&mut record[9..73]).map_err(|_| OwnershipError::Unavailable)?;
        record[73..105].copy_from_slice(&self.installation);
        self.publish_record(role, &record[..], mode)?;
        Ok(LaunchClaim {
            generation: hex(&record[9..41]),
        })
    }

    /// Claims only the current pending generation, retaining its exclusive lease.
    /// This must happen before the child opens Runtime storage or serves control.
    ///
    /// # Errors
    /// Rejects contention, repeated or delayed claims, and invalid protected state.
    pub fn claim(
        &self,
        role: ProcessRole,
        generation: &str,
    ) -> Result<ProcessLease, OwnershipError> {
        let (lock, _) = self.lock(role, false)?;
        let mut record = self
            .read_record(role)?
            .ok_or(OwnershipError::StaleGeneration)?;
        if hex(&record[9..41]) != generation || record[109] != 0 {
            return Err(OwnershipError::StaleGeneration);
        }
        record[105..109].copy_from_slice(&std::process::id().to_le_bytes());
        record[109] = 1;
        self.publish_record(role, &record[..], PublicationMode::Replace)?;
        Ok(ProcessLease {
            _lock: lock,
            ownership: self.clone(),
            role,
            generation: generation.to_owned(),
            proof: None,
        })
    }

    /// Fences one abandoned generation before a coordinator permits replacement.
    /// This is explicit recovery, not automatic adoption or proof of clean stop.
    ///
    /// # Errors
    /// Rejects a held lifetime lease, a different generation, or uncertain storage.
    pub fn cancel_abandoned(
        &self,
        role: ProcessRole,
        generation: &str,
    ) -> Result<(), OwnershipError> {
        let (_lock, _) = self.lock(role, false)?;
        let mut record = self
            .read_record(role)?
            .ok_or(OwnershipError::StaleGeneration)?;
        if hex(&record[9..41]) != generation {
            return Err(OwnershipError::StaleGeneration);
        }
        record[109] = 4;
        self.publish_record(role, &record[..], PublicationMode::Replace)
    }

    fn record_path(&self, role: ProcessRole) -> PathBuf {
        self.state.join(format!("managed-{}.v1", role.name()))
    }

    fn read_record(&self, role: ProcessRole) -> Result<Option<Zeroizing<Vec<u8>>>, OwnershipError> {
        let path = self.record_path(role);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(OwnershipError::Unavailable),
            Ok(_) => {}
        }
        let record = Zeroizing::new(
            SecretSource::File(path)
                .read_bounded(RECORD_BYTES)
                .map_err(|_| OwnershipError::Invalid)?,
        );
        if record.len() != RECORD_BYTES
            || &record[..8] != MAGIC
            || record[8] != role.byte()
            || record[73..105] != self.installation
            || !matches!(record[109], 0..=4)
        {
            return Err(OwnershipError::Invalid);
        }
        record_endpoint(&record)?;
        Ok(Some(record))
    }

    pub(crate) fn proof_identity(
        &self,
        role: ProcessRole,
    ) -> Result<ProofIdentity, OwnershipError> {
        let record = self.read_record(role)?.ok_or(OwnershipError::Unavailable)?;
        if record[109] != 2 {
            return Err(OwnershipError::Unavailable);
        }
        let pid = u32::from_le_bytes(
            record[105..109]
                .try_into()
                .map_err(|_| OwnershipError::Invalid)?,
        );
        if pid == 0 {
            return Err(OwnershipError::Invalid);
        }
        Ok(ProofIdentity {
            installation: hex(&self.installation),
            role,
            generation: hex(&record[9..41]),
            pid,
            endpoint: record_endpoint(&record)?.ok_or(OwnershipError::Invalid)?,
            key: Zeroizing::new(
                record[41..73]
                    .try_into()
                    .map_err(|_| OwnershipError::Invalid)?,
            ),
        })
    }

    fn temporary_path(&self) -> Result<PathBuf, OwnershipError> {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| OwnershipError::Unavailable)?;
        Ok(self
            .state
            .join(format!(".managed-process-{}.tmp", hex(&nonce))))
    }

    fn publish_record(
        &self,
        role: ProcessRole,
        record: &[u8],
        mode: PublicationMode,
    ) -> Result<(), OwnershipError> {
        let temporary = self.temporary_path()?;
        let mut file = create_owner_only_file(&temporary).map_err(|_| OwnershipError::Invalid)?;
        let result = match file.write_all(record) {
            Ok(()) => publish(file, &temporary, &self.record_path(role), mode),
            Err(error) => {
                drop(file);
                Err(error)
            }
        };
        let _ = fs::remove_file(&temporary);
        result.map_err(|_| OwnershipError::PublicationUncertain)
    }

    fn lock(
        &self,
        role: ProcessRole,
        allow_create: bool,
    ) -> Result<(fs::File, bool), OwnershipError> {
        let path = self.state.join(format!("managed-{}.lock", role.name()));
        let mut created = false;
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !allow_create {
                    return Err(OwnershipError::StaleGeneration);
                }
                // A retained generation without its permanent lock is damaged
                // ownership, not a new installation that may be initialized.
                if self.read_record(role)?.is_some() {
                    return Err(OwnershipError::Invalid);
                }
                let temporary = self.temporary_path()?;
                let file =
                    create_owner_only_file(&temporary).map_err(|_| OwnershipError::Invalid)?;
                let result = publish(file, &temporary, &path, PublicationMode::CreateNew);
                let _ = fs::remove_file(&temporary);
                match result {
                    Ok(()) => created = true,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(OwnershipError::PublicationUncertain),
                }
            }
            Err(_) => return Err(OwnershipError::Unavailable),
        }
        // Do not contend with the fresh creator before it publishes the first
        // record. Otherwise a losing initializer can briefly take its lock and
        // leave every caller rejected with an orphaned empty installation.
        if !created && self.read_record(role)?.is_none() {
            return Err(OwnershipError::Invalid);
        }
        validate_owner_only_file(&path).map_err(|_| OwnershipError::Invalid)?;
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            options.share_mode(0x0000_0001 | 0x0000_0002);
        }
        let lock = options
            .open(&path)
            .map_err(|_| OwnershipError::Unavailable)?;
        validate_owner_only_file(&path).map_err(|_| OwnershipError::Invalid)?;
        lock.try_lock().map_err(|_| OwnershipError::Unavailable)?;
        Ok((lock, created))
    }
}

fn record_endpoint(record: &[u8]) -> Result<Option<SocketAddr>, OwnershipError> {
    let tail = &record[110..];
    let length = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(OwnershipError::Invalid)?;
    if tail[length..].iter().any(|byte| *byte != 0) {
        return Err(OwnershipError::Invalid);
    }
    if length == 0 {
        return if record[109] == 2 {
            Err(OwnershipError::Invalid)
        } else {
            Ok(None)
        };
    }
    let endpoint: SocketAddr = std::str::from_utf8(&tail[..length])
        .map_err(|_| OwnershipError::Invalid)?
        .parse()
        .map_err(|_| OwnershipError::Invalid)?;
    if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
        return Err(OwnershipError::Invalid);
    }
    Ok(Some(endpoint))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    result
}
