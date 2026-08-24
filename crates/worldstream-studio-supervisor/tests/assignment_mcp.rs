#![allow(clippy::panic)]

use std::{
    fs,
    io::Cursor,
    net::TcpListener,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::Duration,
};

use clap::{CommandFactory as _, Parser as _};
use serde_json::{Value, json};
use tempfile::tempdir;
use tungstenite::{Message, accept_hdr};
use worldstream_protocol::{
    ActionOffer, ActivityPackCatalogAction, ActivityPackCatalogResponse,
    ActivityPackCatalogRevisionDetail, ActivityPackCatalogRevisionResponse,
    ActivityPackCatalogRevisionSummary, ActivityPackCatalogSchema, PackReference, Projection,
    RoomHead, SealedCapabilityBearerV1,
};
use worldstream_studio_supervisor::activity_packs::{
    ActivityPackProxyErrorV1, DaemonActivityPackSource,
};
use worldstream_studio_supervisor::assignment_mcp::{
    self, AssignedMembershipAuthorityV1, AssignedMembershipGatewayErrorV1,
    AssignedMembershipGatewayV1, AssignedMembershipLaunchBindingV1,
    AssignedMembershipLaunchSourceV1, AssignedMembershipSourceErrorV1, AssignedMembershipSourceV1,
    AssignmentMcpCliV1, AssignmentMcpLaunchRegistryV1, AssignmentMcpServerV1,
    AssignmentMcpSupervisorV1, FixedDaemonAssignedMembershipGatewayV1, MembershipStreamSnapshotV1,
    ObservationDeliveryV1, open_registered_assignment_mcp, run_assignment_mcp_stdio,
};
use worldstream_studio_supervisor::secrets::{FileSecretVaultV1, SecretKindV1, SecretReferenceV1};

const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB0";
const OTHER_ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB1";
const ROOM: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const MEMBER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const RUNNER: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";
const BEARER: &str = "wsb1:abababababababababababababababababababababababababababababababab";

#[derive(Clone)]
struct FakeSource;

#[derive(Clone)]
struct FakeLaunchSource {
    authority_reference: SecretReferenceV1,
    runner_authority_reference: SecretReferenceV1,
}

#[derive(Clone)]
struct OversizedPackSource;

impl DaemonActivityPackSource for OversizedPackSource {
    fn catalog(&self) -> Result<ActivityPackCatalogResponse, ActivityPackProxyErrorV1> {
        Err(ActivityPackProxyErrorV1::InvalidResponse)
    }

    fn revision(
        &self,
        exact_digest: &str,
    ) -> Result<ActivityPackCatalogRevisionResponse, ActivityPackProxyErrorV1> {
        let pack = PackReference {
            id: "worldstream.counter".to_owned(),
            version: "1.0.0".to_owned(),
            digest: exact_digest.to_owned(),
        };
        Ok(ActivityPackCatalogRevisionResponse {
            version: "activity_pack_catalog.v1".to_owned(),
            revision: ActivityPackCatalogRevisionDetail {
                summary: ActivityPackCatalogRevisionSummary {
                    pack,
                    name: "Counter".to_owned(),
                    selectable_for_new_rooms: true,
                    runnable_for_retained_rooms: true,
                },
                roles: Vec::new(),
                configuration_schema: ActivityPackCatalogSchema {
                    schema_id: "config".to_owned(),
                    schema_digest: digest('b'),
                    schema: json!({"type":"object"}),
                },
                actions: vec![ActivityPackCatalogAction {
                    action_type: "increment".to_owned(),
                    payload_schema: ActivityPackCatalogSchema {
                        schema_id: "increment".to_owned(),
                        schema_digest: digest('a'),
                        schema: json!({"type":"string","description":"x".repeat(70_000)}),
                    },
                }],
                lobby_compatibility: None,
            },
        })
    }
}

