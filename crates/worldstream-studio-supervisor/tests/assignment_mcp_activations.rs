#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::result_large_err,
    clippy::too_many_lines
)]

#[allow(dead_code)]
#[path = "../src/assignment_mcp_activations.rs"]
mod assignment_mcp_activations;

use std::{
    io::ErrorKind,
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use assignment_mcp_activations::{
    ActivationCompletionArgumentsV1, ActivationCompletionDispositionV1,
    ActivationCompletionReceiptV1, ActivationLeaseClockErrorV1, ActivationLeaseClockV1,
    ActivationLedgerErrorV1, ActivationLedgerSnapshotV1, ActivationOperationLedgerV1,
    ActivationTerminalOutcomeV1, ActivationToolErrorCodeV1, AssignedActivationLeaseV1,
    AssignedRunnerActivationAuthorityV1, AssignmentActivationToolsV1,
    FixedDaemonRunnerActivationGatewayV1, PreparedActivationAbandonV1, PreparedActivationAcquireV1,
    PreparedActivationCompletionV1, RunnerActivationGatewayErrorV1, RunnerActivationGatewayV1,
    SystemActivationLeaseClockV1,
};
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket, accept_hdr};
use worldstream_core::{
    ActivationContextInputV1, ActivationDeliveryV1 as CoreActivationDeliveryV1, CanonicalJsonV1,
    CompleteHeadV1, RoomSequenceV1, prepare_activation_context,
};
use worldstream_protocol::{
    ActionOffer, ActivationClaim, ActivationDelivery, ActivationInvocationContext,
    ActivationLeaseOperation, ActivationOffer, ActivationOfferRequest, ActivationOffers,
    ActivationOperationReply, ActivationResultCode, ClientHello, MAX_MESSAGE_BYTES,
    PROTOCOL_VERSION, PackReference, Principal, PrincipalKind, RoomHead, RunnerHello, RunnerReady,
    SealedCapabilityBearerV1, ServerWelcome, UlidString, VersionedEnvelope, decode_envelope,
};

const ASSIGNMENT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const PRINCIPAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const RUNNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const ROOM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const MEMBER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const ACTIVATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const OFFER_OPERATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const CLAIM_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
const COMPLETION_OPERATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";
const COMPLETION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB6";
const COMPLETION_REQUEST_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB7";
const CONTEXT_HASH: &str =
    "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BEARER: &str = "wsb1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[derive(Clone)]
struct FakeLedger {
    state: Arc<Mutex<ActivationLedgerSnapshotV1>>,
    completion_prepares: Arc<Mutex<u64>>,
}

impl FakeLedger {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ActivationLedgerSnapshotV1::Idle {
                last_cursor: 0,
            })),
            completion_prepares: Arc::new(Mutex::new(0)),
        }
    }
}

impl ActivationOperationLedgerV1 for FakeLedger {
    fn load(
        &self,
        _assignment_id: &str,
    ) -> Result<ActivationLedgerSnapshotV1, ActivationLedgerErrorV1> {
        Ok(self.state.lock().expect("ledger state").clone())
    }

    fn begin_acquire(
        &self,
        assignment_id: &str,
        activation_cursor: u64,
    ) -> Result<PreparedActivationAcquireV1, ActivationLedgerErrorV1> {
        let prepared = PreparedActivationAcquireV1::new(
            assignment_id,
            activation_cursor,
            OFFER_OPERATION_ID,
            CLAIM_ID,
        )?;
        *self.state.lock().expect("ledger state") =
            ActivationLedgerSnapshotV1::Acquiring(prepared.clone());
        Ok(prepared)
    }

    fn select_offer(
        &self,
        prepared: &PreparedActivationAcquireV1,
        offer: &ActivationOffer,
    ) -> Result<PreparedActivationAcquireV1, ActivationLedgerErrorV1> {
        let selected = prepared.clone().with_selected_offer(offer.clone())?;
        *self.state.lock().expect("ledger state") =
            ActivationLedgerSnapshotV1::Acquiring(selected.clone());
        Ok(selected)
    }

    fn retain_lease(
        &self,
        _prepared: &PreparedActivationAcquireV1,
        lease: &AssignedActivationLeaseV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        *self.state.lock().expect("ledger state") =
            ActivationLedgerSnapshotV1::Leased(Box::new(lease.clone()));
        Ok(())
    }

    fn retain_no_offer(
        &self,
        prepared: &PreparedActivationAcquireV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        *self.state.lock().expect("ledger state") = ActivationLedgerSnapshotV1::Idle {
            last_cursor: prepared.activation_cursor(),
        };
        Ok(())
    }

    fn begin_completion(
        &self,
        lease: &AssignedActivationLeaseV1,
        canonical_request_hash: &str,
        disposition: &str,
    ) -> Result<PreparedActivationCompletionV1, ActivationLedgerErrorV1> {
        *self.completion_prepares.lock().expect("prepare count") += 1;
        let prepared = PreparedActivationCompletionV1::new(
            lease,
            COMPLETION_OPERATION_ID,
            COMPLETION_ID,
            COMPLETION_REQUEST_ID,
            canonical_request_hash,
            disposition,
        )?;
        *self.state.lock().expect("ledger state") = ActivationLedgerSnapshotV1::Completing {
            lease: Box::new(lease.clone()),
            prepared: prepared.clone(),
        };
        Ok(prepared)
    }

