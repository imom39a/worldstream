use std::{str::FromStr, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::info;
use tracing_subscriber::EnvFilter;
use worldstream_core::{
    AuthorityBootstrapV1, AuthorityChangeId, AuthorityChangeReceiptV1, AuthorityChangeResultV1,
    AuthorityCheckedAt, AuthorityStoreV1, AuthorityV1, CapabilityBearerV1, CapabilityId,
    PackRegistryV1, PrincipalId, PrincipalKindV1, builtin_worldstream_registry,
};
use worldstream_postgres::{PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore};
use worldstream_runtime::{
    SecretSource, StorageConfig, StorageProfile, prepare_data_directory,
    validate_sqlite_data_filesystem,
};
use worldstream_server::{
    CommonConfigArgs, OperatorState, PostgresGatewayBackend, SqliteGatewayBackend,
    StructuredLogTelemetryExporter, operator_router, read_postgres_dsn, telemetry,
};
use worldstream_sqlite::SqliteRoomStore;
use worldstream_transfer::{DeploymentIdentityV1, DigestV1, PackIdentityV1};

const BOOTSTRAP_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC4";
const BOOTSTRAP_PRINCIPAL_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC2";
const BOOTSTRAP_CAPABILITY_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FC3";

#[derive(Debug, Parser)]
#[command(
    name = "worldstreamd",
    version,
    about = "WorldStream process shell",
    after_long_help = concat!("Source revision: ", env!("WORLDSTREAM_BUILD_REVISION"))
)]
struct DaemonArgs {
    #[command(flatten)]
    config: CommonConfigArgs,
}