impl AssignedMembershipLaunchSourceV1 for FakeLaunchSource {
    fn resolve_launch_binding(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipLaunchBindingV1, AssignedMembershipSourceErrorV1> {
        if assignment_id != ASSIGNMENT {
            return Err(AssignedMembershipSourceErrorV1::NotFound);
        }
        Ok(AssignedMembershipLaunchBindingV1 {
            assignment_id: ASSIGNMENT.to_owned(),
            profile_id: "counter-agent".to_owned(),
            profile_revision: "r1".to_owned(),
            role: "counter".to_owned(),
            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY".to_owned(),
            room_id: ROOM.to_owned(),
            member_id: MEMBER.to_owned(),
            pack: PackReference {
                id: "worldstream.counter".to_owned(),
                version: "1.0.0".to_owned(),
                digest: digest('2'),
            },
            authority_reference: self.authority_reference.clone(),
            runner_id: RUNNER.to_owned(),
            runner_authority_reference: self.runner_authority_reference.clone(),
        })
    }
}

impl AssignedMembershipSourceV1 for FakeSource {
    fn resolve_assignment(
        &self,
        assignment_id: &str,
    ) -> Result<AssignedMembershipAuthorityV1, AssignedMembershipSourceErrorV1> {
        if assignment_id != ASSIGNMENT {
            return Err(AssignedMembershipSourceErrorV1::NotFound);
        }
        AssignedMembershipAuthorityV1::new(
            ASSIGNMENT,
            "counter-agent",
            "r1",
            "counter",
            "01ARZ3NDEKTSV4RRFFQ69G5FAY",
            ROOM,
            MEMBER,
            SealedCapabilityBearerV1::parse(BEARER.to_owned())
                .map_err(|_| AssignedMembershipSourceErrorV1::Unavailable)?,
        )
    }
}

#[derive(Clone)]
struct FakeGateway {
    calls: Arc<Mutex<Vec<GatewayCall>>>,
    snapshots:
        Arc<Mutex<Vec<Result<MembershipStreamSnapshotV1, AssignedMembershipGatewayErrorV1>>>>,
}

type GatewayCall = (String, String, String, Option<u64>);

impl FakeGateway {
    fn new(
        snapshots: Vec<Result<MembershipStreamSnapshotV1, AssignedMembershipGatewayErrorV1>>,
    ) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            snapshots: Arc::new(Mutex::new(snapshots)),
        }
    }
}

impl AssignedMembershipGatewayV1 for FakeGateway {
    fn synchronize(
        &self,
        authority: &AssignedMembershipAuthorityV1,
    ) -> Result<MembershipStreamSnapshotV1, AssignedMembershipGatewayErrorV1> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                authority.room_id().to_owned(),
                authority.member_id().to_owned(),
                "observe".to_owned(),
                None,
            ));
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(0)
    }

    fn acknowledge(
        &self,
        authority: &AssignedMembershipAuthorityV1,
        through_frame_seq: u64,
    ) -> Result<u64, AssignedMembershipGatewayErrorV1> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((
                authority.room_id().to_owned(),
                authority.member_id().to_owned(),
                "ack".to_owned(),
                Some(through_frame_seq),
            ));
        Ok(through_frame_seq)
    }
}

#[test]
fn context_lists_only_its_assignment_and_never_discloses_routing_or_authority() {
    let gateway = FakeGateway::new(vec![Ok(snapshot(7, 8, true))]);
    let supervisor = AssignmentMcpSupervisorV1::new(FakeSource, gateway);
    let context = supervisor
        .issue_context(ASSIGNMENT)
        .unwrap_or_else(|error| panic!("context: {error:?}"));
    let mut server = AssignmentMcpServerV1::open(context);

    let tasks = server
        .list_assigned_tasks(json!({}))
        .unwrap_or_else(|error| panic!("list: {error:?}"));
    assert_eq!(tasks["tasks"].as_array().map(Vec::len), Some(1));
    assert_eq!(tasks["tasks"][0]["task_id"], ASSIGNMENT);
    assert!(!tasks.to_string().contains(OTHER_ASSIGNMENT));
    assert_safe(&tasks);

    let observation = server
        .observe(json!({}))
        .unwrap_or_else(|error| panic!("observe: {error:?}"));
    assert_eq!(observation["stream"]["frame_head"], 8);
    assert_eq!(
        observation["projection_reset"]["projection"]["activity"]["value"],
        7
    );
    assert_safe(&observation);
    assert!(server.list_assigned_tasks(json!({"room_id":ROOM})).is_err());
    assert!(server.observe(json!({"member_id":MEMBER})).is_err());
}

