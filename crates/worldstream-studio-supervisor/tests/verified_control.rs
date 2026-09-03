use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use worldstream_runtime::prepare_data_directory;
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    process_ownership::{ProcessOwnership, ProcessRole},
    verified_control::{ProofRequest, VerifiedConnection},
};

#[test]
fn controller_proves_the_actual_socket_before_receiving_control_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let control = ControlAccess::initialize(&state)?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, launch.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let service = lease.publish_endpoint(listener.local_addr()?)?;
    let server_control = ControlAccess::open(&state)?;
    let server = thread::spawn(move || -> Result<(), &'static str> {
        let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let actual = stream.local_addr().map_err(|_| "local address failed")?;
        let mut stream = BufReader::new(stream);
        let proof_request = request(&mut stream)?;
        assert!(proof_request.line == "POST /api/v1/control/proof HTTP/1.1");
        assert!(!proof_request.headers.contains_key("authorization"));
        let challenge: ProofRequest =
            serde_json::from_slice(&proof_request.body).map_err(|_| "challenge failed")?;
        let proof = service
            .respond(&challenge, actual)
            .map_err(|_| "proof failed")?;
        response(
            &mut stream,
            &serde_json::to_vec(&proof).map_err(|_| "proof encode failed")?,
        )?;
        let command = request(&mut stream)?;
        assert!(command.line == "GET /api/v1/control/status HTTP/1.1");
        assert!(
            server_control
                .authenticate(&command.headers)
                .map_err(|_| "control read failed")?
        );
        response(&mut stream, b"{\"ready\":true}")?;
        Ok(())
    });
    let connection =
        VerifiedConnection::connect(&ownership, ProcessRole::Controller, Duration::from_secs(3))?;
    let result = connection.request("GET", "/api/v1/control/status", b"", || {
        control.authorization_header()
    })?;
    assert_eq!(result.status, 200);
    assert!(result.body == b"{\"ready\":true}");
    server.join().map_err(|_| "server panicked")??;
    drop(lease);
    Ok(())
}

#[test]
fn runtime_control_authority_is_separate_from_controller_access()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let control = ControlAccess::initialize(&state)?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    let mut lease = ownership.claim(ProcessRole::Runtime, launch.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let service = lease.publish_endpoint(listener.local_addr()?)?;
    let mut controller_headers = HeaderMap::new();
    controller_headers.insert("authorization", control.authorization_header()?);
    assert!(!service.authenticate_runtime(&controller_headers));
    let server = thread::spawn(move || -> Result<(), &'static str> {
        let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let actual = stream.local_addr().map_err(|_| "local address failed")?;
        let mut stream = BufReader::new(stream);
        let challenge = request(&mut stream)?;
        assert!(!challenge.headers.contains_key("authorization"));
        let challenge: ProofRequest =
            serde_json::from_slice(&challenge.body).map_err(|_| "challenge failed")?;
        let proof = service
            .respond(&challenge, actual)
            .map_err(|_| "proof failed")?;
        response(
            &mut stream,
            &serde_json::to_vec(&proof).map_err(|_| "proof encode failed")?,
        )?;
        let command = request(&mut stream)?;
        assert!(service.authenticate_runtime(&command.headers));
        assert!(
            !control
                .authenticate(&command.headers)
                .map_err(|_| "control read failed")?
        );
        response(&mut stream, b"{}")?;
        Ok(())
    });
    let connection =
        VerifiedConnection::connect(&ownership, ProcessRole::Runtime, Duration::from_secs(3))?;
    let result = connection.request_runtime("POST", "/api/v1/control/stop", b"{}")?;
    assert_eq!(result.status, 200);
    server.join().map_err(|_| "server panicked")??;
    Ok(())
}

