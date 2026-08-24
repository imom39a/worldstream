use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread,
    time::Duration,
};

use serde_json::json;
use tempfile::tempdir;
use worldstream_protocol::{
    CreateRoomRequest, CreateRoomResponse, LobbyLaunchRequest, LobbyLaunchResponse,
    MemberCapabilityProvisionRequestV1, MemberCapabilityProvisionResponseV1,
    OperatorRunnerConnectionV1, OperatorRunnerFreshnessV1, OperatorRunnerPresenceV1, PackReference,
    RoomHead, RunnerCapabilityProvisionRequestV1, RunnerCapabilityProvisionResponseV1,
};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::assignment_mcp::AssignedMembershipLaunchSourceV1;
use worldstream_studio_supervisor::{
    agent_profiles::{
        AgentHostContractV1, AgentProfileRevisionV1, AgentProfileSecretSettingV1,
        AgentProfileStoreV1, ManagedReferenceProviderV1,
    },
    participant_handoff::{
        ParticipantConsoleReadinessSourceV1, ParticipantConsoleSessionHealthV1,
        ParticipantHandoffAuthorityErrorV1, ParticipantHandoffAuthoritySourceV1,
    },
    room_creation::{
        DaemonRoomCreatorV1, RoomCreationAttemptErrorV1, RoomCreationSupervisorV1,
        room_creation_router,
    },
    room_drafts::{
        RoomDraftErrorV1, RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1,
        RunnerTemplateRevisionReferenceV1,
    },
    secrets::{FileSecretVaultV1, SecretKindV1},
    task_setup::{
        DaemonTaskLaunchSourceV1, DaemonTaskSetupProvisionerV1, FileAssignedMembershipSourceV1,
        TaskLaunchAttemptErrorV1, TaskLaunchStateV1, TaskRunnerObservationV1,
        TaskRunnerReadinessSourceV1, TaskSeatReadinessReasonV1, TaskSetupAttemptErrorV1,
        TaskSetupStateV1, TaskSetupSupervisorV1, task_setup_router,
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
                "principal_kind": "agent", "agent_assignment": "external",
                "agent_profile": { "profile_id": "analyst", "revision": "rev-1" }
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
struct Creator(CreateRoomResponse);

impl DaemonRoomCreatorV1 for Creator {
    fn create(
        &self,
        _request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        Ok(self.0.clone())
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
    open_creation_for(root, &reviewed_draft(), room_response())
}

fn open_creation_for(
    root: &std::path::Path,
    draft: &RoomDraftV1,
    response: CreateRoomResponse,
) -> RoomCreationSupervisorV1 {
    let drafts = RoomDraftStoreV1::open(&root.join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    if drafts.load("setup-alpha").is_err() {
        drafts
            .save(draft)
            .unwrap_or_else(|error| unreachable!("save reviewed draft: {error:?}"));
    }
    let creation =
        RoomCreationSupervisorV1::open(&root.join("creations"), drafts, Creator(response))
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
fn action_routes_construct_with_distinct_path_segments() {
    let creation_directory =
        tempdir().unwrap_or_else(|error| unreachable!("temporary creation root: {error}"));
    let _creation_router = room_creation_router(open_creation(creation_directory.path()));

    let setup_directory =
        tempdir().unwrap_or_else(|error| unreachable!("temporary setup root: {error}"));
    let _setup_router = task_setup_router(open_setup(
        setup_directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    ));
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
fn transient_agent_profile_binding_unavailability_is_retryable() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let profile_root = directory.path().join("agent-profiles");
    let profiles = AgentProfileStoreV1::open(&profile_root, vault.clone())
        .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    profiles
        .publish(&AgentProfileRevisionV1 {
            schema: "worldstream/studio-agent-profile/v1".to_owned(),
            profile_id: "analyst".to_owned(),
            revision: "rev-1".to_owned(),
            display_name: "Analyst".to_owned(),
            non_secret_configuration: BTreeMap::new(),
            secret_settings: Vec::new(),
            host_contract: AgentHostContractV1::default(),
        })
        .unwrap_or_else(|error| unreachable!("publish profile: {error:?}"));
    std::fs::remove_dir(profile_root.join("assignments"))
        .unwrap_or_else(|error| unreachable!("remove assignments directory: {error}"));
    let supervisor = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        open_creation(directory.path()),
        vault.clone(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_agent_profiles(profiles.clone());

    let blocked = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start setup: {error:?}"));
    assert_eq!(blocked.state, TaskSetupStateV1::NeedsAttention);
    assert_eq!(
        blocked
            .attention
            .as_ref()
            .map(|attention| attention.retryable),
        Some(true)
    );
    assert_eq!(
        blocked
            .attention
            .as_ref()
            .map(|attention| attention.code.as_str()),
        Some("daemon_result_ambiguous")
    );

    prepare_data_directory(&profile_root.join("assignments"))
        .unwrap_or_else(|error| unreachable!("restore assignments directory: {error}"));
    let ready = supervisor
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry setup: {error:?}"));
    assert_eq!(ready.state, TaskSetupStateV1::Ready);

    let assignment = profiles
        .assignments()
        .unwrap_or_else(|error| unreachable!("assignments: {error:?}"))
        .assignments
        .into_iter()
        .next()
        .unwrap_or_else(|| unreachable!("Agent assignment"));
    let launch_source =
        FileAssignedMembershipSourceV1::open(&directory.path().join("setups"), profiles, vault)
            .unwrap_or_else(|error| unreachable!("launch source: {error:?}"));
    let binding = launch_source
        .resolve_launch_binding(&assignment.assignment_id)
        .unwrap_or_else(|error| unreachable!("launch binding: {error:?}"));
    assert_eq!(binding.assignment_id, assignment.assignment_id);
    assert_eq!(binding.room_id, assignment.membership.room_id);
    assert_eq!(binding.member_id, assignment.membership.member_id);
    assert_eq!(binding.principal_id, assignment.membership.principal_id);
}

#[test]
fn managed_reference_profile_is_rejected_for_an_external_agent_seat() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let credential = vault
        .store(SecretKindV1::ModelProvider, b"provider-credential")
        .unwrap_or_else(|error| unreachable!("model credential: {error:?}"));
    let profiles =
        AgentProfileStoreV1::open(&directory.path().join("agent-profiles"), vault.clone())
            .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    profiles
        .publish(&AgentProfileRevisionV1 {
            schema: "worldstream/studio-agent-profile/v1".to_owned(),
            profile_id: "analyst".to_owned(),
            revision: "rev-1".to_owned(),
            display_name: "Managed Analyst".to_owned(),
            non_secret_configuration: BTreeMap::new(),
            secret_settings: vec![AgentProfileSecretSettingV1 {
                key: "MODEL_PROVIDER_TOKEN".to_owned(),
                kind: SecretKindV1::ModelProvider,
                reference: credential,
            }],
            host_contract: AgentHostContractV1::ManagedReference {
                host_contract_revision: "v1".to_owned(),
                runner_template: RunnerTemplateRevisionReferenceV1 {
                    template_id: "reference-agent-host".to_owned(),
                    revision: "r1".to_owned(),
                },
                provider: ManagedReferenceProviderV1::OpenAiCompatible,
                provider_address: "127.0.0.1:11434"
                    .parse()
                    .unwrap_or_else(|error| unreachable!("provider address: {error}")),
                model_id: "test-model".to_owned(),
            },
        })
        .unwrap_or_else(|error| unreachable!("publish profile: {error:?}"));
    let supervisor = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        open_creation(directory.path()),
        vault,
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_agent_profiles(profiles);

    let rejected = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("closed setup rejection: {error:?}"));
    assert_eq!(rejected.state, TaskSetupStateV1::NeedsAttention);
    assert_eq!(
        rejected.attention.as_ref().map(|value| value.code.as_str()),
        Some("setup_stage_rejected")
    );
    assert_eq!(
        rejected.attention.as_ref().map(|value| value.retryable),
        Some(false)
    );
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
    assert!(matches!(
        supervisor.resolve_provisioned_human_seat("setup-alpha", "navigator-1"),
        Err(ParticipantHandoffAuthorityErrorV1::Unavailable)
    ));
    assert_eq!(
        ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .member_requests
            .len(),
        calls_before_corruption
    );
}

#[derive(Clone)]
struct ConsoleHealth(Arc<Mutex<ParticipantConsoleSessionHealthV1>>);

impl ParticipantConsoleReadinessSourceV1 for ConsoleHealth {
    fn session_health(
        &self,
        _room_id: &str,
        _member_id: &str,
    ) -> ParticipantConsoleSessionHealthV1 {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone)]
struct TaskSetupBackedConsole(TaskSetupSupervisorV1);

impl ParticipantConsoleReadinessSourceV1 for TaskSetupBackedConsole {
    fn session_health(
        &self,
        _room_id: &str,
        _member_id: &str,
    ) -> ParticipantConsoleSessionHealthV1 {
        match self
            .0
            .resolve_provisioned_human_seat("setup-alpha", "navigator-1")
        {
            Ok(_) => ParticipantConsoleSessionHealthV1::Usable,
            Err(_) => ParticipantConsoleSessionHealthV1::Invalid,
        }
    }
}

#[test]
fn task_setup_backed_console_authority_does_not_reenter_the_mutation_lock() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let base = open_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    );
    base.start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    let supervisor = base.clone().with_launch_readiness(
        TaskSetupBackedConsole(base),
        RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
        Launcher(Arc::new(Mutex::new(LaunchLedger::default()))),
    );
    let (sender, receiver) = mpsc::channel();
    let status_supervisor = supervisor.clone();
    thread::spawn(move || {
        sender
            .send(status_supervisor.status("setup-alpha"))
            .unwrap_or_else(|error| unreachable!("send status: {error}"));
    });
    let status = receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|error| unreachable!("circular readiness source deadlocked: {error}"))
        .unwrap_or_else(|error| unreachable!("status: {error:?}"));
    assert_eq!(
        status.readiness.seats[0].reason,
        TaskSeatReadinessReasonV1::Ready
    );
    let launched = supervisor
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("launch: {error:?}"));
    assert_eq!(
        launched.launch.as_ref().map(|launch| launch.state),
        Some(TaskLaunchStateV1::Launched)
    );
}