    fn retain_completion(
        &self,
        _prepared: &PreparedActivationCompletionV1,
        receipt: &ActivationCompletionReceiptV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        *self.state.lock().expect("ledger state") =
            ActivationLedgerSnapshotV1::Completed(receipt.clone());
        Ok(())
    }

    fn retain_terminal(
        &self,
        lease: &AssignedActivationLeaseV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        *self.state.lock().expect("ledger state") = ActivationLedgerSnapshotV1::Terminal {
            activation_cursor: lease.activation_cursor,
            activation_id: lease.activation_id().to_owned(),
            claim_id: lease.claim_id().to_owned(),
            lease_generation: lease.lease_generation(),
            outcome,
        };
        Ok(())
    }

    fn retain_abandoned(
        &self,
        prepared: &PreparedActivationAcquireV1,
        outcome: ActivationTerminalOutcomeV1,
    ) -> Result<(), ActivationLedgerErrorV1> {
        *self.state.lock().expect("ledger state") = ActivationLedgerSnapshotV1::Abandoned(
            PreparedActivationAbandonV1::new(prepared, outcome)?,
        );
        Ok(())
    }
}

#[derive(Clone)]
struct FakeGateway {
    offer: ActivationOffer,
    context_override: Arc<Mutex<Option<ActivationInvocationContext>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    fail_completion_once: Arc<Mutex<bool>>,
    completion_code: Arc<Mutex<Option<RunnerActivationGatewayErrorV1>>>,
    claim_code: Arc<Mutex<Option<RunnerActivationGatewayErrorV1>>>,
}

impl FakeGateway {
    fn new() -> Self {
        let offer = activation_offer();
        Self {
            offer,
            context_override: Arc::new(Mutex::new(None)),
            calls: Arc::new(Mutex::new(Vec::new())),
            fail_completion_once: Arc::new(Mutex::new(false)),
            completion_code: Arc::new(Mutex::new(None)),
            claim_code: Arc::new(Mutex::new(None)),
        }
    }
}

impl RunnerActivationGatewayV1 for FakeGateway {
    fn offers(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        operation_id: &str,
    ) -> Result<Vec<ActivationOffer>, RunnerActivationGatewayErrorV1> {
        self.calls.lock().expect("calls").push(json!({
            "method": "offers",
            "assignment_id": authority.assignment_id(),
            "runner_id": authority.runner_id(),
            "room_id": authority.room_id(),
            "member_id": authority.member_id(),
            "operation_id": operation_id,
        }));
        Ok(vec![self.offer.clone()])
    }

    fn claim(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        prepared: &PreparedActivationAcquireV1,
    ) -> Result<AssignedActivationLeaseV1, RunnerActivationGatewayErrorV1> {
        self.calls.lock().expect("calls").push(json!({
            "method": "claim",
            "runner_id": authority.runner_id(),
            "activation_id": prepared.selected_offer().expect("selected").activation_id,
            "claim_id": prepared.claim_id(),
        }));
        if let Some(error) = *self.claim_code.lock().expect("claim code") {
            return Err(error);
        }
        let context = self
            .context_override
            .lock()
            .expect("context override")
            .clone()
            .unwrap_or_else(activation_context);
        let context_hash =
            assignment_mcp_activations::canonical_activation_context_hash_v1(&context)
                .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)?;
        AssignedActivationLeaseV1::new(
            ASSIGNMENT_ID,
            prepared.activation_cursor(),
            ACTIVATION_ID,
            CLAIM_ID,
            &context_hash,
            context,
        )
        .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)
    }

    fn complete(
        &self,
        authority: &AssignedRunnerActivationAuthorityV1,
        lease: &AssignedActivationLeaseV1,
        prepared: &PreparedActivationCompletionV1,
    ) -> Result<ActivationCompletionReceiptV1, RunnerActivationGatewayErrorV1> {
        self.calls.lock().expect("calls").push(json!({
            "method": "complete",
            "runner_id": authority.runner_id(),
            "activation_id": lease.activation_id(),
            "operation_id": prepared.operation_id(),
        }));
        if *self.fail_completion_once.lock().expect("fail flag") {
            *self.fail_completion_once.lock().expect("fail flag") = false;
            return Err(RunnerActivationGatewayErrorV1::Disconnected);
        }
        if let Some(error) = *self.completion_code.lock().expect("code") {
            return Err(error);
        }
        ActivationCompletionReceiptV1::new(
            lease,
            prepared.operation_id(),
            prepared.completion_id(),
            false,
        )
        .map_err(|_| RunnerActivationGatewayErrorV1::InvalidData)
    }
}

#[derive(Clone, Copy)]
struct FixedClock(bool);

impl ActivationLeaseClockV1 for FixedClock {
    fn is_expired(&self, _lease_until: &str) -> Result<bool, ActivationLeaseClockErrorV1> {
        Ok(self.0)
    }
}

