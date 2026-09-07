use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::Parser;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{HouseAgentRevision, ListingRevision};
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_studio_supervisor::{
    HttpDaemonStatusSource,
    activity_packs::HttpDaemonActivityPackSource,
    agent_profiles::AgentProfileStoreV1,
    assignment_mcp::{AssignmentMcpLaunchRegistryV1, assignment_mcp_launch_router},
    attention_inbox::{
        AttentionInboxV1, FileAttentionHistoryV1, LiveAttentionInboxSourceV1,
        attention_inbox_router,
    },
    backups::{
        BackupOperationsV1, BackupStorageProfileV1, HttpDaemonBackupExecutorV1,
        prepare_shared_backup_root,
    },
    client_bindings::ClientBindingStoreV1,
    lifecycle::ConfiguredDaemonLifecycle,
    managed_agent_host::{ManagedAgentHostOperationsV1, managed_agent_host_router},
    managed_agent_host_seats::managed_agent_host_seat_router,
    model_provider_credentials::ModelProviderCredentialRegistryV1,
    participant_handoff::{FixedDaemonParticipantConsoleGatewayV1, ParticipantHandoffBrokerV1},
    room_creation::{HttpDaemonRoomCreatorV1, RoomCreationSupervisorV1},
    room_drafts::{ExactActivityPackDraftValidatorV1, RoomDraftStoreV1},
    rooms::HttpDaemonRoomSource,
    runner_attention::{
        FileRunnerRestartStoreV1, HttpDaemonRunnerAttentionSourceV1, LiveRunnerAttentionSourceV1,
        PersistedAgentSeatAssignmentSourceV1, RunnerAttentionSupervisorV1, runner_attention_router,
    },
    runner_templates::{RunnerSupervisorV1, RunnerTemplateRegistryV1},
    secrets::{FileSecretVaultV1, SecretReferenceV1},
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_setup_templates_and_model_provider_credentials,
    task_setup::{
        CatalogTaskLaunchApplicabilitySourceV1, FileAssignedMembershipSourceV1,
        HttpDaemonTaskRuntimeV1, HttpDaemonTaskSetupProvisionerV1, LiveTaskRunnerReadinessSourceV1,
        TaskSetupSupervisorV1,
    },
    task_templates::{InstalledTaskTemplateDependenciesV1, TaskTemplateStoreV1},
};
use worldstream_studio_supervisor::{
    control_access::ControlAccess,
    control_admission::protect_operator_routes,
    hosted_browser_sessions::{HostedBrowserSessionBrokerV1, hosted_browser_session_router},
    hosted_house_runners::HostedHouseRunnerOperationsV1,
    hosted_launch::{HostedLaunchAccessV1, HostedLaunchOperationsV1, hosted_launch_router},
    hosted_public_streams::{HostedPublicStreamBrokerV1, hosted_public_stream_router},
    hosted_result_source::{HOSTED_RESULT_SOURCE_TIMEOUT, HttpHostedResultSourceV1},
    local_initialization::validate_initialized,
    managed_controller::{
        ControllerLifecycle, managed_controller_router, managed_lifecycle_router,
        managed_pack_facts_router,
    },
    managed_http::AcceptedLocalSocket,
    managed_lifecycle::ManagedLifecycle,
    process_ownership::{ProcessLease, ProcessOwnership, ProcessRole, ProcessTermination},
    process_runtime::{ManagedRuntimeSpec, ProcessRuntimeControl},
    room_setup_operations::{RoomSetupOperationsV1, room_setup_operations_router},
    startup_authority::validate_existing_host_authority,
    task_setup::ClientNeutralReadinessSourceV1,
};

#[derive(Debug, Parser)]
#[command(
    name = "worldstream-studio-supervisor",
    version,
    about = "Local bounded WorldStream Controller"
)]
struct Args {
    /// Internal launch generation selected by the CLI for this installation.
    #[arg(long, hide = true)]
    managed_generation: Option<String>,
    /// Compatibility assertion; operator control is always enforced.
    #[arg(long = "require-operator-control", hide = true)]
    _require_operator_control: bool,

    /// `WorldStream` Controller API listener.
    #[arg(long, default_value = "127.0.0.1:9420")]
    bind: SocketAddr,

