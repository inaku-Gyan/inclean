//! `inclean apply` — apply rewrites in place.
//!
//! Fixable rewrites are written even when the same run produced
//! unfixable violations elsewhere. A file containing *any* unfixable
//! outcome is skipped entirely (no partial writes per file). After
//! writing, the unfixable report is printed; the exit code is non-zero
//! iff at least one unfixable violation surfaced.

use anyhow::Result;

use super::{ApplyArgs, report, style as cli_style};
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
        report::print_skipped_parse_failures(&summary.skipped);
    }
    report::print_warnings(&summary.warnings);
    let status = if skipped_for_errors == 0 {
        cli_style::success("wrote")
    } else {
        cli_style::warning_out("wrote")
    };
    println!(
        "{status} {written} file(s); {skipped_for_errors} file(s) skipped due to unfixable violations"
    );
    let unfixable_report = report::render_unfixable_report(&summary);
    if !unfixable_report.is_empty() {
        eprint!("{unfixable_report}");
    }
    Ok(run::summary_exit_code(&summary))
}
