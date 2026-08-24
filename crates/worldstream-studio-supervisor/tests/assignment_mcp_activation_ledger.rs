#![allow(dead_code, unused_imports)]

use serde_json::json;
use tempfile::TempDir;
use worldstream_protocol::{
    ActionOffer, ActivationDelivery, ActivationInvocationContext, ActivationOffer, PackReference,
    RoomHead,
};

mod assignment_mcp {
    pub use worldstream_studio_supervisor::assignment_mcp::MembershipStreamSnapshotV1;
}

#[path = "../src/assignment_mcp_operations.rs"]
mod assignment_mcp_operations;

#[path = "../src/assignment_mcp_activations.rs"]
mod assignment_mcp_activations;

#[path = "../src/assignment_mcp_activation_ledger.rs"]
mod assignment_mcp_activation_ledger;

use assignment_mcp_activation_ledger::FileActivationOperationLedgerV1;
use assignment_mcp_activations::{
    ActivationCompletionReceiptV1, ActivationLedgerErrorV1, ActivationLedgerSnapshotV1,
    ActivationOperationLedgerV1, ActivationTerminalOutcomeV1, AssignedActivationLeaseV1,
    PreparedActivationAbandonV1,
};

const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OTHER_ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const ACTIVATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const DIGEST: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn open(temporary: &TempDir) -> FileActivationOperationLedgerV1 {
    FileActivationOperationLedgerV1::open(temporary.path().join("activation-ledger"))
        .unwrap_or_else(|error| unreachable!("private ledger should open: {error:?}"))
}

fn offer() -> ActivationOffer {
    ActivationOffer {
        activation_id: ACTIVATION.to_owned(),
        room_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ".to_owned(),
        member_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned(),
        cause_room_seq: 4,
        reason_code: "activity_requested".to_owned(),
        priority: 1,
        deadline: None,
        lease_duration_ms: 30_000,
    }
}

fn context(claim_id: &str) -> ActivationInvocationContext {
    ActivationInvocationContext {
        activation_id: ACTIVATION.to_owned(),
        claim_id: claim_id.to_owned(),
        cause_room_seq: 4,
        reason_code: "activity_requested".to_owned(),
        lease_generation: 7,
        lease_until: "2099-01-01T00:00:00Z".to_owned(),
        deadline: None,
        room_head: RoomHead {
            room_id: offer().room_id,
            room_seq: 4,
            genesis_or_transition_hash: DIGEST.to_owned(),
            core_schema_version: "worldstream/core/v1".to_owned(),
            pack_digest: PackReference {
                id: "com.example.activity".to_owned(),
                version: "1.0.0".to_owned(),
                digest: DIGEST.to_owned(),
            }
            .digest,
            core_state_hash: DIGEST.to_owned(),
            activity_state_hash: DIGEST.to_owned(),
            authoritative_state_hash: DIGEST.to_owned(),
        },
        integrity_generation: 1,
        policy_revision: 1,
        authority_generation: 1,
        membership_generation: 1,
        frame_head: 2,
        retained_floor: 0,
        cursor: Some(2),
        projection_schema: "schema/projection/v1".to_owned(),
        projection: json!({
            "core": {"status": "running"},
            "activity": {"private_activation_input": "agent-only"},
            "action_offers": [{
                "domain": "activity",
                "action_type": "generic_action",
                "payload_schema_digest": DIGEST
            }]
        }),
        action_offers: vec![ActionOffer {
            domain: "activity".to_owned(),
            action_type: "generic_action".to_owned(),
            payload_schema_digest: DIGEST.to_owned(),
            eligibility_window: None,
        }],
        runner_budget: json!({"remaining": 10}),
        runner_limits: json!({"maximum": 10}),
        artifact_references: Vec::new(),
        delivery: ActivationDelivery::RetainedFrames {
            cursor_exclusive: 2,
            through_frame_head: 2,
            frames: Vec::new(),
        },
    }
}

