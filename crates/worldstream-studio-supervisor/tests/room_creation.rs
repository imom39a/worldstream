#![allow(dead_code)]

use worldstream_studio_supervisor::{managed_daemon_transport, process_ownership};

mod activity_packs {
    pub use worldstream_studio_supervisor::activity_packs::*;
}

mod secrets {
    pub use worldstream_studio_supervisor::secrets::*;
}

mod room_setup_spec {
    pub use worldstream_studio_supervisor::room_setup_spec::*;
}

#[path = "../src/configuration_safety.rs"]
mod configuration_safety;
#[path = "../src/room_creation.rs"]
mod room_creation;
#[path = "../src/room_drafts.rs"]
mod room_drafts;

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

use room_creation::{
    DaemonRoomCreatorV1, RoomCreationAttemptErrorV1, RoomCreationStateV1, RoomCreationSupervisorV1,
};
use room_drafts::{RoomDraftErrorV1, RoomDraftStoreV1, RoomDraftV1, RoomDraftValidatorV1};
use serde_json::json;
use tempfile::tempdir;
use worldstream_protocol::{CreateRoomRequest, CreateRoomResponse};

const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

#[derive(Clone, Copy)]
struct ValidDraft;

impl RoomDraftValidatorV1 for ValidDraft {
    fn validate(
        &self,
        _draft: &RoomDraftV1,
    ) -> Result<Vec<room_drafts::RoomDraftFieldErrorV1>, RoomDraftErrorV1> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct CurrentDependencies(Arc<AtomicBool>);

impl RoomDraftValidatorV1 for CurrentDependencies {
    fn validate(
        &self,
        _draft: &RoomDraftV1,
    ) -> Result<Vec<room_drafts::RoomDraftFieldErrorV1>, RoomDraftErrorV1> {
        Ok(if self.0.load(Ordering::SeqCst) {
            Vec::new()
        } else {
            vec![room_drafts::RoomDraftFieldErrorV1 {
                path: "/seats/0/runner_template".to_owned(),
                code: "runner_template_missing".to_owned(),
                message: "The exact Runner Template revision is unavailable.".to_owned(),
            }]
        })
    }
}

fn reviewed_draft() -> RoomDraftV1 {
    serde_json::from_value(json!({
        "schema": "worldstream/studio-room-draft/v1",
        "draft_id": "launch-alpha",
        "pack": { "id": "counter", "version": "2.0.0", "digest": DIGEST },
        "configuration": { "initial_value": 0, "maximum_value": 8 },
        "seats": [{
            "seat_id": "counter-1",
            "role": "counter",
            "required": true,
            "display_name": "Counter 1",
            "principal_id": PRINCIPAL,
            "principal_kind": "human"
        }],
        "readiness": [{ "seat_id": "counter-1", "role": "counter", "required": true }],
        "last_valid_step": "review"
    }))
    .unwrap_or_else(|error| unreachable!("reviewed draft: {error}"))
}

fn response() -> CreateRoomResponse {
    serde_json::from_value(json!({
        "room_id": ROOM,
        "member_ids": ["01ARZ3NDEKTSV4RRFFQ69G5FAX"],
        "room_head": {
            "room_id": ROOM,
            "room_seq": 0,
            "genesis_or_transition_hash": format!("blake3:{}", "b".repeat(64)),
            "core_schema_version": "worldstream/core-room-state/v1",
            "pack_digest": DIGEST,
            "core_state_hash": format!("blake3:{}", "c".repeat(64)),
            "activity_state_hash": format!("blake3:{}", "d".repeat(64)),
            "authoritative_state_hash": format!("blake3:{}", "e".repeat(64))
        }
    }))
    .unwrap_or_else(|error| unreachable!("create response: {error}"))
}

fn operator_response() -> CreateRoomResponse {
    let mut response = response();
    response
        .member_ids
        .push("01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned());
    response
}

fn operator_reviewed_draft() -> RoomDraftV1 {
    let mut draft = reviewed_draft();
    draft.operator_view = true;
    draft
}

#[derive(Default)]
struct CreatorState {
    requests: Vec<CreateRoomRequest>,
    committed: Option<CreateRoomResponse>,
    lose_next_response: bool,
    failure: Option<CreatorFailure>,
}

#[derive(Clone, Copy)]
enum CreatorFailure {
    Ambiguous,
    OperatorFixRequired,
    Rejected,
}

#[derive(Clone)]
struct ExactOnceCreator(Arc<Mutex<CreatorState>>);

impl DaemonRoomCreatorV1 for ExactOnceCreator {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.requests.push(request.clone());
        if let Some(failure) = state.failure {
            return Err(match failure {
                CreatorFailure::Ambiguous => RoomCreationAttemptErrorV1::Ambiguous,
                CreatorFailure::OperatorFixRequired => {
                    RoomCreationAttemptErrorV1::OperatorFixRequired
                }
                CreatorFailure::Rejected => RoomCreationAttemptErrorV1::Rejected,
            });
        }
        if let Some(committed) = &state.committed {
            return Ok(committed.clone());
        }
        let committed = response();
        state.committed = Some(committed.clone());
        if state.lose_next_response {
            state.lose_next_response = false;
            Err(RoomCreationAttemptErrorV1::Ambiguous)
        } else {
            Ok(committed)
        }
    }
}

#[derive(Default)]
struct DurableDaemonLedger {
    committed: Option<(CreateRoomRequest, CreateRoomResponse)>,
}

#[derive(Clone)]
struct RestartableDaemonCreator {
    durable: Arc<Mutex<DurableDaemonLedger>>,
    requests: Arc<Mutex<Vec<CreateRoomRequest>>>,
    lose_response_after_commit: bool,
}

#[derive(Clone)]
struct OperatorCreator(Arc<Mutex<CreatorState>>);

impl DaemonRoomCreatorV1 for OperatorCreator {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.requests.push(request.clone());
        if let Some(committed) = &state.committed {
            return Ok(committed.clone());
        }
        let committed = operator_response();
        state.committed = Some(committed.clone());
        if state.lose_next_response {
            state.lose_next_response = false;
            Err(RoomCreationAttemptErrorV1::Ambiguous)
        } else {
            Ok(committed)
        }
    }
}

