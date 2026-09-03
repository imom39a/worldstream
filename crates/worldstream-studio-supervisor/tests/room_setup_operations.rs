use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use worldstream_core::{agent_heist_lobby_digest, builtin_agent_heist_registry};
use worldstream_protocol::{
    ActivityPackCatalogResponse, ActivityPackCatalogRevisionResponse, CreateRoomRequest,
    CreateRoomResponse, MemberCapabilityProvisionRequestV1, MemberCapabilityProvisionResponseV1,
    RunnerCapabilityProvisionRequestV1, RunnerCapabilityProvisionResponseV1,
};
use worldstream_studio_supervisor::{
    activity_packs::{ActivityPackProxyErrorV1, DaemonActivityPackSource},
    agent_profiles::AgentProfileStoreV1,
    client_bindings::ClientBindingStoreV1,
    control_access::ControlAccess,
    control_admission::protect_operator_routes,
    participant_handoff::{
        CurrentMembershipSnapshotV1, HumanSeatAuthorityV1, ParticipantActionRequestV1,
        ParticipantConsoleGatewayErrorV1, ParticipantConsoleGatewayV1,
        ParticipantConsoleObservationV1, ParticipantHandoffBrokerV1,
        operator_client_handoff_router, participant_handoff_router,
    },
    room_creation::{DaemonRoomCreatorV1, RoomCreationAttemptErrorV1, RoomCreationSupervisorV1},
    room_drafts::{ExactActivityPackDraftValidatorV1, RoomDraftStoreV1},
    room_setup_operations::{
        RoomSetupCreateRequestV1, RoomSetupOperationStageV1, RoomSetupOperationsV1,
        room_setup_operations_router,
    },
    room_setup_spec::resolve_setup_specification,
    runner_templates::RunnerTemplateRegistryV1,
    scoped_connections::{MembershipCredentialsV1, RunnerCredentialsV1, scoped_credentials_router},
    secrets::FileSecretVaultV1,
    task_setup::{
        CatalogTaskLaunchApplicabilitySourceV1, DaemonTaskSetupProvisionerV1,
        TaskSetupAttemptErrorV1, TaskSetupSupervisorV1,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const NEGOTIATE_ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";

#[derive(Clone)]
struct Catalog(Arc<ActivityPackCatalogRevisionResponse>);

impl DaemonActivityPackSource for Catalog {
    fn catalog(&self) -> Result<ActivityPackCatalogResponse, ActivityPackProxyErrorV1> {
        Err(ActivityPackProxyErrorV1::InvalidResponse)
    }

    fn revision(
        &self,
        digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, ActivityPackProxyErrorV1> {
        if digest == self.0.revision.summary.pack.digest {
            Ok((*self.0).clone())
        } else {
            Err(ActivityPackProxyErrorV1::RevisionUnavailable)
        }
    }
}

fn catalog() -> TestResult<Catalog> {
    let registry = builtin_agent_heist_registry()?;
    let digest = agent_heist_lobby_digest();
    let revision = registry.catalog_revision(&digest)?;
    let reference = &revision.descriptor.configuration_schema;
    let schema: Value =
        serde_json::from_slice(&registry.resolve_schema(&digest, reference)?.to_bytes()?)?;
    let configuration_schema = json!({"schema_id": reference.schema_id,
        "schema_digest": reference.schema_digest.to_string(), "schema": schema});
    Ok(Catalog(Arc::new(serde_json::from_value(json!({
        "version":"activity_pack_catalog.v1", "revision": {
            "summary": {"pack":{"id":"worldstream.agent-heist","version":"0.2.0","digest":digest.to_string()},
                "name":"Agent Heist","selectable_for_new_rooms":true,"runnable_for_retained_rooms":true},
            "roles": revision.descriptor.roles.iter().map(|r| json!({"role":r.role,"minimum":r.minimum,"maximum":r.maximum})).collect::<Vec<_>>(),
            "configuration_schema":configuration_schema, "actions":[],
            "lobby_compatibility":{"contract":"worldstream/lobby/v1","configuration_schema":configuration_schema}
        }
    }))?)))
}

fn negotiate_catalog() -> TestResult<Catalog> {
    use worldstream_core::Blake3DigestV1;
    use worldstream_pack_bundle::PackBundleVerifierV1;

    let bytes = include_bytes!(
        "../../../packs/negotiate/releases/0.2.0/worldstream-negotiate-83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8.wspack"
    );
    let bundle = PackBundleVerifierV1.inspect(Arc::from(bytes.as_slice()))?;
    let descriptor = bundle.descriptor();
    let reference = &descriptor.configuration_schema;
    assert_eq!(
        reference.schema_digest,
        Blake3DigestV1::hash(br#"{"type":"object"}"#)
    );
    Ok(Catalog(Arc::new(serde_json::from_value(json!({
        "version": "activity_pack_catalog.v1",
        "revision": {
            "summary": {
                "pack": {
                    "id": descriptor.pack_id,
                    "version": descriptor.explanatory_version,
                    "digest": bundle.revision_digest().to_string()
                },
                "name": descriptor.name,
                "selectable_for_new_rooms": true,
                "runnable_for_retained_rooms": true
            },
            "roles": descriptor.roles.iter().map(|role| json!({
                "role": role.role,
                "minimum": role.minimum,
                "maximum": role.maximum
            })).collect::<Vec<_>>(),
            "configuration_schema": {
                "schema_id": reference.schema_id,
                "schema_digest": reference.schema_digest.to_string(),
                "schema": {"type": "object"}
            },
            "actions": []
        }
    }))?)))
}

#[derive(Clone)]
struct Daemon;

impl DaemonRoomCreatorV1 for Daemon {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        if request.members.len() != 2 {
            return Err(RoomCreationAttemptErrorV1::Rejected);
        }
        serde_json::from_value(json!({"room_id":ROOM,
            "member_ids":["01ARZ3NDEKTSV4RRFFQ69G5FAY","01ARZ3NDEKTSV4RRFFQ69G5FAZ"],
            "room_head":{"room_id":ROOM,"room_seq":0,"genesis_or_transition_hash":format!("blake3:{}","b".repeat(64)),
                "core_schema_version":"worldstream/core-room-state/v1","pack_digest":request.pack.digest,
                "core_state_hash":format!("blake3:{}","c".repeat(64)),"activity_state_hash":format!("blake3:{}","d".repeat(64)),
                "authoritative_state_hash":format!("blake3:{}","e".repeat(64))}}))
            .map_err(|_| RoomCreationAttemptErrorV1::Rejected)
    }
}

impl DaemonTaskSetupProvisionerV1 for Daemon {
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        Ok(MemberCapabilityProvisionResponseV1 {
            capability_id: request.capability.capability_id.clone(),
            room_id: request.room_id.clone(),
            member_id: request.member_id.clone(),
            principal_id: request.principal_id.clone(),
            scopes: request.scopes.clone(),
        })
    }

    fn provision_runner(
        &self,
        request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        Ok(RunnerCapabilityProvisionResponseV1 {
            capability_id: request.capability.capability_id.clone(),
            runner_id: request.runner_id.clone(),
            owner_principal_id: request.owner_principal_id.clone(),
            permitted_memberships: request.permitted_memberships.clone(),
            scopes: request.scopes.clone(),
        })
    }
}

#[derive(Clone)]
struct NegotiateDaemon;

impl DaemonRoomCreatorV1 for NegotiateDaemon {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        if request.members.len() != 4 {
            return Err(RoomCreationAttemptErrorV1::Rejected);
        }
        serde_json::from_value(json!({
            "room_id": NEGOTIATE_ROOM,
            "member_ids": [
                "01ARZ3NDEKTSV4RRFFQ69G5FB1",
                "01ARZ3NDEKTSV4RRFFQ69G5FB2",
                "01ARZ3NDEKTSV4RRFFQ69G5FB3",
                "01ARZ3NDEKTSV4RRFFQ69G5FB4"
            ],
            "room_head": {
                "room_id": NEGOTIATE_ROOM,
                "room_seq": 0,
                "genesis_or_transition_hash": format!("blake3:{}", "b".repeat(64)),
                "core_schema_version": "worldstream/core-room-state/v1",
                "pack_digest": request.pack.digest,
                "core_state_hash": format!("blake3:{}", "c".repeat(64)),
                "activity_state_hash": format!("blake3:{}", "d".repeat(64)),
                "authoritative_state_hash": format!("blake3:{}", "e".repeat(64))
            }
        }))
        .map_err(|_| RoomCreationAttemptErrorV1::Rejected)
    }
}

impl DaemonTaskSetupProvisionerV1 for NegotiateDaemon {
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        Daemon.provision_member(request)
    }

    fn provision_runner(
        &self,
        request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        Daemon.provision_runner(request)
    }
}

