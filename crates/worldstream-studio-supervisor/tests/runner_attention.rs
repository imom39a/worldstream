#![allow(clippy::expect_used, clippy::panic, clippy::result_large_err)]

use std::{
    fs,
    io::{Read as _, Write as _},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use axum::{
    body::Body,
    http::{Method, Request},
};
use http_body_util::BodyExt as _;
use tempfile::TempDir;
use tower::ServiceExt as _;
use worldstream_protocol::{
    OperatorActivationStatusV1, OperatorRunnerConnectionV1, OperatorRunnerFreshnessV1,
    PackReference,
};
use worldstream_studio_supervisor::runner_attention::{
    ActivationAttentionStateV1, AgentSeatAssignmentSourceV1, AgentSeatAssignmentV1,
    ApprovedRunnerRestartV1, AuthoritativeRunnerSnapshotV1, DaemonRunnerAttentionSourceV1,
    FileRunnerRestartStoreV1, HttpDaemonRunnerAttentionSourceV1, LiveRunnerAttentionSourceV1,
    LocalRunnerInstanceSnapshotV1, LocalRunnerStateV1, RunnerAttentionErrorV1,
    RunnerAttentionSourceErrorV1, RunnerAttentionSourceV1, RunnerAttentionSupervisorV1,
    RunnerCompatibilityV1, RunnerPresenceSnapshotV1, RunnerRestartControlErrorV1,
    RunnerRestartOperationStateV1, SeatActivationAggregateV1, runner_attention_router,
};
use worldstream_studio_supervisor::runner_templates::{
    RunnerSupervisorV1, RunnerTemplateRegistryV1,
};
use worldstream_studio_supervisor::secrets::{FileSecretVaultV1, SecretKindV1};

const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAY";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const OPERATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";

#[derive(Clone)]
struct FakeSource(Arc<Mutex<Result<AuthoritativeRunnerSnapshotV1, RunnerAttentionSourceErrorV1>>>);

impl RunnerAttentionSourceV1 for FakeSource {
    fn snapshot(&self) -> Result<AuthoritativeRunnerSnapshotV1, RunnerAttentionSourceErrorV1> {
        self.0.lock().expect("source").clone()
    }
}

#[derive(Clone)]
struct FakeRestart(Arc<Mutex<Result<(), RunnerRestartControlErrorV1>>>);

impl ApprovedRunnerRestartV1 for FakeRestart {
    fn restart(&self, _instance_id: &str) -> Result<(), RunnerRestartControlErrorV1> {
        *self.0.lock().expect("restart")
    }
}

#[derive(Clone)]
struct StaticAssignments(Vec<AgentSeatAssignmentV1>);

impl AgentSeatAssignmentSourceV1 for StaticAssignments {
    fn assignments(&self) -> Result<Vec<AgentSeatAssignmentV1>, RunnerAttentionSourceErrorV1> {
        Ok(self.0.clone())
    }
}

#[derive(Clone)]
struct MissingManagedRunnerDaemon;

impl DaemonRunnerAttentionSourceV1 for MissingManagedRunnerDaemon {
    fn presence(
        &self,
        _runner_id: &str,
    ) -> Result<RunnerPresenceSnapshotV1, RunnerAttentionSourceErrorV1> {
        Err(RunnerAttentionSourceErrorV1::Unavailable)
    }

    fn activation_counts(
        &self,
        _room_id: &str,
        _member_id: &str,
    ) -> Result<OperatorActivationStatusV1, RunnerAttentionSourceErrorV1> {
        Ok(OperatorActivationStatusV1 {
            version: "worldstream/operator-activation-status/v1".to_owned(),
            waiting: 2,
            leased: 0,
            observed_at_unix_ms: 2_000,
        })
    }
}

#[test]
fn stale_presence_and_capacity_exhaustion_are_actionable_without_private_data() {
    let temporary = TempDir::new().expect("temporary");
    let mut snapshot = healthy_snapshot();
    snapshot.runners[0].freshness = OperatorRunnerFreshnessV1::Stale;
    snapshot.runners[0].active_activations = 2;
    snapshot.runners[0].available_activations = 0;
    snapshot.activations[0].waiting = 3;
    let supervisor = open(&temporary, snapshot, Ok(()));

    let task = supervisor.task(ROOM).expect("task status");
    assert_eq!(task.seats.len(), 1);
    assert_eq!(
        task.seats[0].compatibility,
        RunnerCompatibilityV1::Compatible
    );
    assert_eq!(task.seats[0].capacity.available, 0);
    assert_eq!(
        task.seats[0].activation.state,
        ActivationAttentionStateV1::Attention
    );
    assert_eq!(task.seats[0].activation.waiting, 3);
    assert_eq!(
        task.seats[0].next_action,
        "Restore compatible Runner capacity, then retry status."
    );
    assert_eq!(task.freshness.as_str(), "stale");

    let encoded = serde_json::to_string(&task).expect("browser-safe JSON");
    for forbidden in [
        "activation_id",
        "claim_id",
        "invocation",
        "context",
        "prompt",
        "response",
        "memory",
        "bearer",
        "secret_reference",
        "payload",
    ] {
        assert!(
            !encoded.to_ascii_lowercase().contains(forbidden),
            "{forbidden}"
        );
    }
}

#[test]
fn missing_managed_reference_presence_retains_live_activation_counts_without_a_launch() {
    let temporary = TempDir::new().expect("temporary");
    let vault = FileSecretVaultV1::open(&temporary.path().join("secrets")).expect("vault");
    let owner_manifests = temporary.path().join("owner-manifests");
    fs::create_dir(&owner_manifests).expect("owner manifests");
    let registry = RunnerTemplateRegistryV1::open(
        &temporary.path().join("installed-templates"),
        &owner_manifests,
    )
    .expect("empty template registry");
    let runners = RunnerSupervisorV1::open(
        registry,
        &temporary.path().join("runner-runtime"),
        vault,
        Duration::from_secs(1),
    )
    .expect("runner supervisor");
    let mut assignment = healthy_snapshot().assignments.remove(0);
    assignment.managed_reference = true;
    let source = LiveRunnerAttentionSourceV1::new(
        StaticAssignments(vec![assignment]),
        MissingManagedRunnerDaemon,
        runners.clone(),
    );

    let snapshot = source.snapshot().expect("missing presence is bounded");
    assert!(snapshot.runners.is_empty());
    assert_eq!(snapshot.activations[0].waiting, 2);
    assert_eq!(snapshot.activations[0].leased, 0);

    let supervisor = RunnerAttentionSupervisorV1::new(
        source,
        FakeRestart(Arc::new(Mutex::new(Ok(())))),
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts")).expect("restart store"),
    );
    let task = supervisor.task(ROOM).expect("agent attention");
    assert_eq!(
        task.seats[0].compatibility,
        RunnerCompatibilityV1::Unavailable
    );
    assert_eq!(task.seats[0].capacity.available, 0);
    assert_eq!(task.seats[0].activation.waiting, 2);
    assert_eq!(
        task.seats[0].activation.state,
        ActivationAttentionStateV1::Attention
    );

    let mut external_assignments = healthy_snapshot().assignments;
    let external = LiveRunnerAttentionSourceV1::new(
        StaticAssignments(vec![external_assignments.remove(0)]),
        MissingManagedRunnerDaemon,
        runners,
    );
    assert_eq!(
        external.snapshot(),
        Err(RunnerAttentionSourceErrorV1::Unavailable)
    );
}

#[test]
fn leased_work_is_delayed_during_restart_and_advances_after_reconciliation() {
    let temporary = TempDir::new().expect("temporary");
    let source = FakeSource(Arc::new(Mutex::new(Ok(healthy_snapshot()))));
    let restart = FakeRestart(Arc::new(Mutex::new(Ok(()))));
    let store =
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts")).expect("restart store");
    let supervisor = RunnerAttentionSupervisorV1::new(source.clone(), restart, store.clone());

    {
        let mut snapshot = source.0.lock().expect("source");
        let value = snapshot.as_mut().expect("snapshot");
        value.activations[0].leased = 1;
        value.instances[0].state = LocalRunnerStateV1::Restarting;
    }
    let operation = supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("approved restart");
    assert_eq!(operation.state, RunnerRestartOperationStateV1::Reconciling);
    let delayed = supervisor.task(ROOM).expect("delayed task");
    assert_eq!(
        delayed.seats[0].activation.state,
        ActivationAttentionStateV1::Delayed
    );
    assert_eq!(
        delayed.seats[0].next_action,
        "Wait for the retained lease to expire or reconcile."
    );

    {
        let mut snapshot = source.0.lock().expect("source");
        let value = snapshot.as_mut().expect("snapshot");
        value.instances[0].state = LocalRunnerStateV1::Running;
        value.activations[0].leased = 0;
        value.activations[0].waiting = 1;
        value.observed_at_unix_ms += 1;
    }
    let restarted = RunnerAttentionSupervisorV1::new(
        source,
        FakeRestart(Arc::new(Mutex::new(Ok(())))),
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts"))
            .expect("reopen restart store"),
    );
    let reconciled = restarted.operations().expect("reconciled status");
    assert_eq!(reconciled.restart_attempts[0].operation_id, OPERATION);
    assert_eq!(
        reconciled.restart_attempts[0].state,
        RunnerRestartOperationStateV1::Succeeded
    );
    assert_eq!(reconciled.runners[0].capacity.in_use, 0);
    assert_eq!(reconciled.runners[0].capacity.available, 2);
}

#[test]
fn failed_restart_is_durable_idempotent_and_explains_a_safe_next_action() {
    let temporary = TempDir::new().expect("temporary");
    let mut snapshot = healthy_snapshot();
    snapshot.instances.push(LocalRunnerInstanceSnapshotV1 {
        instance_id: "other-runner".to_owned(),
        state: LocalRunnerStateV1::Running,
        healthy: true,
    });
    let supervisor = open(
        &temporary,
        snapshot,
        Err(RunnerRestartControlErrorV1::Failed),
    );
    let failed = supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("typed failure status");
    assert_eq!(failed.attempts, 1);
    assert_eq!(failed.state, RunnerRestartOperationStateV1::Failed);
    assert_eq!(
        failed.next_action,
        "Inspect bounded Runner diagnostics, then retry restart."
    );

    let duplicate = supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("idempotent retry");
    assert_eq!(duplicate, failed);
    assert_eq!(
        supervisor.restart("other-runner", OPERATION),
        Err(RunnerAttentionErrorV1::Conflict)
    );
}

#[test]
fn ambiguous_restart_is_not_reissued_and_reconciles_after_restart() {
    let temporary = TempDir::new().expect("temporary");
    let mut snapshot = healthy_snapshot();
    snapshot.instances[0].state = LocalRunnerStateV1::Restarting;
    let source = FakeSource(Arc::new(Mutex::new(Ok(snapshot))));
    let control = FakeRestart(Arc::new(Mutex::new(Err(
        RunnerRestartControlErrorV1::Ambiguous,
    ))));
    let supervisor = RunnerAttentionSupervisorV1::new(
        source.clone(),
        control.clone(),
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts")).expect("restart store"),
    );
    let first = supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("ambiguous restart retained");
    assert_eq!(first.state, RunnerRestartOperationStateV1::Reconciling);

    *control.0.lock().expect("control") = Err(RunnerRestartControlErrorV1::Failed);
    let duplicate = supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("duplicate reconciles, never republishes");
    assert_eq!(duplicate.state, RunnerRestartOperationStateV1::Reconciling);
    assert_eq!(duplicate.attempts, 1);

    {
        let mut state = source.0.lock().expect("source");
        state.as_mut().expect("snapshot").instances[0].state = LocalRunnerStateV1::Running;
    }
    let restarted = RunnerAttentionSupervisorV1::new(
        source,
        control,
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts"))
            .expect("reopen restart store"),
    );
    assert_eq!(
        restarted.operations().expect("reconciled").restart_attempts[0].state,
        RunnerRestartOperationStateV1::Succeeded
    );
}

#[test]
fn healthy_unassigned_approved_instance_can_finish_restart_reconciliation() {
    let temporary = TempDir::new().expect("temporary");
    let mut snapshot = healthy_snapshot();
    snapshot.assignments.clear();
    snapshot.activations.clear();
    let supervisor = open(&temporary, snapshot, Ok(()));
    let result = supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("unassigned restart");
    assert_eq!(result.state, RunnerRestartOperationStateV1::Succeeded);
}

#[test]
fn corrupt_restart_record_fails_closed() {
    let temporary = TempDir::new().expect("temporary");
    let supervisor = open(&temporary, healthy_snapshot(), Ok(()));
    supervisor
        .restart("managed-runner-01", OPERATION)
        .expect("restart record");
    std::fs::write(
        temporary
            .path()
            .join("restarts")
            .join(format!("{OPERATION}.json")),
        br#"{"schema":"wrong","operation_id":"01ARZ3NDEKTSV4RRFFQ69G5FB1"}"#,
    )
    .expect("corrupt record");
    assert_eq!(
        supervisor.operations(),
        Err(RunnerAttentionErrorV1::InvalidData)
    );
}

#[test]
fn failed_refresh_labels_retained_state_stale_and_empty_state_unavailable() {
    let temporary = TempDir::new().expect("temporary");
    let source = FakeSource(Arc::new(Mutex::new(Ok(healthy_snapshot()))));
    let supervisor = RunnerAttentionSupervisorV1::new(
        source.clone(),
        FakeRestart(Arc::new(Mutex::new(Ok(())))),
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts")).expect("restart store"),
    );
    assert_eq!(
        supervisor.operations().expect("live").freshness.as_str(),
        "live"
    );
    *source.0.lock().expect("source") = Err(RunnerAttentionSourceErrorV1::Unavailable);
    assert_eq!(
        supervisor.operations().expect("stale").freshness.as_str(),
        "stale"
    );

    let empty_temporary = TempDir::new().expect("empty temporary");
    let empty_source = FakeSource(Arc::new(Mutex::new(Err(
        RunnerAttentionSourceErrorV1::Unavailable,
    ))));
    let empty = RunnerAttentionSupervisorV1::new(
        empty_source,
        FakeRestart(Arc::new(Mutex::new(Ok(())))),
        FileRunnerRestartStoreV1::open(&empty_temporary.path().join("restarts"))
            .expect("empty restart store"),
    );
    assert_eq!(
        empty.operations().expect("unavailable").freshness.as_str(),
        "unavailable"
    );
}

#[test]
fn stale_retained_state_neither_authorizes_nor_completes_a_restart() {
    let temporary = TempDir::new().expect("temporary");
    let mut initial = healthy_snapshot();
    initial.instances[0].state = LocalRunnerStateV1::Restarting;
    let source = FakeSource(Arc::new(Mutex::new(Ok(initial))));
    let supervisor = RunnerAttentionSupervisorV1::new(
        source.clone(),
        FakeRestart(Arc::new(Mutex::new(Ok(())))),
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts")).expect("restart store"),
    );
    assert_eq!(
        supervisor
            .restart("managed-runner-01", OPERATION)
            .expect("restart")
            .state,
        RunnerRestartOperationStateV1::Reconciling
    );
    {
        let mut state = source.0.lock().expect("source");
        state.as_mut().expect("snapshot").instances[0].state = LocalRunnerStateV1::Running;
    }
    supervisor.task(ROOM).expect("cache healthy observation");
    *source.0.lock().expect("source") = Err(RunnerAttentionSourceErrorV1::Unavailable);
    let stale = supervisor.operations().expect("stale operations");
    assert_eq!(stale.freshness.as_str(), "stale");
    assert_eq!(
        stale.restart_attempts[0].state,
        RunnerRestartOperationStateV1::Reconciling
    );

    let other_operation = "01ARZ3NDEKTSV4RRFFQ69G5FB2";
    assert_eq!(
        supervisor.restart("managed-runner-01", other_operation),
        Err(RunnerAttentionErrorV1::Unavailable)
    );
}

#[tokio::test]
async fn routes_return_safe_aggregates_and_reject_command_shaped_restart_input() {
    let temporary = TempDir::new().expect("temporary");
    let router = runner_attention_router(open(&temporary, healthy_snapshot(), Ok(())));
    let operations = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/runner-attention")
                .body(Body::empty())
                .expect("operations request"),
        )
        .await
        .expect("operations response");
    assert_eq!(operations.status(), 200);
    let operations = operations
        .into_body()
        .collect()
        .await
        .expect("operations body")
        .to_bytes();
    let encoded = String::from_utf8(operations.to_vec()).expect("operations text");
    assert!(encoded.contains("worldstream/studio-runner-attention/v1"));
    assert!(!encoded.contains("activation_id"));
    assert!(!encoded.contains("secret_reference"));

    let invalid = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/runner-attention/managed-runner-01/restart")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"operation_id":"{OPERATION}","command":"kill -9"}}"#
                )))
                .expect("invalid request"),
        )
        .await
        .expect("invalid response");
    assert_eq!(invalid.status(), 422);
    let invalid = invalid
        .into_body()
        .collect()
        .await
        .expect("invalid body")
        .to_bytes();
    assert!(!String::from_utf8_lossy(&invalid).contains("kill -9"));

    let restart = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/runner-attention/managed-runner-01/restart")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"operation_id":"{OPERATION}"}}"#)))
                .expect("restart request"),
        )
        .await
        .expect("restart response");
    assert_eq!(restart.status(), 200);
    let restart = restart
        .into_body()
        .collect()
        .await
        .expect("restart body")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&restart).expect("restart JSON");
    assert_eq!(json["operation_id"], OPERATION);
    assert_eq!(json["instance_id"], "managed-runner-01");
    assert!(json.get("command").is_none());
}

