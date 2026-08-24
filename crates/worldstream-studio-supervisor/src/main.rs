use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::Parser;
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
    lifecycle::ConfiguredDaemonLifecycle,
    managed_agent_host::{ManagedAgentHostOperationsV1, managed_agent_host_router},
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
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_setup_and_templates,
    task_setup::{
        FileAssignedMembershipSourceV1, HttpDaemonTaskRuntimeV1, HttpDaemonTaskSetupProvisionerV1,
        LiveTaskRunnerReadinessSourceV1, TaskSetupSupervisorV1,
    },
    task_templates::{InstalledTaskTemplateDependenciesV1, TaskTemplateStoreV1},
};

#[derive(Debug, Parser)]
#[command(
    name = "worldstream-studio-supervisor",
    version,
    about = "Local bounded Supervisor for WorldStream Studio"
)]
struct Args {
    /// Studio Supervisor API listener.
    #[arg(long, default_value = "127.0.0.1:9420")]
    bind: SocketAddr,

    /// Existing worldstreamd operator listener.
    #[arg(long, default_value = "127.0.0.1:9410")]
    daemon: SocketAddr,

    /// Timeout for each bounded daemon status request.
    #[arg(long, default_value_t = 750)]
    probe_timeout_ms: u64,

    /// Fixed worldstreamd executable controlled by this Supervisor.
    #[arg(long, default_value = "target/debug/worldstreamd")]
    daemon_executable: PathBuf,

    /// Fixed worldstreamd configuration controlled by this Supervisor.
    #[arg(long, default_value = "config/development.toml")]
    daemon_config: PathBuf,

    /// Time allowed for a graceful daemon stop.
    #[arg(long, default_value_t = 10_000)]
    graceful_stop_timeout_ms: u64,

    /// Owner-only Supervisor state directory.
    #[arg(long, default_value = ".worldstream/studio")]
    state_dir: PathBuf,

    /// Fixed assignment-bound MCP helper used for managed reference hosts.
    #[arg(long, default_value = "target/debug/worldstream-assignment-mcp")]
    assignment_mcp_executable: PathBuf,

    /// Exact retained Host authority reference used by bounded daemon proxies.
    #[arg(long, value_parser = parse_secret_reference)]
    host_authority_reference: Option<SecretReferenceV1>,

    /// Owner-controlled directory of approved Runner Template manifests.
    #[arg(long, default_value = "config/runner-templates")]
    runner_templates_dir: PathBuf,

    /// Storage profile configured for the controlled worldstreamd process.
    #[arg(long, default_value = "sqlite-bundled", value_parser = parse_backup_profile)]
    storage_profile: BackupStorageProfileV1,

    /// Exact loopback Studio browser origin admitted for handoff creation.
    #[arg(long, default_value = "http://127.0.0.1:5173")]
    studio_origin: String,

