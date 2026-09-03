#![cfg(feature = "cli-operator-preview")]

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

use worldstream_protocol::{
    CreateRoomRequest, LobbyLaunchRequest, MAX_MESSAGE_BYTES, RunnerCapabilityProvisionRequestV1,
};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::{
    activity_packs::{DaemonActivityPackSource, HttpDaemonActivityPackSource},
    backups::{
        BackupExecutionErrorV1, BackupStorageProfileV1, HttpDaemonBackupExecutorV1,
        LiveBackupExecutorV1,
    },
    process_ownership::{ProcessOwnership, ProcessRole},
    room_creation::{DaemonRoomCreatorV1, HttpDaemonRoomCreatorV1, RoomCreationAttemptErrorV1},
    rooms::{DaemonRoomSource, HttpDaemonRoomSource},
    runner_attention::{DaemonRunnerAttentionSourceV1, HttpDaemonRunnerAttentionSourceV1},
    secrets::{FileSecretVaultV1, SecretKindV1},
    task_setup::{
        DaemonTaskLaunchSourceV1, DaemonTaskSetupProvisionerV1, HttpDaemonTaskRuntimeV1,
        HttpDaemonTaskSetupProvisionerV1, TaskLaunchAttemptErrorV1, TaskSetupAttemptErrorV1,
    },
    verified_control::ProofRequest,
};

#[test]
fn managed_catalog_sends_host_authority_only_after_proof_on_the_same_socket()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let generation = ownership.reserve(ProcessRole::Runtime)?;
    let mut lease = ownership.claim(ProcessRole::Runtime, generation.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let proof = lease.publish_endpoint(address)?;
    let vault = FileSecretVaultV1::open(&state.join("vault"))?;
    let reference = vault.store(SecretKindV1::HostAuthority, &[0xab; 32])?;
    let source = HttpDaemonActivityPackSource::new_managed(
        address,
        Duration::from_secs(3),
        vault,
        Some(reference),
        ownership,
    );
    let server = thread::spawn(move || -> Result<(), &'static str> {
        // An impostor can occupy the recorded address but cannot sign a proof.
        let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let mut stream = BufReader::new(stream);
        let initial = request(&mut stream)?;
        assert!(initial.line == "POST /api/v1/control/proof HTTP/1.1");
        assert!(
            initial.authorization.is_none(),
            "impostor received authority"
        );
        response(&mut stream, b"{}")?;
        let mut followup = [0_u8; 1024];
        assert!(stream.read(&mut followup).map_err(|_| "followup failed")? == 0);
        drop(stream);

        let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let actual = stream.local_addr().map_err(|_| "local address failed")?;
        let mut stream = BufReader::new(stream);
        let initial = request(&mut stream)?;
        assert!(initial.authorization.is_none());
        let nonce: ProofRequest =
            serde_json::from_slice(&initial.body).map_err(|_| "challenge failed")?;
        let signed = proof.respond(&nonce, actual).map_err(|_| "proof failed")?;
        response(
            &mut stream,
            &serde_json::to_vec(&signed).map_err(|_| "proof encode failed")?,
        )?;
        // No second accept/reconnect is allowed between proof and Host request.
        let catalog = request(&mut stream)?;
        assert!(catalog.line == "GET /v1/operator/activity-packs HTTP/1.1");
        let expected = format!("Bearer wsb1:{}", "ab".repeat(32));
        assert!(catalog.authorization.as_deref() == Some(expected.as_str()));
        response(
            &mut stream,
            br#"{"version":"activity_pack_catalog.v1","revisions":[]}"#,
        )?;
        Ok(())
    });
    assert!(source.catalog().is_err());
    let catalog = source.catalog().map_err(|_| "catalog rejected")?;
    assert!(catalog.revisions.is_empty());
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn managed_catalog_retains_the_exact_whole_http_response_limit()
-> Result<(), Box<dyn std::error::Error>> {
    for (wire_bytes, accepted) in [(MAX_MESSAGE_BYTES, true), (MAX_MESSAGE_BYTES + 1, false)] {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        let generation = ownership.reserve(ProcessRole::Runtime)?;
        let mut lease = ownership.claim(ProcessRole::Runtime, generation.generation())?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let proof = lease.publish_endpoint(address)?;
        let vault = FileSecretVaultV1::open(&state.join("vault"))?;
        let reference = vault.store(SecretKindV1::HostAuthority, &[0xab; 32])?;
        let source = HttpDaemonActivityPackSource::new_managed(
            address,
            Duration::from_secs(3),
            vault,
            Some(reference),
            ownership,
        );
        let server = thread::spawn(move || -> Result<(), &'static str> {
            let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .map_err(|_| "timeout failed")?;
            let actual = stream.local_addr().map_err(|_| "local address failed")?;
            let mut stream = BufReader::new(stream);
            let initial = request(&mut stream)?;
            assert!(initial.authorization.is_none());
            let nonce: ProofRequest =
                serde_json::from_slice(&initial.body).map_err(|_| "challenge failed")?;
            let signed = proof.respond(&nonce, actual).map_err(|_| "proof failed")?;
            response(
                &mut stream,
                &serde_json::to_vec(&signed).map_err(|_| "proof encode failed")?,
            )?;
            assert!(request(&mut stream)?.authorization.is_some());
            let mut body = br#"{"version":"activity_pack_catalog.v1","revisions":[]}"#.to_vec();
            let mut length = wire_bytes;
            loop {
                let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n");
                let remaining = wire_bytes - header.len();
                if remaining == length {
                    break;
                }
                length = remaining;
            }
            body.resize(length, b' ');
            let written = response(&mut stream, &body);
            if accepted {
                written?;
            }
            // Rejection may close immediately after headers, before the fixture
            // can write all deliberately oversized bytes.
            Ok(())
        });
        let result = source.catalog();
        server.join().map_err(|_| "server panicked")??;
        assert!(
            result.is_ok() == accepted,
            "whole-response boundary changed"
        );
    }
    Ok(())
}

