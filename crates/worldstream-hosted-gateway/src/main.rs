use std::{
    collections::BTreeSet, env, error::Error, fs, future::IntoFuture as _, io, net::SocketAddr,
    path::Path, time::Duration,
};

use worldstream_hosted_gateway::{
    FixedHostAdapterBackend, HostedGatewayConfig, hosted_gateway_router,
};
use zeroize::Zeroizing;

const HOST_ADAPTER_OPERATION_TIMEOUT: Duration = Duration::from_secs(25);

#[test]
fn host_adapter_budget_contains_two_bounded_runtime_result_reads() {
    assert!(HOST_ADAPTER_OPERATION_TIMEOUT > Duration::from_secs(20));
    assert!(HOST_ADAPTER_OPERATION_TIMEOUT <= Duration::from_secs(30));
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
    let upstream = required("WORLDSTREAM_HOST_ADAPTER_UPSTREAM")?.parse::<SocketAddr>()?;
    let runtime_upstream = required("WORLDSTREAM_RUNTIME_UPSTREAM")?.parse::<SocketAddr>()?;
    let controller_authority = required_secret("WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY")?;
    let backend = FixedHostAdapterBackend::new(
        upstream,
        controller_authority.to_string(),
        // A result read performs two sequential bounded Runtime operations:
        // current Projection and Replay. Leave room for both and decoding.
        HOST_ADAPTER_OPERATION_TIMEOUT,
    )?;
    let public_authority = required("WORLDSTREAM_PUBLIC_AUTHORITY")?;
    let service_authority = required_secret("WORLDSTREAM_VERCEL_SERVICE_AUTHORITY")?;
    let listings = required("WORLDSTREAM_LISTING_ALLOWLIST")?
        .split(',')
        .map(str::trim)
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let deployment = env::var("WORLDSTREAM_DEPLOYMENT_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned());
    let config = HostedGatewayConfig::new(
        deployment,
        service_authority.as_str(),
        listings,
        public_authority,
        upstream,
        120,
        Duration::from_mins(1),
    )?
    .with_browser_stream(
        runtime_upstream,
        required("WORLDSTREAM_HOSTED_CLIENT_ORIGIN")?,
    )?;
    drop(service_authority);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
    let server = axum::serve(
        listener,
        hosted_gateway_router(config, backend).into_make_service_with_connect_info::<SocketAddr>(),
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

fn required_secret(name: &str) -> Result<Zeroizing<String>, io::Error> {
    let direct = env::var(name).ok();
    let file_name = format!("{name}_FILE");
    let file = env::var(&file_name).ok();
    if direct.is_some() == file.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("exactly one of {name} or {file_name} is required"),
        ));
    }
    let value = match (direct, file) {
        (Some(value), None) => value,
        (None, Some(path)) => read_owner_only_secret(Path::new(&path))?,
        _ => unreachable!("exclusive secret source checked above"),
    };
    if value.len() < 32 || value.len() > 512 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{name} is invalid"),
        ));
    }
    Ok(Zeroizing::new(value))
}

fn read_owner_only_secret(path: &Path) -> Result<String, io::Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 512 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "secret file is invalid",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "secret file permissions are invalid",
            ));
        }
    }
    fs::read_to_string(path)
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
