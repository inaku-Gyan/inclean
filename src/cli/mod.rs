use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod apply;
mod check;
mod diff;
mod explain;
mod init;
mod validate;

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
    /// Verify the inclean.toml configuration only; do not touch source files
    Validate {
        /// Directory containing the root inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
    },
    /// Report rewrites and validation errors without modifying files
    Check {
        /// Directory containing the root inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
        #[arg(long)]
        no_validate: bool,
    },
    /// Show a unified diff of would-be rewrites without modifying files
    Diff {
        /// Directory containing the root inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
        #[arg(long)]
        no_validate: bool,
    },
    /// Apply rewrites to files in place
    Apply {
        /// Directory containing the root inclean.toml
        #[arg(default_value = ".")]
        dir: std::path::PathBuf,
        #[arg(long)]
        no_validate: bool,
    },
    /// Trace which rule matches an include in a given file
    Explain {
        file: std::path::PathBuf,
        include: Option<String>,
    },
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init { dir } => init::run(dir),
        Command::Validate { dir } => validate::run(dir),
        Command::Check { dir, no_validate } => check::run(dir, !no_validate),
        Command::Diff { dir, no_validate } => diff::run(dir, !no_validate),
        Command::Apply { dir, no_validate } => apply::run(dir, !no_validate),
        Command::Explain { file, include } => explain::run(file, include),
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}
