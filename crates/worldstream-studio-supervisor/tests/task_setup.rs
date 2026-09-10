use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread,
    time::Duration,
};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::json;
use tempfile::tempdir;
use tower::ServiceExt as _;
use worldstream_hosted_contract::{
    HostedBrowserHandoffRequestV1, HostedGenesisAccessModeV1, HostedGenesisMembershipPurposeV1,
    HostedGenesisPrincipalKindV1, PackReference as HostedPackReference,
};
use worldstream_protocol::{
    CreateRoomRequest, CreateRoomResponse, LobbyLaunchRequest, LobbyLaunchResponse,
    MemberCapabilityProvisionRequestV1, MemberCapabilityProvisionResponseV1,
    OperatorRunnerConnectionV1, OperatorRunnerFreshnessV1, OperatorRunnerPresenceV1, PackReference,
    RoomHead, RunnerCapabilityProvisionRequestV1, RunnerCapabilityProvisionResponseV1,
};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::assignment_mcp::{
    AssignedMembershipLaunchSourceV1, AssignmentMcpLaunchRegistryV1,
};
use worldstream_studio_supervisor::runner_attention::{
    AgentSeatAssignmentSourceV1, PersistedAgentSeatAssignmentSourceV1,
};
use worldstream_studio_supervisor::{
    agent_profiles::{
        AgentHostContractV1, AgentProfileRevisionV1, AgentProfileSecretSettingV1,
        AgentProfileStoreV1, ManagedReferenceProviderV1,
    },
    hosted_browser_sessions::HostedBrowserMembershipAuthoritySourceV1,
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
        ManagedRunnerAssignmentV1, TaskLaunchApplicabilitySourceV1, TaskLaunchApplicabilityV1,
        TaskLaunchAttemptErrorV1, TaskLaunchStateV1, TaskRunnerObservationV1,
        TaskRunnerReadinessSourceV1, TaskSeatReadinessReasonV1, TaskSetupAttemptErrorV1,
        TaskSetupErrorV1, TaskSetupStateV1, TaskSetupSupervisorV1, task_setup_router,
    },
};

const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

#[tokio::test]
async fn public_room_launch_reuses_the_retained_input_after_a_lost_reply()
-> Result<(), Box<dyn std::error::Error>> {
    use worldstream_studio_supervisor::room_launch::room_launch_router;

    let directory = tempdir()?;
    let provisions = Arc::new(Mutex::new(ProvisionLedger::default()));
    let launches = Arc::new(Mutex::new(LaunchLedger {
        lose_first_response: true,
        ..LaunchLedger::default()
    }));
    let runner = Arc::new(Mutex::new(RunnerMode::Ready));
    let make = || {
        open_launch_ready_setup(
            directory.path(),
            DurableProvisioner(Arc::clone(&provisions)),
            ConsoleHealth(Arc::new(Mutex::new(
                ParticipantConsoleSessionHealthV1::Usable,
            ))),
            RunnerHealth(Arc::clone(&runner)),
            Launcher(Arc::clone(&launches)),
        )
    };
    let setup = make();
    setup.start("setup-alpha")?;
    let app = room_launch_router(open_creation(directory.path()), setup);
    let (code, assessment) = public_launch_request(&app, "GET").await?;
    assert_eq!(code, 200);
    assert_eq!(assessment["operation"], "setup-alpha");
    assert_eq!(assessment["readiness"]["ready_to_launch"], true);
    let (code, pending) = public_launch_request(&app, "POST").await?;
    assert_eq!(code, 202);
    assert_eq!(pending["launch"]["state"], "needs_attention");
    drop(app);

    *runner.lock().unwrap_or_else(PoisonError::into_inner) = RunnerMode::Full;
    let restarted = room_launch_router(open_creation(directory.path()), make());
    let (code, committed) = public_launch_request(&restarted, "POST").await?;
    assert_eq!(code, 200);
    assert_eq!(committed["room_id"], ROOM);
    assert_eq!(committed["launch"]["state"], "launched");
    assert_eq!(committed["readiness"]["ready_to_launch"], false);
    let ledger = launches.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.calls.len(), 1);
    Ok(())
}

