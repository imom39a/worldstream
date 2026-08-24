use std::io;

use anyhow::{Context as _, Result};
use clap::Parser as _;
use worldstream_studio_supervisor::assignment_mcp::{
    AssignmentMcpCliV1, open_registered_assignment_mcp, run_assignment_mcp_stdio,
};

fn main() -> Result<()> {
    let args = AssignmentMcpCliV1::parse();
    let mut server = open_registered_assignment_mcp(args.state_dir(), args.launch_reference())
        .context("assignment MCP launch registration is unavailable")?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_assignment_mcp_stdio(&mut server, &mut stdin.lock(), &mut stdout.lock())
        .context("assignment MCP stdio transport failed")
}
