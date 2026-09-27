#![cfg(feature = "managed-local-runtime")]

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use worldstream_agent_swarm::execution::provider::{
    ClaudeAdapter, CodexAdapter, ControlledAdapter, ControlledBehavior, EphemeralProviderFile,
    EphemeralProviderProfile, InvocationRequest, KiroAdapter, NativeProviderProbe, OutputContract,
    ProbeOutput, ProviderAdapter, ProviderCapabilities, ProviderError, ProviderProbe,
};
use worldstream_agent_swarm::execution::{
    ConfigurationResolution, DesiredExecution, EffectIntent, EffectOutcome, ExecutionCommand,
    ExecutionPhase, ExecutionSupervisor, FileExecutionJournal, InstalledProviderQualification,
    InvocationKind, InvocationResolution, InvocationTicket, MemoryExecutionJournal,
    NativeProcessSpawner, OwnedCommand, PreparedInvocation, ProcessError, ProviderCapacity,
    ProviderKind, ProviderQualification, ProviderQualificationBinding, ProviderRegistry,
    QualificationError, QualifiedSelection, ResourcePolicy, RunBudget, SchedulerState,
    SessionSelection,
};

const CONTROLLED: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-controlled-worker");
const GUARD: &str = env!("CARGO_BIN_EXE_worldstream-agent-swarm-process-guard");

#[test]
fn manual_cap_journal_reopens_without_becoming_roster_policy() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().join("legacy-shape");
    let journal = FileExecutionJournal::open(&root)?;
    let mut supervisor = ExecutionSupervisor::open(journal, 0)?;
    supervisor.command(
        ExecutionCommand::RegisterSwarm {
            swarm_id: "legacy-swarm".to_owned(),
            priority: 1,
            budget: RunBudget::default(),
            roster_provider_counts: BTreeMap::new(),
        },
        0,
    )?;
    supervisor.command(
        ExecutionCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 2,
        },
        0,
    )?;
    supervisor.close_cleanly()?;

    let bytes = std::fs::read(root.join("execution.json"))?;
    let encoded = String::from_utf8(bytes)?;
    assert!(encoded.contains("\"provider_caps\""));
    assert!(!encoded.contains("roster_provider_counts"));

    let journal = FileExecutionJournal::open(&root)?;
    let supervisor = ExecutionSupervisor::open(journal, 1)?;
    assert_eq!(
        supervisor.snapshot(1).provider_caps,
        BTreeMap::from([(ProviderKind::Controlled, 2)])
    );
    Ok(())
}

#[derive(Clone)]
struct FixedProbe {
    version: &'static str,
    help: &'static str,
}

impl ProviderProbe for FixedProbe {
    fn run(&self, _executable: &Path, arguments: &[&str]) -> Result<ProbeOutput, ProviderError> {
        let stdout = if arguments == ["--version"] {
            self.version
        } else {
            self.help
        };
        Ok(ProbeOutput {
            success: true,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        })
    }
}

#[test]
fn controlled_provider_runs_with_exact_report_and_isolated_settings() -> Result<(), Box<dyn Error>>
{
    let temporary = tempfile::tempdir()?;
    let working_area = temporary.path().join("work with spaces");
    std::fs::create_dir(&working_area)?;
    let registry = ProviderRegistry::new();
    let native_probe = NativeProviderProbe::default();
    let inspection = registry.inspect(
        ProviderKind::Controlled,
        &native_probe,
        Path::new(CONTROLLED),
    );
    assert_eq!(inspection.blocker, None);
    let capabilities = inspection.capabilities.ok_or("missing capabilities")?;
    let request = invocation_request(working_area, "inv-controlled", "model-a", Some("high"));
    let prepared = registry.prepare(ProviderKind::Controlled, &request, &capabilities)?;
    prepared.ensure_spawnable()?;
    assert!(prepared.environment_remove.contains("OPENAI_API_KEY"));
    assert!(prepared.environment_remove.contains("ANTHROPIC_API_KEY"));
    assert!(prepared.environment_remove.contains("KIRO_API_KEY"));
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    let mut process = spawner.spawn(&prepared)?;
    let exit = wait_for_exit(&mut process, Duration::from_secs(5))?;
    assert!(
        exit.success,
        "stderr={}",
        String::from_utf8_lossy(&exit.stderr)
    );
    let output = registry.decode(&prepared, &exit.stdout)?;
    assert_eq!(
        output.verify(&prepared)?,
        ConfigurationResolution::Verified {
            model: "model-a".to_owned(),
            effort: Some("high".to_owned()),
        }
    );
    assert_eq!(output.text, "authorized current context");
    Ok(())
}

