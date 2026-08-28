#![allow(dead_code)]

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicUsize, Ordering},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::tempdir;
use worldstream_protocol::{
    ActionOffer, ActivityPackCatalogAction, ActivityPackCatalogSchema, PackReference, RoomHead,
    SealedCapabilityBearerV1,
};
mod assignment_mcp {
    pub use worldstream_studio_supervisor::assignment_mcp::{
        AssignedMembershipAuthorityV1, MembershipStreamSnapshotV1,
    };
}

#[path = "../src/assignment_mcp_operations.rs"]
mod assignment_mcp_operations;

use assignment_mcp::{AssignedMembershipAuthorityV1, MembershipStreamSnapshotV1};

#[path = "../src/assignment_mcp_actions.rs"]
mod assignment_mcp_actions;

use assignment_mcp_actions::{
    AssignmentMcpActionDaemonResultV1, AssignmentMcpActionErrorV1,
    AssignmentMcpActionGatewayErrorV1, AssignmentMcpActionGatewayV1,
    AssignmentMcpActionSchemaErrorV1, AssignmentMcpActionSchemaSourceV1,
    AssignmentMcpActionSubmitResultV1, AssignmentMcpExactActionRequestV1,
    AssignmentMcpSubmitActionV1, FixedDaemonAssignmentMcpActionGatewayV1,
    list_current_action_offers, resume_reserved_action, submit_current_action,
};
use assignment_mcp_operations::FileAssignmentMcpOperationLedgerV1;

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const HEAD_HASH: &str = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const OPERATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAT";
const PRINCIPAL: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAS";
const BEARER: &str = "wsb1:abababababababababababababababababababababababababababababababab";

#[derive(Clone)]
struct FixedSchemas {
    action: ActivityPackCatalogAction,
}

#[derive(Clone, Default)]
struct RecordingGateway {
    requests: Arc<Mutex<Vec<AssignmentMcpExactActionRequestV1>>>,
}

impl AssignmentMcpActionGatewayV1 for RecordingGateway {
    fn submit_exact(
        &self,
        _authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        Ok(accepted(request, false))
    }
}

#[derive(Clone, Default)]
struct LostResponseGateway {
    requests: Arc<Mutex<Vec<AssignmentMcpExactActionRequestV1>>>,
    calls: Arc<AtomicUsize>,
}

impl AssignmentMcpActionGatewayV1 for LostResponseGateway {
    fn submit_exact(
        &self,
        _authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(AssignmentMcpActionGatewayErrorV1::Ambiguous)
        } else {
            Ok(accepted(request, true))
        }
    }
}

#[derive(Clone, Default)]
struct ExpiredGateway;

impl AssignmentMcpActionGatewayV1 for ExpiredGateway {
    fn submit_exact(
        &self,
        _authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
        Ok(AssignmentMcpActionDaemonResultV1::Rejected {
            action_id: request.action_id.clone(),
            admitted_at: "2026-08-24T12:00:00Z".to_owned(),
            code: "deadline_passed".to_owned(),
            message: "the action deadline has passed".to_owned(),
            current_room_seq: request.based_on.room_seq,
            current_action_offers: Vec::new(),
            retryable_with_same_action_id: false,
            may_submit_revised_action: true,
            duplicate: false,
            details: json!({}),
        })
    }
}

#[derive(Clone, Default)]
struct LeapAheadGateway;

impl AssignmentMcpActionGatewayV1 for LeapAheadGateway {
    fn submit_exact(
        &self,
        _authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
        let mut result = accepted(request, false);
        let AssignmentMcpActionDaemonResultV1::Accepted { committed_head, .. } = &mut result else {
            unreachable!("accepted fixture")
        };
        committed_head.room_seq += 1;
        Ok(result)
    }
}

#[derive(Clone, Default)]
struct ContradictoryStaleGateway;