#[test]
fn next_activation_is_empty_input_and_exact_assignment_scoped() {
    let ledger = FakeLedger::new();
    let gateway = FakeGateway::new();
    let tools =
        AssignmentActivationToolsV1::new(authority(), gateway.clone(), ledger, FixedClock(false));

    let acquired = tools.next_activation(&json!({})).expect("acquire");
    assert_eq!(acquired.activation_cursor, 1);
    assert_eq!(acquired.activation.activation_id(), ACTIVATION_ID);
    assert!(!acquired.reconnected);
    assert_eq!(
        gateway.calls.lock().expect("calls").as_slice(),
        &[
            json!({
                "method": "offers",
                "assignment_id": ASSIGNMENT_ID,
                "runner_id": RUNNER_ID,
                "room_id": ROOM_ID,
                "member_id": MEMBER_ID,
                "operation_id": OFFER_OPERATION_ID,
            }),
            json!({
                "method": "claim",
                "runner_id": RUNNER_ID,
                "activation_id": ACTIVATION_ID,
                "claim_id": CLAIM_ID,
            }),
        ]
    );

    let rejected = tools
        .next_activation(&json!({"room_id": "attacker-selected"}))
        .expect_err("arbitrary scope must be rejected");
    assert_eq!(rejected.code(), ActivationToolErrorCodeV1::InvalidArguments);
}

#[test]
fn reconnect_returns_exact_retained_lease_without_claiming_another() {
    let ledger = FakeLedger::new();
    let first_gateway = FakeGateway::new();
    let first = AssignmentActivationToolsV1::new(
        authority(),
        first_gateway,
        ledger.clone(),
        FixedClock(false),
    );
    first.next_activation(&json!({})).expect("first acquire");

    let restarted_gateway = FakeGateway::new();
    let restarted = AssignmentActivationToolsV1::new(
        authority(),
        restarted_gateway.clone(),
        ledger,
        FixedClock(false),
    );
    let resumed = restarted.next_activation(&json!({})).expect("resume");
    assert!(resumed.reconnected);
    assert_eq!(resumed.activation.activation_id(), ACTIVATION_ID);
    assert!(restarted_gateway.calls.lock().expect("calls").is_empty());
}

#[test]
fn completion_binds_current_cursor_generation_context_and_reuses_operation_identity() {
    let ledger = FakeLedger::new();
    let gateway = FakeGateway::new();
    let tools = AssignmentActivationToolsV1::new(
        authority(),
        gateway.clone(),
        ledger.clone(),
        FixedClock(false),
    );
    tools.next_activation(&json!({})).expect("acquire");
    *gateway.fail_completion_once.lock().expect("fail flag") = true;
    let arguments = json!({
        "activation_cursor": 1,
        "lease_generation": 7,
        "context_hash": activation_context_hash(),
        "disposition": "handled"
    });

    let lost = tools
        .complete(arguments.clone())
        .expect_err("lost response is retryable");
    assert_eq!(lost.code(), ActivationToolErrorCodeV1::Disconnected);
    let receipt = tools.complete(arguments).expect("stable retry");
    assert_eq!(receipt.activation_cursor, 1);
    assert_eq!(receipt.operation_id, COMPLETION_OPERATION_ID);
    assert_eq!(*ledger.completion_prepares.lock().expect("count"), 1);
    let completion_calls: Vec<_> = gateway
        .calls
        .lock()
        .expect("calls")
        .iter()
        .filter(|call| call["method"] == "complete")
        .cloned()
        .collect();
    assert_eq!(completion_calls.len(), 2);
    assert!(
        completion_calls
            .iter()
            .all(|call| call["operation_id"] == COMPLETION_OPERATION_ID)
    );
}

#[test]
fn altered_or_duplicate_completion_cannot_target_a_different_activation() {
    let ledger = FakeLedger::new();
    let gateway = FakeGateway::new();
    let tools = AssignmentActivationToolsV1::new(authority(), gateway, ledger, FixedClock(false));
    tools.next_activation(&json!({})).expect("acquire");

    let invalid_disposition = tools
        .complete(json!({
            "activation_cursor": 1,
            "lease_generation": 7,
            "context_hash": activation_context_hash(),
            "disposition": "completed"
        }))
        .expect_err("completion disposition is a closed protocol value");
    assert_eq!(
        invalid_disposition.code(),
        ActivationToolErrorCodeV1::InvalidArguments
    );

    let wrong = tools
        .complete(json!({
            "activation_cursor": 2,
            "lease_generation": 7,
            "context_hash": activation_context_hash(),
            "disposition": "handled"
        }))
        .expect_err("wrong cursor");
    assert_eq!(wrong.code(), ActivationToolErrorCodeV1::StaleCursor);

    let arguments = ActivationCompletionArgumentsV1 {
        activation_cursor: 1,
        lease_generation: 7,
        context_hash: activation_context_hash(),
        disposition: ActivationCompletionDispositionV1::Handled,
    };
    tools
        .complete(serde_json::to_value(&arguments).expect("arguments"))
        .expect("complete");
    let duplicate = tools
        .complete(serde_json::to_value(&arguments).expect("arguments"))
        .expect_err("terminal duplicate is explicit");
    assert_eq!(
        duplicate.code(),
        ActivationToolErrorCodeV1::AlreadyCompleted
    );
}

