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
    if !is_git_clean(&summary.project_root) {
        eprintln!(
            "warning: working tree at {} is not clean; consider committing first",
            summary.project_root.display()
        );
    }
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
                    )
                })
        })
        .count();
    println!(
        "wrote {written} file(s); {skipped_for_errors} file(s) skipped due to errors"
    );
    Ok(code)
}

/// Best-effort check that there are no uncommitted tracked changes in
/// `dir`. Returns `true` when the check is unavailable or the working tree
/// looks clean.
fn is_git_clean(dir: &std::path::Path) -> bool {
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("status")
        .arg("--porcelain")
        .output()
    else {
        return true;
    };
    if !out.status.success() {
        return true;
    }
    out.stdout.is_empty()
}