#[test]
fn production_daemon_gateway_uses_exact_host_authority_and_bounded_safe_routes() {
    let temporary = TempDir::new().expect("temporary");
    let vault = FileSecretVaultV1::open(&temporary.path().join("secrets")).expect("vault");
    let secret = vault
        .store(SecretKindV1::HostAuthority, &[0xab; 32])
        .expect("host authority");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let fixture = thread::spawn(move || {
        for expected_path in [
            format!("/v1/operator/runners/{RUNNER}/presence"),
            format!("/v1/operator/rooms/{ROOM}/members/{MEMBER}/activation-status"),
        ] {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.starts_with(&format!("GET {expected_path} HTTP/1.1\r\n")));
            assert!(request.contains("Authorization: Bearer "));
            assert!(!request.contains("secret_reference"));
            let body = if expected_path.contains("/runners/") {
                serde_json::to_vec(&worldstream_protocol::OperatorRunnerPresenceV1 {
                    version: "worldstream/operator-runner-presence/v1".to_owned(),
                    runner_id: RUNNER.to_owned(),
                    connection: OperatorRunnerConnectionV1::Connected,
                    freshness: OperatorRunnerFreshnessV1::Fresh,
                    maximum_concurrent_activations: 2,
                    active_activations: 1,
                    available_activations: 1,
                    supported_pack_revisions: vec![pack()],
                    observed_at_unix_ms: 2_000,
                })
                .expect("presence JSON")
            } else {
                serde_json::to_vec(&serde_json::json!({
                    "version":"worldstream/operator-activation-status/v1",
                    "waiting":2,
                    "leased":1,
                    "observed_at_unix_ms":2_001
                }))
                .expect("activation JSON")
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .and_then(|()| stream.write_all(&body))
            .expect("write response");
        }
    });
    let source = HttpDaemonRunnerAttentionSourceV1::new(
        address,
        Duration::from_secs(2),
        vault,
        Some(secret),
    );
    let presence = source.presence(RUNNER).expect("presence");
    assert_eq!(presence.available_activations, 1);
    let counts = source.activation_counts(ROOM, MEMBER).expect("counts");
    assert_eq!((counts.waiting, counts.leased), (2, 1));
    let encoded = serde_json::to_string(&(presence, counts)).expect("safe aggregate JSON");
    for forbidden in ["bearer", "secret", "payload", "invocation", "activation_id"] {
        assert!(!encoded.contains(forbidden), "{forbidden}");
    }
    fixture.join().expect("fixture");
}