async fn public_launch_request(
    app: &axum::Router,
    method: &str,
) -> Result<(axum::http::StatusCode, serde_json::Value), Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/api/v1/rooms/{ROOM}/launch"))
                .body(Body::empty())?,
        )
        .await?;
    let status = response.status();
    let body = serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    Ok((status, body))
}

#[derive(Clone, Copy)]
struct ValidDraft;

#[derive(Clone, Copy)]
struct LaunchApplicability(TaskLaunchApplicabilityV1);

impl TaskLaunchApplicabilitySourceV1 for LaunchApplicability {
    fn applicability(
        &self,
        _pack: &PackReference,
    ) -> Result<TaskLaunchApplicabilityV1, TaskSetupErrorV1> {
        Ok(self.0)
    }
}

#[derive(Clone, Copy)]
struct UnavailableLaunchApplicability;

impl TaskLaunchApplicabilitySourceV1 for UnavailableLaunchApplicability {
    fn applicability(
        &self,
        _pack: &PackReference,
    ) -> Result<TaskLaunchApplicabilityV1, TaskSetupErrorV1> {
        Err(TaskSetupErrorV1::Unavailable)
    }
}

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
    member_id: Option<String>,
    scopes: Vec<String>,
}

impl ProvisionCall {
    fn from_member(request: &MemberCapabilityProvisionRequestV1) -> Self {
        Self {
            capability_id: request.capability.capability_id.clone(),
            change_id: request.capability.capability_idempotency_key.clone(),
            bearer_digest: blake3::hash(request.capability.bearer.as_str().as_bytes())
                .to_hex()
                .to_string(),
            member_id: Some(request.member_id.clone()),
            scopes: request.scopes.clone(),
        }
    }

    fn from_runner(request: &RunnerCapabilityProvisionRequestV1) -> Self {
        Self {
            capability_id: request.capability.capability_id.clone(),
            change_id: request.capability.capability_idempotency_key.clone(),
            bearer_digest: blake3::hash(request.capability.bearer.as_str().as_bytes())
                .to_hex()
                .to_string(),
            member_id: None,
            scopes: request.scopes.clone(),
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
    .with_launch_applicability(LaunchApplicability(
        TaskLaunchApplicabilityV1::ActiveAtGenesis,
    ))
}

#[test]
fn reviewed_operator_genesis_member_is_excluded_from_ordinary_task_setup_seats() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let mut draft = reviewed_draft();
    draft.operator_view = true;
    let mut creation_response = room_response();
    creation_response
        .member_ids
        .push("01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned());
    let creation = open_creation_for(directory.path(), &draft, creation_response);
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let ledger = Arc::new(Mutex::new(ProvisionLedger::default()));
    let setup = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        creation,
        vault,
        DurableProvisioner(Arc::clone(&ledger)),
    )
    .unwrap_or_else(|error| unreachable!("setup: {error:?}"))
    .with_launch_applicability(LaunchApplicability(
        TaskLaunchApplicabilityV1::ActiveAtGenesis,
    ));

    let status = setup
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    assert_eq!(status.total_stages, 3);
    assert_eq!(status.seats.len(), 3);
    assert_eq!(
        status.seats[0].member_id.as_deref(),
        Some("01ARZ3NDEKTSV4RRFFQ69G5FAY")
    );
    assert_eq!(
        status.seats[1].member_id.as_deref(),
        Some("01ARZ3NDEKTSV4RRFFQ69G5FAZ")
    );
    let ledger = ledger.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.member_requests.len(), 2);
    let human = ledger
        .member_requests
        .iter()
        .find(|request| request.member_id.as_deref() == Some("01ARZ3NDEKTSV4RRFFQ69G5FAY"))
        .unwrap_or_else(|| unreachable!("human member request"));
    assert_eq!(
        human.scopes,
        [
            "room:attach",
            "room:act",
            "room:observe_member",
            "room:replay"
        ]
    );
    let agent = ledger
        .member_requests
        .iter()
        .find(|request| request.member_id.as_deref() == Some("01ARZ3NDEKTSV4RRFFQ69G5FAZ"))
        .unwrap_or_else(|| unreachable!("agent member request"));
    assert_eq!(
        agent.scopes,
        ["room:attach", "room:act", "room:observe_member"]
    );
    assert!(
        ledger
            .member_requests
            .iter()
            .all(|request| { request.member_id.as_deref() != Some("01ARZ3NDEKTSV4RRFFQ69G5FB0") })
    );
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

#[tokio::test]
async fn counter_setup_is_active_at_genesis_and_never_submits_a_lobby_launch() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let supervisor = open_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    );
    let setup = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    assert_eq!(
        setup.launch_applicability,
        TaskLaunchApplicabilityV1::ActiveAtGenesis
    );
    assert!(setup.launch.is_none());
    assert_eq!(
        supervisor.launch("setup-alpha"),
        Err(TaskSetupErrorV1::NotReady)
    );
    let response = task_setup_router(supervisor)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/task-setups/setup-alpha/launch")
                .body(Body::empty())
                .unwrap_or_else(|error| unreachable!("launch request: {error}")),
        )
        .await
        .unwrap_or_else(|error| unreachable!("launch response: {error}"));
    assert_eq!(response.status(), 409);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| unreachable!("launch body: {error}"))
        .to_bytes();
    let error: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|error| unreachable!("launch JSON: {error}"));
    assert_eq!(error["error"]["code"], "task_launch_inapplicable");
    assert_eq!(error["error"]["retryable"], false);
}

