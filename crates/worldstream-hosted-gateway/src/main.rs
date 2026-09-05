use std::{
    collections::BTreeSet, env, error::Error, future::IntoFuture as _, io, net::SocketAddr,
    time::Duration,
};

use worldstream_hosted_gateway::{
    DEVELOPMENT_GATEWAY_BACKEND_MODE, HostedGatewayBackend, HostedGatewayBackendMode,
    HostedGatewayConfig, HostedGatewayError, HostedServiceRequestV1, hosted_gateway_router,
    select_hosted_gateway_backend_mode,
};

#[derive(Clone, Copy, Debug)]
enum ProcessBackend {
    Unavailable,
    DevelopmentSubstitute,
}

impl HostedGatewayBackend for ProcessBackend {
    fn ready(&self) -> bool {
        matches!(self, Self::DevelopmentSubstitute)
    }

    fn launch(&self, _request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError> {
        match self {
            Self::Unavailable => Err(HostedGatewayError::Unavailable),
            Self::DevelopmentSubstitute => Ok(()),
        }
    }

    fn evidence(&self, request: &HostedServiceRequestV1) -> Result<(), HostedGatewayError> {
        self.launch(request)
    }
}

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
    let backend_mode = select_hosted_gateway_backend_mode(
        env::var("WORLDSTREAM_DEVELOPMENT_GATEWAY_BACKEND")
            .ok()
            .as_deref(),
        env::var("WORLDSTREAM_DEPLOYMENT_ENVIRONMENT")
            .ok()
            .as_deref(),
        bind,
        !cfg!(debug_assertions),
    )?;
    let backend = match backend_mode {
        HostedGatewayBackendMode::Unavailable => ProcessBackend::Unavailable,
        HostedGatewayBackendMode::DevelopmentSubstitute => {
            tracing::warn!(
                target: "worldstream.hosted_gateway",
                mode = DEVELOPMENT_GATEWAY_BACKEND_MODE,
                "VISIBLE DEVELOPMENT GATEWAY BACKEND ENABLED"
            );
            ProcessBackend::DevelopmentSubstitute
        }
    };
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
    let server = axum::serve(listener, hosted_gateway_router(config, backend))
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