#[tokio::test]
async fn reviewed_heist_setup_creates_provisions_and_exposes_one_public_operation() -> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = catalog()?;
    let drafts = RoomDraftStoreV1::open(
        &temp.path().join("drafts"),
        ExactActivityPackDraftValidatorV1::new(catalog.clone()),
    )?;
    let creation = RoomCreationSupervisorV1::open(&temp.path().join("creation"), drafts, Daemon)?;
    let vault = FileSecretVaultV1::open(&temp.path().join("vault"))?;
    let profiles = AgentProfileStoreV1::open(&temp.path().join("profiles"), vault.clone())?;
    let runners = RunnerTemplateRegistryV1::open_installed(&temp.path().join("templates"))?;
    let setup =
        TaskSetupSupervisorV1::open(&temp.path().join("setup"), creation.clone(), vault, Daemon)?
            .with_launch_applicability(CatalogTaskLaunchApplicabilitySourceV1::new(
                catalog.clone(),
            ));
    let app = room_setup_operations_router(RoomSetupOperationsV1::new(
        creation.clone(),
        setup.clone(),
        catalog,
        profiles,
        runners,
    ));
    let specification: Value = serde_json::from_str(include_str!(
        "../../../examples/room-setup/agent-heist-0.2.0.json"
    ))?;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/room-setup-operations/cli-heist-first")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(
                    &json!({"specification":specification,"acknowledge_start":false}),
                )?))?,
        )
        .await?;
    assert_eq!(response.status(), 200);
    let receipt: Value = serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(receipt["operation"], "cli-heist-first");
    assert_eq!(receipt["room_id"], ROOM);
    assert_eq!(receipt["complete"], true);
    assert_eq!(receipt["stage"], "complete");
    let persisted = creation.status("cli-heist-first")?;
    assert_eq!(persisted.request.members.len(), 2);
    assert!(!persisted.review.operator_view);
    let provisioned = setup.status("cli-heist-first")?;
    assert_eq!(provisioned.seats.len(), 3);
    assert!(provisioned.seats[2].member_id.is_none());
    let status = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/room-setup-operations/cli-heist-first")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(status.status(), 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&status.into_body().collect().await?.to_bytes())?,
        receipt
    );
    Ok(())
}

