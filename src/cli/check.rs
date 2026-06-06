//! `inclean check {config|unfixable|all}` — read-only check.
//!
//! - `config`: parse / validate / run copy resolution only.
//! - `unfixable`: full pipeline; only print errors / evaluation failures
//!   / conflicts (everything fixable is silenced).
//! - `all` (default for bare `inclean check`): full pipeline; print matched
//!   per-include outcomes by default.

use std::path::PathBuf;

use anyhow::Result;

use super::{CheckRunArgs, CheckScanArgs, report, style as cli_style};
use crate::pipeline::run::{self, CheckMode, IncludeOutcome, Summary};

/// `inclean check config` / `inclean config check`. Validates the
/// inclean.toml alone without opening any source files.
pub fn run_config(config: Option<PathBuf>) -> Result<u8> {
    let start_dir = start_dir_for(config.as_deref(), &[]);
    let summary = run::run(config.as_deref(), &start_dir, &[], None, CheckMode::Config)?;
    print_config_report(config.as_deref(), &start_dir)?;
    report::print_warnings(&summary.warnings);
    Ok(run::summary_exit_code(&summary))
}

/// `inclean check unfixable` / `inclean check all`. Runs the full
/// pipeline; the `filter` controls which outcomes are printed.
pub fn run_full(args: CheckScanArgs, filter: ReportFilter) -> Result<u8> {
    let start_dir = start_dir_for(args.config.as_deref(), &args.paths);
    let summary = run::run(
        args.config.as_deref(),
        &start_dir,
        &args.paths,
        args.jobs,
        CheckMode::Run,
    )?;
    print_full_report(&summary, filter);
    report::print_warnings(&summary.warnings);
    Ok(run::summary_exit_code(&summary))
}

