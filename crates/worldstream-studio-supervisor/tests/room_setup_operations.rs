#![cfg(feature = "cli-operator-preview")]

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
    room_creation::{DaemonRoomCreatorV1, RoomCreationAttemptErrorV1, RoomCreationSupervisorV1},
    room_drafts::{ExactActivityPackDraftValidatorV1, RoomDraftStoreV1},
    room_setup_operations::{
        RoomSetupCreateRequestV1, RoomSetupOperationStageV1, RoomSetupOperationsV1,
        room_setup_operations_router,
    },
    runner_templates::RunnerTemplateRegistryV1,
    secrets::FileSecretVaultV1,
    task_setup::{
        CatalogTaskLaunchApplicabilitySourceV1, DaemonTaskSetupProvisionerV1,
        TaskSetupAttemptErrorV1, TaskSetupSupervisorV1,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

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

/// An external daemon boundary ledger. A committed Runner receipt can be lost
/// once; repeated wire intent must address the same retained capability.
#[derive(Default)]
struct DaemonLedger {
    creations: Vec<CreateRoomRequest>,
    members: BTreeMap<String, MemberCapabilityProvisionResponseV1>,
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