#[test]
fn lease_expiry_revocation_and_stale_lease_are_distinct() {
    let ledger = acquired_ledger();
    let expired = AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        ledger.clone(),
        FixedClock(true),
    )
    .next_activation(&json!({}))
    .expect_err("expired");
    assert_eq!(expired.code(), ActivationToolErrorCodeV1::LeaseExpired);

    let revoked_gateway = FakeGateway::new();
    *revoked_gateway.completion_code.lock().expect("code") =
        Some(RunnerActivationGatewayErrorV1::Revoked);
    let ledger = acquired_ledger();
    let revoked = AssignmentActivationToolsV1::new(
        authority(),
        revoked_gateway,
        ledger.clone(),
        FixedClock(false),
    )
    .complete(completion_arguments())
    .expect_err("revoked");
    assert_eq!(revoked.code(), ActivationToolErrorCodeV1::AuthorityRevoked);

    let ledger = acquired_ledger();
    let stale_gateway = FakeGateway::new();
    *stale_gateway.completion_code.lock().expect("code") =
        Some(RunnerActivationGatewayErrorV1::StaleLease);
    let stale =
        AssignmentActivationToolsV1::new(authority(), stale_gateway, ledger, FixedClock(false))
            .complete(completion_arguments())
            .expect_err("stale lease");
    assert_eq!(stale.code(), ActivationToolErrorCodeV1::StaleCursor);
}

fn acquired_ledger() -> FakeLedger {
    let ledger = FakeLedger::new();
    AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        ledger.clone(),
        FixedClock(false),
    )
    .next_activation(&json!({}))
    .expect("acquire");
    ledger
}

#[test]
fn runner_authority_is_redacted_and_has_no_participant_client_or_serializable_credential() {
    let authority = authority();
    let debug = format!("{authority:?}");
    assert!(debug.contains("REDACTED"));
    assert!(!debug.contains(BEARER));
    assert_eq!(authority.runner_id(), RUNNER_ID);
    assert_eq!(authority.room_id(), ROOM_ID);
    assert_eq!(authority.member_id(), MEMBER_ID);
}

#[test]
fn daemon_context_is_hash_verified_and_bound_to_exact_room_pack_and_frame_order() {
    let tampered = AssignedActivationLeaseV1::new(
        ASSIGNMENT_ID,
        1,
        ACTIVATION_ID,
        CLAIM_ID,
        CONTEXT_HASH,
        activation_context(),
    );
    assert_eq!(tampered, Err(ActivationLedgerErrorV1::InvalidData));

    let mut wrong_room = activation_context();
    wrong_room.room_head.room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB8".to_owned();
    assert_context_rejected_by_scope(wrong_room);

    let mut wrong_pack = activation_context();
    wrong_pack.room_head.pack_digest = CONTEXT_HASH.to_owned();
    assert_context_rejected_by_scope(wrong_pack);

    let payload_hash = format!("blake3:{}", blake3::hash(b"{}").to_hex());
    let mut unordered = activation_context();
    unordered.cursor = Some(0);
    unordered.frame_head = 2;
    unordered.delivery = ActivationDelivery::RetainedFrames {
        cursor_exclusive: 0,
        through_frame_head: 2,
        frames: vec![
            worldstream_protocol::ActivationFrame {
                frame_seq: 2,
                cause_room_seq: 4,
                payload_hash: payload_hash.clone(),
                payload: json!({}),
            },
            worldstream_protocol::ActivationFrame {
                frame_seq: 1,
                cause_room_seq: 4,
                payload_hash,
                payload: json!({}),
            },
        ],
    };
    assert!(assignment_mcp_activations::canonical_activation_context_hash_v1(&unordered).is_err());
}

#[test]
fn daemon_context_reconstructs_the_exact_authoritative_pack_view_before_hashing() {
    let context = activation_context();
    let protocol_projection: worldstream_protocol::Projection =
        serde_json::from_value(context.projection.clone()).expect("protocol projection");
    let exact_view = json!({
        "action_offers": &protocol_projection.action_offers,
        "authorized_core": &protocol_projection.core,
        "projection": &protocol_projection.activity,
        "projection_schema": &context.projection_schema,
    });
    let canonical = |value: &Value| {
        CanonicalJsonV1::parse(&serde_json::to_vec(value).expect("json"))
            .and_then(|value| value.to_bytes())
            .expect("canonical")
    };
    let room_head: CompleteHeadV1 =
        serde_json::from_value(serde_json::to_value(&context.room_head).expect("room head value"))
            .expect("complete head");
    let expected = prepare_activation_context(ActivationContextInputV1 {
        activation_id: context.activation_id.clone(),
        claim_id: context.claim_id.clone(),
        cause_room_seq: RoomSequenceV1::new(context.cause_room_seq).expect("room sequence"),
        reason_code: context.reason_code.clone(),
        lease_generation: context.lease_generation,
        lease_until: context.lease_until.clone(),
        semantic_deadline: context
            .deadline
            .as_deref()
            .map(str::parse)
            .transpose()
            .expect("semantic deadline"),
        room_head,
        integrity_generation: context.integrity_generation,
        policy_revision: context.policy_revision,
        authority_generation: context.authority_generation,
        membership_generation: context.membership_generation,
        frame_head: context.frame_head,
        retained_floor: context.retained_floor,
        cursor: context.cursor,
        projection_schema: context.projection_schema.clone(),
        projection_bytes: canonical(&exact_view),
        action_offers_bytes: canonical(
            &serde_json::to_value(&context.action_offers).expect("offers value"),
        ),
        runner_budget_bytes: canonical(&context.runner_budget),
        runner_limits_bytes: canonical(&context.runner_limits),
        artifact_references: vec![],
        delivery: CoreActivationDeliveryV1::RetainedFrames {
            cursor_exclusive: 2,
            through_frame_head: 2,
            frames: vec![],
        },
    })
    .expect("exact Core context");

    assert_eq!(
        assignment_mcp_activations::canonical_activation_context_hash_v1(&context)
            .expect("wire context hash"),
        expected
            .context_hash()
            .expect("Core context hash")
            .to_string(),
    );
}

