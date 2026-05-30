//! Top-level orchestration for v0.3.
//!
//! Two modes:
//!
//! * [`CheckMode::Config`] — parse + validate + run copy resolution only.
//!   No source files are opened.
//! * [`CheckMode::Run`] — walk source files, lex includes, run each rule's
//!   text match layers plus optional include-directory resolution, evaluate
//!   every matched rule's action and trailing-comment policy, then decide
//!   per-include conflicts.
//!
//! Conflict detection (the v0.3 model): for an include matched by N
//! rules, compare action candidates separately from trailing-comment
//! candidates. Field-level `"skip"` contributes no candidate for that
//! dimension; `keep` still contributes its kept text. If either dimension
//! disagrees, the include is a [`Conflict`].
//!
//! Output ordering: candidate source files are pre-sorted by relative
//! path before the parallel work starts; rayon's `par_iter().collect()`
//! preserves input order, so `Summary.files` ends up lexicographically
//! ordered without any extra channel + heap machinery. (The
//! channel-based incremental-output design from refactor.md §"并行与输出
//! 保序" is reserved for a future streaming-progress hook.)
//!
//! Encoding: files are read as bytes; a UTF-8 BOM is detected and
//! preserved across the write-back. Line endings are not normalized —
//! [`crate::rule::action`] picks the line terminator from the line it
//! is editing, so the file's existing CRLF/LF mix survives.
//!
//! Per-file parse failures (malformed UTF-8) are reported in
//! [`Summary::skipped`] and do not contribute to the exit code.
//!
//! Walk policy: per refactor.md §Engine, the walker does **not** honor
//! `.gitignore` and does **not** implicitly skip `.git` / `target` /
//! `node_modules`. The only built-in filter is to skip `inclean.toml`
//! files themselves (they're not source).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Once;

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::config::copy::{self, ResolvedRule};
use crate::config::discover;
use crate::config::schema::IncludeForm;
use crate::lex::include_line::{self, Include};
use crate::profile::CONFIG_FILENAME;
use crate::rule::action::{self, ActionOutcome, TrailingOutcome};
use crate::rule::engine::{self, CompiledRule};

/// Which slice of the pipeline to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMode {
    /// Config-only: parse, validate, copy resolution. No source scan.
    Config,
    /// Full: scan source, evaluate every matched rule, detect conflicts.
    Run,
}

/// Aggregate result of a pipeline invocation.
#[derive(Debug)]
pub struct Summary {
    pub mode: CheckMode,
    pub project_root: PathBuf,
    pub files: Vec<FileResult>,
    pub conflicts: Vec<Conflict>,
    pub skipped: Vec<SkippedFile>,
    /// Every unfixable violation surfaced during the run. Populated for
    /// `apply` / `diff` / `check unfixable` / `check all` reporting.
    pub unfixable: Vec<UnfixableDetail>,
    /// Non-fatal advisory messages (e.g. duplicate-literal-element warnings
    /// in config check; per-line lex parse skip notes). Always printed; do
    /// not affect exit codes.
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct FileResult {
    pub relpath: PathBuf,
    pub original: String,
    /// `Some(_)` only when the file accumulated at least one applied edit.
    pub rewritten: Option<String>,
    pub include_results: Vec<IncludeResult>,
    /// `true` when the original file started with a UTF-8 BOM. The BOM
    /// is stripped from `original` and `rewritten`; the apply step writes
    /// it back.
    pub had_bom: bool,
    /// Per-line lex warnings (unterminated quote/angle, `#include<id>`
    /// token, etc.) produced while scanning this file. Lifted into
    /// `Summary.warnings` after the parallel walk.
    pub lex_warnings: Vec<String>,
}

#[derive(Debug)]
pub struct SkippedFile {
    pub relpath: PathBuf,
    pub reason: String,
}

#[derive(Debug)]
pub struct IncludeResult {
    pub include: Include,
    pub outcome: IncludeOutcome,
}

#[derive(Debug)]
pub enum IncludeOutcome {
    /// No rule matched this include.
    NoMatch,
    /// All matched rules agreed on `Keep` — no edit.
    Keep { rules: Vec<String> },
    /// All matched rules agreed on the same rewrite text.
    Rewritten {
        rules: Vec<String>,
        edit_range: Range<usize>,
        new_text: String,
    },
    /// One of the matched rules produced an `action.error`. Exit code 2.
    Error { rule: String, message: String },
    /// One of the matched rules' `trailing_comment.transform.action`
    /// produced an `error`. Exit code 3 (unfixable).
    TrailingCommentError { rule: String, message: String },
    /// Action evaluation failed (resolve missed, multiple matches, etc.).
    EvaluationFailure { rule: String, message: String },
    /// Matched rules disagreed on the final text. Exit code 3.
    Conflict {
        rule_outputs: Vec<(String, String)>,
        differing_aspects: Vec<DiffAspect>,
    },
}

/// Which sub-part of the final include line differs across the rules
/// that produced a conflict. Computed at conflict-detection time by
/// parsing each rule's candidate final-line text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffAspect {
    /// Path inside the quotes / angles.
    IncludePath,
    /// Quote vs angle (Macro forms are excluded from rewriting and
    /// won't reach conflict detection).
    OutputForm,
    /// Trailing comment text (after the close-quote / `>`).
    TrailingComment,
}

/// A conflict surfaced for a specific include (final-text disagreement).
#[derive(Debug)]
pub struct Conflict {
    pub file_relpath: PathBuf,
    pub include_line: usize,
    pub include_text: String,
    /// Per-rule final-line text (the bytes that rule would have written).
    pub rule_outputs: Vec<(String, String)>,
    pub differing_aspects: Vec<DiffAspect>,
}