#[tokio::main]
#[allow(clippy::redundant_closure_for_method_calls, clippy::too_many_lines)]
async fn main() -> Result<()> {
    let args = DaemonArgs::parse();
    let config = args.config.load().context("configuration rejected")?;

    if let Err(error) = tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init()
    {
        anyhow::bail!("structured logging initialization failed: {error}");
    }

    let data_dir = prepare_data_directory(&config.storage.data_dir)
        .context("data-directory validation failed")?;
    let bind = config.server.bind;
    let profile = config.storage.profile;
    let telemetry_exporter = build_telemetry_exporter(
        config
            .telemetry
            .otlp_endpoint
            .as_ref()
            .map(|endpoint| endpoint.as_str()),
    );
    if let Some(diagnostic) = config.redacted().telemetry.diagnostic {
        tracing::warn!(
            schema = telemetry::TELEMETRY_SCHEMA_V1,
            event = "storage_diagnostic",
            reason = "exporter_malformed_endpoint",
            diagnostic,
            "telemetry exporter configuration was rejected; using bounded structured logs"
        );
    }
    let telemetry_runtime =
        telemetry::TelemetryRuntime::new(telemetry::TelemetryConfig::default(), telemetry_exporter)
            .map_err(|error| {
                anyhow::anyhow!("diagnostic telemetry runtime initialization failed: {error:?}")
            })?;
    let sqlite_telemetry: Arc<dyn worldstream_sqlite::SqliteTelemetrySink> = Arc::new(
        telemetry::SqliteTelemetryBridge::new(telemetry_runtime.handle()),
    );
    let state = match profile {
        StorageProfile::SqliteBundled => {
            validate_sqlite_data_filesystem(&data_dir)
                .context("SQLite data-filesystem validation failed")?;
            let database_path = data_dir.join("worldstream.sqlite3");
            let store = SqliteRoomStore::open_with_telemetry(
                &database_path,
                Some(sqlite_telemetry.clone()),
            )
            .with_context(|| {
                format!(
                    "SQLite durable store initialization/verification failed at {}",
                    database_path.display()
                )
            })?;
            if store.source_transfer_state()
                != worldstream_sqlite::SqliteSourceTransferStateV1::SourceAuthoritative
            {
                anyhow::bail!(
                    "SQLite source is {:?}; this process must not serve or mutate it",
                    store.source_transfer_state()
                );
            }
            let canonical_metadata = initialize_canonical_metadata(&store, &config.storage)
                .context("SQLite canonical deployment metadata initialization failed")?;
            let registry = builtin_worldstream_registry()
                .context("WorldStream Activity Pack registry initialization/verification failed")?;
            let deployment_identity = initialize_deployment_identity(&store, &registry)
                .context("SQLite deployment Pack identity initialization failed")?;
            let _bootstrap_receipt =
                bootstrap_authority(&store, config.authority.bootstrap_secret.as_ref())
                    .context("SQLite authority bootstrap failed closed")?;
            let (sqlite_version, sqlite_source_id) = store.engine_identity();
            let backup_root = worldstream_runtime::prepare_live_backup_root(&data_dir)
                .context("shared SQLite live-backup root initialization failed")?;
            let backend = Arc::new(
                SqliteGatewayBackend::new(store, Arc::new(registry))
                    .with_live_backup_root(&backup_root)
                    .context("SQLite live-backup root initialization failed")?,
            );
            let state = OperatorState::new(config)
                .context("operator state initialization failed")?
                .with_backend(backend)
                .with_telemetry_runtime(telemetry_runtime)
                .with_readiness(
                    worldstream_server::RuntimeReadiness::durable_store_verified()
                        .with_authority_bootstrapped(),
                )
                .with_verified_sqlite_engine(sqlite_version, sqlite_source_id)
                .with_scheduler()
                .context("activation scheduler initialization failed")?;
            info!(
                storage_profile = %profile,
                data_directory = %data_dir.display(),
                sqlite_engine_version = sqlite_version,
                sqlite_source_id = sqlite_source_id,
                schema = "verified",
                storage = "writable",
                writer = "running_at_startup",
                scheduler = "running",
                authority_bootstrap = "verified",
                canonical_export_metadata = canonical_metadata.as_str(),
                deployment_identity,
                readiness = "ready",
                "WorldStream SQLite operator shell started; runtime, host authority bootstrap, and Activation scheduler are running"
            );
            state
        }
        StorageProfile::PostgresPrimary => {
            // The provider uses synchronous PostgreSQL clients. Keep every
            // provider call, including the scheduler's first tick, off the
            // daemon's Tokio worker: postgres::Config::connect creates and
            // blocks a private runtime internally.
            let (state, engine_identity) = run_on_blocking_worker(move || {
                let source = config.storage.postgresql_dsn.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("postgres-primary runtime DSN secret source is missing")
                })?;
                let dsn = read_postgres_dsn(source)?;
                let connection =
                    PostgresConnectionConfig::runtime(dsn, PostgresConnectionPath::Direct)
                        .map_err(|_| {
                            anyhow::anyhow!("postgres-primary runtime DSN policy was rejected")
                        })?;
                let pg_telemetry: Arc<dyn worldstream_postgres::PostgresTelemetrySink> = Arc::new(
                    telemetry::PostgresTelemetryBridge::new(telemetry_runtime.handle()),
                );
                let store = PostgresRoomStore::new(connection)
                    .map_err(|_| {
                        anyhow::anyhow!("postgres-primary runtime store profile was rejected")
                    })?
                    .with_telemetry(pg_telemetry);
                store.verify_schema().map_err(|_| {
                    anyhow::anyhow!("postgres-primary runtime schema verification failed closed")
                })?;
                let engine_identity = store.engine_identity().map_err(|_| {
                    anyhow::anyhow!("postgres-primary engine identity probe failed closed")
                })?;
                let _bootstrap_receipt =
                    bootstrap_authority(&store, config.authority.bootstrap_secret.as_ref())
                        .context("PostgreSQL authority bootstrap failed closed")?;
                let engine_identity = engine_identity.formatted().to_owned();
                let backend = PostgresGatewayBackend::new(store);
                let state = OperatorState::new(config)
                    .context("operator state initialization failed")?
                    .with_backend(Arc::new(backend))
                    .with_telemetry_runtime(telemetry_runtime)
                    .with_readiness(
                        worldstream_server::RuntimeReadiness::durable_store_verified()
                            .with_authority_bootstrapped(),
                    )
                    .with_verified_engine(engine_identity.clone())
                    .with_scheduler()
                    .context("PostgreSQL activation scheduler initialization failed")?;
                Ok((state, engine_identity))
            })
            .await?;
            info!(
                storage_profile = %profile,
                data_directory = %data_dir.display(),
                engine_identity = %engine_identity,
                schema = "verified",
                storage = "runtime-primary",
                writer = "running_at_startup",
                scheduler = "running",
                authority_bootstrap = "verified",
                readiness = "ready",
                "WorldStream PostgreSQL operator shell started; durable authority and Activation scheduler are running"
            );
            state
        }
    };
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("listener bind failed at {bind}"))?;
    info!(
        listen_address = %bind,
        storage_profile = %profile,
        "WorldStream operator listener bound"
    );

    axum::serve(
        listener,
        operator_router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("operator HTTP server failed")
}

