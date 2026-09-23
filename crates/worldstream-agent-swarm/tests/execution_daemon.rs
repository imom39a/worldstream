#![cfg(feature = "managed-local-runtime")]

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::OpenOptions,
    io::{BufRead as _, BufReader, Write as _},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use worldstream_agent_swarm::execution::provider::OutputContract;
use worldstream_agent_swarm::execution::{
    ConfigurationResolution, ControlCommand, ControlFault, ControlResult, DaemonError,
    DesiredExecution, EffectIntent, EffectOutcome, ExecutionControlClient, ExecutionDaemon,
    ExecutionPhase, InvocationKind, InvocationResolution, InvocationTicket, JournalError,
    PreparedInvocation, ProviderKind, RunBudget, SessionSelection,
};
use worldstream_runtime::validate_owner_only_file;

const CONTROLLED: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-controlled-worker");
const GUARD: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-process-guard");
const DAEMON: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarmd");

#[test]
fn native_daemon_waits_for_an_authenticated_request_after_accepting_connection()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let mut daemon = Command::new(DAEMON)
        .args(["--state"])
        .arg(&state)
        .args(["--process-guard", GUARD])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Exercise run() through the actual binary: serve_once() makes its listener
    // blocking and therefore cannot reproduce macOS accepted-socket inheritance.
    let response = (|| -> Result<serde_json::Value, Box<dyn Error>> {
        let publication = state.join("execution-control.v1.json");
        wait_for_file(&publication, Duration::from_secs(5))?;
        let endpoint: serde_json::Value = serde_json::from_slice(&std::fs::read(publication)?)?;
        let address: SocketAddr = endpoint["endpoint"]
            .as_str()
            .ok_or("missing endpoint")?
            .parse()?;
        let generation = endpoint["generation"]
            .as_str()
            .ok_or("missing generation")?;
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        thread::sleep(Duration::from_millis(150));
        let mut request = serde_json::to_vec(&serde_json::json!({
            "schema":"worldstream/agent-swarm-execution-control-request@1",
            "generation":generation,
            "command":{"command":"status","swarm_id":null}
        }))?;
        request.push(b'\n');
        stream.write_all(&request)?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        Ok(serde_json::from_str(&reply)?)
    })();
    // Always reap the daemon before propagating an IO failure or test assertion.
    let killed = daemon.kill();
    daemon.wait()?;
    killed?;
    let response = response?;
    assert_eq!(
        response["schema"],
        "worldstream/agent-swarm-execution-control-response@1"
    );
    assert!(response.get("fault").is_none());
    assert_eq!(response["result"]["kind"], "status");
    assert_eq!(response["result"]["value"]["swarms"], serde_json::json!([]));
    Ok(())
}

#[test]
fn legacy_register_payload_remains_valid_and_fail_closed() -> Result<(), Box<dyn Error>> {
    let payload = serde_json::json!({
        "command": "register_swarm",
        "swarm_id": "legacy-swarm",
        "priority": 1,
        "budget": {
            "invocation_limit": null,
            "active_time_limit_ms": null
        }
    });
    let decoded: ControlCommand = serde_json::from_value(payload)?;
    assert_eq!(decoded, register("legacy-swarm"));
    let encoded = serde_json::to_value(decoded)?;
    assert!(encoded.get("roster_provider_counts").is_none());
    Ok(())
}

#[test]
fn daemon_survives_client_detach_and_enforces_single_owner() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");

    let mut daemon = ExecutionDaemon::bind(&state)?;
    assert!(validate_owner_only_file(&state.join("execution-control.v1.json")).is_ok());
    assert!(validate_owner_only_file(&state.join("execution.json")).is_ok());
    assert!(matches!(
        ExecutionDaemon::bind(&state),
        Err(DaemonError::Journal(JournalError::WriterActive))
    ));

    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..4 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });

    let first = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    first.request(register("swarm-a"))?;
    let resumed = first.request(ControlCommand::Resume {
        swarm_id: "swarm-a".to_owned(),
    })?;
    assert!(matches!(resumed, ControlResult::Mutation(_)));
    drop(first);

    // A new client observes the daemon-owned state after the first client/TUI
    // has detached; no client process is the execution lifetime owner.
    let second = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    let status = second.request(ControlCommand::Status {
        swarm_id: Some("swarm-a".to_owned()),
    })?;
    let ControlResult::Status(status) = status else {
        return Err("status returned a mutation".into());
    };
    assert_eq!(status.swarms.len(), 1);
    assert_eq!(status.swarms[0].desired, DesiredExecution::Running);
    assert_eq!(status.swarms[0].phase, ExecutionPhase::Running);
    second.request(ControlCommand::Pause {
        swarm_id: "swarm-a".to_owned(),
    })?;

    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    assert!(!state.join("execution-control.v1.json").exists());
    Ok(())
}