#[test]
fn managed_room_inventory_requires_proof_before_host_authority()
-> Result<(), Box<dyn std::error::Error>> {
    managed_host_exchange(
        "GET /v1/operator/rooms?limit=10 HTTP/1.1",
        None,
        200,
        br#"{"rooms":[],"next_after_room_id":null}"#,
        |inputs| {
            HttpDaemonRoomSource::new_managed(
                inputs.address,
                Duration::from_secs(3),
                inputs.vault.clone(),
                Some(inputs.reference.clone()),
                inputs.ownership.clone(),
            )
            .inventory(None, 10)
            .is_ok_and(|page| page.rooms.is_empty())
        },
    )
}

#[test]
fn managed_room_creation_preserves_exact_post_intent_and_rejection_status()
-> Result<(), Box<dyn std::error::Error>> {
    const INTENT: &[u8] = br#"{"pack":{"id":"test.pack","version":"1","digest":"blake3:test"},"configuration":{},"members":[],"idempotency_key":"retained-create-intent"}"#;
    let intent: CreateRoomRequest = serde_json::from_slice(INTENT)?;
    managed_host_exchange(
        "POST /v1/rooms HTTP/1.1",
        Some(INTENT),
        409,
        b"{}",
        |inputs| {
            let creator = HttpDaemonRoomCreatorV1::new_managed(
                inputs.address,
                Duration::from_secs(3),
                inputs.vault.clone(),
                Some(inputs.reference.clone()),
                inputs.ownership.clone(),
            );
            matches!(
                creator.create(&intent),
                Err(RoomCreationAttemptErrorV1::Rejected)
            )
        },
    )
}