impl AssignmentMcpActionGatewayV1 for ContradictoryStaleGateway {
    fn submit_exact(
        &self,
        _authority: &AssignedMembershipAuthorityV1,
        request: &AssignmentMcpExactActionRequestV1,
    ) -> Result<AssignmentMcpActionDaemonResultV1, AssignmentMcpActionGatewayErrorV1> {
        Ok(AssignmentMcpActionDaemonResultV1::Rejected {
            action_id: request.action_id.clone(),
            admitted_at: "2026-08-24T12:00:00Z".to_owned(),
            code: "stale_room_state".to_owned(),
            message: "the action was based on stale room state".to_owned(),
            current_room_seq: request.based_on.room_seq,
            current_action_offers: Vec::new(),
            retryable_with_same_action_id: true,
            may_submit_revised_action: false,
            duplicate: false,
            details: json!({}),
        })
    }
}

fn authority(assignment_id: &str) -> AssignedMembershipAuthorityV1 {
    AssignedMembershipAuthorityV1::new(
        assignment_id,
        "counter-agent",
        "r1",
        "counter",
        PRINCIPAL,
        ROOM,
        MEMBER,
        SealedCapabilityBearerV1::parse(BEARER.to_owned())
            .unwrap_or_else(|error| unreachable!("sealed bearer: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("assignment authority: {error:?}"))
}

fn accepted(
    request: &AssignmentMcpExactActionRequestV1,
    duplicate: bool,
) -> AssignmentMcpActionDaemonResultV1 {
    AssignmentMcpActionDaemonResultV1::Accepted {
        action_id: request.action_id.clone(),
        transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
        admitted_at: "2026-08-24T12:00:00Z".to_owned(),
        committed_head: assignment_mcp_actions::AssignmentMcpActionHeadV1 {
            room_seq: request.based_on.room_seq + 1,
            head_hash: "blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                .to_owned(),
        },
        duplicate,
    }
}

impl AssignmentMcpActionSchemaSourceV1 for FixedSchemas {
    fn exact_action(
        &self,
        pack: &PackReference,
        action_type: &str,
    ) -> Result<ActivityPackCatalogAction, AssignmentMcpActionSchemaErrorV1> {
        if pack.id != "counter" || pack.version != "2.0.0" || action_type != "increment" {
            return Err(AssignmentMcpActionSchemaErrorV1::Missing);
        }
        Ok(self.action.clone())
    }
}

fn payload_schema() -> ActivityPackCatalogSchema {
    let schema = json!({
        "type": "object",
        "properties": {"amount": {"type": "integer", "minimum": 1, "maximum": 5}},
        "required": ["amount"],
        "additionalProperties": false
    });
    ActivityPackCatalogSchema {
        schema_id: "counter/increment-payload/v1".to_owned(),
        schema_digest: canonical_digest(&schema),
        schema,
    }
}

fn canonical_digest(value: &Value) -> String {
    let canonical = worldstream_core::CanonicalJsonV1::parse(
        &serde_json::to_vec(value).unwrap_or_else(|error| unreachable!("schema JSON: {error}")),
    )
    .unwrap_or_else(|error| unreachable!("canonical schema: {error}"));
    worldstream_core::Blake3DigestV1::hash(
        &canonical
            .to_bytes()
            .unwrap_or_else(|error| unreachable!("canonical bytes: {error}")),
    )
    .to_string()
}

fn snapshot(offers: Vec<ActionOffer>) -> MembershipStreamSnapshotV1 {
    MembershipStreamSnapshotV1 {
        room_head: RoomHead {
            room_id: ROOM.to_owned(),
            room_seq: 7,
            genesis_or_transition_hash: HEAD_HASH.to_owned(),
            core_schema_version: "worldstream/core/v1".to_owned(),
            pack_digest: "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_owned(),
            core_state_hash:
                "blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned(),
            activity_state_hash:
                "blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_owned(),
            authoritative_state_hash:
                "blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
        },
        pack: PackReference {
            id: "counter".to_owned(),
            version: "2.0.0".to_owned(),
            digest: "blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_owned(),
        },
        cursor: Some(5),
        frame_head: 5,
        retained_floor: 1,
        current_action_offers: offers,
        projection_reset: None,
        observations: Vec::new(),
    }
}

fn increment_offer(schema: &ActivityPackCatalogSchema) -> ActionOffer {
    ActionOffer {
        domain: "worldstream/action-offer/v1".to_owned(),
        action_type: "increment".to_owned(),
        payload_schema_digest: schema.schema_digest.clone(),
        eligibility_window: None,
    }
}

#[test]
fn lists_current_offer_with_actual_declared_schema_and_exact_head_precondition() {
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let listed = list_current_action_offers(
        &current,
        &FixedSchemas {
            action: ActivityPackCatalogAction {
                action_type: "increment".to_owned(),
                payload_schema: schema.clone(),
            },
        },
    )
    .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));

    assert_eq!(listed.schema, "worldstream/assignment-action-offer-list/v1");
    assert_eq!(listed.precondition.room_seq, 7);
    assert_eq!(listed.precondition.head_hash, HEAD_HASH);
    assert_eq!(listed.offers.len(), 1);
    assert_eq!(
        listed.offers[0].offer_id,
        format!("7:0:{}", schema.schema_digest)
    );
    assert_eq!(listed.offers[0].action_type, "increment");
    assert_eq!(listed.offers[0].payload_schema, schema);
    let encoded = serde_json::to_value(listed)
        .unwrap_or_else(|error| unreachable!("encode listed offers: {error}"));
    assert!(!contains_prohibited_material(&encoded));
}

#[test]
fn refuses_to_claim_an_unsupported_declared_payload_schema() {
    let mut schema = payload_schema();
    schema.schema["oneOf"] = json!([{"type": "integer"}, {"type": "string"}]);
    schema.schema_digest = canonical_digest(&schema.schema);
    let current = snapshot(vec![increment_offer(&schema)]);
    let result = list_current_action_offers(
        &current,
        &FixedSchemas {
            action: ActivityPackCatalogAction {
                action_type: "increment".to_owned(),
                payload_schema: schema,
            },
        },
    );

    assert_eq!(result, Err(AssignmentMcpActionErrorV1::InvalidOfferData));
}

#[test]
fn submits_only_the_exact_listed_offer_with_stable_ids_and_valid_payload() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(directory.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema.clone(),
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let gateway = RecordingGateway::default();

    let result = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &gateway,
        AssignmentMcpSubmitActionV1 {
            operation_id: OPERATION.to_owned(),
            offer_id: listed.offers[0].offer_id.clone(),
            precondition: listed.precondition,
            payload: json!({"amount": 3}),
        },
    )
    .unwrap_or_else(|error| unreachable!("submit exact offer: {error:?}"));

    assert!(matches!(
        result,
        AssignmentMcpActionSubmitResultV1::Accepted {
            ref operation_id,
            ref action_id,
            ref transition_id,
            ..
        } if operation_id == OPERATION
            && action_id == OPERATION
            && transition_id == "01ARZ3NDEKTSV4RRFFQ69G5FAY"
    ));
    let requests = gateway
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].request_id, OPERATION);
    assert_eq!(requests[0].action_id, OPERATION);
    assert_eq!(requests[0].action_type, "increment");
    assert_eq!(requests[0].payload_schema_digest, schema.schema_digest);
    assert_eq!(requests[0].based_on.room_seq, 7);
    assert_eq!(requests[0].payload, json!({"amount": 3}));
    let encoded = serde_json::to_value(result)
        .unwrap_or_else(|error| unreachable!("encode submit result: {error}"));
    assert!(!contains_prohibited_material(&encoded));
}

