#[path = "../src/assignment_mcp_operations.rs"]
mod assignment_mcp_operations;

use assignment_mcp_operations::{
    AssignmentMcpOperationErrorV1, AssignmentMcpOperationIdentityV1,
    AssignmentMcpOperationIntentV1, AssignmentMcpOperationKindV1, AssignmentMcpOperationLedgerV1,
    AssignmentMcpOperationPhaseV1, AssignmentMcpRemoteAcceptanceV1, AssignmentMcpRemoteOutcomeV1,
    FileAssignmentMcpOperationLedgerV1,
};
use tempfile::TempDir;

const ASSIGNMENT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OPERATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const REMOTE_REQUEST_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const ACTION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const ACTIVATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const CLAIM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const COMPLETION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";

fn identity() -> AssignmentMcpOperationIdentityV1 {
    AssignmentMcpOperationIdentityV1::new(ASSIGNMENT_ID, OPERATION_ID)
        .unwrap_or_else(|error| unreachable!("fixture identity should be valid: {error:?}"))
}

fn action_intent(payload: &str) -> AssignmentMcpOperationIntentV1 {
    AssignmentMcpOperationIntentV1::new_action(
        identity(),
        ACTION_ID,
        REMOTE_REQUEST_ID,
        format!(
            "{{\"action_id\":\"{ACTION_ID}\",\"assignment_id\":\"{ASSIGNMENT_ID}\",\"head\":{{\"generation\":4,\"room_seq\":9}},\"offer\":{{\"digest\":\"blake3:{}\",\"offer_id\":\"move\"}},\"operation_id\":\"{OPERATION_ID}\",\"payload\":{payload},\"request_id\":\"{REMOTE_REQUEST_ID}\"}}",
            "a".repeat(64),
        )
        .into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("fixture intent should be valid: {error:?}"))
}

fn accepted_action(receipt: &str) -> AssignmentMcpRemoteAcceptanceV1 {
    AssignmentMcpRemoteAcceptanceV1::new_action(
        REMOTE_REQUEST_ID,
        ACTION_ID,
        AssignmentMcpRemoteOutcomeV1::Accepted,
        format!(
            "{{\"action_id\":\"{ACTION_ID}\",\"receipt_id\":\"{receipt}\",\"request_id\":\"{REMOTE_REQUEST_ID}\",\"status\":\"accepted\"}}"
        )
        .into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("fixture acceptance should be valid: {error:?}"))
}

#[test]
fn prepared_operation_is_exactly_idempotent_across_restart() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should be available: {error}"));
    let root = temporary.path().join("operations");
    let ledger = FileAssignmentMcpOperationLedgerV1::open(&root)
        .unwrap_or_else(|error| unreachable!("ledger should open: {error:?}"));
    let intent = action_intent("{\"direction\":\"north\"}");

    let first = ledger
        .reserve(&intent)
        .unwrap_or_else(|error| unreachable!("first reservation should succeed: {error:?}"));
    let replay = ledger
        .reserve(&intent)
        .unwrap_or_else(|error| unreachable!("identical replay should succeed: {error:?}"));
    assert_eq!(first, replay);
    assert_eq!(first.phase(), AssignmentMcpOperationPhaseV1::Prepared);
    assert_eq!(first.canonical_request(), intent.canonical_request());
    assert_eq!(first.remote_request_id(), REMOTE_REQUEST_ID);

    drop(ledger);
    let reopened = FileAssignmentMcpOperationLedgerV1::open(&root)
        .unwrap_or_else(|error| unreachable!("ledger should reopen: {error:?}"));
    let loaded = reopened
        .load(&identity())
        .unwrap_or_else(|error| unreachable!("prepared record should load: {error:?}"))
        .unwrap_or_else(|| unreachable!("prepared record should exist"));
    assert_eq!(loaded, first);

    let changed = action_intent("{\"direction\":\"south\"}");
    assert_eq!(
        reopened.reserve(&changed),
        Err(AssignmentMcpOperationErrorV1::Conflict)
    );
}

