use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};
use worldstream_protocol::UlidString;
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_owner_only_file,
};

use crate::managed_agent_host::{
    ManagedAgentActivationDispositionV1, ManagedAgentActivationStateV1,
    ManagedAgentActivationStatusV1,
};

const SCHEMA: &str = "worldstream/managed-agent-activation-status/v1";
const MAX_RECORD_BYTES: u64 = 4 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ManagedActivationStatusErrorV1 {
    Invalid,
    Unavailable,
}

#[derive(Clone)]
pub struct ManagedActivationStatusStoreV1 {
    root: Arc<PathBuf>,
    mutation: Arc<Mutex<()>>,
    process_lock: Arc<fs::File>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedActivationStatusRecordV1 {
    schema: String,
    assignment_id: String,
    activation: ManagedAgentActivationStatusV1,
    pending_confirmation: Option<PendingConfirmationV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingConfirmationV1 {
    activation_id: UlidString,
    disposition: ManagedAgentActivationDispositionV1,
}

impl PendingConfirmationV1 {
    #[must_use]
    pub fn activation_id(&self) -> &str {
        self.activation_id.as_str()
    }

    #[must_use]
    pub const fn disposition(&self) -> ManagedAgentActivationDispositionV1 {
        self.disposition
    }
}

struct ProcessFileLockV1<'a>(&'a fs::File);

impl Drop for ProcessFileLockV1<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl ManagedActivationStatusStoreV1 {
    /// Opens the durable activation-status store.
    ///
    /// # Errors
    ///
    /// Returns an error when the storage directory or process lock cannot be
    /// securely created or validated.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ManagedActivationStatusErrorV1> {
        let root = prepare_data_directory(root.as_ref())
            .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
        let lock_path = root.join(".status.lock");
        let process_lock = match create_owner_only_file(&lock_path) {
            Ok(file) => file,
            Err(_) if lock_path.exists() => fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?,
            Err(_) => return Err(ManagedActivationStatusErrorV1::Unavailable),
        };
        validate_owner_only_file(&lock_path)
            .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
        Ok(Self {
            root: Arc::new(root),
            mutation: Arc::new(Mutex::new(())),
            process_lock: Arc::new(process_lock),
        })
    }