#[test]
fn unavailable_exact_pack_declaration_cannot_be_assumed_active_at_genesis() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let supervisor = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        open_creation(directory.path()),
        vault,
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_launch_applicability(UnavailableLaunchApplicability);
    assert_eq!(
        supervisor.start("setup-alpha"),
        Err(TaskSetupErrorV1::Unavailable)
    );
    assert_eq!(
        supervisor.status("setup-alpha"),
        Err(TaskSetupErrorV1::NotFound)
    );
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
fn external_agent_without_a_profile_provisions_when_the_profile_store_exists() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let profiles =
        AgentProfileStoreV1::open(&directory.path().join("agent-profiles"), vault.clone())
            .unwrap_or_else(|error| unreachable!("profile store: {error:?}"));
    let mut draft = reviewed_draft();
    let analyst = draft
        .seats
        .iter_mut()
        .find(|seat| seat.seat_id == "analyst-1")
        .unwrap_or_else(|| unreachable!("analyst seat"));
    analyst.agent_profile = None;
    let ledger = Arc::new(Mutex::new(ProvisionLedger::default()));
    let supervisor = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        open_creation_for(directory.path(), &draft, room_response()),
        vault,
        DurableProvisioner(Arc::clone(&ledger)),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_agent_profiles(profiles.clone())
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch));

    let ready = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("start setup: {error:?}"));
    assert_eq!(ready.state, TaskSetupStateV1::Ready);
    assert_eq!(ready.completed_stages, 3);
    assert!(
        profiles
            .assignments()
            .unwrap_or_else(|error| unreachable!("assignments: {error:?}"))
            .assignments
            .is_empty()
    );
    let ledger = ledger.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.runner_receipts.len(), 1);
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
    .with_agent_profiles(profiles.clone())
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch));

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
    .with_agent_profiles(profiles)
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch));

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
#[allow(
    clippy::too_many_lines,
    reason = "keeps setup, readiness, Room launch, and assignment discovery in one ordered scenario"
)]
fn ready_managed_reference_assignment_is_visible_before_its_first_mcp_launch() {
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
    let mut draft = reviewed_draft();
    let analyst = draft
        .seats
        .iter_mut()
        .find(|seat| seat.seat_id == "analyst-1")
        .unwrap_or_else(|| unreachable!("analyst seat"));
    analyst.agent_assignment =
        Some(worldstream_studio_supervisor::room_drafts::AgentAssignmentModeV1::Managed);
    analyst.runner_template = Some(RunnerTemplateRevisionReferenceV1 {
        template_id: "reference-agent-host".to_owned(),
        revision: "r1".to_owned(),
    });
    let setup = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        open_creation_for(directory.path(), &draft, room_response()),
        vault.clone(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_agent_profiles(profiles.clone())
    .with_launch_readiness(
        ConsoleHealth(Arc::new(Mutex::new(
            ParticipantConsoleSessionHealthV1::Usable,
        ))),
        ManagedReferenceRunner,
        Launcher(Arc::new(Mutex::new(LaunchLedger::default()))),
    )
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch));
    let ready = setup
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("ready setup: {error:?}"));
    assert_eq!(ready.state, TaskSetupStateV1::Ready);
    assert!(ready.readiness.ready_to_launch);
    assert_eq!(
        ready.readiness.seats[1].reason,
        TaskSeatReadinessReasonV1::Ready
    );
    let room_started = setup
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("launch managed-reference Room: {error:?}"));
    assert_eq!(
        room_started.launch.as_ref().map(|launch| launch.state),
        Some(TaskLaunchStateV1::Launched)
    );

    let launch_source = FileAssignedMembershipSourceV1::open(
        &directory.path().join("setups"),
        profiles.clone(),
        vault,
    )
    .unwrap_or_else(|error| unreachable!("launch source: {error:?}"));
    let launches = AssignmentMcpLaunchRegistryV1::open(
        &directory.path().join("launches"),
        &directory.path().join("continuity"),
        launch_source,
        "127.0.0.1:39001"
            .parse()
            .unwrap_or_else(|error| unreachable!("daemon address: {error}")),
        Duration::from_secs(1),
    )
    .unwrap_or_else(|error| unreachable!("launch registry: {error:?}"));
    let assignments = PersistedAgentSeatAssignmentSourceV1::new(profiles, launches, setup)
        .assignments()
        .unwrap_or_else(|error| unreachable!("pre-launch assignment inventory: {error:?}"));
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].seat_id, "analyst-1");
    assert!(assignments[0].managed_reference);
    assert_eq!(
        assignments[0].instance_id.as_deref(),
        Some("reference-runner")
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

#[test]
fn hosted_browser_authority_requires_the_exact_ready_human_membership() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let supervisor = open_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    );
    let status = supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("ready setup: {error:?}"));
    let human = &status.seats[0];
    let binding = HostedBrowserHandoffRequestV1 {
        schema: "worldstream/hosted-browser-handoff-request/v1".to_owned(),
        platform_account_id: "10000000-0000-4000-8000-000000000001".to_owned(),
        run_id: "20000000-0000-4000-8000-000000000001".to_owned(),
        listing_revision_digest: format!("blake3:{}", "1".repeat(64)),
        host_installation_id: "hosted-preview-1".to_owned(),
        room_setup_operation_id: status.draft_id,
        room_id: status.room_id.clone(),
        pack: HostedPackReference {
            id: "counter".to_owned(),
            version: "2.0.0".to_owned(),
            digest: DIGEST.to_owned(),
        },
        client_release_digest: format!("blake3:{}", "2".repeat(64)),
        client_surface_id: "participant".to_owned(),
        access_mode: HostedGenesisAccessModeV1::Participant,
        purpose: HostedGenesisMembershipPurposeV1::Participant,
        seat_id: Some(human.seat_id.clone()),
        role: Some(human.role.clone()),
        principal_kind: HostedGenesisPrincipalKindV1::Human,
        principal_id: human
            .principal_id
            .clone()
            .unwrap_or_else(|| unreachable!("human principal")),
        membership_id: human
            .member_id
            .clone()
            .unwrap_or_else(|| unreachable!("human Membership")),
    };
    let authority = supervisor
        .resolve_hosted_browser_membership(&binding)
        .unwrap_or_else(|error| unreachable!("hosted authority: {error:?}"));
    assert_eq!(authority.room_id(), status.room_id);
    assert_eq!(authority.member_id(), binding.membership_id);
    assert_eq!(
        authority.access_mode(),
        worldstream_protocol::AccessMode::Participant
    );
    assert_eq!(authority.role(), Some("navigator"));

    for changed in [
        {
            let mut value = binding.clone();
            value.membership_id = "01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned();
            value
        },
        {
            let mut value = binding.clone();
            value.role = Some("analyst".to_owned());
            value
        },
        {
            let mut value = binding.clone();
            value.room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB2".to_owned();
            value
        },
    ] {
        assert_eq!(
            supervisor
                .resolve_hosted_browser_membership(&changed)
                .map(|_| ()),
            Err(ParticipantHandoffAuthorityErrorV1::AuthorityInvalid)
        );
    }
}

