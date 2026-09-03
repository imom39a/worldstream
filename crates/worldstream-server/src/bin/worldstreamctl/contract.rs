//! Additive operator CLI contract. No process or storage engines live here.
//! Kept with the CLI binary; these argument types are not Runtime interfaces.

use super::cli_reference::PublicReference;
use clap::{ArgGroup, Args, Subcommand};
use std::{ffi::OsString, io::IsTerminal, net::SocketAddr, path::PathBuf};

/// Classify the actual command position, never a legacy option's value.
/// This is used only to redact parser failures, not to parse or dispatch commands.
#[must_use]
pub fn operator_family(arguments: &[OsString]) -> Option<&'static str> {
    let mut values = arguments.iter().skip(1);
    match next_command_token(&mut values)? {
        "init" => Some("init"),
        "server" => Some("server"),
        "room" => Some("room"),
        "runner" => Some("runner"),
        "client" => Some("client"),
        "pack" => (next_command_token(&mut values)? == "list").then_some("pack list"),
        _ => None,
    }
}

fn next_command_token<'a>(values: &mut impl Iterator<Item = &'a OsString>) -> Option<&'a str> {
    const GLOBAL_VALUE_OPTIONS: [&str; 4] =
        ["--config", "--bind", "--storage-profile", "--data-dir"];
    while let Some(value) = values.next() {
        let value = value.to_str()?;
        if GLOBAL_VALUE_OPTIONS.contains(&value) {
            values.next()?;
            continue;
        }
        if value
            .split_once('=')
            .is_some_and(|(name, _)| GLOBAL_VALUE_OPTIONS.contains(&name))
        {
            continue;
        }
        return Some(value);
    }
    None
}

/// Output selection for new commands only; legacy output remains unchanged.
#[derive(Debug, Args)]
pub struct CommandOptions {
    /// Emit one stable JSON document on stdout; diagnostics remain on stderr.
    #[arg(long)]
    pub json: bool,
    /// Owner-only controller state (independent of the Runtime data directory).
    #[arg(long, default_value = ".worldstream/studio", value_parser = local_path)]
    pub state_dir: PathBuf,
    /// Literal loopback control address; --bind still selects the Runtime listener.
    #[arg(long, default_value = "127.0.0.1:9420", value_parser = local_controller)]
    pub controller: SocketAddr,
    /// Bounded wait for the requested operation; a timeout is not success.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u16).range(1..=300))]
    pub timeout_seconds: u16,
}

/// New operator command families; existing commands remain in their original parser.
#[derive(Debug, Subcommand)]
pub enum OperatorCommand {
    /// Explicit protected initialization or reviewed prerequisite import; starts no processes.
    Init(InitArgs),
    /// Manage the configured local installation.
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    /// Create and operate Rooms through bounded setup operations.
    Room {
        #[command(subcommand)]
        command: RoomCommand,
    },
    /// Manage Runner seats by public setup operation and seat labels.
    Runner {
        #[command(subcommand)]
        command: RunnerCommand,
    },
    /// Open an approved Activity Client or explicitly export participant credentials.
    Client {
        #[command(subcommand)]
        command: ClientCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum RoomCommand {
    /// Generate a setup specification from an installed immutable Pack revision.
    Example(ExampleArgs),
    /// Validate a setup file without creating a Room or starting processes.
    Validate(FileArgs),
    /// Persist a setup operation and ask the Runtime to create its Room.
    Create(CreateArgs),
    /// List Rooms without creating or launching anything.
    List(CommandOptions),
    /// Inspect a Room by its public identity.
    Inspect(RoomArgs),
    /// Explicitly request the Activity Pack's declared lobby-to-active launch.
    Launch(RoomArgs),
    /// Inspect or resume a persisted setup operation, never replace its inputs.
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
    },
}

#[derive(Debug, Args)]
pub struct ExampleArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    /// Literal installed Pack name and version, NAME@VERSION; not a version range.
    #[arg(long, value_name = "NAME@VERSION", value_parser = pack_selector)]
    pub pack: String,
    /// New local specification file; existing files are never overwritten.
    #[arg(long, value_name = "FILE", value_parser = local_path)]
    pub output: PathBuf,
    /// Explicit terminal-only convenience prompts; never selected automatically.
    #[arg(long, conflicts_with = "json")]
    pub interactive: bool,
}

