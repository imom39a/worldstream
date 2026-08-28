mod compatibility;
mod counter_dependency_closure;
mod gates;

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
    /// Run the manifest-driven compatibility and supply-chain gates.
    Gates {
        /// Gate tier: fast, pre-push, minimal-ci, release, or provider-smoke.
        #[arg(default_value = "fast")]
        tier: String,
        /// Execute the selected native CI cell.
        #[arg(long)]
        cell: Option<String>,
        /// Treat incomplete external checks as failures.
        #[arg(long)]
        strict: bool,
        /// Mark this invocation as CI (also makes skips fail closed).
        #[arg(long)]
        ci: bool,
        /// Keep dependency resolution offline.
        #[arg(long)]
        offline: bool,
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
    /// Regenerate Counter v3's checked-in normal/build dependency closure.
    Generate,
    /// Verify Counter v1/v2's frozen closure and Counter v3's resolved closure.
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
        Command::Gates {
            tier,
            cell,
            strict,
            ci,
            offline,
        } => gates::run(
            &repository_root,
            &tier,
            cell.as_deref(),
            strict,
            ci,
            offline,
        ),
    }
}
