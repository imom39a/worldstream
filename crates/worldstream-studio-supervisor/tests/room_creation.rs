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

use std::sync::{Arc, Mutex, PoisonError};

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
    let drafts = RoomDraftStoreV1::open(&root.join("drafts"), ValidDraft)
        .unwrap_or_else(|error| unreachable!("draft store: {error:?}"));
    drafts
        .save(&reviewed_draft())
        .unwrap_or_else(|error| unreachable!("save reviewed draft: {error:?}"));
    RoomCreationSupervisorV1::open(&root.join("operations"), drafts, creator)
        .unwrap_or_else(|error| unreachable!("creation supervisor: {error:?}"))
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
