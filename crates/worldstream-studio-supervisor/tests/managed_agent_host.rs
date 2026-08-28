#![allow(dead_code)]

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tempfile::tempdir;

use worldstream_studio_supervisor::agent_profiles::ManagedReferenceProviderV1;
use worldstream_studio_supervisor::managed_agent_host::{
    ManagedAgentHostErrorV1, ManagedAgentHostLaunchPlanV1, ManagedAgentHostOperationsV1,
    ManagedAgentHostPreparedLaunchV1, ManagedAgentHostProcessLauncherV1, ManagedAgentHostProcessV1,
    ManagedAgentHostProfileV1, ManagedAgentHostStartSourceV1, OsManagedAgentHostProcessLauncherV1,
};

const ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const SECOND_ASSIGNMENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const SECRET_REFERENCE: &str = "abababababababababababababababababababababababababababababababab";

fn provider_address() -> SocketAddr {
    "127.0.0.1:11434"
        .parse()
        .unwrap_or_else(|error| unreachable!("provider address: {error}"))
}

#[derive(Clone)]
struct FixedStartSource {
    root: std::path::PathBuf,
    capacity: u32,
    stale_after: Duration,
}

impl FixedStartSource {
    fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            capacity: 1,
            stale_after: Duration::from_secs(30),
        }
    }
}

impl ManagedAgentHostStartSourceV1 for FixedStartSource {
    fn prepare(
        &self,
        assignment_id: &str,
    ) -> Result<ManagedAgentHostPreparedLaunchV1, ManagedAgentHostErrorV1> {
        let profile = ManagedAgentHostProfileV1::new(
            assignment_id,
            "reference-agent-host",
            "r1",
            "openai-compatible",
            provider_address(),
            "test-model",
            SECRET_REFERENCE,
            self.capacity,
            self.stale_after,
        )?;
        Ok(ManagedAgentHostPreparedLaunchV1::new(
            profile,
            ManagedAgentHostLaunchPlanV1::new(
                &self.root.join("worldstream-assignment-mcp"),
                &self.root,
                SECRET_REFERENCE,
                &self.root.join("reference-agent-host"),
                "openai-compatible",
                provider_address(),
                "test-model",
            )?,
            zeroize::Zeroizing::new(b"provider-secret".to_vec()),
        ))
    }
}

#[derive(Clone, Default)]
struct RecordingProcessLauncher(Arc<AtomicUsize>);

impl ManagedAgentHostProcessLauncherV1 for RecordingProcessLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        assert_eq!(credential.as_slice(), b"provider-secret");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FixedProcess {
            observed_at_ms: test_now_ms(),
        }))
    }
}

struct FixedProcess {
    observed_at_ms: u64,
}

#[derive(Clone, Copy)]
struct ExitingProcessLauncher;

impl ManagedAgentHostProcessLauncherV1 for ExitingProcessLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        Ok(Box::new(ExitedProcess))
    }
}

struct ExitedProcess;

impl ManagedAgentHostProcessV1 for ExitedProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(Some(17))
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        Ok(())
    }
}

impl ManagedAgentHostProcessV1 for FixedProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(None)
    }

    fn last_activity_at_ms(&self) -> Option<u64> {
        Some(self.observed_at_ms)
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        Ok(())
    }
}

#[cfg(unix)]
#[derive(Clone)]
struct MakeOperationDirectoryReadOnlyLauncher {
    operations: std::path::PathBuf,
}

#[cfg(unix)]
impl ManagedAgentHostProcessLauncherV1 for MakeOperationDirectoryReadOnlyLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(&self.operations, std::fs::Permissions::from_mode(0o500))
            .unwrap_or_else(|error| unreachable!("make operation directory read-only: {error}"));
        Ok(Box::new(FixedProcess {
            observed_at_ms: test_now_ms(),
        }))
    }
}

