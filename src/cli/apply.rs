//! `inclean apply` — apply rewrites in place.
//!
//! Fixable rewrites are written even when the same run produced
//! unfixable violations elsewhere. A file containing *any* unfixable
//! outcome is skipped entirely (no partial writes per file). After
//! writing, the unfixable report is printed; the exit code is non-zero
//! iff at least one unfixable violation surfaced.

use anyhow::Result;

use super::ApplyArgs;
use crate::pipeline::run::{self, CheckMode};

pub fn run(args: ApplyArgs) -> Result<u8> {
    let start_dir = super::check::start_dir_for(args.config.as_deref(), &args.paths);
    // Pre-flight config-only check so failures surface with a friendly
    // prefix rather than mid-pipeline.
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
    let written = run::apply(&summary)?;
    let skipped_for_errors = summary
        .files
        .iter()
        .filter(|f| run::file_has_errors(f))
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
    for w in &summary.warnings {
        eprintln!("{w}");
    }
    println!(
        "wrote {written} file(s); {skipped_for_errors} file(s) skipped due to unfixable violations"
    );
    let report = run::render_unfixable_report(&summary);
    if !report.is_empty() {
        eprint!("{report}");
    }
    Ok(run::summary_exit_code(&summary))
}
