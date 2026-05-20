//! `inclean check <DIR>` — dry-run report. Reports every would-be rewrite
//! and any errors / evaluation failures. Never writes a file.

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run::{self, IncludeOutcome, Summary};

pub fn run(dir: PathBuf, validate: bool) -> Result<u8> {
    let summary = run::run(&dir, validate)?;
    print_report(&summary);
    Ok(run::summary_exit_code(&summary))
}

pub(super) fn print_report(summary: &Summary) {
    for w in &summary.config_warnings {
        eprintln!("warning: {w}");
    }

    let mut interesting = 0usize;
    for file in &summary.files {
        let any = file
            .include_results
            .iter()
            .any(|r| !matches!(r.outcome, IncludeOutcome::NoMatch));
        if !any {
            continue;
        }
        interesting += 1;
        println!("{}:", file.relpath.display());
        for r in &file.include_results {
            match &r.outcome {
                IncludeOutcome::NoMatch => continue,
                IncludeOutcome::Keep { rule } => println!(
                    "  L{:>4} keep    \"{}\"   (rule: {rule})",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::Rewritten {
                    rule, new_text, ..
                } => println!(
                    "  L{:>4} rewrite \"{}\"  ->  {new_text}   (rule: {rule})",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::Error { rule, message } => eprintln!(
                    "  L{:>4} error   \"{}\"   (rule: {rule}): {message}",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::EvaluationFailure { rule, message } => eprintln!(
                    "  L{:>4} fail    \"{}\"   (rule: {rule}): {message}",
                    r.include.line, r.include.content
                ),
            }
            if let Some(msg) = &r.validation_error {
                eprintln!(
                    "  L{:>4} validate \"{}\":  {msg}",
                    r.include.line, r.include.content
                );
            }
        }
    }
    if interesting == 0 {
        println!("no changes proposed");
    }
}