impl DaemonRoomCreatorV1 for RestartableDaemonCreator {
    fn create(
        &self,
        request: &CreateRoomRequest,
    ) -> Result<CreateRoomResponse, RoomCreationAttemptErrorV1> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        let mut durable = self.durable.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((original_request, original_response)) = &durable.committed {
            return if original_request == request {
                Ok(original_response.clone())
            } else {
                Err(RoomCreationAttemptErrorV1::Rejected)
            };
        }
        let committed = response();
        durable.committed = Some((request.clone(), committed.clone()));
        if self.lose_response_after_commit {
            Err(RoomCreationAttemptErrorV1::Ambiguous)
        } else {
            Ok(committed)
        }
    }
}

#[test]
fn repaired_authorization_retries_only_the_original_immutable_operation() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState {
        failure: Some(CreatorFailure::OperatorFixRequired),
        ..CreatorState::default()
    }));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));

    let needs_fix = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    assert_eq!(needs_fix.state, RoomCreationStateV1::NeedsAttention);
    assert_eq!(
        needs_fix
            .attention
            .as_ref()
            .map(|attention| attention.retryable),
        Some(true)
    );
    state.lock().unwrap_or_else(PoisonError::into_inner).failure = None;

    let succeeded = supervisor
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("retry after repair: {error:?}"));
    assert_eq!(succeeded.state, RoomCreationStateV1::Succeeded);
    assert_eq!(succeeded.operation_id, needs_fix.operation_id);
    assert_eq!(succeeded.idempotency_key, needs_fix.idempotency_key);
    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(state.requests.len(), 2);
    assert_eq!(state.requests[0], state.requests[1]);
}