#[test]
fn controlled_substitution_and_missing_report_fail_closed() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let registry = ProviderRegistry::new();
    let inspection = registry.inspect(
        ProviderKind::Controlled,
        &NativeProviderProbe::default(),
        Path::new(CONTROLLED),
    );
    let capabilities = inspection.capabilities.ok_or("missing capabilities")?;
    let request = invocation_request(
        temporary.path().to_path_buf(),
        "inv-mismatch",
        "model-a",
        Some("medium"),
    );
    let adapter = ControlledAdapter;
    let mismatch = adapter.prepare_with_behavior(
        &request,
        &capabilities,
        ControlledBehavior::MismatchModel,
    )?;
    let output =
        br#"{"type":"configuration","model":"substituted-model","effort":"medium","session_id":"s"}
{"type":"result","text":"ignored"}
"#;
    let decoded = adapter.decode(&mismatch, output)?;
    assert!(decoded.verify(&mismatch).is_err());

    let unreported =
        adapter.prepare_with_behavior(&request, &capabilities, ControlledBehavior::Unreported)?;
    assert!(
        adapter
            .decode(
                &unreported,
                br#"{"type":"result","text":"ignored"}
"#
            )
            .is_err()
    );
    Ok(())
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the paired provider contract assertions share one discovered executable fixture"
)]
fn codex_and_claude_build_explicit_commands_but_static_discovery_stays_blocked()
-> Result<(), Box<dyn Error>> {
    let executable = Path::new(CONTROLLED).canonicalize()?;
    let codex_probe = FixedProbe {
        version: "codex-cli 0.149.0",
        help: "--config <key=value> --stdio --strict-config",
    };
    let codex_inspection = CodexAdapter.inspect(&codex_probe, &executable);
    assert!(codex_inspection.blocker.is_some());
    assert!(
        !codex_inspection
            .capabilities
            .ok_or("missing codex capabilities")?
            .reports_effective_configuration
    );
    let mut request = invocation_request(
        executable
            .parent()
            .ok_or("controlled executable parent missing")?
            .to_path_buf(),
        "inv-explicit",
        "gpt-5.3-codex",
        Some("high"),
    );
    request.session = SessionSelection::Fresh { requested_id: None };
    let codex = CodexAdapter.prepare(&request, &qualified(ProviderKind::Codex, &executable)?)?;
    assert_eq!(
        codex.arguments.first().map(String::as_str),
        Some("app-server")
    );
    let codex_request: serde_json::Value = serde_json::from_str(&codex.stdin)?;
    assert_eq!(codex_request["model"], "gpt-5.3-codex");
    assert_eq!(codex_request["effort"], "high");
    assert_eq!(codex.output_contract, OutputContract::CodexAppServer);
    assert!(
        codex
            .arguments
            .iter()
            .any(|argument| argument == "model_reasoning_effort=\"high\"")
    );
    assert!(
        !codex
            .arguments
            .iter()
            .any(|argument| argument == "--ephemeral")
    );
    assert!(codex.environment_remove.contains("OPENAI_API_KEY"));
    let mut unqualified_request = invocation_request(
        executable
            .parent()
            .ok_or("controlled executable parent missing")?
            .to_path_buf(),
        "inv-unqualified",
        "another-model",
        Some("high"),
    );
    unqualified_request.session = SessionSelection::Fresh { requested_id: None };
    assert_eq!(
        CodexAdapter.prepare(
            &unqualified_request,
            &qualified(ProviderKind::Codex, &executable)?
        ),
        Err(ProviderError::InvalidRequest)
    );

    let mut named_tool_request = request.clone();
    named_tool_request.allowed_tools = vec!["Read".to_owned()];
    assert_eq!(
        CodexAdapter.prepare(
            &named_tool_request,
            &qualified(ProviderKind::Codex, &executable)?
        ),
        Err(ProviderError::ToolPolicyUnavailable)
    );
    let claude = ClaudeAdapter.prepare(
        &named_tool_request,
        &qualified(ProviderKind::Claude, &executable)?,
    )?;
    assert!(
        claude
            .arguments
            .windows(2)
            .any(|pair| pair == ["--model", "gpt-5.3-codex"])
    );
    assert!(
        claude
            .arguments
            .windows(2)
            .any(|pair| pair == ["--effort", "high"])
    );
    assert!(
        claude
            .arguments
            .windows(2)
            .any(|pair| pair == ["--setting-sources", ""])
    );
    assert!(!claude.arguments.iter().any(|argument| {
        matches!(
            argument.as_str(),
            "--bare" | "--fallback-model" | "--dangerously-skip-permissions"
        )
    }));
    assert!(claude.environment_remove.contains("ANTHROPIC_API_KEY"));
    assert!(
        claude
            .environment_remove
            .contains("ANTHROPIC_FOUNDRY_API_KEY")
    );
    assert!(claude.environment_remove.contains("AWS_ACCESS_KEY_ID"));
    assert!(claude.environment_remove.contains("ANTHROPIC_MODEL"));
    assert!(
        claude
            .environment_remove
            .contains("ANTHROPIC_DEFAULT_SONNET_MODEL")
    );
    assert!(
        claude
            .environment_remove
            .contains("CLAUDE_CODE_EFFORT_LEVEL")
    );
    assert!(
        claude
            .environment_remove
            .contains("CLAUDE_CODE_DISABLE_THINKING")
    );
    assert!(claude.environment_remove.contains("MAX_THINKING_TOKENS"));
    assert!(
        claude
            .environment_remove
            .contains("CLAUDE_CODE_SUBAGENT_MODEL")
    );
    Ok(())
}