#[test]
fn absent_daemon_runner_presence_is_unavailable_not_corrupt_data() {
    let temporary = TempDir::new().expect("temporary");
    let vault = FileSecretVaultV1::open(&temporary.path().join("secrets")).expect("vault");
    let secret = vault
        .store(SecretKindV1::HostAuthority, &[0xab; 32])
        .expect("host authority");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let fixture = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0_u8; 4096];
        let count = stream.read(&mut request).expect("read request");
        let request = String::from_utf8_lossy(&request[..count]);
        assert!(request.starts_with(&format!(
            "GET /v1/operator/runners/{RUNNER}/presence HTTP/1.1\r\n"
        )));
        write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write response");
    });
    let source = HttpDaemonRunnerAttentionSourceV1::new(
        address,
        Duration::from_secs(2),
        vault,
        Some(secret),
    );

    assert_eq!(
        source.presence(RUNNER),
        Err(RunnerAttentionSourceErrorV1::Unavailable)
    );
    fixture.join().expect("fixture");
}

fn open(
    temporary: &TempDir,
    snapshot: AuthoritativeRunnerSnapshotV1,
    restart: Result<(), RunnerRestartControlErrorV1>,
) -> RunnerAttentionSupervisorV1<FakeSource, FakeRestart> {
    RunnerAttentionSupervisorV1::new(
        FakeSource(Arc::new(Mutex::new(Ok(snapshot)))),
        FakeRestart(Arc::new(Mutex::new(restart))),
        FileRunnerRestartStoreV1::open(&temporary.path().join("restarts")).expect("restart store"),
    )
}