#[test]
fn ack_resume_reconnect_and_stale_cursor_recovery_preserve_assignment_isolation() {
    let gateway = FakeGateway::new(vec![
        Ok(snapshot(3, 5, false)),
        Err(AssignedMembershipGatewayErrorV1::Disconnected),
        Ok(snapshot(5, 9, true)),
    ]);
    let calls = gateway.calls.clone();
    let context = AssignmentMcpSupervisorV1::new(FakeSource, gateway)
        .issue_context(ASSIGNMENT)
        .unwrap_or_else(|error| panic!("context: {error:?}"));
    let mut server = AssignmentMcpServerV1::open(context);

    let first = server
        .observe(json!({}))
        .unwrap_or_else(|error| panic!("observe: {error:?}"));
    assert_eq!(
        first["observations"]
            .as_array()
            .and_then(|values| values.last())
            .and_then(|value| value["frame_seq"].as_u64()),
        Some(5),
    );
    assert_eq!(first["action_offers"][0]["action_type"], "increment");
    let acknowledged = server
        .acknowledge(json!({"through_frame_seq":5}))
        .unwrap_or_else(|error| panic!("ack: {error:?}"));
    assert_eq!(acknowledged["cursor"], 5);
    assert!(server.acknowledge(json!({"through_frame_seq":6})).is_err());

    let disconnected = match server.observe(json!({})) {
        Ok(value) => panic!("expected disconnect, received {value}"),
        Err(error) => error,
    };
    assert_eq!(disconnected.code(), "assignment_daemon_disconnected");
    assert_eq!(disconnected.next_action(), "retry_observe");
    let recovered = server
        .observe(json!({}))
        .unwrap_or_else(|error| panic!("recover: {error:?}"));
    assert_eq!(
        recovered["projection_reset"]["reset_reason"],
        "retained_range_unavailable"
    );
    assert_eq!(recovered["stream"]["cursor"], 5);

    assert!(
        calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .all(|call| call.0 == ROOM && call.1 == MEMBER)
    );
}

#[test]
fn stdio_exposes_only_generic_bounded_tools_and_safe_errors() {
    let context = AssignmentMcpSupervisorV1::new(
        FakeSource,
        FakeGateway::new(vec![Ok(snapshot(0, 0, true))]),
    )
    .issue_context(ASSIGNMENT)
    .unwrap_or_else(|error| panic!("context: {error:?}"));
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"worldstream.list_assigned_tasks","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"worldstream.observe","arguments":{"bearer":BEARER}}}),
    ].into_iter().map(|value| value.to_string()).collect::<Vec<_>>().join("\n") + "\n";
    let mut output = Vec::new();
    run_assignment_mcp_stdio(
        &mut AssignmentMcpServerV1::open(context),
        &mut Cursor::new(input.into_bytes()),
        &mut output,
    )
    .unwrap_or_else(|error| panic!("stdio: {error:?}"));
    let output = String::from_utf8(output).unwrap_or_else(|error| panic!("utf8: {error}"));
    assert!(output.contains("worldstream.list_assigned_tasks"));
    assert!(output.contains("worldstream.observe"));
    assert!(output.contains("worldstream.acknowledge"));
    assert!(output.contains("worldstream.list_current_action_offers"));
    assert!(output.contains("worldstream.submit_action"));
    assert!(output.contains("worldstream.next_activation"));
    assert!(output.contains("worldstream.complete_activation"));
    assert!(!output.contains(ROOM));
    assert!(!output.contains(MEMBER));
    assert!(!output.contains(BEARER));
    assert!(!output.contains("agent-heist"));
}