#[test]
fn provider_session_contracts_preserve_exact_conversation_identity() -> Result<(), Box<dyn Error>> {
    let executable = Path::new(CONTROLLED).canonicalize()?;
    let working_area = executable.parent().ok_or("missing parent")?.to_path_buf();

    let mut codex_request = invocation_request(
        working_area.clone(),
        "codex-session",
        "gpt-5.3-codex",
        Some("high"),
    );
    assert_eq!(
        CodexAdapter.prepare(
            &codex_request,
            &qualified(ProviderKind::Codex, &executable)?
        ),
        Err(ProviderError::Unsupported),
        "Codex cannot silently discard a caller-selected fresh thread ID"
    );
    codex_request.session = SessionSelection::Fresh { requested_id: None };
    let codex = CodexAdapter.prepare(
        &codex_request,
        &qualified(ProviderKind::Codex, &executable)?,
    )?;
    let codex_output = CodexAdapter.decode(
        &codex,
        br#"{"schema":"worldstream/codex-app-server-turn@1",
"configuration":{"model":"gpt-5.3-codex","effort":"high","thread_id":"provider-thread","provider_session_id":"session-tree"},
"turn_id":"turn-a","completion":{"threadId":"provider-thread","turn":{"id":"turn-a","status":"completed","error":null}},
"messages":[{"id":"message-a","type":"agentMessage","phase":"final_answer","text":"{}"}]}"#,
    )?;
    assert!(matches!(
        codex_output.verify(&codex),
        Ok(ConfigurationResolution::Verified { .. })
    ));

    let mut claude_request = invocation_request(
        working_area,
        "claude-session",
        "gpt-5.3-codex",
        Some("high"),
    );
    assert_eq!(
        ClaudeAdapter.prepare(
            &claude_request,
            &qualified(ProviderKind::Claude, &executable)?
        ),
        Err(ProviderError::InvalidRequest),
        "Claude's --session-id requires a UUID"
    );
    let session_id = "2d931510-d99f-494a-8c67-87feb05e1594";
    claude_request.session = SessionSelection::Fresh {
        requested_id: Some(session_id.to_owned()),
    };
    let claude = ClaudeAdapter.prepare(
        &claude_request,
        &qualified(ProviderKind::Claude, &executable)?,
    )?;
    let claude_output = ClaudeAdapter.decode(
        &claude,
        format!(
            "{{\"type\":\"system\",\"subtype\":\"init\",\"model\":\"gpt-5.3-codex\",\"effort\":\"high\",\"session_id\":\"{session_id}\"}}\n{{\"type\":\"result\",\"result\":\"{{}}\",\"session_id\":\"{session_id}\"}}\n"
        )
        .as_bytes(),
    )?;
    assert!(matches!(
        claude_output.verify(&claude),
        Ok(ConfigurationResolution::Verified { .. })
    ));
    let mut substituted_session = claude_output;
    substituted_session.session_id = Some("70662770-5102-4b42-8a02-17234a9e1a00".to_owned());
    assert_eq!(
        substituted_session.verify(&claude),
        Err(worldstream_agent_swarm::execution::ProviderBlocker::ConfigurationMismatch)
    );
    Ok(())
}

