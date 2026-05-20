use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod apply;
mod check;
mod diff;
mod explain;
mod init;

#[derive(Parser, Debug)]
#[command(name = "inclean", version, about = "C/C++ #include path normalizer")]
struct Cli {
    /// Override the project root / config search start point
    #[arg(long, global = true)]
    config: Option<std::path::PathBuf>,

    /// Parallel worker count (defaults to CPU count)
    #[arg(short, long, global = true)]
    jobs: Option<usize>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate a starter inclean.toml in the current directory
    Init,
    /// Report rewrites and validation errors without modifying files
    Check {
        path: Option<std::path::PathBuf>,
        #[arg(long)]
        no_validate: bool,
    },
    /// Show a unified diff of would-be rewrites without modifying files
    Diff {
        path: Option<std::path::PathBuf>,
        #[arg(long)]
        no_validate: bool,
    },
    /// Apply rewrites to files in place
    Apply {
        path: Option<std::path::PathBuf>,
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
        Command::Init => init::run(),
        Command::Check { path, no_validate } => check::run(path, !no_validate),
        Command::Diff { path, no_validate } => diff::run(path, !no_validate),
        Command::Apply { path, no_validate } => apply::run(path, !no_validate),
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
