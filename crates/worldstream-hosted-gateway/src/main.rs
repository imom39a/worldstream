use std::{
    collections::BTreeSet, env, error::Error, future::IntoFuture as _, io, net::SocketAddr,
    time::Duration,
};

use worldstream_hosted_gateway::{
    HostedGatewayConfig, UnavailableHostedGatewayBackend, hosted_gateway_router,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init()?;

    let bind = env::var("HOSTED_GATEWAY_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_owned())
        .parse::<SocketAddr>()?;
    let upstream = required("WORLDSTREAM_RUNTIME_UPSTREAM")?.parse::<SocketAddr>()?;
    let public_authority = required("WORLDSTREAM_PUBLIC_AUTHORITY")?;
    let service_authority = required("WORLDSTREAM_VERCEL_SERVICE_AUTHORITY")?;
    let listings = required("WORLDSTREAM_LISTING_ALLOWLIST")?
        .split(',')
        .map(str::trim)
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let deployment = env::var("WORLDSTREAM_DEPLOYMENT_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    let config = HostedGatewayConfig::new(
        deployment,
        &service_authority,
        listings,
        public_authority,
        upstream,
        120,
        Duration::from_mins(1),
    )?;
    drop(service_authority);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
    let server = axum::serve(
        listener,
        hosted_gateway_router(config, UnavailableHostedGatewayBackend),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown_receiver.await;
    })
    .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result?,
        signal = shutdown_signal() => {
            signal?;
            let _ = shutdown_sender.send(());
            tokio::time::timeout(Duration::from_mins(2), &mut server)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "gateway shutdown exceeded 120 seconds"))??;
        }
    }
    Ok(())
}

fn required(name: &str) -> Result<String, io::Error> {
    env::var(name)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, format!("{name} is required")))
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