#[test]
fn daemon_reopen_after_unclean_exit_requires_explicit_resume() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");

    let mut daemon = ExecutionDaemon::bind(&state)?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..2 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });
    let client = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    client.request(register("swarm-crash"))?;
    client.request(ControlCommand::Resume {
        swarm_id: "swarm-crash".to_owned(),
    })?;
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon); // Simulates an exit without the supervisor's clean-close marker.

    let mut recovered = ExecutionDaemon::bind(&state)?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        daemon_status_once(&mut recovered)?;
        Ok(recovered)
    });
    let client = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    let ControlResult::Status(status) = client.request(ControlCommand::Status {
        swarm_id: Some("swarm-crash".to_owned()),
    })?
    else {
        return Err("status returned a mutation".into());
    };
    assert_eq!(status.swarms[0].phase, ExecutionPhase::RecoveryRequired);
    let recovered = server.join().map_err(|_| "daemon thread panicked")??;
    drop(recovered);
    Ok(())
}

#[test]
fn daemon_accepts_policy_queue_and_explicit_tick_without_launching_provider()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let mut daemon = ExecutionDaemon::bind(&state)?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..4 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });
    let client = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    client.request(register_with_roster(
        "swarm-queue",
        ProviderKind::Controlled,
        1,
    ))?;
    client.request(ControlCommand::Resume {
        swarm_id: "swarm-queue".to_owned(),
    })?;
    client.request(ControlCommand::Enqueue {
        ticket: InvocationTicket {
            invocation_id: "invocation-1".to_owned(),
            swarm_id: "swarm-queue".to_owned(),
            member_id: "member-1".to_owned(),
            provider: ProviderKind::Controlled,
            configuration_revision: 1,
            kind: InvocationKind::Work,
            due_sequence: 1,
        },
    })?;
    let ControlResult::Tick(tick) = client.request(ControlCommand::Tick)? else {
        return Err("tick returned the wrong result kind".into());
    };
    assert_eq!(tick.admissions.len(), 1);
    assert_eq!(tick.admissions[0].ticket.invocation_id, "invocation-1");
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn control_commands_fail_closed_for_missing_swarms() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let mut daemon = ExecutionDaemon::bind(&state)?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        daemon.serve_once()?;
        Ok(daemon)
    });
    let client = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    assert!(matches!(
        client.request(ControlCommand::Stop {
            swarm_id: "missing".to_owned()
        }),
        Err(DaemonError::Control(
            worldstream_agent_swarm::execution::ControlFault::NotFound
        ))
    ));
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn daemon_effect_journal_blocks_crash_ambiguity_until_explicit_reconciliation()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let mut daemon = ExecutionDaemon::bind(&state)?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..3 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });
    let client = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    client.request(register("swarm-effect"))?;
    client.request(ControlCommand::PrepareEffect {
        intent: effect_intent(),
    })?;
    client.request(ControlCommand::MarkEffectDispatched {
        operation_id: "effect-1".to_owned(),
    })?;
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);

    let mut recovered = ExecutionDaemon::bind(&state)?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..5 {
            recovered.serve_once()?;
        }
        Ok(recovered)
    });
    let client = ExecutionControlClient::open(&state, Duration::from_secs(2))?;
    let ControlResult::Status(blocked) = client.request(ControlCommand::Status {
        swarm_id: Some("swarm-effect".to_owned()),
    })?
    else {
        return Err("status returned the wrong result kind".into());
    };
    assert_eq!(blocked.swarms[0].phase, ExecutionPhase::BlockedUnknown);
    assert_eq!(blocked.swarms[0].unknown_effects, ["effect-1"]);
    client.request(ControlCommand::ObserveEffect {
        operation_id: "effect-1".to_owned(),
        outcome: EffectOutcome::Applied,
    })?;
    client.request(ControlCommand::AcknowledgeEffect {
        operation_id: "effect-1".to_owned(),
    })?;
    client.request(ControlCommand::Resume {
        swarm_id: "swarm-effect".to_owned(),
    })?;
    let ControlResult::Status(running) = client.request(ControlCommand::Status {
        swarm_id: Some("swarm-effect".to_owned()),
    })?
    else {
        return Err("status returned the wrong result kind".into());
    };
    assert_eq!(running.swarms[0].phase, ExecutionPhase::Running);
    assert!(running.swarms[0].unknown_effects.is_empty());
    let recovered = server.join().map_err(|_| "daemon thread panicked")??;
    drop(recovered);
    Ok(())
}

