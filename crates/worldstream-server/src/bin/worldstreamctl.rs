use std::{
    env, fs,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig,
    native_restore::{
        NATIVE_POSTGRES_ADMISSION_ARG_V1, NATIVE_POSTGRES_COMMIT_ARG_V1,
        NATIVE_POSTGRES_REPAIR_ARG_V1, NATIVE_POSTGRES_WORKER_ARG_V1,
        NativePostgresArtifactDirectoryIdentityV1, NativePostgresEndpointV1,
        NativePostgresRestoreConfig, NativePostgresTlsModeV1,
        execute_native_postgres_restore_admission_worker,
        execute_native_postgres_restore_commit_worker,
        execute_native_postgres_restore_repair_worker, execute_native_postgres_restore_worker,
        native_postgres_artifact_directory_identity, rebuild_native_snapshot_cache,
        recover_native_postgres_restore, run_native_postgres_restore,
    },
};
use worldstream_runtime::{
    CompatibilitySummary, SecretSource, embedded_manifest, prepare_data_directory,
    validate_owner_only_file, validate_sqlite_data_filesystem,
};
use worldstream_server::{
    CommonConfigArgs,
    operator_storage::{backup_sqlite, restore_sqlite, verify_sqlite},
    operator_transfer::{abort_transfer, begin_transfer, finalize_transfer, resume_transfer},
};

