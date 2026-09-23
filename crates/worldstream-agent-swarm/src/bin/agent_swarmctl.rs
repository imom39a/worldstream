//! Agent Swarm execution daemon control client.

use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use worldstream_agent_swarm::execution::{
    ControlCommand, EffectIntent, EffectOutcome, ExecutionControlClient, InvocationResolution,
    InvocationTicket, PreparedInvocation, ProviderKind, RunBudget,
};

#[derive(Parser)]
#[command(name = "worldstream-agent-swarmctl")]
struct Arguments {
    /// Existing owner-only execution state directory.
    #[arg(long)]
    state: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Status {
        #[arg(long)]
        swarm_id: Option<String>,
    },
    Pause {
        swarm_id: String,
    },
    Resume {
        swarm_id: String,
    },
    Stop {
        swarm_id: String,
    },
    /// Interrupt one exact active Invocation without pausing its Swarm.
    CancelInvocation {
        swarm_id: String,
        invocation_id: String,
    },
    /// Register stopped; omitted roster counts grant no automatic capacity.
    Register {
        swarm_id: String,
        #[arg(long, default_value_t = 1)]
        priority: u8,
        #[arg(long)]
        invocation_limit: Option<u64>,
        #[arg(long)]
        active_time_limit_ms: Option<u64>,
        /// Selected Codex members in the authoritative roster.
        #[arg(long)]
        codex_count: Option<u16>,
        /// Selected Claude members in the authoritative roster.
        #[arg(long)]
        claude_count: Option<u16>,
        /// Selected Kiro members in the authoritative roster.
        #[arg(long)]
        kiro_count: Option<u16>,
        /// Controlled members used by native qualification/tests.
        #[arg(long)]
        controlled_count: Option<u16>,
    },
    SetProviderCap {
        provider: String,
        limit: u16,
    },
    SetPriority {
        swarm_id: String,
        priority: u8,
    },
    SetBudget {
        swarm_id: String,
        #[arg(long)]
        invocation_limit: Option<u64>,
        #[arg(long)]
        active_time_limit_ms: Option<u64>,
    },
    Enqueue {
        #[arg(long)]
        ticket: PathBuf,
    },
    Tick,
    Launch {
        #[arg(long)]
        ticket: PathBuf,
        #[arg(long)]
        prepared: PathBuf,
    },
    Collect {
        invocation_id: String,
    },
    AcknowledgeCompletion {
        invocation_id: String,
    },
    /// Persist an exact write-ahead external-effect intent from JSON.
    PrepareEffect {
        #[arg(long)]
        intent: PathBuf,
    },
    /// Persist the last cut before contacting the external target.
    MarkEffectDispatched {
        operation_id: String,
    },
    /// Record the target's exact idempotency observation.
    ObserveEffect {
        operation_id: String,
        outcome: String,
    },
    /// Acknowledge a terminal retained target observation.
    AcknowledgeEffect {
        operation_id: String,
    },
    ResolveInvocation {
        swarm_id: String,
        invocation_id: String,
        resolution: String,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "keep the complete one-to-one CLI-to-control-command dispatch visible in one place"
)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = Arguments::parse();
    let command = match arguments.command {
        Command::Status { swarm_id } => ControlCommand::Status { swarm_id },
        Command::Pause { swarm_id } => ControlCommand::Pause { swarm_id },
        Command::Resume { swarm_id } => ControlCommand::Resume { swarm_id },
        Command::Stop { swarm_id } => ControlCommand::Stop { swarm_id },
        Command::CancelInvocation {
            swarm_id,
            invocation_id,
        } => ControlCommand::CancelInvocation {
            swarm_id,
            invocation_id,
        },
        Command::Register {
            swarm_id,
            priority,
            invocation_limit,
            active_time_limit_ms,
            codex_count,
            claude_count,
            kiro_count,
            controlled_count,
        } => ControlCommand::RegisterSwarm {
            swarm_id,
            priority,
            budget: RunBudget {
                invocation_limit,
                active_time_limit_ms,
            },
            roster_provider_counts: [
                (ProviderKind::Codex, codex_count),
                (ProviderKind::Claude, claude_count),
                (ProviderKind::Kiro, kiro_count),
                (ProviderKind::Controlled, controlled_count),
            ]
            .into_iter()
            .filter_map(|(provider, count)| count.map(|count| (provider, count)))
            .collect::<BTreeMap<_, _>>(),
        },
        Command::SetProviderCap { provider, limit } => ControlCommand::SetProviderCap {
            provider: parse_provider(&provider)?,
            limit,
        },
        Command::SetPriority { swarm_id, priority } => {
            ControlCommand::SetPriority { swarm_id, priority }
        }
        Command::SetBudget {
            swarm_id,
            invocation_limit,
            active_time_limit_ms,
        } => ControlCommand::SetBudget {
            swarm_id,
            budget: RunBudget {
                invocation_limit,
                active_time_limit_ms,
            },
        },
        Command::Enqueue { ticket } => {
            let ticket: InvocationTicket = read_json(&ticket)?;
            ControlCommand::Enqueue { ticket }
        }
        Command::Tick => ControlCommand::Tick,
        Command::Launch { ticket, prepared } => {
            let ticket: InvocationTicket = read_json(&ticket)?;
            let prepared: PreparedInvocation = read_json(&prepared)?;
            ControlCommand::Launch {
                ticket,
                prepared: Box::new(prepared),
            }
        }
        Command::Collect { invocation_id } => ControlCommand::Collect { invocation_id },
        Command::AcknowledgeCompletion { invocation_id } => {
            ControlCommand::AcknowledgeCompletion { invocation_id }
        }
        Command::PrepareEffect { intent } => ControlCommand::PrepareEffect {
            intent: read_json::<EffectIntent>(&intent)?,
        },
        Command::MarkEffectDispatched { operation_id } => {
            ControlCommand::MarkEffectDispatched { operation_id }
        }
        Command::ObserveEffect {
            operation_id,
            outcome,
        } => ControlCommand::ObserveEffect {
            operation_id,
            outcome: parse_effect_outcome(&outcome)?,
        },
        Command::AcknowledgeEffect { operation_id } => {
            ControlCommand::AcknowledgeEffect { operation_id }
        }
        Command::ResolveInvocation {
            swarm_id,
            invocation_id,
            resolution,
        } => ControlCommand::ResolveInvocation {
            swarm_id,
            invocation_id,
            resolution: parse_resolution(&resolution)?,
        },
    };
    let client = ExecutionControlClient::open(&arguments.state, Duration::from_secs(5))?;
    let result = client.request(command)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(
    path: &PathBuf,
) -> Result<T, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn parse_provider(value: &str) -> Result<ProviderKind, Box<dyn std::error::Error>> {
    ProviderKind::from_roster_name(value)
        .ok_or_else(|| "provider must be codex, claude, kiro, or controlled".into())
}

fn parse_resolution(value: &str) -> Result<InvocationResolution, Box<dyn std::error::Error>> {
    match value {
        "completed" => Ok(InvocationResolution::Completed),
        "failed" => Ok(InvocationResolution::Failed),
        "terminated" => Ok(InvocationResolution::Terminated),
        "unknown" => Ok(InvocationResolution::Unknown),
        _ => Err("resolution must be completed, failed, terminated, or unknown".into()),
    }
}

fn parse_effect_outcome(value: &str) -> Result<EffectOutcome, Box<dyn std::error::Error>> {
    match value {
        "applied" => Ok(EffectOutcome::Applied),
        "duplicate" => Ok(EffectOutcome::Duplicate),
        "not-applied" => Ok(EffectOutcome::NotApplied),
        "rejected" => Ok(EffectOutcome::Rejected),
        "unknown" => Ok(EffectOutcome::Unknown),
        "target-mismatch" => Ok(EffectOutcome::TargetMismatch),
        _ => Err(
            "outcome must be applied, duplicate, not-applied, rejected, unknown, or target-mismatch"
                .into(),
        ),
    }
}