#[tokio::test]
async fn reviewed_negotiate_setup_validates_and_creates_with_domain_identifiers_but_not_secrets()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let catalog = negotiate_catalog()?;
    let source = include_bytes!("../../../examples/room-setup/negotiate-0.2.0.json");
    let validated = resolve_setup_specification(source, catalog.0.as_ref())?;
    assert_eq!(
        validated.configuration["transaction_id"],
        "txn_calibration_worldstream_01"
    );
    assert_eq!(
        validated.configuration["a202_revision"],
        "fa85aa8b49bfe7b3f7ded487c98500a600e92e41"
    );

    let mut secret_bearing: Value = serde_json::from_slice(source)?;
    secret_bearing["configuration"]["api_key"] = json!("sk-THIS_MUST_NEVER_ENTER_A_RETAINED_DRAFT");
    assert!(
        resolve_setup_specification(&serde_json::to_vec(&secret_bearing)?, catalog.0.as_ref())
            .is_err()
    );

    let drafts = RoomDraftStoreV1::open(
        &temp.path().join("drafts"),
        ExactActivityPackDraftValidatorV1::new(catalog.clone()),
    )?;
    let creation =
        RoomCreationSupervisorV1::open(&temp.path().join("creation"), drafts, NegotiateDaemon)?;
    let vault = FileSecretVaultV1::open(&temp.path().join("vault"))?;
    let profiles = AgentProfileStoreV1::open(&temp.path().join("profiles"), vault.clone())?;
    let runners = RunnerTemplateRegistryV1::open_installed(&temp.path().join("templates"))?;
    let setup = TaskSetupSupervisorV1::open(
        &temp.path().join("setup"),
        creation.clone(),
        vault,
        NegotiateDaemon,
    )?
    .with_launch_applicability(CatalogTaskLaunchApplicabilitySourceV1::new(catalog.clone()));
    let operations =
        RoomSetupOperationsV1::new(creation.clone(), setup, catalog, profiles, runners);
    let result = operations.create(
        "cli-negotiate-first",
        &RoomSetupCreateRequestV1 {
            specification: serde_json::from_slice(source)?,
            acknowledge_start: true,
        },
    )?;

    assert!(result.complete);
    assert_eq!(result.room_id.as_deref(), Some(NEGOTIATE_ROOM));
    assert_eq!(result.stage, RoomSetupOperationStageV1::Complete);
    let retained = creation.status("cli-negotiate-first")?;
    assert_eq!(
        retained.review.configuration["transaction_id"],
        "txn_calibration_worldstream_01"
    );
    assert_eq!(retained.request.members.len(), 4);
    Ok(())
}

/// An external daemon boundary ledger. A committed Runner receipt can be lost
/// once; repeated wire intent must address the same retained capability.
#[derive(Default)]
struct DaemonLedger {
    creations: Vec<CreateRoomRequest>,
    members: BTreeMap<String, MemberCapabilityProvisionResponseV1>,
    member_requests: Vec<Value>,
    runners: BTreeMap<String, RunnerCapabilityProvisionResponseV1>,
    runner_requests: Vec<Value>,
    lose_runner_reply: bool,
}