#[test]
fn daemon_owns_delayed_invocation_across_client_detach_and_retains_result()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let working_area = temporary.path().join("work");
    std::fs::create_dir(&working_area)?;
    let ticket = ticket("delayed-1", "swarm-delayed", "member-delayed");
    let prepared = delayed_prepared(&ticket, &working_area)?;
    let mut daemon = ExecutionDaemon::bind_with_guard(&state, Path::new(GUARD))?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..31 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });

    let first = ExecutionControlClient::open(&state, Duration::from_secs(5))?;
    prepare_admission(&first, &ticket)?;
    let mut mismatched = ticket.clone();
    mismatched.due_sequence = 2;
    assert!(matches!(
        first.request(ControlCommand::Launch {
            ticket: mismatched,
            prepared: Box::new(prepared.clone()),
        }),
        Err(DaemonError::Control(ControlFault::Conflict))
    ));
    let ControlResult::Launch(launch) = first.request(ControlCommand::Launch {
        ticket: ticket.clone(),
        prepared: Box::new(prepared.clone()),
    })?
    else {
        return Err("launch returned the wrong result kind".into());
    };
    assert_eq!(launch.invocation_id, ticket.invocation_id);
    assert!(launch.pid.is_some());
    let ControlResult::Launch(reconciled) = first.request(ControlCommand::Launch {
        ticket: ticket.clone(),
        prepared: Box::new(prepared),
    })?
    else {
        return Err("idempotent launch retry returned the wrong result kind".into());
    };
    assert_eq!(reconciled, launch);
    drop(first);

    let second = ExecutionControlClient::open(&state, Duration::from_secs(5))?;
    let mut tick = None;
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(100));
        let ControlResult::Tick(observed) = second.request(ControlCommand::Tick)? else {
            return Err("tick returned the wrong result kind".into());
        };
        tick = Some(observed);
    }
    let tick = tick.ok_or("no daemon tick was observed")?;
    assert_eq!(tick.completions.len(), 1);
    assert_eq!(
        tick.completions[0].resolution,
        InvocationResolution::Completed
    );
    let ControlResult::Completion(result) = second.request(ControlCommand::Collect {
        invocation_id: ticket.invocation_id.clone(),
    })?
    else {
        return Err("collect returned the wrong result kind".into());
    };
    let exit = result.exit.ok_or("completed process had no exit")?;
    assert!(exit.success);
    assert!(String::from_utf8(exit.stdout)?.contains("authorized after detach"));
    second.request(ControlCommand::AcknowledgeCompletion {
        invocation_id: ticket.invocation_id.clone(),
    })?;
    let ControlResult::Status(status) = second.request(ControlCommand::Status {
        swarm_id: Some(ticket.swarm_id.clone()),
    })?
    else {
        return Err("status returned the wrong result kind".into());
    };
    assert!(status.swarms[0].active.is_empty());

    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn daemon_stop_reaps_owned_descendant_tree() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let working_area = temporary.path().join("work");
    std::fs::create_dir(&working_area)?;
    let lock = temporary.path().join("owned lock");
    let ready = temporary.path().join("owned ready");
    let ticket = ticket("tree-1", "swarm-tree", "member-tree");
    let prepared = tree_prepared(&ticket, &working_area, &lock, &ready)?;
    let mut daemon = ExecutionDaemon::bind_with_guard(&state, Path::new(GUARD))?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..7 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });

    let client = ExecutionControlClient::open(&state, Duration::from_secs(10))?;
    prepare_admission(&client, &ticket)?;
    client.request(ControlCommand::Launch {
        ticket: ticket.clone(),
        prepared: Box::new(prepared),
    })?;
    wait_for_file(&ready, Duration::from_secs(5))?;
    let ControlResult::Mutation(stopped) = client.request(ControlCommand::Stop {
        swarm_id: ticket.swarm_id,
    })?
    else {
        return Err("stop returned the wrong result kind".into());
    };
    assert_eq!(stopped.phase, Some(ExecutionPhase::Stopped));
    wait_for_unlock(&lock, Duration::from_secs(5))?;

    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

