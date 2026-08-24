//! Owner-only durable private Activation lifecycle state.
//!
//! Private Invocation Context is stored only here. Completion intent and bounded safe terminal
//! results are additionally fenced through the shared assignment MCP operation ledger.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use worldstream_core::CanonicalJsonV1;
use worldstream_protocol::UlidString;
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_owner_only_file,
};

use crate::assignment_mcp_activations::{
    ActivationCompletionReceiptV1, ActivationLedgerErrorV1, ActivationLedgerSnapshotV1,
    ActivationOperationLedgerV1, ActivationTerminalOutcomeV1, AssignedActivationLeaseV1,
    PreparedActivationAbandonV1, PreparedActivationAcquireV1, PreparedActivationCompletionV1,
};
use crate::assignment_mcp_operations::{
    AssignmentMcpOperationIdentityV1, AssignmentMcpOperationIntentV1,
    AssignmentMcpOperationLedgerV1, AssignmentMcpOperationPhaseV1, AssignmentMcpRemoteAcceptanceV1,
    AssignmentMcpRemoteOutcomeV1, FileAssignmentMcpOperationLedgerV1,
};

const PRIVATE_SCHEMA_V1: &str = "worldstream/assignment-activation-private-ledger@1";
const MAX_PRIVATE_RECORD_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ASSIGNMENTS: usize = 256;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PrivateActivationRecordV1 {
    schema: String,
    assignment_id: String,
    snapshot: ActivationLedgerSnapshotV1,
    integrity_hash: String,
}

#[derive(Serialize)]
struct PrivateIntegrityWitnessV1<'a> {
    schema: &'a str,
    assignment_id: &'a str,
    snapshot: &'a ActivationLedgerSnapshotV1,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome", deny_unknown_fields)]
enum RetainedCompletionResultV1 {
    Completed {
        receipt: ActivationCompletionReceiptV1,
    },
    Terminal {
        activation_cursor: u64,
        activation_id: String,
        claim_id: String,
        lease_generation: u64,
        terminal_outcome: ActivationTerminalOutcomeV1,
    },
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RetainedCompletionResponseV1 {
    activation_id: String,
    completion_id: String,
    request_id: String,
    result: RetainedCompletionResultV1,
}

/// Durable owner-only implementation of the assigned Activation lifecycle ledger.
#[derive(Clone)]
pub struct FileActivationOperationLedgerV1 {
    private_root: Arc<PathBuf>,
    operations: FileAssignmentMcpOperationLedgerV1,
    mutation: Arc<Mutex<()>>,
    process_lock: Arc<fs::File>,
}

struct ProcessFileLockV1<'a>(&'a fs::File);

