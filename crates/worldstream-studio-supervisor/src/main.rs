use std::{net::SocketAddr, time::Duration};

use anyhow::{Context as _, Result};
use clap::Parser;
use worldstream_studio_supervisor::{HttpDaemonStatusSource, supervisor_router};

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("Studio Supervisor listener bind failed at {}", args.bind))?;
    let source =
        HttpDaemonStatusSource::new(args.daemon, Duration::from_millis(args.probe_timeout_ms));
    axum::serve(listener, supervisor_router(source))
        .await
        .context("Studio Supervisor server failed")
}
