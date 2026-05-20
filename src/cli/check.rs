//! `inclean check <DIR> [-l/--level config|rules|full]` — three-level
//! read-only check. Never writes a file.
//!
//! - `--level config`: just verify the configuration's structural
//!   invariants (TOML syntax, [project].root sigil, extends graph, name
//!   uniqueness, `@std.*` constants, layer-5 rejection). No source files
//!   are opened.
//! - `--level rules`: also scan source files and enforce the rule-tree
//!   invariants — child rules' match sets must be subsets of their
//!   ancestors', and rules on different chains must not overlap.
//! - `--level full` (default): also evaluate actions and validate
//!   post-action includes against `allowed_include_dirs`.

use anyhow::Result;

use super::CheckArgs;
use crate::pipeline::run::{self, CheckMode, ConflictKindOwned, IncludeOutcome, Summary};

pub fn run(args: CheckArgs) -> Result<u8> {
    let mode: CheckMode = args.level.into();
    let summary = run::run(&args.dir, mode)?;
    match summary.mode {
        CheckMode::Config => print_config_report(&args)?,
        CheckMode::Rules => print_rules_report(&summary),
        CheckMode::Full => print_full_report(&summary),
    }
    Ok(run::summary_exit_code(&summary))
}

/// Config mode lists the config files and rules loaded. The pipeline has
/// already validated their structure; we re-walk discovery so we can show
/// file paths and rule origins.
fn print_config_report(args: &CheckArgs) -> Result<()> {
    use crate::config::discover;
    use crate::config::inherit;
    let configs = discover::load_all_configs(&args.dir)?;
    discover::validate_loaded(&configs, &args.dir)?;
    let resolved = inherit::resolve(&configs)?;
    println!(
        "ok: loaded {} config file(s), {} rule(s)",
        configs.len(),
        resolved.len()
    );
    for cfg in &configs {
        println!("  config: {}", cfg.path.display());
    }
    for (name, rule) in &resolved {
        let extends = rule
            .extends
            .as_deref()
            .map(|p| format!(" extends `{p}`"))
            .unwrap_or_default();
        println!(
            "  rule:   `{name}`{extends}  ({} :: #{})",
            rule.origin.config_path.display(),
            rule.origin.index
        );
    }
    Ok(())
}

fn print_rules_report(summary: &Summary) {
    let ambiguities: Vec<_> = summary
        .files
        .iter()
        .flat_map(|f| {
            f.include_results
                .iter()
                .filter(|r| matches!(r.outcome, IncludeOutcome::Layer5Ambiguous { .. }))
                .map(move |r| (f, r))
        })
        .collect();

    if summary.conflicts.is_empty() && ambiguities.is_empty() {
        let matched: usize = summary
            .files
            .iter()
            .flat_map(|f| f.include_results.iter())
            .filter(|r| matches!(r.outcome, IncludeOutcome::Matched { .. }))
            .count();
        println!(
            "ok: scanned {} file(s), {matched} include(s) matched a rule, no conflicts",
            summary.files.len()
        );
        return;
    }
    if !summary.conflicts.is_empty() {
        print_conflicts(summary);
    }
    if !ambiguities.is_empty() {
        eprintln!();
        eprintln!("{} layer-5 ambiguity(ies) detected:", ambiguities.len());
        for (file, r) in ambiguities {
            if let IncludeOutcome::Layer5Ambiguous { rule, candidates } = &r.outcome {
                eprintln!(
                    "  {}:{} {}  rule `{rule}` (narrow original_include_dirs):",
                    file.relpath.display(),
                    r.include.line,
                    r.include.content
                );
                for c in candidates {
                    eprintln!("    - {}", c.display());
                }
            }
        }
    }
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
                IncludeOutcome::Matched { rule } => println!(
                    "  L{:>4} match   \"{}\"   (rule: {rule})",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::Keep { rule } => println!(
                    "  L{:>4} keep    \"{}\"   (rule: {rule})",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::Rewritten { rule, new_text, .. } => println!(
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
                IncludeOutcome::Conflict => eprintln!(
                    "  L{:>4} conflict \"{}\"   (see conflicts block)",
                    r.include.line, r.include.content
                ),
                IncludeOutcome::Layer5Ambiguous { rule, candidates } => {
                    eprintln!(
                        "  L{:>4} ambig   \"{}\"   (rule: {rule}): include resolves to {} candidates under original_include_dirs:",
                        r.include.line,
                        r.include.content,
                        candidates.len()
                    );
                    for c in candidates {
                        eprintln!("           - {}", c.display());
                    }
                }
            }
            if let Some(msg) = &r.validation_error {
                eprintln!(
                    "  L{:>4} validate \"{}\":  {msg}",
                    r.include.line, r.include.content
                );
            }
        }
    }
    if interesting == 0 && summary.conflicts.is_empty() {
        println!("no changes proposed");
    }
    if !summary.conflicts.is_empty() {
        print_conflicts(summary);
    }
}

fn print_conflicts(summary: &Summary) {
    eprintln!();
    eprintln!(
        "{} rule-tree conflict(s) detected:",
        summary.conflicts.len()
    );
    for c in &summary.conflicts {
        match &c.kind {
            ConflictKindOwned::ChildWiderThanParent {
                child,
                missing_ancestor,
            } => eprintln!(
                "  {}:{} {}  rule `{child}` matched but its ancestor `{missing_ancestor}` did not",
                c.file_relpath.display(),
                c.include_line,
                c.include_text
            ),
            ConflictKindOwned::CrossChain { a, b } => eprintln!(
                "  {}:{} {}  rules `{a}` and `{b}` both match but are not on the same extends chain",
                c.file_relpath.display(),
                c.include_line,
                c.include_text
            ),
        }
    }
}
