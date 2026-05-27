use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::pipeline::run::CheckMode;

mod apply;
mod check;
mod diff;
mod init;
mod schema;

#[derive(Parser, Debug)]
#[command(name = "inclean", version, about = "C/C++ #include path normalizer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a starter inclean.toml at the given path (alias of `config new`).
    Init(InitArgs),
    /// Read-only check.
    Check(CheckArgs),
    /// Apply rewrites in place.
    Apply(ApplyArgs),
    /// Show a unified diff of would-be rewrites.
    Diff(DiffArgs),
    /// Subcommands for managing the inclean.toml config file.
    Config(ConfigArgs),
}

// ---- Check ---------------------------------------------------------------

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Which subset of violations to report:
    ///   config: only validate the inclean.toml (no source files opened);
    ///   unfixable: errors / evaluation failures / conflicts only;
    ///   all (default): every per-include outcome including fixable.
    #[arg(value_enum, default_value_t = CheckKind::All)]
    pub kind: CheckKind,

    /// Path to inclean.toml. When omitted, the CLI walks upward from the
    /// current directory to find one. `config` mode honors this; `unfixable`
    /// / `all` modes use it to anchor the project root.
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Parallel worker count. `config` mode ignores this. Default = CPU count.
    #[arg(short, long)]
    pub jobs: Option<usize>,

    /// Optional file/directory restrictions. When given, only source files
    /// rooted at one of these paths are processed. `config` mode ignores this.
    pub paths: Vec<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum CheckKind {
    Config,
    Unfixable,
    All,
}

impl CheckKind {
    pub fn check_mode(self) -> CheckMode {
        match self {
            CheckKind::Config => CheckMode::Config,
            CheckKind::Unfixable | CheckKind::All => CheckMode::Run,
        }
    }
}

// ---- Apply / Diff --------------------------------------------------------

#[derive(Args, Debug)]
pub struct ApplyArgs {
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    #[arg(short, long)]
    pub jobs: Option<usize>,
    pub paths: Vec<PathBuf>,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Write the unified diff to PATH instead of stdout.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    #[arg(short, long)]
    pub jobs: Option<usize>,
    pub paths: Vec<PathBuf>,
}

// ---- Init ----------------------------------------------------------------

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Target path. Existing dir → create inclean.toml inside. Nonexistent
    /// path → see init module docs. Omitted → CWD.
    pub path: Option<PathBuf>,
}

// ---- Config sub-commands -------------------------------------------------

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigSub,
}

#[derive(Subcommand, Debug)]
pub enum ConfigSub {
    /// Validate inclean.toml (alias of `check config`).
    Check {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Generate a starter inclean.toml at the given path (alias of `init`).
    New(InitArgs),
    /// Emit (or validate) the JSON Schema for inclean.toml.
    Schema(schema::SchemaArgs),
}

// ---- Entry ---------------------------------------------------------------

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init(args) => init::run(args.path),
        Command::Check(args) => check::run(args),
        Command::Apply(args) => apply::run(args),
        Command::Diff(args) => diff::run(args),
        Command::Config(ConfigArgs { command }) => match command {
            ConfigSub::Check { config } => check::run(CheckArgs {
                kind: CheckKind::Config,
                config,
                jobs: None,
                paths: vec![],
            }),
            ConfigSub::New(args) => init::run(args.path),
            ConfigSub::Schema(args) => schema::run(args),
        },
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}