    /// Existing worldstreamd operator listener.
    #[arg(long, default_value = "127.0.0.1:9410")]
    daemon: SocketAddr,

    /// Timeout for each bounded daemon status request.
    #[arg(long, default_value_t = 750)]
    probe_timeout_ms: u64,

    /// Fixed worldstreamd executable controlled by this Controller.
    #[arg(long, default_value = "target/debug/worldstreamd")]
    daemon_executable: PathBuf,

    /// Fixed worldstreamd configuration controlled by this Controller.
    #[arg(long, default_value = "config/development.toml")]
    daemon_config: PathBuf,

    /// Time allowed for a graceful daemon stop.
    #[arg(long, default_value_t = 10_000)]
    graceful_stop_timeout_ms: u64,

    /// Owner-only Controller state directory.
    #[arg(long, default_value = ".worldstream/studio")]
    state_dir: PathBuf,

    /// Fixed assignment-bound MCP helper used for managed reference hosts.
    #[arg(long, default_value = "target/debug/worldstream-assignment-mcp")]
    assignment_mcp_executable: PathBuf,

    /// Exact retained Host authority reference used by bounded daemon proxies.
    #[arg(long, value_parser = parse_secret_reference)]
    host_authority_reference: Option<SecretReferenceV1>,

    /// Storage profile configured for the controlled worldstreamd process.
    #[arg(long, default_value = "sqlite-bundled", value_parser = parse_backup_profile)]
    storage_profile: BackupStorageProfileV1,

    /// Exact loopback operator browser origin admitted for handoff creation.
    #[arg(long, default_value = "http://127.0.0.1:5174")]
    studio_origin: String,