#[derive(Clone, Copy)]
struct OptionalSeatDisconnected;

impl ParticipantConsoleReadinessSourceV1 for OptionalSeatDisconnected {
    fn session_health(&self, _room_id: &str, member_id: &str) -> ParticipantConsoleSessionHealthV1 {
        if member_id == "01ARZ3NDEKTSV4RRFFQ69G5FB1" {
            ParticipantConsoleSessionHealthV1::Disconnected
        } else {
            ParticipantConsoleSessionHealthV1::Usable
        }
    }
}

#[derive(Clone)]
struct RunnerHealth(Arc<Mutex<RunnerMode>>);

#[derive(Clone, Copy)]
enum RunnerMode {
    Ready,
    Missing,
    Stale,
    Disconnected,
    Full,
    Incompatible,
}

impl TaskRunnerReadinessSourceV1 for RunnerHealth {
    fn presence(&self, runner_id: &str) -> TaskRunnerObservationV1 {
        let mode = *self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(mode, RunnerMode::Missing) {
            return TaskRunnerObservationV1::Missing;
        }
        TaskRunnerObservationV1::Present(OperatorRunnerPresenceV1 {
            version: "worldstream/operator-runner-presence/v1".to_owned(),
            runner_id: runner_id.to_owned(),
            connection: if matches!(mode, RunnerMode::Disconnected) {
                OperatorRunnerConnectionV1::Disconnected
            } else {
                OperatorRunnerConnectionV1::Connected
            },
            freshness: if matches!(mode, RunnerMode::Stale) {
                OperatorRunnerFreshnessV1::Stale
            } else {
                OperatorRunnerFreshnessV1::Fresh
            },
            maximum_concurrent_activations: 1,
            active_activations: u32::from(matches!(mode, RunnerMode::Full)),
            available_activations: u32::from(!matches!(mode, RunnerMode::Full)),
            supported_pack_revisions: if matches!(mode, RunnerMode::Incompatible) {
                Vec::new()
            } else {
                vec![PackReference {
                    id: "counter".to_owned(),
                    version: "2.0.0".to_owned(),
                    digest: DIGEST.to_owned(),
                }]
            },
            observed_at_unix_ms: 1,
        })
    }
}