#[test]
fn two_child_plan_gives_storage_only_to_the_fixed_helper_and_stdio_only_to_the_host() {
    let plan = ManagedAgentHostLaunchPlanV1::new(
        std::path::Path::new("/approved/worldstream-assignment-mcp"),
        std::path::Path::new("/owner/worldstream-state"),
        SECRET_REFERENCE,
        std::path::Path::new("/approved/reference-agent-host"),
        "openai-compatible",
        provider_address(),
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));

    assert_eq!(
        plan.helper_arguments(),
        [
            "--state-dir",
            "/owner/worldstream-state",
            "--launch-reference",
            SECRET_REFERENCE
        ]
    );
    assert_eq!(
        plan.host_arguments(),
        vec![
            "--transport".to_owned(),
            "stdio".to_owned(),
            "--provider".to_owned(),
            "openai-compatible".to_owned(),
            "--provider-address".to_owned(),
            "127.0.0.1:11434".to_owned(),
            "--model".to_owned(),
            "test-model".to_owned()
        ]
    );
    assert!(!plan.host_arguments().iter().any(|argument| {
        argument.contains("state")
            || argument.contains(SECRET_REFERENCE)
            || argument.contains("bearer")
    }));
    assert_eq!(plan.model(), "test-model");
}

#[test]
fn durable_post_setup_operation_surfaces_restart_recovery_without_private_material() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operations_root = root.join("operations");
    let source = FixedStartSource::new(root.clone());
    let launches = RecordingProcessLauncher::default();
    let operations =
        ManagedAgentHostOperationsV1::open_with(&operations_root, source.clone(), launches.clone())
            .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));
    let running = operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("start host: {error:?}"));
    assert!(running.ready);
    assert_eq!(launches.0.load(Ordering::SeqCst), 1);
    drop(operations);

    let restarted =
        ManagedAgentHostOperationsV1::open_with(&operations_root, source, launches.clone())
            .unwrap_or_else(|error| unreachable!("restart host operations: {error:?}"));
    let attention = restarted
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("restart status: {error:?}"));
    assert!(!attention.ready);
    assert_eq!(
        attention.failure.as_ref().map(|value| value.code.as_str()),
        Some("host_restart_required")
    );
    let recovered = restarted
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("retry same host: {error:?}"));
    assert!(recovered.ready);
    assert_eq!(launches.0.load(Ordering::SeqCst), 2);
    let safe = serde_json::to_string(&recovered)
        .unwrap_or_else(|error| unreachable!("serialize status: {error}"));
    for prohibited in [
        SECRET_REFERENCE,
        "provider-secret",
        "launch_reference",
        "prompt",
    ] {
        assert!(!safe.contains(prohibited));
    }
}

#[test]
fn child_exit_is_retained_across_later_status_polls() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operations = ManagedAgentHostOperationsV1::open_with(
        &root.join("managed"),
        FixedStartSource::new(root),
        ExitingProcessLauncher,
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));
    operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("start host: {error:?}"));

    let first = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("first status: {error:?}"));
    assert_eq!(
        first.failure.as_ref().map(|value| value.code.as_str()),
        Some("host_exited")
    );
    assert!(
        first
            .failure
            .as_ref()
            .is_some_and(|value| value.message.contains("17"))
    );

    let later = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("later status: {error:?}"));
    assert_eq!(
        later.failure.as_ref().map(|value| value.code.as_str()),
        Some("host_exited")
    );
}

#[test]
fn silent_live_process_becomes_stale_without_fabricating_a_health_observation() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let source = FixedStartSource {
        root,
        capacity: 1,
        stale_after: Duration::from_millis(100),
    };
    let operations = ManagedAgentHostOperationsV1::open_with(
        &directory.path().join("managed"),
        source,
        SilentProcessLauncher,
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));

    let running = operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("start host: {error:?}"));
    assert!(running.ready);
    std::thread::sleep(Duration::from_millis(150));
    let stale = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("stale status: {error:?}"));

    assert_eq!(
        stale.freshness,
        worldstream_studio_supervisor::managed_agent_host::ManagedAgentHostFreshnessV1::Stale
    );
    assert!(!stale.ready);
}

#[derive(Clone, Copy)]
struct SilentProcessLauncher;

impl ManagedAgentHostProcessLauncherV1 for SilentProcessLauncher {
    fn launch_bridged(
        &self,
        _plan: &ManagedAgentHostLaunchPlanV1,
        _credential: &zeroize::Zeroizing<Vec<u8>>,
    ) -> Result<Box<dyn ManagedAgentHostProcessV1>, ManagedAgentHostErrorV1> {
        Ok(Box::new(SilentProcess))
    }
}

struct SilentProcess;