/// A categorized unfixable detail aggregated for the apply / diff / check
/// reports. Per refactor.md §"inclean apply": "文件路径、行号、原始
/// #include 行、违规类型、触发的规则名称".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnfixableKind {
    Error,
    EvaluationFailure,
    Conflict,
    TrailingCommentError,
}

#[derive(Debug)]
pub struct UnfixableDetail {
    pub file_relpath: PathBuf,
    pub line: usize,
    /// Original `#include` line in the source file, including the
    /// trailing comment (no terminating newline).
    pub original_line: String,
    pub kind: UnfixableKind,
    /// `(rule_name, final_text)` for every participating rule. For
    /// `Conflict`, all participating rules with their candidate final
    /// texts. For the other kinds, the single triggering rule with
    /// `final_text = None`.
    pub rules: Vec<(String, Option<String>)>,
    pub differing_aspects: Vec<DiffAspect>,
    pub message: Option<String>,
}

// ---- Entry point ---------------------------------------------------------

/// Run the pipeline.
///
/// - `config_path`: when `Some`, load that file directly; when `None`,
///   walk upward from `start_dir` to find the nearest `inclean.toml`.
/// - `start_dir`: where the upward walk begins (used only when
///   `config_path` is `None`); also the cwd-relative anchor for any
///   `paths` filter entries.
/// - `paths`: when non-empty, restricts processing to source files
///   rooted at one of these paths (file or directory; resolved relative
///   to the current working directory). `CheckMode::Config` ignores it.
/// - `jobs`: when `Some(n)`, install a rayon global thread pool of that
///   size. Best-effort: a second call with a different `n` is a no-op
///   because rayon's global pool is set-once.
pub fn run(
    config_path: Option<&Path>,
    start_dir: &Path,
    paths: &[PathBuf],
    jobs: Option<usize>,
    mode: CheckMode,
) -> Result<Summary> {
    let resolved_config_path: PathBuf = match config_path {
        Some(p) => {
            if !p.is_file() {
                anyhow::bail!("--config path does not point at a file: {}", p.display(),);
            }
            p.to_path_buf()
        }
        None => discover::find_root_config(start_dir)?,
    };
    let cfg = discover::load_root_config(&resolved_config_path)?;
    let project_root_abs = discover::resolve_project_root(&resolved_config_path, &cfg.raw.project)?;
    let resolved = copy::resolve(std::slice::from_ref(&cfg))?;

    let duplicate_warnings = collect_duplicate_literal_warnings(&cfg);
    let compiled = compile_rules(&resolved)?;

    if mode == CheckMode::Config {
        return Ok(Summary {
            mode,
            project_root: project_root_abs,
            files: Vec::new(),
            conflicts: Vec::new(),
            skipped: Vec::new(),
            unfixable: Vec::new(),
            warnings: duplicate_warnings,
        });
    }

    install_thread_pool(jobs);

    let path_filter = build_path_filter(&project_root_abs, paths)?;

    // Walk + filter + sort candidate files.
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in source_files(&project_root_abs) {
        let entry = entry?;
        let relpath = entry
            .strip_prefix(&project_root_abs)
            .unwrap_or(&entry)
            .to_path_buf();
        if !path_filter.matches(&relpath) {
            continue;
        }
        if any_rule_eligible(&compiled, &relpath) {
            candidates.push(relpath);
        }
    }
    candidates.sort();

    type PerFile = Result<FileResult, SkippedFile>;
    let per_file: Vec<PerFile> = candidates
        .par_iter()
        .map(|relpath| process_file_outer(&compiled, &project_root_abs, relpath))
        .collect();

    let mut files: Vec<FileResult> = Vec::with_capacity(per_file.len());
    let mut skipped: Vec<SkippedFile> = Vec::new();
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut unfixable: Vec<UnfixableDetail> = Vec::new();
    for res in per_file {
        match res {
            Ok(file_result) => {
                // Pull conflicts + unfixable details out of include_results.
                for r in &file_result.include_results {
                    let original_line = source_line_for(&file_result.original, r.include.line);
                    match &r.outcome {
                        IncludeOutcome::Conflict {
                            rule_outputs,
                            differing_aspects,
                        } => {
                            conflicts.push(Conflict {
                                file_relpath: file_result.relpath.clone(),
                                include_line: r.include.line,
                                include_text: format_include_text(&r.include),
                                rule_outputs: rule_outputs.clone(),
                                differing_aspects: differing_aspects.clone(),
                            });
                            unfixable.push(UnfixableDetail {
                                file_relpath: file_result.relpath.clone(),
                                line: r.include.line,
                                original_line,
                                kind: UnfixableKind::Conflict,
                                rules: rule_outputs
                                    .iter()
                                    .map(|(n, t)| (n.clone(), Some(t.clone())))
                                    .collect(),
                                differing_aspects: differing_aspects.clone(),
                                message: None,
                            });
                        }
                        IncludeOutcome::Error { rule, message } => {
                            unfixable.push(UnfixableDetail {
                                file_relpath: file_result.relpath.clone(),
                                line: r.include.line,
                                original_line,
                                kind: UnfixableKind::Error,
                                rules: vec![(rule.clone(), None)],
                                differing_aspects: vec![],
                                message: Some(message.clone()),
                            });
                        }
                        IncludeOutcome::TrailingCommentError { rule, message } => {
                            unfixable.push(UnfixableDetail {
                                file_relpath: file_result.relpath.clone(),
                                line: r.include.line,
                                original_line,
                                kind: UnfixableKind::TrailingCommentError,
                                rules: vec![(rule.clone(), None)],
                                differing_aspects: vec![],
                                message: Some(message.clone()),
                            });
                        }
                        IncludeOutcome::EvaluationFailure { rule, message } => {
                            unfixable.push(UnfixableDetail {
                                file_relpath: file_result.relpath.clone(),
                                line: r.include.line,
                                original_line,
                                kind: UnfixableKind::EvaluationFailure,
                                rules: vec![(rule.clone(), None)],
                                differing_aspects: vec![],
                                message: Some(message.clone()),
                            });
                        }
                        _ => {}
                    }
                }
                files.push(file_result);
            }
            Err(s) => skipped.push(s),
        }
    }

    // Lift per-file lex warnings into the top-level warnings vec.
    let mut warnings = duplicate_warnings;
    for f in &files {
        warnings.extend(f.lex_warnings.iter().cloned());
    }

    Ok(Summary {
        mode,
        project_root: project_root_abs,
        files,
        conflicts,
        skipped,
        unfixable,
        warnings,
    })
}