#[test]
fn slow_drip_proof_cannot_extend_the_total_connection_deadline()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, launch.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let _service = lease.publish_endpoint(listener.local_addr()?)?;
    let server = thread::spawn(move || -> Result<(), &'static str> {
        let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let mut stream = BufReader::new(stream);
        let challenge = request(&mut stream)?;
        assert!(!challenge.headers.contains_key("authorization"));
        // Every byte arrives before an individual read timeout; only a whole
        // exchange deadline stops this peer promptly.
        for byte in
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
        {
            if stream.get_mut().write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    });
    let started = Instant::now();
    let result = VerifiedConnection::connect(
        &ownership,
        ProcessRole::Controller,
        Duration::from_millis(120),
    );
    let elapsed = started.elapsed();
    server.join().map_err(|_| "server panicked")??;
    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_millis(700),
        "slow peer extended the total connection deadline"
    );
    Ok(())
}

#[test]
fn invalid_proof_fields_never_receive_a_followup_bearer() -> Result<(), Box<dyn std::error::Error>>
{
    for case in [
        "nonce",
        "role",
        "generation",
        "installation",
        "endpoint",
        "pid",
        "mac",
    ] {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        let launch = ownership.reserve(ProcessRole::Controller)?;
        let mut lease = ownership.claim(ProcessRole::Controller, launch.generation())?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let service = lease.publish_endpoint(listener.local_addr()?)?;
        let server = thread::spawn(move || -> Result<(), &'static str> {
            let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .map_err(|_| "timeout failed")?;
            let actual = stream.local_addr().map_err(|_| "local address failed")?;
            let mut stream = BufReader::new(stream);
            let initial = request(&mut stream)?;
            assert!(!initial.headers.contains_key("authorization"));
            let mut challenge: ProofRequest =
                serde_json::from_slice(&initial.body).map_err(|_| "challenge failed")?;
            if case == "nonce" {
                challenge.nonce = "0".repeat(64);
            }
            let proof = service
                .respond(&challenge, actual)
                .map_err(|_| "proof failed")?;
            let mut document = serde_json::to_value(proof).map_err(|_| "proof encode failed")?;
            match case {
                "role" => document["role"] = serde_json::json!("runtime"),
                "generation" | "installation" | "mac" => {
                    document[case] = serde_json::json!("0".repeat(64))
                }
                "endpoint" => document["endpoint"] = serde_json::json!("127.0.0.1:1"),
                "pid" => document["pid"] = serde_json::json!(0),
                "nonce" => {}
                _ => return Err("unknown proof case"),
            }
            response(
                &mut stream,
                &serde_json::to_vec(&document).map_err(|_| "proof encode failed")?,
            )?;
            let mut followup = [0_u8; 2048];
            let count = stream
                .read(&mut followup)
                .map_err(|_| "followup read failed")?;
            assert!(count == 0, "invalid proof received a followup request");
            Ok(())
        });
        let result = VerifiedConnection::connect(
            &ownership,
            ProcessRole::Controller,
            Duration::from_secs(3),
        )
        .and_then(|connection| {
            connection.request("GET", "/api/v1/control/status", b"", || {
                Ok::<_, ()>(HeaderValue::from_static("Bearer PROOF_REJECTION_CANARY"))
            })
        });
        assert!(result.is_err(), "invalid proof was accepted for {case}");
        server.join().map_err(|_| "server panicked")??;
    }
    Ok(())
}

#[test]
fn proof_never_moves_to_another_socket_or_outlives_its_lease()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    let mut lease = ownership.claim(ProcessRole::Runtime, launch.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let actual = listener.local_addr()?;
    let service = lease.publish_endpoint(actual)?;
    let challenge = ProofRequest {
        nonce: "1".repeat(64),
    };
    assert!(service.respond(&challenge, actual).is_ok());
    assert!(service.respond(&challenge, "127.0.0.1:0".parse()?).is_err());
    drop(lease);
    assert!(service.respond(&challenge, actual).is_err());
    assert!(!ownership.is_leased(ProcessRole::Runtime)?);
    assert!(ownership.reserve(ProcessRole::Runtime).is_err());
    Ok(())
}

