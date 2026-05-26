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
    /// Parallel worker count (defaults to CPU count). Currently advisory
    /// — the pipeline uses rayon's default thread pool.
    #[arg(short, long, global = true)]
    #[allow(dead_code)]
    jobs: Option<usize>,

    /// Path to the project's inclean.toml. If omitted, the CLI walks
    /// upward from the working directory to find one.
    #[arg(short, long, global = true)]
    #[allow(dead_code)]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a starter inclean.toml at the given path (alias of `config new`).
    Init {
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
    },
    /// Read-only check.
    Check(CheckArgs),
    /// Apply rewrites in place.
    Apply {
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Show a unified diff of would-be rewrites.
    Diff {
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Subcommands for managing the inclean.toml config file.
    Config(ConfigArgs),
    /// Emit (or validate) the JSON Schema for inclean.toml.
    /// (Kept as a top-level shortcut alongside `config schema`.)
    Schema(schema::SchemaArgs),
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Directory containing the root inclean.toml.
    #[arg(default_value = ".")]
    pub dir: std::path::PathBuf,
    /// Which slice of the pipeline to run.
    ///   config: only validate inclean.toml (no source files opened).
    ///   full (default): scan source and report every per-include outcome.
    #[arg(short, long, value_enum, default_value_t = CheckLevel::Full)]
    pub level: CheckLevel,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum CheckLevel {
    Config,
    Full,
}

impl From<CheckLevel> for CheckMode {
    fn from(level: CheckLevel) -> Self {
        match level {
            CheckLevel::Config => CheckMode::Config,
            CheckLevel::Full => CheckMode::Run,
        }
    }
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigSub,
}

#[derive(Subcommand, Debug)]
pub enum ConfigSub {
    /// Validate inclean.toml (alias of `check --level config`).
    Check {
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Generate a starter inclean.toml at the given path (alias of `init`).
    New {
        #[arg(default_value = ".")]
        path: std::path::PathBuf,
    },
    /// Emit (or validate) the JSON Schema for inclean.toml.
    Schema(schema::SchemaArgs),
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init { path } => init::run(path),
        Command::Check(args) => check::run(args),
        Command::Apply { dir } => apply::run(dir),
        Command::Diff { dir } => diff::run(dir),
        Command::Schema(args) => schema::run(args),
        Command::Config(ConfigArgs { command }) => match command {
            ConfigSub::Check { dir } => check::run(CheckArgs {
                dir,
                level: CheckLevel::Config,
            }),
            ConfigSub::New { path } => init::run(path),
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