    /// Exact loopback Participant Console origin placed in one-use URLs.
    #[arg(long, default_value = "http://127.0.0.1:5173")]
    participant_console_origin: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut lease = args
        .managed_generation
        .as_ref()
        .map(|generation| {
            ProcessOwnership::open(&args.state_dir)?.claim(ProcessRole::Controller, generation)
        })
        .transpose()
        .context("managed Controller ownership could not be claimed")?;
    #[cfg(unix)]
    if lease.is_some() {
        rustix::process::setsid().context("managed Controller could not detach its session")?;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Controller executor initialization failed")?;
    let result = runtime.block_on(run(args, &mut lease));
    if let Some(lease) = &lease {
        lease.stop_proving();
    }
    drop(runtime);
    if let Some(lease) = lease {
        lease
            .finish(if result.is_ok() {
                ProcessTermination::Stopped
            } else {
                ProcessTermination::Failed
            })
            .context("managed Controller terminal publication is uncertain")?;
    }
    result
}

#[allow(clippy::too_many_lines)]
async fn run(args: Args, managed_lease: &mut Option<ProcessLease>) -> Result<()> {
    // There is no runtime flag which disables operator admission.
    let (daemon_effective, control, vault, host_authority_reference) = {
        if !args.bind.ip().is_loopback() || args.bind.port() == 0 {
            anyhow::bail!("operator control requires a loopback address and nonzero port");
        }
        let loader =
            ConfigLoader::from_process(Some(args.daemon_config.clone()), CliOverrides::default())
                .map_err(|_| {
                anyhow::anyhow!("controlled worldstreamd configuration could not be selected")
            })?;
        let effective = validate_initialized(&loader, &args.state_dir)
            .context("explicit local initialization is required before operator startup")?;
        let control = ControlAccess::open(&args.state_dir)
            .context("protected local operator control is unavailable")?;
        let vault = FileSecretVaultV1::open_existing(&args.state_dir.join("secrets"))
            .context("retained protected secret backend is unavailable")?;
        let reference = validate_existing_host_authority(
            &args.state_dir,
            effective.authority.bootstrap_secret.as_ref(),
        )
        .context("retained Host authority is unavailable")?;
        if args
            .host_authority_reference
            .as_ref()
            .is_some_and(|supplied| supplied != &reference)
        {
            anyhow::bail!("configured Host authority does not match the initialized installation");
        }
        (effective, control, vault, reference)
    };

    let backup_root = prepare_shared_backup_root(
        &args.state_dir,
        &daemon_effective.storage.data_dir,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "the Controller and worldstreamd must use one identical canonical backup root: {error:?}"
        )
    })?;
    let daemon_timeout = Duration::from_millis(args.probe_timeout_ms);
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("Controller listener bind failed at {}", args.bind))?;
    let source = HttpDaemonStatusSource::new(args.daemon, daemon_timeout);
    let lifecycle = ConfiguredDaemonLifecycle::new(
        args.daemon_executable.clone(),
        args.daemon_config.clone(),
        Duration::from_millis(args.graceful_stop_timeout_ms),
        source.clone(),
    );
    let managed_transport_ownership = if managed_lease.is_some() {
        Some(ProcessOwnership::open(&args.state_dir)?)
    } else {
        None
    };
    let activity_packs = HttpDaemonActivityPackSource::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let activity_packs = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonActivityPackSource::new_managed(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        activity_packs
    };
    let runner_registry = RunnerTemplateRegistryV1::open_installed(
        &args.state_dir.join("runner-templates/installed"),
    );
    let runner_registry =
        runner_registry.context("Controller Runner Template registry is unavailable")?;
    let agent_profiles =
        AgentProfileStoreV1::open(&args.state_dir.join("agent-profiles"), vault.clone())
            .context("Controller Agent Profile store is unavailable")?;
    let model_provider_credentials = ModelProviderCredentialRegistryV1::open_installed(
        &args.state_dir.join("model-provider-credentials/installed"),
        vault.clone(),
    );
    let model_provider_credentials = model_provider_credentials
        .context("Controller model-provider credential registry is unavailable")?;
    let draft_dependencies = InstalledTaskTemplateDependenciesV1::new(
        ExactActivityPackDraftValidatorV1::new(activity_packs.clone()),
        agent_profiles.clone(),
        runner_registry.clone(),
    );
    let drafts = RoomDraftStoreV1::open(
        &args.state_dir.join("room-drafts"),
        draft_dependencies.clone(),
    )
    .context("Controller protected Room draft store is unavailable")?;
    let task_templates = TaskTemplateStoreV1::open(
        &args.state_dir.join("task-templates"),
        drafts.clone(),
        draft_dependencies,
    )
    .context("Controller protected Task Template store is unavailable")?;
    let rooms = HttpDaemonRoomSource::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let rooms = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonRoomSource::new_managed(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        rooms
    };
    let room_creator = HttpDaemonRoomCreatorV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let room_creator = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonRoomCreatorV1::new_managed(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        room_creator
    };
    let room_creation = RoomCreationSupervisorV1::open(
        &args.state_dir.join("room-creations"),
        drafts.clone(),
        room_creator,
    )
    .context("Controller protected Room creation store is unavailable")?;
    let backup_executor = HttpDaemonBackupExecutorV1::new(
        args.daemon,
        daemon_timeout,
        args.storage_profile,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let backup_executor = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonBackupExecutorV1::new_managed(
            args.daemon,
            daemon_timeout,
            args.storage_profile,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        backup_executor
    };
    let backups = BackupOperationsV1::open(&backup_root, backup_executor).map_err(|error| {
        anyhow::anyhow!("Controller protected backup operation store is unavailable: {error:?}")
    })?;
    let runners = RunnerSupervisorV1::open(
        runner_registry.clone(),
        &args.state_dir.join("runner-templates/runtime"),
        vault.clone(),
        Duration::from_millis(args.graceful_stop_timeout_ms),
    )
    .context("Controller Runner instance state is unavailable")?;
    let task_runtime = HttpDaemonTaskRuntimeV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let task_runtime = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonTaskRuntimeV1::new_managed(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        task_runtime
    };
    let task_setup_provisioner = HttpDaemonTaskSetupProvisionerV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let task_setup_provisioner = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonTaskSetupProvisionerV1::new_managed(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        task_setup_provisioner
    };
    let task_setup_base = TaskSetupSupervisorV1::open(
        &args.state_dir.join("task-setups"),
        room_creation.clone(),
        vault.clone(),
        task_setup_provisioner,
    )
    .context("Controller protected Task setup store is unavailable")?
    .with_agent_profiles(agent_profiles.clone())
    .with_launch_applicability(CatalogTaskLaunchApplicabilitySourceV1::new(
        activity_packs.clone(),
    ));
    // Startup uses retained reviewed policy only; it never imports declarations
    // or promotes deployment trust from a working-directory configuration file.
    let client_policy = ClientBindingStoreV1::installed_policy(
        &args.state_dir.join("client-bindings"),
    )
    .map_err(|_| anyhow::anyhow!("reviewed Activity Client selection policy is unavailable"))?;
    let client_bindings = ClientBindingStoreV1::open_installed(
        &args.state_dir.join("client-bindings"),
        client_policy,
    );
    let client_bindings = client_bindings
        .map_err(|error| anyhow::anyhow!("Activity Client bindings are unavailable: {error}"))?;
    let participant_gateway =
        FixedDaemonParticipantConsoleGatewayV1::new(args.daemon, daemon_timeout);
    let participant_handoff = ParticipantHandoffBrokerV1::new(
        &args.studio_origin,
        &args.participant_console_origin,
        Duration::from_secs(90),
        256,
        task_setup_base.clone(),
        participant_gateway,
        client_bindings.clone(),
    )
    .map_err(|error| anyhow::anyhow!("Participant Console handoff is unavailable: {error:?}"))?;
    let assignment_launch_source = FileAssignedMembershipSourceV1::open(
        &args.state_dir.join("task-setups"),
        agent_profiles.clone(),
        vault.clone(),
    )
    .context("assignment MCP launch source is unavailable")?;
    let assignment_mcp_launches = AssignmentMcpLaunchRegistryV1::open(
        &args.state_dir.join("assignment-mcp-launches"),
        &args.state_dir.join("assignment-mcp-progress"),
        assignment_launch_source,
        args.daemon,
        daemon_timeout,
    )
    .context("assignment MCP launch registry is unavailable")?
    .with_activity_packs(activity_packs.clone());
    let canonical_state_dir = args
        .state_dir
        .canonicalize()
        .context("Controller state directory is unavailable")?;
    let assignment_mcp_executable = args
        .assignment_mcp_executable
        .canonicalize()
        .context("fixed assignment MCP helper is unavailable")?;
    let managed_agent_hosts = ManagedAgentHostOperationsV1::open_production(
        &args.state_dir.join("managed-agent-hosts"),
        &canonical_state_dir,
        &assignment_mcp_executable,
        agent_profiles.clone(),
        runners.clone(),
        assignment_mcp_launches.clone(),
        vault.clone(),
        task_setup_base.clone(),
    )
    .map_err(|error| anyhow::anyhow!("managed Agent Host operations are unavailable: {error:?}"))?;
    let task_setup = task_setup_base.with_launch_readiness(
        ClientNeutralReadinessSourceV1::new(
            task_runtime.clone(),
            participant_handoff.browser_readiness(),
        ),
        LiveTaskRunnerReadinessSourceV1::new(task_runtime.clone(), runners.clone())
            .with_managed_reference_hosts(managed_agent_hosts.clone()),
        task_runtime,
    );
    let runner_attention_assignments = PersistedAgentSeatAssignmentSourceV1::new(
        agent_profiles.clone(),
        assignment_mcp_launches.clone(),
        task_setup.clone(),
    );
    let daemon_runner_attention = HttpDaemonRunnerAttentionSourceV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        Some(host_authority_reference.clone()),
    );
    let daemon_runner_attention = if let Some(ownership) = &managed_transport_ownership {
        HttpDaemonRunnerAttentionSourceV1::new_managed(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            Some(host_authority_reference.clone()),
            ownership.clone(),
        )
    } else {
        daemon_runner_attention
    };
    let runner_attention_source = LiveRunnerAttentionSourceV1::new(
        runner_attention_assignments,
        daemon_runner_attention,
        runners.clone(),
    );
    let runner_attention = RunnerAttentionSupervisorV1::new(
        runner_attention_source,
        runners.clone(),
        FileRunnerRestartStoreV1::open(&args.state_dir.join("runner-attention/restarts"))
            .map_err(|error| anyhow::anyhow!("Runner attention store is unavailable: {error:?}"))?,
    );
    let runner_attention = runner_attention.with_managed_hosts(managed_agent_hosts.clone());
    let managed_lifecycle = if managed_lease.is_some() {
        Some(ManagedLifecycle::open(
            &args.state_dir,
            ProcessRuntimeControl::open(ManagedRuntimeSpec {
                // Capture lexical absolute paths without requiring the Runtime
                // executable to exist merely to inspect or stop the Controller.
                executable: std::path::absolute(&args.daemon_executable)?,
                config: std::path::absolute(&args.daemon_config)?,
                state: canonical_state_dir.clone(),
                working_directory: std::env::current_dir()?,
                endpoint: args.daemon,
                timeout: Duration::from_millis(args.graceful_stop_timeout_ms),
            })?,
            runners.clone(),
            managed_agent_hosts.clone(),
        )?)
    } else {
        None
    };
    let lifecycle = match &managed_lifecycle {
        Some(control) => ControllerLifecycle::Managed(control.clone()),
        None => ControllerLifecycle::Foreground(lifecycle),
    };
    let attention_inbox = AttentionInboxV1::new(
        LiveAttentionInboxSourceV1::new(
            lifecycle.clone(),
            task_setup.clone(),
            room_creation.clone(),
            backups.clone(),
            runner_attention.clone(),
        ),
        FileAttentionHistoryV1::open(&args.state_dir.join("attention-inbox"))
            .map_err(|error| anyhow::anyhow!("attention inbox is unavailable: {error:?}"))?,
    );
    let room_operations = RoomSetupOperationsV1::new(
        room_creation.clone(),
        task_setup.clone(),
        activity_packs.clone(),
        agent_profiles.clone(),
        runner_registry.clone(),
    );
    let hosted_launch = match (
        env::var("WORLDSTREAM_HOSTED_INSTALLATION_ID").ok(),
        env::var("WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY").ok(),
    ) {
        (None, None) => None,
        (Some(installation_id), Some(authority)) => {
            let (listings, house_agents) = reviewed_hosted_artifacts()?;
            let development_house_provider = development_house_provider_address()?;
            let result_source = if let Some(ownership) = &managed_transport_ownership {
                HttpHostedResultSourceV1::new_managed(
                    args.daemon,
                    HOSTED_RESULT_SOURCE_TIMEOUT,
                    vault.clone(),
                    ownership.clone(),
                )
            } else {
                HttpHostedResultSourceV1::new(
                    args.daemon,
                    HOSTED_RESULT_SOURCE_TIMEOUT,
                    vault.clone(),
                )
            }
            .map_err(|_| anyhow::anyhow!("hosted result-source adapter is unavailable"))?;
            let house_runners = if let Some(provider_address) = development_house_provider {
                HostedHouseRunnerOperationsV1::open_development_loopback(
                    &args.state_dir.join("hosted-house-runners"),
                    &canonical_state_dir,
                    &assignment_mcp_executable,
                    &installation_id,
                    listings.clone(),
                    house_agents.clone(),
                    agent_profiles.clone(),
                    runner_registry.clone(),
                    runners.clone(),
                    assignment_mcp_launches.clone(),
                    vault.clone(),
                    task_setup.clone(),
                    managed_agent_hosts.clone(),
                    provider_address,
                )
            } else {
                HostedHouseRunnerOperationsV1::open_production(
                    &args.state_dir.join("hosted-house-runners"),
                    &canonical_state_dir,
                    &assignment_mcp_executable,
                    &installation_id,
                    listings.clone(),
                    house_agents.clone(),
                    agent_profiles.clone(),
                    runner_registry.clone(),
                    runners.clone(),
                    assignment_mcp_launches.clone(),
                    vault.clone(),
                    task_setup.clone(),
                    managed_agent_hosts.clone(),
                )
            }
            .map_err(|_| anyhow::anyhow!("hosted House Runner adapter is unavailable"))?;
            let operations = HostedLaunchOperationsV1::open(
                &args.state_dir.join("hosted-launches"),
                &installation_id,
                listings,
                house_agents,
                room_operations.clone(),
                task_setup.clone(),
            )
            .map_err(|_| anyhow::anyhow!("hosted launch adapter is unavailable"))?
            .with_house_runners(house_runners)
            .with_result_source(result_source);
            let access = HostedLaunchAccessV1::new(&authority)
                .map_err(|_| anyhow::anyhow!("hosted launch authority is invalid"))?;
            let client_origin = env::var("WORLDSTREAM_HOSTED_CLIENT_ORIGIN").map_err(|_| {
                anyhow::anyhow!(
                    "WORLDSTREAM_HOSTED_CLIENT_ORIGIN is required with hosted launch configuration"
                )
            })?;
            let browser_sessions = HostedBrowserSessionBrokerV1::new(
                &installation_id,
                &client_origin,
                Duration::from_mins(1),
                512,
                task_setup.clone(),
                participant_gateway,
                client_bindings.clone(),
            )
            .map_err(|_| anyhow::anyhow!("hosted Browser Activity Sessions are unavailable"))?;
            let public_streams = HostedPublicStreamBrokerV1::open(
                &args.state_dir.join("hosted-public-relays"),
                operations.clone(),
                vault.clone(),
                FixedDaemonParticipantConsoleGatewayV1::new(args.daemon, daemon_timeout),
                &client_origin,
            )
            .map_err(|_| anyhow::anyhow!("hosted Public Projection relay is unavailable"))?;
            let lobby_reconciler = operations.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_millis(500));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let mut last_failure = None;
                loop {
                    interval.tick().await;
                    let operations = lobby_reconciler.clone();
                    match tokio::task::spawn_blocking(move || operations.reconcile_ready_lobbies())
                        .await
                    {
                        Ok(Ok(())) => {
                            if last_failure.take().is_some() {
                                eprintln!("hosted Lobby readiness reconciliation recovered");
                            }
                        }
                        Ok(Err(error)) => {
                            // Runtime startup and planned restarts can temporarily
                            // make an exact readiness check unavailable. Keep each
                            // launch closed, but do not abandon future checks.
                            if last_failure != Some(error) {
                                eprintln!(
                                    "hosted Lobby readiness reconciliation unavailable: {error:?}"
                                );
                                last_failure = Some(error);
                            }
                        }
                        Err(error) => {
                            eprintln!(
                                "hosted Lobby readiness reconciliation task stopped: {error}"
                            );
                            break;
                        }
                    }
                }
            });
            drop(authority);
            Some(
                hosted_launch_router(operations, access.clone())
                    .merge(hosted_browser_session_router(
                        browser_sessions,
                        access.clone(),
                    ))
                    .merge(hosted_public_stream_router(public_streams, access)),
            )
        }
        _ => anyhow::bail!(
            "hosted launch installation and Controller authority must be configured together"
        ),
    };
    let room_launch = worldstream_studio_supervisor::room_launch::room_launch_router(
        room_creation.clone(),
        task_setup.clone(),
    );
    let scoped_credentials =
        worldstream_studio_supervisor::scoped_connections::scoped_credentials_router(
            task_setup.clone(),
            args.daemon,
        );
    let client_handoff =
        worldstream_studio_supervisor::participant_handoff::operator_client_handoff_router(
            participant_handoff.clone(),
        );
    let room_runners = worldstream_studio_supervisor::scoped_runners::room_runner_router(
        worldstream_studio_supervisor::scoped_runners::RoomRunnerControlV1::new(
            task_setup.clone(),
            agent_profiles.clone(),
            runners.clone(),
            args.state_dir.join("runner-templates/installed"),
        )
        .with_managed_hosts(managed_agent_hosts.clone()),
    );
    let router = supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_setup_templates_and_model_provider_credentials(
        source,
        lifecycle,
        vault,
        runners,
        activity_packs,
        rooms,
        drafts,
        backups,
        room_creation,
        task_setup,
        agent_profiles.clone(),
        model_provider_credentials,
        participant_handoff,
        task_templates,
    )
    .merge(assignment_mcp_launch_router(assignment_mcp_launches))
    .merge(managed_agent_host_router(managed_agent_hosts.clone()))
    .merge(managed_agent_host_seat_router(agent_profiles, managed_agent_hosts))
    .merge(runner_attention_router(runner_attention))
    .merge(attention_inbox_router(attention_inbox));
    let mut router = router
        .merge(room_setup_operations_router(room_operations))
        .merge(room_launch)
        .merge(scoped_credentials)
        .merge(client_handoff)
        .merge(room_runners);
    if let Some(hosted_launch) = hosted_launch {
        router = router.merge(hosted_launch);
    }

    // Admission must wrap the complete graph, including all late merges and
    // assignment-MCP aliases. No operator routes may be merged after this point.
    if let Some(lease) = managed_lease.as_mut() {
        let proof = lease
            .publish_endpoint(listener.local_addr()?)
            .context("managed Controller endpoint publication failed")?;
        let (shutdown, mut requested) = tokio::sync::watch::channel(false);
        let lifecycle = managed_lifecycle.context("managed lifecycle is unavailable")?;
        let router = router.merge(managed_lifecycle_router(lifecycle));
        let router = router.merge(managed_pack_facts_router(
            managed_transport_ownership.context("managed transport ownership is unavailable")?,
            daemon_timeout,
        ));
        let router = managed_controller_router(router, control, proof.clone(), shutdown);
        return axum::serve(
            listener,
            router.into_make_service_with_connect_info::<AcceptedLocalSocket>(),
        )
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = requested.changed() => {},
            }
            proof.disable_for_shutdown();
        })
        .await
        .context("managed Controller server failed");
    }
    let router = protect_operator_routes(router, control);
    axum::serve(listener, router)
        .await
        .context("Controller server failed")
}

