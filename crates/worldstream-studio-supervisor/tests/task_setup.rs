use std::sync::{Arc, Mutex, PoisonError};

use serde_json::json;
use tempfile::tempdir;
use worldstream_protocol::{
    CreateRoomRequest, CreateRoomResponse, MemberCapabilityProvisionRequestV1,
    MemberCapabilityProvisionResponseV1, RunnerCapabilityProvisionRequestV1,
    RunnerCapabilityProvisionResponseV1,
};
use worldstream_studio_supervisor::{
    room_creation::{DaemonRoomCreatorV1, RoomCreationAttemptErrorV1, RoomCreationSupervisorV1},
    room_drafts::{RoomDraftErrorV1, RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1},
    secrets::FileSecretVaultV1,
    task_setup::{
        DaemonTaskSetupProvisionerV1, TaskSetupAttemptErrorV1, TaskSetupStateV1,
        TaskSetupSupervisorV1,
    },
};

const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

#[derive(Clone, Copy)]
struct ValidDraft;

impl RoomDraftValidatorV1 for ValidDraft {
    fn validate(
        &self,
        _draft: &RoomDraftV1,
    ) -> Result<
        Vec<worldstream_studio_supervisor::room_drafts::RoomDraftFieldErrorV1>,
        RoomDraftErrorV1,
    > {
        Ok(Vec::new())
    }
}

fn reviewed_draft() -> RoomDraftV1 {
    serde_json::from_value(json!({
        "schema": "worldstream/studio-room-draft/v1",
        "draft_id": "setup-alpha",
        "pack": { "id": "counter", "version": "2.0.0", "digest": DIGEST },
        "configuration": { "initial_value": 0, "maximum_value": 8 },
        "seats": [
            {
                "seat_id": "navigator-1", "role": "navigator", "required": true,
                "display_name": "Navigator", "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                "principal_kind": "human"
            },
            {
                "seat_id": "analyst-1", "role": "analyst", "required": true,
                "display_name": "Analyst", "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                "principal_kind": "agent", "agent_assignment": "external"
            },
            {
                "seat_id": "broker-1", "role": "broker", "required": false,
                "display_name": "Broker"
            }
        ],
        "readiness": [
            { "seat_id": "navigator-1", "role": "navigator", "required": true },
            { "seat_id": "analyst-1", "role": "analyst", "required": true },
            { "seat_id": "broker-1", "role": "broker", "required": false }
        ],
        "last_valid_step": "review"
    }))
    .unwrap_or_else(|error| unreachable!("reviewed draft: {error}"))
}

fn room_response() -> CreateRoomResponse {
    serde_json::from_value(json!({
        "room_id": ROOM,
        "member_ids": ["01ARZ3NDEKTSV4RRFFQ69G5FAY", "01ARZ3NDEKTSV4RRFFQ69G5FAZ"],
        "room_head": {
            "room_id": ROOM, "room_seq": 0,
            "genesis_or_transition_hash": format!("blake3:{}", "b".repeat(64)),
            "core_schema_version": "worldstream/core-room-state/v1", "pack_digest": DIGEST,
            "core_state_hash": format!("blake3:{}", "c".repeat(64)),
            "activity_state_hash": format!("blake3:{}", "d".repeat(64)),
            "authoritative_state_hash": format!("blake3:{}", "e".repeat(64))
        }
    }))
    .unwrap_or_else(|error| unreachable!("room response: {error}"))
}

#[derive(Clone)]
struct Creator;

impl DaemonRoomCreatorV1 for Creator {
    fn create(
        &self,
        _request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        Ok(room_response())
    }
}

#[derive(Default)]
struct ProvisionLedger {
    member_requests: Vec<ProvisionCall>,
    runner_requests: Vec<ProvisionCall>,
    member_receipts: Vec<MemberCapabilityProvisionResponseV1>,
    runner_receipts: Vec<RunnerCapabilityProvisionResponseV1>,
    lose_first_member_response: bool,
    lose_first_runner_response: bool,
    reject_runner_once: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProvisionCall {
    capability_id: String,
    change_id: String,
    bearer_digest: String,
}

impl ProvisionCall {
    fn from_member(request: &MemberCapabilityProvisionRequestV1) -> Self {
        Self {
            capability_id: request.capability.capability_id.clone(),
            change_id: request.capability.capability_idempotency_key.clone(),
            bearer_digest: blake3::hash(request.capability.bearer.as_str().as_bytes())
                .to_hex()
                .to_string(),
        }
    }