#[test]
fn terminal_expiry_stale_and_already_completed_resume_at_next_cursor() {
    let local_ledger = FakeLedger::new();
    AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        local_ledger.clone(),
        FixedClock(false),
    )
    .next_activation(&json!({}))
    .expect("local lease");
    let expired = AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        local_ledger.clone(),
        FixedClock(true),
    )
    .next_activation(&json!({}))
    .expect_err("expiry is reported once");
    assert_eq!(expired.code(), ActivationToolErrorCodeV1::LeaseExpired);
    assert_terminal_witness(&local_ledger, ActivationTerminalOutcomeV1::Expired);
    let after_expiry = AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        local_ledger,
        FixedClock(false),
    )
    .next_activation(&json!({}))
    .expect("next after expiry");
    assert_eq!(after_expiry.activation_cursor, 2);

    let leased_completion_ledger = acquired_ledger();
    let leased_expiry = AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        leased_completion_ledger.clone(),
        FixedClock(true),
    )
    .complete(completion_arguments())
    .expect_err("leased completion expiry");
    assert_eq!(
        leased_expiry.code(),
        ActivationToolErrorCodeV1::LeaseExpired
    );
    assert_terminal_witness(
        &leased_completion_ledger,
        ActivationTerminalOutcomeV1::Expired,
    );

    let completing_ledger = acquired_ledger();
    let disconnected_gateway = FakeGateway::new();
    *disconnected_gateway
        .fail_completion_once
        .lock()
        .expect("fail once") = true;
    AssignmentActivationToolsV1::new(
        authority(),
        disconnected_gateway,
        completing_ledger.clone(),
        FixedClock(false),
    )
    .complete(completion_arguments())
    .expect_err("ambiguous completion");
    let resolved_after_expiry = AssignmentActivationToolsV1::new(
        authority(),
        FakeGateway::new(),
        completing_ledger,
        FixedClock(true),
    )
    .complete(completion_arguments())
    .expect("authoritative retry resolves after local expiry");
    assert_eq!(resolved_after_expiry.operation_id, COMPLETION_OPERATION_ID);

    for (remote, expected) in [
        (
            RunnerActivationGatewayErrorV1::StaleLease,
            ActivationToolErrorCodeV1::StaleCursor,
        ),
        (
            RunnerActivationGatewayErrorV1::AlreadyCompleted,
            ActivationToolErrorCodeV1::AlreadyCompleted,
        ),
        (
            RunnerActivationGatewayErrorV1::Expired,
            ActivationToolErrorCodeV1::LeaseExpired,
        ),
    ] {
        let ledger = FakeLedger::new();
        let gateway = FakeGateway::new();
        let tools = AssignmentActivationToolsV1::new(
            authority(),
            gateway.clone(),
            ledger.clone(),
            FixedClock(false),
        );
        tools.next_activation(&json!({})).expect("remote lease");
        *gateway.completion_code.lock().expect("code") = Some(remote);
        let outcome = tools
            .complete(completion_arguments())
            .expect_err("terminal remote result");
        assert_eq!(outcome.code(), expected);
        let expected_terminal = match remote {
            RunnerActivationGatewayErrorV1::StaleLease => ActivationTerminalOutcomeV1::Stale,
            RunnerActivationGatewayErrorV1::AlreadyCompleted => {
                ActivationTerminalOutcomeV1::AlreadyCompleted
            }
            RunnerActivationGatewayErrorV1::Expired => ActivationTerminalOutcomeV1::Expired,
            _ => unreachable!("terminal fixture"),
        };
        assert_terminal_witness(&ledger, expected_terminal);
        let resumed = AssignmentActivationToolsV1::new(
            authority(),
            FakeGateway::new(),
            ledger,
            FixedClock(false),
        )
        .next_activation(&json!({}))
        .expect("next after terminal result");
        assert_eq!(resumed.activation_cursor, 2);
    }
}