#[derive(Clone)]
struct ConsoleHealth(Arc<Mutex<ParticipantConsoleSessionHealthV1>>);

impl ParticipantConsoleReadinessSourceV1 for ConsoleHealth {
    fn session_health(&self, _room_id: &str, member_id: &str) -> ParticipantConsoleSessionHealthV1 {
        // This fixture models only the human Navigator's browser. The Agent
        // seat uses its separate Runner unless a test explicitly connects it.
        if member_id != "01ARZ3NDEKTSV4RRFFQ69G5FAY" {
            return ParticipantConsoleSessionHealthV1::Missing;
        }
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone)]
struct AgentBrowserHealth(Arc<Mutex<ParticipantConsoleSessionHealthV1>>);

impl ParticipantConsoleReadinessSourceV1 for AgentBrowserHealth {
    fn session_health(&self, _room_id: &str, member_id: &str) -> ParticipantConsoleSessionHealthV1 {
        match member_id {
            "01ARZ3NDEKTSV4RRFFQ69G5FAY" => ParticipantConsoleSessionHealthV1::Usable,
            "01ARZ3NDEKTSV4RRFFQ69G5FAZ" => *self.0.lock().unwrap_or_else(PoisonError::into_inner),
            _ => ParticipantConsoleSessionHealthV1::Missing,
        }
    }
}