    fn from_runner(request: &RunnerCapabilityProvisionRequestV1) -> Self {
        Self {
            capability_id: request.capability.capability_id.clone(),
            change_id: request.capability.capability_idempotency_key.clone(),
            bearer_digest: blake3::hash(request.capability.bearer.as_str().as_bytes())
                .to_hex()
                .to_string(),
        }
    }
}

#[derive(Clone)]
struct DurableProvisioner(Arc<Mutex<ProvisionLedger>>);

impl DaemonTaskSetupProvisionerV1 for DurableProvisioner {
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger
            .member_requests
            .push(ProvisionCall::from_member(request));
        if let Some(receipt) = ledger
            .member_receipts
            .iter()
            .find(|receipt| receipt.capability_id == request.capability.capability_id)
            .cloned()
        {
            return Ok(receipt);
        }
        let receipt = MemberCapabilityProvisionResponseV1 {
            capability_id: request.capability.capability_id.clone(),
            room_id: request.room_id.clone(),
            member_id: request.member_id.clone(),
            principal_id: request.principal_id.clone(),
            scopes: request.scopes.clone(),
        };
        ledger.member_receipts.push(receipt.clone());
        if ledger.lose_first_member_response {
            ledger.lose_first_member_response = false;
            Err(TaskSetupAttemptErrorV1::Ambiguous)
        } else {
            Ok(receipt)
        }
    }

    fn provision_runner(
        &self,
        request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger
            .runner_requests
            .push(ProvisionCall::from_runner(request));
        if ledger.reject_runner_once {
            ledger.reject_runner_once = false;
            return Err(TaskSetupAttemptErrorV1::OperatorFixRequired);
        }
        if let Some(receipt) = ledger
            .runner_receipts
            .iter()
            .find(|receipt| receipt.capability_id == request.capability.capability_id)
            .cloned()
        {
            return Ok(receipt);
        }
        let receipt = RunnerCapabilityProvisionResponseV1 {
            capability_id: request.capability.capability_id.clone(),
            runner_id: request.runner_id.clone(),
            owner_principal_id: request.owner_principal_id.clone(),
            permitted_memberships: request.permitted_memberships.clone(),
            scopes: request.scopes.clone(),
        };
        ledger.runner_receipts.push(receipt.clone());
        if ledger.lose_first_runner_response {
            ledger.lose_first_runner_response = false;
            Err(TaskSetupAttemptErrorV1::Ambiguous)
        } else {
            Ok(receipt)
        }
    }
}

fn open_creation(root: &std::path::Path) -> RoomCreationSupervisorV1 {
    let drafts = RoomDraftStoreV1::open(&root.join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    if drafts.load("setup-alpha").is_err() {
        drafts
            .save(&reviewed_draft())
            .unwrap_or_else(|error| unreachable!("save reviewed draft: {error:?}"));
    }
    let creation = RoomCreationSupervisorV1::open(&root.join("creations"), drafts, Creator)
        .unwrap_or_else(|error| unreachable!("creation store: {error:?}"));
    creation
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("create Room: {error:?}"));
    creation
}

fn open_setup(root: &std::path::Path, provisioner: DurableProvisioner) -> TaskSetupSupervisorV1 {
    let vault = FileSecretVaultV1::open(&root.join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    TaskSetupSupervisorV1::open(
        &root.join("setups"),
        open_creation(root),
        vault,
        provisioner,
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
}

#[test]
fn lost_response_and_supervisor_restart_reconcile_the_same_stable_stage() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = Arc::new(Mutex::new(ProvisionLedger {
        lose_first_member_response: true,
        ..ProvisionLedger::default()
    }));
    let first = open_setup(directory.path(), DurableProvisioner(Arc::clone(&ledger)));
    let blocked = first
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start setup: {error:?}"));
    assert_eq!(blocked.state, TaskSetupStateV1::NeedsAttention);
    assert_eq!(blocked.completed_stages, 0);
    let operation_id = blocked.operation_id.clone();
    drop(first);

    let restarted = open_setup(directory.path(), DurableProvisioner(Arc::clone(&ledger)));
    let ready = restarted
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("restart retry: {error:?}"));
    assert_eq!(ready.state, TaskSetupStateV1::Ready);
    assert_eq!(ready.operation_id, operation_id);
    assert_eq!(ready.completed_stages, 3);
    assert_eq!(ready.total_stages, 3);
    assert_eq!(ready.seats[2].member_authority, "unfilled_optional");
    let ledger = ledger.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.member_requests[0], ledger.member_requests[1]);
    assert_eq!(ledger.runner_requests.len(), 1);
    assert_ne!(
        ledger.member_requests[1].bearer_digest,
        ledger.runner_requests[0].bearer_digest
    );
}

#[test]
fn lost_runner_response_retries_only_the_same_runner_subcommit() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = Arc::new(Mutex::new(ProvisionLedger {
        lose_first_runner_response: true,
        ..ProvisionLedger::default()
    }));
    let supervisor = open_setup(directory.path(), DurableProvisioner(Arc::clone(&ledger)));

    let blocked = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start setup: {error:?}"));
    assert_eq!(blocked.state, TaskSetupStateV1::NeedsAttention);
    assert_eq!(blocked.completed_stages, 2);

    let ready = supervisor
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry runner setup: {error:?}"));
    assert_eq!(ready.state, TaskSetupStateV1::Ready);
    let ledger = ledger.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.member_requests.len(), 2);
    assert_eq!(ledger.runner_requests.len(), 2);
    assert_eq!(ledger.runner_requests[0], ledger.runner_requests[1]);
    assert_eq!(ledger.runner_receipts.len(), 1);
}