const MAX_DSN_BYTES: usize = 16 * 1024;
const MAX_HEALTH_RESPONSE_BYTES: u64 = 16 * 1024;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Parser)]
#[command(
    name = "worldstreamctl",
    version,
    about = "WorldStream operator CLI",
    after_long_help = concat!("Source revision: ", env!("WORLDSTREAM_BUILD_REVISION"))
)]
struct Cli {
    #[command(flatten)]
    config: CommonConfigArgs,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate or display effective configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Run bounded local config, manifest, and data-path checks.
    Doctor,
    /// Probe the configured daemon's process-liveness endpoint.
    Health,
    /// Print the embedded compatibility summary.
    Version,
    /// Run an explicit offline `PostgreSQL` direct-admin operation.
    Postgres {
        #[command(subcommand)]
        command: Box<PostgresCommand>,
    },
    /// Run an explicit offline bundled-SQLite operation.
    Sqlite {
        #[command(subcommand)]
        command: SqliteCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Strictly parse, layer, and validate configuration.
    Validate,
    /// Print effective configuration with every secret reference redacted.
    Effective,
}

#[derive(Debug, Subcommand)]
enum PostgresCommand {
    /// Apply forward-only migrations through a direct admin connection.
    Migrate(OfflinePostgresArgs),
    /// Verify the `PostgreSQL` engine and reviewed migration/schema contract.
    Verify(OfflinePostgresArgs),
    /// Run the resumable offline SQLite-to-PostgreSQL deployment transfer.
    Transfer {
        #[command(subcommand)]
        command: PostgresTransferCommand,
    },
    /// Rebuild reviewed disposable snapshot caches for native backup.
    Snapshots {
        #[command(subcommand)]
        command: PostgresSnapshotsCommand,
    },
    /// Run crash-recoverable provider-native backup and restore operations.
    Native {
        #[command(subcommand)]
        command: Box<PostgresNativeCommand>,
    },
}

#[derive(Debug, Subcommand)]
enum PostgresSnapshotsCommand {
    /// Rebuild disposable snapshots for every healthy Room.
    Rebuild(NativePostgresSnapshotRebuildArgs),
}

#[derive(Debug, Subcommand)]
enum PostgresNativeCommand {
    /// Observe the exact protected artifact-directory identity for admission.
    DirectoryIdentity(NativePostgresDirectoryIdentityArgs),
    /// Create, restore, verify, and durably report one native backup point.
    Restore(NativePostgresRestoreArgs),
    /// Repair one interrupted restore from its exact durable recovery journal.
    Recover(NativePostgresRecoverArgs),
}

#[derive(Debug, Args)]
struct NativePostgresSnapshotRebuildArgs {
    #[arg(long)]
    host: String,
    #[arg(long)]
    port: u16,
    #[arg(long)]
    database: String,
    #[arg(long)]
    username: String,
    #[arg(long)]
    tls_mode: NativePostgresTlsModeV1,
    /// Owner-only libpq passfile. Credential bytes are never accepted in argv.
    #[arg(long, value_name = "FILE")]
    passfile: PathBuf,
}

#[derive(Debug, Args)]
struct NativePostgresRestoreArgs {
    #[arg(long)]
    source_host: String,
    #[arg(long)]
    source_port: u16,
    #[arg(long)]
    source_database: String,
    #[arg(long)]
    source_username: String,
    #[arg(long)]
    source_tls_mode: NativePostgresTlsModeV1,
    #[arg(long)]
    target_host: String,
    #[arg(long)]
    target_port: u16,
    #[arg(long)]
    target_database: String,
    #[arg(long)]
    target_username: String,
    #[arg(long)]
    target_tls_mode: NativePostgresTlsModeV1,
    /// Owner-only libpq passfile containing source and target credentials.
    #[arg(long, value_name = "FILE")]
    passfile: PathBuf,
    #[arg(long, value_name = "FILE")]
    pg_dump: PathBuf,
    #[arg(long, value_name = "FILE")]
    pg_restore: PathBuf,
    #[arg(long, value_name = "FILE")]
    psql: PathBuf,
    /// New provider custom-format dump path.
    #[arg(long, value_name = "FILE")]
    dump: PathBuf,
    /// New durable canonical Ready report path.
    #[arg(long, value_name = "FILE")]
    report: PathBuf,
    #[command(flatten)]
    artifact_directory: NativePostgresArtifactDirectoryIdentityArgs,
    #[arg(long, default_value_t = 1_200)]
    timeout_seconds: u64,
}

#[derive(Debug, Args)]
struct NativePostgresDirectoryIdentityArgs {
    /// Existing owner-only dump/report/journal directory.
    #[arg(long, value_name = "DIR")]
    path: PathBuf,
}

#[derive(Debug, Args)]
struct NativePostgresArtifactDirectoryIdentityArgs {
    /// Fixed-width lowercase hexadecimal filesystem/volume identity.
    #[arg(long, value_name = "HEX")]
    artifact_directory_storage_id: String,
    /// Fixed-width lowercase hexadecimal inode/full file identity.
    #[arg(long, value_name = "HEX")]
    artifact_directory_file_id: String,
}

#[derive(Debug, Args)]
struct NativePostgresRecoverArgs {
    #[arg(long)]
    target_host: String,
    #[arg(long)]
    target_port: u16,
    #[arg(long)]
    target_database: String,
    #[arg(long)]
    target_username: String,
    #[arg(long)]
    target_tls_mode: NativePostgresTlsModeV1,
    /// Fresh owner-only libpq passfile used only for target repair.
    #[arg(long, value_name = "FILE")]
    passfile: PathBuf,
    /// Exact owner-only append-only recovery journal.
    #[arg(long, value_name = "FILE")]
    recovery: PathBuf,
    /// Fixed-width lowercase hexadecimal filesystem/volume identity of the journal.
    #[arg(long, value_name = "HEX")]
    recovery_storage_id: String,
    /// Fixed-width lowercase hexadecimal inode/full file identity of the journal.
    #[arg(long, value_name = "HEX")]
    recovery_file_id: String,
    #[command(flatten)]
    artifact_directory: NativePostgresArtifactDirectoryIdentityArgs,
    #[arg(long, default_value_t = 1_200)]
    timeout_seconds: u64,
}

#[derive(Debug, Subcommand)]
enum PostgresTransferCommand {
    /// Freeze `SQLite` and persist its verified backup, bundle, and checkpoint.
    Begin(TransferBeginArgs),
    /// Apply remaining bounded chunks while both backends remain fenced.
    Resume(TransferResumeArgs),
    /// Explicitly verify the target and perform the authority handoff.
    Finalize(TransferProviderArgs),
    /// Discard the incomplete target before restoring `SQLite` authority.
    Abort(TransferProviderArgs),
}

#[derive(Debug, Args)]
struct TransferBeginArgs {
    /// Authoritative bundled-SQLite database while serving is stopped.
    #[arg(long, value_name = "FILE")]
    sqlite: PathBuf,
    /// New native backup path, directly inside the owner-only state directory.
    #[arg(long, value_name = "FILE")]
    backup: PathBuf,
    /// New deterministic bundle path, directly inside the state directory.
    #[arg(long, value_name = "FILE")]
    bundle: PathBuf,
    /// Dedicated owner-only transfer state directory.
    #[arg(long, value_name = "DIR")]
    state_dir: PathBuf,
    /// Stable non-secret identifier embedded in the deterministic bundle.
    #[arg(long, value_name = "ID")]
    transfer_id: String,
}

#[derive(Debug, Args)]
struct TransferArtifactsArgs {
    /// Frozen bundled-SQLite source database.
    #[arg(long, value_name = "FILE")]
    sqlite: PathBuf,
    /// Exact deterministic bundle inside the owner-only state directory.
    #[arg(long, value_name = "FILE")]
    bundle: PathBuf,
    /// Dedicated owner-only transfer state directory.
    #[arg(long, value_name = "DIR")]
    state_dir: PathBuf,
}

#[derive(Debug, Args)]
struct TransferProviderArgs {
    #[command(flatten)]
    artifacts: TransferArtifactsArgs,
    /// Owner-only file containing the direct-admin DSN. The value is never
    /// accepted as a command-line argument or printed in diagnostics.
    #[arg(long, value_name = "FILE")]
    dsn_file: PathBuf,
}

#[derive(Debug, Args)]
struct TransferResumeArgs {
    #[command(flatten)]
    provider: TransferProviderArgs,
    /// Maximum logical records in one durable destination transaction.
    #[arg(long, default_value_t = 64)]
    chunk_records: usize,
}

#[derive(Debug, Subcommand)]
enum SqliteCommand {
    /// Create a native backup plus a sealed full-verifier companion envelope.
    Backup(SqliteBackupArgs),
    /// Restore an exact backup and require its full-verifier envelope.
    Restore(SqliteRestoreArgs),
    /// Run the native verifier only; this does not assert restore readiness.
    Verify(SqliteVerifyArgs),
}

#[derive(Debug, Args)]
struct SqliteBackupArgs {
    /// Source bundled-SQLite database.
    #[arg(long, value_name = "FILE")]
    database: PathBuf,
    /// New immutable backup destination. Existing paths are refused.
    #[arg(long, value_name = "FILE")]
    output: PathBuf,
    /// Canonical owner-only envelope template containing the companion facts
    /// that `SQLite` intentionally does not persist.
    #[arg(long, value_name = "FILE")]
    companion: PathBuf,
    /// New owner-only sealed envelope destination. Existing paths are refused.
    #[arg(long, value_name = "FILE")]
    envelope: PathBuf,
}

#[derive(Debug, Args)]
struct SqliteRestoreArgs {
    /// Source native `SQLite` backup.
    #[arg(long, value_name = "FILE")]
    backup: PathBuf,
    /// New restored database destination. Existing paths are refused.
    #[arg(long, value_name = "FILE")]
    database: PathBuf,
    /// Exact owner-only sealed envelope published with the native backup.
    #[arg(long, value_name = "FILE")]
    envelope: PathBuf,
}

#[derive(Debug, Args)]
struct SqliteVerifyArgs {
    /// Bundled-SQLite database to verify read-only.
    #[arg(long, value_name = "FILE")]
    database: PathBuf,
}

#[derive(Debug, Args)]
struct OfflinePostgresArgs {
    /// Owner-only file containing the direct-admin DSN. The value is never
    /// accepted as a command-line argument or printed in diagnostics.
    #[arg(long, value_name = "FILE")]
    dsn_file: PathBuf,
}

#[derive(Serialize)]
struct Status<'a> {
    status: &'a str,
    storage_profile: &'a str,
}