#[test]
fn retained_remote_reply_finishes_after_restart_without_another_remote_call() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should be available: {error}"));
    let root = temporary.path().join("operations");
    let ledger = FileAssignmentMcpOperationLedgerV1::open(&root)
        .unwrap_or_else(|error| unreachable!("ledger should open: {error:?}"));
    ledger
        .reserve(&action_intent("{\"direction\":\"north\"}"))
        .unwrap_or_else(|error| unreachable!("reservation should succeed: {error:?}"));
    let acceptance = accepted_action("receipt-7");
    let accepted = ledger
        .record_remote(&identity(), &acceptance)
        .unwrap_or_else(|error| unreachable!("remote reply should persist: {error:?}"));
    assert_eq!(
        accepted.phase(),
        AssignmentMcpOperationPhaseV1::RemoteAccepted
    );
    assert_eq!(accepted.remote_acceptance(), Some(&acceptance));
    assert_eq!(acceptance.remote_request_id(), REMOTE_REQUEST_ID);
    assert_eq!(acceptance.outcome(), AssignmentMcpRemoteOutcomeV1::Accepted);
    assert!(
        acceptance
            .canonical_response()
            .ends_with(b"\"status\":\"accepted\"}")
    );
    assert!(acceptance.response_hash().starts_with("blake3:"));
    assert_eq!(
        accepted.intent(),
        &action_intent("{\"direction\":\"north\"}")
    );
    assert_eq!(
        accepted.kind(),
        &AssignmentMcpOperationKindV1::Action {
            action_id: ACTION_ID.to_owned()
        }
    );
    assert_eq!(accepted.request_hash(), accepted.intent().request_hash());

    drop(ledger);
    let reopened = FileAssignmentMcpOperationLedgerV1::open(&root)
        .unwrap_or_else(|error| unreachable!("ledger should reopen: {error:?}"));
    let recovered = reopened
        .load(&identity())
        .unwrap_or_else(|error| unreachable!("record should load: {error:?}"))
        .unwrap_or_else(|| unreachable!("record should exist"));
    assert_eq!(recovered.remote_acceptance(), Some(&acceptance));
    let completed = reopened
        .complete(&identity(), &acceptance)
        .unwrap_or_else(|error| unreachable!("retained reply should complete: {error:?}"));
    assert_eq!(completed.phase(), AssignmentMcpOperationPhaseV1::Complete);
    assert_eq!(
        reopened.complete(&identity(), &acceptance),
        Ok(completed.clone())
    );

    let changed = accepted_action("receipt-8");
    assert_eq!(
        reopened.record_remote(&identity(), &changed),
        Err(AssignmentMcpOperationErrorV1::Conflict)
    );
    assert_eq!(
        reopened.complete(&identity(), &changed),
        Err(AssignmentMcpOperationErrorV1::Conflict)
    );
}