    /// Returns the current durable status for an assignment.
    ///
    /// # Errors
    ///
    /// Returns an error when the assignment identifier or retained record is
    /// invalid, or when the store cannot be read.
    pub fn status(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentActivationStatusV1, ManagedActivationStatusErrorV1> {
        validate_assignment_id(assignment_id)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let path = self.path(assignment_id);
        if !path.exists() {
            return Ok(ManagedAgentActivationStatusV1::default());
        }
        let record = read_record(&path)?;
        if record.assignment_id != assignment_id || record.schema != SCHEMA {
            return Err(ManagedActivationStatusErrorV1::Invalid);
        }
        Ok(record.activation)
    }

    /// Records the host activation state while retaining any confirmed disposition.
    ///
    /// # Errors
    ///
    /// Returns an error when the assignment identifier is invalid or durable
    /// state cannot be read or written.
    pub fn note_state(
        &self,
        assignment_id: &str,
        state: ManagedAgentActivationStateV1,
    ) -> Result<(), ManagedActivationStatusErrorV1> {
        self.persist(assignment_id, state, None)
    }

    /// Records a receipt-confirmed managed action disposition.
    ///
    /// # Errors
    ///
    /// Returns an error when the pending activation does not match or durable
    /// state cannot be read or written.
    pub fn note_confirmed(
        &self,
        assignment_id: &str,
        activation_id: &str,
        disposition: ManagedAgentActivationDispositionV1,
    ) -> Result<(), ManagedActivationStatusErrorV1> {
        validate_assignment_id(assignment_id)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let path = self.path(assignment_id);
        let prior = self.record_unlocked(assignment_id)?;
        let pending = prior.pending_confirmation.as_ref();
        if pending.map(PendingConfirmationV1::activation_id) != Some(activation_id)
            || pending.map(PendingConfirmationV1::disposition) != Some(disposition)
        {
            return Err(ManagedActivationStatusErrorV1::Invalid);
        }
        persist_record(
            &self.root,
            &path,
            &ManagedActivationStatusRecordV1 {
                schema: SCHEMA.to_owned(),
                assignment_id: assignment_id.to_owned(),
                activation: ManagedAgentActivationStatusV1 {
                    state: ManagedAgentActivationStateV1::Unavailable,
                    last_confirmed_disposition: Some(disposition),
                },
                pending_confirmation: None,
            },
        )
    }

    /// Marks an activation as awaiting a receipt-confirmed disposition.
    ///
    /// # Errors
    ///
    /// Returns an error when an identifier is invalid, a different completion
    /// is already pending, or durable state cannot be read or written.
    pub fn note_completion_pending(
        &self,
        assignment_id: &str,
        activation_id: &str,
        disposition: ManagedAgentActivationDispositionV1,
    ) -> Result<(), ManagedActivationStatusErrorV1> {
        let activation_id = activation_id
            .parse::<UlidString>()
            .map_err(|_| ManagedActivationStatusErrorV1::Invalid)?;
        validate_assignment_id(assignment_id)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let path = self.path(assignment_id);
        let prior = if path.exists() {
            self.record_unlocked(assignment_id)?
        } else {
            ManagedActivationStatusRecordV1 {
                schema: SCHEMA.to_owned(),
                assignment_id: assignment_id.to_owned(),
                activation: ManagedAgentActivationStatusV1::default(),
                pending_confirmation: None,
            }
        };
        let pending = PendingConfirmationV1 {
            activation_id,
            disposition,
        };
        match prior.pending_confirmation.as_ref() {
            Some(retained) if retained == &pending => return Ok(()),
            Some(_) => return Err(ManagedActivationStatusErrorV1::Invalid),
            None => {}
        }
        persist_record(
            &self.root,
            &path,
            &ManagedActivationStatusRecordV1 {
                schema: SCHEMA.to_owned(),
                assignment_id: assignment_id.to_owned(),
                activation: ManagedAgentActivationStatusV1 {
                    state: ManagedAgentActivationStateV1::Unavailable,
                    last_confirmed_disposition: prior.activation.last_confirmed_disposition,
                },
                pending_confirmation: Some(pending),
            },
        )
    }

    /// Returns a pending receipt confirmation, if any.
    ///
    /// # Errors
    ///
    /// Returns an error when the assignment identifier or retained record is
    /// invalid, or when the store cannot be read.
    pub fn pending_confirmation(
        &self,
        assignment_id: &str,
    ) -> Result<Option<PendingConfirmationV1>, ManagedActivationStatusErrorV1> {
        validate_assignment_id(assignment_id)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let path = self.path(assignment_id);
        if !path.exists() {
            return Ok(None);
        }
        let record = read_record(&path)?;
        if record.assignment_id != assignment_id || record.schema != SCHEMA {
            return Err(ManagedActivationStatusErrorV1::Invalid);
        }
        Ok(record.pending_confirmation)
    }

    fn persist(
        &self,
        assignment_id: &str,
        state: ManagedAgentActivationStateV1,
        disposition: Option<ManagedAgentActivationDispositionV1>,
    ) -> Result<(), ManagedActivationStatusErrorV1> {
        validate_assignment_id(assignment_id)?;
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let path = self.path(assignment_id);
        let prior = if path.exists() {
            self.record_unlocked(assignment_id)?
        } else {
            ManagedActivationStatusRecordV1 {
                schema: SCHEMA.to_owned(),
                assignment_id: assignment_id.to_owned(),
                activation: ManagedAgentActivationStatusV1::default(),
                pending_confirmation: None,
            }
        };
        let record = ManagedActivationStatusRecordV1 {
            schema: SCHEMA.to_owned(),
            assignment_id: assignment_id.to_owned(),
            activation: ManagedAgentActivationStatusV1 {
                state,
                last_confirmed_disposition: disposition
                    .or(prior.activation.last_confirmed_disposition),
            },
            pending_confirmation: prior.pending_confirmation,
        };
        persist_record(&self.root, &path, &record)
    }

    fn record_unlocked(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedActivationStatusRecordV1, ManagedActivationStatusErrorV1> {
        let record = read_record(&self.path(assignment_id))?;
        if record.assignment_id != assignment_id || record.schema != SCHEMA {
            return Err(ManagedActivationStatusErrorV1::Invalid);
        }
        Ok(record)
    }

    fn path(&self, assignment_id: &str) -> PathBuf {
        self.root.join(format!("{assignment_id}.json"))
    }

    fn lock_process(&self) -> Result<ProcessFileLockV1<'_>, ManagedActivationStatusErrorV1> {
        self.process_lock
            .lock()
            .map(|()| ProcessFileLockV1(&self.process_lock))
            .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)
    }
}

fn validate_assignment_id(value: &str) -> Result<(), ManagedActivationStatusErrorV1> {
    value
        .parse::<UlidString>()
        .map(|_| ())
        .map_err(|_| ManagedActivationStatusErrorV1::Invalid)
}

fn read_record(
    path: &Path,
) -> Result<ManagedActivationStatusRecordV1, ManagedActivationStatusErrorV1> {
    validate_owner_only_file(path).map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
    if fs::metadata(path)
        .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?
        .len()
        > MAX_RECORD_BYTES
    {
        return Err(ManagedActivationStatusErrorV1::Invalid);
    }
    serde_json::from_slice(
        &fs::read(path).map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?,
    )
    .map_err(|_| ManagedActivationStatusErrorV1::Invalid)
}