pub fn run_check(args: CheckRunArgs) -> Result<u8> {
    let filter = if args.only_unmatched {
        ReportFilter::UnmatchedOnly
    } else if args.show_unmatched {
        ReportFilter::MatchedAndUnmatched
    } else {
        ReportFilter::Matched
    };
    run_full(args.scan, filter)
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
        if let Some(parent) = first.parent()
            && !parent.as_os_str().is_empty()
        {
            return parent.to_path_buf();
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
    let resolved = copy::resolve(std::slice::from_ref(&cfg))?;
    println!(
        "{} loaded {}, project root = {} ({} rule(s))",
        cli_style::success("ok:"),
        cli_style::path(cfg.path.display()),
        cli_style::path(project_root.display()),
        resolved.len()
    );
    for (name, rule) in resolved.iter() {
        let copied = rule
            .copied_from
            .as_deref()
            .map(|p| {
                format!(
                    " {} `{}`",
                    cli_style::label("copied from"),
                    cli_style::rule(p)
                )
            })
            .unwrap_or_default();
        println!(
            "  {} `{}`{copied}  ({} :: #{})",
            cli_style::label("rule:"),
            cli_style::rule(name),
            cli_style::path(rule.origin.config_path.display()),
            rule.origin.index
        );
    }
    Ok(())
}

#[derive(Copy, Clone)]
pub enum ReportFilter {
    Matched,
    MatchedAndUnmatched,
    UnmatchedOnly,
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
        println!("{}:", cli_style::path(file.relpath.display()));
        for r in &file.include_results {
            if !should_print(&r.outcome, filter) {
                continue;
            }
            match &r.outcome {
                IncludeOutcome::NoMatch => println!(
                    "  {} {} #include {}",
                    cli_style::line_tag(r.include.line),
                    cli_style::status("no-match"),
                    cli_style::include(&r.include.full_content())
                ),
                IncludeOutcome::Skipped { .. } => {}
                IncludeOutcome::Keep { rules } => println!(
                    "  {} {}    {}   ({} {})",
                    cli_style::line_tag(r.include.line),
                    cli_style::keep("keep"),
                    cli_style::include(&r.include.full_content()),
                    cli_style::label("rules:"),
                    cli_style::rules(rules)
                ),
                IncludeOutcome::Rewritten {
                    rules, new_text, ..
                } => println!(
                    "  {} {} {}  {}  {}   ({} {})",
                    cli_style::line_tag(r.include.line),
                    cli_style::rewrite("rewrite"),
                    cli_style::include(&r.include.full_content()),
                    cli_style::label("->"),
                    cli_style::include(new_text),
                    cli_style::label("rules:"),
                    cli_style::rules(rules)
                ),
                IncludeOutcome::Error { rule, message } => eprintln!(
                    "  {} {}   {}   ({} {}): {message}",
                    cli_style::line_tag_err(r.include.line),
                    cli_style::error("error"),
                    cli_style::include_err(&r.include.full_content()),
                    cli_style::label_err("rule:"),
                    cli_style::rule_err(rule)
                ),
                IncludeOutcome::EvaluationFailure { rule, message } => eprintln!(
                    "  {} {}    {}   ({} {}): {message}",
                    cli_style::line_tag_err(r.include.line),
                    cli_style::failure("fail"),
                    cli_style::include_err(&r.include.full_content()),
                    cli_style::label_err("rule:"),
                    cli_style::rule_err(rule)
                ),
                IncludeOutcome::Conflict {
                    rule_outputs,
                    differing_aspects,
                } => {
                    eprintln!(
                        "  {} {} {}:",
                        cli_style::line_tag_err(r.include.line),
                        cli_style::conflict("conflict"),
                        cli_style::include_err(&r.include.full_content())
                    );
                    for (rule, text) in rule_outputs {
                        eprintln!(
                            "           {} `{}` {} {}",
                            cli_style::label_err("rule"),
                            cli_style::rule_err(rule),
                            cli_style::label_err("->"),
                            cli_style::include_err(text)
                        );
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
                        eprintln!(
                            "           {} {}",
                            cli_style::label_err("differs in:"),
                            cli_style::warning(parts.join(", "))
                        );
                    }
                }
                IncludeOutcome::TrailingCommentError { rule, message } => eprintln!(
                    "  {} {} {}   ({} {}): {message}",
                    cli_style::line_tag_err(r.include.line),
                    cli_style::error("trailing-comment error"),
                    cli_style::include_err(&r.include.full_content()),
                    cli_style::label_err("rule:"),
                    cli_style::rule_err(rule)
                ),
            }
        }
    }
    if !summary.skipped.is_empty() {
        eprintln!();
        report::print_skipped_parse_failures(&summary.skipped);
    }
    if interesting == 0 && summary.conflicts.is_empty() {
        match filter {
            ReportFilter::Matched | ReportFilter::MatchedAndUnmatched => {
                println!("{}", cli_style::success("no changes proposed"))
            }
            ReportFilter::UnmatchedOnly => {
                println!("{}", cli_style::success("no unmatched includes"))
            }
            ReportFilter::UnfixableOnly => {
                println!("{}", cli_style::success("no unfixable violations"))
            }
        }
    }
}

fn should_print(outcome: &IncludeOutcome, filter: ReportFilter) -> bool {
    match filter {
        ReportFilter::Matched => !matches!(
            outcome,
            IncludeOutcome::NoMatch | IncludeOutcome::Skipped { .. }
        ),
        ReportFilter::MatchedAndUnmatched => !matches!(outcome, IncludeOutcome::Skipped { .. }),
        ReportFilter::UnmatchedOnly => matches!(outcome, IncludeOutcome::NoMatch),
        ReportFilter::UnfixableOnly => matches!(
            outcome,
            IncludeOutcome::Error { .. }
                | IncludeOutcome::TrailingCommentError { .. }
                | IncludeOutcome::EvaluationFailure { .. }
                | IncludeOutcome::Conflict { .. }
        ),
    }
}