#[derive(Serialize)]
struct Doctor<'a> {
    status: &'a str,
    config: &'a str,
    manifest: &'a str,
    data_directory: &'a str,
    storage: &'a str,
}

#[derive(Serialize)]
struct VersionDocument {
    #[serde(flatten)]
    compatibility: CompatibilitySummary,
    product_build: ControlProductBuild,
}

#[derive(Serialize)]
struct ControlProductBuild {
    product: String,
    binary: &'static str,
    build_version: String,
    source_revision: &'static str,
}

fn version_document(compatibility: CompatibilitySummary) -> VersionDocument {
    let product = compatibility.contracts.product.clone();
    VersionDocument {
        compatibility,
        product_build: ControlProductBuild {
            product: product.clone(),
            binary: "worldstreamctl",
            build_version: product,
            source_revision: env!("WORLDSTREAM_BUILD_REVISION"),
        },
    }
}

fn main() -> Result<()> {
    if let Some(result) = dispatch_hidden_native_postgres_worker() {
        return result;
    }
    let Cli {
        config: config_args,
        command,
    } = Cli::parse();

    match command {
        Command::Postgres { command } => run_postgres_admin(*command),
        Command::Sqlite { command } => run_sqlite_operator(command),
        Command::Config {
            command: ConfigCommand::Validate,
        } => {
            let config = config_args.load().context("configuration rejected")?;
            write_json(&Status {
                status: "valid",
                storage_profile: config.storage.profile.as_str(),
            })
        }
        Command::Config {
            command: ConfigCommand::Effective,
        } => {
            let config = config_args.load().context("configuration rejected")?;
            write_json(&config.redacted())
        }
        Command::Doctor => {
            let config = config_args.load().context("configuration rejected")?;
            let data_dir = prepare_data_directory(&config.storage.data_dir)
                .context("data-directory validation failed")?;
            if config.storage.profile == worldstream_runtime::StorageProfile::SqliteBundled {
                validate_sqlite_data_filesystem(&data_dir)
                    .context("SQLite data-filesystem validation failed")?;
            }
            let _manifest = embedded_manifest().context("embedded manifest rejected")?;
            write_json(&Doctor {
                status: "incomplete",
                config: "valid",
                manifest: "valid_specification",
                data_directory: "owner_only",
                storage: "not_initialized",
            })
        }
        Command::Health => {
            let config = config_args.load().context("configuration rejected")?;
            probe_health(config.server.bind).context("daemon health probe failed")?;
            write_json(&Health {
                status: "ok",
                probe: "daemon-healthz",
            })
        }
        Command::Version => {
            let _config = config_args.load().context("configuration rejected")?;
            let manifest = embedded_manifest().context("embedded manifest rejected")?;
            write_json(&version_document(manifest.summary()))
        }
    }
}