impl ManagedAgentHostProcessV1 for SilentProcess {
    fn try_wait(&mut self) -> Result<Option<i32>, ManagedAgentHostErrorV1> {
        Ok(None)
    }

    fn stop(&mut self) -> Result<(), ManagedAgentHostErrorV1> {
        Ok(())
    }
}

fn test_now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[test]
fn one_exact_host_capacity_is_shared_by_assignments_using_that_host() {
    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operations = ManagedAgentHostOperationsV1::open_with(
        &directory.path().join("managed"),
        FixedStartSource::new(root),
        RecordingProcessLauncher::default(),
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));

    let first = operations
        .start(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("first host: {error:?}"));
    assert_eq!(first.capacity, 1);
    assert_eq!(first.active_invocations, 1);
    assert_eq!(
        operations.start(SECOND_ASSIGNMENT),
        Err(ManagedAgentHostErrorV1::AtCapacity)
    );
}

#[cfg(unix)]
#[test]
fn live_child_reconciles_starting_checkpoint_after_running_publication_failure() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let root = directory
        .path()
        .canonicalize()
        .unwrap_or_else(|error| unreachable!("root: {error}"));
    let operation_root = root.join("managed");
    let operations_directory = operation_root.join("operations");
    let operations = ManagedAgentHostOperationsV1::open_with(
        &operation_root,
        FixedStartSource::new(root),
        MakeOperationDirectoryReadOnlyLauncher {
            operations: operations_directory.clone(),
        },
    )
    .unwrap_or_else(|error| unreachable!("host operations: {error:?}"));

    assert_eq!(
        operations.start(ASSIGNMENT),
        Err(ManagedAgentHostErrorV1::Unavailable)
    );
    std::fs::set_permissions(
        &operations_directory,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap_or_else(|error| unreachable!("restore operation directory: {error}"));

    let recovered = operations
        .status(ASSIGNMENT)
        .unwrap_or_else(|error| unreachable!("reconcile live process: {error:?}"));
    assert_eq!(
        recovered.state,
        worldstream_studio_supervisor::managed_agent_host::ManagedAgentHostStateV1::Running
    );
    assert!(recovered.ready);
}

#[cfg(unix)]
#[test]
fn production_bridge_reaps_the_peer_when_either_fixed_child_exits() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let helper = directory.path().join("worldstream-assignment-mcp");
    let host = directory.path().join("reference-agent-host");
    let host_pid = directory.path().join("host.pid");
    std::fs::write(&helper, "#!/bin/sh\n/bin/sleep 1\nexit 17\n")
        .unwrap_or_else(|error| unreachable!("helper fixture: {error}"));
    std::fs::write(
        &host,
        format!(
            "#!/bin/sh\necho $$ > '{}'; exec sleep 30\n",
            host_pid.display()
        ),
    )
    .unwrap_or_else(|error| unreachable!("host fixture: {error}"));
    for path in [&helper, &host] {
        let mut permissions = std::fs::metadata(path)
            .unwrap_or_else(|error| unreachable!("fixture metadata: {error}"))
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)
            .unwrap_or_else(|error| unreachable!("fixture permissions: {error}"));
    }
    let plan = ManagedAgentHostLaunchPlanV1::new(
        &helper,
        directory.path(),
        SECRET_REFERENCE,
        &host,
        "openai-compatible",
        provider_address(),
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));
    let mut process = OsManagedAgentHostProcessLauncherV1
        .launch_bridged(&plan, &zeroize::Zeroizing::new(b"provider-secret".to_vec()))
        .unwrap_or_else(|error| unreachable!("launch bridge: {error:?}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let code = loop {
        if let Some(code) = process
            .try_wait()
            .unwrap_or_else(|error| unreachable!("poll bridge: {error:?}"))
        {
            break code;
        }
        assert!(Instant::now() < deadline, "helper exit was not observed");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(code, 17);
    let pid = std::fs::read_to_string(host_pid)
        .unwrap_or_else(|error| unreachable!("host peer pid was not published: {error}"));
    let status = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|error| unreachable!("inspect peer process: {error}"));
    assert!(!status.success(), "host peer remained alive");
}