#[test]
fn kiro_discovers_only_documented_headless_controls_and_stays_model_blocked()
-> Result<(), Box<dyn Error>> {
    let executable = Path::new(CONTROLLED).canonicalize()?;
    let probe = FixedProbe {
        version: "kiro-cli 3.0.0",
        help: "chat --no-interactive --agent-engine <v1|v2|v3> --output-format stream-json --effort <LEVEL> --resume-id <ID> --trust-tools <TOOLS>",
    };
    let inspection = KiroAdapter.inspect(&probe, &executable);
    assert_eq!(
        inspection.blocker,
        Some(worldstream_agent_swarm::execution::ProviderBlocker::ModelControlUnavailable)
    );
    let capabilities = inspection.capabilities.ok_or("missing Kiro capabilities")?;
    assert!(!capabilities.explicit_model);
    assert!(capabilities.explicit_effort);
    assert!(capabilities.session_reuse);
    let working_area = executable.parent().ok_or("missing parent")?.to_path_buf();
    let request = invocation_request(working_area.clone(), "kiro", "model-a", Some("high"));
    assert_eq!(
        KiroAdapter.prepare(&request, &capabilities),
        Err(ProviderError::Unsupported)
    );
    assert_eq!(
        KiroAdapter.decode(
            &PreparedInvocation {
                provider: ProviderKind::Kiro,
                invocation_id: "kiro".to_owned(),
                member_id: "member".to_owned(),
                configuration_revision: 1,
                program: executable,
                executable_digest: format!("blake3:{}", "0".repeat(64)),
                qualification: None,
                arguments: Vec::new(),
                working_area,
                environment_remove: BTreeSet::new(),
                environment_set: BTreeMap::new(),
                ephemeral_profile: None,
                stdin: String::new(),
                requested_model: "model-a".to_owned(),
                requested_effort: Some("high".to_owned()),
                requested_session: SessionSelection::Fresh { requested_id: None },
                resolution: ConfigurationResolution::Unsupported,
                output_contract: OutputContract::KiroUnsupported,
            },
            b"{}",
        ),
        Err(ProviderError::Unsupported)
    );
    Ok(())
}

