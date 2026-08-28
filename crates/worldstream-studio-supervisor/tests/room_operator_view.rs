use std::sync::{Arc, Mutex, PoisonError};

use serde_json::json;
use tempfile::tempdir;
use worldstream_protocol::{
    AccessMode, CreateRoomRequest, CreateRoomResponse, MemberCapabilityProvisionRequestV1,
    MemberCapabilityProvisionResponseV1, Projection, ProjectionResponse, RoomHead,
    RunnerCapabilityProvisionRequestV1, RunnerCapabilityProvisionResponseV1,
    SealedCapabilityBearerV1,
};
use worldstream_studio_supervisor::{
    room_creation::{DaemonRoomCreatorV1, RoomCreationAttemptErrorV1, RoomCreationSupervisorV1},
    room_drafts::{RoomDraftErrorV1, RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1},
    room_operator_view::{
        DaemonRoomOperatorProjectionV1, EnableRoomOperatorViewRequestV1,
        RoomOperatorProjectionErrorV1, RoomOperatorViewStateV1, RoomOperatorViewSupervisorV1,
    },
    secrets::FileSecretVaultV1,
    task_setup::{DaemonTaskSetupProvisionerV1, TaskSetupAttemptErrorV1},
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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

fn reviewed_operator_draft() -> RoomDraftV1 {
    serde_json::from_value(json!({
        "schema": "worldstream/studio-room-draft/v1",
        "draft_id": "operator-counter",
        "pack": { "id": "counter", "version": "2.0.0", "digest": DIGEST },
        "configuration": { "initial_value": 0, "maximum_value": 16 },
        "seats": [{
            "seat_id": "counter-1", "role": "counter", "required": true,
            "display_name": "Counter", "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "principal_kind": "human"
        }],
        "readiness": [{ "seat_id": "counter-1", "role": "counter", "required": true }],
        "operator_view": true,
        "last_valid_step": "review"
    }))
    .unwrap_or_else(|error| unreachable!("draft: {error}"))
}

fn creation_response() -> CreateRoomResponse {
    serde_json::from_value(json!({
        "room_id": ROOM,
        "member_ids": ["01ARZ3NDEKTSV4RRFFQ69G5FAX", "01ARZ3NDEKTSV4RRFFQ69G5FAY"],
        "room_head": {
            "room_id": ROOM, "room_seq": 0,
            "genesis_or_transition_hash": format!("blake3:{}", "b".repeat(64)),
            "core_schema_version": "worldstream/core-room-state/v1", "pack_digest": DIGEST,
            "core_state_hash": format!("blake3:{}", "c".repeat(64)),
            "activity_state_hash": format!("blake3:{}", "d".repeat(64)),
            "authoritative_state_hash": format!("blake3:{}", "e".repeat(64))
        }
    }))
    .unwrap_or_else(|error| unreachable!("creation response: {error}"))
}

#[derive(Clone, Copy)]
struct Creator;

impl DaemonRoomCreatorV1 for Creator {
    fn create(
        &self,
        _request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        Ok(creation_response())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Call {
    room_id: String,
    member_id: String,
    principal_id: String,
    capability_id: String,
    change_id: String,
    bearer_digest: String,
    role: Option<String>,
    access_mode: AccessMode,
    scopes: Vec<String>,
}

#[derive(Default)]
struct Ledger {
    calls: Vec<Call>,
    lost_once: bool,
}

#[derive(Clone)]
struct LostReceiptProvisioner(Arc<Mutex<Ledger>>);

impl DaemonTaskSetupProvisionerV1 for LostReceiptProvisioner {
    fn provision_member(
        &self,
        request: &MemberCapabilityProvisionRequestV1,
    ) -> Result<MemberCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        let mut ledger = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let call = Call {
            room_id: request.room_id.clone(),
            member_id: request.member_id.clone(),
            principal_id: request.principal_id.clone(),
            capability_id: request.capability.capability_id.clone(),
            change_id: request.capability.capability_idempotency_key.clone(),
            bearer_digest: blake3::hash(request.capability.bearer.as_str().as_bytes())
                .to_hex()
                .to_string(),
            role: request.role.clone(),
            access_mode: request.access_mode,
            scopes: request.scopes.clone(),
        };
        ledger.calls.push(call);
        let receipt = MemberCapabilityProvisionResponseV1 {
            capability_id: request.capability.capability_id.clone(),
            room_id: request.room_id.clone(),
            member_id: request.member_id.clone(),
            principal_id: request.principal_id.clone(),
            scopes: request.scopes.clone(),
        };
        if ledger.lost_once {
            Ok(receipt)
        } else {
            ledger.lost_once = true;
            Err(TaskSetupAttemptErrorV1::Ambiguous)
        }
    }

    fn provision_runner(
        &self,
        _request: &RunnerCapabilityProvisionRequestV1,
    ) -> Result<RunnerCapabilityProvisionResponseV1, TaskSetupAttemptErrorV1> {
        unreachable!("operator views cannot provision Runner authority")
    }
}

#[derive(Clone, Copy)]
struct CounterProjection;

impl DaemonRoomOperatorProjectionV1 for CounterProjection {
    fn current_projection(
        &self,
        room_id: &str,
        _bearer: &SealedCapabilityBearerV1,
    ) -> Result<ProjectionResponse, RoomOperatorProjectionErrorV1> {
        Ok(ProjectionResponse {
            room_id: room_id.to_owned(),
            room_head: RoomHead {
                room_id: room_id.to_owned(),
                room_seq: 4,
                genesis_or_transition_hash: "h".to_owned(),
                core_schema_version: "core.v1".to_owned(),
                pack_digest: DIGEST.to_owned(),
                core_state_hash: "c".to_owned(),
                activity_state_hash: "a".to_owned(),
                authoritative_state_hash: "s".to_owned(),
            },
            room_health: "healthy".to_owned(),
            integrity_generation: 1,
            projection_schema: "counter/public-projection/v1".to_owned(),
            projection: Projection {
                core: json!({}),
                activity: json!({"value": 4}),
                action_offers: Vec::new(),
            },
            projection_hash: "hash".to_owned(),
        })
    }
}

fn creation(root: &std::path::Path) -> RoomCreationSupervisorV1 {
    let drafts = RoomDraftStoreV1::open(&root.join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("drafts: {error:?}"));
    drafts
        .save(&reviewed_operator_draft())
        .unwrap_or_else(|error| unreachable!("save: {error:?}"));
    let creation = RoomCreationSupervisorV1::open(&root.join("creations"), drafts, Creator)
        .unwrap_or_else(|error| unreachable!("creation: {error:?}"));
    creation
        .start("operator-counter")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    creation
}

fn request() -> EnableRoomOperatorViewRequestV1 {
    EnableRoomOperatorViewRequestV1 {
        schema: "worldstream/studio-room-operator-view-enable/v1".to_owned(),
    }
}

#[test]
fn operator_capability_retry_and_restart_reuse_one_exact_read_only_authority() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let creation = creation(directory.path());
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let first = RoomOperatorViewSupervisorV1::open(
        &directory.path().join("operator-views"),
        vault.clone(),
        creation.clone(),
        LostReceiptProvisioner(Arc::clone(&ledger)),
        CounterProjection,
    )
    .unwrap_or_else(|error| unreachable!("first supervisor: {error:?}"));
    let unavailable = first
        .enable(ROOM, &request())
        .unwrap_or_else(|error| unreachable!("first enable: {error:?}"));
    assert_eq!(unavailable.state, RoomOperatorViewStateV1::Unavailable);
    drop(first);

    let restarted = RoomOperatorViewSupervisorV1::open(
        &directory.path().join("operator-views"),
        vault,
        creation,
        LostReceiptProvisioner(Arc::clone(&ledger)),
        CounterProjection,
    )
    .unwrap_or_else(|error| unreachable!("restarted supervisor: {error:?}"));
    let available = restarted
        .enable(ROOM, &request())
        .unwrap_or_else(|error| unreachable!("retry: {error:?}"));
    assert_eq!(available.state, RoomOperatorViewStateV1::Available);
    assert_eq!(
        available.counter.as_ref().map(|counter| counter.value),
        Some(4)
    );
    let ledger = ledger.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(ledger.calls.len(), 2);
    assert_eq!(ledger.calls[0], ledger.calls[1]);
    assert_eq!(ledger.calls[0].room_id, ROOM);
    assert_eq!(ledger.calls[0].member_id, "01ARZ3NDEKTSV4RRFFQ69G5FAY");
    assert_eq!(ledger.calls[0].role, None);
    assert_eq!(ledger.calls[0].access_mode, AccessMode::Operator);
    assert_eq!(ledger.calls[0].scopes, ["room:observe_member"]);
}

#[test]
fn missing_retained_operator_secret_fails_closed_without_replacing_authority() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let ledger = Arc::new(Mutex::new(Ledger {
        lost_once: true,
        ..Ledger::default()
    }));
    let supervisor = RoomOperatorViewSupervisorV1::open(
        &directory.path().join("operator-views"),
        vault,
        creation(directory.path()),
        LostReceiptProvisioner(Arc::clone(&ledger)),
        CounterProjection,
    )
    .unwrap_or_else(|error| unreachable!("supervisor: {error:?}"));
    supervisor
        .enable(ROOM, &request())
        .unwrap_or_else(|error| unreachable!("enable: {error:?}"));
    let secret = std::fs::read_dir(directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("secrets: {error}"))
        .find_map(Result::ok)
        .map_or_else(|| unreachable!("operator secret"), |entry| entry.path());
    std::fs::remove_file(secret).unwrap_or_else(|error| unreachable!("remove secret: {error}"));
    assert!(matches!(
        supervisor.view(ROOM),
        Err(
            worldstream_studio_supervisor::room_operator_view::RoomOperatorViewErrorV1::Unavailable
        )
    ));
    assert_eq!(
        ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .len(),
        1
    );
}

#[test]
fn corrupt_published_operator_intent_never_mints_replacement_authority() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("secrets"))
        .unwrap_or_else(|error| unreachable!("vault: {error:?}"));
    let ledger = Arc::new(Mutex::new(Ledger {
        lost_once: true,
        ..Ledger::default()
    }));
    let supervisor = RoomOperatorViewSupervisorV1::open(
        &directory.path().join("operator-views"),
        vault,
        creation(directory.path()),
        LostReceiptProvisioner(Arc::clone(&ledger)),
        CounterProjection,
    )
    .unwrap_or_else(|error| unreachable!("supervisor: {error:?}"));
    supervisor
        .enable(ROOM, &request())
        .unwrap_or_else(|error| unreachable!("enable: {error:?}"));
    std::fs::write(
        directory
            .path()
            .join("operator-views")
            .join(format!("{ROOM}.json")),
        b"corrupt",
    )
    .unwrap_or_else(|error| unreachable!("corrupt intent: {error}"));
    assert!(matches!(
        supervisor.enable(ROOM, &request()),
        Err(
            worldstream_studio_supervisor::room_operator_view::RoomOperatorViewErrorV1::Unavailable
        )
    ));
    assert_eq!(
        ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
            .len(),
        1
    );
}