#[test]
fn invalid_payload_and_unlisted_offer_never_reach_the_daemon() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(directory.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let gateway = RecordingGateway::default();
    let invalid = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &gateway,
        AssignmentMcpSubmitActionV1 {
            operation_id: OPERATION.to_owned(),
            offer_id: listed.offers[0].offer_id.clone(),
            precondition: listed.precondition.clone(),
            payload: json!({"amount": 6}),
        },
    );
    assert_eq!(invalid, Err(AssignmentMcpActionErrorV1::InvalidPayload));

    let unlisted = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &gateway,
        AssignmentMcpSubmitActionV1 {
            operation_id: "01ARZ3NDEKTSV4RRFFQ69G5FB0".to_owned(),
            offer_id:
                "7:99:blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
            precondition: listed.precondition,
            payload: json!({"amount": 3}),
        },
    );
    assert_eq!(unlisted, Err(AssignmentMcpActionErrorV1::Unoffered));
    assert!(
        gateway
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
}

#[test]
fn stale_head_and_expired_offer_return_structured_refresh_without_redirect() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(directory.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let gateway = RecordingGateway::default();
    let mut stale_precondition = listed.precondition.clone();
    stale_precondition.room_seq -= 1;
    let stale = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &gateway,
        AssignmentMcpSubmitActionV1 {
            operation_id: OPERATION.to_owned(),
            offer_id: listed.offers[0].offer_id.clone(),
            precondition: stale_precondition,
            payload: json!({"amount": 3}),
        },
    )
    .unwrap_or_else(|error| unreachable!("stale result: {error:?}"));
    assert!(matches!(
        stale,
        AssignmentMcpActionSubmitResultV1::RefreshRequired {
            reason: assignment_mcp_actions::AssignmentMcpActionRefreshReasonV1::StaleHead,
            current_room_seq: 7,
            ..
        }
    ));
    assert!(
        gateway
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );

    let expired = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &ExpiredGateway,
        AssignmentMcpSubmitActionV1 {
            operation_id: "01ARZ3NDEKTSV4RRFFQ69G5FB1".to_owned(),
            offer_id: listed.offers[0].offer_id.clone(),
            precondition: listed.precondition,
            payload: json!({"amount": 3}),
        },
    )
    .unwrap_or_else(|error| unreachable!("expired result: {error:?}"));
    assert!(matches!(
        expired,
        AssignmentMcpActionSubmitResultV1::RefreshRequired {
            reason: assignment_mcp_actions::AssignmentMcpActionRefreshReasonV1::OfferExpired,
            current_head_hash: None,
            ..
        }
    ));
    assert!(!contains_prohibited_material(
        &serde_json::to_value(expired)
            .unwrap_or_else(|error| unreachable!("encode refresh: {error}"))
    ));
}