#[cfg(unix)]
#[test]
fn dropping_a_live_bridge_reaps_both_children() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let helper = directory.path().join("worldstream-assignment-mcp");
    let host = directory.path().join("reference-agent-host");
    let helper_pid = directory.path().join("helper.pid");
    let host_pid = directory.path().join("host.pid");
    for (path, pid) in [(&helper, &helper_pid), (&host, &host_pid)] {
        std::fs::write(
            path,
            format!("#!/bin/sh\necho $$ > '{}'; exec sleep 30\n", pid.display()),
        )
        .unwrap_or_else(|error| unreachable!("child fixture: {error}"));
        let mut permissions = std::fs::metadata(path)
            .unwrap_or_else(|error| unreachable!("fixture metadata: {error}"))
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions)
            .unwrap_or_else(|error| unreachable!("fixture permissions: {error}"));
    }
    let plan = ManagedAgentHostLaunchPlanV1::new(
        &helper,
        directory.path(),
        SECRET_REFERENCE,
        &host,
        "openai-compatible",
        provider_address(),
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));
    let process = OsManagedAgentHostProcessLauncherV1
        .launch_bridged(&plan, &zeroize::Zeroizing::new(b"provider-secret".to_vec()))
        .unwrap_or_else(|error| unreachable!("launch bridge: {error:?}"));
    for _ in 0..100 {
        if helper_pid.exists() && host_pid.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(process);
    for pid_path in [helper_pid, host_pid] {
        if let Ok(pid) = std::fs::read_to_string(pid_path) {
            let status = std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .unwrap_or_else(|error| unreachable!("inspect child process: {error}"));
            assert!(!status.success(), "bridged child remained alive");
        }
    }
}