#[test]
fn direct_agent_client_requires_a_synchronized_session_without_a_runner() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let agent = Arc::new(Mutex::new(ParticipantConsoleSessionHealthV1::Missing));
    let setup = open_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch))
    .with_launch_readiness(
        AgentBrowserHealth(Arc::clone(&agent)),
        RunnerHealth(Arc::new(Mutex::new(RunnerMode::Missing))),
        Launcher(Arc::new(Mutex::new(LaunchLedger::default()))),
    );
    setup
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    for health in [
        ParticipantConsoleSessionHealthV1::Missing,
        ParticipantConsoleSessionHealthV1::Stale,
        ParticipantConsoleSessionHealthV1::Disconnected,
        ParticipantConsoleSessionHealthV1::Usable,
    ] {
        *agent.lock().unwrap_or_else(PoisonError::into_inner) = health;
        let status = setup
            .status("setup-alpha")
            .unwrap_or_else(|error| unreachable!("status: {error:?}"));
        assert_eq!(
            status.readiness.ready_to_launch,
            health == ParticipantConsoleSessionHealthV1::Usable
        );
        assert_eq!(
            status.readiness.seats[1].reason,
            if health == ParticipantConsoleSessionHealthV1::Usable {
                TaskSeatReadinessReasonV1::Ready
            } else {
                TaskSeatReadinessReasonV1::RunnerMissing
            }
        );
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
    )
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch));
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

#[derive(Clone, Copy)]
struct ManagedReferenceRunner;

impl TaskRunnerReadinessSourceV1 for ManagedReferenceRunner {
    fn presence(&self, _runner_id: &str) -> TaskRunnerObservationV1 {
        TaskRunnerObservationV1::Missing
    }

    fn managed_reference_reason(&self, assignment_id: &str) -> TaskSeatReadinessReasonV1 {
        if assignment_id
            .parse::<worldstream_protocol::UlidString>()
            .is_ok()
        {
            TaskSeatReadinessReasonV1::Ready
        } else {
            TaskSeatReadinessReasonV1::RunnerMissing
        }
    }

    fn select_managed_reference(
        &self,
        pack: &PackReference,
        requested: &RunnerTemplateRevisionReferenceV1,
    ) -> Option<ManagedRunnerAssignmentV1> {
        (pack.id == "counter"
            && pack.version == "2.0.0"
            && pack.digest == DIGEST
            && requested.template_id == "reference-agent-host"
            && requested.revision == "r1")
            .then(|| ManagedRunnerAssignmentV1 {
                instance_id: "reference-runner".to_owned(),
                template_id: requested.template_id.clone(),
                template_revision: requested.revision.clone(),
            })
    }
}