#[test]
fn targeted_cancel_reaps_only_the_named_owned_descendant_tree() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let state = temporary.path().join("execution state");
    let working_area = temporary.path().join("work");
    std::fs::create_dir(&working_area)?;
    let first_lock = temporary.path().join("first lock");
    let first_ready = temporary.path().join("first ready");
    let second_lock = temporary.path().join("second lock");
    let second_ready = temporary.path().join("second ready");
    let first = ticket("tree-first", "swarm-targeted", "member-first");
    let second = ticket("tree-second", "swarm-targeted", "member-second");
    let first_prepared = tree_prepared(&first, &working_area, &first_lock, &first_ready)?;
    let second_prepared = tree_prepared(&second, &working_area, &second_lock, &second_ready)?;
    let mut daemon = ExecutionDaemon::bind_with_guard(&state, Path::new(GUARD))?;
    let server = thread::spawn(move || -> Result<ExecutionDaemon, DaemonError> {
        for _ in 0..11 {
            daemon.serve_once()?;
        }
        Ok(daemon)
    });

    let client = ExecutionControlClient::open(&state, Duration::from_secs(10))?;
    client.request(register_with_roster(
        "swarm-targeted",
        ProviderKind::Controlled,
        2,
    ))?;
    client.request(ControlCommand::Resume {
        swarm_id: "swarm-targeted".to_owned(),
    })?;
    for admitted in [&first, &second] {
        client.request(ControlCommand::Enqueue {
            ticket: admitted.clone(),
        })?;
    }
    let ControlResult::Tick(tick) = client.request(ControlCommand::Tick)? else {
        return Err("tick returned the wrong result kind".into());
    };
    assert_eq!(tick.admissions.len(), 2);
    client.request(ControlCommand::Launch {
        ticket: first.clone(),
        prepared: Box::new(first_prepared),
    })?;
    client.request(ControlCommand::Launch {
        ticket: second.clone(),
        prepared: Box::new(second_prepared),
    })?;
    wait_for_file(&first_ready, Duration::from_secs(5))?;
    wait_for_file(&second_ready, Duration::from_secs(5))?;

    let ControlResult::Mutation(cancelled) = client.request(ControlCommand::CancelInvocation {
        swarm_id: first.swarm_id.clone(),
        invocation_id: first.invocation_id.clone(),
    })?
    else {
        return Err("cancel returned the wrong result kind".into());
    };
    assert_eq!(cancelled.phase, Some(ExecutionPhase::Running));
    wait_for_unlock(&first_lock, Duration::from_secs(5))?;
    assert!(is_locked(&second_lock)?);

    // A lost control reply can repeat the exact retained cancellation without
    // targeting the remaining Invocation.
    client.request(ControlCommand::CancelInvocation {
        swarm_id: first.swarm_id.clone(),
        invocation_id: first.invocation_id,
    })?;
    let ControlResult::Status(status) = client.request(ControlCommand::Status {
        swarm_id: Some(first.swarm_id.clone()),
    })?
    else {
        return Err("status returned the wrong result kind".into());
    };
    assert_eq!(status.swarms[0].desired, DesiredExecution::Running);
    assert_eq!(status.swarms[0].phase, ExecutionPhase::Running);
    assert_eq!(status.swarms[0].active.len(), 1);
    assert_eq!(
        status.swarms[0].active[0].ticket.invocation_id,
        second.invocation_id
    );

    client.request(ControlCommand::Stop {
        swarm_id: first.swarm_id,
    })?;
    wait_for_unlock(&second_lock, Duration::from_secs(5))?;
    let daemon = server.join().map_err(|_| "daemon thread panicked")??;
    drop(daemon);
    Ok(())
}

fn daemon_status_once(daemon: &mut ExecutionDaemon) -> Result<(), DaemonError> {
    daemon.serve_once()
}

fn register(swarm_id: &str) -> ControlCommand {
    ControlCommand::RegisterSwarm {
        swarm_id: swarm_id.to_owned(),
        priority: 1,
        budget: RunBudget::default(),
        roster_provider_counts: BTreeMap::new(),
    }
}

fn register_with_roster(swarm_id: &str, provider: ProviderKind, count: u16) -> ControlCommand {
    ControlCommand::RegisterSwarm {
        swarm_id: swarm_id.to_owned(),
        priority: 1,
        budget: RunBudget::default(),
        roster_provider_counts: BTreeMap::from([(provider, count)]),
    }
}

fn effect_intent() -> EffectIntent {
    EffectIntent {
        operation_id: "effect-1".to_owned(),
        swarm_id: "swarm-effect".to_owned(),
        work_id: "work-1".to_owned(),
        work_revision: 1,
        owner_member_id: "member-1".to_owned(),
        invocation_id: "invocation-1".to_owned(),
        attempt_id: "attempt-1".to_owned(),
        configuration_revision: 1,
        execution_epoch: 1,
        target_id: "target-1".to_owned(),
        request_digest: format!("blake3:{}", "a".repeat(64)),
    }
}