async fn run_on_blocking_worker<F, T>(work: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .context("blocking PostgreSQL startup worker failed")?
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CanonicalMetadataStartup {
    Absent,
    Initialized,
    AlreadyInitialized,
}

impl CanonicalMetadataStartup {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Initialized => "initialized",
            Self::AlreadyInitialized => "already_initialized",
        }
    }
}

fn initialize_canonical_metadata(
    store: &SqliteRoomStore,
    storage: &StorageConfig,
) -> Result<CanonicalMetadataStartup> {
    match (&storage.deployment_lineage, storage.storage_epoch) {
        (Some(lineage), Some(epoch)) => {
            let result = store
                .initialize_canonical_metadata(lineage.as_str(), epoch.get())
                .context("explicit deployment metadata was rejected")?;
            Ok(match result {
                worldstream_sqlite::SqliteCanonicalMetadataInitializationV1::Initialized => {
                    CanonicalMetadataStartup::Initialized
                }
                worldstream_sqlite::SqliteCanonicalMetadataInitializationV1::AlreadyInitialized => {
                    CanonicalMetadataStartup::AlreadyInitialized
                }
            })
        }
        (None, None) => Ok(CanonicalMetadataStartup::Absent),
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("deployment lineage and storage epoch must be supplied together")
        }
    }
}

fn initialize_deployment_identity(
    store: &SqliteRoomStore,
    registry: &PackRegistryV1,
) -> Result<&'static str> {
    let packs = registry
        .retained_revision_locks()
        .map(|revision_lock| {
            let semantic_digest = revision_lock
                .revision_digest()
                .context("validated Pack revision identity could not be reproduced")?;
            let digest = DigestV1::from_bytes(semantic_digest.digest().as_bytes())
                .context("validated Pack digest could not be represented for transfer")?;
            PackIdentityV1::new(
                revision_lock.pack_id.clone(),
                revision_lock.explanatory_version.clone(),
                digest,
            )
            .context("validated Pack identity could not be represented for transfer")
        })
        .collect::<Result<Vec<_>>>()?;
    let identity = DeploymentIdentityV1::new(packs, Vec::new())
        .context("embedded deployment identity was rejected")?;
    let initialized = store
        .initialize_deployment_identity(identity)
        .context("embedded deployment identity conflicts with durable SQLite identity")?;
    Ok(match initialized {
        worldstream_sqlite::SqliteDeploymentIdentityInitializationV1::Initialized => "initialized",
        worldstream_sqlite::SqliteDeploymentIdentityInitializationV1::AlreadyInitialized => {
            "already_initialized"
        }
    })
}

#[derive(Debug, Eq, PartialEq)]
enum TelemetryExporterSelection {
    StructuredLog,
    PlainHttpOtlp(String),
    HttpsOtlp(String),
}

fn select_telemetry_exporter(endpoint: Option<&str>) -> Result<TelemetryExporterSelection> {
    let Some(endpoint) = endpoint else {
        return Ok(TelemetryExporterSelection::StructuredLog);
    };
    let endpoint = telemetry::OtlpEndpointV1::parse(endpoint)
        .map_err(|_| anyhow::anyhow!("telemetry OTLP endpoint was malformed"))?;
    if endpoint.as_str().starts_with("http://") {
        return Ok(TelemetryExporterSelection::PlainHttpOtlp(
            endpoint.as_str().to_owned(),
        ));
    }
    if endpoint.as_str().starts_with("https://") {
        return Ok(TelemetryExporterSelection::HttpsOtlp(
            endpoint.as_str().to_owned(),
        ));
    }
    anyhow::bail!("telemetry OTLP endpoint used an unsupported scheme")
}

