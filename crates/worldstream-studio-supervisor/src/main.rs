use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::Parser;
use worldstream_runtime::{CliOverrides, ConfigLoader};
use worldstream_studio_supervisor::{
    HttpDaemonStatusSource,
    activity_packs::HttpDaemonActivityPackSource,
    backups::{
        BackupOperationsV1, BackupStorageProfileV1, HttpDaemonBackupExecutorV1,
        prepare_shared_backup_root,
    },
    lifecycle::ConfiguredDaemonLifecycle,
    room_creation::{HttpDaemonRoomCreatorV1, RoomCreationSupervisorV1},
    room_drafts::{ExactActivityPackDraftValidatorV1, RoomDraftStoreV1},
    rooms::HttpDaemonRoomSource,
    runner_templates::{RunnerSupervisorV1, RunnerTemplateRegistryV1},
    secrets::{FileSecretVaultV1, SecretReferenceV1},
    supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_and_setup,
    task_setup::{HttpDaemonTaskSetupProvisionerV1, TaskSetupSupervisorV1},
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

    /// Exact retained Host authority reference used by bounded daemon proxies.
    #[arg(long, value_parser = parse_secret_reference)]
    host_authority_reference: Option<SecretReferenceV1>,

    /// Owner-controlled directory of approved Runner Template manifests.
    #[arg(long, default_value = "config/runner-templates")]
    runner_templates_dir: PathBuf,

    /// Storage profile configured for the controlled worldstreamd process.
    #[arg(long, default_value = "sqlite-bundled", value_parser = parse_backup_profile)]
    storage_profile: BackupStorageProfileV1,
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
    let draft_validator = ExactActivityPackDraftValidatorV1::new(activity_packs.clone());
    let drafts = RoomDraftStoreV1::open(&args.state_dir.join("room-drafts"), draft_validator)
        .context("Studio Supervisor protected Room draft store is unavailable")?;
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
    let task_setup_provisioner = HttpDaemonTaskSetupProvisionerV1::new(
        args.daemon,
        daemon_timeout,
        vault.clone(),
        args.host_authority_reference.clone(),
    );
    let task_setup = TaskSetupSupervisorV1::open(
        &args.state_dir.join("task-setups"),
        room_creation.clone(),
        vault.clone(),
        task_setup_provisioner,
    )
    .context("Studio Supervisor protected Task setup store is unavailable")?;
    let backup_executor = HttpDaemonBackupExecutorV1::new(
        args.daemon,
        daemon_timeout,
        args.storage_profile,
        vault.clone(),
        args.host_authority_reference,
    );
    let backups = BackupOperationsV1::open(&backup_root, backup_executor).map_err(|error| {
        anyhow::anyhow!(
            "Studio Supervisor protected backup operation store is unavailable: {error:?}"
        )
    })?;
    let runner_registry = RunnerTemplateRegistryV1::open(
        &args.state_dir.join("runner-templates/installed"),
        &args.runner_templates_dir,
    )
    .context("Studio Supervisor Runner Template registry is unavailable")?;
    let runners = RunnerSupervisorV1::open(
        runner_registry,
        &args.state_dir.join("runner-templates/runtime"),
        vault.clone(),
        Duration::from_millis(args.graceful_stop_timeout_ms),
    )
    .context("Studio Supervisor Runner instance state is unavailable")?;
    axum::serve(
        listener,
        supervisor_router_with_lifecycle_secrets_runners_activity_packs_rooms_drafts_backups_creation_and_setup(
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
        ),
    )
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
