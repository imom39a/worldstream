//! IMO-230 acceptance through the real portable Pack and production backends.
//! Explicitly ignored: both tests require a built `.wspack`; the PostgreSQL
//! test additionally requires a fresh migrated, least-privileged runtime DSN.
#![allow(clippy::unwrap_used, clippy::panic, clippy::too_many_lines)]

use crate::{
    ActionReply, BackendError, GatewayBackend, GatewaySession, MemberCapabilityIssueRequest,
    postgres_backend::{PostgresGatewayBackend, read_postgres_dsn},
    sqlite_backend::SqliteGatewayBackend,
};
use serde_json::{Value, json};
use std::{
    env, fs,
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};
use worldstream_component_host::ComponentPackHostV1;
use worldstream_core::{
    AuthorityBootstrapV1, AuthorityChangeV1, AuthorityStoreV1, AuthorityV1, CapabilityBearerV1,
    CapabilityScopeV1, CoreTraceV1, PackRegistryStatusV1, PackRegistryV1, PresentedCapabilityV1,
    PrincipalKindV1, RecordedStimulusV1, RoomAdmissionLanesV1, RoomId, builtin_counter_registry,
};
use worldstream_pack_bundle::PackBundleVerifierV1;
use worldstream_postgres::{PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore};
use worldstream_protocol::{
    AccessMode, ActionSubmit, BearerWireV1, CreateMember, CreateRoomRequest,
    EXTERNAL_INPUT_INGRESS_REQUEST_VERSION, ExternalInputIngressRequestV1,
    ExternalInputIngressResponseV1, PackReference, PrincipalKind, RoomAttach, RoomSyncAck,
    TimerFireRequest,
};
use worldstream_runtime::SecretSource;
use worldstream_sqlite::SqliteRoomStore;

const HOST_CAPABILITY: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC3";
const SOURCE: &str = worldstream_core::EXTERNAL_INPUT_INGRESS_SOURCE_ID;
const TIMER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FH3";
const RECORDED: &str = "2026-09-12T15:03:00.123456Z";

trait AcceptanceBackend: GatewayBackend {
    fn lanes(&self) -> &RoomAdmissionLanesV1;
}
impl AcceptanceBackend for SqliteGatewayBackend {
    fn lanes(&self) -> &RoomAdmissionLanesV1 {
        self.test_admission_lanes()
    }
}
impl AcceptanceBackend for PostgresGatewayBackend {
    fn lanes(&self) -> &RoomAdmissionLanesV1 {
        self.test_admission_lanes()
    }
}

fn registry() -> (Arc<PackRegistryV1>, PackReference) {
    // The production Component Host permits one concurrent compilation.
    // Both provider fixtures share one admitted immutable Pack in this process.
    static REGISTRY: OnceLock<(Arc<PackRegistryV1>, PackReference)> = OnceLock::new();
    REGISTRY.get_or_init(load_registry).clone()
}

fn load_registry() -> (Arc<PackRegistryV1>, PackReference) {
    let path = env::var_os("WORLDSTREAM_EXTERNAL_INPUT_BUNDLE")
        .expect("set WORLDSTREAM_EXTERNAL_INPUT_BUNDLE to the built late-join-reference.wspack");
    let bundle = PackBundleVerifierV1
        .inspect(Arc::<[u8]>::from(fs::read(path).unwrap()))
        .unwrap();
    let pack = PackReference {
        id: bundle.descriptor().pack_id.clone(),
        version: bundle.descriptor().explanatory_version.clone(),
        digest: bundle.revision_digest().to_string(),
    };
    assert_eq!(pack.id, "worldstream.late-join-reference");
    let admission = ComponentPackHostV1::new()
        .unwrap()
        .admit(
            bundle,
            PackRegistryStatusV1 {
                selectable_for_new_rooms: true,
                runnable_for_retained_rooms: true,
                approved_for_activity_start: false,
            },
        )
        .unwrap();
    (
        Arc::new(
            builtin_counter_registry()
                .unwrap()
                .admit_portable([admission])
                .unwrap(),
        ),
        pack,
    )
}