#[test]
fn terminal_claim_replies_are_durably_abandoned_and_restart_advances() {
    for (remote, expected_error, expected_outcome) in [
        (
            RunnerActivationGatewayErrorV1::Expired,
            ActivationToolErrorCodeV1::NoActivation,
            ActivationTerminalOutcomeV1::Expired,
        ),
        (
            RunnerActivationGatewayErrorV1::StaleLease,
            ActivationToolErrorCodeV1::StaleCursor,
            ActivationTerminalOutcomeV1::Stale,
        ),
        (
            RunnerActivationGatewayErrorV1::AlreadyCompleted,
            ActivationToolErrorCodeV1::AlreadyCompleted,
            ActivationTerminalOutcomeV1::AlreadyCompleted,
        ),
    ] {
        let ledger = FakeLedger::new();
        let terminal_gateway = FakeGateway::new();
        *terminal_gateway.claim_code.lock().expect("claim code") = Some(remote);
        let terminal = AssignmentActivationToolsV1::new(
            authority(),
            terminal_gateway,
            ledger.clone(),
            FixedClock(false),
        )
        .next_activation(&json!({}))
        .expect_err("claim terminal response");
        assert_eq!(terminal.code(), expected_error);

        let snapshot = ledger.load(ASSIGNMENT_ID).expect("abandoned snapshot");
        let ActivationLedgerSnapshotV1::Abandoned(abandoned) = snapshot else {
            panic!("expected an exact abandoned acquisition witness");
        };
        assert_eq!(abandoned.assignment_id(), ASSIGNMENT_ID);
        assert_eq!(abandoned.activation_cursor(), 1);
        assert_eq!(abandoned.activation_id(), ACTIVATION_ID);
        assert_eq!(abandoned.claim_id(), CLAIM_ID);
        assert_eq!(abandoned.outcome(), expected_outcome);

        let restart_gateway = FakeGateway::new();
        let restarted = AssignmentActivationToolsV1::new(
            authority(),
            restart_gateway.clone(),
            ledger,
            FixedClock(false),
        )
        .next_activation(&json!({}))
        .expect("restart advances after abandoned claim");
        assert_eq!(restarted.activation_cursor, 2);
        assert_eq!(
            restart_gateway
                .calls
                .lock()
                .expect("restart calls")
                .iter()
                .filter(|call| call["method"] == "claim")
                .count(),
            1,
            "the abandoned cursor is not replayed"
        );
    }
}

#[test]
fn private_acquisition_and_completion_state_is_redacted_from_debug() {
    let selected = PreparedActivationAcquireV1::new(ASSIGNMENT_ID, 1, OFFER_OPERATION_ID, CLAIM_ID)
        .expect("prepared acquire")
        .with_selected_offer(activation_offer())
        .expect("selected offer");
    let acquiring_debug = format!(
        "{:?}",
        ActivationLedgerSnapshotV1::Acquiring(selected.clone())
    );
    for private in [
        ROOM_ID,
        MEMBER_ID,
        ACTIVATION_ID,
        CLAIM_ID,
        OFFER_OPERATION_ID,
    ] {
        assert!(!acquiring_debug.contains(private));
    }

    let abandoned =
        PreparedActivationAbandonV1::new(&selected, ActivationTerminalOutcomeV1::AlreadyCompleted)
            .expect("abandoned witness");
    let abandoned_debug = format!("{:?}", ActivationLedgerSnapshotV1::Abandoned(abandoned));
    for private in [ROOM_ID, MEMBER_ID, ACTIVATION_ID, CLAIM_ID] {
        assert!(!abandoned_debug.contains(private));
    }
}

fn assert_terminal_witness(ledger: &FakeLedger, expected: ActivationTerminalOutcomeV1) {
    let snapshot = ledger.load(ASSIGNMENT_ID).expect("terminal snapshot");
    assert_eq!(
        snapshot,
        ActivationLedgerSnapshotV1::Terminal {
            activation_cursor: 1,
            activation_id: ACTIVATION_ID.to_owned(),
            claim_id: CLAIM_ID.to_owned(),
            lease_generation: 7,
            outcome: expected,
        }
    );
}

fn assert_context_rejected_by_scope(context: ActivationInvocationContext) {
    let ledger = FakeLedger::new();
    let gateway = FakeGateway::new();
    *gateway.context_override.lock().expect("context override") = Some(context);
    let error = AssignmentActivationToolsV1::new(authority(), gateway, ledger, FixedClock(false))
        .next_activation(&json!({}))
        .expect_err("wrong daemon scope");
    assert_eq!(error.code(), ActivationToolErrorCodeV1::InvalidDaemonData);
}

#[test]
fn production_gateway_uses_runner_protocol_and_exact_sealed_scope() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || serve_runner_sessions(&listener));
    let ledger = FakeLedger::new();
    let gateway = FixedDaemonRunnerActivationGatewayV1::new(address, Duration::from_secs(2))
        .expect("gateway");
    gateway.establish(&authority()).expect("established Runner");
    let tools = AssignmentActivationToolsV1::new(
        authority(),
        gateway,
        ledger,
        SystemActivationLeaseClockV1,
    );

    let acquired = tools.next_activation(&json!({})).expect("real acquire");
    assert_eq!(acquired.activation.activation_id(), ACTIVATION_ID);
    let completed = tools
        .complete(completion_arguments())
        .expect("real completion");
    assert_eq!(completed.operation_id, COMPLETION_OPERATION_ID);
    server.join().expect("mock server");
}