#[test]
fn stdio_enforces_mcp_lifecycle_and_standard_json_rpc_errors() {
    let context = AssignmentMcpSupervisorV1::new(FakeSource, FakeGateway::new(vec![]))
        .issue_context(ASSIGNMENT)
        .unwrap_or_else(|error| panic!("context: {error:?}"));
    let input = [
        "{".to_owned(),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string(),
        json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"unsupported","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}).to_string(),
        json!({"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}).to_string(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        json!({"jsonrpc":"2.0","id":4,"method":"unknown"}).to_string(),
    ]
    .join("\n")
        + "\n";
    let mut output = Vec::new();
    run_assignment_mcp_stdio(
        &mut AssignmentMcpServerV1::open(context),
        &mut Cursor::new(input.into_bytes()),
        &mut output,
    )
    .unwrap_or_else(|error| panic!("stdio: {error:?}"));
    let responses = String::from_utf8(output)
        .unwrap_or_else(|error| panic!("utf8: {error}"))
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("JSON-RPC response: {error}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 5);
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert_eq!(responses[1]["error"]["code"], -32600);
    assert_eq!(responses[2]["error"]["code"], -32602);
    assert_eq!(responses[3]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(responses[4]["error"]["code"], -32601);
}

#[test]
fn rejects_incoherent_reset_and_retained_cursor_ranges() {
    let mut skipped = snapshot(3, 5, false);
    skipped.observations.remove(0);
    let context = AssignmentMcpSupervisorV1::new(FakeSource, FakeGateway::new(vec![Ok(skipped)]))
        .issue_context(ASSIGNMENT)
        .unwrap_or_else(|error| panic!("context: {error:?}"));
    let mut server = AssignmentMcpServerV1::open(context);
    let skipped = match server.observe(json!({})) {
        Ok(value) => panic!("expected invalid range, received {value}"),
        Err(error) => error,
    };
    assert_eq!(skipped.code(), "assignment_daemon_data_invalid");

    let mut wrong_reset = snapshot(3, 5, true);
    if let Some(reset) = &mut wrong_reset.projection_reset {
        reset.baseline_frame_head = 4;
    }
    let context =
        AssignmentMcpSupervisorV1::new(FakeSource, FakeGateway::new(vec![Ok(wrong_reset)]))
            .issue_context(ASSIGNMENT)
            .unwrap_or_else(|error| panic!("context: {error:?}"));
    let mut server = AssignmentMcpServerV1::open(context);
    let wrong_reset = match server.observe(json!({})) {
        Ok(value) => panic!("expected invalid reset, received {value}"),
        Err(error) => error,
    };
    assert_eq!(wrong_reset.next_action(), "return_to_task_setup");
}

#[test]
fn rejects_nested_authority_material_from_the_daemon_without_disclosing_it() {
    let mut compromised = snapshot(3, 4, false);
    compromised.observations[0].observation = json!({
        "otherwise_safe": {
            "secret_reference": "membership-authority-reference",
            "nested": [BEARER]
        }
    });
    let context =
        AssignmentMcpSupervisorV1::new(FakeSource, FakeGateway::new(vec![Ok(compromised)]))
            .issue_context(ASSIGNMENT)
            .unwrap_or_else(|error| panic!("context: {error:?}"));
    let mut server = AssignmentMcpServerV1::open(context);

    let error = match server.observe(json!({})) {
        Ok(value) => panic!("expected unsafe payload rejection, received {value}"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "assignment_daemon_data_invalid");
    let diagnostic = format!("{} {}", error.code(), error.next_action());
    assert!(!diagnostic.contains(BEARER));
    assert!(!diagnostic.contains("membership-authority-reference"));
}

#[test]
fn production_cli_accepts_only_launch_reference_and_ordinary_local_state() {
    let launch_reference = "a".repeat(64);
    let parsed = AssignmentMcpCliV1::try_parse_from([
        "worldstream-assignment-mcp",
        "--launch-reference",
        &launch_reference,
        "--state-dir",
        ".worldstream/studio",
    ])
    .unwrap_or_else(|error| panic!("bounded CLI: {error}"));
    assert_eq!(parsed.launch_reference(), launch_reference);

    for forbidden in [
        "--assignment",
        "--room-id",
        "--member-id",
        "--bearer",
        "--daemon",
        "--database-query",
    ] {
        assert!(
            AssignmentMcpCliV1::try_parse_from([
                "worldstream-assignment-mcp",
                "--launch-reference",
                &launch_reference,
                forbidden,
                "unsafe",
            ])
            .is_err(),
            "{forbidden} must not be accepted",
        );
    }
    let help = AssignmentMcpCliV1::command().render_long_help().to_string();
    assert!(!help.contains("--assignment"));
    assert!(!help.contains("room-id"));
    assert!(!help.contains("member-id"));
    assert!(!help.contains("bearer"));
    assert!(!help.contains("credential"));
}

#[test]
fn supervisor_issued_launch_is_exclusive_restartable_revocable_and_non_secret() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("Runner fixture listener: {error}"));
    let daemon = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("Runner fixture address: {error}"));
    let runner_fixture = thread::spawn(move || serve_launch_runner_sessions(&listener, 2));
    let directory = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
    let state = directory.path().join("studio");
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("vault: {error:?}"));
    let authority_reference = vault
        .store(SecretKindV1::MembershipAuthority, &[0xab; 32])
        .unwrap_or_else(|error| panic!("authority: {error:?}"));
    let runner_authority_reference = vault
        .store(SecretKindV1::RunnerAuthority, &[0xcd; 32])
        .unwrap_or_else(|error| panic!("runner authority: {error:?}"));
    let registry = AssignmentMcpLaunchRegistryV1::open(
        &state.join("assignment-mcp-launches"),
        &state.join("assignment-mcp-progress"),
        FakeLaunchSource {
            authority_reference,
            runner_authority_reference,
        },
        daemon,
        Duration::from_millis(250),
    )
    .unwrap_or_else(|error| panic!("registry: {error:?}"));
    let first_registry = registry.clone();
    let second_registry = registry.clone();
    let first_issue = thread::spawn(move || first_registry.issue(ASSIGNMENT));
    let second_issue = thread::spawn(move || second_registry.issue(ASSIGNMENT));
    let launch_reference = first_issue
        .join()
        .unwrap_or_else(|_| panic!("first issue thread"))
        .unwrap_or_else(|error| panic!("issue: {error:?}"));
    let concurrent_reference = second_issue
        .join()
        .unwrap_or_else(|_| panic!("second issue thread"))
        .unwrap_or_else(|error| panic!("concurrent issue: {error:?}"));
    assert_eq!(concurrent_reference, launch_reference);
    let retried_reference = registry
        .issue(ASSIGNMENT)
        .unwrap_or_else(|error| panic!("retry issue: {error:?}"));
    assert_eq!(retried_reference, launch_reference);
    let registration = fs::read_to_string(
        state
            .join("assignment-mcp-launches")
            .join(format!("{launch_reference}.json")),
    )
    .unwrap_or_else(|error| panic!("registration: {error}"));
    assert!(!registration.contains(BEARER));
    assert!(!registration.contains("runner_authority"));
    assert!(!registration.contains(RUNNER));
    let activation_registration = fs::read_to_string(
        state
            .join("assignment-mcp-launches")
            .join(format!("{launch_reference}.activation.json")),
    )
    .unwrap_or_else(|error| panic!("activation registration: {error}"));
    assert!(activation_registration.contains(RUNNER));
    assert!(!activation_registration.contains(BEARER));
    let first = open_registered_assignment_mcp(&state, &launch_reference)
        .unwrap_or_else(|error| panic!("open first: {error:?}"));
    assert!(open_registered_assignment_mcp(&state, &launch_reference).is_err());
    drop(first);
    let restarted = open_registered_assignment_mcp(&state, &launch_reference)
        .unwrap_or_else(|error| panic!("restart: {error:?}"));
    let tasks = restarted
        .list_assigned_tasks(json!({}))
        .unwrap_or_else(|error| panic!("list: {error:?}"));
    assert_eq!(tasks["tasks"][0]["task_id"], ASSIGNMENT);
    assert_safe(&tasks);
    assert_eq!(
        restarted.next_activation(&json!({"runner_id":RUNNER})),
        Err(assignment_mcp::AssignmentMcpErrorV1::InvalidInput),
    );
    assert_eq!(
        restarted.complete_activation(json!({"activation_id":ASSIGNMENT})),
        Err(assignment_mcp::AssignmentMcpErrorV1::InvalidInput),
    );
    registry
        .revoke(&launch_reference)
        .unwrap_or_else(|error| panic!("revoke: {error:?}"));
    assert_eq!(
        restarted.list_assigned_tasks(json!({})),
        Err(assignment_mcp::AssignmentMcpErrorV1::AssignmentRevoked),
    );
    drop(restarted);
    let replacement_reference = registry
        .issue(ASSIGNMENT)
        .unwrap_or_else(|error| panic!("replacement issue: {error:?}"));
    assert_ne!(replacement_reference, launch_reference);
    runner_fixture
        .join()
        .unwrap_or_else(|_| panic!("Runner fixture thread"));
}

#[allow(clippy::result_large_err)]
fn serve_launch_runner_sessions(listener: &TcpListener, sessions: usize) {
    let expected_authorization = format!("Bearer wsb1:{}", "cd".repeat(32));
    for _ in 0..sessions {
        let (stream, _) = listener
            .accept()
            .unwrap_or_else(|error| panic!("accept Runner fixture: {error}"));
        let expected = expected_authorization.clone();
        let mut socket = accept_hdr(
            stream,
            move |request: &tungstenite::handshake::server::Request,
                  mut response: tungstenite::handshake::server::Response| {
                assert_eq!(request.uri().path(), "/v1/runner/stream");
                assert_eq!(
                    request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some(expected.as_str()),
                );
                response.headers_mut().insert(
                    "Sec-WebSocket-Protocol",
                    worldstream_protocol::WEBSOCKET_SUBPROTOCOL
                        .parse()
                        .unwrap_or_else(|error| panic!("Runner subprotocol: {error}")),
                );
                Ok(response)
            },
        )
        .unwrap_or_else(|error| panic!("Runner fixture handshake: {error}"));
        let _client_hello = socket
            .read()
            .unwrap_or_else(|error| panic!("read Runner client hello: {error}"));
        send_fixture(
            &mut socket,
            "server.welcome",
            &json!({
                "session_id":"01ARZ3NDEKTSV4RRFFQ69G5FAZ",
                "selected_protocol":worldstream_protocol::PROTOCOL_VERSION,
                "server_version":"fixture",
                "heartbeat_interval_ms":1000,
                "maximum_message_bytes":worldstream_protocol::MAX_MESSAGE_BYTES,
                "authenticated_principal":{"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","kind":"agent"}
            }),
        );
        let _runner_hello = socket
            .read()
            .unwrap_or_else(|error| panic!("read Runner hello: {error}"));
        send_fixture(&mut socket, "runner.ready", &json!({"runner_id":RUNNER}));
        while socket.read().is_ok() {}
    }
}

#[test]
fn oversized_pinned_schema_rejects_launch_before_any_reference_is_published() {
    let directory = tempdir().unwrap_or_else(|error| panic!("temporary directory: {error}"));
    let state = directory.path().join("studio");
    let vault = FileSecretVaultV1::open(&state.join("secrets"))
        .unwrap_or_else(|error| panic!("vault: {error:?}"));
    let authority_reference = vault
        .store(SecretKindV1::MembershipAuthority, &[0xab; 32])
        .unwrap_or_else(|error| panic!("authority: {error:?}"));
    let runner_authority_reference = vault
        .store(SecretKindV1::RunnerAuthority, &[0xcd; 32])
        .unwrap_or_else(|error| panic!("runner authority: {error:?}"));
    let launches = state.join("assignment-mcp-launches");
    let registry = AssignmentMcpLaunchRegistryV1::open(
        &launches,
        &state.join("assignment-mcp-progress"),
        FakeLaunchSource {
            authority_reference,
            runner_authority_reference,
        },
        "127.0.0.1:9410"
            .parse()
            .unwrap_or_else(|error| panic!("local daemon: {error}")),
        Duration::from_millis(250),
    )
    .unwrap_or_else(|error| panic!("registry: {error:?}"))
    .with_activity_packs(OversizedPackSource);
    assert_eq!(
        registry.issue(ASSIGNMENT),
        Err(AssignedMembershipSourceErrorV1::Invalid),
    );
    assert_eq!(
        fs::read_dir(launches)
            .unwrap_or_else(|error| panic!("launch directory: {error}"))
            .count(),
        0,
    );
}

#[test]
fn production_gateway_binds_typed_handshake_role_reset_and_projection_integrity() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("fixture listener: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("fixture address: {error}"));
    let fixture = thread::spawn(move || serve_reset_fixture(&listener, false));
    let authority = fake_authority();
    let gateway = FixedDaemonAssignedMembershipGatewayV1::new(address, Duration::from_secs(2));
    let snapshot = gateway
        .synchronize(&authority)
        .unwrap_or_else(|error| panic!("production synchronize: {error:?}"));
    assert_eq!(snapshot.room_head.room_id, ROOM);
    assert!(snapshot.projection_reset.is_some());
    assert_eq!(snapshot.action_offers()[0].action_type, "increment");
    fixture
        .join()
        .unwrap_or_else(|error| panic!("fixture thread: {error:?}"));

    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("wrong-role listener: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("wrong-role address: {error}"));
    let fixture = thread::spawn(move || serve_reset_fixture(&listener, true));
    let gateway = FixedDaemonAssignedMembershipGatewayV1::new(address, Duration::from_secs(2));
    assert_eq!(
        gateway.synchronize(&authority),
        Err(AssignedMembershipGatewayErrorV1::Revoked),
    );
    fixture
        .join()
        .unwrap_or_else(|error| panic!("wrong-role fixture thread: {error:?}"));
}

fn fake_authority() -> AssignedMembershipAuthorityV1 {
    AssignedMembershipAuthorityV1::new(
        ASSIGNMENT,
        "counter-agent",
        "r1",
        "counter",
        "01ARZ3NDEKTSV4RRFFQ69G5FAY",
        ROOM,
        MEMBER,
        SealedCapabilityBearerV1::parse(BEARER.to_owned())
            .unwrap_or_else(|error| panic!("fixture bearer: {error:?}")),
    )
    .unwrap_or_else(|error| panic!("fixture authority: {error:?}"))
}

fn serve_reset_fixture(listener: &TcpListener, wrong_role: bool) {
    let (stream, _) = listener
        .accept()
        .unwrap_or_else(|error| panic!("accept fixture: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap_or_else(|error| panic!("fixture read timeout: {error}"));
    let mut socket = accept_hdr(stream, authorize_fixture_handshake)
        .unwrap_or_else(|error| panic!("fixture handshake: {error}"));
    let _hello = socket
        .read()
        .unwrap_or_else(|error| panic!("read hello: {error}"));
    send_fixture(
        &mut socket,
        "server.welcome",
        &json!({
            "session_id":"01ARZ3NDEKTSV4RRFFQ69G5FAZ",
            "selected_protocol":worldstream_protocol::PROTOCOL_VERSION,
            "server_version":"fixture",
            "heartbeat_interval_ms":1000,
            "maximum_message_bytes":worldstream_protocol::MAX_MESSAGE_BYTES,
            "authenticated_principal":{"principal_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","kind":"agent"}
        }),
    );
    let _attach = socket
        .read()
        .unwrap_or_else(|error| panic!("read attach: {error}"));
    let projection = Projection {
        core: json!({"membership":{"status":"enabled"}}),
        activity: json!({"value":7}),
        action_offers: vec![ActionOffer {
            domain: "worldstream/action-offer/v1".to_owned(),
            action_type: "increment".to_owned(),
            payload_schema_digest: digest('a'),
            eligibility_window: None,
        }],
    };
    let projection_bytes = serde_json::to_vec(&json!({
        "action_offers": [{
            "domain":"worldstream/action-offer/v1",
            "action_type":"increment",
            "payload_schema_digest":digest('a'),
            "eligibility_window":null
        }],
        "authorized_core": &projection.core,
        "projection": &projection.activity,
        "projection_schema": "agent-heist/projection/v1",
    }))
    .unwrap_or_else(|error| panic!("projection JSON: {error}"));
    let canonical = worldstream_core::CanonicalJsonV1::parse(&projection_bytes)
        .and_then(|value| value.to_bytes())
        .unwrap_or_else(|error| panic!("projection canonical: {error}"));
    let projection_hash = worldstream_core::projection_hash_for_canonical_bytes(&canonical)
        .unwrap_or_else(|error| panic!("projection hash: {error}"))
        .to_string();
    send_fixture(
        &mut socket,
        "room.attached",
        &json!({
            "room_id":ROOM,"member_id":MEMBER,"principal_kind":"agent","access_mode":"participant",
            "role":if wrong_role { "other" } else { "counter" },"membership_status":"enabled",
            "room_status":"active","room_health":"healthy","integrity_generation":1,
            "room_head":fixture_head(),"cursor":null,"frame_head":0,"retained_floor":0,
            "sync_token":"fixture-sync-token",
            "sync":{"kind":"projection_reset","baseline_frame_head":0,"reason":"first_attach"},
            "pack":{"id":"worldstream.counter","version":"1.0.0","digest":digest('2')}
        }),
    );
    if wrong_role {
        return;
    }
    send_fixture(
        &mut socket,
        "projection.reset",
        &json!({
            "room_id":ROOM,"member_id":MEMBER,"room_head":fixture_head(),"room_health":"healthy",
            "integrity_generation":1,"baseline_frame_head":0,"reset_reason":"first_attach",
            "projection_schema":"agent-heist/projection/v1",
            "projection":projection,"projection_hash":projection_hash
        }),
    );
    let _sync_ack = socket
        .read()
        .unwrap_or_else(|error| panic!("read sync ACK: {error}"));
    send_fixture(
        &mut socket,
        "room.sync_acked",
        &json!({"through_frame_head":0}),
    );
}

#[expect(
    clippy::result_large_err,
    clippy::unnecessary_wraps,
    reason = "the callback result is fixed by tungstenite's handshake API"
)]
fn authorize_fixture_handshake(
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
        Some(expected.as_str()),
    );
    response.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        worldstream_protocol::WEBSOCKET_SUBPROTOCOL
            .parse()
            .unwrap_or_else(|error| panic!("subprotocol: {error}")),
    );
    Ok(response)
}