#[derive(Default)]
struct LaunchLedger {
    calls: Vec<LobbyLaunchRequest>,
    response: Option<LobbyLaunchResponse>,
    lose_first_response: bool,
    hide_committed_head_once: bool,
}

#[derive(Clone)]
struct Launcher(Arc<Mutex<LaunchLedger>>);

fn launched_head() -> RoomHead {
    let mut head = room_response().room_head;
    head.room_seq = 1;
    head.genesis_or_transition_hash = format!("blake3:{}", "f".repeat(64));
    head
}

impl DaemonTaskLaunchSourceV1 for Launcher {
    fn launch(
        &self,
        room_id: &str,
        request: &LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, TaskLaunchAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger.calls.push(request.clone());
        let response = ledger
            .response
            .get_or_insert_with(|| LobbyLaunchResponse {
                room_id: room_id.to_owned(),
                input_id: request.input_id.clone(),
                transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned(),
                room_head: launched_head(),
                duplicate: false,
            })
            .clone();
        if ledger.lose_first_response {
            ledger.lose_first_response = false;
            Err(TaskLaunchAttemptErrorV1::Ambiguous)
        } else {
            Ok(response)
        }
    }

    fn room_head(&self, _room_id: &str) -> Result<RoomHead, TaskLaunchAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if ledger.hide_committed_head_once {
            ledger.hide_committed_head_once = false;
            return Ok(room_response().room_head);
        }
        ledger
            .response
            .as_ref()
            .map(|response| response.room_head.clone())
            .ok_or(TaskLaunchAttemptErrorV1::Ambiguous)
    }
}