#[test]
fn production_gateway_establishes_presence_without_issuing_activation_work() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let (live_sender, live_receiver) = mpsc::channel();
    let (closed_sender, closed_receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut socket = accept_hdr(
            stream,
            |_: &tungstenite::handshake::server::Request,
             mut response: tungstenite::handshake::server::Response| {
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    tungstenite::http::HeaderValue::from_static(
                        worldstream_protocol::WEBSOCKET_SUBPROTOCOL,
                    ),
                );
                Ok(response)
            },
        )
        .expect("websocket");
        let hello: ClientHello = read_client(&mut socket, "client.hello");
        assert_eq!(hello.mode, worldstream_protocol::ClientMode::Runner);
        send_server(
            &mut socket,
            "server.welcome",
            &ServerWelcome {
                session_id: "01ARZ3NDEKTSV4RRFFQ69G5FB4".parse().expect("session"),
                selected_protocol: PROTOCOL_VERSION.to_owned(),
                server_version: "0.1.0".to_owned(),
                heartbeat_interval_ms: 5_000,
                maximum_message_bytes: MAX_MESSAGE_BYTES,
                authenticated_principal: Principal {
                    principal_id: PRINCIPAL_ID.to_owned(),
                    kind: PrincipalKind::Agent,
                },
            },
        );
        let runner: RunnerHello = read_client(&mut socket, "runner.hello");
        assert_eq!(runner.runner_id, RUNNER_ID);
        send_server(
            &mut socket,
            "runner.ready",
            &RunnerReady {
                runner_id: RUNNER_ID.to_owned(),
            },
        );
        socket
            .get_ref()
            .set_nonblocking(true)
            .expect("nonblocking fixture");
        let mut byte = [0_u8; 1];
        assert!(matches!(
            socket.get_ref().peek(&mut byte),
            Err(error) if error.kind() == ErrorKind::WouldBlock
        ));
        live_sender.send(()).expect("live signal");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match socket.get_ref().peek(&mut byte) {
                Ok(0) => {
                    closed_sender.send(()).expect("closed signal");
                    return;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                result => unreachable!("retained socket close: {result:?}"),
            }
        }
        unreachable!("retained Runner socket did not close with the gateway");
    });
    let gateway = FixedDaemonRunnerActivationGatewayV1::new(address, Duration::from_secs(2))
        .expect("gateway");
    gateway.establish(&authority()).expect("established Runner");
    live_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("Runner session retained");
    drop(gateway);
    closed_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("Runner session dropped");
    server.join().expect("mock server");
}

fn serve_runner_sessions(listener: &TcpListener) {
    let (stream, _) = listener.accept().expect("accept");
    let mut socket = accept_hdr(
        stream,
        |request: &tungstenite::handshake::server::Request,
         mut response: tungstenite::handshake::server::Response| {
            assert_eq!(request.uri().path(), "/v1/runner/stream");
            assert_eq!(
                request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some(format!("Bearer {BEARER}").as_str())
            );
            response.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                tungstenite::http::HeaderValue::from_static(
                    worldstream_protocol::WEBSOCKET_SUBPROTOCOL,
                ),
            );
            Ok(response)
        },
    )
    .expect("websocket");
    let hello: ClientHello = read_client(&mut socket, "client.hello");
    assert_eq!(hello.mode, worldstream_protocol::ClientMode::Runner);
    send_server(
        &mut socket,
        "server.welcome",
        &ServerWelcome {
            session_id: "01ARZ3NDEKTSV4RRFFQ69G5FB4".parse().expect("session"),
            selected_protocol: PROTOCOL_VERSION.to_owned(),
            server_version: "0.1.0".to_owned(),
            heartbeat_interval_ms: 5_000,
            maximum_message_bytes: MAX_MESSAGE_BYTES,
            authenticated_principal: Principal {
                principal_id: PRINCIPAL_ID.to_owned(),
                kind: PrincipalKind::Agent,
            },
        },
    );
    let runner: RunnerHello = read_client(&mut socket, "runner.hello");
    assert_eq!(runner.runner_id, RUNNER_ID);
    assert_eq!(runner.supported_pack_revisions, vec![authority_pack()]);
    send_server(
        &mut socket,
        "runner.ready",
        &RunnerReady {
            runner_id: RUNNER_ID.to_owned(),
        },
    );
    for stage in 0..3 {
        match stage {
            0 => {
                let request: ActivationOfferRequest = read_client(&mut socket, "activation.offer");
                assert_eq!(request.operation_id, OFFER_OPERATION_ID);
                assert_eq!(request.runner_id, RUNNER_ID);
                assert_eq!(request.room_id, ROOM_ID);
                assert_eq!(request.member_id, MEMBER_ID);
                send_server(&mut socket, "server.ping", &json!({}));
                let pong: Value = read_client(&mut socket, "client.pong");
                assert_eq!(pong, json!({}));
                send_server(
                    &mut socket,
                    "activation.offers",
                    &ActivationOffers {
                        operation_id: OFFER_OPERATION_ID.to_owned(),
                        runner_id: RUNNER_ID.to_owned(),
                        offers: vec![activation_offer()],
                    },
                );
            }
            1 => {
                let request: ActivationClaim = read_client(&mut socket, "activation.claim");
                assert_eq!(request.activation_id, ACTIVATION_ID);
                assert_eq!(request.claim_id, CLAIM_ID);
                assert_eq!(request.runner_id, RUNNER_ID);
                send_server(
                    &mut socket,
                    "activation.claimed",
                    &ActivationOperationReply {
                        operation_id: CLAIM_ID.to_owned(),
                        activation_id: Some(ACTIVATION_ID.to_owned()),
                        claim_id: Some(CLAIM_ID.to_owned()),
                        runner_id: RUNNER_ID.to_owned(),
                        code: ActivationResultCode::Granted,
                        state: Some(worldstream_protocol::ActivationIntentState::Leased),
                        lease_generation: Some(7),
                        context_hash: Some(activation_context_hash()),
                        context: Some(activation_context()),
                    },
                );
            }
            2 => {
                let envelope: VersionedEnvelope<ActivationLeaseOperation> =
                    read_client_envelope(&mut socket, "activation.complete");
                assert_eq!(
                    envelope.request_id.as_ref().map(UlidString::as_str),
                    Some(COMPLETION_REQUEST_ID)
                );
                let request = envelope.body;
                assert_eq!(request.activation_id, ACTIVATION_ID);
                assert_eq!(request.claim_id, CLAIM_ID);
                assert_eq!(request.runner_id, RUNNER_ID);
                assert_eq!(request.operation_id, COMPLETION_OPERATION_ID);
                send_server(
                    &mut socket,
                    "activation.completed",
                    &ActivationOperationReply {
                        operation_id: COMPLETION_OPERATION_ID.to_owned(),
                        activation_id: Some(ACTIVATION_ID.to_owned()),
                        claim_id: Some(CLAIM_ID.to_owned()),
                        runner_id: RUNNER_ID.to_owned(),
                        code: ActivationResultCode::Completed,
                        state: Some(worldstream_protocol::ActivationIntentState::Completed),
                        lease_generation: Some(7),
                        context_hash: Some(activation_context_hash()),
                        context: None,
                    },
                );
            }
            _ => unreachable!("three stages"),
        }
    }
}

