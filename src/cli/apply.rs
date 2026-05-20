//! `inclean apply <DIR>` — apply rewrites in place. Files that report any
//! `Error` or `EvaluationFailure` are skipped (no partial writes); the
//! exit code reflects the highest-severity outcome.

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run;

pub fn run(dir: PathBuf, _validate: bool) -> Result<u8> {
    let summary = run::run(&dir)?;
    if !is_git_clean(&summary.project_root) {
        eprintln!(
            "warning: working tree at {} is not clean; consider committing first",
            summary.project_root.display()
        );
    }
    for w in &summary.config_warnings {
        eprintln!("warning: {w}");
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
                        run::IncludeOutcome::Error { .. }
                            | run::IncludeOutcome::EvaluationFailure { .. }
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