/// Walk every rule's raw array fields for duplicates among elements the
/// user typed *literally* in this rule (i.e. excluding any `${copied}`
/// splat token). Per refactor.md §"inclean config check": `${copied}`
/// splat-expanded duplicates are intentional and never warn.
fn collect_duplicate_literal_warnings(cfg: &crate::config::schema::LoadedConfig) -> Vec<String> {
    use std::collections::HashSet;
    let mut out: Vec<String> = Vec::new();
    for raw in &cfg.raw.rules {
        check_list_dup(&raw.name, "file_paths", raw.file_paths.as_deref(), &mut out);
        check_list_dup(
            &raw.name,
            "file_suffixes",
            raw.file_suffixes.as_deref(),
            &mut out,
        );
        check_list_dup(
            &raw.name,
            "include_match",
            raw.include_match.as_deref(),
            &mut out,
        );
        check_list_dup(
            &raw.name,
            "include_directories",
            raw.include_directories.as_deref(),
            &mut out,
        );
        if let Some(forms) = raw.include_forms.as_ref() {
            let mut seen = HashSet::new();
            for f in forms {
                let key = format!("{f:?}");
                if !seen.insert(key.clone()) {
                    out.push(format!(
                        "warning: rule '{}': duplicate literal element '{}' in include_forms",
                        raw.name,
                        format!("{f:?}").to_lowercase(),
                    ));
                }
            }
        }
    }
    out
}

fn check_list_dup(rule_name: &str, field: &str, list: Option<&[String]>, out: &mut Vec<String>) {
    use std::collections::HashSet;
    let Some(v) = list else { return };
    let mut seen: HashSet<&str> = HashSet::new();
    for elem in v {
        if elem == "${copied}" {
            // Splat token: never counted; per-spec the splat-expanded
            // copies coming from the parent are intentional.
            continue;
        }
        if !seen.insert(elem.as_str()) {
            out.push(format!(
                "warning: rule '{rule_name}': duplicate literal element '{elem}' in {field}"
            ));
        }
    }
}

/// Slice the source's physical line for diagnostics. 1-based line number.
fn source_line_for(source: &str, line: usize) -> String {
    if line == 0 {
        return String::new();
    }
    source.lines().nth(line - 1).unwrap_or("").to_string()
}

/// Apply rewrites to disk.
///
/// Per refactor.md §"inclean apply": when unfixable violations coexist
/// with fixable rewrites, the fixable parts are written; files that
/// contain *any* unfixable outcome (Error / TrailingCommentError /
/// EvaluationFailure / Conflict) are skipped entirely (no partial
/// writes per file). The caller (`cli::apply`) then prints a separate
/// unfixable report from `summary.unfixable`.
pub fn apply(summary: &Summary) -> Result<usize> {
    let mut written = 0usize;
    for f in &summary.files {
        if file_has_errors(f) {
            continue;
        }
        let Some(new) = &f.rewritten else { continue };
        let path = summary.project_root.join(&f.relpath);
        let mut bytes: Vec<u8> = Vec::with_capacity(new.len() + 3);
        if f.had_bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        bytes.extend_from_slice(new.as_bytes());
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

pub fn file_has_errors(f: &FileResult) -> bool {
    f.include_results.iter().any(|r| {
        matches!(
            r.outcome,
            IncludeOutcome::Error { .. }
                | IncludeOutcome::TrailingCommentError { .. }
                | IncludeOutcome::EvaluationFailure { .. }
                | IncludeOutcome::Conflict { .. }
        )
    })
}

/// Render the unfixable report for human consumption. Each entry
/// includes file path, line, original `#include` line, violation kind,
/// triggering rule name(s), and (for conflicts) the differing aspects.
/// Empty string when there are no unfixable entries.
pub fn render_unfixable_report(summary: &Summary) -> String {
    if summary.unfixable.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "{} unfixable violation(s):\n",
        summary.unfixable.len()
    ));
    for u in &summary.unfixable {
        let kind = match u.kind {
            UnfixableKind::Error => "error",
            UnfixableKind::EvaluationFailure => "evaluation_failure",
            UnfixableKind::Conflict => "conflict",
            UnfixableKind::TrailingCommentError => "trailing_comment_error",
        };
        out.push_str(&format!(
            "  {}:{}: {kind}\n",
            u.file_relpath.display(),
            u.line
        ));
        out.push_str(&format!("    original: {}\n", u.original_line));
        if let Some(msg) = &u.message {
            out.push_str(&format!("    message:  {msg}\n"));
        }
        for (rule, final_text) in &u.rules {
            match final_text {
                Some(text) => {
                    // Per refactor.md §"规则冲突": show the per-rule final
                    // line with `#include ` reattached so the diagnostic
                    // reads as the bytes that rule would write.
                    out.push_str(&format!("    rule `{rule}`: #include {text}\n"))
                }
                None => out.push_str(&format!("    rule `{rule}`\n")),
            }
        }
        if !u.differing_aspects.is_empty() {
            let parts: Vec<&str> = u
                .differing_aspects
                .iter()
                .map(|a| match a {
                    DiffAspect::IncludePath => "include path",
                    DiffAspect::OutputForm => "output_form",
                    DiffAspect::TrailingComment => "trailing_comment",
                })
                .collect();
            out.push_str(&format!("    differs in: {}\n", parts.join(", ")));
        }
    }
    out
}

