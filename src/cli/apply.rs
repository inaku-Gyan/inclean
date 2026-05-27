//! `inclean apply` — apply rewrites in place. Refuses to write any
//! file if the run produced conflicts. Files that report any per-include
//! Error / EvaluationFailure / Conflict are skipped (no partial writes).

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run::{self, CheckMode, IncludeOutcome};

pub fn run(dir: PathBuf) -> Result<u8> {
    let summary = run::run(None, &dir, &[], None, CheckMode::Run)?;
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