#[test]
fn activation_completion_binds_lease_cursor_and_completion_identity() {
    let request = format!(
        "{{\"activation_id\":\"{ACTIVATION_ID}\",\"assignment_id\":\"{ASSIGNMENT_ID}\",\"claim_id\":\"{CLAIM_ID}\",\"completion\":{{\"disposition\":\"completed\"}},\"completion_id\":\"{COMPLETION_ID}\",\"context_hash\":\"blake3:{}\",\"cursor\":21,\"lease_generation\":3,\"operation_id\":\"{OPERATION_ID}\",\"request_id\":\"{REMOTE_REQUEST_ID}\"}}",
        "b".repeat(64)
    );
    let intent = AssignmentMcpOperationIntentV1::new_activation_completion(
        identity(),
        ACTIVATION_ID,
        CLAIM_ID,
        3,
        21,
        COMPLETION_ID,
        REMOTE_REQUEST_ID,
        request.into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("completion intent should be valid: {error:?}"));
    assert_eq!(
        intent.kind(),
        &AssignmentMcpOperationKindV1::ActivationCompletion {
            activation_id: ACTIVATION_ID.to_owned(),
            claim_id: CLAIM_ID.to_owned(),
            lease_generation: 3,
            acquisition_cursor: 21,
            completion_id: COMPLETION_ID.to_owned(),
        }
    );

    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should be available: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(temporary.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("ledger should open: {error:?}"));
    ledger
        .reserve(&intent)
        .unwrap_or_else(|error| unreachable!("completion should reserve: {error:?}"));
    let altered_request = String::from_utf8(intent.canonical_request().to_vec())
        .unwrap_or_else(|error| unreachable!("fixture should be UTF-8: {error}"))
        .replace("\"completed\"", "\"failed\"");
    let altered = AssignmentMcpOperationIntentV1::new_activation_completion(
        identity(),
        ACTIVATION_ID,
        CLAIM_ID,
        3,
        21,
        COMPLETION_ID,
        REMOTE_REQUEST_ID,
        altered_request.into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("altered completion should be valid: {error:?}"));
    assert_eq!(
        ledger.reserve(&altered),
        Err(AssignmentMcpOperationErrorV1::Conflict)
    );

    let response = format!(
        "{{\"activation_id\":\"{ACTIVATION_ID}\",\"completion_id\":\"{COMPLETION_ID}\",\"request_id\":\"{REMOTE_REQUEST_ID}\",\"status\":\"accepted\"}}"
    );
    let acceptance = AssignmentMcpRemoteAcceptanceV1::new_activation_completion(
        REMOTE_REQUEST_ID,
        ACTIVATION_ID,
        COMPLETION_ID,
        AssignmentMcpRemoteOutcomeV1::Accepted,
        response.into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("completion response should be valid: {error:?}"));
    assert_eq!(
        ledger
            .record_remote(&identity(), &acceptance)
            .unwrap_or_else(|error| unreachable!("completion response should persist: {error:?}"))
            .remote_acceptance(),
        Some(&acceptance)
    );

    let private = format!(
        "{{\"activation_id\":\"{ACTIVATION_ID}\",\"context\":{{\"private_memory\":\"do-not-store\"}}}}"
    );
    assert_eq!(
        AssignmentMcpOperationIntentV1::new_activation_completion(
            identity(),
            ACTIVATION_ID,
            CLAIM_ID,
            3,
            21,
            COMPLETION_ID,
            REMOTE_REQUEST_ID,
            private.into_bytes(),
        ),
        Err(AssignmentMcpOperationErrorV1::PrivateData)
    );
}

#[test]
fn credential_shaped_or_private_remote_material_is_rejected_and_errors_are_redacted() {
    let secret = "Bearer should-never-be-retained";
    let secret_intent = AssignmentMcpOperationIntentV1::new_action(
        identity(),
        ACTION_ID,
        REMOTE_REQUEST_ID,
        format!("{{\"payload\":{{\"authorization\":\"{secret}\"}}}} ").into_bytes(),
    );
    assert_eq!(
        secret_intent,
        Err(AssignmentMcpOperationErrorV1::CredentialData)
    );

    let private_reply = AssignmentMcpRemoteAcceptanceV1::new_activation_completion(
        REMOTE_REQUEST_ID,
        ACTIVATION_ID,
        COMPLETION_ID,
        AssignmentMcpRemoteOutcomeV1::Accepted,
        br#"{"status":"accepted","projection":{"private":"state"}}"#.to_vec(),
    );
    assert_eq!(
        private_reply,
        Err(AssignmentMcpOperationErrorV1::PrivateData)
    );
    assert!(!format!("{secret_intent:?}").contains(secret));
}