fn parse_secret_reference(value: &str) -> Result<SecretReferenceV1, &'static str> {
    SecretReferenceV1::parse(value.to_owned())
        .map_err(|_| "reference must be exactly 64 lowercase hexadecimal characters")
}

fn parse_backup_profile(value: &str) -> Result<BackupStorageProfileV1, &'static str> {
    match value {
        "sqlite-bundled" => Ok(BackupStorageProfileV1::SqliteBundled),
        "postgres-primary" => Ok(BackupStorageProfileV1::PostgresPrimary),
        "ephemeral" => Ok(BackupStorageProfileV1::Ephemeral),
        _ => Err("profile must be sqlite-bundled, postgres-primary, or ephemeral"),
    }
}

fn development_house_provider_address() -> Result<Option<SocketAddr>> {
    let Some(value) = env::var("WORLDSTREAM_DEVELOPMENT_HOUSE_OPENROUTER_ADDRESS").ok() else {
        return Ok(None);
    };
    if env::var("WORLDSTREAM_DEPLOYMENT_ENVIRONMENT").as_deref() != Ok("development")
        || env::var("WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER").as_deref()
            != Ok("visible-local-only")
        || env::var("NODE_ENV").as_deref() == Ok("production")
        || env::var("VERCEL_ENV").as_deref() == Ok("production")
    {
        anyhow::bail!("development House provider is forbidden outside explicit development");
    }
    let address = value
        .parse::<SocketAddr>()
        .map_err(|_| anyhow::anyhow!("development House provider address is invalid"))?;
    if !address.ip().is_loopback() || address.port() == 0 {
        anyhow::bail!("development House provider must be a nonzero loopback address");
    }
    Ok(Some(address))
}

fn reviewed_hosted_artifacts() -> Result<(Vec<ListingRevision>, Vec<HouseAgentRevision>)> {
    const LISTINGS: &[&[u8]] = &[
        include_bytes!("../../../config/hosted/listings/agent-heist-0.2.0.json"),
        include_bytes!("../../../config/hosted/listings/agent-heist-0.3.0.json"),
    ];
    const HOUSE_AGENTS: &[&[u8]] = &[
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-1.json"),
        include_bytes!("../../../config/hosted/house-agents/skeptical-auditor-1.json"),
    ];
    let listings = LISTINGS
        .iter()
        .map(|source| {
            let bytes = CanonicalJsonV1::parse(source)?.to_bytes()?;
            ListingRevision::from_canonical_bytes(&bytes)
                .map_err(|_| anyhow::anyhow!("reviewed hosted Listing is invalid"))
        })
        .collect::<Result<Vec<_>>>()?;
    let house_agents = HOUSE_AGENTS
        .iter()
        .map(|source| {
            let bytes = CanonicalJsonV1::parse(source)?.to_bytes()?;
            HouseAgentRevision::from_canonical_bytes(&bytes)
                .map_err(|_| anyhow::anyhow!("reviewed House Agent is invalid"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((listings, house_agents))
}
