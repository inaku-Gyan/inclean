use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::pipeline::run::CheckMode;

mod apply;
mod check;
mod diff;
mod explain;
mod init;
mod schema;

#[derive(Parser, Debug)]
#[command(name = "inclean", version, about = "C/C++ #include path normalizer")]
struct Cli {
    /// Parallel worker count (defaults to CPU count)
    #[arg(short, long, global = true)]
    jobs: Option<usize>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a starter inclean.toml in the given directory
    Init {
        /// Directory in which to create inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Three-level read-only check (default: full)
    Check(CheckArgs),
    /// Show a unified diff of would-be rewrites without modifying files
    Diff {
        /// Directory containing the root inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Apply rewrites to files in place
    Apply {
        /// Directory containing the root inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Trace which rule matches an include in a given file
    Explain {
        file: std::path::PathBuf,
        include: Option<String>,
    },
    /// Emit (or validate) the JSON Schema for inclean.toml
    Schema(schema::SchemaArgs),
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Directory containing the root inclean.toml
    #[arg(default_value = ".")]
    pub dir: std::path::PathBuf,
    /// How deep to check.
    ///   config: only validate inclean.toml (no source files opened).
    ///   rules:  also scan source and enforce rule-tree invariants.
    ///   full:   also evaluate actions and validate against allowed_include_dirs.
    #[arg(short, long, value_enum, default_value_t = CheckLevel::Full)]
    pub level: CheckLevel,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum CheckLevel {
    Config,
    Rules,
    Full,
}

impl From<CheckLevel> for CheckMode {
    fn from(level: CheckLevel) -> Self {
        match level {
            CheckLevel::Config => CheckMode::Config,
            CheckLevel::Rules => CheckMode::Rules,
            CheckLevel::Full => CheckMode::Full,
        }
    }
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init { dir } => init::run(dir),
        Command::Check(args) => check::run(args),
        Command::Diff { dir } => diff::run(dir),
        Command::Apply { dir } => apply::run(dir),
        Command::Explain { file, include } => explain::run(file, include),
        Command::Schema(args) => schema::run(args),
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}