#[test]
fn legacy_codex_qualification_cannot_authorize_new_app_server_transport()
-> Result<(), Box<dyn Error>> {
    let executable = Path::new(CONTROLLED).canonicalize()?;
    let probe = FixedProbe {
        version: "codex-cli 0.149.0",
        help: "--model <MODEL> model_reasoning_effort resume Resume a previous",
    };
    let inspection = CodexAdapter.inspect(&probe, &executable);
    let capabilities = inspection
        .capabilities
        .as_ref()
        .ok_or("missing inspected capabilities")?;
    let qualification = ProviderQualification {
        schema: "worldstream/agent-swarm-provider-qualification/v2".to_owned(),
        provider: ProviderKind::Codex,
        executable_digest: capabilities.executable_digest.clone(),
        version: capabilities.version.clone(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        evidence_sha256: "a".repeat(64),
        evidence_path: "evidence.json".to_owned(),
        receipt_manifest_sha256: "a".repeat(64),
        subscription_login: true,
        explicit_model: true,
        explicit_effort: true,
        session_reuse: true,
        reports_effective_configuration: true,
        delegation_contained: true,
        native_cancellation_qualified: true,
        resource_confinement_qualified: true,
        selections: vec![QualifiedSelection {
            model: "gpt-qualified".to_owned(),
            effort: Some("high".to_owned()),
        }],
    };
    let temporary = tempfile::tempdir()?;
    let record = temporary
        .path()
        .join("protected qualifications")
        .join("codex-qualification.json");
    qualification.store(&record)?;
    let loaded = ProviderQualification::load(&record)?;

    assert_eq!(
        loaded.apply(inspection.clone(), "gpt-qualified", Some("high")),
        Err(QualificationError::StaticInspectionBlocked),
        "old CLI identity and boolean proofs do not qualify a changed transport"
    );
    assert_eq!(
        loaded.apply(inspection, "gpt-qualified", Some("medium")),
        Err(QualificationError::SelectionUnqualified)
    );
    Ok(())
}

#[test]
fn external_provider_qualification_requires_explicit_effort_evidence() -> Result<(), Box<dyn Error>>
{
    let executable = Path::new(CONTROLLED).canonicalize()?;
    let probe = FixedProbe {
        version: "codex-cli 0.149.0",
        help: "--model <MODEL> model_reasoning_effort resume Resume a previous",
    };
    let inspection = CodexAdapter.inspect(&probe, &executable);
    let capabilities = inspection
        .capabilities
        .as_ref()
        .ok_or("missing inspected capabilities")?;
    let mut qualification = ProviderQualification {
        schema: "worldstream/agent-swarm-provider-qualification/v2".to_owned(),
        provider: ProviderKind::Codex,
        executable_digest: capabilities.executable_digest.clone(),
        version: capabilities.version.clone(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        evidence_sha256: "c".repeat(64),
        evidence_path: "evidence.json".to_owned(),
        receipt_manifest_sha256: "c".repeat(64),
        subscription_login: true,
        explicit_model: true,
        explicit_effort: false,
        session_reuse: true,
        reports_effective_configuration: true,
        delegation_contained: true,
        native_cancellation_qualified: true,
        resource_confinement_qualified: true,
        selections: vec![QualifiedSelection {
            model: "gpt-qualified".to_owned(),
            effort: None,
        }],
    };
    assert_eq!(
        qualification.apply_all(inspection.clone()),
        Err(QualificationError::InvalidRecord)
    );
    qualification.explicit_effort = true;
    assert_eq!(
        qualification.apply_all(inspection),
        Err(QualificationError::InvalidRecord)
    );
    Ok(())
}

#[test]
fn installed_qualification_rechecks_bound_executable_bytes() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let executable = temporary.path().join(if cfg!(windows) {
        "controlled-copy.exe"
    } else {
        "controlled-copy"
    });
    std::fs::copy(CONTROLLED, &executable)?;
    let registry = ProviderRegistry::new();
    let inspection = registry.inspect(
        ProviderKind::Controlled,
        &NativeProviderProbe::default(),
        &executable,
    );
    let capabilities = inspection
        .capabilities
        .as_ref()
        .ok_or("missing controlled capabilities")?;
    let qualification = ProviderQualification {
        schema: "worldstream/agent-swarm-provider-qualification/v2".to_owned(),
        provider: ProviderKind::Controlled,
        executable_digest: capabilities.executable_digest.clone(),
        version: capabilities.version.clone(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        evidence_sha256: "b".repeat(64),
        evidence_path: "evidence.json".to_owned(),
        receipt_manifest_sha256: "b".repeat(64),
        subscription_login: true,
        explicit_model: true,
        explicit_effort: true,
        session_reuse: true,
        reports_effective_configuration: true,
        delegation_contained: true,
        native_cancellation_qualified: true,
        resource_confinement_qualified: true,
        selections: vec![QualifiedSelection {
            model: "controlled".to_owned(),
            effort: None,
        }],
    };
    let portable_record = temporary.path().join("portable").join("controlled.json");
    qualification.store(&portable_record)?;
    let installed = InstalledProviderQualification::bind(&portable_record, &executable)?;
    let record = temporary.path().join("protected").join("controlled.json");
    installed.store(&record)?;
    assert_eq!(
        InstalledProviderQualification::load(&record)?
            .capabilities()?
            .executable,
        executable.canonicalize()?
    );

    let portable_bytes = std::fs::read(&portable_record)?;
    let mut forged: serde_json::Value = serde_json::from_slice(&portable_bytes)?;
    forged["resource_confinement_qualified"] = serde_json::Value::Bool(false);
    std::fs::write(&portable_record, serde_json::to_vec(&forged)?)?;
    assert!(InstalledProviderQualification::load(&record).is_err());
    std::fs::write(&portable_record, portable_bytes)?;

    OpenOptions::new()
        .append(true)
        .open(&executable)?
        .write_all(b"substituted")?;
    assert!(InstalledProviderQualification::load(&record).is_err());
    Ok(())
}

#[test]
fn process_spawn_rejects_executable_changed_after_prepare() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let executable = temporary.path().join(if cfg!(windows) {
        "controlled-spawn.exe"
    } else {
        "controlled-spawn"
    });
    std::fs::copy(CONTROLLED, &executable)?;
    let registry = ProviderRegistry::new();
    let capabilities = registry
        .inspect(
            ProviderKind::Controlled,
            &NativeProviderProbe::default(),
            &executable,
        )
        .capabilities
        .ok_or("missing controlled capabilities")?;
    let request = invocation_request(
        temporary.path().to_path_buf(),
        "inv-substituted",
        "controlled",
        None,
    );
    let prepared = registry.prepare(ProviderKind::Controlled, &request, &capabilities)?;
    OpenOptions::new()
        .append(true)
        .open(&executable)?
        .write_all(b"substituted")?;

    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    assert!(matches!(
        spawner.spawn(&prepared),
        Err(ProcessError::InvalidRequest)
    ));
    Ok(())
}

#[test]
fn scheduler_enforces_cap_fairness_and_review_priority() {
    let mut scheduler = SchedulerState::default();
    let capacities = BTreeMap::from([(
        ProviderKind::Controlled,
        ProviderCapacity {
            limit: 1,
            active: 0,
        },
    )]);
    let priorities = BTreeMap::from([("a".to_owned(), 1), ("b".to_owned(), 1)]);
    let blocked = BTreeSet::new();
    let queued = vec![
        ticket("a-work", "a", "a1", InvocationKind::Work, 1),
        ticket("a-review", "a", "a2", InvocationKind::ProgressReview, 2),
        ticket("b-work", "b", "b1", InvocationKind::Work, 1),
    ];
    let first = scheduler.schedule(&queued, &[], &capacities, &priorities, &blocked);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].ticket.invocation_id, "a-review");
    let second = scheduler.schedule(&queued, &[], &capacities, &priorities, &blocked);
    assert_eq!(second[0].ticket.swarm_id, "b");
}