fn ticket(invocation_id: &str, swarm_id: &str, member_id: &str) -> InvocationTicket {
    InvocationTicket {
        invocation_id: invocation_id.to_owned(),
        swarm_id: swarm_id.to_owned(),
        member_id: member_id.to_owned(),
        provider: ProviderKind::Controlled,
        configuration_revision: 1,
        kind: InvocationKind::Work,
        due_sequence: 1,
    }
}

fn prepare_admission(
    client: &ExecutionControlClient,
    ticket: &InvocationTicket,
) -> Result<(), DaemonError> {
    client.request(register(&ticket.swarm_id))?;
    client.request(ControlCommand::SetProviderCap {
        provider: ProviderKind::Controlled,
        limit: 1,
    })?;
    client.request(ControlCommand::Resume {
        swarm_id: ticket.swarm_id.clone(),
    })?;
    client.request(ControlCommand::Enqueue {
        ticket: ticket.clone(),
    })?;
    let ControlResult::Tick(tick) = client.request(ControlCommand::Tick)? else {
        return Err(DaemonError::InvalidResponse);
    };
    if tick.admissions.len() != 1 || tick.admissions[0].ticket != *ticket {
        return Err(DaemonError::InvalidResponse);
    }
    Ok(())
}

fn delayed_prepared(
    ticket: &InvocationTicket,
    working_area: &Path,
) -> Result<PreparedInvocation, Box<dyn Error>> {
    Ok(PreparedInvocation {
        provider: ticket.provider,
        invocation_id: ticket.invocation_id.clone(),
        member_id: ticket.member_id.clone(),
        configuration_revision: ticket.configuration_revision,
        program: Path::new(CONTROLLED).canonicalize()?,
        executable_digest: controlled_digest()?,
        qualification: None,
        arguments: vec![
            "invoke".to_owned(),
            "--invocation-id".to_owned(),
            ticket.invocation_id.clone(),
            "--model".to_owned(),
            "controlled".to_owned(),
            "--behavior".to_owned(),
            "delayed".to_owned(),
            "--new-session".to_owned(),
        ],
        working_area: working_area.canonicalize()?,
        environment_remove: BTreeSet::new(),
        environment_set: BTreeMap::from([(
            "WORLDSTREAM_CONTROLLED_PROVIDER".to_owned(),
            "1".to_owned(),
        )]),
        ephemeral_profile: None,
        // Exercise Launch framing above the old 64 KiB control limit.
        stdin: format!("authorized after detach {}", "x".repeat(96 * 1024)),
        requested_model: "controlled".to_owned(),
        requested_effort: None,
        requested_session: SessionSelection::Fresh { requested_id: None },
        resolution: ConfigurationResolution::Verified {
            model: "controlled".to_owned(),
            effort: None,
        },
        output_contract: OutputContract::ControlledJsonLines,
    })
}

fn tree_prepared(
    ticket: &InvocationTicket,
    working_area: &Path,
    lock: &Path,
    ready: &Path,
) -> Result<PreparedInvocation, Box<dyn Error>> {
    Ok(PreparedInvocation {
        provider: ticket.provider,
        invocation_id: ticket.invocation_id.clone(),
        member_id: ticket.member_id.clone(),
        configuration_revision: ticket.configuration_revision,
        program: Path::new(CONTROLLED).canonicalize()?,
        executable_digest: controlled_digest()?,
        qualification: None,
        arguments: vec![
            "hold-tree".to_owned(),
            "--lock".to_owned(),
            lock.to_string_lossy().into_owned(),
            "--ready".to_owned(),
            ready.to_string_lossy().into_owned(),
        ],
        working_area: working_area.canonicalize()?,
        environment_remove: BTreeSet::new(),
        environment_set: BTreeMap::new(),
        ephemeral_profile: None,
        stdin: String::new(),
        requested_model: "controlled".to_owned(),
        requested_effort: None,
        requested_session: SessionSelection::Fresh { requested_id: None },
        resolution: ConfigurationResolution::Verified {
            model: "controlled".to_owned(),
            effort: None,
        },
        output_contract: OutputContract::ControlledJsonLines,
    })
}

fn wait_for_file(path: &Path, timeout: Duration) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err("ready file did not appear".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn controlled_digest() -> Result<String, std::io::Error> {
    Ok(format!(
        "blake3:{}",
        blake3::hash(&std::fs::read(CONTROLLED)?).to_hex()
    ))
}

fn is_locked(path: &Path) -> Result<bool, Box<dyn Error>> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    Ok(file.try_lock().is_err())
}

fn wait_for_unlock(path: &Path, timeout: Duration) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        if file.try_lock().is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("owned descendant lock stayed held".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}