#[test]
fn permanent_rejection_cannot_be_retried_or_replaced() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState {
        failure: Some(CreatorFailure::Rejected),
        ..CreatorState::default()
    }));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));

    let rejected = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    assert_eq!(rejected.state, RoomCreationStateV1::NeedsAttention);
    assert_eq!(
        rejected
            .attention
            .as_ref()
            .map(|attention| attention.retryable),
        Some(false)
    );
    let repeated = supervisor
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("terminal status: {error:?}"));
    assert_eq!(repeated, rejected);
    let statuses = supervisor
        .statuses()
        .unwrap_or_else(|error| unreachable!("creation statuses: {error:?}"));
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].draft_id, "launch-alpha");
    assert_eq!(statuses[0].state, RoomCreationStateV1::NeedsAttention);
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .len(),
        1
    );
}

#[test]
fn corrupt_retryability_cannot_turn_a_permanent_rejection_into_a_retry() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState {
        failure: Some(CreatorFailure::Rejected),
        ..CreatorState::default()
    }));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("rejected start: {error:?}"));
    let path = directory.path().join("operations/launch-alpha.json");
    let mut record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| unreachable!("read: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("parse: {error}"));
    record["attention"]["retryable"] = json!(true);
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&record)
            .unwrap_or_else(|error| unreachable!("serialize: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("write: {error}"));

    assert_eq!(
        supervisor.reconcile("launch-alpha"),
        Err(room_creation::RoomCreationErrorV1::Unavailable)
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .len(),
        1
    );
}

fn setup(root: &std::path::Path, creator: impl DaemonRoomCreatorV1) -> RoomCreationSupervisorV1 {
    setup_with_draft(root, creator, &reviewed_draft())
}

fn setup_with_draft(
    root: &std::path::Path,
    creator: impl DaemonRoomCreatorV1,
    draft: &RoomDraftV1,
) -> RoomCreationSupervisorV1 {
    let drafts = RoomDraftStoreV1::open(&root.join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(draft)
        .unwrap_or_else(|error| unreachable!("save reviewed draft: {error:?}"));
    RoomCreationSupervisorV1::open(&root.join("operations"), drafts, creator)
        .unwrap_or_else(|error| unreachable!("creation supervisor: {error:?}"))
}

#[test]
fn reviewed_operator_membership_is_genesis_bound_and_ambiguous_retry_reuses_exact_request() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState {
        lose_next_response: true,
        ..CreatorState::default()
    }));
    let supervisor = setup_with_draft(
        directory.path(),
        OperatorCreator(Arc::clone(&state)),
        &operator_reviewed_draft(),
    );

    let first = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("initial creation: {error:?}"));
    assert_eq!(first.state, RoomCreationStateV1::Retrying);
    let completed = supervisor
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("retry creation: {error:?}"));
    assert_eq!(completed.state, RoomCreationStateV1::Succeeded);
    let principal_id = {
        let state = state.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(state.requests.len(), 2);
        assert_eq!(state.requests[0], state.requests[1]);
        let operator = state.requests[0]
            .members
            .last()
            .unwrap_or_else(|| unreachable!("operator genesis member"));
        assert_eq!(
            operator.access_mode,
            worldstream_protocol::AccessMode::Operator
        );
        assert_eq!(
            operator.principal_kind,
            worldstream_protocol::PrincipalKind::Human
        );
        assert_eq!(operator.role, None);
        operator.principal_id.clone()
    };
    let binding = supervisor
        .reviewed_operator_membership(ROOM)
        .unwrap_or_else(|error| unreachable!("operator binding: {error:?}"))
        .unwrap_or_else(|| unreachable!("reviewed binding"));
    assert_eq!(binding.principal_id, principal_id);
    assert_eq!(binding.member_id, "01ARZ3NDEKTSV4RRFFQ69G5FAY");
}