fn open_launch_ready_setup(
    root: &std::path::Path,
    provisioner: DurableProvisioner,
    console: ConsoleHealth,
    runners: RunnerHealth,
    launcher: Launcher,
) -> TaskSetupSupervisorV1 {
    open_setup(root, provisioner).with_launch_readiness(console, runners, launcher)
}

#[test]
fn readiness_reports_exact_console_and_runner_reasons_and_optional_unfilled_is_nonblocking() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let console = Arc::new(Mutex::new(ParticipantConsoleSessionHealthV1::Usable));
    let runner = Arc::new(Mutex::new(RunnerMode::Ready));
    let supervisor = open_launch_ready_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
        ConsoleHealth(Arc::clone(&console)),
        RunnerHealth(Arc::clone(&runner)),
        Launcher(Arc::new(Mutex::new(LaunchLedger::default()))),
    );
    let status = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    assert!(status.readiness.ready_to_launch);
    assert_eq!(
        status.readiness.seats[2].reason,
        TaskSeatReadinessReasonV1::OptionalUnfilled
    );

    for (health, reason) in [
        (
            ParticipantConsoleSessionHealthV1::Missing,
            TaskSeatReadinessReasonV1::ConsoleMissing,
        ),
        (
            ParticipantConsoleSessionHealthV1::Stale,
            TaskSeatReadinessReasonV1::ConsoleStale,
        ),
        (
            ParticipantConsoleSessionHealthV1::Disconnected,
            TaskSeatReadinessReasonV1::ConsoleDisconnected,
        ),
    ] {
        *console.lock().unwrap_or_else(PoisonError::into_inner) = health;
        let status = supervisor
            .status("setup-alpha")
            .unwrap_or_else(|error| unreachable!("status: {error:?}"));
        assert_eq!(status.readiness.seats[0].reason, reason);
        assert!(!status.readiness.ready_to_launch);
    }
    *console.lock().unwrap_or_else(PoisonError::into_inner) =
        ParticipantConsoleSessionHealthV1::Usable;
    for (mode, reason) in [
        (
            RunnerMode::Missing,
            TaskSeatReadinessReasonV1::RunnerMissing,
        ),
        (RunnerMode::Stale, TaskSeatReadinessReasonV1::RunnerStale),
        (
            RunnerMode::Disconnected,
            TaskSeatReadinessReasonV1::RunnerDisconnected,
        ),
        (
            RunnerMode::Full,
            TaskSeatReadinessReasonV1::RunnerOverCapacity,
        ),
        (
            RunnerMode::Incompatible,
            TaskSeatReadinessReasonV1::RunnerIncompatible,
        ),
    ] {
        *runner.lock().unwrap_or_else(PoisonError::into_inner) = mode;
        let status = supervisor
            .status("setup-alpha")
            .unwrap_or_else(|error| unreachable!("status: {error:?}"));
        assert_eq!(status.readiness.seats[1].reason, reason);
        assert!(!status.readiness.ready_to_launch);
    }
}

