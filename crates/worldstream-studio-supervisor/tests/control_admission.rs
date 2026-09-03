use std::{net::TcpListener, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;
use worldstream_studio_supervisor::{
    HttpDaemonStatusSource, control_access::ControlAccess,
    control_admission::protect_operator_routes, supervisor_router,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct UnavailableSeatAuthority(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl worldstream_studio_supervisor::participant_handoff::ParticipantHandoffAuthoritySourceV1
    for UnavailableSeatAuthority
{
    fn resolve_provisioned_human_seat(
        &self,
        _draft_id: &str,
        _seat_id: &str,
    ) -> Result<
        worldstream_studio_supervisor::participant_handoff::HumanSeatAuthorityV1,
        worldstream_studio_supervisor::participant_handoff::ParticipantHandoffAuthorityErrorV1,
    > {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(worldstream_studio_supervisor::participant_handoff::ParticipantHandoffAuthorityErrorV1::Unavailable)
    }
}

#[tokio::test]
async fn operator_status_requires_control_authentication_before_probing_the_daemon() -> TestResult {
    let directory = tempfile::tempdir()?;
    let control = ControlAccess::initialize(&directory.path().join("state"))?;
    let reserved_daemon = TcpListener::bind("127.0.0.1:0")?;
    let source =
        HttpDaemonStatusSource::new(reserved_daemon.local_addr()?, Duration::from_millis(10));
    let router = protect_operator_routes(supervisor_router(source), control);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/daemon/status")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let body = response.into_body().collect().await?.to_bytes();
    assert_eq!(body.as_ref(), br#"{"schema":"worldstream/operator-control-error/v1","code":"control_authentication_required","message":"Operator control authentication is required."}"#);
    Ok(())
}

#[tokio::test]
async fn exact_browser_session_keeps_its_existing_membership_admission() -> TestResult {
    use worldstream_studio_supervisor::{
        client_bindings::{ClientBindingStoreV1, ClientDeploymentTrustPolicyV1},
        participant_handoff::{
            FixedDaemonParticipantConsoleGatewayV1, ParticipantHandoffBrokerV1,
            participant_handoff_router,
        },
    };
    let directory = tempfile::tempdir()?;
    let control = ControlAccess::initialize(&directory.path().join("state"))?;
    let reserved_daemon = TcpListener::bind("127.0.0.1:0")?;
    let clients = ClientBindingStoreV1::open_installed(
        &directory.path().join("clients"),
        ClientDeploymentTrustPolicyV1::VerifiedOnly,
    )?;
    let authority_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let broker = ParticipantHandoffBrokerV1::new(
        "http://127.0.0.1:5174",
        "http://127.0.0.1:5173",
        Duration::from_secs(90),
        16,
        UnavailableSeatAuthority(authority_calls.clone()),
        FixedDaemonParticipantConsoleGatewayV1::new(
            reserved_daemon.local_addr()?,
            Duration::from_millis(10),
        ),
        clients,
    )
    .map_err(|_| "bounded browser broker fixture is invalid")?;
    let router = protect_operator_routes(participant_handoff_router(broker), control);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/participant-console/session")
                .header(header::ORIGIN, "http://127.0.0.1:5173")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().collect().await?.to_bytes();
    assert!(std::str::from_utf8(&body)?.contains("participant_session_missing"));
    assert_eq!(authority_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    Ok(())
}

mod production {
    use std::{
        io::{Read as _, Write as _},
        net::{SocketAddr, TcpStream},
        process::{Child, Command, Stdio},
        thread,
        time::Instant,
    };

    use worldstream_runtime::CliOverrides;
    use worldstream_studio_supervisor::local_initialization::{
        InitializationRequest, initialize_local,
    };

    use super::*;

    // Literal inventory of the production graph, including routes merged by main.
    // Authenticated TRACE checks that these are real registered paths without
    // invoking their domain handlers. A malformed body keeps admission regressions
    // from reaching mutation handlers during the unauthenticated matrix.
    const OPERATOR_ROUTES: &[(&str, &[&str])] = &[
        ("/api/v1/daemon/status", &["GET"]),
        ("/api/v1/daemon/lifecycle", &["GET"]),
        ("/api/v1/daemon/start", &["POST"]),
        ("/api/v1/daemon/stop", &["POST"]),
        ("/api/v1/daemon/restart", &["POST"]),
        ("/api/v1/secrets", &["GET"]),
        ("/api/v1/secrets/host/public-ref", &["GET"]),
        ("/api/v1/runner-templates", &["GET"]),
        ("/api/v1/runner-instances", &["GET"]),
        ("/api/v1/runner-instances/public-ref/start", &["POST"]),
        ("/api/v1/runner-instances/public-ref/stop", &["POST"]),
        ("/api/v1/runner-instances/public-ref/restart", &["POST"]),
        ("/api/v1/activity-packs", &["GET"]),
        ("/api/v1/activity-packs/public-ref", &["GET"]),
        ("/api/v1/rooms", &["GET"]),
        ("/api/v1/rooms/public-ref", &["GET"]),
        ("/api/v1/room-drafts/public-ref", &["GET", "PUT"]),
        ("/api/v1/backups/health", &["GET"]),
        ("/api/v1/backups/public-ref", &["GET", "POST"]),
        ("/api/v1/room-creations/public-ref", &["GET"]),
        ("/api/v1/room-creations/public-ref/start", &["POST"]),
        ("/api/v1/room-creations/public-ref/retry", &["POST"]),
        ("/api/v1/task-setups/public-ref", &["GET"]),
        ("/api/v1/task-setups/public-ref/start", &["POST"]),
        ("/api/v1/task-setups/public-ref/retry", &["POST"]),
        ("/api/v1/task-setups/public-ref/launch", &["POST"]),
        ("/api/v1/agent-profiles", &["GET", "POST"]),
        ("/api/v1/agent-profiles/public-ref/revisions/1", &["GET"]),
        ("/api/v1/agent-profile-assignments", &["GET"]),
        ("/api/v1/model-provider-credentials", &["GET"]),
        ("/api/v1/participant-console/handoffs", &["POST", "OPTIONS"]),
        ("/api/v1/task-templates", &["GET", "POST"]),
        ("/api/v1/task-templates/public-ref/revisions/1", &["GET"]),
        (
            "/api/v1/task-templates/public-ref/revisions/1/instantiate",
            &["POST"],
        ),
        (
            "/v1/agent-profile-assignments/public-ref/mcp-launches",
            &["POST"],
        ),
        ("/v1/assignment-mcp-launches/public-ref", &["DELETE"]),
        ("/api/v1/managed-agent-hosts/public-ref", &["GET"]),
        ("/api/v1/managed-agent-hosts/public-ref/start", &["POST"]),
        ("/api/v1/managed-agent-hosts/public-ref/retry", &["POST"]),
        ("/api/v1/managed-agent-hosts/public-ref/stop", &["POST"]),
        (
            "/api/v1/rooms/public-ref/agent-seats/public-seat/managed-host/start",
            &["POST"],
        ),
        (
            "/api/v1/rooms/public-ref/agent-seats/public-seat/managed-host/retry",
            &["POST"],
        ),
        (
            "/api/v1/rooms/public-ref/agent-seats/public-seat/managed-host/stop",
            &["POST"],
        ),
        ("/api/v1/runner-attention", &["GET"]),
        ("/api/v1/rooms/public-ref/agent-attention", &["GET"]),
        ("/api/v1/runner-attention/public-ref/restart", &["POST"]),
        ("/api/v1/attention-inbox", &["GET"]),
    ];

    struct Supervisor {
        child: Child,
        address: SocketAddr,
        control: ControlAccess,
        _daemon: TcpListener,
        _directory: tempfile::TempDir,
    }

    impl Supervisor {
        fn start(require_control_assertion: bool) -> Result<Self, Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let config = directory.path().join(".worldstream/worldstream.toml");
            let state = directory.path().join(".worldstream/studio");
            initialize_local(&InitializationRequest {
                config: Some(config.clone()),
                overrides: CliOverrides::default(),
                state_dir: state.clone(),
                working_directory: directory.path().to_path_buf(),
                environment: Vec::new(),
                preview: false,
            })?;
            let control = ControlAccess::open(&state)?;
            let daemon = TcpListener::bind("127.0.0.1:0")?;
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let address = listener.local_addr()?;
            let mut command = Command::new(env!("CARGO_BIN_EXE_worldstream-studio-supervisor"));
            for (name, _) in std::env::vars_os() {
                if name.to_string_lossy().starts_with("WORLDSTREAM_") {
                    command.env_remove(name);
                }
            }
            if require_control_assertion {
                command.arg("--require-operator-control");
            }
            command
                .current_dir(directory.path())
                .arg("--bind")
                .arg(address.to_string())
                .arg("--daemon")
                .arg(daemon.local_addr()?.to_string())
                .arg("--probe-timeout-ms")
                .arg("10")
                .arg("--daemon-config")
                .arg(config)
                .arg("--state-dir")
                .arg(state)
                .arg("--daemon-executable")
                .arg(directory.path().join("never-a-runtime"))
                .arg("--assignment-mcp-executable")
                .arg(env!("CARGO_BIN_EXE_worldstream-assignment-mcp"))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            drop(listener);
            let child = command.spawn()?;
            let mut instance = Self {
                child,
                address,
                control,
                _daemon: daemon,
                _directory: directory,
            };
            let deadline = Instant::now() + Duration::from_mins(3);
            loop {
                if instance.child.try_wait()?.is_some() {
                    return Err("disposable Supervisor exited before becoming available".into());
                }
                if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
                    return Ok(instance);
                }
                if Instant::now() >= deadline {
                    return Err("disposable Supervisor startup deadline exceeded".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
        }

        fn request(
            &self,
            method: &str,
            path: &str,
            headers: &[(&str, &str)],
            body: &str,
        ) -> Result<HttpResponse, Box<dyn std::error::Error>> {
            let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(1))?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            write!(
                stream,
                "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
                self.address,
                body.len()
            )?;
            for (name, value) in headers {
                write!(stream, "{name}: {value}\r\n")?;
            }
            write!(stream, "\r\n{body}")?;
            read_http_response(&mut stream, method)
        }

        fn assert_redacted_output(&mut self, canaries: &[&str]) -> TestResult {
            if self.child.try_wait()?.is_none() {
                self.child.kill()?;
            }
            self.child.wait()?;
            if let Some(stdout) = self.child.stdout.take() {
                assert_redacted_stream(stdout, canaries)?;
            }
            if let Some(stderr) = self.child.stderr.take() {
                assert_redacted_stream(stderr, canaries)?;
            }
            Ok(())
        }
    }

    fn assert_redacted_stream(output: impl std::io::Read, canaries: &[&str]) -> TestResult {
        let mut captured = Vec::new();
        output.take(65_537).read_to_end(&mut captured)?;
        assert!(
            captured.len() <= 65_536,
            "bounded child output limit exceeded"
        );
        for canary in canaries {
            assert!(
                !captured
                    .windows(canary.len())
                    .any(|window| window == canary.as_bytes()),
                "child output must not expose control credentials"
            );
        }
        Ok(())
    }

    impl Drop for Supervisor {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct HttpResponse {
        status: u16,
        head: String,
        body: String,
    }

    fn read_http_response(
        reader: &mut impl std::io::Read,
        method: &str,
    ) -> Result<HttpResponse, Box<dyn std::error::Error>> {
        use std::io::BufRead as _;
        // Admission can close without draining a rejected request body. The OS
        // may reset afterward: a complete HTTP response needs no clean TCP EOF.
        // This fixture accepts bounded fixed-length responses, not streaming HTTP.
        let mut reader = std::io::BufReader::new(reader.take(65_536));
        let mut head = String::new();
        loop {
            let previous = head.len();
            if reader.read_line(&mut head)? == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
            if head.len() > 16_384 {
                return Err("test response headers exceed bound".into());
            }
            if &head[previous..] == "\r\n" {
                break;
            }
        }
        let header_bytes = head.len();
        let head = head
            .strip_suffix("\r\n\r\n")
            .ok_or("invalid test HTTP headers")?;
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or("missing test HTTP status")?
            .parse::<u16>()?;
        let mut content_length = None;
        for line in head.lines().skip(1) {
            let (name, value) = line.split_once(':').ok_or("invalid test HTTP header")?;
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err("streaming test responses are unsupported".into());
            }
            if name.eq_ignore_ascii_case("content-length") {
                if content_length.is_some() {
                    return Err("duplicate test response length".into());
                }
                content_length = Some(value.trim().parse::<usize>()?);
            }
        }
        let length = if method == "HEAD" || matches!(status, 204 | 304) {
            0
        } else {
            content_length.ok_or("missing test response length")?
        };
        if length > 65_536 - header_bytes {
            return Err("test response body exceeds bound".into());
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        let body = String::from_utf8(body).map_err(|_| "invalid test response encoding")?;
        Ok(HttpResponse {
            status,
            head: head.to_owned(),
            body,
        })
    }

    struct ResetAfterBytes(std::io::Cursor<&'static [u8]>);

    impl std::io::Read for ResetAfterBytes {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = self.0.read(output)?;
            if count == 0 && !output.is_empty() {
                return Err(std::io::ErrorKind::ConnectionReset.into());
            }
            Ok(count)
        }
    }

    #[test]
    fn complete_framed_response_does_not_require_clean_socket_eof() -> TestResult {
        let mut reader = ResetAfterBytes(std::io::Cursor::new(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\n{}",
        ));
        let response = read_http_response(&mut reader, "POST")?;
        assert_eq!(response.status, 401);
        assert_eq!(response.body, "{}");
        Ok(())
    }

    #[test]
    fn truncated_framed_response_followed_by_reset_remains_an_error() -> TestResult {
        let mut reader = ResetAfterBytes(std::io::Cursor::new(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 5\r\n\r\n{}",
        ));
        let error = read_http_response(&mut reader, "POST")
            .err()
            .ok_or("truncated response must fail")?;
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::ConnectionReset)
        );
        Ok(())
    }

    #[test]
    fn truncated_framed_response_followed_by_eof_remains_an_error() -> TestResult {
        let mut reader =
            std::io::Cursor::new(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 5\r\n\r\n{}");
        let error = read_http_response(&mut reader, "POST")
            .err()
            .ok_or("truncated response must fail")?;
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::UnexpectedEof)
        );
        Ok(())
    }

    #[test]
    fn preview_rejects_non_loopback_and_ephemeral_binds_before_creating_state() -> TestResult {
        for bind in ["0.0.0.0:9420", "[::]:9420", "127.0.0.1:0", "[::1]:0"] {
            let directory = tempfile::tempdir()?;
            let mut command = Command::new(env!("CARGO_BIN_EXE_worldstream-studio-supervisor"));
            for (name, _) in std::env::vars_os() {
                if name.to_string_lossy().starts_with("WORLDSTREAM_") {
                    command.env_remove(name);
                }
            }
            let output = command
                .current_dir(directory.path())
                .args(["--require-operator-control", "--bind", bind])
                .arg("--daemon-config")
                .arg(directory.path().join("worldstream.toml"))
                .arg("--state-dir")
                .arg(directory.path().join("studio"))
                .stdin(Stdio::null())
                .output()?;
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("operator control requires a loopback address and nonzero port")
            );
            assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
        }
        Ok(())
    }

    #[test]
    fn full_process_route_graph_rejects_non_control_authority_and_accepts_current_generation()
    -> TestResult {
        // Omitting the assertion flag never disables hardened preview admission.
        let mut supervisor = Supervisor::start(false)?;
        let authorization = supervisor.control.authorization_header()?;
        let authorization = authorization.to_str()?;
        let query_token = authorization
            .strip_prefix("Bearer ")
            .ok_or("invalid credential scheme")?;
        let cookie = format!("worldstream_control={query_token}");
        for &(path, methods) in OPERATOR_ROUTES {
            let response =
                supervisor.request("TRACE", path, &[("Authorization", authorization)], "")?;
            assert_eq!(response.status, 405, "registered path: {path}");
            let allow = response
                .head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("allow").then_some(value.trim())
                })
                .ok_or("registered path has no Allow header")?;
            for method in methods {
                assert!(
                    allow.split(',').any(|value| value.trim() == *method),
                    "registered method: {method} {path}"
                );
                let invalid_headers = [
                    Vec::new(),
                    vec![("Authorization", "Bearer WRONG_CONTROL_CANARY")],
                    vec![
                        ("Authorization", authorization),
                        ("Authorization", authorization),
                    ],
                    vec![("Cookie", cookie.as_str())],
                    vec![("Origin", "http://127.0.0.1:5174")],
                ];
                for headers in invalid_headers {
                    let response = supervisor.request(method, path, &headers, "{")?;
                    assert_eq!(response.status, 401, "privileged route: {method} {path}");
                    assert!(response.body.contains("control_authentication_required"));
                    assert!(!response.body.contains(query_token));
                    assert!(!response.body.contains("WRONG_CONTROL_CANARY"));
                }
                let with_query = format!("{path}?authorization={query_token}");
                assert_eq!(
                    supervisor.request(method, &with_query, &[], "{")?.status,
                    401,
                    "query is not authority: {method} {path}"
                );
            }
        }
        for (method, path) in [
            ("GET", "/not-a-route"),
            ("TRACE", "/api/v1/daemon/start"),
            ("HEAD", "/api/v1/participant-console/session"),
            ("POST", "/api/v1/participant-console/session"),
            ("GET", "/api/v1/participant-console/session:act"),
            ("GET", "/api/v1/participant-console/session/extra"),
            ("GET", "/api/v1/participant-console/%73ession"),
        ] {
            assert_eq!(
                supervisor.request(method, path, &[], "")?.status,
                401,
                "closed unknown route/method: {method} {path}"
            );
        }
        for (path, status) in [("/not-a-route", 404), ("/api/v1/daemon/status", 200)] {
            assert_eq!(
                supervisor
                    .request("GET", path, &[("Authorization", authorization)], "")?
                    .status,
                status
            );
        }
        supervisor.control.rotate()?;
        let rotated = supervisor.control.authorization_header()?;
        for (credential, status) in [(authorization, 401), (rotated.to_str()?, 200)] {
            assert_eq!(
                supervisor
                    .request(
                        "GET",
                        "/api/v1/daemon/status",
                        &[("Authorization", credential)],
                        ""
                    )?
                    .status,
                status
            );
        }
        supervisor.assert_redacted_output(&[
            query_token,
            rotated
                .to_str()?
                .strip_prefix("Bearer ")
                .ok_or("invalid credential scheme")?,
            "WRONG_CONTROL_CANARY",
        ])?;
        Ok(())
    }

    const BROWSER_ROUTES: &[(&str, &str, &str, &str)] = &[
        (
            "POST",
            "/api/v1/participant-console/handoffs:redeem",
            "",
            "participant_handoff_invalid",
        ),
        (
            "GET",
            "/api/v1/participant-console/session",
            "",
            "participant_session_missing",
        ),
        (
            "POST",
            "/api/v1/participant-console/session:observe",
            "{}",
            "participant_session_missing",
        ),
        (
            "POST",
            "/api/v1/participant-console/session:act",
            r#"{"action_id":"01ARZ3NDEKTSV4RRFFQ69G5FAY","based_on_room_seq":7,"offer_id":"7:host_launch:0","schema_digest":"blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","action_type":"host_launch","payload":{}}"#,
            "participant_session_missing",
        ),
        (
            "POST",
            "/api/v1/participant-console/session:replay",
            r#"{"at_room_seq":0}"#,
            "participant_session_missing",
        ),
    ];

    #[test]
    fn participant_routes_keep_membership_origin_and_handoff_admission_separate() -> TestResult {
        let mut supervisor = Supervisor::start(true)?;
        let authorization = supervisor.control.authorization_header()?;
        let authorization = authorization.to_str()?;
        let console_origin = ("Origin", "http://127.0.0.1:5173");
        for &(method, path, body, missing_authority_code) in BROWSER_ROUTES {
            let response = supervisor.request(method, path, &[], body)?;
            assert_eq!(
                response.status, 403,
                "browser Origin still required: {path}"
            );
            for headers in [
                vec![console_origin],
                vec![console_origin, ("Authorization", authorization)],
            ] {
                let response = supervisor.request(method, path, &headers, body)?;
                assert_eq!(
                    response.status, 401,
                    "Membership or one-use handoff required: {path}"
                );
                assert!(
                    response.body.contains(missing_authority_code),
                    "browser authentication owns rejection: {path}"
                );
                assert!(!response.body.contains(authorization));
                assert!(
                    response
                        .head
                        .to_ascii_lowercase()
                        .contains("cache-control: no-store")
                );
            }
            let preflight = [console_origin, ("Access-Control-Request-Method", method)];
            assert_eq!(
                supervisor.request("OPTIONS", path, &preflight, "")?.status,
                204,
                "browser preflight remains available: {path}"
            );
            assert_eq!(
                supervisor
                    .request(
                        "OPTIONS",
                        path,
                        &[
                            ("Origin", "http://127.0.0.1:5174"),
                            ("Access-Control-Request-Method", method)
                        ],
                        ""
                    )?
                    .status,
                403
            );
            assert_eq!(
                supervisor
                    .request(
                        "OPTIONS",
                        path,
                        &[
                            console_origin,
                            ("Access-Control-Request-Method", method),
                            ("Access-Control-Request-Headers", "authorization")
                        ],
                        ""
                    )?
                    .status,
                403,
                "browser cannot request installation Authorization: {path}"
            );
        }
        supervisor.assert_redacted_output(&[
            authorization
                .strip_prefix("Bearer ")
                .ok_or("invalid credential scheme")?,
            "WRONG_CONTROL_CANARY",
        ])?;
        Ok(())
    }
}