#[test]
fn false_operator_view_preserves_legacy_single_member_creation_request() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("legacy creation: {error:?}"));
    let request = state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .requests[0]
        .clone();
    assert_eq!(request.members.len(), 1);
    assert_eq!(
        request.members[0].access_mode,
        worldstream_protocol::AccessMode::Participant
    );
    assert!(
        supervisor
            .reviewed_operator_membership(ROOM)
            .unwrap_or_else(|error| unreachable!("legacy binding: {error:?}"))
            .is_none()
    );
}

#[test]
fn dependency_disappearance_after_review_blocks_room_creation_before_daemon_call() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let available = Arc::new(AtomicBool::new(true));
    let drafts = RoomDraftStoreV1::open(
        &directory.path().join("drafts"),
        CurrentDependencies(Arc::clone(&available)),
    )
    .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save reviewed draft: {error:?}"));
    available.store(false, Ordering::SeqCst);
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = RoomCreationSupervisorV1::open(
        &directory.path().join("operations"),
        drafts,
        ExactOnceCreator(Arc::clone(&state)),
    )
    .unwrap_or_else(|error| unreachable!("creation supervisor: {error:?}"));

    assert_eq!(
        supervisor.start("launch-alpha"),
        Err(room_creation::RoomCreationErrorV1::InvalidDraft)
    );
    assert!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .is_empty()
    );
}

#[test]
fn crash_after_initial_intent_publication_resumes_the_same_durable_operation() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    supervisor.fail_after_intent_once_for_test();

    assert_eq!(
        supervisor.start("launch-alpha"),
        Err(room_creation::RoomCreationErrorV1::Unavailable)
    );
    assert!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .is_empty()
    );
    let published = supervisor
        .status("launch-alpha")
        .unwrap_or_else(|error| unreachable!("published status: {error:?}"));
    drop(supervisor);

    let reopened = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    let succeeded = reopened
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("restart reconcile: {error:?}"));
    assert_eq!(succeeded.operation_id, published.operation_id);
    assert_eq!(succeeded.idempotency_key, published.idempotency_key);
    assert_eq!(succeeded.room_id.as_deref(), Some(ROOM));
}

#[test]
fn lost_create_response_reissues_identical_intent_and_converges_to_original_room() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState {
        lose_next_response: true,
        ..CreatorState::default()
    }));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));

    let waiting = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    assert_eq!(waiting.state, RoomCreationStateV1::Retrying);
    assert!(waiting.room_id.is_none());
    let succeeded = supervisor
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("reconcile: {error:?}"));
    assert_eq!(succeeded.state, RoomCreationStateV1::Succeeded);
    assert_eq!(succeeded.room_id.as_deref(), Some(ROOM));

    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(state.requests.len(), 2);
    assert_eq!(state.requests[0], state.requests[1]);
    assert_eq!(state.requests[0].idempotency_key, succeeded.idempotency_key);
}

#[test]
fn duplicate_submission_permanently_returns_the_one_bound_room() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));

    let first = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("first start: {error:?}"));
    let duplicate = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("duplicate start: {error:?}"));

    assert_eq!(first, duplicate);
    assert_eq!(duplicate.room_id.as_deref(), Some(ROOM));
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .len(),
        1
    );
}

#[test]
fn supervisor_restart_loads_the_same_intent_and_reconciles_it() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState {
        failure: Some(CreatorFailure::Ambiguous),
        ..CreatorState::default()
    }));
    let first = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    let retrying = first
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("start: {error:?}"));
    drop(first);
    state.lock().unwrap_or_else(PoisonError::into_inner).failure = None;

    let reopened = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    let succeeded = reopened
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("restart reconcile: {error:?}"));

    assert_eq!(succeeded.state, RoomCreationStateV1::Succeeded);
    assert_eq!(succeeded.operation_id, retrying.operation_id);
    assert_eq!(succeeded.idempotency_key, retrying.idempotency_key);
}

