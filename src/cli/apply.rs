//! `inclean apply <DIR>` — apply rewrites in place. Always runs the full
//! pipeline. Refuses to write anything if the rule tree has unresolved
//! conflicts. Files that report any per-include `Error` or
//! `EvaluationFailure` are skipped (no partial writes); the exit code
//! reflects the highest-severity outcome.

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run::{self, CheckMode, IncludeOutcome};

pub fn run(dir: PathBuf) -> Result<u8> {
    let summary = run::run(&dir, CheckMode::Full)?;
    let written = run::apply(&summary)?;
    let code = run::summary_exit_code(&summary);
    let skipped_for_errors = summary
        .files
        .iter()
        .filter(|f| {
            f.rewritten.is_some()
                && f.include_results.iter().any(|r| {
                    matches!(
                        r.outcome,
                        IncludeOutcome::Error { .. }
                            | IncludeOutcome::EvaluationFailure { .. }
                            | IncludeOutcome::Conflict
                            | IncludeOutcome::Layer5Ambiguous { .. }
                    )
                })
        })
        .count();
    println!("wrote {written} file(s); {skipped_for_errors} file(s) skipped due to errors");
    Ok(code)
}