fn read_client<T: serde::de::DeserializeOwned>(
    socket: &mut WebSocket<std::net::TcpStream>,
    expected: &str,
) -> T {
    read_client_envelope(socket, expected).body
}

fn read_client_envelope<T: serde::de::DeserializeOwned>(
    socket: &mut WebSocket<std::net::TcpStream>,
    expected: &str,
) -> VersionedEnvelope<T> {
    let Message::Text(text) = socket.read().expect("client message") else {
        panic!("expected text message");
    };
    let envelope = decode_envelope::<T>(text.as_bytes()).expect("typed envelope");
    assert_eq!(envelope.message_type, expected);
    envelope
}

fn send_server<T: serde::Serialize>(
    socket: &mut WebSocket<std::net::TcpStream>,
    message_type: &str,
    body: &T,
) {
    let envelope = VersionedEnvelope {
        protocol: PROTOCOL_VERSION.to_owned(),
        message_type: message_type.to_owned(),
        message_id: "01ARZ3NDEKTSV4RRFFQ69G5FB5"
            .parse::<UlidString>()
            .expect("message id"),
        request_id: None,
        body,
    };
    socket
        .send(Message::Text(
            serde_json::to_string(&envelope).expect("envelope").into(),
        ))
        .expect("server message");
}

fn authority() -> AssignedRunnerActivationAuthorityV1 {
    AssignedRunnerActivationAuthorityV1::new(
        ASSIGNMENT_ID,
        PRINCIPAL_ID,
        RUNNER_ID,
        ROOM_ID,
        MEMBER_ID,
        authority_pack(),
        SealedCapabilityBearerV1::parse(BEARER.to_owned()).expect("bearer"),
    )
    .expect("authority")
}

fn authority_pack() -> PackReference {
    PackReference {
        id: "com.example.activity".to_owned(),
        version: "1.0.0".to_owned(),
        digest: "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_owned(),
    }
}

fn activation_offer() -> ActivationOffer {
    ActivationOffer {
        activation_id: ACTIVATION_ID.to_owned(),
        room_id: ROOM_ID.to_owned(),
        member_id: MEMBER_ID.to_owned(),
        cause_room_seq: 4,
        reason_code: "activity_requested".to_owned(),
        priority: 10,
        deadline: Some("2099-01-01T00:00:00Z".to_owned()),
        lease_duration_ms: 30_000,
    }
}

fn activation_context() -> ActivationInvocationContext {
    ActivationInvocationContext {
        activation_id: ACTIVATION_ID.to_owned(),
        claim_id: CLAIM_ID.to_owned(),
        cause_room_seq: 4,
        reason_code: "activity_requested".to_owned(),
        lease_generation: 7,
        lease_until: "2099-01-01T00:00:00Z".to_owned(),
        deadline: Some("2099-01-01T00:00:00Z".to_owned()),
        room_head: RoomHead {
            room_id: ROOM_ID.to_owned(),
            room_seq: 4,
            genesis_or_transition_hash: CONTEXT_HASH.to_owned(),
            core_schema_version: "worldstream/core/v1".to_owned(),
            pack_digest: authority_pack().digest,
            core_state_hash: CONTEXT_HASH.to_owned(),
            activity_state_hash: CONTEXT_HASH.to_owned(),
            authoritative_state_hash: CONTEXT_HASH.to_owned(),
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
                "payload_schema_digest": CONTEXT_HASH
            }]
        }),
        action_offers: vec![ActionOffer {
            domain: "activity".to_owned(),
            action_type: "generic_action".to_owned(),
            payload_schema_digest: CONTEXT_HASH.to_owned(),
            eligibility_window: None,
        }],
        runner_budget: json!({"remaining": 10}),
        runner_limits: json!({"maximum": 10}),
        artifact_references: vec![],
        delivery: ActivationDelivery::RetainedFrames {
            cursor_exclusive: 2,
            through_frame_head: 2,
            frames: vec![],
        },
    }
}

fn completion_arguments() -> Value {
    json!({
        "activation_cursor": 1,
        "lease_generation": 7,
        "context_hash": activation_context_hash(),
        "disposition": "handled"
    })
}

fn activation_context_hash() -> String {
    assignment_mcp_activations::canonical_activation_context_hash_v1(&activation_context())
        .expect("canonical context")
}