fn bootstrap(store: Arc<dyn AuthorityStoreV1>) {
    AuthorityV1::new(store)
        .bootstrap(
            AuthorityBootstrapV1::new(
                "01ARZ3NDEKTSV4RRFFQ69G5FC4".parse().unwrap(),
                "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse().unwrap(),
                PrincipalKindV1::Human,
                HOST_CAPABILITY.parse().unwrap(),
                CapabilityBearerV1::from_bytes([0xa9; 32]).token_hash(),
                None,
            )
            .unwrap(),
            "2026-09-12T12:00:00Z".parse().unwrap(),
        )
        .unwrap();
}

fn host() -> GatewaySession {
    GatewaySession::new_with_wire(
        "01ARZ3NDEKTSV4RRFFQ69G5FC5".parse().unwrap(),
        CapabilityBearerV1::from_bytes([0xa9; 32]),
        BearerWireV1::from_bytes([0xa9; 32]),
    )
}

fn revoke(store: Arc<dyn AuthorityStoreV1>) {
    AuthorityV1::new(store)
        .change(
            &PresentedCapabilityV1::new(
                HOST_CAPABILITY.parse().unwrap(),
                CapabilityBearerV1::from_bytes([0xa9; 32]),
            ),
            AuthorityChangeV1::RevokeCapability {
                change_id: "01ARZ3NDEKTSV4RRFFQ69G5FK0".parse().unwrap(),
                capability_id: HOST_CAPABILITY.parse().unwrap(),
                expected_generation: worldstream_core::AuthorityGenerationV1::new(1).unwrap(),
                reason_code: worldstream_core::AuthorityReasonCodeV1::new("acceptance_revoked")
                    .unwrap(),
            },
            "2026-09-13T00:00:00Z".parse().unwrap(),
        )
        .unwrap();
}

fn verify_room_scope(
    store: Arc<dyn AuthorityStoreV1>,
    backend: &dyn GatewayBackend,
    result: &SuiteResult,
) {
    let scoped = CapabilityBearerV1::from_bytes([0xb9; 32]);
    AuthorityV1::new(store)
        .change(
            &PresentedCapabilityV1::new(
                HOST_CAPABILITY.parse().unwrap(),
                CapabilityBearerV1::from_bytes([0xa9; 32]),
            ),
            AuthorityChangeV1::RegisterCapability {
                change_id: "01ARZ3NDEKTSV4RRFFQ69G5FK1".parse().unwrap(),
                capability: worldstream_core::NewCapabilityV1::new(
                    "01ARZ3NDEKTSV4RRFFQ69G5FK2".parse().unwrap(),
                    scoped.token_hash(),
                    "01ARZ3NDEKTSV4RRFFQ69G5FC2".parse().unwrap(),
                    worldstream_core::CapabilityProfileV1::HostOperator {
                        room_id: Some(result.room_id.parse().unwrap()),
                    },
                    worldstream_core::CapabilityScopeSetV1::new([
                        CapabilityScopeV1::OperatorRoomAdmin,
                    ])
                    .unwrap(),
                    None,
                )
                .unwrap(),
            },
            "2026-09-13T00:00:00Z".parse().unwrap(),
        )
        .unwrap();
    let session = GatewaySession::new_with_wire(
        "01ARZ3NDEKTSV4RRFFQ69G5FK3".parse().unwrap(),
        scoped,
        BearerWireV1::from_bytes([0xb9; 32]),
    );
    assert!(
        backend
            .ingest_external_input(&session, &result.room_id, result.first.clone())
            .unwrap()
            .duplicate
    );
    expect_error(
        backend.ingest_external_input(&session, "01ARZ3NDEKTSV4RRFFQ69G5FZZ", result.first.clone()),
        "Forbidden",
    );
}

fn request(
    pack: &PackReference,
    id: &str,
    basis: u64,
    version: u64,
    arrival: u64,
) -> ExternalInputIngressRequestV1 {
    ExternalInputIngressRequestV1 {
        version: EXTERNAL_INPUT_INGRESS_REQUEST_VERSION.to_owned(),
        source_id: SOURCE.to_owned(),
        input_id: id.to_owned(),
        input_type: worldstream_core::EXTERNAL_INPUT_INGRESS_TYPE.to_owned(),
        based_on_room_seq: basis,
        pack_digest: pack.digest.clone(),
        recorded_at: Some(RECORDED.to_owned()),
        payload: json!({"connection_id":"C17", "arrival_version":version, "arrival_minute":arrival,
            "departure_version":1,"departure_minute":650,"minimum_transfer_minutes":25}),
    }
}