fn persist_record(
    root: &Path,
    target: &Path,
    record: &ManagedActivationStatusRecordV1,
) -> Result<(), ManagedActivationStatusErrorV1> {
    let bytes = serde_json::to_vec(record).map_err(|_| ManagedActivationStatusErrorV1::Invalid)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECORD_BYTES {
        return Err(ManagedActivationStatusErrorV1::Invalid);
    }
    let temporary = root.join(format!(
        ".{}.tmp",
        target
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(ManagedActivationStatusErrorV1::Invalid)?
    ));
    let _ = fs::remove_file(&temporary);
    let mut file = create_owner_only_renameable_file(&temporary)
        .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
    drop(file);
    replace_file(&temporary, target).map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
    let directory =
        fs::File::open(root).map_err(|_| ManagedActivationStatusErrorV1::Unavailable)?;
    directory
        .sync_all()
        .map_err(|_| ManagedActivationStatusErrorV1::Unavailable)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    use winsafe::{MOVEFILE, prelude::kernel_Hpath};

    source.move_file(target, MOVEFILE::REPLACE_EXISTING)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn retained_confirmation_survives_a_later_leased_state() {
        let root = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary status root: {error}"));
        let status_root = root.path().join("status");
        let store = ManagedActivationStatusStoreV1::open(&status_root)
            .unwrap_or_else(|error| unreachable!("status store: {error:?}"));
        store
            .note_completion_pending(
                ASSIGNMENT,
                ASSIGNMENT,
                ManagedAgentActivationDispositionV1::Handled,
            )
            .unwrap_or_else(|error| unreachable!("pending confirmation: {error:?}"));
        store
            .note_confirmed(
                ASSIGNMENT,
                ASSIGNMENT,
                ManagedAgentActivationDispositionV1::Handled,
            )
            .unwrap_or_else(|error| unreachable!("confirmation: {error:?}"));
        store
            .note_state(ASSIGNMENT, ManagedAgentActivationStateV1::Leased)
            .unwrap_or_else(|error| unreachable!("lease: {error:?}"));

        let reopened = ManagedActivationStatusStoreV1::open(&status_root)
            .unwrap_or_else(|error| unreachable!("reopened status: {error:?}"));
        assert_eq!(
            reopened
                .status(ASSIGNMENT)
                .unwrap_or_else(|error| unreachable!("retained status: {error:?}")),
            ManagedAgentActivationStatusV1 {
                state: ManagedAgentActivationStateV1::Leased,
                last_confirmed_disposition: Some(ManagedAgentActivationDispositionV1::Handled),
            }
        );
    }

    #[test]
    fn absent_status_is_unavailable_without_a_disposition() {
        let root = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary status root: {error}"));
        let store = ManagedActivationStatusStoreV1::open(root.path().join("status"))
            .unwrap_or_else(|error| unreachable!("status store: {error:?}"));
        assert_eq!(
            store
                .status(ASSIGNMENT)
                .unwrap_or_else(|error| unreachable!("default status: {error:?}")),
            ManagedAgentActivationStatusV1::default()
        );
    }

    #[test]
    fn pending_confirmation_rejects_a_different_completion_without_erasing_the_first() {
        const OTHER_ACTIVATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

        let root = tempfile::tempdir()
            .unwrap_or_else(|error| unreachable!("temporary status root: {error}"));
        let store = ManagedActivationStatusStoreV1::open(root.path().join("status"))
            .unwrap_or_else(|error| unreachable!("status store: {error:?}"));
        store
            .note_completion_pending(
                ASSIGNMENT,
                ASSIGNMENT,
                ManagedAgentActivationDispositionV1::Handled,
            )
            .unwrap_or_else(|error| unreachable!("first pending completion: {error:?}"));
        store
            .note_completion_pending(
                ASSIGNMENT,
                ASSIGNMENT,
                ManagedAgentActivationDispositionV1::Handled,
            )
            .unwrap_or_else(|error| unreachable!("idempotent pending completion: {error:?}"));
        assert_eq!(
            store.note_completion_pending(
                ASSIGNMENT,
                OTHER_ACTIVATION,
                ManagedAgentActivationDispositionV1::Handled,
            ),
            Err(ManagedActivationStatusErrorV1::Invalid)
        );
        assert_eq!(
            store
                .pending_confirmation(ASSIGNMENT)
                .unwrap_or_else(|error| unreachable!("retained pending completion: {error:?}"))
                .map(|pending| (pending.activation_id().to_owned(), pending.disposition())),
            Some((
                ASSIGNMENT.to_owned(),
                ManagedAgentActivationDispositionV1::Handled,
            ))
        );
    }
}
