#[cfg(feature = "managed-local-runtime")]
use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(feature = "managed-local-runtime")]
use clap::Args;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "worldstream-agent-swarm")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run deterministic fixture-only persistence and terminal checks.
    SmokeTest {
        /// Existing directory used for disposable fixture state.
        #[arg(long)]
        state_dir: PathBuf,
    },
    /// Exercise the real terminal lifecycle against fixture-only storage.
    #[command(hide = true)]
    TuiNativeFixture {
        /// Disposable fixture store used only by the native PTY acceptance test.
        #[arg(long)]
        state_file: PathBuf,
        /// End native input after the first real draw to exercise restoration.
        #[arg(long, hide = true)]
        inject_input_termination: bool,
    },
    /// Inspect installed provider CLI contracts without logging in or running a model.
    #[cfg(feature = "managed-local-runtime")]
    Providers {
        /// Additional exact Codex executable to inspect before PATH candidates.
        #[arg(long)]
        codex_path: Option<PathBuf>,
        /// Additional exact Claude Code executable to inspect before PATH candidates.
        #[arg(long)]
        claude_path: Option<PathBuf>,
        /// Additional exact Kiro executable to inspect before PATH candidates.
        #[arg(long)]
        kiro_path: Option<PathBuf>,
        /// Additional exact controlled-worker executable used by native tests.
        #[arg(long)]
        controlled_path: Option<PathBuf>,
    },
    /// Validate one exact provider selection against retained native evidence.
    #[cfg(feature = "managed-local-runtime")]
    QualifyProvider {
        /// Owner-only installed qualification bound to one exact executable.
        #[arg(long)]
        qualification: PathBuf,
        /// Exact requested model identifier.
        #[arg(long)]
        model: String,
        /// Exact requested effort, when applicable.
        #[arg(long)]
        effort: Option<String>,
    },
    /// Install a native provider qualification into owner-only local state.
    #[cfg(feature = "managed-local-runtime")]
    InstallProviderQualification {
        /// Qualification JSON produced from genuine native evidence.
        #[arg(long)]
        input: PathBuf,
        /// Exact installed provider executable to bind to the evidence.
        #[arg(long)]
        executable: PathBuf,
        /// New file below a protected application-state directory.
        #[arg(long)]
        output: PathBuf,
    },
    /// Create one authenticated local Swarm Room from a JSON request.
    #[cfg(feature = "managed-local-runtime")]
    Create {
        #[command(flatten)]
        local: LocalRuntime,
        /// JSON document matching the `CreateSwarm` application contract.
        #[arg(long)]
        request: PathBuf,
    },
    /// List authenticated local Swarms retained by this application.
    #[cfg(feature = "managed-local-runtime")]
    List {
        #[command(flatten)]
        local: LocalRuntime,
    },
    /// Reopen one authenticated local Swarm without creating another Room.
    #[cfg(feature = "managed-local-runtime")]
    Open {
        #[command(flatten)]
        local: LocalRuntime,
        /// Random application Swarm identifier returned by `create`.
        #[arg(long)]
        swarm_id: String,
    },
    /// Observe one exact authenticated Swarm participant.
    #[cfg(feature = "managed-local-runtime")]
    Observe {
        #[command(flatten)]
        local: LocalRuntime,
        /// Random application Swarm identifier returned by `create`.
        #[arg(long)]
        swarm_id: String,
        /// `human` or `worker:<member-key>`.
        #[arg(long)]
        actor: String,
    },
    /// Submit one caller-retained Action against an exact observed Room Head.
    #[cfg(feature = "managed-local-runtime")]
    Act {
        #[command(flatten)]
        local: LocalRuntime,
        /// Random application Swarm identifier returned by `create`.
        #[arg(long)]
        swarm_id: String,
        /// JSON document matching the `ExactSwarmAction` contract.
        #[arg(long)]
        request: PathBuf,
        /// Test-only authority seam for an explicitly controlled fixture member.
        #[arg(long, hide = true)]
        allow_controlled_worker_fixture: bool,
    },
    /// Verify and resolve one Room-referenced local artifact.
    #[cfg(feature = "managed-local-runtime")]
    Artifact {
        #[command(flatten)]
        local: LocalRuntime,
        /// Swarm whose approved working area contains the artifact.
        #[arg(long)]
        swarm_id: String,
        /// Portable path relative to the approved working area.
        #[arg(long)]
        path: String,
        /// Owner-only content-addressed artifact state directory.
        #[arg(long)]
        artifact_state: PathBuf,
        /// Expected `blake3:` digest from the authoritative Room artifact ref.
        #[arg(long)]
        expected_digest: Option<String>,
    },
    /// Prepare, inspect, seal, check, review, and safely write back code changes.
    #[cfg(feature = "managed-local-runtime")]
    CodeChange {
        #[command(flatten)]
        workspace: CodeChangeArguments,
        #[command(subcommand)]
        operation: CodeChangeCommand,
    },
    /// Deterministic checker used only by the packaged native smoke test.
    #[cfg(feature = "managed-local-runtime")]
    #[command(hide = true)]
    CodeChangeCheckFixture {
        /// Candidate-relative file to inspect.
        #[arg(long)]
        path: String,
        /// Exact UTF-8 content required for success.
        #[arg(long)]
        expected: String,
    },
    /// Retain and enqueue one exact worker Action plan.
    #[cfg(feature = "managed-local-runtime")]
    WorkerStage {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
        /// JSON document matching `WorkerActionPlan`.
        #[arg(long)]
        plan: PathBuf,
    },
    /// Run one explicit coordinator scheduling/launch tick.
    #[cfg(feature = "managed-local-runtime")]
    WorkerDispatch {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
    },
    /// Poll owned work, retain evidence, and submit exact Actions.
    #[cfg(feature = "managed-local-runtime")]
    WorkerHarvest {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
    },
    /// Continuously restore retained plans and coordinate eligible work.
    #[cfg(feature = "managed-local-runtime")]
    WorkerRun {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
        /// Perform one bounded restore/dispatch/harvest cycle and return.
        #[arg(long)]
        once: bool,
        /// Delay between continuous coordinator cycles (default: 1000 ms).
        #[arg(long, default_value_t = 1_000)]
        poll_interval_ms: u64,
        /// Opt in to model-directed planning with an immutable bounded JSON policy.
        #[arg(long)]
        autonomy_policy: Option<PathBuf>,
    },
    /// Retry only an exact transport-uncertain retained Action.
    #[cfg(feature = "managed-local-runtime")]
    WorkerRetryAction {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
        #[arg(long)]
        invocation_id: String,
    },
    /// Reconcile an uncertain launch using its byte-identical retained request.
    #[cfg(feature = "managed-local-runtime")]
    WorkerRetryLaunch {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
        #[arg(long)]
        invocation_id: String,
    },
    /// Inspect one retained coordinator intent without changing it.
    #[cfg(feature = "managed-local-runtime")]
    WorkerIntent {
        #[command(flatten)]
        local: LocalRuntime,
        #[command(flatten)]
        coordinator: CoordinatorArguments,
        #[arg(long)]
        invocation_id: String,
    },
    /// Run the authenticated local interactive dashboard.
    #[cfg(feature = "managed-local-runtime")]
    Tui {
        #[command(flatten)]
        local: LocalRuntime,
        /// Optional creation-form prefill used when `n` is pressed.
        #[arg(long)]
        request: Option<PathBuf>,
        /// Deterministic JSON input array; uses an in-memory terminal for CI.
        #[arg(long)]
        script: Option<PathBuf>,
        /// JSON array of exact human Actions staged with `a` and confirmed with Enter.
        #[arg(long)]
        actions: Option<PathBuf>,
        /// JSON array of daemon policies staged with `l` and confirmed with Enter.
        #[arg(long)]
        execution_policies: Option<PathBuf>,
        /// Owner-protected coordinator state to show as local operational data.
        #[arg(long)]
        coordinator_state: Option<PathBuf>,
    },
}

