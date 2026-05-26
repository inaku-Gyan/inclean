//! `inclean check [-l/--level config|full]` — read-only check.
//!
//! - `config`: parse / validate / run copy resolution only.
//! - `full` (default): full pipeline, report all per-include outcomes.

use anyhow::Result;

use super::CheckArgs;
use crate::pipeline::run::{self, CheckMode, IncludeOutcome, Summary};

pub fn run(args: CheckArgs) -> Result<u8> {
    let mode: CheckMode = args.level.into();
    let summary = run::run(&args.dir, mode)?;
    match summary.mode {
        CheckMode::Config => print_config_report(&args)?,
        CheckMode::Run => print_full_report(&summary),
    }
    Ok(run::summary_exit_code(&summary))
}

fn print_config_report(args: &CheckArgs) -> Result<()> {
    use crate::config::copy;
    use crate::config::discover;
    let config_path = discover::find_root_config(&args.dir)?;
    let cfg = discover::load_root_config(&config_path)?;
    let project = cfg
        .raw
        .project
        .as_ref()
        .expect("load_root_config guarantees [project] is present");
    let project_root = discover::resolve_project_root(&config_path, project)?;
    discover::assert_no_extra_configs(&project_root, &config_path)?;
    let resolved = copy::resolve(std::slice::from_ref(&cfg))?;
    println!(
        "ok: loaded {}, project root = {} ({} rule(s))",
        cfg.path.display(),
        project_root.display(),
        resolved.len()
    );
    for (name, rule) in &resolved {
        let copied = rule
            .copied_from
            .as_deref()
            .map(|p| format!(" copied_from `{p}`"))
            .unwrap_or_default();
        println!(
            "  rule:   `{name}`{copied}  ({} :: #{})",
            rule.origin.config_path.display(),
            rule.origin.index
        );
    }
    Ok(())
}

fn print_full_report(summary: &Summary) {
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
                IncludeOutcome::Keep { rules } => println!(
                    "  L{:>4} keep    \"{}\"   (rules: {})",
                    r.include.line,
                    r.include.content,
                    rules.join(", ")
                ),
                IncludeOutcome::Rewritten {
                    rules, new_text, ..
                } => println!(
                    "  L{:>4} rewrite \"{}\"  ->  {new_text}   (rules: {})",
                    r.include.line,
                    r.include.content,
                    rules.join(", ")
                ),
                IncludeOutcome::Error { rule, message } => eprintln!(
                    "  L{:>4} error   \"{}\"   (rule: {rule}): {message}",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::EvaluationFailure { rule, message } => eprintln!(
                    "  L{:>4} fail    \"{}\"   (rule: {rule}): {message}",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::Conflict { rule_outputs } => {
                    eprintln!(
                        "  L{:>4} conflict \"{}\":",
                        r.include.line, r.include.content
                    );
                    for (rule, text) in rule_outputs {
                        eprintln!("           rule `{rule}` -> {text}");
                    }
                }
            }
        }
    }
    if !summary.skipped.is_empty() {
        eprintln!();
        eprintln!(
            "warning: skipped {} file(s) that could not be parsed:",
            summary.skipped.len()
        );
        for s in &summary.skipped {
            eprintln!("  - {}: {}", s.relpath.display(), s.reason);
        }
    }
    if interesting == 0 && summary.conflicts.is_empty() {
        println!("no changes proposed");
    }
}
