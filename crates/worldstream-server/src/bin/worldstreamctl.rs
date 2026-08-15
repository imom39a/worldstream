use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use worldstream_runtime::{embedded_manifest, prepare_data_directory};
use worldstream_server::CommonConfigArgs;

#[derive(Debug, Parser)]
#[command(name = "worldstreamctl", version, about = "WorldStream operator CLI")]
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
    /// Print the embedded compatibility summary.
    Version,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Strictly parse, layer, and validate configuration.
    Validate,
    /// Print effective configuration with every secret reference redacted.
    Effective,
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
    let cli = Cli::parse();
    let config = cli.config.load().context("configuration rejected")?;

    match cli.command {
        Command::Config {
            command: ConfigCommand::Validate,
        } => write_json(&Status {
            status: "valid",
            storage_profile: config.storage.profile.as_str(),
        }),
        Command::Config {
            command: ConfigCommand::Effective,
        } => write_json(&config.redacted()),
        Command::Doctor => {
            let _data_dir = prepare_data_directory(&config.storage.data_dir)
                .context("data-directory validation failed")?;
            let _manifest = embedded_manifest().context("embedded manifest rejected")?;
            write_json(&Doctor {
                status: "incomplete",
                config: "valid",
                manifest: "valid_specification",
                data_directory: "owner_only",
                storage: "not_initialized",
            })
        }
        Command::Version => {
            let manifest = embedded_manifest().context("embedded manifest rejected")?;
            write_json(&manifest.summary())
        }
    }
}

fn write_json(value: &impl Serialize) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, value).context("JSON output failed")?;
    writeln!(output).context("JSON output failed")?;
    Ok(())
}