#[test]
fn lost_response_restart_reissues_identical_action_and_completed_retry_is_local() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger_root = directory.path().join("operations");
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let arguments = AssignmentMcpSubmitActionV1 {
        operation_id: OPERATION.to_owned(),
        offer_id: listed.offers[0].offer_id.clone(),
        precondition: listed.precondition,
        payload: json!({"amount": 3}),
    };
    let gateway = LostResponseGateway::default();
    {
        let ledger = FileAssignmentMcpOperationLedgerV1::open(&ledger_root)
            .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
        assert_eq!(
            submit_current_action(
                &authority(ASSIGNMENT),
                &current,
                &schemas,
                &ledger,
                &gateway,
                arguments.clone(),
            ),
            Err(AssignmentMcpActionErrorV1::AmbiguousRetrySameOperation)
        );
    }

    let restarted = FileAssignmentMcpOperationLedgerV1::open(&ledger_root)
        .unwrap_or_else(|error| unreachable!("restart operation ledger: {error:?}"));
    let advanced = snapshot(Vec::new());
    let resolved = submit_current_action(
        &authority(ASSIGNMENT),
        &advanced,
        &schemas,
        &restarted,
        &gateway,
        arguments.clone(),
    )
    .unwrap_or_else(|error| unreachable!("reconcile lost response: {error:?}"));
    assert!(matches!(
        resolved,
        AssignmentMcpActionSubmitResultV1::Accepted {
            duplicate: true,
            ..
        }
    ));
    let local_replay = submit_current_action(
        &authority(ASSIGNMENT),
        &advanced,
        &schemas,
        &restarted,
        &gateway,
        arguments,
    )
    .unwrap_or_else(|error| unreachable!("local completed replay: {error:?}"));
    assert_eq!(local_replay, resolved);
    let requests = gateway
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[test]
fn managed_restart_reconciles_the_retained_action_without_another_offer_snapshot() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger_root = directory.path().join("operations");
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let arguments = AssignmentMcpSubmitActionV1 {
        operation_id: OPERATION.to_owned(),
        offer_id: listed.offers[0].offer_id.clone(),
        precondition: listed.precondition,
        payload: json!({"amount": 3}),
    };
    let gateway = LostResponseGateway::default();
    {
        let ledger = FileAssignmentMcpOperationLedgerV1::open(&ledger_root)
            .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
        assert_eq!(
            submit_current_action(
                &authority(ASSIGNMENT),
                &current,
                &schemas,
                &ledger,
                &gateway,
                arguments,
            ),
            Err(AssignmentMcpActionErrorV1::AmbiguousRetrySameOperation)
        );
    }

    let restarted = FileAssignmentMcpOperationLedgerV1::open(&ledger_root)
        .unwrap_or_else(|error| unreachable!("restart operation ledger: {error:?}"));
    let accepted = resume_reserved_action(&authority(ASSIGNMENT), &restarted, &gateway, OPERATION)
        .unwrap_or_else(|error| unreachable!("managed action recovery: {error:?}"));
    assert!(matches!(
        accepted,
        AssignmentMcpActionSubmitResultV1::Accepted {
            duplicate: true,
            ..
        }
    ));
    assert_eq!(
        resume_reserved_action(&authority(ASSIGNMENT), &restarted, &gateway, OPERATION)
            .unwrap_or_else(|error| unreachable!("local recovery replay: {error:?}")),
        accepted
    );
    let requests = gateway
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[test]
fn assignment_scoped_identity_conflicts_never_disclose_or_redirect() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(directory.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let gateway = RecordingGateway::default();
    let arguments = AssignmentMcpSubmitActionV1 {
        operation_id: OPERATION.to_owned(),
        offer_id: listed.offers[0].offer_id.clone(),
        precondition: listed.precondition,
        payload: json!({"amount": 3}),
    };
    let first = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &gateway,
        arguments.clone(),
    )
    .unwrap_or_else(|error| unreachable!("first assignment: {error:?}"));
    let second = submit_current_action(
        &authority("01ARZ3NDEKTSV4RRFFQ69G5FB2"),
        &current,
        &schemas,
        &ledger,
        &gateway,
        arguments,
    )
    .unwrap_or_else(|error| unreachable!("second assignment: {error:?}"));
    assert_eq!(first, second);
    let encoded = serde_json::to_value([first, second])
        .unwrap_or_else(|error| unreachable!("encode isolated results: {error}"));
    assert!(!contains_prohibited_material(&encoded));
    let requests = gateway
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].assignment_id, requests[1].assignment_id);
    assert_eq!(requests[0].action_type, requests[1].action_type);
}

