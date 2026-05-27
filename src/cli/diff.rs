//! `inclean diff` — render a unified diff of would-be rewrites.

use anyhow::{Context, Result};

use super::DiffArgs;
use crate::pipeline::run::{self, CheckMode};

pub fn run(args: DiffArgs) -> Result<u8> {
    let start_dir = super::check::start_dir_for(args.config.as_deref(), &args.paths);
    let _ = run::run(
        args.config.as_deref(),
        &start_dir,
        &[],
        None,
        CheckMode::Config,
    )
    .map_err(|e| anyhow::anyhow!("config-only check failed: {e:#}"))?;

    let summary = run::run(
        args.config.as_deref(),
        &start_dir,
        &args.paths,
        args.jobs,
        CheckMode::Run,
    )?;
    let body = run::render_diff(&summary);
    match args.output.as_deref() {
        Some(path) => {
            std::fs::write(path, &body)
                .with_context(|| format!("writing diff to {}", path.display()))?;
            eprintln!("wrote diff to {}", path.display());
        }
        None => {
            print!("{body}");
        }
    }
    Ok(run::summary_exit_code(&summary))
}