fn lease(assignment_id: &str, claim_id: &str) -> AssignedActivationLeaseV1 {
    let context = context(claim_id);
    let context_hash = assignment_mcp_activations::canonical_activation_context_hash_v1(&context)
        .unwrap_or_else(|error| unreachable!("context should hash: {error:?}"));
    AssignedActivationLeaseV1::new(
        assignment_id,
        1,
        ACTIVATION,
        claim_id,
        &context_hash,
        context,
    )
    .unwrap_or_else(|error| unreachable!("lease fixture should be valid: {error:?}"))
}

#[test]
fn acquisition_and_private_lease_are_exact_and_restart_reproducible() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should exist: {error}"));
    let ledger = open(&temporary);
    let prepared = ledger
        .begin_acquire(ASSIGNMENT, 1)
        .unwrap_or_else(|error| unreachable!("acquisition should prepare: {error:?}"));
    let replay = ledger
        .begin_acquire(ASSIGNMENT, 1)
        .unwrap_or_else(|error| unreachable!("identical prepare should replay: {error:?}"));
    assert_eq!(prepared, replay);
    let selected = ledger
        .select_offer(&prepared, &offer())
        .unwrap_or_else(|error| unreachable!("offer should retain: {error:?}"));
    ledger
        .retain_lease(&selected, &lease(ASSIGNMENT, selected.claim_id()))
        .unwrap_or_else(|error| unreachable!("lease should retain: {error:?}"));
    drop(ledger);

    let restarted = open(&temporary);
    assert_eq!(
        restarted
            .load(ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("lease should reload: {error:?}")),
        ActivationLedgerSnapshotV1::Leased(Box::new(lease(ASSIGNMENT, selected.claim_id())))
    );
    assert_eq!(
        restarted
            .load(OTHER_ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("other assignment should load: {error:?}")),
        ActivationLedgerSnapshotV1::Idle { last_cursor: 0 }
    );
}

#[test]
fn completion_ids_and_result_reconcile_exactly_across_restart() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should exist: {error}"));
    let ledger = open(&temporary);
    let prepared = ledger
        .begin_acquire(ASSIGNMENT, 1)
        .and_then(|value| ledger.select_offer(&value, &offer()))
        .unwrap_or_else(|error| unreachable!("offer should prepare: {error:?}"));
    let lease = lease(ASSIGNMENT, prepared.claim_id());
    ledger
        .retain_lease(&prepared, &lease)
        .unwrap_or_else(|error| unreachable!("lease should retain: {error:?}"));
    let completion = ledger
        .begin_completion(&lease, DIGEST, "handled")
        .unwrap_or_else(|error| unreachable!("completion should prepare: {error:?}"));
    drop(ledger);

    let restarted = open(&temporary);
    let replay = restarted
        .begin_completion(&lease, DIGEST, "handled")
        .unwrap_or_else(|error| unreachable!("completion should replay: {error:?}"));
    assert_eq!(replay, completion);
    assert_eq!(
        restarted.begin_completion(&lease, DIGEST, "failed"),
        Err(ActivationLedgerErrorV1::Conflict)
    );
    let receipt = ActivationCompletionReceiptV1::new(
        &lease,
        completion.operation_id(),
        completion.completion_id(),
        false,
    )
    .unwrap_or_else(|error| unreachable!("receipt should be valid: {error:?}"));
    restarted
        .retain_completion(&completion, &receipt)
        .unwrap_or_else(|error| unreachable!("receipt should retain: {error:?}"));
    drop(restarted);

    assert_eq!(
        open(&temporary)
            .load(ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("completion should reload: {error:?}")),
        ActivationLedgerSnapshotV1::Completed(receipt)
    );
}