#[test]
fn managed_task_launch_keeps_exact_input_and_conflict_semantics_after_proof()
-> Result<(), Box<dyn std::error::Error>> {
    const INPUT: &[u8] = br#"{"input_id":"reviewed-launch-input","based_on_room_seq":7}"#;
    let input: LobbyLaunchRequest = serde_json::from_slice(INPUT)?;
    managed_host_exchange(
        "POST /v1/operator/rooms/01ARZ3NDEKTSV4RRFFQ69G5FAV/lobby/launch HTTP/1.1",
        Some(INPUT),
        409,
        b"{}",
        |inputs| {
            let runtime = HttpDaemonTaskRuntimeV1::new_managed(
                inputs.address,
                Duration::from_secs(3),
                inputs.vault.clone(),
                Some(inputs.reference.clone()),
                inputs.ownership.clone(),
            );
            matches!(
                runtime.launch("01ARZ3NDEKTSV4RRFFQ69G5FAV", &input),
                Err(TaskLaunchAttemptErrorV1::Rejected)
            )
        },
    )
}

#[test]
fn managed_provisioning_sends_sealed_authority_only_on_the_proved_stream()
-> Result<(), Box<dyn std::error::Error>> {
    const INPUT: &[u8] = br#"{"runner_id":"test-runner","owner_principal_id":"test-owner","permitted_memberships":[],"scopes":[],"principal_idempotency_key":"retained-principal","runner_idempotency_key":"retained-runner","capability":{"capability_id":"test-capability","capability_idempotency_key":"retained-capability","bearer":"wsb1:0000000000000000000000000000000000000000000000000000000000000000"},"expires_at":null}"#;
    let input: RunnerCapabilityProvisionRequestV1 = serde_json::from_slice(INPUT)?;
    managed_host_exchange(
        "POST /v1/operator/runner-capabilities:provision HTTP/1.1",
        Some(INPUT),
        403,
        b"{}",
        |inputs| {
            let provisioner = HttpDaemonTaskSetupProvisionerV1::new_managed(
                inputs.address,
                Duration::from_secs(3),
                inputs.vault.clone(),
                Some(inputs.reference.clone()),
                inputs.ownership.clone(),
            );
            matches!(
                provisioner.provision_runner(&input),
                Err(TaskSetupAttemptErrorV1::OperatorFixRequired)
            )
        },
    )
}