impl Drop for ProcessFileLockV1<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl FileActivationOperationLedgerV1 {
    /// Opens or creates bounded owner-only private and safe-operation stores.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable or invalid-data error for unsafe or corrupt state.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ActivationLedgerErrorV1> {
        let root = prepare_data_directory(root.as_ref()).map_err(|_| unavailable())?;
        let private_root =
            prepare_data_directory(&root.join("private")).map_err(|_| unavailable())?;
        let operations = FileAssignmentMcpOperationLedgerV1::open(root.join("operations"))
            .map_err(map_operation_error)?;
        let lock_path = private_root.join(".ledger.lock");
        let process_lock = match create_owner_only_file(&lock_path) {
            Ok(file) => file,
            Err(_) if lock_path.exists() => fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|_| unavailable())?,
            Err(_) => return Err(unavailable()),
        };
        validate_owner_only_file(&lock_path).map_err(|_| unavailable())?;
        let ledger = Self {
            private_root: Arc::new(private_root),
            operations,
            mutation: Arc::new(Mutex::new(())),
            process_lock: Arc::new(process_lock),
        };
        let process_guard = ledger.lock_process()?;
        ledger.validate_existing()?;
        drop(process_guard);
        Ok(ledger)
    }

    fn lock_process(&self) -> Result<ProcessFileLockV1<'_>, ActivationLedgerErrorV1> {
        self.process_lock.lock().map_err(|_| unavailable())?;
        Ok(ProcessFileLockV1(&self.process_lock))
    }

    fn path(&self, assignment_id: &str) -> PathBuf {
        self.private_root
            .join(format!("{}.json", encode_bytes(assignment_id.as_bytes())))
    }

    fn validate_existing(&self) -> Result<(), ActivationLedgerErrorV1> {
        let mut count = 0_usize;
        for entry in fs::read_dir(self.private_root.as_ref()).map_err(|_| unavailable())? {
            let entry = entry.map_err(|_| unavailable())?;
            let file_type = entry.file_type().map_err(|_| unavailable())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".ledger.lock" && file_type.is_file() {
                validate_owner_only_file(&entry.path()).map_err(|_| unavailable())?;
                continue;
            }
            if name.starts_with('.') && name.ends_with(".tmp") && file_type.is_file() {
                validate_owner_only_file(&entry.path()).map_err(|_| unavailable())?;
                fs::remove_file(entry.path()).map_err(|_| unavailable())?;
                continue;
            }
            if !file_type.is_file() || !name.ends_with(".json") {
                return Err(invalid());
            }
            count = count.saturating_add(1);
            if count > MAX_ASSIGNMENTS {
                return Err(unavailable());
            }
            let record = read_private_record(&entry.path())?;
            if self.path(&record.assignment_id) != entry.path() {
                return Err(invalid());
            }
        }
        sync_directory(self.private_root.as_ref()).map_err(|_| unavailable())
    }

    fn load_raw(
        &self,
        assignment_id: &str,
    ) -> Result<ActivationLedgerSnapshotV1, ActivationLedgerErrorV1> {
        validate_assignment_id(assignment_id)?;
        let path = self.path(assignment_id);
        if !path.exists() {
            return Ok(ActivationLedgerSnapshotV1::Idle { last_cursor: 0 });
        }
        let record = read_private_record(&path)?;
        if record.assignment_id != assignment_id {
            return Err(invalid());
        }
        Ok(record.snapshot)
    }

    fn persist(
        &self,
        assignment_id: &str,
        snapshot: &ActivationLedgerSnapshotV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        validate_snapshot(assignment_id, snapshot)?;
        if !self.path(assignment_id).exists() && self.record_count()? >= MAX_ASSIGNMENTS {
            return Err(unavailable());
        }
        let mut record = PrivateActivationRecordV1 {
            schema: PRIVATE_SCHEMA_V1.to_owned(),
            assignment_id: assignment_id.to_owned(),
            snapshot: snapshot.clone(),
            integrity_hash: String::new(),
        };
        record.integrity_hash = private_integrity_hash(&record)?;
        let bytes = serde_json::to_vec(&record).map_err(|_| invalid())?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PRIVATE_RECORD_BYTES {
            return Err(invalid());
        }
        let temporary = self.private_root.join(format!(
            ".{}-{}.tmp",
            encode_bytes(assignment_id.as_bytes()),
            random_hex()?
        ));
        let target = self.path(assignment_id);
        let mut file = create_owner_only_renameable_file(&temporary).map_err(|_| unavailable())?;
        let result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_file(&temporary, &target))
            .and_then(|()| sync_directory(self.private_root.as_ref()))
            .map_err(|_| unavailable());
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn record_count(&self) -> Result<usize, ActivationLedgerErrorV1> {
        fs::read_dir(self.private_root.as_ref())
            .map_err(|_| unavailable())?
            .try_fold(0_usize, |count, entry| {
                let entry = entry.map_err(|_| unavailable())?;
                Ok(count
                    + usize::from(
                        entry
                            .path()
                            .extension()
                            .is_some_and(|value| value == "json"),
                    ))
            })
    }

    fn reconcile_completion(
        &self,
        lease: &AssignedActivationLeaseV1,
        prepared: &PreparedActivationCompletionV1,
    ) -> Result<ActivationLedgerSnapshotV1, ActivationLedgerErrorV1> {
        let (identity, intent) = completion_intent(lease, prepared)?;
        let record = self
            .operations
            .reserve(&intent)
            .map_err(map_operation_error)?;
        let Some(acceptance) = record.remote_acceptance() else {
            return Ok(ActivationLedgerSnapshotV1::Completing {
                lease: Box::new(lease.clone()),
                prepared: prepared.clone(),
            });
        };
        let response = decode_completion_response(lease, prepared, acceptance)?;
        if record.phase() != AssignmentMcpOperationPhaseV1::Complete {
            self.operations
                .complete(&identity, acceptance)
                .map_err(map_operation_error)?;
        }
        Ok(match response.result {
            RetainedCompletionResultV1::Completed { receipt } => {
                ActivationLedgerSnapshotV1::Completed(receipt)
            }
            RetainedCompletionResultV1::Terminal {
                activation_cursor,
                activation_id,
                claim_id,
                lease_generation,
                terminal_outcome,
            } => ActivationLedgerSnapshotV1::Terminal {
                activation_cursor,
                activation_id,
                claim_id,
                lease_generation,
                outcome: terminal_outcome,
            },
        })
    }

    fn load_and_reconcile(
        &self,
        assignment_id: &str,
    ) -> Result<ActivationLedgerSnapshotV1, ActivationLedgerErrorV1> {
        let snapshot = self.load_raw(assignment_id)?;
        let ActivationLedgerSnapshotV1::Completing { lease, prepared } = snapshot else {
            return Ok(snapshot);
        };
        let reconciled = self.reconcile_completion(&lease, &prepared)?;
        if !matches!(reconciled, ActivationLedgerSnapshotV1::Completing { .. }) {
            self.persist(assignment_id, &reconciled)?;
        }
        Ok(reconciled)
    }
}