#[test]
fn mismatched_room_and_incoherent_daemon_receipts_fail_closed() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(directory.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let arguments = |operation_id: &str| AssignmentMcpSubmitActionV1 {
        operation_id: operation_id.to_owned(),
        offer_id: listed.offers[0].offer_id.clone(),
        precondition: listed.precondition.clone(),
        payload: json!({"amount": 3}),
    };
    let mut other_room = current.clone();
    other_room.room_head.room_id = "01ARZ3NDEKTSV4RRFFQ69G5FB3".to_owned();
    assert_eq!(
        submit_current_action(
            &authority(ASSIGNMENT),
            &other_room,
            &schemas,
            &ledger,
            &RecordingGateway::default(),
            arguments("01ARZ3NDEKTSV4RRFFQ69G5FB4"),
        ),
        Err(AssignmentMcpActionErrorV1::InvalidOfferData)
    );
    assert_eq!(
        submit_current_action(
            &authority(ASSIGNMENT),
            &current,
            &schemas,
            &ledger,
            &LeapAheadGateway,
            arguments("01ARZ3NDEKTSV4RRFFQ69G5FB5"),
        ),
        Err(AssignmentMcpActionErrorV1::InvalidDaemonData)
    );
    assert_eq!(
        submit_current_action(
            &authority(ASSIGNMENT),
            &current,
            &schemas,
            &ledger,
            &ContradictoryStaleGateway,
            arguments("01ARZ3NDEKTSV4RRFFQ69G5FB6"),
        ),
        Err(AssignmentMcpActionErrorV1::InvalidDaemonData)
    );
}