#[test]
fn managed_backup_preserves_operation_and_unsupported_status_after_proof()
-> Result<(), Box<dyn std::error::Error>> {
    managed_host_exchange(
        "POST /v1/operator/backups HTTP/1.1",
        Some(br#"{"operation_id":"retained-backup-operation"}"#),
        501,
        b"{}",
        |inputs| {
            let executor = HttpDaemonBackupExecutorV1::new_managed(
                inputs.address,
                Duration::from_secs(3),
                BackupStorageProfileV1::SqliteBundled,
                inputs.vault.clone(),
                Some(inputs.reference.clone()),
                inputs.ownership.clone(),
            );
            let destination = inputs.state.join("must-not-be-created");
            let result = executor.execute("retained-backup-operation", &destination);
            assert!(!destination.exists());
            matches!(result, Err(BackupExecutionErrorV1::Unsupported))
        },
    )
}

#[test]
fn managed_runner_attention_reads_typed_counts_only_after_runtime_proof()
-> Result<(), Box<dyn std::error::Error>> {
    managed_host_exchange(
        "GET /v1/operator/rooms/01ARZ3NDEKTSV4RRFFQ69G5FAV/members/01ARZ3NDEKTSV4RRFFQ69G5FAV/activation-status HTTP/1.1",
        None,
        200,
        br#"{"version":"worldstream/operator-activation-status/v1","waiting":2,"leased":1,"observed_at_unix_ms":7}"#,
        |inputs| {
            let source = HttpDaemonRunnerAttentionSourceV1::new_managed(
                inputs.address, Duration::from_secs(3), inputs.vault.clone(),
                Some(inputs.reference.clone()), inputs.ownership.clone(),
            );
            source.activation_counts("01ARZ3NDEKTSV4RRFFQ69G5FAV", "01ARZ3NDEKTSV4RRFFQ69G5FAV")
                .is_ok_and(|counts| counts.waiting == 2 && counts.leased == 1)
        },
    )
}

struct ManagedInputs {
    state: std::path::PathBuf,
    address: std::net::SocketAddr,
    vault: FileSecretVaultV1,
    reference: worldstream_studio_supervisor::secrets::SecretReferenceV1,
    ownership: ProcessOwnership,
}

fn managed_host_exchange(
    expected_line: &'static str,
    expected_body: Option<&'static [u8]>,
    response_status: u16,
    response_body: &'static [u8],
    invoke: impl Fn(&ManagedInputs) -> bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for valid_proof in [false, true] {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        let generation = ownership.reserve(ProcessRole::Runtime)?;
        let mut lease = ownership.claim(ProcessRole::Runtime, generation.generation())?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let proof = lease.publish_endpoint(address)?;
        let vault = FileSecretVaultV1::open(&state.join("vault"))?;
        let reference = vault.store(SecretKindV1::HostAuthority, &[0xab; 32])?;
        let server = thread::spawn(move || -> Result<(), &'static str> {
            let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .map_err(|_| "timeout failed")?;
            let actual = stream.local_addr().map_err(|_| "local address failed")?;
            let mut stream = BufReader::new(stream);
            let initial = request(&mut stream)?;
            assert!(initial.line == "POST /api/v1/control/proof HTTP/1.1");
            assert!(initial.authorization.is_none(), "authority preceded proof");
            if !valid_proof {
                response(&mut stream, b"{}")?;
                let mut followup = [0_u8; 1024];
                assert!(stream.read(&mut followup).map_err(|_| "followup failed")? == 0);
                return Ok(());
            }
            let nonce: ProofRequest =
                serde_json::from_slice(&initial.body).map_err(|_| "challenge failed")?;
            let signed = proof.respond(&nonce, actual).map_err(|_| "proof failed")?;
            response(
                &mut stream,
                &serde_json::to_vec(&signed).map_err(|_| "proof encode failed")?,
            )?;
            let command = request(&mut stream)?;
            assert!(command.line == expected_line);
            if let Some(expected_body) = expected_body {
                let expected: serde_json::Value =
                    serde_json::from_slice(expected_body).map_err(|_| "expected request failed")?;
                let actual: serde_json::Value =
                    serde_json::from_slice(&command.body).map_err(|_| "request body failed")?;
                assert!(actual == expected, "retained request intent changed");
            }
            let expected = format!("Bearer wsb1:{}", "ab".repeat(32));
            assert!(command.authorization.as_deref() == Some(expected.as_str()));
            status_response(&mut stream, response_status, response_body)?;
            Ok(())
        });
        let accepted = invoke(&ManagedInputs {
            state,
            address,
            vault,
            reference,
            ownership,
        });
        server.join().map_err(|_| "server panicked")??;
        assert!(
            accepted == valid_proof,
            "managed proxy admission or typed response changed"
        );
    }
    Ok(())
}

struct Request {
    line: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

fn request(stream: &mut BufReader<TcpStream>) -> Result<Request, &'static str> {
    let mut line = String::new();
    stream.read_line(&mut line).map_err(|_| "request failed")?;
    let mut result = Request {
        line: line.trim_end().to_owned(),
        authorization: None,
        body: Vec::new(),
    };
    let mut length = 0;
    for _ in 0..32 {
        line.clear();
        stream.read_line(&mut line).map_err(|_| "header failed")?;
        if line == "\r\n" {
            if length > 4096 {
                return Err("request too large");
            }
            result.body.resize(length, 0);
            stream
                .read_exact(&mut result.body)
                .map_err(|_| "body failed")?;
            return Ok(result);
        }
        let (name, value) = line.split_once(':').ok_or("invalid header")?;
        if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().map_err(|_| "invalid length")?;
        } else if name.eq_ignore_ascii_case("authorization") {
            result.authorization = Some(value.trim().to_owned());
        }
    }
    Err("too many headers")
}

fn response(stream: &mut BufReader<TcpStream>, body: &[u8]) -> Result<(), &'static str> {
    status_response(stream, 200, body)
}

fn status_response(
    stream: &mut BufReader<TcpStream>,
    status: u16,
    body: &[u8],
) -> Result<(), &'static str> {
    write!(
        stream.get_mut(),
        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .and_then(|()| stream.get_mut().write_all(body))
    .map_err(|_| "response failed")
}