#[cfg(unix)]
#[test]
#[allow(clippy::too_many_lines)]
fn separate_reference_host_process_completes_one_real_stdio_mcp_turn_via_loopback_provider() {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        os::unix::fs::PermissionsExt as _,
    };

    let directory = tempdir().unwrap_or_else(|error| unreachable!("temporary root: {error}"));
    let helper = directory.path().join("worldstream-assignment-mcp");
    let calls = directory.path().join("calls.jsonl");
    let script = format!(
        r#"#!/usr/bin/python3
import json, sys
calls = open({calls:?}, "a", buffering=1)
completed = False
terminal_submission = False
activation_polls = 0
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    calls.write(json.dumps({{"method":method}}) + "\n")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        result = {{"protocolVersion":"2025-06-18"}}
    else:
        params = request["params"]
        name = params["name"]
        arguments = params["arguments"]
        calls.write(json.dumps({{"name":name,"arguments":arguments}}) + "\n")
        if name == "worldstream.prepare_managed_turn":
            activation_polls += 1
            if activation_polls == 1 or completed:
                result = {{"content":[],"structuredContent":{{"code":"assignment_activation_none_available","retryable":True,"next_action":"wait_then_request_next_activation"}},"isError":True}}
                print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
                continue
            if terminal_submission:
                completed = True
                value = {{"schema":"worldstream/managed-turn-preparation/v1","state":"reconciled"}}
                result = {{"content":[],"structuredContent":value,"isError":False}}
                print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
                continue
            value = {{"schema":"worldstream/managed-turn-preparation/v1","instruction":"Select exactly one listed offer and return its offer_id and payload.","observation":{{"observations":[{{"frame_seq":9,"observation":{{"turn":3}}}}]}},"offers":{{"precondition":{{"room_seq":7,"head_hash":"blake3:" + "b"*64}},"offers":[{{"offer_id":"7:0:digest","action_type":"increment","payload_schema":{{"schema":{{"type":"object"}}}}}}]}}}}
        elif name == "worldstream.submit_managed_turn_action":
            if terminal_submission:
                raise RuntimeError("unexpected duplicate managed submission")
            terminal_submission = True
            result = {{"content":[],"structuredContent":{{"code":"assignment_activation_lease_expired","retryable":True,"next_action":"request_next_activation"}},"isError":True}}
            print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
            continue
        else:
            raise RuntimeError("unexpected tool")
        result = {{"content":[],"structuredContent":value,"isError":False}}
    print(json.dumps({{"jsonrpc":"2.0","id":request["id"],"result":result}}), flush=True)
"#,
        calls = calls.display()
    );
    std::fs::write(&helper, script).unwrap_or_else(|error| unreachable!("helper fixture: {error}"));
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))
        .unwrap_or_else(|error| unreachable!("helper permissions: {error}"));

    let provider = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| unreachable!("provider listener: {error}"));
    let provider_address = provider
        .local_addr()
        .unwrap_or_else(|error| unreachable!("provider address: {error}"));
    let provider_thread = std::thread::spawn(move || {
        let (mut stream, _) = provider
            .accept()
            .unwrap_or_else(|error| unreachable!("provider accept: {error}"));
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap_or_else(|error| unreachable!("provider timeout: {error}"));
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream
                .read(&mut buffer)
                .unwrap_or_else(|error| unreachable!("provider request: {error}"));
            request.extend_from_slice(&buffer[..read]);
            let complete = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .and_then(|boundary| {
                    let headers = std::str::from_utf8(&request[..boundary]).ok()?;
                    let length = headers.lines().find_map(|line| {
                        line.strip_prefix("Content-Length: ")?.parse::<usize>().ok()
                    })?;
                    Some(request.len() >= boundary + 4 + length)
                })
                .unwrap_or(false);
            if read == 0 || complete {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.contains("Authorization: Bearer provider-secret"));
        assert!(request_text.contains("7:0:digest"));
        let body = r#"{"choices":[{"message":{"content":"{\"offer_id\":\"7:0:digest\",\"payload\":{\"amount\":1}}"}}]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap_or_else(|error| unreachable!("provider response: {error}"));
    });

    let host = std::path::PathBuf::from(
        std::env::var("CARGO_BIN_EXE_worldstream-managed-agent-host")
            .unwrap_or_else(|error| unreachable!("managed host binary: {error}")),
    );
    let plan = ManagedAgentHostLaunchPlanV1::new_managed_reference(
        &helper,
        directory.path(),
        SECRET_REFERENCE,
        &host,
        ManagedReferenceProviderV1::OpenAiCompatible,
        provider_address,
        "test-model",
    )
    .unwrap_or_else(|error| unreachable!("launch plan: {error:?}"));
    let mut process = OsManagedAgentHostProcessLauncherV1
        .launch_bridged(&plan, &zeroize::Zeroizing::new(b"provider-secret".to_vec()))
        .unwrap_or_else(|error| unreachable!("launch host: {error:?}"));
    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        let managed_turn_calls = std::fs::read_to_string(&calls).map_or(0, |calls| {
            calls
                .lines()
                .filter(|line| line.contains("worldstream.prepare_managed_turn"))
                .count()
        });
        if managed_turn_calls >= 4 {
            break;
        }
        assert_eq!(
            process
                .try_wait()
                .unwrap_or_else(|error| unreachable!("poll host: {error:?}")),
            None
        );
        assert!(
            Instant::now() < deadline,
            "host did not recover the terminal post-model lease; calls={}",
            std::fs::read_to_string(&calls).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    process
        .stop()
        .unwrap_or_else(|error| unreachable!("stop host: {error:?}"));
    provider_thread
        .join()
        .unwrap_or_else(|_| unreachable!("provider thread panicked"));

    let retained_calls =
        std::fs::read_to_string(calls).unwrap_or_else(|error| unreachable!("read calls: {error}"));
    for tool in [
        "worldstream.prepare_managed_turn",
        "worldstream.submit_managed_turn_action",
    ] {
        assert!(retained_calls.contains(tool), "missing tool {tool}");
    }
    for prohibited in [SECRET_REFERENCE, "provider-secret", "state-dir", "bearer"] {
        assert!(!retained_calls.contains(prohibited));
    }
    let calls = retained_calls
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .zip(value.get("arguments").cloned())
        })
        .collect::<Vec<_>>();
    assert!(calls.iter().all(|(name, _)| {
        !matches!(
            name.as_str(),
            "worldstream.next_activation"
                | "worldstream.observe"
                | "worldstream.acknowledge"
                | "worldstream.list_current_action_offers"
                | "worldstream.submit_action"
                | "worldstream.complete_activation"
        )
    }));
    let completion = &calls
        .iter()
        .find(|(name, _)| name == "worldstream.submit_managed_turn_action")
        .unwrap_or_else(|| unreachable!("managed turn submission"))
        .1;
    assert_eq!(
        completion
            .get("offer_id")
            .and_then(serde_json::Value::as_str),
        Some("7:0:digest")
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(name, _)| name == "worldstream.submit_managed_turn_action")
            .count(),
        1,
        "terminal recovery must reuse the retained Action without another model-selected submission"
    );
}
