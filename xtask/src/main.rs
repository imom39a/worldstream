mod compatibility;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "WorldStream repository maintenance")]
struct Xtask {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate or verify the canonical compatibility JSON mirror.
    #[command(name = "compat", visible_alias = "compatibility")]
    Compatibility {
        #[command(subcommand)]
        command: CompatibilityCommand,
    },
}

#[derive(Debug, Subcommand)]
enum CompatibilityCommand {
    /// Deterministically regenerate compatibility.json from compatibility.toml.
    Generate,
    /// Fail when syntax, semantic parity, sorting, or toolchain pins drift.
    Verify,
}

fn main() -> Result<()> {
    let task = Xtask::parse();
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    match task.command {
        Command::Compatibility {
            command: CompatibilityCommand::Generate,
        } => compatibility::generate(&repository_root),
        Command::Compatibility {
            command: CompatibilityCommand::Verify,
        } => compatibility::verify(&repository_root),
    }
}
