//! `inclean explain <FILE> [INCLUDE]` — trace which rule matches each
//! `#include` in `FILE`, showing layer-by-layer trial outcomes.
//!
//! - `FILE` is any path inside the project; the root `inclean.toml` is
//!   discovered by walking upward from it.
//! - `INCLUDE` filters which includes are traced. Accepted forms:
//!   `"foo.h"`, `<foo.h>`, or just `foo.h` (matches either form).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::discover;
use crate::config::inherit;
use crate::config::schema::IncludeForm;
use crate::lex::include_line::{self, Include};
use crate::rule::action::{self, Outcome};
use crate::rule::engine::{self, CompiledRule, RuleTrial};

pub fn run(file: PathBuf, include_filter: Option<String>) -> Result<u8> {
    let file_abs =
        std::fs::canonicalize(&file).with_context(|| format!("canonicalize {}", file.display()))?;

    let config_path = discover::find_root_config(&file_abs)?;
    let cfg = discover::load_root_config(&config_path)?;
    let project = cfg
        .raw
        .project
        .as_ref()
        .expect("load_root_config guarantees [project] is present");
    let project_root = discover::resolve_project_root(&config_path, project)?;
    discover::assert_no_extra_configs(&project_root, &config_path)?;
    let resolved = inherit::resolve(std::slice::from_ref(&cfg))?;

    let compiled: Vec<CompiledRule<'_>> = resolved
        .values()
        .map(|r| CompiledRule::new(r, &project_root))
        .collect::<Result<Vec<_>>>()?;

    let source = std::fs::read_to_string(&file_abs)
        .with_context(|| format!("reading {}", file_abs.display()))?;
    let includes = include_line::scan(&source);
    let file_relpath = file_abs
        .strip_prefix(&project_root)
        .unwrap_or(&file_abs)
        .to_path_buf();

    let filter = include_filter.as_deref().map(parse_filter);

    println!("File:           {}", file_relpath.display());
    println!("Project root:   {}", project_root.display());
    println!("Config:         {}", cfg.path.display());
    println!();

    let mut printed_any = false;
    for include in &includes {
        if let Some(filt) = &filter {
            if !filt.matches(include) {
                continue;
            }
        }
        printed_any = true;
        print_include_trace(&compiled, &file_relpath, &project_root, &source, include);
        println!();
    }
    if !printed_any {
        if filter.is_some() {
            println!(
                "(no #include in {} matched the filter)",
                file_relpath.display()
            );
        } else {
            println!(
                "(no #include directives found in {})",
                file_relpath.display()
            );
        }
    }
    Ok(0)
}

fn print_include_trace(
    rules: &[CompiledRule<'_>],
    file_relpath: &Path,
    project_root: &Path,
    source: &str,
    include: &Include,
) {
    let form_word = match include.form {
        IncludeForm::Quote => "quote",
        IncludeForm::Angle => "angle",
        IncludeForm::Macro => "macro",
    };
    println!(
        "Include: L{}  form={form_word}  content={:?}",
        include.line, include.content
    );

    let trials = engine::trace_match(rules, file_relpath, include, project_root);
    if trials.is_empty() {
        println!("  (no eligible rules — no config covers this file's directory)");
        return;
    }

    println!("Rule trial order:");
    let mut matched_idx: Option<usize> = None;
    for (i, t) in trials.iter().enumerate() {
        print_one_trial(t);
        if t.matched_overall {
            matched_idx = Some(i);
            break;
        }
    }

    if let Some(i) = matched_idx {
        let t = &trials[i];
        println!();
        println!("Action evaluation:");
        let m = engine::Match {
            rule: t.rule,
            captures: t.captures.clone().unwrap_or_default(),
            resolved: t.resolved.clone(),
        };
        match action::evaluate(&m, include, source, file_relpath, project_root) {
            Ok(Outcome::Keep) => println!("  keep — leave the include unchanged"),
            Ok(Outcome::Rewrite { new_text, .. }) => {
                println!("  rewrite → {new_text}");
            }
            Ok(Outcome::Error { message }) => println!("  error — {message}"),
            Err(err) => println!("  evaluation failed: {err:#}"),
        }
    } else {
        println!();
        println!("Result: no rule matched (include left as-is)");
    }
}

fn print_one_trial(t: &RuleTrial<'_>) {
    let label = format!(
        "  {} :: \"{}\"",
        t.rule.rule.origin.config_path.display(),
        t.rule.rule.name
    );
    let extends = t
        .rule
        .rule
        .extends
        .as_deref()
        .map(|e| format!("  (extends \"{e}\")"))
        .unwrap_or_default();
    println!("{label}{extends}");
    let mark = |b: bool| if b { "✓" } else { "✗" };
    if let Some(l) = &t.layer1_paths {
        println!("    layer 1 (paths)      {} {}", mark(l.passed), l.detail);
    }
    if let Some(l) = &t.layer2_extensions {
        println!("    layer 2 (extensions) {} {}", mark(l.passed), l.detail);
    }
    if let Some(l) = &t.layer3_forms {
        println!("    layer 3 (forms)      {} {}", mark(l.passed), l.detail);
    }
    if let Some(l) = &t.layer4_match {
        println!("    layer 4 (match)      {} {}", mark(l.passed), l.detail);
    }
    if let Some(l) = &t.layer5_resolved {
        println!("    layer 5 (resolved)   {} {}", mark(l.passed), l.detail);
    }
    if t.matched_overall {
        if let Some(caps) = &t.captures {
            if caps.len() > 1 {
                println!("    captures: {:?}", &caps[1..]);
            }
        }
        println!("    → ★ MATCHED");
    } else {
        println!("    → not matched");
    }
}

#[derive(Debug)]
enum Filter {
    /// User passed bare content; match any form.
    Content(String),
    /// User passed quoted form `"foo.h"`.
    Quote(String),
    /// User passed angle form `<foo.h>`.
    Angle(String),
}

impl Filter {
    fn matches(&self, include: &Include) -> bool {
        match self {
            Filter::Content(s) => include.content == *s,
            Filter::Quote(s) => include.form == IncludeForm::Quote && include.content == *s,
            Filter::Angle(s) => include.form == IncludeForm::Angle && include.content == *s,
        }
    }
}

fn parse_filter(s: &str) -> Filter {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        Filter::Quote(s[1..s.len() - 1].to_string())
    } else if s.starts_with('<') && s.ends_with('>') && s.len() >= 2 {
        Filter::Angle(s[1..s.len() - 1].to_string())
    } else {
        Filter::Content(s.to_string())
    }
}