#[test]
fn supervisor_drains_budget_and_requires_resume_after_crash() -> Result<(), Box<dyn Error>> {
    let journal = MemoryExecutionJournal::new();
    let mut supervisor = ExecutionSupervisor::open(journal.clone(), 10)?;
    supervisor.command(
        ExecutionCommand::RegisterSwarm {
            swarm_id: "swarm-a".to_owned(),
            priority: 1,
            budget: RunBudget {
                invocation_limit: Some(1),
                active_time_limit_ms: None,
            },
            roster_provider_counts: BTreeMap::new(),
        },
        10,
    )?;
    supervisor.command(
        ExecutionCommand::SetProviderCap {
            provider: ProviderKind::Controlled,
            limit: 1,
        },
        10,
    )?;
    supervisor.command(
        ExecutionCommand::Resume {
            swarm_id: "swarm-a".to_owned(),
        },
        10,
    )?;
    supervisor.command(
        ExecutionCommand::Enqueue(ticket(
            "turn-1",
            "swarm-a",
            "member-a",
            InvocationKind::Work,
            1,
        )),
        10,
    )?;
    assert_eq!(supervisor.schedule(20)?.len(), 1);
    let snapshot = supervisor.snapshot(20);
    assert_eq!(snapshot.swarms[0].phase, ExecutionPhase::Pausing);
    drop(supervisor);

    let mut recovered = ExecutionSupervisor::open(journal, 30)?;
    let snapshot = recovered.snapshot(30);
    assert_eq!(snapshot.swarms[0].phase, ExecutionPhase::RecoveryRequired);
    assert_eq!(
        snapshot.swarms[0].active[0].resolution,
        InvocationResolution::Unknown
    );
    assert!(recovered.schedule(30)?.is_empty());
    recovered.command(
        ExecutionCommand::ResolveInvocation {
            swarm_id: "swarm-a".to_owned(),
            invocation_id: "turn-1".to_owned(),
            resolution: InvocationResolution::Terminated,
        },
        30,
    )?;
    assert_eq!(
        recovered.snapshot(30).swarms[0].phase,
        ExecutionPhase::Paused
    );
    assert_eq!(
        recovered.snapshot(30).swarms[0].desired,
        DesiredExecution::Paused
    );
    Ok(())
}

#[test]
fn ambiguous_effect_blocks_resume_until_reconciled() -> Result<(), Box<dyn Error>> {
    let journal = MemoryExecutionJournal::new();
    let mut supervisor = ExecutionSupervisor::open(journal.clone(), 0)?;
    supervisor.command(
        ExecutionCommand::RegisterSwarm {
            swarm_id: "swarm-effect".to_owned(),
            priority: 1,
            budget: RunBudget::default(),
            roster_provider_counts: BTreeMap::new(),
        },
        0,
    )?;
    supervisor.command(ExecutionCommand::PrepareEffect(effect()), 1)?;
    supervisor.command(
        ExecutionCommand::MarkEffectDispatched {
            operation_id: "operation-1".to_owned(),
        },
        2,
    )?;
    drop(supervisor);
    let mut recovered = ExecutionSupervisor::open(journal, 3)?;
    assert_eq!(
        recovered.snapshot(3).swarms[0].phase,
        ExecutionPhase::BlockedUnknown
    );
    assert!(
        recovered
            .command(
                ExecutionCommand::Resume {
                    swarm_id: "swarm-effect".to_owned(),
                },
                3,
            )
            .is_err()
    );
    recovered.command(
        ExecutionCommand::ObserveEffect {
            operation_id: "operation-1".to_owned(),
            outcome: EffectOutcome::Applied,
        },
        4,
    )?;
    recovered.command(
        ExecutionCommand::AcknowledgeEffect {
            operation_id: "operation-1".to_owned(),
        },
        4,
    )?;
    recovered.command(
        ExecutionCommand::Resume {
            swarm_id: "swarm-effect".to_owned(),
        },
        4,
    )?;
    assert_eq!(
        recovered.snapshot(4).swarms[0].phase,
        ExecutionPhase::Running
    );
    Ok(())
}