#[test]
fn fixed_daemon_gateway_authenticates_syncs_and_submits_exact_stable_action() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| unreachable!("bind Action fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("Action fixture address: {error}"));
    let fixture = thread::spawn(move || serve_action_fixture(&listener));
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let ledger = FileAssignmentMcpOperationLedgerV1::open(directory.path().join("operations"))
        .unwrap_or_else(|error| unreachable!("operation ledger: {error:?}"));
    let schema = payload_schema();
    let current = snapshot(vec![increment_offer(&schema)]);
    let schemas = FixedSchemas {
        action: ActivityPackCatalogAction {
            action_type: "increment".to_owned(),
            payload_schema: schema,
        },
    };
    let listed = list_current_action_offers(&current, &schemas)
        .unwrap_or_else(|error| unreachable!("list offers: {error:?}"));
    let gateway = FixedDaemonAssignmentMcpActionGatewayV1::new(address, Duration::from_secs(2))
        .unwrap_or_else(|error| unreachable!("fixed daemon gateway: {error:?}"));
    let result = submit_current_action(
        &authority(ASSIGNMENT),
        &current,
        &schemas,
        &ledger,
        &gateway,
        AssignmentMcpSubmitActionV1 {
            operation_id: OPERATION.to_owned(),
            offer_id: listed.offers[0].offer_id.clone(),
            precondition: listed.precondition,
            payload: json!({"amount": 3}),
        },
    )
    .unwrap_or_else(|error| unreachable!("production Action submit: {error:?}"));
    assert!(matches!(
        result,
        AssignmentMcpActionSubmitResultV1::Accepted {
            ref action_id,
            ref transition_id,
            duplicate: false,
            ..
        } if action_id == OPERATION && transition_id == "01ARZ3NDEKTSV4RRFFQ69G5FAY"
    ));
    fixture
        .join()
        .unwrap_or_else(|error| unreachable!("Action fixture thread: {error:?}"));
}

