use std::{
    fs,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use worldstream_postgres::{PostgresAdmin, PostgresConnectionConfig};
use worldstream_runtime::{
    SecretSource, embedded_manifest, prepare_data_directory, validate_owner_only_file,
    validate_sqlite_data_filesystem,
};
use worldstream_server::CommonConfigArgs;

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
        command: PostgresCommand,
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

fn main() -> Result<()> {
    let Cli {
        config: config_args,
        command,
    } = Cli::parse();

    match command {
        Command::Postgres { command } => run_postgres_admin(command),
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
            write_json(&manifest.summary())
        }
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
    };
    write_json(&PostgresAdminResult {
        status: "ok",
        operation,
        connection: "direct-admin",
        release_evidence: false,
    })
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
    use super::validate_health_response;

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
}