#[cfg(feature = "managed-local-runtime")]
#[derive(Args)]
struct LocalRuntime {
    /// Existing initialized `WorldStream` state directory.
    #[arg(long)]
    state_dir: PathBuf,
    /// Protected managed Controller socket.
    #[arg(long)]
    controller_address: SocketAddr,
    /// Local Runtime participant socket.
    #[arg(long)]
    runtime_address: SocketAddr,
    /// Exact Agent Swarm Activity Pack identifier.
    #[arg(long)]
    pack_id: String,
    /// Exact Agent Swarm Activity Pack explanatory version.
    #[arg(long)]
    pack_version: String,
    /// Exact Agent Swarm Activity Pack semantic revision digest.
    #[arg(long)]
    pack_digest: String,
    /// Per-operation Controller and Runtime timeout.
    #[arg(long, default_value_t = 10_000)]
    timeout_ms: u64,
    /// Owner-only execution daemon state directory. When supplied, new Swarms
    /// are registered stopped and TUI execution controls attach to this daemon.
    #[arg(long)]
    execution_state: Option<PathBuf>,
}

#[cfg(feature = "managed-local-runtime")]
#[derive(Args)]
struct CoordinatorArguments {
    /// Swarm whose authoritative working area bounds coordinator artifacts.
    #[arg(long)]
    swarm_id: String,
    /// Owner-protected coordinator and content-addressed artifact state root.
    #[arg(long)]
    coordinator_state: PathBuf,
    /// Owner-only installed provider qualification. Repeat for each real provider.
    #[arg(long = "provider-qualification")]
    provider_qualifications: Vec<PathBuf>,
    /// Native, expiring read-only Codex trial evidence for one exact working area.
    /// This is local development admission and never release qualification.
    #[arg(long)]
    local_codex_evidence: Option<PathBuf>,
    /// Exact deterministic controlled-worker executable for local/native tests.
    /// This never enables Codex, Claude, or Kiro without qualification.
    #[arg(long)]
    controlled_path: Option<PathBuf>,
}

#[cfg(feature = "managed-local-runtime")]
#[derive(Args)]
struct CodeChangeArguments {
    /// Existing repository or working tree whose paths may be changed.
    #[arg(long)]
    working_area: PathBuf,
    /// Owner-only state for candidates, evidence, and write-back journals.
    #[arg(long)]
    code_change_state: PathBuf,
}

#[cfg(feature = "managed-local-runtime")]
#[derive(Subcommand)]
enum CodeChangeCommand {
    /// Create or reopen an isolated editable candidate from a JSON request.
    Prepare {
        #[arg(long)]
        request: PathBuf,
    },
    /// List durable candidates, sealed revisions, evidence, and write-backs.
    Status,
    /// Seal the candidate's current exact bytes and validation policy.
    Seal {
        #[arg(long)]
        request: PathBuf,
    },
    /// Run one criterion through the owned process guard and retain evidence.
    Check {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        process_guard: PathBuf,
    },
    /// Record an independent review and evaluate the acceptance gate.
    Review {
        #[arg(long)]
        request: PathBuf,
    },
    /// Durably stage an accepted exact revision for write-back.
    Writeback {
        #[arg(long)]
        request: PathBuf,
    },
    /// Reconcile a previously staged write-back operation.
    Reconcile {
        #[arg(long)]
        operation_id: String,
    },
}