#[derive(Debug, Args)]
pub struct FileArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    #[arg(long, value_name = "FILE", value_parser = local_path)]
    pub file: PathBuf,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    #[command(flatten)]
    pub source: FileArgs,
    /// Explicitly acknowledge an Activity whose execution starts at Genesis.
    #[arg(long)]
    pub acknowledge_start: bool,
}

#[derive(Debug, Args)]
pub struct RoomArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    #[arg(value_name = "ROOM", value_parser = reference)]
    pub room: String,
}

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// Inspect an operation, or list unfinished operations when omitted.
    Status(OperationListArgs),
    /// Resume only the persisted specification for this operation.
    Resume(OperationArgs),
}

#[derive(Debug, Args)]
pub struct OperationListArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    #[arg(value_name = "OPERATION", value_parser = reference)]
    pub operation: Option<String>,
}

#[derive(Debug, Args)]
pub struct OperationArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    #[arg(value_name = "OPERATION", value_parser = reference)]
    pub operation: String,
}

#[derive(Debug, Subcommand)]
pub enum RunnerCommand {
    List(RunnerListArgs),
    Inspect(SeatArgs),
    Start(SeatArgs),
    Stop(SeatArgs),
    ExportCredentials(ExportArgs),
}

#[derive(Debug, Args)]
pub struct RunnerListArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    #[arg(long, value_parser = reference)]
    pub operation: Option<String>,
}

#[derive(Debug, Args)]
pub struct SeatArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    /// Public persisted Room Setup Operation identity.
    #[arg(long, value_parser = reference)]
    pub operation: String,
    /// Stable seat label from that operation's specification.
    #[arg(long, value_parser = reference)]
    pub seat: String,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    #[command(flatten)]
    pub seat: SeatArgs,
    /// Explicit owner-only output file; credentials are never printed.
    #[arg(long, value_name = "FILE", value_parser = local_path)]
    pub output: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum ClientCommand {
    Open(ClientOpenArgs),
    ExportCredentials(ExportArgs),
}

#[derive(Debug, Args)]
pub struct ClientOpenArgs {
    #[command(flatten)]
    pub seat: SeatArgs,
    /// Exact approved local client binding; omission uses the operation's binding.
    #[arg(long, value_parser = reference)]
    pub binding: Option<String>,
}

/// Exact-input prerequisite review is separate from Room setup and process startup.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("imports").args(["runner_template", "provider_declaration", "client_declaration", "agent_profile"]).multiple(true)))]
#[command(group(ArgGroup::new("import_review").args(["preview", "approve_imports"])))]
pub struct InitArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    /// Nonmutating preview of protected initialization and exact declaration identities.
    #[arg(long)]
    pub preview: bool,
    /// Exact aggregate BLAKE3 digest from preview, never a blanket approval.
    #[arg(long, value_name = "DIGEST", requires = "imports", value_parser = exact_digest)]
    pub approve_imports: Option<String>,
    /// Owner-controlled Runner Template declaration (repeat at most 16 times).
    #[arg(long, value_name = "FILE", requires = "import_review", value_parser = local_path)]
    pub runner_template: Vec<PathBuf>,
    /// Non-secret named provider declaration (repeat at most 16 times).
    #[arg(long, value_name = "FILE", requires = "import_review", value_parser = local_path)]
    pub provider_declaration: Vec<PathBuf>,
    /// Exact Activity Client declarations (repeat at most 16 times).
    #[arg(long, value_name = "FILE", requires = "import_review", value_parser = local_path)]
    pub client_declaration: Vec<PathBuf>,
    /// Immutable Agent Profile declaration (repeat at most 16 times).
    #[arg(long, value_name = "FILE", requires = "import_review", value_parser = local_path)]
    pub agent_profile: Vec<PathBuf>,
}

/// Explicit managed Runtime operations.
#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Report controller, ownership, liveness, and readiness without starting processes.
    Status(CommandOptions),
    /// Explicitly start or reuse the controller and its configured Runtime.
    Start(CommandOptions),
    /// Stop owned managed Runners, then the Runtime; leave the controller running.
    Stop(CommandOptions),
    /// Restart the Runtime and restore only previously running eligible managed Runners.
    Restart(CommandOptions),
    /// Stop only the controller, leaving the Runtime running.
    ControllerStop(CommandOptions),
    /// Rotate only the owner-local control credential.
    RotateControlCredential(CommandOptions),
    /// Read bounded secret-safe logs without starting processes.
    Logs(LogArgs),
}