#[derive(Default)]
struct LaunchLedger {
    calls: Vec<LobbyLaunchRequest>,
    response: Option<LobbyLaunchResponse>,
    corrupt_launch_response: Option<LaunchResponseCorruption>,
    lose_first_response: bool,
    hide_committed_head_once: bool,
    reject_resolve: bool,
    reject_launch: bool,
    preserve_resolve_duplicate: bool,
}

#[derive(Clone, Copy)]
enum LaunchResponseCorruption {
    WrongPack,
    InvalidTransition,
    SequenceJump,
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
    fn resolve(
        &self,
        _room_id: &str,
        _request: &LobbyLaunchRequest,
    ) -> Result<Option<LobbyLaunchResponse>, TaskLaunchAttemptErrorV1> {
        let ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if ledger.reject_resolve {
            return Err(TaskLaunchAttemptErrorV1::Ambiguous);
        }
        Ok(ledger.response.clone().map(|mut response| {
            if !ledger.preserve_resolve_duplicate {
                response.duplicate = true;
            }
            response
        }))
    }

    fn launch(
        &self,
        room_id: &str,
        request: &LobbyLaunchRequest,
    ) -> Result<LobbyLaunchResponse, TaskLaunchAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger.calls.push(request.clone());
        if ledger.reject_launch {
            return Err(TaskLaunchAttemptErrorV1::Rejected);
        }
        let mut response = ledger
            .response
            .get_or_insert_with(|| LobbyLaunchResponse {
                room_id: room_id.to_owned(),
                input_id: request.input_id.clone(),
                transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned(),
                room_head: launched_head(),
                duplicate: false,
            })
            .clone();
        match ledger.corrupt_launch_response {
            Some(LaunchResponseCorruption::WrongPack) => {
                response.room_head.pack_digest = format!("blake3:{}", "0".repeat(64));
            }
            Some(LaunchResponseCorruption::InvalidTransition) => {
                response.transition_id = "not-a-transition".to_owned();
            }
            Some(LaunchResponseCorruption::SequenceJump) => {
                response.room_head.room_seq = request.based_on_room_seq.saturating_add(2);
            }
            None => {}
        }
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

#[test]
fn first_receipt_resolution_failure_is_durable_without_claiming_a_launch_attempt() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let launches = Arc::new(Mutex::new(LaunchLedger {
        reject_resolve: true,
        ..LaunchLedger::default()
    }));
    let make = || {
        open_launch_ready_setup(
            directory.path(),
            DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
            ConsoleHealth(Arc::new(Mutex::new(
                ParticipantConsoleSessionHealthV1::Usable,
            ))),
            RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
            Launcher(Arc::clone(&launches)),
        )
    };
    let first = make();
    first
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    let attention = first
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("initial receipt resolution: {error:?}"));
    let launch = attention
        .launch
        .as_ref()
        .unwrap_or_else(|| unreachable!("retained launch intent"));
    assert_eq!(launch.state, TaskLaunchStateV1::NeedsAttention);
    assert_eq!(launch.attempts, 0);
    assert!(
        launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .is_empty()
    );
    drop(first);

    launches
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .reject_resolve = false;
    let recovered = make()
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry retained receipt resolution: {error:?}"));
    let launch = recovered
        .launch
        .as_ref()
        .unwrap_or_else(|| unreachable!("recovered launch"));
    assert_eq!(launch.state, TaskLaunchStateV1::Launched);
    assert_eq!(launch.attempts, 1);
    assert_eq!(
        launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .len(),
        1
    );
}