impl ActivationOperationLedgerV1 for FileActivationOperationLedgerV1 {
    fn load(
        &self,
        assignment_id: &str,
    ) -> Result<ActivationLedgerSnapshotV1, ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        self.load_and_reconcile(assignment_id)
    }

    fn begin_acquire(
        &self,
        assignment_id: &str,
        activation_cursor: u64,
    ) -> Result<PreparedActivationAcquireV1, ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let snapshot = self.load_and_reconcile(assignment_id)?;
        if let ActivationLedgerSnapshotV1::Acquiring(prepared) = &snapshot {
            return if prepared.activation_cursor() == activation_cursor {
                Ok(prepared.clone())
            } else {
                Err(conflict())
            };
        }
        let expected = next_cursor(&snapshot)?;
        if activation_cursor != expected {
            return Err(conflict());
        }
        let prepared = PreparedActivationAcquireV1::new(
            assignment_id,
            activation_cursor,
            &next_ulid()?,
            &next_ulid()?,
        )?;
        self.persist(
            assignment_id,
            &ActivationLedgerSnapshotV1::Acquiring(prepared.clone()),
        )?;
        Ok(prepared)
    }

    fn select_offer(
        &self,
        prepared: &PreparedActivationAcquireV1,
        offer: &worldstream_protocol::ActivationOffer,
    ) -> Result<PreparedActivationAcquireV1, ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let current = self.load_raw(prepared.assignment_id())?;
        let ActivationLedgerSnapshotV1::Acquiring(current) = current else {
            return Err(conflict());
        };
        if current != *prepared {
            return Err(conflict());
        }
        let selected = current.with_selected_offer(offer.clone())?;
        self.persist(
            prepared.assignment_id(),
            &ActivationLedgerSnapshotV1::Acquiring(selected.clone()),
        )?;
        Ok(selected)
    }

    fn retain_lease(
        &self,
        prepared: &PreparedActivationAcquireV1,
        lease: &AssignedActivationLeaseV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let current = self.load_raw(prepared.assignment_id())?;
        if current == ActivationLedgerSnapshotV1::Leased(Box::new(lease.clone())) {
            return Ok(());
        }
        let ActivationLedgerSnapshotV1::Acquiring(current) = current else {
            return Err(conflict());
        };
        let selected = current.selected_offer().ok_or_else(conflict)?;
        if current != *prepared
            || lease.assignment_id() != prepared.assignment_id()
            || lease.activation_cursor != prepared.activation_cursor()
            || lease.activation_id() != selected.activation_id
            || lease.claim_id() != prepared.claim_id()
        {
            return Err(conflict());
        }
        self.persist(
            prepared.assignment_id(),
            &ActivationLedgerSnapshotV1::Leased(Box::new(lease.clone())),
        )
    }

    fn retain_no_offer(
        &self,
        prepared: &PreparedActivationAcquireV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let current = self.load_raw(prepared.assignment_id())?;
        let idle = ActivationLedgerSnapshotV1::Idle {
            last_cursor: prepared.activation_cursor(),
        };
        if current == idle {
            return Ok(());
        }
        if current != ActivationLedgerSnapshotV1::Acquiring(prepared.clone()) {
            return Err(conflict());
        }
        self.persist(prepared.assignment_id(), &idle)
    }

    fn begin_completion(
        &self,
        lease: &AssignedActivationLeaseV1,
        canonical_request_hash: &str,
        disposition: &str,
    ) -> Result<PreparedActivationCompletionV1, ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let current = self.load_and_reconcile(lease.assignment_id())?;
        if let ActivationLedgerSnapshotV1::Completing {
            lease: current_lease,
            prepared,
        } = current
        {
            return if *current_lease == *lease
                && prepared.canonical_request_hash() == canonical_request_hash
                && prepared.disposition() == disposition
            {
                Ok(prepared)
            } else {
                Err(conflict())
            };
        }
        if current != ActivationLedgerSnapshotV1::Leased(Box::new(lease.clone())) {
            return Err(conflict());
        }
        let prepared = PreparedActivationCompletionV1::new(
            lease,
            &next_ulid()?,
            &next_ulid()?,
            &next_ulid()?,
            canonical_request_hash,
            disposition,
        )?;
        let completing = ActivationLedgerSnapshotV1::Completing {
            lease: Box::new(lease.clone()),
            prepared: prepared.clone(),
        };
        self.persist(lease.assignment_id(), &completing)?;
        let reconciled = self.reconcile_completion(lease, &prepared)?;
        if reconciled != completing {
            self.persist(lease.assignment_id(), &reconciled)?;
        }
        Ok(prepared)
    }

    fn retain_completion(
        &self,
        prepared: &PreparedActivationCompletionV1,
        receipt: &ActivationCompletionReceiptV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let current = self.load_and_reconcile(prepared.assignment_id())?;
        if current == ActivationLedgerSnapshotV1::Completed(receipt.clone()) {
            return Ok(());
        }
        let ActivationLedgerSnapshotV1::Completing {
            lease,
            prepared: current_prepared,
        } = current
        else {
            return Err(conflict());
        };
        validate_receipt(&lease, &current_prepared, prepared, receipt)?;
        let response = RetainedCompletionResponseV1 {
            activation_id: lease.activation_id().to_owned(),
            completion_id: prepared.completion_id().to_owned(),
            request_id: prepared.remote_request_id().to_owned(),
            result: RetainedCompletionResultV1::Completed {
                receipt: receipt.clone(),
            },
        };
        self.retain_shared_response(
            &lease,
            prepared,
            &response,
            AssignmentMcpRemoteOutcomeV1::Accepted,
        )?;
        self.persist(
            lease.assignment_id(),
            &ActivationLedgerSnapshotV1::Completed(receipt.clone()),
        )
    }

    fn retain_terminal(
        &self,
        lease: &AssignedActivationLeaseV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let current = self.load_and_reconcile(lease.assignment_id())?;
        let terminal = ActivationLedgerSnapshotV1::Terminal {
            activation_cursor: lease.activation_cursor,
            activation_id: lease.activation_id().to_owned(),
            claim_id: lease.claim_id().to_owned(),
            lease_generation: lease.lease_generation(),
            outcome,
        };
        if current == terminal {
            return Ok(());
        }
        match current {
            ActivationLedgerSnapshotV1::Leased(current_lease) if *current_lease == *lease => {}
            ActivationLedgerSnapshotV1::Completing {
                lease: current_lease,
                prepared,
            } if *current_lease == *lease => {
                let response = RetainedCompletionResponseV1 {
                    activation_id: lease.activation_id().to_owned(),
                    completion_id: prepared.completion_id().to_owned(),
                    request_id: prepared.remote_request_id().to_owned(),
                    result: RetainedCompletionResultV1::Terminal {
                        activation_cursor: lease.activation_cursor,
                        activation_id: lease.activation_id().to_owned(),
                        claim_id: lease.claim_id().to_owned(),
                        lease_generation: lease.lease_generation(),
                        terminal_outcome: outcome,
                    },
                };
                self.retain_shared_response(
                    lease,
                    &prepared,
                    &response,
                    AssignmentMcpRemoteOutcomeV1::Rejected,
                )?;
            }
            _ => return Err(conflict()),
        }
        self.persist(lease.assignment_id(), &terminal)
    }

    fn retain_abandoned(
        &self,
        prepared: &PreparedActivationAcquireV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        let _guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        let _process_guard = self.lock_process()?;
        let abandoned = PreparedActivationAbandonV1::new(prepared, outcome)?;
        let snapshot = ActivationLedgerSnapshotV1::Abandoned(abandoned);
        let current = self.load_raw(prepared.assignment_id())?;
        if current == snapshot {
            return Ok(());
        }
        if current != ActivationLedgerSnapshotV1::Acquiring(prepared.clone()) {
            return Err(conflict());
        }
        self.persist(prepared.assignment_id(), &snapshot)
    }
}

