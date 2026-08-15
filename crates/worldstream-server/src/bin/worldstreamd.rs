use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;
use worldstream_runtime::prepare_data_directory;
use worldstream_server::{CommonConfigArgs, OperatorState, operator_router};

#[derive(Debug, Parser)]
#[command(name = "worldstreamd", version, about = "WorldStream process shell")]
struct DaemonArgs {
    #[command(flatten)]
    config: CommonConfigArgs,
}

#[tokio::main]
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
    let state = OperatorState::new(config).context("operator state initialization failed")?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("listener bind failed at {bind}"))?;

    info!(
        listen_address = %bind,
        storage_profile = %profile,
        data_directory = %data_dir.display(),
        "WorldStream operator shell started; storage remains uninitialized"
    );

    axum::serve(listener, operator_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("operator HTTP server failed")
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
}