#[test]
fn old_runtime_bearer_is_disabled_before_listener_release_and_not_reused()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Runtime)?;
    let mut lease = ownership.claim(ProcessRole::Runtime, launch.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let actual = listener.local_addr()?;
    let service = lease.publish_endpoint(actual)?;
    let server_service = service.clone();
    let server = thread::spawn(move || -> Result<(HeaderMap, TcpListener), &'static str> {
        let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let actual = stream.local_addr().map_err(|_| "local address failed")?;
        let mut stream = BufReader::new(stream);
        let initial = request(&mut stream)?;
        let challenge: ProofRequest =
            serde_json::from_slice(&initial.body).map_err(|_| "challenge failed")?;
        let proof = server_service
            .respond(&challenge, actual)
            .map_err(|_| "proof failed")?;
        response(
            &mut stream,
            &serde_json::to_vec(&proof).map_err(|_| "proof encode failed")?,
        )?;
        let command = request(&mut stream)?;
        assert!(server_service.authenticate_runtime(&command.headers));
        response(&mut stream, b"{}")?;
        Ok((command.headers, listener))
    });
    VerifiedConnection::connect(&ownership, ProcessRole::Runtime, Duration::from_secs(3))?
        .request_runtime("GET", "/api/v1/control/status", b"")?;
    let (headers, listener) = server.join().map_err(|_| "server panicked")??;
    assert!(service.authenticate_runtime(&headers));
    let mut duplicate = headers.clone();
    duplicate.append(
        "authorization",
        headers.get("authorization").ok_or("missing auth")?.clone(),
    );
    assert!(!service.authenticate_runtime(&duplicate));
    assert!(!service.authenticate_runtime(&HeaderMap::new()));
    service.disable_for_shutdown();
    assert!(!service.authenticate_runtime(&headers));
    assert!(ownership.is_leased(ProcessRole::Runtime)?);
    lease.finish(worldstream_studio_supervisor::process_ownership::ProcessTermination::Stopped)?;
    let replacement = ownership.reserve(ProcessRole::Runtime)?;
    let mut replacement_lease = ownership.claim(ProcessRole::Runtime, replacement.generation())?;
    // Keep this owned address fixed to prove that changing the generation key,
    // not merely changing the port, invalidates the previously observed bearer.
    let replacement_service = replacement_lease.publish_endpoint(listener.local_addr()?)?;
    assert!(!replacement_service.authenticate_runtime(&headers));
    Ok(())
}

#[test]
fn incomplete_or_ambiguous_proof_framing_never_receives_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let fixtures: &[&[u8]] = &[
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{}",
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n{}",
        b"HTTP/1.1 200 OK\r\nContent-Length: 4097\r\n\r\n",
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\n\r\n",
    ];
    for &fixture in fixtures {
        let temporary = tempfile::tempdir()?;
        let state = prepare_data_directory(&temporary.path().join("state"))?;
        let ownership = ProcessOwnership::open(&state)?;
        let launch = ownership.reserve(ProcessRole::Controller)?;
        let mut lease = ownership.claim(ProcessRole::Controller, launch.generation())?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let _service = lease.publish_endpoint(listener.local_addr()?)?;
        let server = thread::spawn(move || -> Result<(), &'static str> {
            let (stream, _) = listener.accept().map_err(|_| "accept failed")?;
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .map_err(|_| "timeout failed")?;
            let mut stream = BufReader::new(stream);
            let initial = request(&mut stream)?;
            assert!(!initial.headers.contains_key("authorization"));
            stream
                .get_mut()
                .write_all(fixture)
                .map_err(|_| "fixture write failed")?;
            stream
                .get_ref()
                .shutdown(std::net::Shutdown::Write)
                .map_err(|_| "fixture shutdown failed")?;
            let mut followup = [0_u8; 2048];
            assert!(
                stream.read(&mut followup).map_err(|_| "followup failed")? == 0,
                "malformed proof received authority"
            );
            Ok(())
        });
        let result = VerifiedConnection::connect(
            &ownership,
            ProcessRole::Controller,
            Duration::from_secs(3),
        )
        .and_then(|connection| {
            connection.request("GET", "/api/v1/control/status", b"", || {
                Ok::<_, ()>(HeaderValue::from_static("Bearer FRAMING_REJECTION_CANARY"))
            })
        });
        assert!(result.is_err());
        server.join().map_err(|_| "server panicked")??;
    }
    Ok(())
}