impl FileActivationOperationLedgerV1 {
    fn retain_shared_response(
        &self,
        lease: &AssignedActivationLeaseV1,
        prepared: &PreparedActivationCompletionV1,
        response: &RetainedCompletionResponseV1,
        outcome: AssignmentMcpRemoteOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        let (identity, intent) = completion_intent(lease, prepared)?;
        self.operations
            .reserve(&intent)
            .map_err(map_operation_error)?;
        let acceptance = AssignmentMcpRemoteAcceptanceV1::new_activation_completion(
            prepared.remote_request_id(),
            lease.activation_id(),
            prepared.completion_id(),
            outcome,
            canonical_bytes(response)?,
        )
        .map_err(map_operation_error)?;
        self.operations
            .record_remote(&identity, &acceptance)
            .map_err(map_operation_error)?;
        self.operations
            .complete(&identity, &acceptance)
            .map_err(map_operation_error)?;
        Ok(())
    }
}

fn completion_intent(
    lease: &AssignedActivationLeaseV1,
    prepared: &PreparedActivationCompletionV1,
) -> Result<
    (
        AssignmentMcpOperationIdentityV1,
        AssignmentMcpOperationIntentV1,
    ),
    ActivationLedgerErrorV1,
> {
    if !prepared.matches_lease(lease) {
        return Err(conflict());
    }
    let identity =
        AssignmentMcpOperationIdentityV1::new(lease.assignment_id(), prepared.operation_id())
            .map_err(map_operation_error)?;
    let request = json!({
        "activation_id": lease.activation_id(),
        "assignment_id": lease.assignment_id(),
        "claim_id": lease.claim_id(),
        "completion_id": prepared.completion_id(),
        "context_hash": lease.context_hash(),
        "cursor": lease.activation_cursor,
        "disposition": prepared.disposition(),
        "lease_generation": lease.lease_generation(),
        "operation_id": prepared.operation_id(),
        "request_hash": prepared.canonical_request_hash(),
        "request_id": prepared.remote_request_id(),
    });
    let intent = AssignmentMcpOperationIntentV1::new_activation_completion(
        identity.clone(),
        lease.activation_id(),
        lease.claim_id(),
        lease.lease_generation(),
        lease.activation_cursor,
        prepared.completion_id(),
        prepared.remote_request_id(),
        canonical_bytes(&request)?,
    )
    .map_err(map_operation_error)?;
    Ok((identity, intent))
}