/// Render a unified diff for every changed file in `summary`.
pub fn render_diff(summary: &Summary) -> String {
    use similar::TextDiff;
    let mut out = String::new();
    for f in &summary.files {
        let Some(new) = &f.rewritten else { continue };
        let diff = TextDiff::from_lines(&f.original, new);

        use crate::utils::PathExt;
        let path_str = f.relpath.to_slash();
        let a_label = format!("a/{}", path_str);
        let b_label = format!("b/{}", path_str);
        let body = diff
            .unified_diff()
            .context_radius(3)
            .header(&a_label, &b_label)
            .to_string();
        out.push_str(&body);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Highest-severity outcome across the whole summary:
///   0 = clean / only NoMatch+Keep+Rewritten
///   2 = any `action.type = "error"` matched
///   3 = any Conflict / EvaluationFailure
pub fn summary_exit_code(summary: &Summary) -> u8 {
    let mut code: u8 = 0;
    if !summary.conflicts.is_empty() {
        code = code.max(3);
    }
    for f in &summary.files {
        for r in &f.include_results {
            match &r.outcome {
                IncludeOutcome::Error { .. } => code = code.max(2),
                IncludeOutcome::EvaluationFailure { .. } => code = code.max(3),
                IncludeOutcome::TrailingCommentError { .. } => code = code.max(3),
                IncludeOutcome::Conflict { .. } => code = code.max(3),
                _ => {}
            }
        }
    }
    code
}

// ---- per-file processing -------------------------------------------------

fn process_file_outer(
    rules: &[CompiledRule<'_>],
    project_root: &Path,
    relpath: &Path,
) -> std::result::Result<FileResult, SkippedFile> {
    let abs = project_root.join(relpath);
    let bytes = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) => {
            return Err(SkippedFile {
                relpath: relpath.to_path_buf(),
                reason: format!("read failed: {e}"),
            });
        }
    };
    let (had_bom, body) = strip_bom(&bytes);
    let original = match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => {
            return Err(SkippedFile {
                relpath: relpath.to_path_buf(),
                reason: "not valid UTF-8".to_string(),
            });
        }
    };
    let processed = process_file(rules, relpath, &original, project_root);
    Ok(FileResult {
        relpath: relpath.to_path_buf(),
        original,
        rewritten: processed.rewritten,
        include_results: processed.include_results,
        had_bom,
        lex_warnings: processed.lex_warnings,
    })
}

fn strip_bom(bytes: &[u8]) -> (bool, &[u8]) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (true, &bytes[3..])
    } else {
        (false, bytes)
    }
}

struct FileProcessing {
    rewritten: Option<String>,
    include_results: Vec<IncludeResult>,
    lex_warnings: Vec<String>,
}