#[derive(Clone, Default)]
struct RetainedDaemon(Arc<Mutex<DaemonLedger>>);

impl DaemonRoomCreatorV1 for RetainedDaemon {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .creations
            .push(request.clone());
        Daemon.create(request)
    }
}

impl DaemonTaskSetupProvisionerV1 for RetainedDaemon {
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger
            .member_requests
            .push(serde_json::to_value(request).map_err(|_| TaskSetupAttemptErrorV1::Rejected)?);
        if let Some(receipt) = ledger.members.get(&request.capability.capability_id) {
            return Ok(receipt.clone());
        }
        let receipt = Daemon.provision_member(request)?;
        ledger
            .members
            .insert(receipt.capability_id.clone(), receipt.clone());
        Ok(receipt)
    }

    fn provision_runner(
        &self,
        request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ledger
            .runner_requests
            .push(serde_json::to_value(request).map_err(|_| TaskSetupAttemptErrorV1::Rejected)?);
        if let Some(receipt) = ledger.runners.get(&request.capability.capability_id) {
            return Ok(receipt.clone());
        }
        let receipt = Daemon.provision_runner(request)?;
        ledger
            .runners
            .insert(receipt.capability_id.clone(), receipt.clone());
        if ledger.lose_runner_reply {
            ledger.lose_runner_reply = false;
            Err(TaskSetupAttemptErrorV1::Ambiguous)
        } else {
            Ok(receipt)
        }
    }
}

fn open_retained_operations(
    root: &Path,
    daemon: &RetainedDaemon,
) -> TestResult<(
    RoomSetupOperationsV1,
    RoomCreationSupervisorV1,
    TaskSetupSupervisorV1,
)> {
    let catalog = catalog()?;
    let drafts = RoomDraftStoreV1::open(
        &root.join("drafts"),
        ExactActivityPackDraftValidatorV1::new(catalog.clone()),
    )?;
    let creation = RoomCreationSupervisorV1::open(&root.join("creation"), drafts, daemon.clone())?;
    let vault = FileSecretVaultV1::open(&root.join("vault"))?;
    let profiles = AgentProfileStoreV1::open(&root.join("profiles"), vault.clone())?;
    let runners = RunnerTemplateRegistryV1::open_installed(&root.join("templates"))?;
    let setup =
        TaskSetupSupervisorV1::open(&root.join("setup"), creation.clone(), vault, daemon.clone())?
            .with_launch_applicability(CatalogTaskLaunchApplicabilitySourceV1::new(
                catalog.clone(),
            ));
    let operations =
        RoomSetupOperationsV1::new(creation.clone(), setup.clone(), catalog, profiles, runners);
    Ok((operations, creation, setup))
}

fn heist_request() -> TestResult<RoomSetupCreateRequestV1> {
    Ok(RoomSetupCreateRequestV1 {
        specification: serde_json::from_str(include_str!(
            "../../../examples/room-setup/agent-heist-0.2.0.json"
        ))?,
        acknowledge_start: false,
    })
}