fn build_telemetry_exporter(endpoint: Option<&str>) -> Arc<dyn telemetry::TelemetryExporter> {
    let selection = match select_telemetry_exporter(endpoint) {
        Ok(selection) => selection,
        Err(error) => {
            tracing::warn!(
                reason = %error,
                "telemetry exporter configuration was rejected; using bounded structured logs"
            );
            return Arc::new(StructuredLogTelemetryExporter);
        }
    };
    match selection {
        TelemetryExporterSelection::StructuredLog => Arc::new(StructuredLogTelemetryExporter),
        TelemetryExporterSelection::PlainHttpOtlp(endpoint) => {
            match telemetry::OtlpLikeExporter::new(&endpoint, telemetry::StdHttpOtlpTransport) {
                Ok(exporter) => Arc::new(exporter),
                Err(error) => {
                    tracing::warn!(
                        reason = ?error,
                        "telemetry exporter initialization failed; using bounded structured logs"
                    );
                    Arc::new(StructuredLogTelemetryExporter)
                }
            }
        }
        TelemetryExporterSelection::HttpsOtlp(endpoint) => {
            match telemetry::OtlpLikeExporter::new(
                &endpoint,
                telemetry::TlsHttpOtlpTransport::default(),
            ) {
                Ok(exporter) => Arc::new(exporter),
                Err(error) => {
                    tracing::warn!(
                        reason = ?error,
                        "telemetry HTTPS exporter initialization failed; using bounded structured logs"
                    );
                    Arc::new(StructuredLogTelemetryExporter)
                }
            }
        }
    }
}

fn bootstrap_authority<S>(
    store: &S,
    source: Option<&SecretSource>,
) -> Result<AuthorityChangeReceiptV1>
where
    S: AuthorityStoreV1 + Clone + 'static,
{
    let source = source.ok_or_else(|| {
        anyhow::anyhow!(
            "SQLite daemon requires authority.bootstrap.secret_file or authority.bootstrap.secret_handle"
        )
    })?;
    let checked_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .context("authority bootstrap clock formatting failed")?
        .parse::<AuthorityCheckedAt>()
        .context("authority bootstrap clock value was invalid")?;
    bootstrap_authority_from_source_at(store, source, checked_at)
}

fn bootstrap_authority_from_source_at<S>(
    store: &S,
    source: &SecretSource,
    checked_at: AuthorityCheckedAt,
) -> Result<AuthorityChangeReceiptV1>
where
    S: AuthorityStoreV1 + Clone + 'static,
{
    let secret = source
        .read_exact_256()
        .context("authority bootstrap secret material is unavailable or invalid")?;
    bootstrap_authority_at(store, secret, checked_at)
}