/// Bounded log selection; following is not part of this command contract.
#[derive(Debug, Args)]
pub struct LogArgs {
    #[command(flatten)]
    pub options: CommandOptions,
    /// Maximum recent log entries to return.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
    pub tail: u16,
}

impl OperatorCommand {
    /// Cross-occurrence bounds and explicit interactive terminal requirements.
    #[must_use]
    pub fn valid_arguments(&self) -> bool {
        match self {
            Self::Init(args) => [
                args.runner_template.len(),
                args.provider_declaration.len(),
                args.client_declaration.len(),
                args.agent_profile.len(),
            ]
            .into_iter()
            .all(|count| count <= 16),
            Self::Room {
                command: RoomCommand::Example(args),
            } => !args.interactive || std::io::stdin().is_terminal(),
            Self::Server { .. } | Self::Room { .. } | Self::Runner { .. } | Self::Client { .. } => {
                true
            }
        }
    }
    /// Returns the stable operation label and requested output mode.
    #[must_use]
    pub fn invocation(&self) -> (&'static str, bool) {
        match self {
            Self::Init(args) => ("init", args.options.json),
            Self::Room { command } => match command {
                RoomCommand::Example(args) => ("room example", args.options.json),
                RoomCommand::Validate(args) => ("room validate", args.options.json),
                RoomCommand::Create(args) => ("room create", args.source.options.json),
                RoomCommand::List(options) => ("room list", options.json),
                RoomCommand::Inspect(args) => ("room inspect", args.options.json),
                RoomCommand::Launch(args) => ("room launch", args.options.json),
                RoomCommand::Setup {
                    command: SetupCommand::Status(args),
                } => ("room setup status", args.options.json),
                RoomCommand::Setup {
                    command: SetupCommand::Resume(args),
                } => ("room setup resume", args.options.json),
            },
            Self::Runner { command } => match command {
                RunnerCommand::List(args) => ("runner list", args.options.json),
                RunnerCommand::Inspect(args) => ("runner inspect", args.options.json),
                RunnerCommand::Start(args) => ("runner start", args.options.json),
                RunnerCommand::Stop(args) => ("runner stop", args.options.json),
                RunnerCommand::ExportCredentials(args) => {
                    ("runner export-credentials", args.seat.options.json)
                }
            },
            Self::Client { command } => match command {
                ClientCommand::Open(args) => ("client open", args.seat.options.json),
                ClientCommand::ExportCredentials(args) => {
                    ("client export-credentials", args.seat.options.json)
                }
            },
            Self::Server { command } => match command {
                ServerCommand::Status(options) => ("server status", options.json),
                ServerCommand::Start(options) => ("server start", options.json),
                ServerCommand::Stop(options) => ("server stop", options.json),
                ServerCommand::Restart(options) => ("server restart", options.json),
                ServerCommand::ControllerStop(options) => ("server controller-stop", options.json),
                ServerCommand::RotateControlCredential(options) => {
                    ("server rotate-control-credential", options.json)
                }
                ServerCommand::Logs(args) => ("server logs", args.options.json),
            },
        }
    }
}

fn reference(value: &str) -> Result<String, &'static str> {
    PublicReference::parse(value).map(PublicReference::into_string)
}

fn pack_selector(value: &str) -> Result<String, &'static str> {
    if value.len() > 191 {
        return Err("expected a bounded NAME@VERSION selector");
    }
    let Some((name, version)) = value.split_once('@') else {
        return Err("expected NAME@VERSION from installed Pack inventory");
    };
    reference(name)?;
    let components: Vec<_> = version.split('.').collect();
    if components.len() != 3
        || components.iter().any(|part| {
            part.is_empty()
                || part.len() > 20
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
                || part.parse::<u64>().is_err()
        })
    {
        return Err("expected an exact numeric X.Y.Z version, not a moving tag or range");
    }
    Ok(value.to_owned())
}

fn exact_digest(value: &str) -> Result<String, &'static str> {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return Err("expected an exact BLAKE3 digest from preview");
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("expected an exact BLAKE3 digest from preview");
    }
    Ok(value.to_owned())
}

fn local_controller(value: &str) -> Result<SocketAddr, &'static str> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| "expected a literal loopback address")?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err("expected a literal loopback address with a nonzero port");
    }
    Ok(address)
}

fn local_path(value: &str) -> Result<PathBuf, &'static str> {
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("expected a bounded local path without control characters");
    }
    Ok(PathBuf::from(value))
}
