//! Long-lived Agent Swarm execution daemon.

use std::path::PathBuf;

use clap::Parser;
use worldstream_agent_swarm::execution::ExecutionDaemon;

#[derive(Parser)]
#[command(name = "worldstream-agent-swarmd")]
struct Arguments {
    /// Owner-only execution state directory.
    #[arg(long)]
    state: PathBuf,
    /// Fixed companion process-guard executable.
    #[arg(long)]
    process_guard: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let mut daemon = ExecutionDaemon::bind_with_guard(&arguments.state, &arguments.process_guard)?;
    println!("{}", serde_json::to_string(&daemon.publication())?);
    daemon.run()?;
    Ok(())
}