fn bootstrap_authority_at<S>(
    store: &S,
    secret: [u8; 32],
    checked_at: AuthorityCheckedAt,
) -> Result<AuthorityChangeReceiptV1>
where
    S: AuthorityStoreV1 + Clone + 'static,
{
    let bearer = CapabilityBearerV1::from_bytes(secret);
    let bootstrap = AuthorityBootstrapV1::new(
        AuthorityChangeId::from_str(BOOTSTRAP_CHANGE_ID)
            .context("authority bootstrap operation identity was invalid")?,
        PrincipalId::from_str(BOOTSTRAP_PRINCIPAL_ID)
            .context("authority bootstrap principal identity was invalid")?,
        PrincipalKindV1::Human,
        CapabilityId::from_str(BOOTSTRAP_CAPABILITY_ID)
            .context("authority bootstrap capability identity was invalid")?,
        bearer.token_hash(),
        None,
    )
    .context("authority bootstrap shape was invalid")?;
    let receipt = AuthorityV1::new(Arc::new(store.clone()))
        .bootstrap(bootstrap, checked_at)
        .context("authority bootstrap transaction was rejected")?;
    if receipt.result() != AuthorityChangeResultV1::AuthorityBootstrapped
        || receipt.resulting_generation() != 1
    {
        anyhow::bail!("authority bootstrap returned an invalid durable receipt");
    }
    Ok(receipt)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

    use tempfile::tempdir;
    use worldstream_runtime::{DeploymentLineageV1, EffectiveConfig, SecretSource, StorageEpochV1};

    use super::{
        BOOTSTRAP_CHANGE_ID, CanonicalMetadataStartup, TelemetryExporterSelection,
        bootstrap_authority, bootstrap_authority_at, build_telemetry_exporter,
        initialize_canonical_metadata, initialize_deployment_identity, run_on_blocking_worker,
        select_telemetry_exporter,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn postgres_startup_worker_allows_sync_runtime_and_scheduler_tick() -> anyhow::Result<()>
    {
        // `postgres::Config::connect` creates and blocks a private Tokio
        // runtime. This is legal on the dedicated startup worker and would
        // panic if the same closure ran on the daemon's Tokio worker.
        run_on_blocking_worker(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async { Ok::<(), anyhow::Error>(()) })
        })
        .await
    }

    #[test]
    fn telemetry_defaults_to_structured_logging() -> anyhow::Result<()> {
        let selection = select_telemetry_exporter(None)?;
        assert_eq!(selection, TelemetryExporterSelection::StructuredLog);
        assert!(
            Arc::strong_count(&build_telemetry_exporter(None)) == 1,
            "default telemetry exporter construction did not return an owned exporter"
        );
        Ok(())
    }

    #[test]
    fn telemetry_selects_validated_plain_http_otlp() -> anyhow::Result<()> {
        let endpoint = "http://127.0.0.1:4318/v1/logs";
        let selection = select_telemetry_exporter(Some(endpoint))?;
        assert_eq!(
            selection,
            TelemetryExporterSelection::PlainHttpOtlp("http://127.0.0.1:4318/v1/logs".to_owned())
        );
        assert!(
            Arc::strong_count(&build_telemetry_exporter(Some(endpoint))) == 1,
            "plain HTTP telemetry exporter construction did not return an owned exporter"
        );
        Ok(())
    }

    #[test]
    fn telemetry_selects_validated_https_otlp() -> anyhow::Result<()> {
        let endpoint = "https://collector.example/v1/logs";
        assert_eq!(
            select_telemetry_exporter(Some(endpoint))?,
            TelemetryExporterSelection::HttpsOtlp(endpoint.to_owned())
        );
        assert_eq!(
            Arc::strong_count(&build_telemetry_exporter(Some(endpoint))),
            1
        );
        assert_eq!(
            Arc::strong_count(&build_telemetry_exporter(Some("not-an-endpoint"))),
            1
        );
        assert!(
            select_telemetry_exporter(Some("https://user:password@collector.example")).is_err()
        );
        assert!(
            select_telemetry_exporter(Some("https://collector.example/v1/logs?token=secret"))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn canonical_metadata_initializes_restarts_exactly_and_conflicts() -> anyhow::Result<()> {
        let directory = tempdir()?;
        let database_path = directory.path().join("worldstream.sqlite3");
        let store = worldstream_sqlite::SqliteRoomStore::open(&database_path)?;
        let mut config = EffectiveConfig::default();
        config.storage.deployment_lineage = Some(DeploymentLineageV1::parse("deployment/one")?);
        config.storage.storage_epoch = Some(StorageEpochV1::new(7)?);

        assert_eq!(
            initialize_canonical_metadata(&store, &config.storage)?,
            CanonicalMetadataStartup::Initialized
        );
        drop(store);

        let restarted = worldstream_sqlite::SqliteRoomStore::open(&database_path)?;
        assert_eq!(
            initialize_canonical_metadata(&restarted, &config.storage)?,
            CanonicalMetadataStartup::AlreadyInitialized
        );

        config.storage.deployment_lineage = Some(DeploymentLineageV1::parse("deployment/two")?);
        let conflict = initialize_canonical_metadata(&restarted, &config.storage)
            .err()
            .ok_or_else(|| anyhow::anyhow!("conflicting metadata unexpectedly succeeded"))?;
        assert!(format!("{conflict:#}").contains("already initialized"));
        Ok(())
    }

    #[test]
    fn canonical_metadata_absence_remains_nonblocking_and_incomplete() -> anyhow::Result<()> {
        let directory = tempdir()?;
        let database_path = directory.path().join("worldstream.sqlite3");
        let store = worldstream_sqlite::SqliteRoomStore::open(&database_path)?;
        let config = EffectiveConfig::default();

        assert_eq!(
            initialize_canonical_metadata(&store, &config.storage)?,
            CanonicalMetadataStartup::Absent
        );
        let registry = worldstream_core::builtin_worldstream_registry()?;
        assert_eq!(
            initialize_deployment_identity(&store, &registry)?,
            "initialized"
        );
        assert_eq!(
            store.begin_source_transfer(directory.path().join("incomplete-transfer.sqlite3")),
            Err(worldstream_sqlite::SqliteSourceTransferErrorV1::SourceEvidenceIncomplete)
        );
        assert_eq!(
            store.export_canonical_evidence(),
            Err(worldstream_sqlite::SqliteCanonicalExportErrorV1::SourceNotTransferPending)
        );
        Ok(())
    }

    #[test]
    fn configured_sqlite_persists_the_complete_embedded_pack_identity() -> anyhow::Result<()> {
        let directory = tempdir()?;
        let database_path = directory.path().join("worldstream.sqlite3");
        let backup_path = directory.path().join("transfer.sqlite3");
        let store = worldstream_sqlite::SqliteRoomStore::open(&database_path)?;
        let mut config = EffectiveConfig::default();
        config.storage.deployment_lineage =
            Some(DeploymentLineageV1::parse("deployment/identity")?);
        config.storage.storage_epoch = Some(StorageEpochV1::new(1)?);
        assert_eq!(
            initialize_canonical_metadata(&store, &config.storage)?,
            CanonicalMetadataStartup::Initialized
        );
        let registry = worldstream_core::builtin_worldstream_registry()?;
        assert_eq!(
            initialize_deployment_identity(&store, &registry)?,
            "initialized"
        );
        assert_eq!(
            initialize_deployment_identity(&store, &registry)?,
            "already_initialized"
        );
        store.begin_source_transfer(&backup_path)?;
        let export = store.export_canonical_evidence()?;
        assert_eq!(export.deployment_identity().packs().len(), registry.len());
        assert!(export.deployment_identity().resources().is_empty());
        Ok(())
    }

    #[test]
    fn authority_bootstrap_is_idempotent_and_durable() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp directory: {error}"));
        let secret_path = directory.path().join("authority.secret");
        fs::write(&secret_path, [0xA9_u8; 32])
            .unwrap_or_else(|error| unreachable!("secret file: {error}"));
        fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("secret permissions: {error}"));
        let source = SecretSource::File(secret_path);
        let database_path = directory.path().join("worldstream.sqlite3");
        let store = worldstream_sqlite::SqliteRoomStore::open(&database_path)
            .unwrap_or_else(|error| unreachable!("SQLite store: {error}"));
        let checked_at = "2026-08-15T12:00:00Z"
            .parse::<worldstream_core::AuthorityCheckedAt>()
            .unwrap_or_else(|error| unreachable!("checked time: {error}"));

        let first = bootstrap_authority_at(&store, [0xA9_u8; 32], checked_at.clone())
            .unwrap_or_else(|error| unreachable!("first bootstrap: {error}"));
        let second = bootstrap_authority_at(&store, [0xA9_u8; 32], checked_at.clone())
            .unwrap_or_else(|error| unreachable!("idempotent bootstrap: {error}"));
        assert_eq!(first, second);
        assert_eq!(first.change_id().as_str(), BOOTSTRAP_CHANGE_ID);

        let _ = super::bootstrap_authority_from_source_at(&store, &source, checked_at)
            .unwrap_or_else(|error| unreachable!("file bootstrap replay: {error}"));
    }

    #[test]
    fn authority_bootstrap_fails_closed_without_or_with_invalid_material() {
        let directory = tempdir().unwrap_or_else(|error| unreachable!("temp directory: {error}"));
        let database_path = directory.path().join("worldstream.sqlite3");
        let store = worldstream_sqlite::SqliteRoomStore::open(&database_path)
            .unwrap_or_else(|error| unreachable!("SQLite store: {error}"));
        let missing = bootstrap_authority(&store, None)
            .err()
            .unwrap_or_else(|| unreachable!("missing bootstrap material must fail"));
        assert!(missing.to_string().contains("requires authority.bootstrap"));

        let secret_path = directory.path().join("short.secret");
        fs::write(&secret_path, [0xA9_u8; 31])
            .unwrap_or_else(|error| unreachable!("short secret file: {error}"));
        fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| unreachable!("secret permissions: {error}"));
        let error = bootstrap_authority(&store, Some(&SecretSource::File(secret_path)))
            .err()
            .unwrap_or_else(|| unreachable!("short bootstrap material must fail"));
        assert!(error.to_string().contains("unavailable or invalid"));
        assert!(!error.to_string().contains("A9"));
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
}