fn decode_completion_response(
    lease: &AssignedActivationLeaseV1,
    prepared: &PreparedActivationCompletionV1,
    acceptance: &AssignmentMcpRemoteAcceptanceV1,
) -> Result<RetainedCompletionResponseV1, ActivationLedgerErrorV1> {
    let response: RetainedCompletionResponseV1 =
        serde_json::from_slice(acceptance.canonical_response()).map_err(|_| invalid())?;
    if response.activation_id != lease.activation_id()
        || response.completion_id != prepared.completion_id()
        || response.request_id != prepared.remote_request_id()
    {
        return Err(invalid());
    }
    match (&response.result, acceptance.outcome()) {
        (
            RetainedCompletionResultV1::Completed { receipt },
            AssignmentMcpRemoteOutcomeV1::Accepted,
        ) => validate_receipt(lease, prepared, prepared, receipt)?,
        (
            RetainedCompletionResultV1::Terminal {
                activation_cursor,
                activation_id,
                claim_id,
                lease_generation,
                ..
            },
            AssignmentMcpRemoteOutcomeV1::Rejected,
        ) if *activation_cursor == lease.activation_cursor
            && activation_id == lease.activation_id()
            && claim_id == lease.claim_id()
            && *lease_generation == lease.lease_generation() => {}
        _ => return Err(invalid()),
    }
    Ok(response)
}