#[test]
fn daemon_restart_after_committed_lost_response_returns_the_original_durable_room() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let durable = Arc::new(Mutex::new(DurableDaemonLedger::default()));
    let before_requests = Arc::new(Mutex::new(Vec::new()));
    let before = setup(
        directory.path(),
        RestartableDaemonCreator {
            durable: Arc::clone(&durable),
            requests: Arc::clone(&before_requests),
            lose_response_after_commit: true,
        },
    );
    let retrying = before
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("pre-restart start: {error:?}"));
    assert_eq!(retrying.state, RoomCreationStateV1::Retrying);
    let original = before_requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)[0]
        .clone();
    drop(before);

    let after_requests = Arc::new(Mutex::new(Vec::new()));
    let after = setup(
        directory.path(),
        RestartableDaemonCreator {
            durable: Arc::clone(&durable),
            requests: Arc::clone(&after_requests),
            lose_response_after_commit: false,
        },
    );
    let succeeded = after
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("post-restart reconcile: {error:?}"));
    let replayed = after_requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)[0]
        .clone();

    assert_eq!(original, replayed);
    assert_eq!(succeeded.idempotency_key, retrying.idempotency_key);
    assert_eq!(succeeded.room_id.as_deref(), Some(ROOM));
    assert_eq!(succeeded.response.as_ref(), Some(&response()));
    assert_eq!(
        durable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .committed
            .as_ref()
            .map(|(_, response)| response.room_id.as_str()),
        Some(ROOM)
    );
}

#[test]
fn crash_after_reply_before_local_receipt_reconciles_without_second_room() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    supervisor.fail_before_receipt_once_for_test();
    let retrying = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("start with receipt crash: {error:?}"));
    assert_eq!(retrying.state, RoomCreationStateV1::Retrying);
    drop(supervisor);

    let reopened = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    let succeeded = reopened
        .reconcile("launch-alpha")
        .unwrap_or_else(|error| unreachable!("receipt reconciliation: {error:?}"));

    assert_eq!(succeeded.room_id.as_deref(), Some(ROOM));
    let state = state.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(state.requests[0], state.requests[1]);
    assert_eq!(
        state
            .committed
            .as_ref()
            .map(|result| result.room_id.as_str()),
        Some(ROOM)
    );
}