    /// Exact loopback Participant Console origin placed in one-use URLs.
    #[arg(long, default_value = "http://127.0.0.1:5174")]
    participant_console_origin: String,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    let args = Args::parse();
    let daemon_effective =
        ConfigLoader::from_process(Some(args.daemon_config.clone()), CliOverrides::default())
            .context("controlled worldstreamd configuration could not be selected")?
            .load()
            .context("controlled worldstreamd configuration is invalid")?;
    let backup_root = prepare_shared_backup_root(
        &args.state_dir,
        &daemon_effective.storage.data_dir,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "Studio and worldstreamd must use one identical canonical backup root: {error:?}"
        )
    })?;
    let daemon_timeout = Duration::from_millis(args.probe_timeout_ms);
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("Studio Supervisor listener bind failed at {}", args.bind))?;
    let source = HttpDaemonStatusSource::new(args.daemon, daemon_timeout);
    let lifecycle = ConfiguredDaemonLifecycle::new(
        args.daemon_executable,
        args.daemon_config,
        Duration::from_millis(args.graceful_stop_timeout_ms),
        source.clone(),
    );
    let vault = FileSecretVaultV1::open(&args.state_dir.join("secrets"))
        .context("Studio Supervisor protected secret backend is unavailable")?;
    let activity_packs = HttpDaemonActivityPackSource::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let runner_registry = RunnerTemplateRegistryV1::open(
        &args.state_dir.join("runner-templates/installed"),
        &args.runner_templates_dir,
    )
    .context("Studio Supervisor Runner Template registry is unavailable")?;
    let agent_profiles =
        AgentProfileStoreV1::open(&args.state_dir.join("agent-profiles"), vault.clone())
            .context("Studio Supervisor Agent Profile store is unavailable")?;
    let draft_dependencies = InstalledTaskTemplateDependenciesV1::new(
        ExactActivityPackDraftValidatorV1::new(activity_packs.clone()),
        agent_profiles.clone(),
        runner_registry.clone(),
    );
    let drafts = RoomDraftStoreV1::open(
        &args.state_dir.join("room-drafts"),
        draft_dependencies.clone(),
    )
    .context("Studio Supervisor protected Room draft store is unavailable")?;
    let task_templates = TaskTemplateStoreV1::open(
        &args.state_dir.join("task-templates"),
        drafts.clone(),
        draft_dependencies,
    )
    .context("Studio Supervisor protected Task Template store is unavailable")?;
    let rooms = HttpDaemonRoomSource::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let room_creator = HttpDaemonRoomCreatorV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let room_creation = RoomCreationSupervisorV1::open(
        &args.state_dir.join("room-creations"),
        drafts.clone(),
        room_creator,
    )
    .context("Studio Supervisor protected Room creation store is unavailable")?;
    let backup_executor = HttpDaemonBackupExecutorV1::new(
        args.daemon,
        daemon_timeout,
        args.storage_profile,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let backups = BackupOperationsV1::open(&backup_root, backup_executor).map_err(|error| {
        anyhow::anyhow!(
            "Studio Supervisor protected backup operation store is unavailable: {error:?}"
        )
    })?;
    let runners = RunnerSupervisorV1::open(
        runner_registry,
        &args.state_dir.join("runner-templates/runtime"),
        vault.clone(),
        Duration::from_millis(args.graceful_stop_timeout_ms),
    )
    .context("Studio Supervisor Runner instance state is unavailable")?;
    let task_runtime = HttpDaemonTaskRuntimeV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let task_setup_provisioner = HttpDaemonTaskSetupProvisionerV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let task_setup_base = TaskSetupSupervisorV1::open(
        &args.state_dir.join("task-setups"),
        room_creation.clone(),
        vault.clone(),
        task_setup_provisioner,
    )
    .context("Studio Supervisor protected Task setup store is unavailable")?
    .with_agent_profiles(agent_profiles.clone());
    let participant_handoff = ParticipantHandoffBrokerV1::new(
        &args.studio_origin,
        &args.participant_console_origin,
        Duration::from_secs(90),
        256,
        task_setup_base.clone(),
        FixedDaemonParticipantConsoleGatewayV1::new(args.daemon, daemon_timeout),
    )
    .map_err(|error| anyhow::anyhow!("Participant Console handoff is unavailable: {error:?}"))?;
    let task_setup = task_setup_base.with_launch_readiness(
        participant_handoff.clone(),
        LiveTaskRunnerReadinessSourceV1::new(task_runtime.clone(), runners.clone()),
        task_runtime,
    );
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
    let runner_attention_assignments = PersistedAgentSeatAssignmentSourceV1::new(
        agent_profiles.clone(),
        assignment_mcp_launches.clone(),
        task_setup.clone(),
    );
    let runner_attention_source = LiveRunnerAttentionSourceV1::new(
        runner_attention_assignments,
        HttpDaemonRunnerAttentionSourceV1::new(
            args.daemon,
            daemon_timeout,
            vault.clone(),
            args.host_authority_reference.clone(),
        ),
        runners.clone(),
    );
    let runner_attention = RunnerAttentionSupervisorV1::new(
        runner_attention_source,
        runners.clone(),
        FileRunnerRestartStoreV1::open(&args.state_dir.join("runner-attention/restarts"))
            .map_err(|error| anyhow::anyhow!("Runner attention store is unavailable: {error:?}"))?,
    );
    let canonical_state_dir = args
        .state_dir
        .canonicalize()
        .context("Studio Supervisor state directory is unavailable")?;
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
        task_setup.clone(),
    )
    .map_err(|error| anyhow::anyhow!("managed Agent Host operations are unavailable: {error:?}"))?;
    let runner_attention = runner_attention.with_managed_hosts(managed_agent_hosts.clone());
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
    let router = supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_setup_and_templates(
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
        agent_profiles,
        participant_handoff,
        task_templates,
    )
    .merge(assignment_mcp_launch_router(assignment_mcp_launches))
    .merge(managed_agent_host_router(managed_agent_hosts))
    .merge(runner_attention_router(runner_attention))
    .merge(attention_inbox_router(attention_inbox));
    axum::serve(listener, router)
        .await
        .context("Studio Supervisor server failed")
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