#[test]
fn collision_prone_identifiers_have_distinct_exact_disk_names() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should be available: {error}"));
    let root = temporary.path().join("operations");
    let ledger = FileAssignmentMcpOperationLedgerV1::open(&root)
        .unwrap_or_else(|error| unreachable!("ledger should open: {error:?}"));
    let first_identity = AssignmentMcpOperationIdentityV1::new(ASSIGNMENT_ID, "action-a")
        .unwrap_or_else(|error| unreachable!("fixture should be valid: {error:?}"));
    let second_identity = AssignmentMcpOperationIdentityV1::new(ASSIGNMENT_ID, "action_a")
        .unwrap_or_else(|error| unreachable!("fixture should be valid: {error:?}"));
    let first = AssignmentMcpOperationIntentV1::new_action(
        first_identity.clone(),
        ACTION_ID,
        REMOTE_REQUEST_ID,
        format!(
            "{{\"action_id\":\"{ACTION_ID}\",\"assignment_id\":\"{ASSIGNMENT_ID}\",\"operation_id\":\"action-a\",\"payload\":{{\"value\":1}},\"request_id\":\"{REMOTE_REQUEST_ID}\"}}"
        )
        .into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("fixture should be valid: {error:?}"));
    let second = AssignmentMcpOperationIntentV1::new_action(
        second_identity.clone(),
        ACTION_ID,
        "01ARZ3NDEKTSV4RRFFQ69G5FB2",
        format!(
            "{{\"action_id\":\"{ACTION_ID}\",\"assignment_id\":\"{ASSIGNMENT_ID}\",\"operation_id\":\"action_a\",\"payload\":{{\"value\":2}},\"request_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FB2\"}}"
        )
        .into_bytes(),
    )
    .unwrap_or_else(|error| unreachable!("fixture should be valid: {error:?}"));
    ledger
        .reserve(&first)
        .unwrap_or_else(|error| unreachable!("first should reserve: {error:?}"));
    ledger
        .reserve(&second)
        .unwrap_or_else(|error| unreachable!("second should reserve: {error:?}"));

    let filenames = std::fs::read_dir(&root)
        .unwrap_or_else(|error| unreachable!("ledger directory should list: {error}"))
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "json")
        })
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    assert_eq!(filenames.len(), 2);
    assert_ne!(filenames[0], filenames[1]);
    assert!(
        ledger
            .load(&first_identity)
            .unwrap_or_else(|error| unreachable!("first should load: {error:?}"))
            .is_some()
    );
    assert!(
        ledger
            .load(&second_identity)
            .unwrap_or_else(|error| unreachable!("second should load: {error:?}"))
            .is_some()
    );
}

#[test]
fn corrupt_record_fails_closed_on_load_and_restart() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should be available: {error}"));
    let root = temporary.path().join("operations");
    let ledger = FileAssignmentMcpOperationLedgerV1::open(&root)
        .unwrap_or_else(|error| unreachable!("ledger should open: {error:?}"));
    ledger
        .reserve(&action_intent("{\"direction\":\"north\"}"))
        .unwrap_or_else(|error| unreachable!("operation should reserve: {error:?}"));
    let record_path = std::fs::read_dir(&root)
        .unwrap_or_else(|error| unreachable!("ledger should list: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|value| value == "json"))
        .unwrap_or_else(|| unreachable!("record should exist"));
    let encoded = std::fs::read_to_string(&record_path)
        .unwrap_or_else(|error| unreachable!("record should read: {error}"));
    let corrupted = encoded.replacen("\"prepared\"", "\"complete\"", 1);
    std::fs::write(&record_path, corrupted)
        .unwrap_or_else(|error| unreachable!("fixture corruption should write: {error}"));

    assert_eq!(
        ledger.load(&identity()),
        Err(AssignmentMcpOperationErrorV1::InvalidData)
    );
    assert!(matches!(
        FileAssignmentMcpOperationLedgerV1::open(&root),
        Err(AssignmentMcpOperationErrorV1::InvalidData)
    ));
}

#[test]
fn topology_and_oversized_requests_are_rejected_before_disk() {
    let with_room = format!(
        "{{\"action_id\":\"{ACTION_ID}\",\"assignment_id\":\"{ASSIGNMENT_ID}\",\"operation_id\":\"{OPERATION_ID}\",\"payload\":{{}},\"request_id\":\"{REMOTE_REQUEST_ID}\",\"room_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FB2\"}}"
    );
    assert_eq!(
        AssignmentMcpOperationIntentV1::new_action(
            identity(),
            ACTION_ID,
            REMOTE_REQUEST_ID,
            with_room.into_bytes()
        ),
        Err(AssignmentMcpOperationErrorV1::PrivateData)
    );
    assert_eq!(
        AssignmentMcpOperationIntentV1::new_action(
            identity(),
            ACTION_ID,
            REMOTE_REQUEST_ID,
            vec![b'x'; 256 * 1024 + 1]
        ),
        Err(AssignmentMcpOperationErrorV1::InvalidData)
    );
}
