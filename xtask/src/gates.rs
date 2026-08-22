use std::process::Command;

use anyhow::{Context, Result, bail};
use std::path::Path;

pub fn run(
    repository_root: &Path,
    tier: &str,
    cell: Option<&str>,
    strict: bool,
    ci: bool,
    offline: bool,
) -> Result<()> {
    let python = if command_available("python3") {
        "python3"
    } else if command_available("python") {
        "python"
    } else {
        bail!("gates require Python 3.11+ (python3/python is unavailable)");
    };
    let mut command = Command::new(python);
    command
        .current_dir(repository_root)
        .arg("scripts/gates.py")
        .arg(tier);
    if let Some(cell) = cell {
        command.args(["--cell", cell]);
    }
    if strict {
        command.arg("--strict");
    }
    if ci {
        command.arg("--ci");
    }
    if offline {
        command.arg("--offline");
    }
    let status = command
        .status()
        .with_context(|| format!("failed to start {python} scripts/gates.py"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("compatibility gate tier {tier} failed with {status}")
    }
}

fn command_available(command: &str) -> bool {
    if cfg!(windows) {
        Command::new("where")
            .arg(command)
            .status()
            .is_ok_and(|status| status.success())
    } else {
        Command::new("sh")
            .args(["-c", &format!("command -v {command} >/dev/null 2>&1")])
            .status()
            .is_ok_and(|status| status.success())
    }
}