fn process_file(
    rules: &[CompiledRule<'_>],
    relpath: &Path,
    original: &str,
    project_root: &Path,
) -> FileProcessing {
    let (includes, report) = include_line::scan_with_report(original);
    let mut lex_warnings: Vec<String> = Vec::new();
    for (line, reason) in &report.skipped_lines {
        lex_warnings.push(format!("{}:{}: {reason}", relpath.display(), line));
    }
    let line_table = include_line::line_table(original);
    let suppressed = engine::compute_all_suppressed(rules, original, &line_table);

    let mut include_results: Vec<IncludeResult> = Vec::with_capacity(includes.len());
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();

    for include in includes {
        let matched = engine::match_all(rules, relpath, &include, &suppressed, project_root);

        if matched.matched.is_empty() && matched.failures.is_empty() {
            include_results.push(IncludeResult {
                include,
                outcome: IncludeOutcome::NoMatch,
            });
            continue;
        }

        // Evaluate every matched rule; action and trailing-comment policy
        // contribute independent conflict candidates.
        let mut evaluations: Vec<RuleEvaluation> = matched
            .failures
            .iter()
            .map(|failure| RuleEvaluation {
                rule_name: failure.rule.rule.name.clone(),
                action: ActionOutcome::EvaluationFailure {
                    message: failure.message.clone(),
                },
                trailing: TrailingOutcome::Skip,
            })
            .collect();

        evaluations.extend(matched.matched.iter().map(|cm| {
            let action = action::evaluate_action_with_resolution(
                cm.rule,
                &include,
                original,
                relpath,
                project_root,
                cm.resolved_header.as_deref(),
            );
            let trailing = action::evaluate_trailing(cm.rule, &include, original, relpath);
            RuleEvaluation {
                rule_name: cm.rule.rule.name.clone(),
                action,
                trailing,
            }
        }));

        let outcome = collapse_outcomes(&include, original, evaluations);

        // Push edit when an unambiguous Rewrite came out.
        if let IncludeOutcome::Rewritten {
            edit_range,
            new_text,
            ..
        } = &outcome
        {
            edits.push((edit_range.clone(), new_text.clone()));
        }

        include_results.push(IncludeResult { include, outcome });
    }

    let rewritten = if edits.is_empty() {
        None
    } else {
        Some(apply_edits(original, &edits))
    };

    FileProcessing {
        rewritten,
        include_results,
        lex_warnings,
    }
}

struct RuleEvaluation {
    rule_name: String,
    action: ActionOutcome,
    trailing: TrailingOutcome,
}

struct CollapsedAction {
    edit_range: Range<usize>,
    new_text: String,
    rules: Vec<String>,
}

struct CollapsedTrailing {
    new_text: String,
    rules: Vec<String>,
}

/// Conflict rules:
///
/// 1. Any `action.error` → `IncludeOutcome::Error`.
/// 2. Any `trailing_comment.transform.action.error` →
///    `IncludeOutcome::TrailingCommentError`.
/// 3. Any action evaluation failure → propagate.
/// 4. Compare action candidates only among non-`skip` action fields.
/// 5. Compare trailing-comment candidates only among non-`skip`
///    trailing_comment fields.
/// 6. Compose the agreed action and trailing-comment pieces into one edit.
fn collapse_outcomes(
    include: &Include,
    source: &str,
    evaluations: Vec<RuleEvaluation>,
) -> IncludeOutcome {
    for ev in &evaluations {
        if let ActionOutcome::Error { message } = &ev.action {
            return IncludeOutcome::Error {
                rule: ev.rule_name.clone(),
                message: message.clone(),
            };
        }
    }
    for ev in &evaluations {
        if let TrailingOutcome::Error { message } = &ev.trailing {
            return IncludeOutcome::TrailingCommentError {
                rule: ev.rule_name.clone(),
                message: message.clone(),
            };
        }
    }
    for ev in &evaluations {
        if let ActionOutcome::EvaluationFailure { message } = &ev.action {
            return IncludeOutcome::EvaluationFailure {
                rule: ev.rule_name.clone(),
                message: message.clone(),
            };
        }
    }

    let mut action_candidates: Vec<(String, Range<usize>, String)> = Vec::new();
    let mut trailing_candidates: Vec<(String, String)> = Vec::new();
    for ev in &evaluations {
        match &ev.action {
            ActionOutcome::Apply {
                edit_range,
                new_text,
            } => {
                action_candidates.push((ev.rule_name.clone(), edit_range.clone(), new_text.clone()))
            }
            ActionOutcome::Skip
            | ActionOutcome::Error { .. }
            | ActionOutcome::EvaluationFailure { .. } => {}
        }
        match &ev.trailing {
            TrailingOutcome::Apply { new_text } => {
                trailing_candidates.push((ev.rule_name.clone(), new_text.clone()));
            }
            TrailingOutcome::Skip | TrailingOutcome::Error { .. } => {}
        }
    }

    let action = match collapse_action_candidates(include, source, action_candidates) {
        Ok(action) => action,
        Err(conflict) => return conflict,
    };
    let trailing = match collapse_trailing_candidates(include, source, &action, trailing_candidates)
    {
        Ok(trailing) => trailing,
        Err(conflict) => return conflict,
    };

    let original_trailing = &source[include.trailing_range.clone()];
    if action.edit_range != include.argument_range {
        if !trailing.rules.is_empty() && trailing.new_text != original_trailing {
            let mut rule_outputs: Vec<(String, String)> = action
                .rules
                .iter()
                .map(|rule| (rule.clone(), action.new_text.clone()))
                .collect();
            rule_outputs.extend(
                trailing
                    .rules
                    .iter()
                    .map(|rule| (rule.clone(), trailing.new_text.clone())),
            );
            return IncludeOutcome::Conflict {
                rule_outputs,
                differing_aspects: vec![DiffAspect::TrailingComment],
            };
        }
        return rewritten_or_keep(source, action.edit_range, action.new_text, action.rules);
    }

    let edit_range = argument_and_trailing_range(include);
    let new_text = format!("{}{}", action.new_text, trailing.new_text);
    let rules = merge_rules(action.rules, trailing.rules);
    rewritten_or_keep(source, edit_range, new_text, rules)
}

fn collapse_action_candidates(
    include: &Include,
    source: &str,
    candidates: Vec<(String, Range<usize>, String)>,
) -> std::result::Result<CollapsedAction, IncludeOutcome> {
    if candidates.is_empty() {
        return Ok(CollapsedAction {
            edit_range: include.argument_range.clone(),
            new_text: source[include.argument_range.clone()].to_string(),
            rules: Vec::new(),
        });
    }
    let (first_range, first_text) = (candidates[0].1.clone(), candidates[0].2.clone());
    let all_same = candidates
        .iter()
        .all(|(_, r, t)| *r == first_range && *t == first_text);
    if all_same {
        return Ok(CollapsedAction {
            edit_range: first_range,
            new_text: first_text,
            rules: candidates.into_iter().map(|(n, _, _)| n).collect(),
        });
    }

    let rule_outputs: Vec<(String, String)> = candidates
        .into_iter()
        .map(|(name, range, text)| {
            let display = action_candidate_display(include, source, &range, &text);
            (name, display)
        })
        .collect();
    let differing_aspects = compute_differing_aspects(
        &rule_outputs
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>(),
    );
    Err(IncludeOutcome::Conflict {
        rule_outputs,
        differing_aspects,
    })
}

fn collapse_trailing_candidates(
    include: &Include,
    source: &str,
    action: &CollapsedAction,
    candidates: Vec<(String, String)>,
) -> std::result::Result<CollapsedTrailing, IncludeOutcome> {
    if candidates.is_empty() {
        return Ok(CollapsedTrailing {
            new_text: source[include.trailing_range.clone()].to_string(),
            rules: Vec::new(),
        });
    }
    let first_text = candidates[0].1.clone();
    if candidates.iter().all(|(_, t)| *t == first_text) {
        return Ok(CollapsedTrailing {
            new_text: first_text,
            rules: candidates.into_iter().map(|(n, _)| n).collect(),
        });
    }

    let action_arg = if action.edit_range == include.argument_range {
        action.new_text.as_str()
    } else {
        &source[include.argument_range.clone()]
    };
    let rule_outputs: Vec<(String, String)> = candidates
        .into_iter()
        .map(|(name, trailing)| (name, format!("{action_arg}{trailing}")))
        .collect();
    Err(IncludeOutcome::Conflict {
        rule_outputs,
        differing_aspects: vec![DiffAspect::TrailingComment],
    })
}

fn action_candidate_display(
    include: &Include,
    source: &str,
    range: &Range<usize>,
    text: &str,
) -> String {
    if *range == include.argument_range {
        format!("{text}{}", &source[include.trailing_range.clone()])
    } else {
        text.to_string()
    }
}

fn rewritten_or_keep(
    source: &str,
    edit_range: Range<usize>,
    new_text: String,
    rules: Vec<String>,
) -> IncludeOutcome {
    if new_text == source[edit_range.clone()] {
        IncludeOutcome::Keep { rules }
    } else {
        IncludeOutcome::Rewritten {
            rules,
            edit_range,
            new_text,
        }
    }
}

fn merge_rules(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    for rule in left.into_iter().chain(right) {
        if !out.contains(&rule) {
            out.push(rule);
        }
    }
    out
}

/// Parse each rule's final-line text and report which sub-parts diverge.
/// The texts span `[argument_start, line_end_excl_newline)`, so they look
/// like `"lib/foo.h" // comment` or `<lib/foo.h>  /* x */`.
fn compute_differing_aspects(texts: &[&str]) -> Vec<DiffAspect> {
    let parsed: Vec<FinalLineParts> = texts.iter().map(|t| parse_final_line(t)).collect();
    let mut out: Vec<DiffAspect> = Vec::new();
    let any_differ = |proj: fn(&FinalLineParts) -> &str| -> bool {
        let first = proj(&parsed[0]);
        parsed.iter().any(|p| proj(p) != first)
    };
    if any_differ(|p| &p.path) {
        out.push(DiffAspect::IncludePath);
    }
    if any_differ(|p| &p.form) {
        out.push(DiffAspect::OutputForm);
    }
    if any_differ(|p| &p.trailing) {
        out.push(DiffAspect::TrailingComment);
    }
    out
}

struct FinalLineParts {
    /// `"`, `<`, or `M` (macro).
    form: String,
    /// Path between the quotes / angles (or whole text for macros).
    path: String,
    /// Trailing comment text (delimiter included), trimmed of surrounding
    /// whitespace.
    trailing: String,
}

fn parse_final_line(s: &str) -> FinalLineParts {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return FinalLineParts {
            form: "\"".to_string(),
            path: rest[..end].to_string(),
            trailing: rest[end + 1..].trim().to_string(),
        };
    }
    if let Some(rest) = t.strip_prefix('<')
        && let Some(end) = rest.find('>')
    {
        return FinalLineParts {
            form: "<".to_string(),
            path: rest[..end].to_string(),
            trailing: rest[end + 1..].trim().to_string(),
        };
    }
    FinalLineParts {
        form: "M".to_string(),
        path: t.to_string(),
        trailing: String::new(),
    }
}