#[test]
fn native_stop_reaps_owned_grandchild_and_preserves_unrelated_process() -> Result<(), Box<dyn Error>>
{
    let temporary = tempfile::tempdir()?;
    let owned_lock = temporary.path().join("owned lock");
    let owned_ready = temporary.path().join("owned ready");
    let unrelated_lock = temporary.path().join("unrelated lock");
    let unrelated_ready = temporary.path().join("unrelated ready");
    let mut unrelated = Command::new(CONTROLLED)
        .arg("hold-lock")
        .arg("--lock")
        .arg(&unrelated_lock)
        .arg("--ready")
        .arg(&unrelated_ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_for_file(&unrelated_ready, Duration::from_secs(5))?;

    let prepared = PreparedInvocation {
        provider: ProviderKind::Controlled,
        invocation_id: "tree-1".to_owned(),
        member_id: "member-tree".to_owned(),
        configuration_revision: 1,
        program: Path::new(CONTROLLED).canonicalize()?,
        executable_digest: executable_digest(Path::new(CONTROLLED))?,
        qualification: None,
        arguments: vec![
            "hold-tree".to_owned(),
            "--lock".to_owned(),
            owned_lock.to_string_lossy().into_owned(),
            "--ready".to_owned(),
            owned_ready.to_string_lossy().into_owned(),
        ],
        working_area: temporary.path().to_path_buf(),
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
    };
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    let mut owned = spawner.spawn(&prepared)?;
    wait_for_file(&owned_ready, Duration::from_secs(5))?;
    let _ = owned.stop(Duration::from_secs(3))?;
    wait_for_unlock(&owned_lock, Duration::from_secs(5))?;
    assert!(is_locked(&unrelated_lock)?);
    unrelated.kill()?;
    let _ = unrelated.wait()?;
    wait_for_unlock(&unrelated_lock, Duration::from_secs(5))?;
    Ok(())
}

#[test]
fn owned_command_rejects_executable_bytes_not_bound_by_caller() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?;
    let request = OwnedCommand {
        program: Path::new(CONTROLLED).canonicalize()?,
        expected_executable_digest: format!("blake3:{}", "0".repeat(64)),
        arguments: vec!["--version".to_owned()],
        working_area: temporary.path().to_path_buf(),
        environment: BTreeMap::new(),
        stdin: String::new(),
    };
    match spawner.spawn_command(&request) {
        Err(ProcessError::InvalidRequest) => Ok(()),
        Err(error) => Err(format!("unexpected process error: {error}").into()),
        Ok(mut process) => {
            let _ = process.stop(Duration::from_secs(1));
            Err("unbound checker executable was launched".into())
        }
    }
}

#[test]
fn ephemeral_provider_profiles_are_exclusive_protected_and_crash_cleaned()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let work = temporary.path().join("work");
    let profiles = temporary.path().join("profiles");
    worldstream_runtime::prepare_data_directory(&work)?;
    worldstream_runtime::prepare_data_directory(&profiles)?;

    let stale = profiles.join("stale-owner");
    worldstream_runtime::prepare_data_directory(&stale)?;
    let _stale_file = worldstream_runtime::create_owner_only_file(&stale.join("agent.json"))?;
    let spawner = NativeProcessSpawner::with_default_capture(Path::new(GUARD))?
        .with_profile_root(&profiles)?;
    assert!(
        !stale.exists(),
        "reopen must remove a prior owner's remnants"
    );

    let registry = ProviderRegistry::new();
    let capabilities = registry
        .inspect(
            ProviderKind::Controlled,
            &NativeProviderProbe::default(),
            Path::new(CONTROLLED),
        )
        .capabilities
        .ok_or("controlled capabilities missing")?;
    let request = invocation_request(work, "profile-invocation", "controlled", Some("medium"));
    let mut prepared = ControlledAdapter.prepare_with_behavior(
        &request,
        &capabilities,
        ControlledBehavior::Delayed,
    )?;
    prepared.ephemeral_profile = Some(EphemeralProviderProfile {
        root_environment_variable: "WORLDSTREAM_PROVIDER_PROFILE".to_owned(),
        files: vec![EphemeralProviderFile {
            name: "agent.json".to_owned(),
            contents: "{\"model\":\"controlled\"}".to_owned(),
        }],
    });
    let directory = profiles.join(format!(
        "invocation-{}",
        blake3::hash(prepared.invocation_id.as_bytes()).to_hex()
    ));
    let mut first = spawner.spawn(&prepared)?;
    worldstream_runtime::validate_data_directory(&directory)?;
    worldstream_runtime::validate_owner_only_file(&directory.join("agent.json"))?;
    let duplicate_error = match spawner.spawn(&prepared) {
        Ok(mut duplicate) => {
            let _ = duplicate.stop(Duration::from_secs(3));
            return Err("a live invocation profile was replaced".into());
        }
        Err(error) => error,
    };
    assert_eq!(
        duplicate_error,
        ProcessError::InvalidRequest,
        "a live invocation profile must never be replaced"
    );
    let _ = wait_for_exit(&mut first, Duration::from_secs(5))?;
    assert!(
        !directory.exists(),
        "terminal process cleanup must remove the profile"
    );
    Ok(())
}