#[test]
fn launch_request_carries_pack_digest_only_for_generic_activity_start() {
    for (applicability, expected_digest) in [
        (
            TaskLaunchApplicabilityV1::ActivityStart,
            Some(DIGEST.to_owned()),
        ),
        (TaskLaunchApplicabilityV1::LobbyLaunch, None),
    ] {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
        let launches = Arc::new(Mutex::new(LaunchLedger::default()));
        let supervisor = open_setup(
            directory.path(),
            DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
        )
        .with_launch_applicability(LaunchApplicability(applicability))
        .with_launch_readiness(
            ConsoleHealth(Arc::new(Mutex::new(
                ParticipantConsoleSessionHealthV1::Usable,
            ))),
            RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
            Launcher(Arc::clone(&launches)),
        );
        supervisor
            .start("setup-alpha")
            .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
        supervisor
            .launch("setup-alpha")
            .unwrap_or_else(|error| unreachable!("launch: {error:?}"));
        let calls = &launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].pack_digest, expected_digest);
    }
}

fn open_launch_ready_setup(
    root: &std::path::Path,
    provisioner: DurableProvisioner,
    console: ConsoleHealth,
    runners: RunnerHealth,
    launcher: Launcher,
) -> TaskSetupSupervisorV1 {
    open_setup(root, provisioner)
        .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch))
        .with_launch_readiness(console, runners, launcher)
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
        let reason = match reason {
            TaskSeatReadinessReasonV1::ConsoleMissing | TaskSeatReadinessReasonV1::ConsoleStale => {
                TaskSeatReadinessReasonV1::ParticipantUnsynchronized
            }
            _ => TaskSeatReadinessReasonV1::ParticipantUnavailable,
        };
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
fn filled_optional_seat_obeys_the_delivery_phase_launch_policy() {
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
    .with_launch_applicability(LaunchApplicability(TaskLaunchApplicabilityV1::LobbyLaunch))
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
        TaskSeatReadinessReasonV1::ParticipantUnavailable
    );
    assert!(status.readiness.ready_to_launch);
    assert!(supervisor.launch("setup-alpha").is_ok());
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
    let committed = restarted.launch("setup-alpha");
    let committed = committed.unwrap_or_else(|error| unreachable!("retry: {error:?}"));
    assert_eq!(
        committed.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::Launched)
    );
    assert!(!committed.readiness.ready_to_launch);
    let calls = &launches
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .calls;
    assert_eq!(calls.len(), 1);
}

#[test]
fn launch_rejects_receipts_that_are_not_the_exact_successor_for_the_reviewed_pack() {
    for corruption in [
        LaunchResponseCorruption::WrongPack,
        LaunchResponseCorruption::InvalidTransition,
        LaunchResponseCorruption::SequenceJump,
    ] {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
        let launches = Arc::new(Mutex::new(LaunchLedger {
            corrupt_launch_response: Some(corruption),
            ..LaunchLedger::default()
        }));
        let supervisor = open_launch_ready_setup(
            directory.path(),
            DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
            ConsoleHealth(Arc::new(Mutex::new(
                ParticipantConsoleSessionHealthV1::Usable,
            ))),
            RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
            Launcher(Arc::clone(&launches)),
        );
        supervisor
            .start("setup-alpha")
            .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
        let rejected = supervisor
            .launch("setup-alpha")
            .unwrap_or_else(|error| unreachable!("launch: {error:?}"));
        assert_eq!(
            rejected.launch.as_ref().map(|value| value.state),
            Some(TaskLaunchStateV1::NeedsAttention)
        );
        assert_eq!(
            launches
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .calls
                .len(),
            1
        );
    }
}

#[test]
fn receipt_resolution_requires_the_daemon_to_mark_the_result_as_a_duplicate() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let launches = Arc::new(Mutex::new(LaunchLedger {
        lose_first_response: true,
        preserve_resolve_duplicate: true,
        ..LaunchLedger::default()
    }));
    let supervisor = open_launch_ready_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
        ConsoleHealth(Arc::new(Mutex::new(
            ParticipantConsoleSessionHealthV1::Usable,
        ))),
        RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
        Launcher(Arc::clone(&launches)),
    );
    supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    let ambiguous = supervisor
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("first launch: {error:?}"));
    assert_eq!(
        ambiguous.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::NeedsAttention)
    );

    let rejected = supervisor
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("resolve: {error:?}"));
    assert_eq!(
        rejected.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::NeedsAttention)
    );
    assert_eq!(
        launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .len(),
        1
    );
}