struct SuiteResult {
    room_id: String,
    member: GatewaySession,
    first: ExternalInputIngressRequestV1,
    receipt: ExternalInputIngressResponseV1,
}

fn expect_error<T: std::fmt::Debug>(result: Result<T, BackendError>, expected: &str) {
    let error = result.expect_err(expected);
    assert_eq!(format!("{error:?}"), expected);
}

/// The gate is only a scheduling barrier. Every queued request still travels
/// through authentication, production ingress, the real Room lane and commit.
fn wait_depth(backend: &dyn AcceptanceBackend, room: &RoomId, depth: usize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if backend.lanes().room_depth(room).unwrap() == depth {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

fn exercise(backend: &dyn AcceptanceBackend, pack: &PackReference) -> SuiteResult {
    let host = host();
    let created = backend
        .create_room(
            &host,
            CreateRoomRequest {
                pack: pack.clone(),
                configuration: json!({"max_open_work":16}),
                members: [
                    ("01ARZ3NDEKTSV4RRFFQ69G5FC6", "analyst"),
                    ("01ARZ3NDEKTSV4RRFFQ69G5FC7", "reviewer"),
                ]
                .map(|(principal_id, role)| CreateMember {
                    principal_id: principal_id.to_owned(),
                    principal_kind: PrincipalKind::Agent,
                    role: Some(role.to_owned()),
                    access_mode: AccessMode::Participant,
                })
                .to_vec(),
                idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FD0".to_owned(),
            },
        )
        .unwrap();
    let room = &created.room_id;
    let issued = backend
        .issue_member_capability(
            &host,
            MemberCapabilityIssueRequest {
                room_id: room.clone(),
                member_id: created.member_ids[0].clone(),
                principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
                scopes: vec![
                    CapabilityScopeV1::RoomAttach,
                    CapabilityScopeV1::RoomObserveMember,
                    CapabilityScopeV1::RoomAct,
                    CapabilityScopeV1::RoomReplay,
                ],
                idempotency_key: "01ARZ3NDEKTSV4RRFFQ69G5FC8".to_owned(),
                expires_at: None,
            },
        )
        .unwrap();
    let member = GatewaySession::new_with_wire(
        "01ARZ3NDEKTSV4RRFFQ69G5FC9".parse().unwrap(),
        CapabilityBearerV1::from_bytes(BearerWireV1::parse(&issued.bearer).unwrap().into_bytes()),
        BearerWireV1::parse(&issued.bearer).unwrap(),
    );
    let attached = backend
        .attach(
            &member,
            RoomAttach {
                room_id: room.clone(),
                member_id: created.member_ids[0].clone(),
                after_frame_seq: None,
            },
        )
        .unwrap();
    backend
        .sync_ack(
            &member,
            RoomSyncAck {
                room_id: room.clone(),
                member_id: created.member_ids[0].clone(),
                through_frame_head: attached.attached.frame_head,
                sync_token: attached.attached.sync_token,
            },
        )
        .unwrap();
    let first = request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FD1", 0, 2, 640);
    expect_error(
        backend.ingest_external_input(&member, room, first.clone()),
        "Forbidden",
    );
    for (field, value) in [
        ("source_id", "01ARZ3NDEKTSV4RRFFQ69G5FH4"),
        ("input_type", "arbitrary-feed"),
        ("version", "worldstream/external-input-ingress.v999"),
    ] {
        let mut wire = serde_json::to_value(&first).unwrap();
        wire[field] = json!(value);
        expect_error(
            backend.ingest_external_input(&host, room, serde_json::from_value(wire).unwrap()),
            "Rejected",
        );
    }
    let mut oversized = first.clone();
    oversized.payload = json!({"value":"x".repeat(64*1024)});
    expect_error(
        backend.ingest_external_input(&host, room, oversized),
        "Rejected",
    );
    let mut wrong_pin = first.clone();
    wrong_pin.pack_digest = format!("blake3:{}", "0".repeat(64));
    expect_error(
        backend.ingest_external_input(&host, room, wrong_pin),
        "Conflict",
    );
    expect_error(
        backend.ingest_external_input(&host, "01ARZ3NDEKTSV4RRFFQ69G5FZZ", first.clone()),
        "NotFound",
    );
    let receipt = backend
        .ingest_external_input(&host, room, first.clone())
        .unwrap();
    assert_eq!(receipt.room_head.room_seq, 1);
    assert_eq!(receipt.recorded_at, RECORDED);
    let mut retry = first.clone();
    retry.recorded_at = Some("2026-09-13T15:03:00Z".to_owned());
    let duplicate = backend.ingest_external_input(&host, room, retry).unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.recorded_at, receipt.recorded_at);
    assert_eq!(duplicate.room_head, receipt.room_head);
    assert_eq!(duplicate.transition_id, receipt.transition_id);
    let mut conflict = first.clone();
    conflict.payload["arrival_minute"] = json!(639);
    expect_error(
        backend.ingest_external_input(&host, room, conflict),
        "Conflict",
    );
    let projection = backend.projection(&member, room).unwrap();
    assert_eq!(
        projection.projection.activity["connections"][0]["arrival_minute"],
        640
    );
    assert!(
        serde_json::to_vec(&projection.projection.activity)
            .unwrap()
            .len()
            < 24576
    );
    assert_eq!(
        backend
            .replay(&member, room, 1)
            .unwrap()
            .projection
            .activity,
        projection.projection.activity
    );

    // An ignored old revision is still a mandatory, canonical input: it must
    // advance Head without replacing current facts or faulting the Room.
    let old = request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FD2", 1, 1, 600);
    assert_eq!(
        backend
            .ingest_external_input(&host, room, old)
            .unwrap()
            .room_head
            .room_seq,
        2
    );
    let same_revision = request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FD3", 2, 2, 639);
    assert_eq!(
        backend
            .ingest_external_input(&host, room, same_revision)
            .unwrap()
            .room_head
            .room_seq,
        3
    );
    assert_eq!(
        backend
            .projection(&member, room)
            .unwrap()
            .projection
            .activity,
        projection.projection.activity
    );
    let correction = request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FD4", 3, 3, 630);
    assert_eq!(
        backend
            .ingest_external_input(&host, room, correction)
            .unwrap()
            .room_head
            .room_seq,
        4
    );

    // Force reservation order Action -> ExternalInput -> Timer behind one
    // held lane position. Only the Action and Timer can commit: the competing
    // input retains its exact old basis and is explicitly refused.
    let room_id: RoomId = room.parse().unwrap();
    let gate = backend.lanes().reserve_host_stimulus(&room_id).unwrap();
    let action = ActionSubmit {
        room_id: room.clone(),
        member_id: created.member_ids[0].clone(),
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FE0".to_owned(),
        based_on_room_seq: 4,
        action_type: "record_assessment".to_owned(),
        payload: json!({"work_id":"work-C17",
            "expected_work_revision":3,"arrival_version":3,"departure_version":1,
            "claim":"connection_at_risk","reason_code":"transfer_time_insufficient"}),
    };
    thread::scope(|scope| {
        let a = scope.spawn(|| backend.action(&member, action));
        let a_queued = wait_depth(backend, &room_id, 2);
        let e = scope.spawn(|| {
            backend.ingest_external_input(
                &host,
                room,
                request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FE1", 4, 4, 635),
            )
        });
        let e_queued = wait_depth(backend, &room_id, 3);
        let t = scope.spawn(|| {
            backend.fire_timer(
                &host,
                room,
                TimerFireRequest {
                    timer_id: TIMER.to_owned(),
                    generation: 2,
                },
            )
        });
        let t_queued = wait_depth(backend, &room_id, 4);
        if !(a_queued && e_queued && t_queued) {
            // Release before scoped threads join, including regression failure.
            drop(gate);
            panic!("all three real requests must join the shared lane");
        }
        let busy = backend.ingest_external_input(
            &host,
            room,
            request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FE2", 4, 4, 636),
        );
        drop(gate);
        expect_error(busy, "Busy");
        match a.join().unwrap().unwrap() {
            ActionReply::Accepted(value) => assert_eq!(value.room_head.room_seq, 5),
            other => panic!("current assessment must commit: {other:?}"),
        }
        expect_error(e.join().unwrap(), "Rejected");
        assert_eq!(t.join().unwrap().unwrap().room_head.room_seq, 6);
    });
    let queue = backend.room_admission_queue_snapshot();
    assert_eq!(queue.process_current, 0);
    assert_eq!(queue.unit_high_water, 4);
    assert_eq!(queue.admitted_total, queue.completed_total);
    assert!(queue.full_total >= 1);
    let final_view = backend.projection(&member, room).unwrap();
    assert_eq!(final_view.room_head.room_seq, 6);
    assert_eq!(final_view.room_health, "healthy");
    assert_eq!(
        final_view.projection.activity["connections"][0]["arrival_version"],
        3
    );
    assert_eq!(
        final_view.projection.activity["action_policy"]["can_record_assessment"],
        false
    );
    assert_eq!(
        final_view.projection.activity["open_work"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // A maximum-length legal source ID must fit its derived `work-` ID in
    // both retained state and the offered assessment payload schema.
    let longest_id = "C".repeat(32);
    let mut longest = request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FE3", 6, 1, 600);
    longest.payload["connection_id"] = json!(longest_id);
    assert_eq!(
        backend
            .ingest_external_input(&host, room, longest)
            .unwrap()
            .room_head
            .room_seq,
        7
    );
    let longest_view = backend.projection(&member, room).unwrap();
    assert_eq!(
        longest_view.projection.activity["open_work"][1]["work_id"],
        format!("work-{longest_id}")
    );
    assert_eq!(
        backend
            .fire_timer(
                &host,
                room,
                TimerFireRequest {
                    timer_id: TIMER.to_owned(),
                    generation: 3
                }
            )
            .unwrap()
            .room_head
            .room_seq,
        8
    );
    // The rejected competing input retains its identity/hash. A caller may
    // not rebase that same identity onto the now-current Head.
    expect_error(
        backend.ingest_external_input(
            &host,
            room,
            request(pack, "01ARZ3NDEKTSV4RRFFQ69G5FE1", 8, 4, 635),
        ),
        "Conflict",
    );
    for sequence in 0..=8 {
        let replay = backend.replay(&member, room, sequence).unwrap();
        assert_eq!(replay.room_head.room_seq, sequence);
        assert_eq!(replay.room_head.pack_digest, pack.digest);
        assert!(
            serde_json::to_vec(&replay.projection.activity)
                .unwrap()
                .len()
                < 24576
        );
    }
    SuiteResult {
        room_id: room.clone(),
        member,
        first,
        receipt,
    }
}

fn verify_trace(trace: &CoreTraceV1) {
    assert_eq!(trace.transitions().len(), 8);
    let kinds: Vec<_> = trace
        .transitions()
        .iter()
        .map(|t| match t.recorded_stimulus() {
            RecordedStimulusV1::ExternalInput(_) => "external_input",
            RecordedStimulusV1::ParticipantAction(_) => "participant_action",
            RecordedStimulusV1::TimerFired(_) => "timer_fired",
            RecordedStimulusV1::CoreProposed(_) => "core_proposed",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "external_input",
            "external_input",
            "external_input",
            "external_input",
            "participant_action",
            "timer_fired",
            "external_input",
            "timer_fired"
        ]
    );
    for (index, transition) in trace.transitions().iter().enumerate() {
        assert_eq!(transition.room_seq().get(), (index + 1) as u64);
        assert_eq!(
            transition.ordered_attention_signals().len(),
            usize::from(index == 0 || index == 3 || index == 6 || index == 7)
        );
        if let RecordedStimulusV1::ExternalInput(input) = transition.recorded_stimulus() {
            assert_eq!(input.source_id.as_str(), SOURCE);
            assert_eq!(input.recorded_at.as_str(), RECORDED);
            let event: Value =
                serde_json::from_slice(&transition.ordered_domain_events()[0].to_bytes().unwrap())
                    .unwrap();
            assert_eq!(event["source_id"], SOURCE);
            assert_eq!(event["input_id"], input.input_id.as_str());
            assert_eq!(event["recorded_at"], RECORDED);
            if index == 1 {
                assert_eq!(event["code"], "stale_source_revision");
            }
            if index == 2 {
                assert_eq!(event["code"], "invalid_source");
            }
        }
    }
}

fn verify_restart(backend: &dyn GatewayBackend, result: &SuiteResult) {
    let duplicate = backend
        .ingest_external_input(&host(), &result.room_id, result.first.clone())
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.room_head, result.receipt.room_head);
    assert_eq!(duplicate.transition_id, result.receipt.transition_id);
    assert_eq!(duplicate.recorded_at, RECORDED);
    assert_eq!(
        backend
            .projection(&result.member, &result.room_id)
            .unwrap()
            .room_head
            .room_seq,
        8
    );
}

#[test]
#[ignore = "requires a built portable late-join-reference Pack; see its README"]
fn sqlite_portable_external_input_acceptance() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = directory.path().join("room.sqlite");
    let store = SqliteRoomStore::open(&path).unwrap();
    bootstrap(Arc::new(store.clone()));
    let (registry, pack) = registry();
    let backend = SqliteGatewayBackend::with_test_admission_lanes(
        store.clone(),
        registry.clone(),
        RoomAdmissionLanesV1::new(4, 1).unwrap(),
    );
    let result = exercise(&backend, &pack);
    verify_trace(
        &store
            .recover_room(&registry, &result.room_id.parse().unwrap())
            .unwrap()
            .unwrap(),
    );
    drop(backend);
    let restarted = SqliteGatewayBackend::new(SqliteRoomStore::open(&path).unwrap(), registry);
    verify_restart(&restarted, &result);
    verify_room_scope(Arc::new(store.clone()), &restarted, &result);
    // Inject only an operational health fence, never canonical history.
    rusqlite::Connection::open(&path).unwrap().execute("UPDATE room_integrity SET status='quarantined', generation=generation+1 WHERE room_id=?1", [&result.room_id]).unwrap();
    expect_error(
        restarted.ingest_external_input(
            &host(),
            &result.room_id,
            request(&pack, "01ARZ3NDEKTSV4RRFFQ69G5FJ0", 8, 4, 635),
        ),
        "RoomQuarantined",
    );
    revoke(Arc::new(store));
    expect_error(
        restarted.ingest_external_input(&host(), &result.room_id, result.first),
        "Forbidden",
    );
    eprintln!(
        "EXTERNAL_INPUT_ACCEPTANCE=PASS backend=sqlite portable=true replay=0..8 lane_capacity=4 overload=busy restart=duplicate"
    );
}

#[test]
#[ignore = "requires fresh migrated PostgreSQL 17 runtime DSN and built portable Pack; see Pack README"]
fn live_postgres_portable_external_input_acceptance() {
    let path = env::var_os("WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE")
        .expect("set a protected PostgreSQL runtime DSN file");
    let dsn = read_postgres_dsn(&SecretSource::File(path.into())).unwrap();
    let config =
        PostgresConnectionConfig::runtime(dsn.clone(), PostgresConnectionPath::Direct).unwrap();
    let store = Arc::new(PostgresRoomStore::new(config.clone()).unwrap());
    store.verify_schema().unwrap();
    bootstrap(store.clone());
    let (registry, pack) = registry();
    let backend = PostgresGatewayBackend::with_test_admission_lanes(
        PostgresRoomStore::new(config.clone()).unwrap(),
        registry.clone(),
        RoomAdmissionLanesV1::new(4, 1).unwrap(),
    );
    let result = exercise(&backend, &pack);
    verify_trace(
        &store
            .recover_room(&registry, &result.room_id)
            .unwrap()
            .unwrap(),
    );
    drop(backend);
    let restarted = PostgresGatewayBackend::new(PostgresRoomStore::new(config).unwrap(), registry);
    verify_restart(&restarted, &result);
    verify_room_scope(store.clone(), &restarted, &result);
    // This fixture uses the same restricted runtime role, only on its own Room.
    postgres::Client::connect(&dsn, postgres::NoTls).unwrap().execute("UPDATE worldstream_room_roots SET integrity_status='quarantined', integrity_generation=integrity_generation+1 WHERE room_id=$1", &[&result.room_id]).unwrap();
    expect_error(
        restarted.ingest_external_input(
            &host(),
            &result.room_id,
            request(&pack, "01ARZ3NDEKTSV4RRFFQ69G5FJ0", 8, 4, 635),
        ),
        "RoomQuarantined",
    );
    revoke(store);
    expect_error(
        restarted.ingest_external_input(&host(), &result.room_id, result.first),
        "Forbidden",
    );
    eprintln!(
        "EXTERNAL_INPUT_ACCEPTANCE=PASS backend=postgres portable=true replay=0..8 lane_capacity=4 overload=busy restart=duplicate"
    );
}