fn dispatch_hidden_native_postgres_worker() -> Option<Result<()>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let marker = arguments.next()?;
    let recognized = [
        NATIVE_POSTGRES_ADMISSION_ARG_V1,
        NATIVE_POSTGRES_COMMIT_ARG_V1,
        NATIVE_POSTGRES_WORKER_ARG_V1,
        NATIVE_POSTGRES_REPAIR_ARG_V1,
    ]
    .into_iter()
    .any(|candidate| marker == candidate);
    if !recognized {
        return None;
    }
    if arguments.next().is_some() {
        return Some(Err(anyhow::anyhow!(
            "contained native PostgreSQL worker invocation failed closed"
        )));
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    if marker == NATIVE_POSTGRES_ADMISSION_ARG_V1 {
        return Some(
            execute_native_postgres_restore_admission_worker(stdin.lock(), stdout.lock()).map_err(
                |_| anyhow::anyhow!("contained native PostgreSQL admission failed closed"),
            ),
        );
    }
    if marker == NATIVE_POSTGRES_REPAIR_ARG_V1 {
        return Some(
            execute_native_postgres_restore_repair_worker(stdin.lock(), stdout.lock())
                .map_err(|_| anyhow::anyhow!("contained native PostgreSQL repair failed closed")),
        );
    }
    if marker == NATIVE_POSTGRES_COMMIT_ARG_V1 {
        return Some(
            execute_native_postgres_restore_commit_worker()
                .map_err(|_| anyhow::anyhow!("contained native PostgreSQL commit failed closed")),
        );
    }
    let stderr = io::stderr();
    Some(
        execute_native_postgres_restore_worker(stdin.lock(), stdout.lock(), stderr.lock())
            .and_then(|accepted| {
                if accepted {
                    Ok(())
                } else {
                    Err(worldstream_postgres::native_restore::NativePostgresError::ProviderCommand)
                }
            })
            .map_err(|_| anyhow::anyhow!("contained native PostgreSQL restore failed closed")),
    )
}

fn run_sqlite_operator(command: SqliteCommand) -> Result<()> {
    match command {
        SqliteCommand::Backup(args) => write_json(
            &backup_sqlite(
                &args.database,
                &args.output,
                &args.companion,
                &args.envelope,
            )
            .context("offline SQLite backup failed")?,
        ),
        SqliteCommand::Restore(args) => write_json(
            &restore_sqlite(&args.backup, &args.envelope, &args.database)
                .context("offline SQLite restore failed")?,
        ),
        SqliteCommand::Verify(args) => write_json(
            &verify_sqlite(&args.database).context("offline SQLite verification failed")?,
        ),
    }
}

#[derive(Serialize)]
struct Health<'a> {
    status: &'a str,
    probe: &'a str,
}