#[test]
fn uncommitted_launch_is_not_dispatched_again_while_required_readiness_is_lost() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let provisions = Arc::new(Mutex::new(ProvisionLedger::default()));
    let launches = Arc::new(Mutex::new(LaunchLedger {
        reject_launch: true,
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
    let rejected = first
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("launch: {error:?}"));
    assert_eq!(
        rejected.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::NeedsAttention)
    );
    drop(first);

    *runner_health.lock().unwrap_or_else(PoisonError::into_inner) = RunnerMode::Full;
    let restarted = make();
    let pending = restarted
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("retry: {error:?}"));
    assert_eq!(
        pending.launch.as_ref().map(|value| value.state),
        Some(TaskLaunchStateV1::NeedsAttention)
    );
    assert_eq!(
        launches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .len(),
        1
    );
}

#[test]
fn legacy_lobby_launch_record_stays_fail_closed_until_retry_resolves_its_exact_declaration() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let supervisor = open_launch_ready_setup(
        directory.path(),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
        ConsoleHealth(Arc::new(Mutex::new(
            ParticipantConsoleSessionHealthV1::Usable,
        ))),
        RunnerHealth(Arc::new(Mutex::new(RunnerMode::Ready))),
        Launcher(Arc::new(Mutex::new(LaunchLedger::default()))),
    );
    supervisor
        .start("setup-alpha")
        .unwrap_or_else(|error| unreachable!("setup: {error:?}"));
    supervisor
        .launch("setup-alpha")
        .unwrap_or_else(|error| unreachable!("launch: {error:?}"));

    let path = directory.path().join("setups/setup-alpha.json");
    let mut legacy: serde_json::Value = serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|error| unreachable!("read setup: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("decode setup: {error}"));
    let fields = legacy
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("setup object"));
    assert!(fields.remove("launch_applicability").is_some());
    fields.insert(
        "checkpoint_hash".to_owned(),
        serde_json::Value::String(String::new()),
    );
    let checkpoint = format!(
        "blake3:{}",
        blake3::hash(
            &serde_json::to_vec(&legacy)
                .unwrap_or_else(|error| unreachable!("legacy checkpoint: {error}")),
        )
        .to_hex()
    );
    legacy
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("setup object"))
        .insert(
            "checkpoint_hash".to_owned(),
            serde_json::Value::String(checkpoint),
        );
    fs::write(
        &path,
        serde_json::to_vec_pretty(&legacy)
            .unwrap_or_else(|error| unreachable!("legacy setup: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("write legacy setup: {error}"));

    let status = supervisor
        .status("setup-alpha")
        .unwrap_or_else(|error| unreachable!("legacy status: {error:?}"));
    assert_eq!(
        status.launch_applicability,
        TaskLaunchApplicabilityV1::Unknown
    );
    assert_eq!(
        status.launch.as_ref().map(|launch| launch.state),
        Some(TaskLaunchStateV1::Launched)
    );

    let unavailable = TaskSetupSupervisorV1::open(
        &directory.path().join("setups"),
        open_creation(directory.path()),
        FileSecretVaultV1::open(&directory.path().join("secrets"))
            .unwrap_or_else(|error| unreachable!("vault: {error:?}")),
        DurableProvisioner(Arc::new(Mutex::new(ProvisionLedger::default()))),
    )
    .unwrap_or_else(|error| unreachable!("setup supervisor: {error:?}"))
    .with_launch_applicability(UnavailableLaunchApplicability);
    assert_eq!(
        unavailable.retry("setup-alpha"),
        Err(TaskSetupErrorV1::Unavailable)
    );
    assert_eq!(
        unavailable
            .status("setup-alpha")
            .unwrap_or_else(|error| unreachable!("unknown status: {error:?}"))
            .launch_applicability,
        TaskLaunchApplicabilityV1::Unknown
    );

    let recovered = supervisor
        .retry("setup-alpha")
        .unwrap_or_else(|error| unreachable!("resolve legacy launch declaration: {error:?}"));
    assert_eq!(
        recovered.launch_applicability,
        TaskLaunchApplicabilityV1::LobbyLaunch
    );
    assert_eq!(
        recovered.launch.as_ref().map(|launch| launch.state),
        Some(TaskLaunchStateV1::Launched)
    );
}
