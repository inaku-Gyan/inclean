use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

mod apply;
mod check;
mod diff;
mod init;
mod report;
mod schema;
mod style;

#[derive(Parser, Debug)]
#[command(
    name = "inclean",
    version,
    about = "C/C++ #include path normalizer",
    color = clap::ColorChoice::Always,
    styles = style::HELP_STYLES
)]
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
    #[command(subcommand)]
    pub command: Option<CheckSub>,
}

#[derive(Subcommand, Debug)]
pub enum CheckSub {
    /// Validate the inclean.toml file only — no source files opened.
    Config(CheckConfigArgs),
    /// Full pipeline; print only unfixable violations (errors, evaluation
    /// failures, conflicts).
    Unfixable(CheckRunArgs),
    /// Full pipeline; print every per-include outcome. This is what bare
    /// `inclean check` runs.
    All(CheckRunArgs),
}

#[derive(Args, Debug)]
pub struct CheckConfigArgs {
    /// Path to inclean.toml. When omitted, the CLI walks upward from the
    /// current directory to find one.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
}

#[derive(Args, Debug, Default)]
pub struct CheckRunArgs {
    /// Path to inclean.toml. When omitted, the CLI walks upward from the
    /// current directory to find one.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    /// Parallel worker count. Default = CPU count.
    #[arg(short, long)]
    pub jobs: Option<usize>,
    /// Optional file/directory restrictions. When given, only source files
    /// rooted at one of these paths are processed.
    pub paths: Vec<PathBuf>,
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
    Check(CheckConfigArgs),
    /// Generate a starter inclean.toml at the given path (alias of `init`).
    New(InitArgs),
    /// Emit (or validate) the JSON Schema for inclean.toml.
    Schema(schema::SchemaArgs),
}

// ---- Entry ---------------------------------------------------------------

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init(args) => init::run(args.path.as_deref()),
        Command::Check(args) => match args.command {
            Some(CheckSub::Config(c)) => check::run_config(c.config),
            Some(CheckSub::Unfixable(r)) => check::run_full(r, check::ReportFilter::UnfixableOnly),
            Some(CheckSub::All(r)) => check::run_full(r, check::ReportFilter::All),
            None => check::run_full(CheckRunArgs::default(), check::ReportFilter::All),
        },
        Command::Apply(args) => apply::run(args),
        Command::Diff(args) => diff::run(args),
        Command::Config(args) => match args.command {
            ConfigSub::Check(c) => check::run_config(c.config),
            ConfigSub::New(args) => init::run(args.path.as_deref()),
            ConfigSub::Schema(args) => schema::run(args),
        },
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("{}", style::error_line(&format!("error: {err:#}")));
            ExitCode::from(1)
        }
    }
}