fn argument_and_trailing_range(include: &Include) -> Range<usize> {
    let start = include.argument_range.start;
    let end = include.trailing_range.end.max(include.argument_range.end);
    start..end
}

// ---- file discovery & filtering ------------------------------------------

fn compile_rules<'a>(rules: &'a [(String, ResolvedRule)]) -> Result<Vec<CompiledRule<'a>>> {
    rules.iter().map(|(_, r)| CompiledRule::new(r)).collect()
}

fn source_files(root: &Path) -> impl Iterator<Item = Result<PathBuf>> {
    // Per refactor.md §Engine: no implicit ignore behavior. Stay flat.
    let walker = WalkBuilder::new(root).standard_filters(false).build();
    walker.filter_map(|res| match res {
        Ok(entry) => {
            if entry.file_type().is_some_and(|ft| ft.is_file())
                && entry.file_name() != CONFIG_FILENAME
            {
                Some(Ok(entry.into_path()))
            } else {
                None
            }
        }
        Err(e) => Some(Err(anyhow::anyhow!(e))),
    })
}

fn any_rule_eligible(rules: &[CompiledRule<'_>], file_relpath: &Path) -> bool {
    rules.iter().any(|r| r.path_matcher.matches(file_relpath))
}

// ---- jobs / paths --------------------------------------------------------

static THREAD_POOL_INIT: Once = Once::new();

fn install_thread_pool(jobs: Option<usize>) {
    let Some(n) = jobs else { return };
    if n == 0 {
        return;
    }
    THREAD_POOL_INIT.call_once(|| {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global();
    });
}

/// A list of project-root-relative path prefixes used to filter the
/// walker output. An empty list matches everything (no filter).
struct PathFilter {
    prefixes: Vec<PathBuf>,
}

impl PathFilter {
    fn empty() -> Self {
        Self { prefixes: vec![] }
    }
    fn matches(&self, relpath: &Path) -> bool {
        if self.prefixes.is_empty() {
            return true;
        }
        self.prefixes.iter().any(|prefix| {
            // Exact-match (file): equality.
            if relpath == prefix {
                return true;
            }
            // Directory prefix: every component of `prefix` must be a
            // prefix of `relpath` and `relpath` must have strictly more
            // components.
            let prefix_components: Vec<_> = prefix.components().collect();
            let rel_components: Vec<_> = relpath.components().collect();
            if prefix_components.len() >= rel_components.len() {
                return false;
            }
            prefix_components
                .iter()
                .zip(rel_components.iter())
                .all(|(a, b)| a == b)
        })
    }
}

/// Normalize user-supplied `paths` to project-root-relative form.
///
/// Each entry is canonicalized when possible. If a user passes a path
/// outside the project root, return an error citing the offending path.
fn build_path_filter(project_root: &Path, paths: &[PathBuf]) -> Result<PathFilter> {
    if paths.is_empty() {
        return Ok(PathFilter::empty());
    }
    let root_canon = std::fs::canonicalize(project_root)
        .with_context(|| format!("canonicalize project root {}", project_root.display()))?;
    let cwd = std::env::current_dir().context("read current working directory")?;
    let mut prefixes: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        let absolute = if p.is_absolute() {
            p.clone()
        } else {
            cwd.join(p)
        };
        let canon = std::fs::canonicalize(&absolute)
            .with_context(|| format!("path filter entry {} does not exist", absolute.display()))?;
        let rel = canon.strip_prefix(&root_canon).map_err(|_| {
            anyhow::anyhow!(
                "path filter entry {} is outside the project root {}",
                canon.display(),
                root_canon.display(),
            )
        })?;
        prefixes.push(rel.to_path_buf());
    }
    Ok(PathFilter { prefixes })
}

