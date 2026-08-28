#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    http::{Method, Request},
};
use http_body_util::BodyExt as _;
use tempfile::TempDir;
use tower::ServiceExt as _;
use worldstream_protocol::OperatorRunnerConnectionV1;
use worldstream_studio_supervisor::{
    attention_inbox::{
        AttentionConditionV1, AttentionInboxErrorV1, AttentionInboxSnapshotV1,
        AttentionInboxSourceV1, AttentionInboxV1, AttentionTargetKindV1, FileAttentionHistoryV1,
        attention_inbox_router,
    },
    backups::{
        BackupFreshnessV1, BackupOperationPhaseV1, BackupOperationStatusV1, BackupStorageHealthV1,
        BackupStorageProfileV1, BackupVerificationV1,
    },
    lifecycle::{DaemonLifecycleFailureV1, DaemonLifecycleStateV1, DaemonLifecycleV1},
    room_creation::{RoomCreationAttentionV1, RoomCreationStateV1, RoomCreationStatusV1},
    runner_attention::{
        ActivationAttentionStateV1, ActivationAttentionV1, AgentSeatAttentionV1,
        RunnerAttentionCapacityV1, RunnerAttentionFreshnessV1, RunnerAttentionOperationsResponseV1,
        RunnerAttentionStatusV1, RunnerCompatibilityV1, TaskAgentAttentionResponseV1,
    },
    task_setup::TaskSetupStatusV1,
};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";

#[derive(Clone)]
struct FakeSource(Arc<Mutex<Result<AttentionInboxSnapshotV1, AttentionInboxErrorV1>>>);

impl AttentionInboxSourceV1 for FakeSource {
    fn snapshot(&self) -> Result<AttentionInboxSnapshotV1, AttentionInboxErrorV1> {
        self.0.lock().expect("source").clone()
    }
}