#[test]
fn fixed_daemon_gateway_classifies_handshake_failures_without_false_revocation() {
    for (status, expected) in [
        (
            "401 Unauthorized",
            AssignmentMcpActionGatewayErrorV1::Revoked,
        ),
        ("403 Forbidden", AssignmentMcpActionGatewayErrorV1::Revoked),
        (
            "429 Too Many Requests",
            AssignmentMcpActionGatewayErrorV1::Unavailable,
        ),
        (
            "503 Service Unavailable",
            AssignmentMcpActionGatewayErrorV1::Unavailable,
        ),
        (
            "404 Not Found",
            AssignmentMcpActionGatewayErrorV1::InvalidData,
        ),
    ] {
        let (address, fixture) = handshake_failure_fixture(Some(status));
        let gateway = FixedDaemonAssignmentMcpActionGatewayV1::new(address, Duration::from_secs(2))
            .unwrap_or_else(|error| unreachable!("fixed daemon gateway: {error:?}"));
        assert_eq!(
            gateway.submit_exact(&authority(ASSIGNMENT), &exact_gateway_request()),
            Err(expected),
            "unexpected classification for HTTP {status}"
        );
        fixture
            .join()
            .unwrap_or_else(|error| unreachable!("handshake fixture thread: {error:?}"));
    }

    let (address, fixture) = handshake_failure_fixture(None);
    let gateway = FixedDaemonAssignmentMcpActionGatewayV1::new(address, Duration::from_secs(2))
        .unwrap_or_else(|error| unreachable!("fixed daemon gateway: {error:?}"));
    assert_eq!(
        gateway.submit_exact(&authority(ASSIGNMENT), &exact_gateway_request()),
        Err(AssignmentMcpActionGatewayErrorV1::Disconnected)
    );
    fixture
        .join()
        .unwrap_or_else(|error| unreachable!("disconnect fixture thread: {error:?}"));
}

fn exact_gateway_request() -> AssignmentMcpExactActionRequestV1 {
    let schema = payload_schema();
    AssignmentMcpExactActionRequestV1 {
        assignment_id: ASSIGNMENT.to_owned(),
        operation_id: OPERATION.to_owned(),
        request_id: OPERATION.to_owned(),
        action_id: OPERATION.to_owned(),
        based_on: assignment_mcp_actions::AssignmentMcpActionHeadV1 {
            room_seq: 7,
            head_hash: HEAD_HASH.to_owned(),
        },
        offer_id: format!("7:0:{}", schema.schema_digest),
        action_type: "increment".to_owned(),
        payload_schema_digest: schema.schema_digest,
        eligibility_window: None,
        payload: json!({"amount": 3}),
    }
}

fn handshake_failure_fixture(
    status: Option<&'static str>,
) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| unreachable!("bind handshake fixture: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| unreachable!("handshake fixture address: {error}"));
    let fixture = thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .unwrap_or_else(|error| unreachable!("accept handshake fixture: {error}"));
        let mut request = [0_u8; 4096];
        let _ = stream
            .read(&mut request)
            .unwrap_or_else(|error| unreachable!("read handshake fixture: {error}"));
        if let Some(status) = status {
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap_or_else(|error| unreachable!("write handshake fixture: {error}"));
        }
    });
    (address, fixture)
}