#[test]
fn partial_provisioning_and_repeated_retry_checkpoint_each_completed_stage() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = Arc::new(Mutex::new(ProvisionLedger {
        reject_runner_once: true,
        ..ProvisionLedger::default()
    }));
    let supervisor = open_setup(directory.path(), DurableProvisioner(Arc::clone(&ledger)));
    let partial = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("partial setup: {error:?}"));
    assert_eq!(partial.state, TaskSetupStateV1::NeedsAttention);
    assert_eq!(partial.completed_stages, 2);
    assert_eq!(
        partial
            .active_stage
            .as_ref()
            .map(|stage| format!("{stage:?}")),
        Some("RunnerCapability { seat_id: \"analyst-1\" }".to_owned())
    );

    let ready = supervisor
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry setup: {error:?}"));
    assert_eq!(ready.state, TaskSetupStateV1::Ready);
    let repeated = supervisor
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("repeated retry: {error:?}"));
    assert_eq!(repeated, ready);
    let ledger = ledger.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.member_requests.len(), 2);
    assert_eq!(ledger.runner_requests.len(), 2);
}

#[test]
fn unavailable_retained_secret_is_visible_only_as_safe_retryable_attention() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = Arc::new(Mutex::new(ProvisionLedger {
        lose_first_member_response: true,
        ..ProvisionLedger::default()
    }));
    let supervisor = open_setup(directory.path(), DurableProvisioner(Arc::clone(&ledger)));
    supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("publish setup: {error:?}"));
    let operation: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("setups/setup-alpha.json"))
            .unwrap_or_else(|error| unreachable!("read operation: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("parse operation: {error}"));
    let reference = operation["seats"][0]["member_capability"]["secret_reference"]
        .as_str()
        .unwrap_or_else(|| unreachable!("member secret reference"));
    std::fs::remove_file(
        directory
            .path()
            .join(format!("secrets/membership-{reference}.secret")),
    )
    .unwrap_or_else(|error| unreachable!("remove retained secret: {error}"));

    let attention = supervisor
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry missing secret: {error:?}"));
    assert_eq!(attention.state, TaskSetupStateV1::NeedsAttention);
    assert_eq!(
        attention
            .attention
            .as_ref()
            .map(|value| value.code.as_str()),
        Some("setup_credential_unavailable")
    );
    let browser = serde_json::to_string(&attention)
        .unwrap_or_else(|error| unreachable!("browser status: {error}"));
    for forbidden in [
        "bearer",
        "secret_reference",
        reference,
        "request",
        "receipt",
    ] {
        assert!(!browser.contains(forbidden), "browser leaked {forbidden}");
    }
}

#[test]
fn corrupt_checkpoint_cannot_claim_ready_or_issue_another_daemon_effect() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = Arc::new(Mutex::new(ProvisionLedger {
        lose_first_member_response: true,
        ..ProvisionLedger::default()
    }));
    let supervisor = open_setup(directory.path(), DurableProvisioner(Arc::clone(&ledger)));
    let blocked = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("blocked setup: {error:?}"));
    assert_eq!(blocked.state, TaskSetupStateV1::NeedsAttention);
    let calls_before_corruption = ledger
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .member_requests
        .len();
    let path = directory.path().join("setups/setup-alpha.json");
    let mut record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| unreachable!("read operation: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("parse operation: {error}"));
    record["state"] = json!("ready");
    record["active_stage"] = serde_json::Value::Null;
    record["attention"] = serde_json::Value::Null;
    for seat in record["seats"]
        .as_array_mut()
        .unwrap_or_else(|| unreachable!("setup seats"))
    {
        if let Some(capability) = seat
            .get_mut("member_capability")
            .and_then(serde_json::Value::as_object_mut)
        {
            capability.insert("provisioned".to_owned(), json!(true));
        }
        if let Some(runner) = seat
            .get_mut("runner")
            .and_then(serde_json::Value::as_object_mut)
            && let Some(capability) = runner
                .get_mut("capability")
                .and_then(serde_json::Value::as_object_mut)
        {
            capability.insert("provisioned".to_owned(), json!(true));
        }
    }
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&record)
            .unwrap_or_else(|error| unreachable!("encode corruption: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("write corruption: {error}"));

    assert_eq!(
        supervisor.status("setup-alpha"),
        Err(worldstream_studio_supervisor::task_setup::TaskSetupErrorV1::Unavailable)
    );
    assert_eq!(
        supervisor.retry("setup-alpha"),
        Err(worldstream_studio_supervisor::task_setup::TaskSetupErrorV1::Unavailable)
    );
    assert_eq!(
        ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .member_requests
            .len(),
        calls_before_corruption
    );
}