fn validate_receipt(
    lease: &AssignedActivationLeaseV1,
    current: &PreparedActivationCompletionV1,
    supplied: &PreparedActivationCompletionV1,
    receipt: &ActivationCompletionReceiptV1,
) -> Result<(), ActivationLedgerErrorV1> {
    if current != supplied
        || !current.matches_lease(lease)
        || receipt.activation_cursor != lease.activation_cursor
        || receipt.activation_id != lease.activation_id()
        || receipt.lease_generation != lease.lease_generation()
        || receipt.operation_id != current.operation_id()
        || receipt.completion_id != current.completion_id()
    {
        return Err(conflict());
    }
    Ok(())
}

fn next_cursor(snapshot: &ActivationLedgerSnapshotV1) -> Result<u64, ActivationLedgerErrorV1> {
    let last = match snapshot {
        ActivationLedgerSnapshotV1::Idle { last_cursor } => *last_cursor,
        ActivationLedgerSnapshotV1::Completed(receipt) => receipt.activation_cursor,
        ActivationLedgerSnapshotV1::Terminal {
            activation_cursor, ..
        } => *activation_cursor,
        ActivationLedgerSnapshotV1::Abandoned(abandoned) => abandoned.activation_cursor(),
        ActivationLedgerSnapshotV1::Acquiring(_)
        | ActivationLedgerSnapshotV1::Leased(_)
        | ActivationLedgerSnapshotV1::Completing { .. } => return Err(conflict()),
    };
    last.checked_add(1).ok_or_else(invalid)
}