fn healthy_snapshot() -> AuthoritativeRunnerSnapshotV1 {
    AuthoritativeRunnerSnapshotV1 {
        observed_at_unix_ms: 1_000,
        assignments: vec![AgentSeatAssignmentV1 {
            assignment_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
            room_id: ROOM.to_owned(),
            seat_id: "navigator-agent".to_owned(),
            member_id: MEMBER.to_owned(),
            runner_id: RUNNER.to_owned(),
            instance_id: Some("managed-runner-01".to_owned()),
            managed_reference: false,
            pack: pack(),
        }],
        runners: vec![RunnerPresenceSnapshotV1 {
            runner_id: RUNNER.to_owned(),
            connection: OperatorRunnerConnectionV1::Connected,
            freshness: OperatorRunnerFreshnessV1::Fresh,
            maximum_concurrent_activations: 2,
            active_activations: 0,
            available_activations: 2,
            supported_pack_revisions: vec![pack()],
            observed_at_unix_ms: 1_000,
        }],
        activations: vec![SeatActivationAggregateV1 {
            room_id: ROOM.to_owned(),
            member_id: MEMBER.to_owned(),
            waiting: 0,
            leased: 0,
        }],
        instances: vec![LocalRunnerInstanceSnapshotV1 {
            instance_id: "managed-runner-01".to_owned(),
            state: LocalRunnerStateV1::Running,
            healthy: true,
        }],
    }
}

fn pack() -> PackReference {
    PackReference {
        id: "agent-heist".to_owned(),
        version: "0.2.0".to_owned(),
        digest: "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_owned(),
    }
}
