//! `inclean apply` — apply rewrites in place.

use anyhow::Result;

use super::ApplyArgs;
use crate::pipeline::run::{self, CheckMode, IncludeOutcome};

pub fn run(args: ApplyArgs) -> Result<u8> {
    let start_dir = super::check::start_dir_for(args.config.as_deref(), &args.paths);
    // Pre-flight config-only check so failures surface with a friendly
    // prefix rather than mid-pipeline.
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
    let written = run::apply(&summary)?;
    let code = run::summary_exit_code(&summary);
    let skipped_for_errors = summary
        .files
        .iter()
        .filter(|f| {
            f.include_results.iter().any(|r| {
                matches!(
                    r.outcome,
                    IncludeOutcome::Error { .. }
                        | IncludeOutcome::EvaluationFailure { .. }
                        | IncludeOutcome::Conflict { .. }
                )
            })
        })
        .count();
    if !summary.skipped.is_empty() {
        eprintln!(
            "warning: skipped {} file(s) that could not be parsed",
            summary.skipped.len()
        );
        for s in &summary.skipped {
            eprintln!("  - {}: {}", s.relpath.display(), s.reason);
        }
    }
    println!("wrote {written} file(s); {skipped_for_errors} file(s) skipped due to errors");
    Ok(code)
}