#[tokio::test]
async fn external_room_runner_is_inspectable_but_never_managed() -> TestResult {
    use worldstream_studio_supervisor::{
        runner_templates::RunnerSupervisorV1,
        scoped_runners::{
            RoomRunnerControlV1, RoomRunnerExecutionV1, RoomRunnerStatusV1, room_runner_router,
        },
    };
    let temp = tempfile::tempdir()?;
    let daemon = RetainedDaemon::default();
    let (operations, _, setup) = open_retained_operations(temp.path(), &daemon)?;
    assert!(
        operations
            .create("external-runner", &heist_request()?)?
            .complete
    );
    let vault = FileSecretVaultV1::open(&temp.path().join("vault"))?;
    let profiles = AgentProfileStoreV1::open(&temp.path().join("profiles"), vault.clone())?;
    let installed = temp.path().join("templates");
    let runners = RunnerSupervisorV1::open(
        RunnerTemplateRegistryV1::open_installed(&installed)?,
        &temp.path().join("runner-runtime"),
        vault,
        std::time::Duration::from_secs(1),
    )?;
    let router = room_runner_router(RoomRunnerControlV1::new(
        setup,
        profiles,
        runners.clone(),
        installed,
    ));
    let endpoint = "/api/v1/room-setup-operations/external-runner/seats/insider/runner";
    let response = router
        .clone()
        .oneshot(Request::builder().uri(endpoint).body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), 200);
    let status: RoomRunnerStatusV1 =
        serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(status.room_id, ROOM);
    assert!(status.runner_id.is_some());
    assert!(matches!(
        status.execution,
        RoomRunnerExecutionV1::External {}
    ));
    for action in ["start", "stop"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{endpoint}/{action}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), 409);
    }
    assert!(runners.statuses().instances.is_empty());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/room-runners?operation=external-runner")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), 200);
    let value: Value = serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(value["runners"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn lost_runner_reply_reopens_and_resumes_original_room_and_authority() -> TestResult {
    let temp = tempfile::tempdir()?;
    let daemon = RetainedDaemon::default();
    daemon
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .lose_runner_reply = true;
    let (operations, creation, setup) = open_retained_operations(temp.path(), &daemon)?;
    let source = temp.path().join("setup-input.json");
    std::fs::write(
        &source,
        serde_json::to_vec(&heist_request()?.specification)?,
    )?;
    let partial = operations.create(
        "cli-lost-reply",
        &RoomSetupCreateRequestV1 {
            specification: serde_json::from_slice(&std::fs::read(&source)?)?,
            acknowledge_start: false,
        },
    )?;
    assert_eq!(partial.room_id.as_deref(), Some(ROOM));
    assert!(!partial.complete);
    assert_eq!(partial.stage, RoomSetupOperationStageV1::Runner);
    assert_eq!(partial.next_action, "resume");
    assert_eq!(operations.unfinished()?.operations.len(), 1);
    let original_creation = creation.status("cli-lost-reply")?;
    let original_setup = setup.status("cli-lost-reply")?;
    drop((operations, creation, setup));

    // Resume has no file parameter: an edited source cannot substitute its intent.
    std::fs::write(&source, b"{\"schema\":\"edited-invalid-source\"}")?;
    let (reopened, creation, setup) = open_retained_operations(temp.path(), &daemon)?;
    let complete = reopened.resume("cli-lost-reply")?;
    assert!(complete.complete);
    assert_eq!(complete.room_id.as_deref(), Some(ROOM));
    assert_eq!(complete.next_action, "inspect_room");
    let retained_creation = creation.status("cli-lost-reply")?;
    assert_eq!(
        retained_creation.operation_id,
        original_creation.operation_id
    );
    assert_eq!(retained_creation.request, original_creation.request);
    let retained_setup = setup.status("cli-lost-reply")?;
    assert_eq!(retained_setup.operation_id, original_setup.operation_id);
    for (before, after) in original_setup.seats.iter().zip(&retained_setup.seats) {
        assert_eq!(before.principal_id, after.principal_id);
        assert_eq!(before.member_id, after.member_id);
    }
    assert!(reopened.unfinished()?.operations.is_empty());
    assert!(reopened.resume("cli-lost-reply")?.complete);
    let ledger = daemon.0.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.creations.len(), 1);
    assert_eq!(ledger.members.len(), 2);
    assert_eq!(ledger.runners.len(), 1);
    assert_eq!(ledger.runner_requests.len(), 2);
    // Requests include a test-only bearer. Boolean equality cannot print it.
    let authority_unchanged = ledger.runner_requests[0] == ledger.runner_requests[1];
    assert!(authority_unchanged);
    assert_eq!(
        ledger.runner_requests[0]["scopes"],
        json!([
            "activation:offer_receive",
            "activation:claim",
            "activation:complete"
        ])
    );
    assert_eq!(
        ledger.runner_requests[0]["permitted_memberships"],
        json!([
            {"room_id":ROOM,"member_id":"01ARZ3NDEKTSV4RRFFQ69G5FAZ"}
        ])
    );
    Ok(())
}

#[test]
fn missing_claimed_setup_requires_restoration_without_replacing_authority() -> TestResult {
    let temp = tempfile::tempdir()?;
    let daemon = RetainedDaemon::default();
    let (operations, creation, setup) = open_retained_operations(temp.path(), &daemon)?;
    assert!(
        operations
            .create("cli-missing-setup", &heist_request()?)?
            .complete
    );
    let original_creation = creation.status("cli-missing-setup")?;
    let original_setup = setup.status("cli-missing-setup")?;
    drop((operations, creation, setup));

    // Owned fixture simulates loss; retain the exact bytes for safe recovery.
    let record = temp.path().join("setup/cli-missing-setup.json");
    let saved = temp.path().join("saved-setup-record.json");
    std::fs::rename(&record, &saved)?;
    let (reopened, creation, _) = open_retained_operations(temp.path(), &daemon)?;
    let partial = reopened.resume("cli-missing-setup")?;
    assert!(!partial.complete);
    assert_eq!(partial.room_id.as_deref(), Some(ROOM));
    assert_eq!(partial.next_action, "restore_setup_record");
    assert!(!record.exists());
    assert_eq!(creation.status("cli-missing-setup")?, original_creation);
    {
        let ledger = daemon.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(ledger.creations.len(), 1);
        assert_eq!(ledger.members.len(), 2);
        assert_eq!(ledger.runners.len(), 1);
        assert_eq!(ledger.runner_requests.len(), 1);
    }
    std::fs::rename(&saved, &record)?;
    assert!(reopened.resume("cli-missing-setup")?.complete);
    let (_, _, restored_setup) = open_retained_operations(temp.path(), &daemon)?;
    assert_eq!(
        restored_setup.status("cli-missing-setup")?.operation_id,
        original_setup.operation_id
    );
    Ok(())
}

#[tokio::test]
async fn explicit_seat_exports_deliver_separate_membership_and_runner_authority() -> TestResult {
    let temp = tempfile::tempdir()?;
    let daemon = RetainedDaemon::default();
    let (operations, _, setup) = open_retained_operations(temp.path(), &daemon)?;
    assert!(operations.create("cli-export", &heist_request()?)?.complete);
    let router = scoped_credentials_router(setup, "127.0.0.1:9300".parse()?);
    let membership_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(
                    "/api/v1/room-setup-operations/cli-export/seats/insider/membership-credentials",
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(membership_response.status(), 200);
    assert_eq!(
        membership_response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let membership: MembershipCredentialsV1 =
        serde_json::from_slice(&membership_response.into_body().collect().await?.to_bytes())?;
    assert_eq!(membership.schema, "worldstream/membership-credentials/v1");
    assert_eq!(membership.runtime_url, "ws://127.0.0.1:9300/v1/stream");
    assert_eq!(membership.room_id, ROOM);
    assert_eq!(membership.member_id, "01ARZ3NDEKTSV4RRFFQ69G5FAZ");
    assert_eq!(membership.role, "insider");
    assert_eq!(
        membership.scopes,
        vec!["room:attach", "room:act", "room:observe_member"]
    );
    let runner_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/room-setup-operations/cli-export/seats/insider/runner-credentials")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(runner_response.status(), 200);
    let runner: RunnerCredentialsV1 =
        serde_json::from_slice(&runner_response.into_body().collect().await?.to_bytes())?;
    assert_eq!(runner.schema, "worldstream/runner-credentials/v1");
    assert_eq!(runner.runtime_url, "ws://127.0.0.1:9300/v1/runner/stream");
    assert_eq!(runner.owner_principal_id, membership.principal_id);
    assert_eq!(runner.permitted_memberships.len(), 1);
    assert_eq!(
        runner.permitted_memberships[0].member_id,
        membership.member_id
    );
    assert_eq!(
        runner.scopes,
        vec![
            "activation:offer_receive",
            "activation:claim",
            "activation:complete"
        ]
    );
    let authorities_are_distinct = runner.bearer.as_str() != membership.bearer.as_str();
    assert!(
        authorities_are_distinct,
        "authority kinds must remain separate"
    );
    {
        let ledger = daemon.0.lock().unwrap_or_else(PoisonError::into_inner);
        let runner_matches_provisioned = ledger.runner_requests[0]["capability"]["bearer"].as_str()
            == Some(runner.bearer.as_str());
        assert!(
            runner_matches_provisioned,
            "export must match provisioned Runner authority"
        );
        let member_wire = ledger
            .member_requests
            .iter()
            .find(|request| request["member_id"] == membership.member_id)
            .ok_or("fixture member request")?;
        let member_matches_provisioned =
            member_wire["capability"]["bearer"].as_str() == Some(membership.bearer.as_str());
        assert!(
            member_matches_provisioned,
            "export must match provisioned Membership authority"
        );
    }
    let human_runner = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/room-setup-operations/cli-export/seats/navigator/runner-credentials")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(human_runner.status(), 409);
    Ok(())
}

struct MembershipGateway;

async fn browser_request(
    router: &axum::Router,
    method: &str,
    path: &str,
    origin: &str,
    cookie: Option<&str>,
    payload: Value,
) -> TestResult<axum::response::Response> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", origin)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    Ok(router
        .clone()
        .oneshot(request.body(Body::from(serde_json::to_vec(&payload)?))?)
        .await?)
}

async fn browser_observation_nonce(router: &axum::Router, cookie: &str) -> TestResult<String> {
    let response = browser_request(
        router,
        "POST",
        "/api/v1/participant-console/session:observe",
        "http://127.0.0.1:5173",
        Some(cookie),
        json!({"after_frame_seq":null}),
    )
    .await?;
    assert_eq!(response.status(), 200);
    assert!(
        response
            .headers()
            .get("access-control-expose-headers")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("X-WorldStream-Delivery-Acknowledgement"))
    );
    Ok(response
        .headers()
        .get("x-worldstream-delivery-acknowledgement")
        .ok_or("delivery acknowledgement missing")?
        .to_str()?
        .to_owned())
}