fn fixture_head() -> Value {
    json!({
        "room_id":ROOM,"room_seq":4,"genesis_or_transition_hash":digest('1'),
        "core_schema_version":"core.v1","pack_digest":digest('2'),"core_state_hash":digest('3'),
        "activity_state_hash":digest('4'),"authoritative_state_hash":digest('5')
    })
}

fn send_fixture(
    socket: &mut tungstenite::WebSocket<std::net::TcpStream>,
    kind: &str,
    body: &Value,
) {
    socket
        .send(Message::Text(
            json!({
                "protocol":worldstream_protocol::PROTOCOL_VERSION,"type":kind,
                "message_id":"01ARZ3NDEKTSV4RRFFQ69G5FB2","request_id":null,"body":body
            })
            .to_string()
            .into(),
        ))
        .unwrap_or_else(|error| panic!("send fixture {kind}: {error}"));
}

fn snapshot(cursor: u64, frame_head: u64, reset: bool) -> MembershipStreamSnapshotV1 {
    MembershipStreamSnapshotV1 {
        room_head: RoomHead {
            room_id: ROOM.to_owned(),
            room_seq: 4,
            genesis_or_transition_hash: digest('1'),
            core_schema_version: "core.v1".to_owned(),
            pack_digest: digest('2'),
            core_state_hash: digest('3'),
            activity_state_hash: digest('4'),
            authoritative_state_hash: digest('5'),
        },
        pack: PackReference {
            id: "worldstream.counter".to_owned(),
            version: "1.0.0".to_owned(),
            digest: digest('2'),
        },
        cursor: Some(cursor),
        frame_head,
        retained_floor: 0,
        current_action_offers: vec![ActionOffer {
            domain: "worldstream/action-offer/v1".to_owned(),
            action_type: "increment".to_owned(),
            payload_schema_digest: digest('a'),
            eligibility_window: None,
        }],
        projection_reset: reset.then(|| assignment_mcp::ProjectionResetV1 {
            baseline_frame_head: frame_head,
            reset_reason: "retained_range_unavailable".to_owned(),
            projection_schema: "counter.participant.v1".to_owned(),
            projection: Projection {
                core: json!({"membership":{"status":"enabled"}}),
                activity: json!({"value":7}),
                action_offers: vec![ActionOffer {
                    domain: "worldstream/action-offer/v1".to_owned(),
                    action_type: "increment".to_owned(),
                    payload_schema_digest: digest('a'),
                    eligibility_window: None,
                }],
            },
            projection_hash: digest('b'),
        }),
        observations: if reset {
            vec![]
        } else {
            ((cursor + 1)..=frame_head)
                .map(|frame_seq| ObservationDeliveryV1 {
                    frame_seq,
                    cause_room_seq: 4,
                    frame_kind: "transition".to_owned(),
                    observation_schema: "counter.observation.v1".to_owned(),
                    observation: json!({"value":7}),
                    frame_payload_hash: digest('c'),
                })
                .collect()
        },
    }
}

fn digest(character: char) -> String {
    format!("blake3:{}", character.to_string().repeat(64))
}

fn assert_safe(value: &Value) {
    let text = value.to_string();
    assert!(!text.contains(ROOM));
    assert!(!text.contains(MEMBER));
    assert!(!text.contains(BEARER));
    assert!(!text.contains("room_id"));
    assert!(!text.contains("member_id"));
    assert!(!text.contains("bearer"));
}
