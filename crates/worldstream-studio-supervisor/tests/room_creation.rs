#![allow(dead_code)]

mod activity_packs {
    pub use worldstream_studio_supervisor::activity_packs::*;
}

mod secrets {
    pub use worldstream_studio_supervisor::secrets::*;
}

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