fn serve_action_fixture(listener: &TcpListener) {
    let (stream, _) = listener
        .accept()
        .unwrap_or_else(|error| unreachable!("accept Action fixture: {error}"));
    let mut socket = tungstenite::accept_hdr(stream, authorize_action_fixture)
        .unwrap_or_else(|error| unreachable!("Action fixture handshake: {error}"));
    let hello = read_fixture(&mut socket);
    assert_eq!(hello["type"], "client.hello");
    send_fixture(
        &mut socket,
        "server.welcome",
        &json!({
            "session_id":"01ARZ3NDEKTSV4RRFFQ69G5FAZ",
            "selected_protocol":worldstream_protocol::PROTOCOL_VERSION,
            "server_version":"fixture",
            "heartbeat_interval_ms":1000,
            "maximum_message_bytes":worldstream_protocol::MAX_MESSAGE_BYTES,
            "authenticated_principal":{"principal_id":PRINCIPAL,"kind":"agent"}
        }),
    );
    let attach = read_fixture(&mut socket);
    assert_eq!(attach["type"], "room.attach");
    assert_eq!(attach["body"]["room_id"], ROOM);
    assert_eq!(attach["body"]["member_id"], MEMBER);
    send_fixture(
        &mut socket,
        "room.attached",
        &json!({
            "room_id":ROOM,"member_id":MEMBER,"principal_kind":"agent","access_mode":"participant",
            "role":"counter","membership_status":"enabled","room_status":"active",
            "room_health":"healthy","integrity_generation":1,"room_head":snapshot(Vec::new()).room_head,
            "cursor":null,"frame_head":0,"retained_floor":0,"sync_token":"action-fixture-sync",
            "sync":{"kind":"retained_frames","cursor_exclusive":0,"through_frame_head":0},
            "pack":{"id":"counter","version":"2.0.0","digest":"blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
        }),
    );
    let sync_ack = read_fixture(&mut socket);
    assert_eq!(sync_ack["type"], "room.sync_ack");
    send_fixture(
        &mut socket,
        "room.sync_acked",
        &json!({"through_frame_head":0}),
    );
    let action = read_fixture(&mut socket);
    assert_eq!(action["type"], "action.submit");
    assert_eq!(action["request_id"], OPERATION);
    assert_eq!(action["body"]["room_id"], ROOM);
    assert_eq!(action["body"]["member_id"], MEMBER);
    assert_eq!(action["body"]["action_id"], OPERATION);
    assert_eq!(action["body"]["based_on_room_seq"], 7);
    assert_eq!(action["body"]["action_type"], "increment");
    assert_eq!(action["body"]["payload"], json!({"amount":3}));
    send_fixture(
        &mut socket,
        "action.accepted",
        &json!({
            "room_id":ROOM,"member_id":MEMBER,"action_id":OPERATION,
            "transition_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","admitted_at":"2026-08-24T12:00:00Z",
            "room_head":{
                "room_id":ROOM,"room_seq":8,
                "genesis_or_transition_hash":"blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "core_schema_version":"worldstream/core/v1",
                "pack_digest":"blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "core_state_hash":"blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "activity_state_hash":"blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "authoritative_state_hash":"blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            },
            "duplicate":false
        }),
    );
}

#[expect(
    clippy::result_large_err,
    clippy::unnecessary_wraps,
    reason = "the callback result is fixed by tungstenite's handshake API"
)]
fn authorize_action_fixture(
    request: &tungstenite::handshake::server::Request,
    mut response: tungstenite::handshake::server::Response,
) -> Result<tungstenite::handshake::server::Response, tungstenite::handshake::server::ErrorResponse>
{
    let expected = format!("Bearer {BEARER}");
    assert_eq!(
        request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some(expected.as_str())
    );
    response.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        worldstream_protocol::WEBSOCKET_SUBPROTOCOL
            .parse()
            .unwrap_or_else(|error| unreachable!("fixture subprotocol: {error}")),
    );
    Ok(response)
}

fn read_fixture(socket: &mut tungstenite::WebSocket<std::net::TcpStream>) -> Value {
    let message = socket
        .read()
        .unwrap_or_else(|error| unreachable!("read fixture message: {error}"));
    let tungstenite::Message::Text(text) = message else {
        unreachable!("fixture expected text message")
    };
    serde_json::from_str(&text)
        .unwrap_or_else(|error| unreachable!("fixture message JSON: {error}"))
}

fn send_fixture(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    kind: &str,
    body: &Value,
) {
    socket
        .send(tungstenite::Message::Text(
            json!({
                "protocol":worldstream_protocol::PROTOCOL_VERSION,"type":kind,
                "message_id":"01ARZ3NDEKTSV4RRFFQ69G5FB3","request_id":OPERATION,"body":body
            })
            .to_string()
            .into(),
        ))
        .unwrap_or_else(|error| unreachable!("send fixture message: {error}"));
}

fn contains_prohibited_material(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            matches!(
                key.as_str(),
                "room_id"
                    | "member_id"
                    | "principal_id"
                    | "bearer"
                    | "authority"
                    | "secret"
                    | "path"
            ) || contains_prohibited_material(child)
        }),
        Value::Array(values) => values.iter().any(contains_prohibited_material),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}
