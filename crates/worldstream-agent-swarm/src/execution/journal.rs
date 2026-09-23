//! Owner-only durable execution journal.

use std::{
    fmt::Write as _,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_owner_only_file,
};

use super::supervisor::PersistedExecution;

const JOURNAL_FILE: &str = "execution.json";
const JOURNAL_LOCK: &str = ".execution.lock";
const JOURNAL_SCHEMA: &str = "worldstream/agent-swarm-execution-journal@1";
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;

/// Minimal durable port exercised by the execution state machine.
pub trait ExecutionJournal {
    /// Loads one exact retained state, or `None` for a new supervisor.
    ///
    /// # Errors
    /// Rejects unavailable, corrupt, or unsafe storage.
    fn load(&self) -> Result<Option<PersistedExecution>, JournalError>;

    /// Atomically replaces the retained state.
    ///
    /// # Errors
    /// Never reports success for an uncertain or partial publication.
    fn store(&self, state: &PersistedExecution) -> Result<(), JournalError>;
}

/// Deterministic crash/reopen journal used by tests.
#[derive(Clone, Debug, Default)]
pub struct MemoryExecutionJournal {
    state: Arc<Mutex<Option<PersistedExecution>>>,
}

impl MemoryExecutionJournal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ExecutionJournal for MemoryExecutionJournal {
    fn load(&self) -> Result<Option<PersistedExecution>, JournalError> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }

    fn store(&self, state: &PersistedExecution) -> Result<(), JournalError> {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = Some(state.clone());
        Ok(())
    }
}

/// Single-writer file journal. The retained process lock prevents two daemons
/// from admitting work from the same application state concurrently.
pub struct FileExecutionJournal {
    root: PathBuf,
    mutation: Mutex<()>,
    _process_lock: fs::File,
}

impl std::fmt::Debug for FileExecutionJournal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileExecutionJournal")
            .finish_non_exhaustive()
    }
}

impl FileExecutionJournal {
    /// Opens or creates one protected journal directory and acquires its
    /// exclusive daemon lease.
    ///
    /// # Errors
    /// Rejects unsafe paths, corrupt retained state, or another live writer.
    pub fn open(root: &Path) -> Result<Self, JournalError> {
        let root = prepare_data_directory(root).map_err(|_| JournalError::Unavailable)?;
        let lock_path = root.join(JOURNAL_LOCK);
        let process_lock = match create_owner_only_file(&lock_path) {
            Ok(file) => file,
            Err(_) if lock_path.exists() => fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|_| JournalError::Unavailable)?,
            Err(_) => return Err(JournalError::Unavailable),
        };
        validate_owner_only_file(&lock_path).map_err(|_| JournalError::UnsafeStorage)?;
        process_lock
            .try_lock()
            .map_err(|_| JournalError::WriterActive)?;
        let journal = Self {
            root,
            mutation: Mutex::new(()),
            _process_lock: process_lock,
        };
        let _ = journal.load()?;
        Ok(journal)
    }

    fn path(&self) -> PathBuf {
        self.root.join(JOURNAL_FILE)
    }
}

impl ExecutionJournal for FileExecutionJournal {
    fn load(&self) -> Result<Option<PersistedExecution>, JournalError> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.path();
        match fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(JournalError::Unavailable),
            Ok(metadata) if metadata.len() > MAX_JOURNAL_BYTES => {
                return Err(JournalError::InvalidData);
            }
            Ok(_) => {}
        }
        validate_owner_only_file(&path).map_err(|_| JournalError::UnsafeStorage)?;
        let bytes = fs::read(path).map_err(|_| JournalError::Unavailable)?;
        let envelope: JournalEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| JournalError::InvalidData)?;
        if envelope.schema != JOURNAL_SCHEMA || envelope.digest != digest(&envelope.state)? {
            return Err(JournalError::InvalidData);
        }
        Ok(Some(envelope.state))
    }

    fn store(&self, state: &PersistedExecution) -> Result<(), JournalError> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let envelope = JournalEnvelope {
            schema: JOURNAL_SCHEMA.to_owned(),
            digest: digest(state)?,
            state: state.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&envelope).map_err(|_| JournalError::InvalidData)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_JOURNAL_BYTES {
            return Err(JournalError::InvalidData);
        }
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|_| JournalError::Unavailable)?;
        let suffix = random.iter().fold(String::new(), |mut suffix, byte| {
            let _ = write!(suffix, "{byte:02x}");
            suffix
        });
        let staged = self.root.join(format!(".{suffix}.execution.tmp"));
        let mut file =
            create_owner_only_renameable_file(&staged).map_err(|_| JournalError::Unavailable)?;
        let result = (|| {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| JournalError::Unavailable)?;
            drop(file);
            let target = self.path();
            if target.exists() {
                atomicwrites::replace_atomic(&staged, &target)
            } else {
                atomicwrites::move_atomic(&staged, &target)
            }
            .map_err(|_| JournalError::Unavailable)
        })();
        let _ = fs::remove_file(staged);
        result
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalEnvelope {
    schema: String,
    state: PersistedExecution,
    digest: String,
}

fn digest(state: &PersistedExecution) -> Result<String, JournalError> {
    let bytes = serde_json::to_vec(state).map_err(|_| JournalError::InvalidData)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

/// Closed journal failures.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum JournalError {
    #[error("execution journal is unavailable")]
    Unavailable,
    #[error("execution journal storage is unsafe")]
    UnsafeStorage,
    #[error("another execution daemon owns the journal")]
    WriterActive,
    #[error("execution journal data is invalid")]
    InvalidData,
}