#[test]
fn filled_optional_seat_is_blocking_when_its_live_readiness_is_false() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let mut draft = reviewed_draft();
    draft.seats[2].principal_id = Some("01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned());
    draft.seats[2].principal_kind = Some(worldstream_protocol::PrincipalKind::Human);
    let mut response = room_response();
    response
        .member_ids
        .push("01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned());
    let creation = open_creation_for(directory.path(), &draft, response);
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let supervisor = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        creation,
        vault,
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_launch_readiness(
        OptionalSeatDisconnected,
        RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
        Launcher(Arc::new(Mutex::new(LaunchLedger::default()))),
    );

    let status = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start setup: {error:?}"));
    assert_eq!(status.state, TaskSetupStateV1::Ready);
    assert!(!status.readiness.seats[2].required);
    assert_eq!(
        status.readiness.seats[2].reason,
        TaskSeatReadinessReasonV1::ConsoleDisconnected
    );
    assert!(!status.readiness.ready_to_launch);
    assert_eq!(
        supervisor.launch("setup-alpha"),
        Err(worldstream_studio_supervisor::task_setup::TaskSetupErrorV1::NotReady)
    );
}

#[test]
fn lost_launch_response_restart_and_readiness_change_resolve_the_same_committed_launch() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let provisions = Arc::new(Mutex::new(ProvisionLedger::default()));
    let launches = Arc::new(Mutex::new(LaunchLedger {
        lose_first_response: true,
        ..LaunchLedger::default()
    }));
    let runner_health = Arc::new(Mutex::new(RunnerMode::Ready));
    let make = || {
        open_launch_ready_setup(
            directory.path(),
            DurableProvisioner(Arc::clone(&provisions)),
            ConsoleHealth(Arc::new(Mutex::new(
                ParticipantConsoleSessionHealthV1::Usable,
            ))),
            RunnerHealth(Arc::clone(&runner_health)),
            Launcher(Arc::clone(&launches)),
        )
    };
    let first = make();
    first
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    let ambiguous = first
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("launch: {error:?}"));
    assert_eq!(
        ambiguous.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::NeedsAttention)
    );
    drop(first);

    // Mutable capacity can disappear after the immutable launch was submitted.
    // Reconciliation must resolve that launch before applying a fresh gate.
    *runner_health.lock().unwrap_or_else(PoisonError::into_inner) = RunnerMode::Full;

    let restarted = make();
    let committed = restarted
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry: {error:?}"));
    assert_eq!(
        committed.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::Launched)
    );
    assert!(!committed.readiness.ready_to_launch);
    let calls = &launches
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .calls;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
}