#[test]
fn terminal_outcomes_are_durable_and_advanceable_after_restart() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should exist: {error}"));
    let ledger = open(&temporary);
    let prepared = ledger
        .begin_acquire(ASSIGNMENT, 1)
        .and_then(|value| ledger.select_offer(&value, &offer()))
        .unwrap_or_else(|error| unreachable!("offer should prepare: {error:?}"));
    let lease = lease(ASSIGNMENT, prepared.claim_id());
    ledger
        .retain_lease(&prepared, &lease)
        .unwrap_or_else(|error| unreachable!("lease should retain: {error:?}"));
    ledger
        .begin_completion(&lease, DIGEST, "handled")
        .unwrap_or_else(|error| unreachable!("completion should prepare: {error:?}"));
    ledger
        .retain_terminal(&lease, ActivationTerminalOutcomeV1::Stale)
        .unwrap_or_else(|error| unreachable!("terminal should retain: {error:?}"));
    drop(ledger);

    let restarted = open(&temporary);
    assert_eq!(
        restarted
            .load(ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("terminal should reload: {error:?}")),
        ActivationLedgerSnapshotV1::Terminal {
            activation_cursor: 1,
            activation_id: ACTIVATION.to_owned(),
            claim_id: prepared.claim_id().to_owned(),
            lease_generation: 7,
            outcome: ActivationTerminalOutcomeV1::Stale
        }
    );
    let next = restarted
        .begin_acquire(ASSIGNMENT, 2)
        .and_then(|value| restarted.select_offer(&value, &offer()))
        .unwrap_or_else(|error| unreachable!("next claim should prepare: {error:?}"));
    restarted
        .retain_abandoned(&next, ActivationTerminalOutcomeV1::AlreadyCompleted)
        .unwrap_or_else(|error| unreachable!("abandonment should retain: {error:?}"));
    let expected =
        PreparedActivationAbandonV1::new(&next, ActivationTerminalOutcomeV1::AlreadyCompleted)
            .unwrap_or_else(|error| unreachable!("abandonment witness should build: {error:?}"));
    drop(restarted);

    let restarted = open(&temporary);
    assert_eq!(
        restarted
            .load(ASSIGNMENT)
            .unwrap_or_else(|error| unreachable!("abandonment should reload: {error:?}")),
        ActivationLedgerSnapshotV1::Abandoned(expected)
    );
    assert!(restarted.begin_acquire(ASSIGNMENT, 3).is_ok());
}

#[test]
fn private_paths_are_encoded_and_corruption_fails_closed() {
    let temporary = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should exist: {error}"));
    let ledger = open(&temporary);
    ledger
        .begin_acquire(ASSIGNMENT, 1)
        .unwrap_or_else(|error| unreachable!("acquisition should prepare: {error:?}"));
    drop(ledger);

    let private_root = temporary.path().join("activation-ledger/private");
    let record_path = std::fs::read_dir(&private_root)
        .unwrap_or_else(|error| unreachable!("private directory should list: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|value| value == "json"))
        .unwrap_or_else(|| unreachable!("private record should exist"));
    assert!(!record_path.to_string_lossy().contains(ASSIGNMENT));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = std::fs::metadata(&record_path)
            .unwrap_or_else(|error| unreachable!("private metadata should read: {error}"))
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
    let encoded = std::fs::read_to_string(&record_path)
        .unwrap_or_else(|error| unreachable!("private record should read: {error}"));
    let corrupt = encoded.replacen(ASSIGNMENT, OTHER_ASSIGNMENT, 1);
    std::fs::write(&record_path, corrupt)
        .unwrap_or_else(|error| unreachable!("fixture corruption should write: {error}"));

    assert!(matches!(
        FileActivationOperationLedgerV1::open(temporary.path().join("activation-ledger")),
        Err(ActivationLedgerErrorV1::InvalidData)
    ));

    let oversized = TempDir::new()
        .unwrap_or_else(|error| unreachable!("temporary directory should exist: {error}"));
    let oversized_ledger = open(&oversized);
    oversized_ledger
        .begin_acquire(ASSIGNMENT, 1)
        .unwrap_or_else(|error| unreachable!("acquisition should prepare: {error:?}"));
    drop(oversized_ledger);
    let oversized_path = std::fs::read_dir(oversized.path().join("activation-ledger/private"))
        .unwrap_or_else(|error| unreachable!("private directory should list: {error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|value| value == "json"))
        .unwrap_or_else(|| unreachable!("private record should exist"));
    std::fs::write(&oversized_path, vec![b' '; 2 * 1024 * 1024 + 1])
        .unwrap_or_else(|error| unreachable!("oversized fixture should write: {error}"));
    assert!(matches!(
        FileActivationOperationLedgerV1::open(oversized.path().join("activation-ledger")),
        Err(ActivationLedgerErrorV1::InvalidData)
    ));
}
