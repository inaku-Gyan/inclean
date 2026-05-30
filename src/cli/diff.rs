//! `inclean diff` — render a unified diff of would-be rewrites.
//!
//! Diff output goes to `-o PATH` (when supplied) or stdout. The
//! unfixable report — if any — is always printed to stderr so the
//! `-o` target stays a clean patch file. Per refactor.md §"inclean
//! diff", the existence of unfixable violations triggers a non-zero
//! exit but does not suppress diff output.

use anyhow::{Context, Result};

use super::{DiffArgs, report, style as cli_style};
use crate::pipeline::run::{self, CheckMode};

pub fn run(args: DiffArgs) -> Result<u8> {
    let start_dir = super::check::start_dir_for(args.config.as_deref(), &args.paths);
    run::run(
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
            eprintln!(
                "{} diff to {}",
                cli_style::success_err("wrote"),
                cli_style::path_err(path.display())
            );
        }
        None => {
            print!("{body}");
        }
    }
    report::print_warnings(&summary.warnings);
    let unfixable_report = report::render_unfixable_report(&summary);
    if !unfixable_report.is_empty() {
        eprint!("{unfixable_report}");
    }
    Ok(run::summary_exit_code(&summary))
}
