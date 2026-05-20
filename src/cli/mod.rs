use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand};

mod apply;
mod check;
mod diff;
mod explain;
mod init;

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
    /// Three-mode read-only check (default: full)
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
}

#[derive(Args, Debug)]
#[command(group(
    ArgGroup::new("check_mode")
        .required(false)
        .multiple(false)
        .args(["syntax_only", "no_rewrites"])
))]
pub struct CheckArgs {
    /// Directory containing the root inclean.toml
    #[arg(default_value = ".")]
    pub dir: std::path::PathBuf,
    /// Only verify the inclean.toml configuration; do not open any source file.
    #[arg(long)]
    pub syntax_only: bool,
    /// Verify config + rule-tree invariants over actual source, but do not
    /// evaluate actions or run allowed_include_dirs validation.
    #[arg(long)]
    pub no_rewrites: bool,
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Init { dir } => init::run(dir),
        Command::Check(args) => check::run(args),
        Command::Diff { dir } => diff::run(dir),
        Command::Apply { dir } => apply::run(dir),
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