#[allow(
    clippy::too_many_lines,
    reason = "the exhaustive clap dispatcher keeps every command outcome on one error path"
)]
fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::SmokeTest { state_dir } => {
            worldstream_agent_swarm::smoke::run(&state_dir).map_err(|error| error.to_string())
        }
        Command::TuiNativeFixture {
            state_file,
            inject_input_termination,
        } => tui_native_fixture(&state_file, inject_input_termination),
        #[cfg(feature = "managed-local-runtime")]
        Command::Providers {
            codex_path,
            claude_path,
            kiro_path,
            controlled_path,
        } => providers(codex_path, claude_path, kiro_path, controlled_path),
        #[cfg(feature = "managed-local-runtime")]
        Command::QualifyProvider {
            qualification,
            model,
            effort,
        } => qualify_provider(&qualification, &model, effort.as_deref()),
        #[cfg(feature = "managed-local-runtime")]
        Command::InstallProviderQualification {
            input,
            executable,
            output,
        } => install_provider_qualification(&input, &executable, &output),
        #[cfg(feature = "managed-local-runtime")]
        Command::Create { local, request } => create(local, &request),
        #[cfg(feature = "managed-local-runtime")]
        Command::List { local } => list(local),
        #[cfg(feature = "managed-local-runtime")]
        Command::Open { local, swarm_id } => open(local, swarm_id),
        #[cfg(feature = "managed-local-runtime")]
        Command::Observe {
            local,
            swarm_id,
            actor,
        } => observe(local, swarm_id, &actor),
        #[cfg(feature = "managed-local-runtime")]
        Command::Act {
            local,
            swarm_id,
            request,
            allow_controlled_worker_fixture,
        } => act(local, swarm_id, &request, allow_controlled_worker_fixture),
        #[cfg(feature = "managed-local-runtime")]
        Command::Artifact {
            local,
            swarm_id,
            path,
            artifact_state,
            expected_digest,
        } => artifact(
            local,
            swarm_id,
            &path,
            &artifact_state,
            expected_digest.as_deref(),
        ),
        #[cfg(feature = "managed-local-runtime")]
        Command::CodeChange {
            workspace,
            operation,
        } => code_change(&workspace, operation),
        #[cfg(feature = "managed-local-runtime")]
        Command::CodeChangeCheckFixture { path, expected } => {
            code_change_check_fixture(&path, &expected)
        }
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerStage {
            local,
            coordinator,
            plan,
        } => worker_stage(local, coordinator, &plan),
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerDispatch { local, coordinator } => worker_dispatch(local, coordinator),
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerHarvest { local, coordinator } => worker_harvest(local, coordinator),
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerRun {
            local,
            coordinator,
            once,
            poll_interval_ms,
            autonomy_policy,
        } => worker_run(
            local,
            coordinator,
            once,
            poll_interval_ms,
            autonomy_policy.as_deref(),
        ),
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerRetryAction {
            local,
            coordinator,
            invocation_id,
        } => worker_retry_action(local, coordinator, &invocation_id),
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerRetryLaunch {
            local,
            coordinator,
            invocation_id,
        } => worker_retry_launch(local, coordinator, &invocation_id),
        #[cfg(feature = "managed-local-runtime")]
        Command::WorkerIntent {
            local,
            coordinator,
            invocation_id,
        } => worker_intent(local, coordinator, &invocation_id),
        #[cfg(feature = "managed-local-runtime")]
        Command::Tui {
            local,
            request,
            script,
            actions,
            execution_policies,
            coordinator_state,
        } => tui(
            local,
            request.as_deref(),
            script.as_deref(),
            actions.as_deref(),
            execution_policies.as_deref(),
            coordinator_state.as_deref(),
        ),
    };
    match result {
        Ok(receipt) => println!("{receipt}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn tui_native_fixture(
    state_file: &std::path::Path,
    inject_input_termination: bool,
) -> Result<String, String> {
    use worldstream_agent_swarm::{
        SwarmApplication,
        fixture::FixtureFileBackend,
        tui::{CrosstermSession, Dashboard, Input, TerminalState, TuiError, TuiSession, run},
    };

    struct InputTerminationSession {
        inner: CrosstermSession,
    }

    impl TuiSession for InputTerminationSession {
        fn enter(&mut self) -> Result<(), TuiError> {
            self.inner.enter()
        }

        fn draw(
            &mut self,
            state: &TerminalState,
            dashboard: Dashboard<'_>,
        ) -> Result<(), TuiError> {
            self.inner.draw(state, dashboard)
        }

        fn next_input(&mut self) -> Result<Input, TuiError> {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "injected native terminal input termination",
            )
            .into())
        }

        fn restore(&mut self) -> Result<(), TuiError> {
            self.inner.restore()
        }
    }

    let mut application = SwarmApplication::new(FixtureFileBackend::new(state_file.to_path_buf()));
    let session = CrosstermSession::open().map_err(|error| error.to_string())?;
    let receipt = if inject_input_termination {
        let mut session = InputTerminationSession { inner: session };
        run(&mut application, &mut session, None)
    } else {
        let mut session = session;
        run(&mut application, &mut session, None)
    }
    .map_err(|error| error.to_string())?;
    serde_json::to_string(&receipt).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn providers(
    codex_path: Option<PathBuf>,
    claude_path: Option<PathBuf>,
    kiro_path: Option<PathBuf>,
    controlled_path: Option<PathBuf>,
) -> Result<String, String> {
    use worldstream_agent_swarm::execution::provider::{
        NativeProviderProbe, ProviderBlocker, ProviderInspection, ProviderKind, ProviderRegistry,
        discover_candidates,
    };

    let probe = NativeProviderProbe::default();
    let registry = ProviderRegistry::new();
    let requested = [
        (ProviderKind::Codex, &["codex"][..], codex_path),
        (ProviderKind::Claude, &["claude"][..], claude_path),
        (ProviderKind::Kiro, &["kiro-cli"][..], kiro_path),
        (
            ProviderKind::Controlled,
            &["worldstream-agent-swarm-controlled-worker"][..],
            controlled_path,
        ),
    ];
    let mut inspections = Vec::new();
    for (provider, names, extra) in requested {
        let extras = extra.into_iter().collect::<Vec<_>>();
        let candidates = discover_candidates(names, &extras);
        if candidates.is_empty() {
            inspections.push(ProviderInspection::unavailable(
                provider,
                extras
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| PathBuf::from(names[0])),
                ProviderBlocker::ExecutableUnavailable,
            ));
            continue;
        }
        inspections.extend(
            candidates
                .iter()
                .map(|candidate| registry.inspect(provider, &probe, candidate)),
        );
    }
    serde_json::to_string_pretty(&inspections).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn qualify_provider(
    qualification: &std::path::Path,
    model: &str,
    effort: Option<&str>,
) -> Result<String, String> {
    use worldstream_agent_swarm::execution::InstalledProviderQualification;
    let qualified = InstalledProviderQualification::load(qualification)
        .and_then(|installed| installed.qualify(model, effort))
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&qualified).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn install_provider_qualification(
    input: &std::path::Path,
    executable: &std::path::Path,
    output: &std::path::Path,
) -> Result<String, String> {
    use worldstream_agent_swarm::execution::InstalledProviderQualification;
    let installed = InstalledProviderQualification::bind(input, executable)
        .map_err(|error| error.to_string())?;
    let capabilities = installed
        .capabilities()
        .map_err(|error| error.to_string())?;
    installed.store(output).map_err(|error| error.to_string())?;
    serde_json::to_string(&serde_json::json!({
        "status": "installed",
        "provider": capabilities.provider,
        "version": capabilities.version,
        "executable": capabilities.executable,
        "executable_digest": capabilities.executable_digest,
        "output": output,
    }))
    .map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn backend(
    local: LocalRuntime,
) -> Result<worldstream_agent_swarm::managed_local::ManagedLocalWorldStreamBackend, String> {
    use std::time::Duration;
    use worldstream_protocol::PackReference;
    worldstream_agent_swarm::managed_local::ManagedLocalWorldStreamBackend::open(
        &local.state_dir,
        local.controller_address,
        local.runtime_address,
        PackReference {
            id: local.pack_id,
            version: local.pack_version,
            digest: local.pack_digest,
        },
        Duration::from_millis(local.timeout_ms),
    )
    .map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn create(local: LocalRuntime, request: &std::path::Path) -> Result<String, String> {
    let execution_state = local.execution_state.clone();
    let timeout_ms = local.timeout_ms;
    let bytes = std::fs::read(request).map_err(|error| error.to_string())?;
    let request = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let mut application = worldstream_agent_swarm::SwarmApplication::new(backend(local)?);
    let view = application
        .create(request)
        .map_err(|error| error.to_string())?;
    let mut execution =
        OptionalExecutionControls::open(execution_state.as_deref(), None, timeout_ms)?;
    worldstream_agent_swarm::tui::ExecutionControls::register(&mut execution, &view)?;
    serde_json::to_string_pretty(&view).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn list(local: LocalRuntime) -> Result<String, String> {
    let application = worldstream_agent_swarm::SwarmApplication::new(backend(local)?);
    let summaries = application.list().map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&summaries).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn open(local: LocalRuntime, swarm_id: String) -> Result<String, String> {
    let swarm_id =
        worldstream_agent_swarm::SwarmId::new(swarm_id).map_err(|error| error.to_string())?;
    let application = worldstream_agent_swarm::SwarmApplication::new(backend(local)?);
    let view = application
        .open(&swarm_id)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&view).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn parse_actor(value: &str) -> Result<worldstream_agent_swarm::SwarmActor, String> {
    if value == "human" {
        return Ok(worldstream_agent_swarm::SwarmActor::HumanCoordinator);
    }
    value
        .strip_prefix("worker:")
        .filter(|member_key| !member_key.is_empty())
        .map(|member_key| worldstream_agent_swarm::SwarmActor::Worker {
            member_key: member_key.to_owned(),
        })
        .ok_or_else(|| "actor must be `human` or `worker:<member-key>`".to_owned())
}

#[cfg(feature = "managed-local-runtime")]
struct OptionalExecutionControls {
    client: Option<worldstream_agent_swarm::execution::ExecutionControlClient>,
    coordinator_state: Option<PathBuf>,
}

#[cfg(feature = "managed-local-runtime")]
fn selected_provider_counts<'a>(
    members: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<std::collections::BTreeMap<worldstream_agent_swarm::execution::ProviderKind, u16>, String>
{
    use worldstream_agent_swarm::execution::ProviderKind;

    let mut counts = std::collections::BTreeMap::new();
    for (member_key, provider_name) in members {
        let provider = ProviderKind::from_roster_name(provider_name).ok_or_else(|| {
            format!(
                "authoritative roster member `{member_key}` has unsupported provider `{provider_name}`"
            )
        })?;
        let count = counts.entry(provider).or_insert(0_u16);
        *count = count
            .checked_add(1)
            .ok_or_else(|| "authoritative roster provider count overflowed".to_owned())?;
    }
    Ok(counts)
}

#[cfg(feature = "managed-local-runtime")]
impl OptionalExecutionControls {
    fn open(
        state: Option<&std::path::Path>,
        coordinator_state: Option<&std::path::Path>,
        timeout_ms: u64,
    ) -> Result<Self, String> {
        let client = state
            .map(|state| {
                worldstream_agent_swarm::execution::ExecutionControlClient::open(
                    state,
                    std::time::Duration::from_millis(timeout_ms),
                )
                .map_err(|error| error.to_string())
            })
            .transpose()?;
        Ok(Self {
            client,
            coordinator_state: coordinator_state.map(std::path::Path::to_path_buf),
        })
    }

    fn request_view(
        &self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
    ) -> Result<Option<worldstream_agent_swarm::tui::ExecutionView>, String> {
        use worldstream_agent_swarm::execution::{
            ControlCommand, ControlFault, ControlResult, DaemonError,
        };
        let Some(client) = &self.client else {
            return Ok(None);
        };
        let result = match client.request(ControlCommand::Status {
            swarm_id: Some(swarm_id.as_str().to_owned()),
        }) {
            Ok(result) => result,
            Err(DaemonError::Control(ControlFault::NotFound)) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let ControlResult::Status(snapshot) = result else {
            return Err("execution daemon returned a non-status result".to_owned());
        };
        let Some(swarm) = snapshot.swarms.into_iter().next() else {
            return Ok(None);
        };
        let coordinator = self
            .coordinator_state
            .as_deref()
            .map(|root| {
                worldstream_agent_swarm::read_coordinator_service_view(root, swarm_id).or_else(
                    |error| match error {
                        worldstream_agent_swarm::CoordinatorServiceError::SwarmMismatch => Ok(None),
                        _ => Err(error),
                    },
                )
            })
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        Ok(Some(worldstream_agent_swarm::tui::ExecutionView {
            desired: desired_label(swarm.desired).to_owned(),
            phase: phase_label(swarm.phase).to_owned(),
            active: swarm.active.len(),
            queued: swarm.queued.len(),
            invocations_started: swarm.invocations_started,
            unknown_effects: swarm.unknown_effects.len(),
            priority: swarm.priority,
            invocation_limit: swarm.budget.invocation_limit,
            active_time_limit_ms: swarm.budget.active_time_limit_ms,
            provider_caps: snapshot
                .provider_caps
                .into_iter()
                .map(|(provider, limit)| (provider.as_str().to_owned(), limit))
                .collect(),
            coordinator,
        }))
    }

    fn mutate(
        &mut self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
        command: worldstream_agent_swarm::execution::ControlCommand,
    ) -> Result<worldstream_agent_swarm::tui::ExecutionView, String> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| "no execution daemon is attached".to_owned())?;
        client.request(command).map_err(|error| error.to_string())?;
        self.request_view(swarm_id)?
            .ok_or_else(|| "execution daemon did not retain the Swarm".to_owned())
    }
}

#[cfg(feature = "managed-local-runtime")]
impl worldstream_agent_swarm::tui::ExecutionControls for OptionalExecutionControls {
    fn register(&mut self, swarm: &worldstream_agent_swarm::SwarmView) -> Result<(), String> {
        use worldstream_agent_swarm::execution::{
            ControlCommand, ControlFault, DaemonError, RunBudget,
        };
        let Some(client) = &self.client else {
            return Ok(());
        };
        let swarm_id = &swarm.swarm_id;
        if self.request_view(swarm_id)?.is_some() {
            return Ok(());
        }
        let roster_provider_counts = selected_provider_counts(
            swarm
                .roster
                .iter()
                .map(|member| (member.member_key.as_str(), member.provider.as_str())),
        )?;
        match client.request(ControlCommand::RegisterSwarm {
            swarm_id: swarm_id.as_str().to_owned(),
            priority: 1,
            budget: RunBudget::default(),
            roster_provider_counts,
        }) {
            Ok(_) | Err(DaemonError::Control(ControlFault::Conflict)) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn status(
        &self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
    ) -> Result<Option<worldstream_agent_swarm::tui::ExecutionView>, String> {
        self.request_view(swarm_id)
    }

    fn pause(
        &mut self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
    ) -> Result<worldstream_agent_swarm::tui::ExecutionView, String> {
        self.mutate(
            swarm_id,
            worldstream_agent_swarm::execution::ControlCommand::Pause {
                swarm_id: swarm_id.as_str().to_owned(),
            },
        )
    }

    fn stop(
        &mut self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
    ) -> Result<worldstream_agent_swarm::tui::ExecutionView, String> {
        self.mutate(
            swarm_id,
            worldstream_agent_swarm::execution::ControlCommand::Stop {
                swarm_id: swarm_id.as_str().to_owned(),
            },
        )
    }

    fn resume(
        &mut self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
    ) -> Result<worldstream_agent_swarm::tui::ExecutionView, String> {
        self.mutate(
            swarm_id,
            worldstream_agent_swarm::execution::ControlCommand::Resume {
                swarm_id: swarm_id.as_str().to_owned(),
            },
        )
    }

    fn apply_policy(
        &mut self,
        swarm_id: &worldstream_agent_swarm::SwarmId,
        policy: &worldstream_agent_swarm::tui::ExecutionPolicy,
    ) -> Result<worldstream_agent_swarm::tui::ExecutionView, String> {
        use worldstream_agent_swarm::{
            execution::{ControlCommand, ProviderKind, RunBudget},
            tui::ExecutionPolicy,
        };
        let command = match policy {
            ExecutionPolicy::SetProviderCap { provider, limit } => {
                let provider = match provider.as_str() {
                    "codex" => ProviderKind::Codex,
                    "claude" => ProviderKind::Claude,
                    "kiro" => ProviderKind::Kiro,
                    "controlled" => ProviderKind::Controlled,
                    _ => return Err("unknown execution provider".to_owned()),
                };
                ControlCommand::SetProviderCap {
                    provider,
                    limit: *limit,
                }
            }
            ExecutionPolicy::SetPriority { priority } => ControlCommand::SetPriority {
                swarm_id: swarm_id.as_str().to_owned(),
                priority: *priority,
            },
            ExecutionPolicy::SetBudget {
                invocation_limit,
                active_time_limit_ms,
            } => ControlCommand::SetBudget {
                swarm_id: swarm_id.as_str().to_owned(),
                budget: RunBudget {
                    invocation_limit: *invocation_limit,
                    active_time_limit_ms: *active_time_limit_ms,
                },
            },
        };
        self.mutate(swarm_id, command)
    }
}

#[cfg(feature = "managed-local-runtime")]
const fn desired_label(
    desired: worldstream_agent_swarm::execution::DesiredExecution,
) -> &'static str {
    use worldstream_agent_swarm::execution::DesiredExecution;
    match desired {
        DesiredExecution::Running => "running",
        DesiredExecution::Paused => "paused",
        DesiredExecution::Stopped => "stopped",
    }
}

#[cfg(feature = "managed-local-runtime")]
const fn phase_label(phase: worldstream_agent_swarm::execution::ExecutionPhase) -> &'static str {
    use worldstream_agent_swarm::execution::ExecutionPhase;
    match phase {
        ExecutionPhase::Running => "running",
        ExecutionPhase::Pausing => "pausing",
        ExecutionPhase::Paused => "paused",
        ExecutionPhase::Stopping => "stopping",
        ExecutionPhase::Stopped => "stopped",
        ExecutionPhase::RecoveryRequired => "recovery_required",
        ExecutionPhase::BlockedUnknown => "blocked_unknown",
    }
}

#[cfg(feature = "managed-local-runtime")]
fn observe(local: LocalRuntime, swarm_id: String, actor: &str) -> Result<String, String> {
    let swarm_id =
        worldstream_agent_swarm::SwarmId::new(swarm_id).map_err(|error| error.to_string())?;
    let application = worldstream_agent_swarm::SwarmApplication::new(backend(local)?);
    let observation = application
        .observe(&swarm_id, &parse_actor(actor)?)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&observation).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn act(
    local: LocalRuntime,
    swarm_id: String,
    request: &std::path::Path,
    allow_controlled_worker_fixture: bool,
) -> Result<String, String> {
    use worldstream_agent_swarm::{ExactSwarmAction, SwarmActor};

    let swarm_id =
        worldstream_agent_swarm::SwarmId::new(swarm_id).map_err(|error| error.to_string())?;
    let request: ExactSwarmAction =
        serde_json::from_slice(&std::fs::read(request).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let mut application = worldstream_agent_swarm::SwarmApplication::new(backend(local)?);
    if let SwarmActor::Worker { member_key } = &request.actor {
        if !allow_controlled_worker_fixture {
            return Err(
                "direct worker Actions are forbidden; provider workers submit through the supervised coordinator"
                    .to_owned(),
            );
        }
        let observation = application
            .observe(&swarm_id, &request.actor)
            .map_err(|error| error.to_string())?;
        validate_controlled_worker_fixture(&observation.activity, member_key)?;
    }
    let receipt = application
        .submit(&swarm_id, &request)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&receipt).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn validate_controlled_worker_fixture(
    activity: &serde_json::Value,
    member_key: &str,
) -> Result<(), String> {
    let roster = activity
        .get("roster")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "authoritative Room activity omitted its roster".to_owned())?;
    let mut matches = roster.iter().filter(|entry| {
        entry.get("member_key").and_then(serde_json::Value::as_str) == Some(member_key)
    });
    let member = matches
        .next()
        .ok_or_else(|| "worker is absent from the authoritative Room roster".to_owned())?;
    if matches.next().is_some() {
        return Err("authoritative Room roster duplicated the worker identity".to_owned());
    }
    if member.get("provider").and_then(serde_json::Value::as_str) != Some("controlled")
        || member
            .get("configuration_state")
            .and_then(serde_json::Value::as_str)
            != Some("fixture_unavailable")
    {
        return Err(
            "direct worker Actions require an authoritative controlled fixture member".to_owned(),
        );
    }
    Ok(())
}

#[cfg(feature = "managed-local-runtime")]
fn artifact(
    local: LocalRuntime,
    swarm_id: String,
    path: &str,
    artifact_state: &std::path::Path,
    expected_digest: Option<&str>,
) -> Result<String, String> {
    use worldstream_agent_swarm::{
        ArtifactPath, ArtifactWorkspace, SwarmActor, SwarmApplication, SwarmId,
        authoritative_artifact_for_path,
    };
    let swarm_id = SwarmId::new(swarm_id).map_err(|error| error.to_string())?;
    let application = SwarmApplication::new(backend(local)?);
    let observation = application
        .observe(&swarm_id, &SwarmActor::HumanCoordinator)
        .map_err(|error| error.to_string())?;
    let artifact_path = ArtifactPath::new(path).map_err(|error| error.to_string())?;
    let workspace = ArtifactWorkspace::open(&observation.swarm.working_area, artifact_state)
        .map_err(|error| error.to_string())?;
    let authoritative = authoritative_artifact_for_path(
        &observation.activity,
        workspace.authorized_root(),
        &artifact_path,
    )
    .map_err(|error| error.to_string())?;
    if expected_digest.is_some_and(|expected| expected != authoritative.digest) {
        return Err(
            "expected digest does not match the authoritative Room artifact ref".to_owned(),
        );
    }
    let reference = workspace
        .capture_authoritative(&artifact_path, &authoritative)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&serde_json::json!({
        "swarm_id": swarm_id,
        "path": workspace.authorized_root().join(artifact_path.as_str()),
        "digest": reference.digest(),
        "byte_length": reference.byte_length(),
        "authoritative_artifact_id": authoritative.artifact_id,
        "authoritative_digest": authoritative.digest,
        "media_type": authoritative.media_type,
    }))
    .map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn code_change(
    arguments: &CodeChangeArguments,
    operation: CodeChangeCommand,
) -> Result<String, String> {
    use worldstream_agent_swarm::{
        CodeChangeService, PrepareCodeChangeRequest, RecordCodeReviewRequest, RunCodeCheckRequest,
        SealCodeChangeRequest, StageCodeWriteBackRequest,
    };

    let service = CodeChangeService::open(&arguments.working_area, &arguments.code_change_state)
        .map_err(|error| error.to_string())?;
    match operation {
        CodeChangeCommand::Prepare { request } => {
            let request: PrepareCodeChangeRequest = read_json_request(&request)?;
            pretty_json(
                &service
                    .prepare(&request)
                    .map_err(|error| error.to_string())?,
            )
        }
        CodeChangeCommand::Status => {
            pretty_json(&service.status().map_err(|error| error.to_string())?)
        }
        CodeChangeCommand::Seal { request } => {
            let request: SealCodeChangeRequest = read_json_request(&request)?;
            pretty_json(&service.seal(&request).map_err(|error| error.to_string())?)
        }
        CodeChangeCommand::Check {
            request,
            process_guard,
        } => {
            let request: RunCodeCheckRequest = read_json_request(&request)?;
            pretty_json(
                &service
                    .run_check(&request, &process_guard)
                    .map_err(|error| error.to_string())?,
            )
        }
        CodeChangeCommand::Review { request } => {
            let request: RecordCodeReviewRequest = read_json_request(&request)?;
            pretty_json(
                &service
                    .record_review(&request)
                    .map_err(|error| error.to_string())?,
            )
        }
        CodeChangeCommand::Writeback { request } => {
            let request: StageCodeWriteBackRequest = read_json_request(&request)?;
            pretty_json(
                &service
                    .stage_write_back(&request)
                    .map_err(|error| error.to_string())?,
            )
        }
        CodeChangeCommand::Reconcile { operation_id } => pretty_json(
            &service
                .reconcile_write_back(&operation_id)
                .map_err(|error| error.to_string())?,
        ),
    }
}

#[cfg(feature = "managed-local-runtime")]
fn read_json_request<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn pretty_json(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn code_change_check_fixture(path: &str, expected: &str) -> Result<String, String> {
    let path =
        worldstream_agent_swarm::ArtifactPath::new(path).map_err(|error| error.to_string())?;
    let bytes = std::fs::read(path.as_str()).map_err(|error| error.to_string())?;
    if bytes != expected.as_bytes() {
        return Err("controlled code-change check did not observe expected bytes".to_owned());
    }
    serde_json::to_string(&serde_json::json!({"status": "passed"}))
        .map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn open_coordinator_service(
    local: LocalRuntime,
    arguments: CoordinatorArguments,
) -> Result<worldstream_agent_swarm::CoordinatorService, String> {
    use std::{collections::BTreeMap, time::Duration};
    use worldstream_agent_swarm::execution::{
        ExecutionControlClient, InstalledProviderQualification, NativeProviderProbe,
        ProviderCapabilities, ProviderKind, ProviderRegistry,
    };
    use worldstream_agent_swarm::{
        ArtifactWorkspace, CoordinatorService, SwarmApplication, SwarmCoordinator, SwarmId,
    };

    let execution_state = local
        .execution_state
        .clone()
        .ok_or_else(|| "worker coordinator commands require --execution-state".to_owned())?;
    let timeout = Duration::from_millis(local.timeout_ms);
    let mut by_provider = BTreeMap::<ProviderKind, ProviderCapabilities>::new();
    if let Some(path) = &arguments.local_codex_evidence {
        let capability = worldstream_agent_swarm::execution::local_codex::load_local_codex(path)?;
        by_provider.insert(ProviderKind::Codex, capability);
    }
    for path in arguments.provider_qualifications {
        let capability = InstalledProviderQualification::load(&path)
            .and_then(|installed| installed.capabilities())
            .map_err(|error| error.to_string())?;
        let provider = capability.provider;
        if by_provider.insert(provider, capability).is_some() {
            return Err(format!(
                "duplicate capability for provider {}",
                provider.as_str()
            ));
        }
    }
    if let Some(path) = arguments.controlled_path {
        let inspection = ProviderRegistry::new().inspect(
            ProviderKind::Controlled,
            &NativeProviderProbe::default(),
            &path,
        );
        if inspection.blocker.is_some() {
            return Err(
                "controlled fixture executable did not satisfy its static contract".to_owned(),
            );
        }
        let capability = inspection
            .capabilities
            .ok_or_else(|| "controlled fixture executable is unavailable".to_owned())?;
        if by_provider
            .insert(ProviderKind::Controlled, capability)
            .is_some()
        {
            return Err("duplicate capability for provider controlled".to_owned());
        }
    }
    if by_provider.is_empty() {
        return Err(
            "at least one --provider-qualification or --controlled-path is required".to_owned(),
        );
    }

    let swarm_id = SwarmId::new(arguments.swarm_id).map_err(|error| error.to_string())?;
    let application = SwarmApplication::new(backend(local)?);
    // Opening first binds artifacts to the canonical authoritative working
    // area; the coordinator independently refreshes participant authority.
    let view = application
        .open(&swarm_id)
        .map_err(|error| error.to_string())?;
    let artifacts = ArtifactWorkspace::open(&view.working_area, &arguments.coordinator_state)
        .map_err(|error| error.to_string())?;
    let execution = ExecutionControlClient::open(&execution_state, timeout)
        .map_err(|error| error.to_string())?;
    let status_execution = ExecutionControlClient::open(&execution_state, timeout)
        .map_err(|error| error.to_string())?;
    let coordinator = SwarmCoordinator::open(application, execution, by_provider, artifacts)
        .map_err(|error| error.to_string())?;
    CoordinatorService::open(
        &arguments.coordinator_state,
        &swarm_id,
        coordinator,
        status_execution,
    )
    .map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_stage(
    local: LocalRuntime,
    arguments: CoordinatorArguments,
    plan: &std::path::Path,
) -> Result<String, String> {
    let plan = serde_json::from_slice(&std::fs::read(plan).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let result = open_coordinator_service(local, arguments)?
        .stage(plan)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_dispatch(local: LocalRuntime, arguments: CoordinatorArguments) -> Result<String, String> {
    let result = open_coordinator_service(local, arguments)?
        .dispatch_once()
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_harvest(local: LocalRuntime, arguments: CoordinatorArguments) -> Result<String, String> {
    let result = open_coordinator_service(local, arguments)?
        .harvest_once()
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_run(
    local: LocalRuntime,
    arguments: CoordinatorArguments,
    once: bool,
    poll_interval_ms: u64,
    autonomy_policy: Option<&std::path::Path>,
) -> Result<String, String> {
    let mut service = open_coordinator_service(local, arguments)?;
    if let Some(path) = autonomy_policy {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        if bytes.len() > 64 * 1024 {
            return Err("autonomy policy exceeds 64 KiB".to_owned());
        }
        let policy = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        service
            .enable_autonomy(policy)
            .map_err(|error| error.to_string())?;
    }
    let result = if once {
        service.run_once()
    } else {
        service.run_continuously(std::time::Duration::from_millis(poll_interval_ms))
    }
    .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_retry_action(
    local: LocalRuntime,
    arguments: CoordinatorArguments,
    invocation_id: &str,
) -> Result<String, String> {
    let result = open_coordinator_service(local, arguments)?
        .retry_uncertain_action(invocation_id)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_retry_launch(
    local: LocalRuntime,
    arguments: CoordinatorArguments,
    invocation_id: &str,
) -> Result<String, String> {
    let result = open_coordinator_service(local, arguments)?
        .retry_uncertain_launch(invocation_id)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn worker_intent(
    local: LocalRuntime,
    arguments: CoordinatorArguments,
    invocation_id: &str,
) -> Result<String, String> {
    let result = open_coordinator_service(local, arguments)?
        .intent(invocation_id)
        .map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())
}

#[cfg(feature = "managed-local-runtime")]
fn tui(
    local: LocalRuntime,
    request: Option<&std::path::Path>,
    script: Option<&std::path::Path>,
    actions: Option<&std::path::Path>,
    execution_policies: Option<&std::path::Path>,
    coordinator_state: Option<&std::path::Path>,
) -> Result<String, String> {
    use worldstream_agent_swarm::tui::{
        CrosstermSession, Input, ScriptedSession, WorkspaceArtifactInspector,
        run_with_plans_execution_and_artifacts,
    };
    let execution_state = local.execution_state.clone();
    let timeout_ms = local.timeout_ms;
    let request = request
        .map(|path| std::fs::read(path).map_err(|error| error.to_string()))
        .transpose()?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .transpose()?;
    let actions = actions
        .map(|path| std::fs::read(path).map_err(|error| error.to_string()))
        .transpose()?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let policies = execution_policies
        .map(|path| std::fs::read(path).map_err(|error| error.to_string()))
        .transpose()?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let mut application = worldstream_agent_swarm::SwarmApplication::new(backend(local)?);
    let mut execution =
        OptionalExecutionControls::open(execution_state.as_deref(), coordinator_state, timeout_ms)?;
    let artifacts = coordinator_state.map(WorkspaceArtifactInspector::new);
    if let Some(path) = script {
        let inputs: Vec<Input> =
            serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        let mut session =
            ScriptedSession::new(inputs, 100, 30).map_err(|error| error.to_string())?;
        let receipt = run_with_plans_execution_and_artifacts(
            &mut application,
            &mut session,
            &mut execution,
            &artifacts,
            request.as_ref(),
            actions,
            policies,
        )
        .map_err(|error| error.to_string())?;
        return serde_json::to_string(&serde_json::json!({
            "status": receipt.status, "created": receipt.created,
            "swarm_id": receipt.swarm_id, "room_id": receipt.room_id,
            "actions_submitted": receipt.actions_submitted,
            "execution_commands": receipt.execution_commands,
            "policies_applied": receipt.policies_applied,
            "draws": session.draws(), "restored": session.restored(),
            "cursor_shown": session.cursor_shown()
        }))
        .map_err(|error| error.to_string());
    }
    let mut session = CrosstermSession::open().map_err(|error| error.to_string())?;
    let receipt = run_with_plans_execution_and_artifacts(
        &mut application,
        &mut session,
        &mut execution,
        &artifacts,
        request.as_ref(),
        actions,
        policies,
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_string(&receipt).map_err(|error| error.to_string())
}

#[cfg(all(test, feature = "managed-local-runtime"))]
mod tests {
    use super::{selected_provider_counts, validate_controlled_worker_fixture};
    use serde_json::json;
    use std::collections::BTreeMap;
    use worldstream_agent_swarm::execution::ProviderKind;

    #[test]
    fn authoritative_roster_counts_each_selected_provider() -> Result<(), String> {
        let counts = selected_provider_counts([
            ("codex-a", "codex"),
            ("codex-b", "codex"),
            ("claude-a", "claude"),
        ])?;
        assert_eq!(
            counts,
            BTreeMap::from([(ProviderKind::Codex, 2), (ProviderKind::Claude, 1)])
        );
        Ok(())
    }

    #[test]
    fn unsupported_roster_provider_is_not_silently_ignored() -> Result<(), String> {
        let Err(error) = selected_provider_counts([("member-a", "ambient-provider")]) else {
            return Err("unknown provider was accepted".to_owned());
        };
        assert!(error.contains("member-a"));
        assert!(error.contains("ambient-provider"));
        Ok(())
    }

    #[test]
    fn direct_worker_fixture_requires_exact_authoritative_controlled_member() -> Result<(), String>
    {
        let activity = json!({
            "roster": [{
                "member_key": "fixture-a",
                "provider": "controlled",
                "configuration_state": "fixture_unavailable"
            }, {
                "member_key": "real-codex",
                "provider": "codex",
                "configuration_state": "resolution_unreported"
            }]
        });
        assert!(validate_controlled_worker_fixture(&activity, "fixture-a").is_ok());
        let Err(error) = validate_controlled_worker_fixture(&activity, "real-codex") else {
            return Err("a production provider used the fixture seam".to_owned());
        };
        assert!(error.contains("controlled fixture"));
        assert!(validate_controlled_worker_fixture(&activity, "missing").is_err());
        Ok(())
    }
}