#[test]
fn creates_and_deduplicates_stable_conditions_from_existing_safe_projections() {
    let temporary = TempDir::new().expect("temporary");
    let mut snapshot = attention_snapshot(1_000);
    snapshot.runner_operations.runners[0].observed_at_unix_ms = Some(500);
    snapshot.task_attention[0].1.observed_at_unix_ms = Some(500);
    let source = FakeSource(Arc::new(Mutex::new(Ok(snapshot))));
    let inbox = open(&temporary, source.clone());

    let first = inbox.refresh().expect("first inbox");
    assert_eq!(first.items.len(), 6);
    assert!(first.items.iter().any(|item| {
        item.condition == AttentionConditionV1::DaemonLifecycle
            && item.target_kind == AttentionTargetKindV1::Process
    }));
    assert!(first.items.iter().any(|item| {
        item.condition == AttentionConditionV1::TaskSetup
            && item.target_kind == AttentionTargetKindV1::Task
    }));
    let ids = first
        .items
        .iter()
        .map(|item| item.attention_id.clone())
        .collect::<Vec<_>>();

    source
        .0
        .lock()
        .expect("source")
        .as_mut()
        .expect("snapshot")
        .observed_at_unix_ms = 2_000;
    let duplicate = inbox.refresh().expect("deduplicated inbox");
    assert_eq!(
        duplicate
            .items
            .iter()
            .map(|item| item.attention_id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    let runner = duplicate
        .items
        .iter()
        .find(|item| item.condition == AttentionConditionV1::RunnerPresence)
        .expect("Runner condition");
    assert_eq!(runner.first_seen_at_unix_ms, 1_000);
    assert_eq!(runner.last_seen_at_unix_ms, 500);
    let lease = duplicate
        .items
        .iter()
        .find(|item| item.condition == AttentionConditionV1::ActivationLease)
        .expect("Activation condition");
    assert_eq!(lease.last_seen_at_unix_ms, 500);
    let lifecycle = duplicate
        .items
        .iter()
        .find(|item| item.condition == AttentionConditionV1::DaemonLifecycle)
        .expect("lifecycle condition");
    assert_eq!(lifecycle.last_seen_at_unix_ms, 2_000);
    assert!(duplicate.recently_resolved.is_empty());
}

#[test]
fn room_creation_and_backup_failures_are_stable_safe_and_resolve() {
    let temporary = TempDir::new().expect("temporary");
    let mut snapshot = healthy_snapshot(1_000);
    snapshot.room_creations = vec![failed_room_creation()];
    snapshot.backup_operations = vec![failed_backup()];
    let source = FakeSource(Arc::new(Mutex::new(Ok(snapshot))));
    let inbox = open(&temporary, source.clone());

    let first = inbox.refresh().expect("operation failures");
    assert_eq!(first.items.len(), 2);
    let creation = first
        .items
        .iter()
        .find(|item| item.condition == AttentionConditionV1::RoomCreation)
        .expect("Room creation condition");
    assert_eq!(creation.deep_link, "#room-creation");
    assert_eq!(creation.target_id, "new-room");
    let backup = first
        .items
        .iter()
        .find(|item| item.condition == AttentionConditionV1::BackupOperation)
        .expect("backup condition");
    assert_eq!(backup.deep_link, "#backups");
    assert_eq!(backup.target_id, "backup-01");
    let stable_ids = first
        .items
        .iter()
        .map(|item| item.attention_id.clone())
        .collect::<Vec<_>>();

    source
        .0
        .lock()
        .expect("source")
        .as_mut()
        .expect("snapshot")
        .observed_at_unix_ms = 2_000;
    let duplicate = inbox.refresh().expect("deduplicated failures");
    assert_eq!(
        duplicate
            .items
            .iter()
            .map(|item| item.attention_id.clone())
            .collect::<Vec<_>>(),
        stable_ids
    );
    *source.0.lock().expect("source") = Ok(healthy_snapshot(3_000));
    let resolved = inbox.refresh().expect("resolved failures");
    assert!(resolved.items.is_empty());
    assert_eq!(resolved.recently_resolved.len(), 2);
    let encoded = serde_json::to_string(&first)
        .expect("safe response")
        .to_ascii_lowercase();
    for forbidden in ["wsb1:", "bearer ", "secret_reference", "/private/"] {
        assert!(!encoded.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn resolution_leaves_active_inbox_and_retains_only_a_minimal_transition() {
    let temporary = TempDir::new().expect("temporary");
    let source = FakeSource(Arc::new(Mutex::new(Ok(attention_snapshot(1_000)))));
    let inbox = open(&temporary, source.clone());
    let original = inbox.refresh().expect("original");

    *source.0.lock().expect("source") = Ok(healthy_snapshot(3_000));
    let resolved = inbox.refresh().expect("resolved");
    assert!(resolved.items.is_empty());
    assert_eq!(resolved.recently_resolved.len(), original.items.len());
    assert!(
        resolved
            .recently_resolved
            .iter()
            .all(|item| item.resolved_at_unix_ms == 3_000)
    );
    let encoded = serde_json::to_value(&resolved.recently_resolved).expect("history JSON");
    assert!(encoded[0].get("reason").is_none());
    assert!(encoded[0].get("next_action").is_none());
    let persisted =
        std::fs::read_to_string(temporary.path().join("attention").join("history.json"))
            .expect("persisted minimal history");
    for discarded in [
        "configured daemon needs attention",
        "retry configured daemon start",
        "deep_link",
        "next_action",
        "reason",
        "title",
    ] {
        assert!(
            !persisted.to_ascii_lowercase().contains(discarded),
            "resolved history retained {discarded}"
        );
    }

    *source.0.lock().expect("source") = Ok(attention_snapshot(5_000));
    let reopened = inbox.refresh().expect("reopened");
    assert_eq!(reopened.items[0].first_seen_at_unix_ms, 5_000);
    assert!(reopened.items.iter().all(|item| {
        original
            .items
            .iter()
            .any(|old| old.attention_id == item.attention_id)
    }));
}

#[test]
fn stale_items_keep_closed_deep_links_and_redact_sensitive_source_text() {
    let temporary = TempDir::new().expect("temporary");
    let inbox = open(
        &temporary,
        FakeSource(Arc::new(Mutex::new(Ok(attention_snapshot(1_000))))),
    );
    let response = inbox.refresh().expect("inbox");
    assert!(response.items.iter().any(|item| {
        item.condition == AttentionConditionV1::RunnerPresence
            && item.freshness == RunnerAttentionFreshnessV1::Stale
    }));
    assert!(response.items.iter().all(|item| matches!(
        item.deep_link.as_str(),
        "#operations"
            | "#task-setup"
            | "#runner-attention"
            | "#tasks"
            | "#room-creation"
            | "#backups"
    )));
    let encoded = serde_json::to_string(&response)
        .expect("response JSON")
        .to_ascii_lowercase();
    for forbidden in [
        "wsb1:",
        "bearer ",
        "secret_reference",
        "token_hash",
        "invocation",
        "payload",
        "prompt",
        "memory",
    ] {
        assert!(!encoded.contains(forbidden), "{forbidden}");
    }
    assert!(encoded.contains("configured daemon needs attention"));
}

#[tokio::test]
async fn router_is_read_only_and_returns_no_dismissal_mutation() {
    let temporary = TempDir::new().expect("temporary");
    let router = attention_inbox_router(open(
        &temporary,
        FakeSource(Arc::new(Mutex::new(Ok(attention_snapshot(1_000))))),
    ));
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/attention-inbox")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), 200);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("worldstream/studio-attention-inbox/v1"));

    let mutation = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/attention-inbox")
                .body(Body::from("{\"dismiss\":true}"))
                .expect("mutation request"),
        )
        .await
        .expect("mutation response");
    assert_eq!(mutation.status(), 405);
}

#[test]
fn corrupt_history_fails_closed() {
    let temporary = TempDir::new().expect("temporary");
    let history_root = temporary.path().join("attention");
    let inbox = AttentionInboxV1::new(
        FakeSource(Arc::new(Mutex::new(Ok(attention_snapshot(1_000))))),
        FileAttentionHistoryV1::open(&history_root).expect("history"),
    );
    inbox.refresh().expect("initial");
    std::fs::write(history_root.join("history.json"), b"{}").expect("corrupt history");
    assert_eq!(inbox.refresh(), Err(AttentionInboxErrorV1::InvalidData));
}

fn open(temporary: &TempDir, source: FakeSource) -> AttentionInboxV1<FakeSource> {
    AttentionInboxV1::new(
        source,
        FileAttentionHistoryV1::open(&temporary.path().join("attention")).expect("history"),
    )
}

fn attention_snapshot(observed_at_unix_ms: u64) -> AttentionInboxSnapshotV1 {
    let mut snapshot = healthy_snapshot(observed_at_unix_ms);
    snapshot.lifecycle = DaemonLifecycleV1 {
        schema: "worldstream/studio-daemon-lifecycle/v1".to_owned(),
        state: DaemonLifecycleStateV1::Failed,
        operation_id: 3,
        managed_by_supervisor: true,
        failure: Some(DaemonLifecycleFailureV1 {
            code: "start_failed".to_owned(),
            explanation: "Bearer wsb1:private should not be displayed".to_owned(),
            next_action: "Retry configured daemon start.".to_owned(),
        }),
    };
    snapshot.task_setups = vec![setup_needing_attention()];
    snapshot
        .runner_operations
        .runners
        .push(RunnerAttentionStatusV1 {
            runner_id: RUNNER.to_owned(),
            instance_id: Some("managed-runner-01".to_owned()),
            connection: OperatorRunnerConnectionV1::Connected,
            freshness: RunnerAttentionFreshnessV1::Stale,
            capacity: RunnerAttentionCapacityV1 {
                advertised: 2,
                in_use: 2,
                available: 0,
            },
            compatible_assignments: 1,
            incompatible_assignments: 1,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            next_action: "Restore compatible Runner capacity.".to_owned(),
        });
    snapshot.task_attention.push((
        ROOM.to_owned(),
        TaskAgentAttentionResponseV1 {
            schema: "worldstream/studio-task-agent-attention/v1".to_owned(),
            freshness: RunnerAttentionFreshnessV1::Stale,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            seats: vec![AgentSeatAttentionV1 {
                seat_id: "navigator-agent".to_owned(),
                instance_id: Some("managed-runner-01".to_owned()),
                compatibility: RunnerCompatibilityV1::Compatible,
                capacity: RunnerAttentionCapacityV1 {
                    advertised: 2,
                    in_use: 2,
                    available: 0,
                },
                activation: ActivationAttentionV1 {
                    state: ActivationAttentionStateV1::Delayed,
                    waiting: 0,
                    leased: 1,
                },
                freshness: RunnerAttentionFreshnessV1::Stale,
                next_action: "Wait for lease reconciliation.".to_owned(),
            }],
        },
    ));
    snapshot
}

fn healthy_snapshot(observed_at_unix_ms: u64) -> AttentionInboxSnapshotV1 {
    AttentionInboxSnapshotV1 {
        observed_at_unix_ms,
        lifecycle: DaemonLifecycleV1 {
            schema: "worldstream/studio-daemon-lifecycle/v1".to_owned(),
            state: DaemonLifecycleStateV1::Running,
            operation_id: 2,
            managed_by_supervisor: true,
            failure: None,
        },
        task_setups: Vec::new(),
        room_creations: Vec::new(),
        backup_operations: Vec::new(),
        runner_operations: RunnerAttentionOperationsResponseV1 {
            schema: "worldstream/studio-runner-attention/v1".to_owned(),
            freshness: RunnerAttentionFreshnessV1::Live,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            runners: Vec::new(),
            managed_hosts: Vec::new(),
            restart_attempts: Vec::new(),
        },
        task_attention: Vec::new(),
    }
}

fn failed_room_creation() -> RoomCreationStatusV1 {
    RoomCreationStatusV1 {
        version: "studio_room_creation.v1".to_owned(),
        draft_id: "new-room".to_owned(),
        operation_id: "room-creation-01".to_owned(),
        idempotency_key: "room-creation-key-01".to_owned(),
        review_hash: format!("blake3:{}", "a".repeat(64)),
        intent_hash: format!("blake3:{}", "b".repeat(64)),
        state: RoomCreationStateV1::NeedsAttention,
        attempts: 1,
        room_id: None,
        attention: Some(RoomCreationAttentionV1 {
            code: "operator_fix_required".to_owned(),
            message: "Bearer wsb1:private /private/config should not be displayed".to_owned(),
            retryable: true,
        }),
    }
}

fn failed_backup() -> BackupOperationStatusV1 {
    BackupOperationStatusV1 {
        schema: "worldstream/studio-backup-operation/v1".to_owned(),
        operation_id: "backup-01".to_owned(),
        storage_profile: BackupStorageProfileV1::SqliteBundled,
        storage_health: BackupStorageHealthV1::Healthy,
        phase: BackupOperationPhaseV1::Failed,
        native_verification: BackupVerificationV1::Failed,
        semantic_verification: BackupVerificationV1::Unavailable,
        semantic_verification_reason: Some("verification_failed".to_owned()),
        freshness: BackupFreshnessV1::Unavailable {
            reason: "Bearer wsb1:private should not be displayed".to_owned(),
        },
        destination: None,
        failure_reason: Some("Bearer wsb1:private /private/backup".to_owned()),
    }
}

fn setup_needing_attention() -> TaskSetupStatusV1 {
    serde_json::from_value(serde_json::json!({
        "version":"studio_task_setup.v1",
        "draft_id":"new-room",
        "operation_id":"01ARZ3NDEKTSV4RRFFQ69G5FB2",
        "room_id":ROOM,
        "state":"needs_attention",
        "attempts":1,
        "completed_stages":0,
        "total_stages":0,
        "active_stage":null,
        "attention":{"code":"operator_fix_required","message":"Task setup needs a local fix.","retryable":true},
        "seats":[],
        "readiness":{"ready_to_launch":false,"seats":[]},
        "launch_applicability":"unknown",
        "launch":null
    }))
    .expect("setup status")
}