fn format_include_text(include: &Include) -> String {
    match include.form {
        IncludeForm::Quote => format!("\"{}\"", include.content),
        IncludeForm::Angle => format!("<{}>", include.content),
        IncludeForm::Macro => include.content.clone(),
    }
}

fn apply_edits(original: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut edits_sorted: Vec<_> = edits.to_vec();
    edits_sorted.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = original.to_string();
    for (range, new) in edits_sorted {
        out.replace_range(range, &new);
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::utils::testing::fs::TmpProject;

    use super::*;
    use std::fs;

    #[test]
    fn config_mode_skips_source_scan() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            action = { type = "keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", [0xFF, 0xFF, 0xFE, b'\n']);

        let summary = run(None, proj.path(), &[], None, CheckMode::Config).unwrap();
        assert!(summary.files.is_empty());
        assert!(summary.conflicts.is_empty());
        assert!(summary.skipped.is_empty());
        assert_eq!(summary_exit_code(&summary), 0);
    }

    fn config_compile_error(rule: &str) -> String {
        let proj = TmpProject::create_with_rules(rule);
        let err = run(None, proj.path(), &[], None, CheckMode::Config).unwrap_err();
        format!("{err:#}")
    }

    #[test]
    fn config_mode_compiles_file_path_globs() {
        let err = config_compile_error(
            r#"
            [[rule]]
            name = "base"
            file_paths = ["["]
            "#,
        );
        assert!(err.contains("file_paths/file_suffixes compile"), "{err}");
    }

    #[test]
    fn config_mode_compiles_include_match_globs() {
        let err = config_compile_error(
            r#"
            [[rule]]
            name = "base"
            include_match = ["["]
            "#,
        );
        assert!(err.contains("include_match glob"), "{err}");
    }

    #[test]
    fn config_mode_compiles_suppression_regexes() {
        let err = config_compile_error(
            r#"
            [[rule]]
            name = "base"
            suppression_comments_regex = { line = "(" }
            "#,
        );
        assert!(
            err.contains("suppression_comments_regex.line compile"),
            "{err}"
        );
    }

    #[test]
    fn config_mode_compiles_trailing_comment_regexes() {
        let err = config_compile_error(
            r#"
            [[rule]]
            name = "base"
            trailing_comment = {
                transform = {
                    content_regex = "(",
                    action = { type = "keep" },
                },
            }
            "#,
        );
        assert!(
            err.contains("trailing_comment.transform.content_regex compile"),
            "{err}"
        );
    }

    #[test]
    fn keep_action_produces_no_edits() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            action = { type = "keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        let f = &summary.files[0];
        assert!(matches!(
            f.include_results[0].outcome,
            IncludeOutcome::Keep { .. }
        ));
        assert!(f.rewritten.is_none());
    }

    #[test]
    fn omitted_action_and_trailing_comment_default_to_skip() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        let f = &summary.files[0];
        assert!(matches!(
            f.include_results[0].outcome,
            IncludeOutcome::Keep { .. }
        ));
        assert!(summary.conflicts.is_empty());
        assert!(f.rewritten.is_none());
    }

    #[test]
    fn trailing_comment_can_run_without_explicit_action() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            trailing_comment = { append_if_absent = "  // IWYU: keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"foo.h\"  // IWYU: keep");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn open_block_trailing_prevents_line_comment_append_after_action_rewrite() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "lib/${original}" }
            trailing_comment = { append_if_absent = "  // IWYU: keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write(
            "src/main.c",
            "#include \"foo.h\" /* opens\nstill comment */\n",
        );

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"lib/foo.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        apply(&summary).unwrap();
        let new = fs::read_to_string(proj.path().join("src/main.c")).unwrap();
        assert_eq!(new, "#include \"lib/foo.h\" /* opens\nstill comment */\n");
    }

    #[test]
    fn replace_action_writes_back() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        let f = &summary.files[0];
        assert!(matches!(
            f.include_results[0].outcome,
            IncludeOutcome::Rewritten { .. }
        ));
        let written = apply(&summary).unwrap();
        assert_eq!(written, 1);
        let new = fs::read_to_string(proj.path().join("src/main.c")).unwrap();
        assert!(new.contains("\"lib/foo.h\""));
    }

    #[test]
    fn error_action_produces_error_outcome_and_exit_2() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_match = ["old.h"]
            action = { type = "error", message = "deprecated" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"old.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(matches!(
            summary.files[0].include_results[0].outcome,
            IncludeOutcome::Error { .. }
        ));
        assert_eq!(summary_exit_code(&summary), 2);
    }

    #[test]
    fn include_forms_replaces_match_forms_for_form_matching() {
        let rule = r#"
            [[rule]]
            name = "angle-only"
            file_paths = ["src/**/*"]
            include_forms = ["angle"]
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n#include <bar.h>\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(matches!(
            summary.files[0].include_results[0].outcome,
            IncludeOutcome::NoMatch
        ));
        assert!(matches!(
            summary.files[0].include_results[1].outcome,
            IncludeOutcome::Rewritten { .. }
        ));
    }

    #[test]
    fn unresolved_include_defaults_to_error_when_directories_are_configured() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["include"]
            action = { type = "keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"missing.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::EvaluationFailure { rule, message } => {
                assert_eq!(rule, "base");
                assert!(message.contains("no include_directories entry contains"));
                assert!(message.contains("missing.h"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn unresolved_include_can_skip_rule() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["include"]
            include_on_unresolved = "skip"
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"missing.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(matches!(
            summary.files[0].include_results[0].outcome,
            IncludeOutcome::NoMatch
        ));
        assert!(summary.files[0].rewritten.is_none());
    }

    #[test]
    fn unresolved_include_can_allow_non_resolve_action() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["include"]
            include_on_unresolved = "allow"
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"missing.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"lib/missing.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn ambiguous_include_defaults_to_error_when_directories_are_configured() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["first", "second"]
            action = { type = "keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");
        proj.write("first/foo.h", "");
        proj.write("second/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::EvaluationFailure { rule, message } => {
                assert_eq!(rule, "base");
                assert!(message.contains("multiple include_directories"));
                assert!(message.contains("first"));
                assert!(message.contains("second"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn same_header_reached_through_multiple_directories_is_not_ambiguous() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["include", "include/."]
            action = { type = "resolve", relative_to = "." }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");
        proj.write("include/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"../include/foo.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn parent_segments_in_include_path_are_normalized_before_resolve() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["Core/Src/**/*"]
            include_directories = ["Core/Inc", "Core/Src"]
            action = { type = "resolve", relative_to = "Core" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("Core/Src/main.c", "#include \"../Inc/foo.h\"\n");
        proj.write("Core/Inc/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"Inc/foo.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn ambiguous_include_can_skip_rule() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["first", "second"]
            include_on_ambiguous = "skip"
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");
        proj.write("first/foo.h", "");
        proj.write("second/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(matches!(
            summary.files[0].include_results[0].outcome,
            IncludeOutcome::NoMatch
        ));
        assert!(summary.files[0].rewritten.is_none());
    }

    #[test]
    fn ambiguous_include_can_use_first_directory_for_resolve() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["first", "second"]
            include_on_ambiguous = "first"
            action = { type = "resolve", relative_to = "." }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");
        proj.write("first/foo.h", "");
        proj.write("second/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"../first/foo.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn conflicting_rules_produce_conflict_and_exit_3() {
        // Two rules that both match but rewrite to different texts.
        let rules = r#"
            [[rule]]
            name = "a"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "A/foo.h" }
            [[rule]]
            name = "b"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "B/foo.h" }
        "#;
        let proj = TmpProject::create_with_rules(rules);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert_eq!(summary.conflicts.len(), 1);
        assert_eq!(summary_exit_code(&summary), 3);
        assert_eq!(summary.unfixable.len(), 1);
        // apply does NOT refuse — the file is skipped (it had conflict),
        // and 0 files get written. fixable parts of OTHER files would be.
        let written = apply(&summary).unwrap();
        assert_eq!(written, 0);
        // Existing source was not overwritten.
        let body = fs::read_to_string(proj.path().join("src/main.c")).unwrap();
        assert_eq!(body, "#include \"foo.h\"\n");
    }

    #[test]
    fn agreeing_rules_collapse_to_a_single_rewrite() {
        // Two rules that both rewrite to the same text: no conflict.
        let rules = r#"
            [[rule]]
            name = "a"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "new/foo.h" }

            [[rule]]
            name = "b"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "new/foo.h" }
        "#;
        let proj = TmpProject::create_with_rules(rules);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(summary.conflicts.is_empty());
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten {
                rules, new_text, ..
            } => {
                assert_eq!(new_text, "\"new/foo.h\"");
                assert_eq!(rules.len(), 2);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn action_and_trailing_comment_skip_compose_without_conflict() {
        let rules = r#"
            [[rule]]
            name = "path"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "lib/${original}" }
            trailing_comment = "skip"

            [[rule]]
            name = "comment"
            file_paths = ["src/**/*"]
            action = "skip"
            trailing_comment = { append_if_absent = "  // IWYU: export" }
        "#;
        let proj = TmpProject::create_with_rules(rules);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(summary.conflicts.is_empty());
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten {
                rules, new_text, ..
            } => {
                assert_eq!(rules, &vec!["path".to_string(), "comment".to_string()]);
                assert_eq!(new_text, "\"lib/foo.h\"  // IWYU: export");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_conflict_is_checked_separately() {
        let rules = r#"
            [[rule]]
            name = "a"
            file_paths = ["src/**/*"]
            action = { type = "keep" }
            trailing_comment = { append_if_absent = "  // A" }

            [[rule]]
            name = "b"
            file_paths = ["src/**/*"]
            action = { type = "keep" }
            trailing_comment = { append_if_absent = "  // B" }
        "#;
        let proj = TmpProject::create_with_rules(rules);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert_eq!(summary.conflicts.len(), 1);
        assert_eq!(
            summary.conflicts[0].differing_aspects,
            vec![DiffAspect::TrailingComment]
        );
    }

    #[test]
    fn bom_is_preserved_across_apply() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        // \u{FEFF} is the BOM in source. We write raw bytes to ensure the
        // BOM is at the very start.
        let bom = [0xEF, 0xBB, 0xBFu8];
        let body = b"#include \"foo.h\"\n";
        let mut payload = Vec::new();
        payload.extend_from_slice(&bom);
        payload.extend_from_slice(body);
        proj.write("src/main.c", &payload);

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(summary.files[0].had_bom);
        apply(&summary).unwrap();
        let read_back = proj.read("src/main.c");
        assert!(read_back.starts_with(&bom));
    }

    #[test]
    fn parse_failure_is_skipped_not_a_hard_error() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            action = { type = "keep" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        // Invalid UTF-8 byte sequence.
        let payload: &[u8] = &[0xFF, 0xFF, 0xFE, b'\n'];
        proj.write("src/bad.c", payload);
        proj.write("src/main.c", "#include \"foo.h\"\n");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert_eq!(summary.skipped.len(), 1);
        assert_eq!(summary.skipped[0].relpath, PathBuf::from("src/bad.c"));
        assert_eq!(summary_exit_code(&summary), 0);
    }
}
