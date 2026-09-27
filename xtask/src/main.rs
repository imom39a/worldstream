mod compatibility;
mod counter_dependency_closure;

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
    /// Generate or verify Counter's scoped normal/build dependency identity.
    #[command(name = "counter-deps")]
    CounterDependencies {
        #[command(subcommand)]
        command: CounterDependenciesCommand,
    },
}

#[derive(Debug, Subcommand)]
enum CompatibilityCommand {
    /// Deterministically regenerate compatibility.json from compatibility.toml.
    Generate,
    /// Fail when syntax, semantic parity, sorting, or toolchain pins drift.
    Verify,
}

#[derive(Debug, Subcommand)]
enum CounterDependenciesCommand {
    /// Regenerate Counter v4's checked-in normal/build dependency closure.
    Generate,
    /// Verify Counter v1/v2/v3's frozen closures and Counter v4's resolved closure.
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
        Command::CounterDependencies {
            command: CounterDependenciesCommand::Generate,
        } => counter_dependency_closure::generate(&repository_root),
        Command::CounterDependencies {
            command: CounterDependenciesCommand::Verify,
        } => counter_dependency_closure::verify(&repository_root),
    }
}