fn validate_snapshot(
    assignment_id: &str,
    snapshot: &ActivationLedgerSnapshotV1,
) -> Result<(), ActivationLedgerErrorV1> {
    validate_assignment_id(assignment_id)?;
    match snapshot {
        ActivationLedgerSnapshotV1::Idle { .. } => Ok(()),
        ActivationLedgerSnapshotV1::Acquiring(prepared) => {
            if prepared.assignment_id() == assignment_id
                && prepared.activation_cursor() > 0
                && prepared.offer_operation_id().parse::<UlidString>().is_ok()
                && prepared.claim_id().parse::<UlidString>().is_ok()
            {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        ActivationLedgerSnapshotV1::Leased(lease) => validate_lease(assignment_id, lease),
        ActivationLedgerSnapshotV1::Completing { lease, prepared } => {
            validate_lease(assignment_id, lease)?;
            if prepared.matches_lease(lease)
                && prepared.operation_id().parse::<UlidString>().is_ok()
                && prepared.completion_id().parse::<UlidString>().is_ok()
                && prepared.remote_request_id().parse::<UlidString>().is_ok()
                && valid_digest(prepared.canonical_request_hash())
            {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        ActivationLedgerSnapshotV1::Completed(receipt) => {
            if receipt.activation_cursor > 0
                && receipt.activation_id.parse::<UlidString>().is_ok()
                && receipt.operation_id.parse::<UlidString>().is_ok()
                && receipt.completion_id.parse::<UlidString>().is_ok()
            {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        ActivationLedgerSnapshotV1::Terminal {
            activation_cursor,
            activation_id,
            claim_id,
            lease_generation,
            ..
        } => {
            if *activation_cursor > 0
                && activation_id.parse::<UlidString>().is_ok()
                && claim_id.parse::<UlidString>().is_ok()
                && *lease_generation > 0
            {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        ActivationLedgerSnapshotV1::Abandoned(abandoned) => {
            if abandoned.assignment_id() == assignment_id
                && abandoned.activation_cursor() > 0
                && abandoned.activation_id().parse::<UlidString>().is_ok()
                && abandoned.claim_id().parse::<UlidString>().is_ok()
            {
                Ok(())
            } else {
                Err(invalid())
            }
        }
    }
}

fn validate_lease(
    assignment_id: &str,
    lease: &AssignedActivationLeaseV1,
) -> Result<(), ActivationLedgerErrorV1> {
    if lease.assignment_id() != assignment_id {
        return Err(invalid());
    }
    let rebuilt = AssignedActivationLeaseV1::new(
        assignment_id,
        lease.activation_cursor,
        lease.activation_id(),
        lease.claim_id(),
        lease.context_hash(),
        lease.context.clone(),
    )?;
    if rebuilt == *lease {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn read_private_record(path: &Path) -> Result<PrivateActivationRecordV1, ActivationLedgerErrorV1> {
    validate_owner_only_file(path).map_err(|_| unavailable())?;
    let metadata = fs::metadata(path).map_err(|_| unavailable())?;
    if metadata.len() > MAX_PRIVATE_RECORD_BYTES {
        return Err(invalid());
    }
    let bytes = fs::read(path).map_err(|_| unavailable())?;
    let record: PrivateActivationRecordV1 =
        serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if record.schema != PRIVATE_SCHEMA_V1
        || record.integrity_hash != private_integrity_hash(&record)?
    {
        return Err(invalid());
    }
    validate_snapshot(&record.assignment_id, &record.snapshot)?;
    Ok(record)
}

fn private_integrity_hash(
    record: &PrivateActivationRecordV1,
) -> Result<String, ActivationLedgerErrorV1> {
    let bytes = serde_json::to_vec(&PrivateIntegrityWitnessV1 {
        schema: &record.schema,
        assignment_id: &record.assignment_id,
        snapshot: &record.snapshot,
    })
    .map_err(|_| invalid())?;
    Ok(digest(&bytes))
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, ActivationLedgerErrorV1> {
    let encoded = serde_json::to_vec(value).map_err(|_| invalid())?;
    CanonicalJsonV1::parse(&encoded)
        .and_then(|value| value.to_bytes())
        .map_err(|_| invalid())
}

fn validate_assignment_id(value: &str) -> Result<(), ActivationLedgerErrorV1> {
    value
        .parse::<UlidString>()
        .map(|_| ())
        .map_err(|_| invalid())
}

fn valid_digest(value: &str) -> bool {
    value.len() == "blake3:".len() + 64
        && value.starts_with("blake3:")
        && value["blake3:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/assignment-activation-private-ledger@1\0");
    hasher.update(bytes);
    format!("blake3:{}", hasher.finalize().to_hex())
}

fn next_ulid() -> Result<String, ActivationLedgerErrorV1> {
    const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
    bytes[0] &= 0x3f;
    let mut value = u128::from_be_bytes(bytes);
    let mut encoded = [b'0'; 26];
    for character in encoded.iter_mut().rev() {
        *character = CROCKFORD[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(encoded.to_vec()).map_err(|_| unavailable())
}

fn random_hex() -> Result<String, ActivationLedgerErrorV1> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
    Ok(encode_bytes(&bytes))
}

fn encode_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

const fn map_operation_error(
    error: crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1,
) -> ActivationLedgerErrorV1 {
    match error {
        crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::Conflict => conflict(),
        crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::Unavailable
        | crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::CapacityExceeded => {
            unavailable()
        }
        crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::InvalidData
        | crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::CredentialData
        | crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::PrivateData
        | crate::assignment_mcp_operations::AssignmentMcpOperationErrorV1::NotFound => invalid(),
    }
}

const fn conflict() -> ActivationLedgerErrorV1 {
    ActivationLedgerErrorV1::Conflict
}

const fn unavailable() -> ActivationLedgerErrorV1 {
    ActivationLedgerErrorV1::Unavailable
}

const fn invalid() -> ActivationLedgerErrorV1 {
    ActivationLedgerErrorV1::InvalidData
}

#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path is not Unicode",
        )
    })?;
    let target = target.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path is not Unicode",
        )
    })?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}