fn invocation_request(
    working_area: PathBuf,
    invocation_id: &str,
    model: &str,
    effort: Option<&str>,
) -> InvocationRequest {
    InvocationRequest {
        invocation_id: invocation_id.to_owned(),
        member_id: "member-a".to_owned(),
        configuration_revision: 1,
        model: model.to_owned(),
        effort: effort.map(str::to_owned),
        moving_alias_acknowledged: false,
        working_area,
        resource_policy: ResourcePolicy::ReadOnly,
        allowed_tools: Vec::new(),
        session: SessionSelection::Fresh {
            requested_id: Some("session-a".to_owned()),
        },
        prompt: "authorized current context".to_owned(),
    }
}

fn qualified(
    provider: ProviderKind,
    executable: &Path,
) -> Result<ProviderCapabilities, std::io::Error> {
    let executable_digest = executable_digest(executable)?;
    Ok(ProviderCapabilities {
        provider,
        executable: executable.to_path_buf(),
        executable_digest: executable_digest.clone(),
        version: "qualified-test-version".to_owned(),
        explicit_model: true,
        explicit_effort: true,
        session_reuse: true,
        reports_effective_configuration: true,
        delegation_contained: true,
        native_cancellation_qualified: true,
        resource_confinement_qualified: true,
        qualification: Some(ProviderQualificationBinding {
            provider,
            executable_digest,
            version: "qualified-test-version".to_owned(),
            evidence_sha256: "a".repeat(64),
            operating_system: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            resource_confinement_qualified: true,
            invocation_contract: Some("worldstream/codex-app-server-invocation@1".to_owned()),
            local_codex_profile: None,
            selections: vec![QualifiedSelection {
                model: "gpt-5.3-codex".to_owned(),
                effort: Some("high".to_owned()),
            }],
        }),
    })
}

fn executable_digest(path: &Path) -> Result<String, std::io::Error> {
    Ok(format!(
        "blake3:{}",
        blake3::hash(&std::fs::read(path)?).to_hex()
    ))
}

fn ticket(
    invocation_id: &str,
    swarm_id: &str,
    member_id: &str,
    kind: InvocationKind,
    sequence: u64,
) -> InvocationTicket {
    InvocationTicket {
        invocation_id: invocation_id.to_owned(),
        swarm_id: swarm_id.to_owned(),
        member_id: member_id.to_owned(),
        provider: ProviderKind::Controlled,
        configuration_revision: 1,
        kind,
        due_sequence: sequence,
    }
}

fn effect() -> EffectIntent {
    EffectIntent {
        operation_id: "operation-1".to_owned(),
        swarm_id: "swarm-effect".to_owned(),
        work_id: "work-1".to_owned(),
        work_revision: 1,
        owner_member_id: "member-a".to_owned(),
        invocation_id: "invocation-1".to_owned(),
        attempt_id: "attempt-1".to_owned(),
        configuration_revision: 1,
        execution_epoch: 1,
        target_id: "controlled-target".to_owned(),
        request_digest: format!("blake3:{}", "0".repeat(64)),
    }
}

fn wait_for_exit(
    process: &mut worldstream_agent_swarm::execution::OwnedInvocation,
    timeout: Duration,
) -> Result<worldstream_agent_swarm::execution::ProcessExit, Box<dyn Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(exit) = process.poll()? {
            return Ok(exit);
        }
        if Instant::now() >= deadline {
            return Err("process did not exit".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
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

fn is_locked(path: &Path) -> Result<bool, Box<dyn Error>> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    Ok(file.try_lock().is_err())
}

fn wait_for_unlock(path: &Path, timeout: Duration) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        if !is_locked(path)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("owned lock stayed held".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}
