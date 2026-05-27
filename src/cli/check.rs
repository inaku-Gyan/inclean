//! `inclean check {config|unfixable|all}` — read-only check.
//!
//! - `config`: parse / validate / run copy resolution only.
//! - `unfixable`: full pipeline; only print errors / evaluation failures
//!   / conflicts (everything fixable is silenced).
//! - `all` (default for bare `inclean check`): full pipeline; print every
//!   per-include outcome.

use std::path::PathBuf;

use anyhow::Result;

use super::CheckRunArgs;
use crate::pipeline::run::{self, CheckMode, IncludeOutcome, Summary};

/// `inclean check config` / `inclean config check`. Validates the
/// inclean.toml alone without opening any source files.
pub fn run_config(config: Option<PathBuf>) -> Result<u8> {
    let start_dir = start_dir_for(config.as_deref(), &[]);
    let summary = run::run(config.as_deref(), &start_dir, &[], None, CheckMode::Config)?;
    print_config_report(config.as_deref(), &start_dir)?;
    for w in &summary.warnings {
        eprintln!("{w}");
    }
    Ok(run::summary_exit_code(&summary))
}

/// `inclean check unfixable` / `inclean check all`. Runs the full
/// pipeline; the `filter` controls which outcomes are printed.
pub fn run_full(args: CheckRunArgs, filter: ReportFilter) -> Result<u8> {
    let start_dir = start_dir_for(args.config.as_deref(), &args.paths);
    let summary = run::run(
        args.config.as_deref(),
        &start_dir,
        &args.paths,
        args.jobs,
        CheckMode::Run,
    )?;
    print_full_report(&summary, filter);
    for w in &summary.warnings {
        eprintln!("{w}");
    }
    Ok(run::summary_exit_code(&summary))
}

/// Pick a starting directory for config discovery. Prefers the config
/// flag's parent if given, else the first user-supplied path (if it's a
/// directory), else CWD.
pub(super) fn start_dir_for(config: Option<&std::path::Path>, paths: &[PathBuf]) -> PathBuf {
    if let Some(c) = config {
        return c
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }
    if let Some(first) = paths.first() {
        if first.is_dir() {
            return first.clone();
        }
        if let Some(parent) = first.parent() {
            if !parent.as_os_str().is_empty() {
                return parent.to_path_buf();
            }
        }
    }
    PathBuf::from(".")
}

fn print_config_report(
    config: Option<&std::path::Path>,
    start_dir: &std::path::Path,
) -> Result<()> {
    use crate::config::copy;
    use crate::config::discover;
    let config_path: PathBuf = match config {
        Some(p) => p.to_path_buf(),
        None => discover::find_root_config(start_dir)?,
    };
    let cfg = discover::load_root_config(&config_path)?;
    let project = &cfg.raw.project;
    let project_root = discover::resolve_project_root(&config_path, project)?;
    discover::assert_no_extra_configs(&project_root, &config_path)?;
    let resolved = copy::resolve(std::slice::from_ref(&cfg))?;
    println!(
        "ok: loaded {}, project root = {} ({} rule(s))",
        cfg.path.display(),
        project_root.display(),
        resolved.len()
    );
    for (name, rule) in resolved.iter() {
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

#[derive(Copy, Clone)]
pub enum ReportFilter {
    All,
    UnfixableOnly,
}

fn print_full_report(summary: &Summary, filter: ReportFilter) {
    let mut interesting = 0usize;
    for file in &summary.files {
        let any = file
            .include_results
            .iter()
            .any(|r| should_print(&r.outcome, filter));
        if !any {
            continue;
        }
        interesting += 1;
        println!("{}:", file.relpath.display());
        for r in &file.include_results {
            if !should_print(&r.outcome, filter) {
                continue;
            }
            match &r.outcome {
                IncludeOutcome::NoMatch => {}
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
                IncludeOutcome::Conflict {
                    rule_outputs,
                    differing_aspects,
                } => {
                    eprintln!(
                        "  L{:>4} conflict \"{}\":",
                        r.include.line, r.include.content
                    );
                    for (rule, text) in rule_outputs {
                        eprintln!("           rule `{rule}` -> {text}");
                    }
                    if !differing_aspects.is_empty() {
                        let parts: Vec<&str> = differing_aspects
                            .iter()
                            .map(|a| match a {
                                crate::pipeline::run::DiffAspect::IncludePath => "include path",
                                crate::pipeline::run::DiffAspect::OutputForm => "output_form",
                                crate::pipeline::run::DiffAspect::TrailingComment => {
                                    "trailing_comment"
                                }
                            })
                            .collect();
                        eprintln!("           differs in: {}", parts.join(", "));
                    }
                }
                IncludeOutcome::TrailingCommentError { rule, message } => eprintln!(
                    "  L{:>4} trailing-comment error \"{}\"   (rule: {rule}): {message}",
                    r.include.line, r.include.content
                ),
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
        match filter {
            ReportFilter::All => println!("no changes proposed"),
            ReportFilter::UnfixableOnly => println!("no unfixable violations"),
        }
    }
}

fn should_print(outcome: &IncludeOutcome, filter: ReportFilter) -> bool {
    match filter {
        ReportFilter::All => !matches!(outcome, IncludeOutcome::NoMatch),
        ReportFilter::UnfixableOnly => matches!(
            outcome,
            IncludeOutcome::Error { .. }
                | IncludeOutcome::TrailingCommentError { .. }
                | IncludeOutcome::EvaluationFailure { .. }
                | IncludeOutcome::Conflict { .. }
        ),
    }
}