fn probe_health(mut address: SocketAddr) -> Result<()> {
    if address.ip().is_unspecified() {
        address.set_ip(match address.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    let mut stream = TcpStream::connect_timeout(&address, HEALTH_TIMEOUT)
        .context("configured daemon listener is unavailable")?;
    stream
        .set_read_timeout(Some(HEALTH_TIMEOUT))
        .context("health probe read timeout could not be configured")?;
    stream
        .set_write_timeout(Some(HEALTH_TIMEOUT))
        .context("health probe write timeout could not be configured")?;
    let host = match address.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    write!(
        stream,
        "GET /healthz HTTP/1.1\r\nHost: {host}:{}\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
        address.port()
    )
    .context("health probe request failed")?;
    stream.flush().context("health probe request failed")?;
    let mut response = Vec::new();
    stream
        .take(MAX_HEALTH_RESPONSE_BYTES + 1)
        .read_to_end(&mut response)
        .context("health probe response failed")?;
    if response.len() as u64 > MAX_HEALTH_RESPONSE_BYTES {
        bail!("health probe response exceeded the bounded limit");
    }
    validate_health_response(&response)
}

fn validate_health_response(response: &[u8]) -> Result<()> {
    let response = std::str::from_utf8(response).context("health probe response is not UTF-8")?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .context("health probe response is not HTTP")?;
    let status = head.lines().next().unwrap_or_default();
    if status != "HTTP/1.1 200 OK" && status != "HTTP/1.0 200 OK" {
        bail!("health probe returned a non-200 status");
    }
    let value: serde_json::Value =
        serde_json::from_str(body).context("health probe body is not JSON")?;
    if value != serde_json::json!({"status": "ok"}) {
        bail!("health probe body does not match the liveness contract");
    }
    Ok(())
}

fn run_postgres_admin(command: PostgresCommand) -> Result<()> {
    let operation = match command {
        PostgresCommand::Migrate(args) => {
            let admin = direct_admin_from_file(&args.dsn_file)?;
            admin
                .migrate()
                .map_err(|_| anyhow::anyhow!("offline PostgreSQL migration failed closed"))?;
            "migrate"
        }
        PostgresCommand::Verify(args) => {
            let admin = direct_admin_from_file(&args.dsn_file)?;
            admin.verify_schema().map_err(|_| {
                anyhow::anyhow!("offline PostgreSQL schema verification failed closed")
            })?;
            "verify"
        }
        PostgresCommand::Transfer { command } => return run_postgres_transfer(command),
        PostgresCommand::Snapshots { command } => return run_postgres_snapshots(command),
        PostgresCommand::Native { command } => return run_postgres_native(*command),
    };
    write_json(&PostgresAdminResult {
        status: "ok",
        operation,
        connection: "direct-admin",
        release_evidence: false,
    })
}

#[derive(Serialize)]
struct NativePostgresSnapshotReceipt {
    schema: &'static str,
    status: &'static str,
    rebuilt_snapshot_count: usize,
    source_provider_identity: worldstream_postgres::native_restore::PostgresProviderIdentityV1,
    secrets_emitted: bool,
}

#[derive(Serialize)]
struct NativePostgresArtifactDirectoryIdentityReceipt {
    schema: &'static str,
    status: &'static str,
    artifact_directory_identity: NativePostgresArtifactDirectoryIdentityV1,
    secrets_emitted: bool,
}

#[derive(Serialize)]
struct NativePostgresRestoreReceipt {
    schema: &'static str,
    status: &'static str,
    report_digest: String,
    report_size_bytes: u64,
    native_dump_digest: String,
    native_dump_size_bytes: u64,
    native_dump_identity: NativePostgresArtifactDirectoryIdentityV1,
    report_identity: NativePostgresArtifactDirectoryIdentityV1,
    recovery_record_identity: NativePostgresArtifactDirectoryIdentityV1,
    recovery_record_name: String,
    source_provider_identity: worldstream_postgres::native_restore::PostgresProviderIdentityV1,
    target_provider_identity: worldstream_postgres::native_restore::PostgresProviderIdentityV1,
    secrets_emitted: bool,
}

#[derive(Serialize)]
struct NativePostgresRecoveryReceipt {
    schema: &'static str,
    status: &'static str,
    target_disposition: &'static str,
    artifacts_disposition: &'static str,
    secrets_emitted: bool,
}

fn run_postgres_snapshots(command: PostgresSnapshotsCommand) -> Result<()> {
    match command {
        PostgresSnapshotsCommand::Rebuild(args) => {
            if !args.passfile.is_absolute() {
                bail!("native PostgreSQL snapshot rebuild requires an absolute passfile path");
            }
            let endpoint = NativePostgresEndpointV1::new(
                args.host,
                args.port,
                args.database,
                args.username,
                args.tls_mode,
            )
            .map_err(|_| anyhow::anyhow!("native PostgreSQL endpoint was rejected"))?;
            let rebuilt = rebuild_native_snapshot_cache(&endpoint, &args.passfile)
                .map_err(|_| anyhow::anyhow!("native PostgreSQL snapshot rebuild failed closed"))?;
            write_json(&NativePostgresSnapshotReceipt {
                schema: "worldstream/postgres-native-snapshot-rebuild-receipt/v1",
                status: "complete",
                rebuilt_snapshot_count: rebuilt.rebuilt_snapshot_count,
                source_provider_identity: rebuilt.source_provider_identity,
                secrets_emitted: false,
            })
        }
    }
}

fn run_postgres_native(command: PostgresNativeCommand) -> Result<()> {
    match command {
        PostgresNativeCommand::DirectoryIdentity(args) => run_postgres_native_identity(&args),
        PostgresNativeCommand::Restore(args) => run_postgres_native_restore(args),
        PostgresNativeCommand::Recover(args) => run_postgres_native_recover(args),
    }
}

fn run_postgres_native_identity(args: &NativePostgresDirectoryIdentityArgs) -> Result<()> {
    if !args.path.is_absolute() {
        bail!("native PostgreSQL artifact directory must be absolute");
    }
    let identity = native_postgres_artifact_directory_identity(&args.path)
        .map_err(|_| anyhow::anyhow!("native PostgreSQL artifact directory was rejected"))?;
    write_json(&NativePostgresArtifactDirectoryIdentityReceipt {
        schema: "worldstream/postgres-native-artifact-directory-identity/v1",
        status: "observed",
        artifact_directory_identity: identity,
        secrets_emitted: false,
    })
}

fn run_postgres_native_restore(args: NativePostgresRestoreArgs) -> Result<()> {
    let source = NativePostgresEndpointV1::new(
        args.source_host,
        args.source_port,
        args.source_database,
        args.source_username,
        args.source_tls_mode,
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL source endpoint was rejected"))?;
    let target = NativePostgresEndpointV1::new(
        args.target_host,
        args.target_port,
        args.target_database,
        args.target_username,
        args.target_tls_mode,
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL target endpoint was rejected"))?;
    let artifact_directory_identity = native_artifact_identity(args.artifact_directory)?;
    let config = NativePostgresRestoreConfig::new(
        source,
        target,
        args.passfile,
        args.pg_dump,
        args.pg_restore,
        args.psql,
        args.dump,
        artifact_directory_identity,
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL restore configuration was rejected"))?;
    let outcome = run_native_postgres_restore(
        &config,
        &args.report,
        Duration::from_secs(args.timeout_seconds),
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL restore failed closed"))?;
    let report = outcome.report();
    let mut exact_report = serde_json::to_vec(report)
        .context("native PostgreSQL report receipt serialization failed")?;
    exact_report.push(b'\n');
    let report_size_bytes = u64::try_from(exact_report.len())
        .context("native PostgreSQL report receipt size is invalid")?;
    write_json(&NativePostgresRestoreReceipt {
        schema: "worldstream/postgres-native-restore-receipt/v1",
        status: "committed",
        report_digest: format!("blake3:{}", blake3::hash(&exact_report).to_hex()),
        report_size_bytes,
        native_dump_digest: report.native_dump_digest.clone(),
        native_dump_size_bytes: report.native_dump_size_bytes,
        native_dump_identity: outcome.native_dump_identity().clone(),
        report_identity: outcome.report_identity().clone(),
        recovery_record_identity: outcome.recovery_record_identity().clone(),
        recovery_record_name: outcome.recovery_record_name().to_owned(),
        source_provider_identity: report.source_provider_identity.clone(),
        target_provider_identity: report.target_provider_identity.clone(),
        secrets_emitted: false,
    })
}

fn run_postgres_native_recover(args: NativePostgresRecoverArgs) -> Result<()> {
    let target = NativePostgresEndpointV1::new(
        args.target_host,
        args.target_port,
        args.target_database,
        args.target_username,
        args.target_tls_mode,
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL target endpoint was rejected"))?;
    let recovery_record_identity = NativePostgresArtifactDirectoryIdentityV1::new(
        args.recovery_storage_id,
        args.recovery_file_id,
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL recovery journal identity was rejected"))?;
    let artifact_directory_identity = native_artifact_identity(args.artifact_directory)?;
    recover_native_postgres_restore(
        &target,
        &args.passfile,
        &args.recovery,
        &recovery_record_identity,
        &artifact_directory_identity,
        Duration::from_secs(args.timeout_seconds),
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL recovery failed closed"))?;
    write_json(&NativePostgresRecoveryReceipt {
        schema: "worldstream/postgres-native-recovery-receipt/v1",
        status: "complete",
        target_disposition: "identity_bound_isolated_and_non_serving",
        artifacts_disposition: "recorded_private_artifacts_scrubbed",
        secrets_emitted: false,
    })
}

fn native_artifact_identity(
    args: NativePostgresArtifactDirectoryIdentityArgs,
) -> Result<NativePostgresArtifactDirectoryIdentityV1> {
    NativePostgresArtifactDirectoryIdentityV1::new(
        args.artifact_directory_storage_id,
        args.artifact_directory_file_id,
    )
    .map_err(|_| anyhow::anyhow!("native PostgreSQL artifact directory identity was rejected"))
}

fn run_postgres_transfer(command: PostgresTransferCommand) -> Result<()> {
    match command {
        PostgresTransferCommand::Begin(args) => write_json(
            &begin_transfer(
                &args.sqlite,
                &args.backup,
                &args.bundle,
                &args.state_dir,
                &args.transfer_id,
            )
            .context("offline SQLite-to-PostgreSQL transfer begin failed")?,
        ),
        PostgresTransferCommand::Resume(args) => {
            let admin = direct_admin_from_file(&args.provider.dsn_file)?;
            let artifacts = args.provider.artifacts;
            write_json(
                &resume_transfer(
                    &artifacts.sqlite,
                    &artifacts.bundle,
                    &artifacts.state_dir,
                    &admin,
                    args.chunk_records,
                )
                .context("offline SQLite-to-PostgreSQL transfer resume failed")?,
            )
        }
        PostgresTransferCommand::Finalize(args) => {
            let admin = direct_admin_from_file(&args.dsn_file)?;
            write_json(
                &finalize_transfer(
                    &args.artifacts.sqlite,
                    &args.artifacts.bundle,
                    &args.artifacts.state_dir,
                    &admin,
                )
                .context("offline SQLite-to-PostgreSQL transfer finalize failed")?,
            )
        }
        PostgresTransferCommand::Abort(args) => {
            let admin = direct_admin_from_file(&args.dsn_file)?;
            write_json(
                &abort_transfer(
                    &args.artifacts.sqlite,
                    &args.artifacts.bundle,
                    &args.artifacts.state_dir,
                    &admin,
                )
                .context("offline SQLite-to-PostgreSQL transfer abort failed")?,
            )
        }
    }
}

fn direct_admin_from_file(path: &Path) -> Result<PostgresAdmin> {
    let dsn = read_dsn_file(&SecretSource::File(path.to_owned()))?;
    let config = PostgresConnectionConfig::direct_admin(dsn)
        .map_err(|_| anyhow::anyhow!("direct-admin PostgreSQL DSN policy was rejected"))?;
    PostgresAdmin::new(config)
        .map_err(|_| anyhow::anyhow!("direct-admin PostgreSQL profile was rejected"))
}

#[derive(Serialize)]
struct PostgresAdminResult {
    status: &'static str,
    operation: &'static str,
    connection: &'static str,
    release_evidence: bool,
}

fn read_dsn_file(source: &SecretSource) -> Result<String> {
    let SecretSource::File(path) = source else {
        bail!("offline PostgreSQL administration requires an owner-only DSN secret file");
    };
    validate_owner_only_file(path)
        .map_err(|_| anyhow::anyhow!("PostgreSQL DSN secret-file validation failed"))?;
    let file = fs::File::open(path)
        .map_err(|_| anyhow::anyhow!("PostgreSQL DSN secret-file read failed"))?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_DSN_BYTES + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("PostgreSQL DSN secret-file read failed"))?;
    if bytes.is_empty() || bytes.len() > MAX_DSN_BYTES {
        bail!("PostgreSQL DSN secret-file size is outside the bounded limit");
    }
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| anyhow::anyhow!("PostgreSQL DSN secret-file is not UTF-8"))?
        .trim();
    if value.is_empty()
        || value
            .bytes()
            .any(|byte| byte == b'\0' || byte == b'\r' || byte == b'\n')
    {
        bail!("PostgreSQL DSN secret-file contains invalid control data");
    }
    Ok(value.to_owned())
}

fn write_json(value: &impl Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, value).context("JSON output failed")?;
    writeln!(output).context("JSON output failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Command, NativePostgresArtifactDirectoryIdentityV1, NativePostgresRestoreReceipt,
        PostgresCommand, PostgresNativeCommand, PostgresSnapshotsCommand, PostgresTransferCommand,
        SqliteCommand, validate_health_response, version_document,
    };
    use clap::Parser as _;
    use worldstream_runtime::embedded_manifest;

    #[test]
    fn shipped_cli_exposes_sqlite_backup_restore_and_full_verification()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (
                vec![
                    "worldstreamctl",
                    "sqlite",
                    "backup",
                    "--database",
                    "worldstream.sqlite3",
                    "--output",
                    "worldstream.backup.sqlite3",
                    "--companion",
                    "worldstream.companion.json",
                    "--envelope",
                    "worldstream.backup.envelope.json",
                ],
                "backup",
            ),
            (
                vec![
                    "worldstreamctl",
                    "sqlite",
                    "restore",
                    "--backup",
                    "worldstream.backup.sqlite3",
                    "--database",
                    "restored.sqlite3",
                    "--envelope",
                    "worldstream.backup.envelope.json",
                ],
                "restore",
            ),
            (
                vec![
                    "worldstreamctl",
                    "sqlite",
                    "verify",
                    "--database",
                    "worldstream.sqlite3",
                ],
                "verify",
            ),
        ];
        for (arguments, operation) in cases {
            let cli = Cli::try_parse_from(arguments)?;
            let Command::Sqlite { command } = cli.command else {
                return Err("expected SQLite command".into());
            };
            let actual = match command {
                SqliteCommand::Backup(_) => "backup",
                SqliteCommand::Restore(_) => "restore",
                SqliteCommand::Verify(_) => "verify",
            };
            assert_eq!(actual, operation);
        }
        Ok(())
    }

    #[test]
    fn shipped_sqlite_restore_refuses_native_only_readiness_claim() {
        let missing_envelope = Cli::try_parse_from([
            "worldstreamctl",
            "sqlite",
            "restore",
            "--backup",
            "worldstream.backup.sqlite3",
            "--database",
            "restored.sqlite3",
        ]);
        assert!(missing_envelope.is_err());
    }

    #[test]
    fn shipped_cli_exposes_resumable_whole_deployment_transfer()
    -> Result<(), Box<dyn std::error::Error>> {
        let common = [
            "--sqlite",
            "worldstream.sqlite3",
            "--bundle",
            "transfer.bundle",
            "--state-dir",
            "transfer-state",
        ];
        let cases = vec![
            (
                vec![
                    "worldstreamctl",
                    "postgres",
                    "transfer",
                    "begin",
                    "--sqlite",
                    "worldstream.sqlite3",
                    "--backup",
                    "transfer.backup.sqlite3",
                    "--bundle",
                    "transfer.bundle",
                    "--state-dir",
                    "transfer-state",
                    "--transfer-id",
                    "offline-cutover-2026-08-22",
                ],
                "begin",
            ),
            (
                vec![
                    "worldstreamctl",
                    "postgres",
                    "transfer",
                    "resume",
                    common[0],
                    common[1],
                    common[2],
                    common[3],
                    common[4],
                    common[5],
                    "--dsn-file",
                    "admin.dsn",
                ],
                "resume",
            ),
            (
                vec![
                    "worldstreamctl",
                    "postgres",
                    "transfer",
                    "finalize",
                    common[0],
                    common[1],
                    common[2],
                    common[3],
                    common[4],
                    common[5],
                    "--dsn-file",
                    "admin.dsn",
                ],
                "finalize",
            ),
            (
                vec![
                    "worldstreamctl",
                    "postgres",
                    "transfer",
                    "abort",
                    common[0],
                    common[1],
                    common[2],
                    common[3],
                    common[4],
                    common[5],
                    "--dsn-file",
                    "admin.dsn",
                ],
                "abort",
            ),
        ];
        for (arguments, expected) in cases {
            let cli = Cli::try_parse_from(arguments)?;
            let Command::Postgres { command } = cli.command else {
                return Err("expected PostgreSQL transfer command".into());
            };
            let PostgresCommand::Transfer { command } = *command else {
                return Err("expected PostgreSQL transfer command".into());
            };
            let actual = match command {
                PostgresTransferCommand::Begin(_) => "begin",
                PostgresTransferCommand::Resume(_) => "resume",
                PostgresTransferCommand::Finalize(_) => "finalize",
                PostgresTransferCommand::Abort(_) => "abort",
            };
            assert_eq!(actual, expected);
        }
        Ok(())
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn shipped_cli_exposes_native_snapshot_restore_and_recovery_without_secret_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let rebuild = Cli::try_parse_from([
            "worldstreamctl",
            "postgres",
            "snapshots",
            "rebuild",
            "--host",
            "127.0.0.1",
            "--port",
            "5432",
            "--database",
            "source",
            "--username",
            "postgres",
            "--tls-mode",
            "disable",
            "--passfile",
            "/state/admin.pgpass",
        ])?;
        assert!(matches!(
            rebuild.command,
            Command::Postgres { command }
                if matches!(*command, PostgresCommand::Snapshots {
                    command: PostgresSnapshotsCommand::Rebuild(_)
                })
        ));

        let restore = Cli::try_parse_from([
            "worldstreamctl",
            "postgres",
            "native",
            "restore",
            "--source-host",
            "127.0.0.1",
            "--source-port",
            "5432",
            "--source-database",
            "source",
            "--source-username",
            "postgres",
            "--source-tls-mode",
            "disable",
            "--target-host",
            "127.0.0.1",
            "--target-port",
            "5433",
            "--target-database",
            "target",
            "--target-username",
            "postgres",
            "--target-tls-mode",
            "disable",
            "--passfile",
            "/state/admin.pgpass",
            "--pg-dump",
            "/usr/bin/pg_dump",
            "--pg-restore",
            "/usr/bin/pg_restore",
            "--psql",
            "/usr/bin/psql",
            "--dump",
            "/state/source.dump",
            "--artifact-directory-storage-id",
            "0000000000000001",
            "--artifact-directory-file-id",
            "00000000000000000000000000000002",
            "--report",
            "/state/native-report.json",
            "--timeout-seconds",
            "1200",
        ])?;
        assert!(matches!(
            restore.command,
            Command::Postgres { command }
                if matches!(command.as_ref(), PostgresCommand::Native { command }
                    if matches!(command.as_ref(), PostgresNativeCommand::Restore(_)))
        ));

        let recovery = Cli::try_parse_from([
            "worldstreamctl",
            "postgres",
            "native",
            "recover",
            "--target-host",
            "127.0.0.1",
            "--target-port",
            "5433",
            "--target-database",
            "target",
            "--target-username",
            "postgres",
            "--target-tls-mode",
            "disable",
            "--passfile",
            "/state/admin.pgpass",
            "--recovery",
            "/state/.worldstream_native_recovery_example.json",
            "--recovery-storage-id",
            "0000000000000003",
            "--recovery-file-id",
            "00000000000000000000000000000004",
            "--artifact-directory-storage-id",
            "0000000000000001",
            "--artifact-directory-file-id",
            "00000000000000000000000000000002",
            "--timeout-seconds",
            "1200",
        ])?;
        assert!(matches!(
            recovery.command,
            Command::Postgres { command }
                if matches!(command.as_ref(), PostgresCommand::Native { command }
                    if matches!(command.as_ref(), PostgresNativeCommand::Recover(_)))
        ));

        let leaked_secret = Cli::try_parse_from([
            "worldstreamctl",
            "postgres",
            "native",
            "recover",
            "--target-host",
            "127.0.0.1",
            "--target-port",
            "5433",
            "--target-database",
            "target",
            "--target-username",
            "postgres",
            "--target-tls-mode",
            "disable",
            "--password",
            "must-not-enter-argv",
            "--recovery",
            "/state/recovery.json",
        ]);
        assert!(leaked_secret.is_err());
        Ok(())
    }

    #[test]
    fn native_restore_receipt_binds_every_committed_artifact_identity() -> anyhow::Result<()> {
        let identity = NativePostgresArtifactDirectoryIdentityV1::new(
            "0000000000000001",
            "00000000000000000000000000000002",
        )?;
        let provider = worldstream_postgres::native_restore::PostgresProviderIdentityV1 {
            system_identifier: "1".to_owned(),
            database_oid: "2".to_owned(),
            database_name: "worldstream".to_owned(),
        };
        let receipt = serde_json::to_value(NativePostgresRestoreReceipt {
            schema: "worldstream/postgres-native-restore-receipt/v1",
            status: "committed",
            report_digest: "blake3:00".to_owned(),
            report_size_bytes: 1,
            native_dump_digest: "00".repeat(32),
            native_dump_size_bytes: 1,
            native_dump_identity: identity.clone(),
            report_identity: identity.clone(),
            recovery_record_identity: identity.clone(),
            recovery_record_name:
                ".worldstream_native_recovery_00000000000000000000000000000000.json".to_owned(),
            source_provider_identity: provider.clone(),
            target_provider_identity: provider,
            secrets_emitted: false,
        })?;
        for field in [
            "native_dump_identity",
            "report_identity",
            "recovery_record_identity",
        ] {
            assert_eq!(receipt[field], serde_json::to_value(&identity)?);
        }
        assert_eq!(
            receipt["recovery_record_name"],
            ".worldstream_native_recovery_00000000000000000000000000000000.json"
        );
        Ok(())
    }

    #[test]
    fn health_response_requires_exact_200_liveness_document() {
        let validation = validate_health_response(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"status\":\"ok\"}",
        );
        assert!(validation.is_ok(), "valid health response: {validation:?}");

        for rejected in [
            &b"HTTP/1.1 503 Service Unavailable\r\n\r\n{\"status\":\"ok\"}"[..],
            &b"HTTP/1.1 200 OK\r\n\r\n{\"status\":\"ready\"}"[..],
            &b"not-http"[..],
        ] {
            assert!(validate_health_response(rejected).is_err());
        }
    }

    #[test]
    fn version_document_attests_the_compiled_binary_revision() -> anyhow::Result<()> {
        let manifest = embedded_manifest()?;
        let value = serde_json::to_value(version_document(manifest.summary()))?;

        assert_eq!(
            value["product_build"]["product"],
            value["contracts"]["product"]
        );
        assert_eq!(value["product_build"]["binary"], "worldstreamctl");
        assert_eq!(
            value["product_build"]["build_version"],
            value["contracts"]["product"]
        );
        assert_eq!(
            value["product_build"]["source_revision"],
            env!("WORLDSTREAM_BUILD_REVISION")
        );
        Ok(())
    }
}