#[test]
fn maximum_operator_deadline_is_accepted_and_larger_budget_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    use worldstream_studio_supervisor::verified_control::ControlTransportError;
    let temporary = tempfile::tempdir()?;
    let state = prepare_data_directory(&temporary.path().join("state"))?;
    let ownership = ProcessOwnership::open(&state)?;
    let launch = ownership.reserve(ProcessRole::Controller)?;
    let mut lease = ownership.claim(ProcessRole::Controller, launch.generation())?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let service = lease.publish_endpoint(listener.local_addr()?)?;
    assert!(matches!(
        VerifiedConnection::connect(
            &ownership,
            ProcessRole::Controller,
            Duration::from_secs(301)
        ),
        Err(ControlTransportError::Protocol)
    ));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    let server = thread::spawn(move || -> Result<TcpListener, &'static str> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return Err("proof connection did not arrive"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|_| "timeout failed")?;
        let actual = stream.local_addr().map_err(|_| "local address failed")?;
        let mut stream = BufReader::new(stream);
        let initial = request(&mut stream)?;
        let challenge: ProofRequest =
            serde_json::from_slice(&initial.body).map_err(|_| "challenge failed")?;
        let proof = service
            .respond(&challenge, actual)
            .map_err(|_| "proof failed")?;
        response(
            &mut stream,
            &serde_json::to_vec(&proof).map_err(|_| "proof encode failed")?,
        )?;
        Ok(listener)
    });
    let connection = VerifiedConnection::connect(
        &ownership,
        ProcessRole::Controller,
        Duration::from_secs(300),
    );
    let server_result = server.join().map_err(|_| "server panicked")?;
    assert!(
        connection.is_ok(),
        "maximum approved operator deadline was rejected"
    );
    let listener = server_result?;
    drop(connection);
    drop(lease);
    drop(listener);
    Ok(())
}

struct Request {
    line: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

fn request(stream: &mut BufReader<TcpStream>) -> Result<Request, &'static str> {
    let mut line = String::new();
    stream
        .read_line(&mut line)
        .map_err(|_| "request line failed")?;
    let mut headers = HeaderMap::new();
    loop {
        let mut header = String::new();
        stream
            .read_line(&mut header)
            .map_err(|_| "request header failed")?;
        if header == "\r\n" {
            break;
        }
        if header.is_empty() || header.len() > 8192 {
            return Err("request headers incomplete");
        }
        let (name, value) = header.trim_end().split_once(':').ok_or("invalid header")?;
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| "invalid header name")?,
            HeaderValue::from_str(value.trim()).map_err(|_| "invalid header value")?,
        );
    }
    let length: usize = headers
        .get("content-length")
        .ok_or("missing length")?
        .to_str()
        .map_err(|_| "invalid length")?
        .parse()
        .map_err(|_| "invalid length")?;
    if length > 4096 {
        return Err("request too large");
    }
    let mut body = vec![0; length];
    stream
        .read_exact(&mut body)
        .map_err(|_| "request body failed")?;
    Ok(Request {
        line: line.trim_end().to_owned(),
        headers,
        body,
    })
}

fn response(stream: &mut BufReader<TcpStream>, body: &[u8]) -> Result<(), &'static str> {
    write!(
        stream.get_mut(),
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
        body.len()
    )
    .map_err(|_| "response header failed")?;
    stream
        .get_mut()
        .write_all(body)
        .map_err(|_| "response body failed")
}