#[test]
fn corrupt_bound_record_fails_closed_without_minting_a_replacement() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("initial start: {error:?}"));

    let path = directory.path().join("operations/launch-alpha.json");
    let mut record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| unreachable!("read record: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("parse record: {error}"));
    record["review"]["seats"][0]["display_name"] = json!("substituted");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&record)
            .unwrap_or_else(|error| unreachable!("serialize record: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("write record: {error}"));

    assert_eq!(
        supervisor.start("launch-alpha"),
        Err(room_creation::RoomCreationErrorV1::Unavailable)
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .len(),
        1
    );
}

#[test]
fn tampered_terminal_room_or_member_receipt_fails_closed() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let state = Arc::new(Mutex::new(CreatorState::default()));
    let supervisor = setup(directory.path(), ExactOnceCreator(Arc::clone(&state)));
    let succeeded = supervisor
        .start("launch-alpha")
        .unwrap_or_else(|error| unreachable!("initial start: {error:?}"));
    assert_eq!(succeeded.state, RoomCreationStateV1::Succeeded);

    let path = directory.path().join("operations/launch-alpha.json");
    let mut record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| unreachable!("read record: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("parse record: {error}"));
    assert!(record["response_hash"].as_str().is_some());
    record["response"]["member_ids"][0] = json!("01ARZ3NDEKTSV4RRFFQ69G5FAY");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&record)
            .unwrap_or_else(|error| unreachable!("serialize record: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("write record: {error}"));

    assert_eq!(
        supervisor.status("launch-alpha"),
        Err(room_creation::RoomCreationErrorV1::Unavailable)
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .len(),
        1
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one contiguous transport capture proves authority separation and retry identity"
)]
fn production_http_creator_reuses_one_sealed_spectator_bearer_on_retry() {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use worldstream_protocol::{
        AccessMode, BearerWireV1, CreateMember, PackReference, PrincipalKind,
    };
    use worldstream_studio_supervisor::{
        room_creation::{
            DaemonRoomCreatorV1 as LiveDaemonRoomCreatorV1, HostedSpectatorCredentialIntentV2,
            HttpDaemonRoomCreatorV1, RetainedHostedRoomCreationIntentV2,
        },
        room_setup_spec::SetupSpectatorPurposeV2,
        secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1},
    };

    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| unreachable!("bind test daemon: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("test daemon address: {error}"));
    let response_body = serde_json::to_vec(&json!({
        "schema": "worldstream/hosted-room-creation-response/v2",
        "room": {
            "room_id": ROOM,
            "member_ids": [
                "01ARZ3NDEKTSV4RRFFQ69G5FAX",
                "01ARZ3NDEKTSV4RRFFQ69G5FB0"
            ],
            "room_head": {
                "room_id": ROOM,
                "room_seq": 0,
                "genesis_or_transition_hash": format!("blake3:{}", "b".repeat(64)),
                "core_schema_version": "worldstream/core-room-state/v1",
                "pack_digest": DIGEST,
                "core_state_hash": format!("blake3:{}", "c".repeat(64)),
                "activity_state_hash": format!("blake3:{}", "d".repeat(64)),
                "authoritative_state_hash": format!("blake3:{}", "e".repeat(64))
            }
        },
        "spectators": [{
            "purpose": "result_indexer",
            "room_id": ROOM,
            "member_id": "01ARZ3NDEKTSV4RRFFQ69G5FB0",
            "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FB1",
            "capability_id": "01ARZ3NDEKTSV4RRFFQ69G5FB2",
            "scopes": ["room:attach", "room:observe_public", "room:replay"]
        }]
    }))
    .unwrap_or_else(|error| unreachable!("response JSON: {error}"));
    let server = std::thread::spawn(move || {
        let mut captured = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener
                .accept()
                .unwrap_or_else(|error| unreachable!("accept request: {error}"));
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap_or_else(|error| unreachable!("request timeout: {error}"));
            let mut reader = BufReader::new(
                stream
                    .try_clone()
                    .unwrap_or_else(|error| unreachable!("clone request stream: {error}")),
            );
            let mut request_line = String::new();
            reader
                .read_line(&mut request_line)
                .unwrap_or_else(|error| unreachable!("request line: {error}"));
            assert_eq!(request_line, "POST /v1/operator/hosted-rooms HTTP/1.1\r\n");
            let mut content_length = None;
            let mut authorization = None;
            loop {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .unwrap_or_else(|error| unreachable!("request header: {error}"));
                if line == "\r\n" {
                    break;
                }
                let (name, value) = line
                    .trim_end()
                    .split_once(':')
                    .unwrap_or_else(|| unreachable!("shaped request header"));
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .unwrap_or_else(|_| unreachable!("content length")),
                    );
                }
                if name.eq_ignore_ascii_case("authorization") {
                    authorization = Some(value.trim().to_owned());
                }
            }
            let mut body = vec![0_u8; content_length.unwrap_or_else(|| unreachable!("body size"))];
            reader
                .read_exact(&mut body)
                .unwrap_or_else(|error| unreachable!("request body: {error}"));
            let body: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| unreachable!("request JSON: {error}"));
            captured.push((
                authorization.unwrap_or_else(|| unreachable!("host authority")),
                body,
            ));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .and_then(|()| stream.write_all(&response_body))
            .unwrap_or_else(|error| unreachable!("write response: {error}"));
        }
        captured
    });

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary vault: {error}"));
    let vault = FileSecretVaultV1::open(&directory.path().join("vault"))
        .unwrap_or_else(|error| unreachable!("open vault: {error}"));
    let host_bytes = [0xa9; 32];
    let host_reference = vault
        .store(SecretKindV1::HostAuthority, &host_bytes)
        .unwrap_or_else(|error| unreachable!("store host authority: {error}"));
    let membership_reference = SecretReferenceV1::parse("1".repeat(64))
        .unwrap_or_else(|error| unreachable!("membership reference: {error}"));
    let creator = HttpDaemonRoomCreatorV1::new(
        address,
        std::time::Duration::from_secs(5),
        vault.clone(),
        Some(host_reference),
    );
    let request = RetainedHostedRoomCreationIntentV2 {
        schema: "worldstream/hosted-room-creation/v2".to_owned(),
        room: CreateRoomRequest {
            pack: PackReference {
                id: "worldstream.counter".to_owned(),
                version: "2.0.0".to_owned(),
                digest: DIGEST.to_owned(),
            },
            configuration: json!({"initial_value": 0, "maximum_value": 8}),
            members: vec![
                CreateMember {
                    principal_id: PRINCIPAL.to_owned(),
                    principal_kind: PrincipalKind::Human,
                    role: Some("counter".to_owned()),
                    access_mode: AccessMode::Participant,
                },
                CreateMember {
                    principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned(),
                    principal_kind: PrincipalKind::Agent,
                    role: None,
                    access_mode: AccessMode::Spectator,
                },
            ],
            idempotency_key: "hosted-create-operation".to_owned(),
        },
        spectators: vec![HostedSpectatorCredentialIntentV2 {
            purpose: SetupSpectatorPurposeV2::ResultIndexer,
            member_index: 1,
            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned(),
            principal_kind: PrincipalKind::Agent,
            capability_id: "01ARZ3NDEKTSV4RRFFQ69G5FB2".to_owned(),
            capability_idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FB3".to_owned(),
            secret_reference: membership_reference.clone(),
        }],
    };

    let first = LiveDaemonRoomCreatorV1::create_hosted(&creator, &request)
        .unwrap_or_else(|error| unreachable!("first hosted request: {error:?}"));
    let duplicate = LiveDaemonRoomCreatorV1::create_hosted(&creator, &request)
        .unwrap_or_else(|error| unreachable!("retried hosted request: {error:?}"));
    assert_eq!(duplicate, first);
    let captured = server
        .join()
        .unwrap_or_else(|_| unreachable!("test daemon join"));
    assert_eq!(captured.len(), 2);
    assert!(
        captured[0] == captured[1],
        "retry must reuse the exact sealed request"
    );
    let host_wire = BearerWireV1::from_bytes(host_bytes).to_wire();
    assert!(
        captured[0].0 == format!("Bearer {host_wire}"),
        "request must use only the retained Host authority"
    );
    let body = &captured[0].1;
    assert!(body["spectators"][0].get("scopes").is_none());
    let spectator_wire = body["spectators"][0]["capability"]["bearer"]
        .as_str()
        .unwrap_or_else(|| unreachable!("sealed spectator bearer"));
    assert!(
        spectator_wire != host_wire,
        "spectator authority must differ from Host authority"
    );
    let retained = vault
        .resolve(SecretKindV1::MembershipAuthority, &membership_reference)
        .unwrap_or_else(|error| unreachable!("retained membership authority: {error}"));
    assert!(
        BearerWireV1::parse(spectator_wire)
            .unwrap_or_else(|error| unreachable!("spectator bearer: {error}"))
            .into_bytes()
            .as_slice()
            == retained.as_bytes(),
        "sealed request must use the retained Membership authority"
    );
    assert!(
        !serde_json::to_string(&first)
            .unwrap_or_else(|error| unreachable!("receipt JSON: {error}"))
            .contains("bearer")
    );
}