struct BrowserReceiptFixture {
    _temp: tempfile::TempDir,
    readiness: worldstream_studio_supervisor::participant_handoff::BrowserDeliveryReadinessV1,
    router: axum::Router,
    cookie: String,
}

async fn browser_receipt_fixture() -> TestResult<BrowserReceiptFixture> {
    let temp = tempfile::tempdir()?;
    let (operations, _, setup) = open_retained_operations(temp.path(), &RetainedDaemon::default())?;
    assert!(
        operations
            .create("browser-receipt", &heist_request()?)?
            .complete
    );
    let configuration = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/activity-clients");
    let clients = ClientBindingStoreV1::open_configured(
        &temp.path().join("clients"),
        &configuration.join("releases"),
        &configuration.join("local-bindings.json"),
    )?;
    let broker = ParticipantHandoffBrokerV1::new(
        "http://127.0.0.1:5174",
        "http://127.0.0.1:5173",
        std::time::Duration::from_secs(30),
        16,
        setup,
        MembershipGateway,
        clients,
    )
    .map_err(|_| "broker fixture")?;
    let readiness = broker.browser_readiness();
    let control = ControlAccess::initialize(&temp.path().join("control"))?;
    let authorization = control.authorization_header()?;
    let router = protect_operator_routes(
        operator_client_handoff_router(broker.clone()).merge(participant_handoff_router(broker)),
        control,
    );
    let issued = router
        .clone()
        .oneshot(
            Request::post(
                "/api/v1/room-setup-operations/browser-receipt/seats/navigator/client-handoff",
            )
            .header("authorization", authorization)
            .body(Body::empty())?,
        )
        .await?;
    assert_eq!(issued.status(), 201);
    let transfer: Value = serde_json::from_slice(&issued.into_body().collect().await?.to_bytes())?;
    let token = transfer["client_url"]
        .as_str()
        .and_then(|url| url.split_once("#handoff="))
        .map(|(_, token)| token)
        .ok_or("handoff missing")?;
    let redeemed = router
        .clone()
        .oneshot(
            Request::post("/api/v1/participant-console/handoffs:redeem")
                .header("origin", "http://127.0.0.1:5173")
                .header("x-worldstream-participant-handoff", token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(redeemed.status(), 200);
    let cookie = redeemed
        .headers()
        .get("set-cookie")
        .ok_or("cookie missing")?
        .to_str()?
        .split(';')
        .next()
        .ok_or("cookie invalid")?
        .to_owned();
    Ok(BrowserReceiptFixture {
        _temp: temp,
        readiness,
        router,
        cookie,
    })
}

#[tokio::test]
async fn browser_readiness_requires_acknowledgement_of_delivered_frame() -> TestResult {
    use worldstream_studio_supervisor::participant_handoff::{
        ParticipantConsoleReadinessSourceV1, ParticipantConsoleSessionHealthV1,
    };
    let BrowserReceiptFixture {
        _temp,
        readiness,
        router,
        cookie,
    } = browser_receipt_fixture().await?;
    let member = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
    assert_ne!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    let acknowledgement = browser_observation_nonce(&router, &cookie).await?;
    assert_ne!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    let path = "/api/v1/participant-console/session:acknowledge";
    let origin = "http://127.0.0.1:5173";
    for (request_origin, request_cookie, frame_head, expected) in [
        (origin, None, 7, 401),
        ("http://untrusted.invalid", Some(cookie.as_str()), 7, 403),
        (origin, Some(cookie.as_str()), 8, 400),
    ] {
        let response = browser_request(
            &router,
            "POST",
            path,
            request_origin,
            request_cookie,
            json!({"acknowledgement":acknowledgement,"frame_head":frame_head}),
        )
        .await?;
        assert_eq!(response.status(), expected);
        assert_ne!(
            readiness.session_health(ROOM, member),
            ParticipantConsoleSessionHealthV1::Usable
        );
    }
    // An incorrect frame consumes the pending nonce; only a fresh delivery can succeed.
    let acknowledgement = browser_observation_nonce(&router, &cookie).await?;
    let payload = json!({"acknowledgement":acknowledgement,"frame_head":7});
    for expected in [200, 400] {
        let response = browser_request(
            &router,
            "POST",
            path,
            origin,
            Some(&cookie),
            payload.clone(),
        )
        .await?;
        assert_eq!(response.status(), expected);
    }
    assert_eq!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    tokio::time::sleep(std::time::Duration::from_millis(5100)).await;
    assert_ne!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    let resumed = browser_request(
        &router,
        "GET",
        "/api/v1/participant-console/session",
        origin,
        Some(&cookie),
        Value::Null,
    )
    .await?;
    assert_eq!(resumed.status(), 200);
    assert_ne!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    let acknowledgement = browser_observation_nonce(&router, &cookie).await?;
    assert_ne!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    let acknowledged = browser_request(
        &router,
        "POST",
        path,
        origin,
        Some(&cookie),
        json!({"acknowledgement":acknowledgement,"frame_head":7}),
    )
    .await?;
    assert_eq!(acknowledged.status(), 200);
    assert_eq!(
        readiness.session_health(ROOM, member),
        ParticipantConsoleSessionHealthV1::Usable
    );
    Ok(())
}

impl ParticipantConsoleGatewayV1 for MembershipGateway {
    fn current_membership(
        &self,
        authority: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<CurrentMembershipSnapshotV1, ParticipantConsoleGatewayErrorV1> {
        Ok(CurrentMembershipSnapshotV1 {
            pack: authority.pack().clone(),
            access_mode: authority.access_mode(),
            role: authority.role().map(str::to_owned),
        })
    }
    fn observe(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
    ) -> Result<ParticipantConsoleObservationV1, ParticipantConsoleGatewayErrorV1> {
        Ok(ParticipantConsoleObservationV1 {
            browser_value: json!({"projection":{"phase":"Lobby"},"frame_head":7}),
            durable_cursor: None,
        })
    }
    fn act(
        &self,
        _: &HumanSeatAuthorityV1,
        _: Option<u64>,
        _: &ParticipantActionRequestV1,
    ) -> Result<Value, ParticipantConsoleGatewayErrorV1> {
        Err(ParticipantConsoleGatewayErrorV1::Unavailable)
    }
}

#[tokio::test]
async fn operator_handoff_requires_control_not_studio_origin_and_keeps_one_use_redemption()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let (operations, _, setup) = open_retained_operations(temp.path(), &RetainedDaemon::default())?;
    assert!(
        operations
            .create("cli-client-open", &heist_request()?)?
            .complete
    );
    let configuration = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/activity-clients");
    let clients = ClientBindingStoreV1::open_configured(
        &temp.path().join("clients"),
        &configuration.join("releases"),
        &configuration.join("local-bindings.json"),
    )?;
    let broker = ParticipantHandoffBrokerV1::new(
        "http://127.0.0.1:5174",
        "http://127.0.0.1:5173",
        std::time::Duration::from_secs(30),
        16,
        setup,
        MembershipGateway,
        clients,
    )
    .map_err(|_| "fixture handoff configuration")?;
    let control = ControlAccess::initialize(&temp.path().join("controller"))?;
    let authorization = control.authorization_header()?;
    let router = protect_operator_routes(
        operator_client_handoff_router(broker.clone()).merge(participant_handoff_router(broker)),
        control,
    );
    let path = "/api/v1/room-setup-operations/cli-client-open/seats/navigator/client-handoff";
    let unauthorized = router
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))?,
        )
        .await?;
    assert_eq!(unauthorized.status(), 401);
    // No Origin header is forged. The installation credential is sufficient
    // only for issuance; it is never part of the resulting browser transfer.
    let response = router
        .clone()
        .oneshot(
            Request::post(path)
                .header("authorization", authorization)
                .header("content-type", "application/json")
                .body(Body::from("{}"))?,
        )
        .await?;
    assert_eq!(response.status(), 201);
    let transfer: Value =
        serde_json::from_slice(&response.into_body().collect().await?.to_bytes())?;
    assert_eq!(transfer["state"], "ready");
    let launch = transfer["client_url"]
        .as_str()
        .ok_or("missing handoff transfer")?;
    assert!(launch.starts_with("http://127.0.0.1:5173/agent-heist/#handoff=wsh1:"));
    assert!(!launch.contains("wsb1:"));
    let (_, token) = launch
        .split_once("#handoff=")
        .ok_or("missing one-use handoff")?;
    let redeem = "/api/v1/participant-console/handoffs:redeem";
    let forbidden = router
        .clone()
        .oneshot(
            Request::post(redeem)
                .header("origin", "http://untrusted.invalid")
                .header("x-worldstream-participant-handoff", token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(forbidden.status(), 403);
    let redeemed = router
        .clone()
        .oneshot(
            Request::post(redeem)
                .header("origin", "http://127.0.0.1:5173")
                .header("x-worldstream-participant-handoff", token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(redeemed.status(), 200);
    let reused = router
        .oneshot(
            Request::post(redeem)
                .header("origin", "http://127.0.0.1:5173")
                .header("x-worldstream-participant-handoff", token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(reused.status(), 401);
    Ok(())
}
