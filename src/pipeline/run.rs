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

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Once;

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::config::copy::{self, ResolvedAction, ResolvedRule};
use crate::config::discover;
use crate::config::schema::{IncludeForm, MacroRewrite};
use crate::lex::include_line::{self, Include};
use crate::lex::macro_define::{self, HeaderMacroDefinition};
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
        /// Primary display edit for the include line. For macro-form
        /// includes this still describes the include use site, while
        /// `edits` may point at the macro definition.
        edit_range: Range<usize>,
        new_text: String,
        /// Actual source edits to apply. Most normal includes have one edit
        /// in the current file; macro includes may edit another file's
        /// `#define` value and optionally the use-site trailing comment.
        edits: Vec<PlannedEdit>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEdit {
    pub relpath: PathBuf,
    pub edit_range: Range<usize>,
    pub new_text: String,
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

    // Walk once. Source candidates obey the user's path filter; macro
    // definitions are indexed from all rule-eligible source files so a
    // filtered run can still report that a needed definition edit is outside
    // the requested path set.
    let mut macro_scan_files: Vec<PathBuf> = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in source_files(&project_root_abs) {
        let entry = entry?;
        let relpath = entry
            .strip_prefix(&project_root_abs)
            .unwrap_or(&entry)
            .to_path_buf();
        if any_rule_eligible(&compiled, &relpath) {
            macro_scan_files.push(relpath.clone());
            if path_filter.matches(&relpath) {
                candidates.push(relpath);
            }
        }
    }
    macro_scan_files.sort();
    candidates.sort();

    let macro_index = build_macro_index(&project_root_abs, &macro_scan_files);

    type PerFile = Result<FileResult, SkippedFile>;
    let per_file: Vec<PerFile> = candidates
        .par_iter()
        .map(|relpath| process_file_outer(&compiled, &project_root_abs, relpath, &macro_index))
        .collect();

    let mut files: Vec<FileResult> = Vec::with_capacity(per_file.len());
    let mut skipped: Vec<SkippedFile> = Vec::new();
    for res in per_file {
        match res {
            Ok(file_result) => files.push(file_result),
            Err(s) => skipped.push(s),
        }
    }

    materialize_missing_edit_targets(&mut files, &project_root_abs)?;
    validate_planned_edits(&mut files, &path_filter);
    apply_planned_edits_to_results(&mut files);

    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut unfixable: Vec<UnfixableDetail> = Vec::new();
    for file_result in &files {
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
            "include_resolved_match",
            raw.include_resolved_match.as_deref(),
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

fn materialize_missing_edit_targets(
    files: &mut Vec<FileResult>,
    project_root: &Path,
) -> Result<()> {
    let existing: BTreeSet<PathBuf> = files.iter().map(|f| f.relpath.clone()).collect();
    let mut missing: BTreeSet<PathBuf> = BTreeSet::new();
    for edit in planned_edits(files) {
        if !existing.contains(&edit.relpath) {
            missing.insert(edit.relpath);
        }
    }

    for relpath in missing {
        let abs = project_root.join(&relpath);
        let bytes = std::fs::read(&abs)
            .with_context(|| format!("reading macro include edit target {}", relpath.display()))?;
        let (had_bom, body) = strip_bom(&bytes);
        let original = std::str::from_utf8(body)
            .with_context(|| {
                format!(
                    "macro include edit target is not UTF-8: {}",
                    relpath.display()
                )
            })?
            .to_string();
        files.push(FileResult {
            relpath,
            original,
            rewritten: None,
            include_results: Vec::new(),
            had_bom,
            lex_warnings: Vec::new(),
        });
    }
    files.sort_by(|a, b| a.relpath.cmp(&b.relpath));
    Ok(())
}

fn validate_planned_edits(files: &mut [FileResult], path_filter: &PathFilter) {
    reject_edits_outside_path_filter(files, path_filter);
    reject_conflicting_planned_edits(files);
}

fn reject_edits_outside_path_filter(files: &mut [FileResult], path_filter: &PathFilter) {
    for file in files {
        for result in &mut file.include_results {
            let IncludeOutcome::Rewritten { rules, edits, .. } = &result.outcome else {
                continue;
            };
            let Some(offending) = edits
                .iter()
                .find(|edit| !path_filter.matches(&edit.relpath))
                .map(|edit| edit.relpath.clone())
            else {
                continue;
            };
            result.outcome = IncludeOutcome::EvaluationFailure {
                rule: rules_label(rules),
                message: format!(
                    "macro include rewrite would edit `{}` outside the requested path filter",
                    offending.display()
                ),
            };
        }
    }
}

#[derive(Clone)]
struct EditEntry {
    relpath: PathBuf,
    range: Range<usize>,
    new_text: String,
    file_idx: usize,
    include_idx: usize,
    label: String,
}

type EditConflictMap = BTreeMap<(usize, usize), (Vec<(String, String)>, Vec<DiffAspect>)>;

fn reject_conflicting_planned_edits(files: &mut [FileResult]) {
    let entries = edit_entries(files);
    let mut conflicts: EditConflictMap = BTreeMap::new();

    let mut by_exact_range: BTreeMap<(PathBuf, usize, usize), Vec<EditEntry>> = BTreeMap::new();
    for entry in &entries {
        by_exact_range
            .entry((entry.relpath.clone(), entry.range.start, entry.range.end))
            .or_default()
            .push(entry.clone());
    }
    for group in by_exact_range.values() {
        let distinct: BTreeSet<&str> = group.iter().map(|entry| entry.new_text.as_str()).collect();
        if distinct.len() > 1 {
            let outputs = group_outputs(group);
            let aspects = differing_aspects_or_path(&outputs);
            for entry in group {
                conflicts
                    .entry((entry.file_idx, entry.include_idx))
                    .or_insert_with(|| (outputs.clone(), aspects.clone()));
            }
        }
    }

    let mut by_file: BTreeMap<PathBuf, Vec<EditEntry>> = BTreeMap::new();
    for entry in entries {
        by_file
            .entry(entry.relpath.clone())
            .or_default()
            .push(entry);
    }
    for mut file_entries in by_file.into_values() {
        file_entries.sort_by_key(|entry| (entry.range.start, entry.range.end));
        for idx in 0..file_entries.len() {
            for next in (idx + 1)..file_entries.len() {
                let left = &file_entries[idx];
                let right = &file_entries[next];
                if right.range.start >= left.range.end {
                    break;
                }
                if left.range.start == right.range.start && left.range.end == right.range.end {
                    continue;
                }
                let group = vec![left.clone(), right.clone()];
                let outputs = group_outputs(&group);
                let aspects = differing_aspects_or_path(&outputs);
                for entry in &group {
                    conflicts
                        .entry((entry.file_idx, entry.include_idx))
                        .or_insert_with(|| (outputs.clone(), aspects.clone()));
                }
            }
        }
    }

    for ((file_idx, include_idx), (rule_outputs, differing_aspects)) in conflicts {
        if let Some(result) = files
            .get_mut(file_idx)
            .and_then(|file| file.include_results.get_mut(include_idx))
        {
            result.outcome = IncludeOutcome::Conflict {
                rule_outputs,
                differing_aspects,
            };
        }
    }
}

fn apply_planned_edits_to_results(files: &mut [FileResult]) {
    let mut by_file: BTreeMap<PathBuf, Vec<(Range<usize>, String)>> = BTreeMap::new();
    let mut seen: BTreeSet<(PathBuf, usize, usize, String)> = BTreeSet::new();
    for edit in planned_edits(files) {
        let key = (
            edit.relpath.clone(),
            edit.edit_range.start,
            edit.edit_range.end,
            edit.new_text.clone(),
        );
        if seen.insert(key) {
            by_file
                .entry(edit.relpath)
                .or_default()
                .push((edit.edit_range, edit.new_text));
        }
    }

    for file in files {
        let Some(edits) = by_file.remove(&file.relpath) else {
            file.rewritten = None;
            continue;
        };
        if edits.is_empty() {
            file.rewritten = None;
        } else {
            file.rewritten = Some(apply_edits(&file.original, &edits));
        }
    }
}

fn planned_edits(files: &[FileResult]) -> Vec<PlannedEdit> {
    let mut out = Vec::new();
    for file in files {
        for result in &file.include_results {
            if let IncludeOutcome::Rewritten { edits, .. } = &result.outcome {
                out.extend(edits.iter().cloned());
            }
        }
    }
    out
}

fn edit_entries(files: &[FileResult]) -> Vec<EditEntry> {
    let mut out = Vec::new();
    for (file_idx, file) in files.iter().enumerate() {
        for (include_idx, result) in file.include_results.iter().enumerate() {
            let IncludeOutcome::Rewritten { rules, edits, .. } = &result.outcome else {
                continue;
            };
            let label = rules_label(rules);
            for edit in edits {
                out.push(EditEntry {
                    relpath: edit.relpath.clone(),
                    range: edit.edit_range.clone(),
                    new_text: edit.new_text.clone(),
                    file_idx,
                    include_idx,
                    label: label.clone(),
                });
            }
        }
    }
    out
}

fn group_outputs(group: &[EditEntry]) -> Vec<(String, String)> {
    group
        .iter()
        .map(|entry| (entry.label.clone(), entry.new_text.clone()))
        .collect()
}

fn differing_aspects_or_path(outputs: &[(String, String)]) -> Vec<DiffAspect> {
    let mut aspects = compute_differing_aspects(
        &outputs
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
    );
    if aspects.is_empty() {
        aspects.push(DiffAspect::IncludePath);
    }
    aspects
}

fn rules_label(rules: &[String]) -> String {
    if rules.is_empty() {
        "macro include".to_string()
    } else {
        rules.join(", ")
    }
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
    macro_index: &MacroIndex,
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
    let processed = process_file(rules, relpath, &original, project_root, macro_index);
    Ok(FileResult {
        relpath: relpath.to_path_buf(),
        original,
        rewritten: None,
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
    include_results: Vec<IncludeResult>,
    lex_warnings: Vec<String>,
}

fn process_file(
    rules: &[CompiledRule<'_>],
    relpath: &Path,
    original: &str,
    project_root: &Path,
    macro_index: &MacroIndex,
) -> FileProcessing {
    let (includes, report) = include_line::scan_with_report(original);
    let mut lex_warnings: Vec<String> = Vec::new();
    for (line, reason) in &report.skipped_lines {
        lex_warnings.push(format!("{}:{}: {reason}", relpath.display(), line));
    }
    let line_table = include_line::line_table(original);
    let suppressed = engine::compute_all_suppressed(rules, original, &line_table);

    let mut include_results: Vec<IncludeResult> = Vec::with_capacity(includes.len());
    for include in includes {
        let macro_expansion = macro_index.expand(&include);
        let outcome = match macro_expansion {
            MacroExpansion::Expanded { definitions } => process_expanded_macro_include(
                rules,
                relpath,
                original,
                project_root,
                &suppressed,
                &include,
                &definitions,
            ),
            MacroExpansion::NotMacro | MacroExpansion::Unresolved => process_direct_include(
                rules,
                relpath,
                original,
                project_root,
                &suppressed,
                &include,
            ),
        };

        include_results.push(IncludeResult { include, outcome });
    }

    FileProcessing {
        include_results,
        lex_warnings,
    }
}

fn process_direct_include(
    rules: &[CompiledRule<'_>],
    relpath: &Path,
    original: &str,
    project_root: &Path,
    suppressed: &BTreeMap<String, HashSet<usize>>,
    include: &Include,
) -> IncludeOutcome {
    let matched = engine::match_all(rules, relpath, include, suppressed, project_root);

    if matched.matched.is_empty() && matched.failures.is_empty() {
        return IncludeOutcome::NoMatch;
    }

    let mut evaluations: Vec<RuleEvaluation> = matched
        .failures
        .iter()
        .map(|failure| RuleEvaluation {
            rule_name: failure.rule.rule.name.clone(),
            action: EvaluatedAction::EvaluationFailure {
                message: failure.message.clone(),
            },
            trailing: TrailingOutcome::Skip,
        })
        .collect();

    evaluations.extend(matched.matched.iter().map(|cm| {
        let action = evaluate_direct_action(
            cm.rule,
            include,
            original,
            relpath,
            project_root,
            cm.resolved_header.as_deref(),
        );
        let trailing = action::evaluate_trailing(cm.rule, include, original, relpath);
        RuleEvaluation {
            rule_name: cm.rule.rule.name.clone(),
            action,
            trailing,
        }
    }));

    collapse_outcomes(include, original, relpath, evaluations)
}

fn process_expanded_macro_include(
    rules: &[CompiledRule<'_>],
    relpath: &Path,
    original: &str,
    project_root: &Path,
    suppressed: &BTreeMap<String, HashSet<usize>>,
    include: &Include,
    definitions: &[&MacroDefinition],
) -> IncludeOutcome {
    let mut evaluations: Vec<RuleEvaluation> = Vec::new();

    for definition in definitions {
        let match_include =
            include_with_expanded_macro(include, definition.form, &definition.content);
        let matched = engine::match_all(rules, relpath, &match_include, suppressed, project_root);

        evaluations.extend(matched.failures.iter().map(|failure| RuleEvaluation {
            rule_name: failure.rule.rule.name.clone(),
            action: EvaluatedAction::EvaluationFailure {
                message: failure.message.clone(),
            },
            trailing: TrailingOutcome::Skip,
        }));

        evaluations.extend(matched.matched.iter().map(|cm| {
            let action = match cm.rule.rule.macro_rewrite {
                MacroRewrite::Definitions => evaluate_macro_definition_action(
                    cm.rule,
                    include,
                    definition,
                    relpath,
                    project_root,
                    cm.resolved_header.as_deref(),
                ),
                MacroRewrite::UseSite => evaluate_macro_use_site_action(
                    cm.rule,
                    include,
                    &match_include,
                    original,
                    relpath,
                    project_root,
                    cm.resolved_header.as_deref(),
                ),
            };
            let trailing = action::evaluate_trailing(cm.rule, include, original, relpath);
            RuleEvaluation {
                rule_name: cm.rule.rule.name.clone(),
                action,
                trailing,
            }
        }));
    }

    if evaluations.is_empty() {
        IncludeOutcome::NoMatch
    } else {
        collapse_macro_outcomes(include, original, relpath, evaluations)
    }
}

#[derive(Debug)]
struct MacroIndex {
    by_name: BTreeMap<String, Vec<MacroDefinition>>,
}

#[derive(Debug, Clone)]
struct MacroDefinition {
    form: IncludeForm,
    content: String,
    relpath: PathBuf,
    line: usize,
    value_range: Range<usize>,
    source: String,
}

enum MacroExpansion<'a> {
    NotMacro,
    Unresolved,
    Expanded {
        definitions: Vec<&'a MacroDefinition>,
    },
}

impl MacroIndex {
    fn expand<'a>(&'a self, include: &Include) -> MacroExpansion<'a> {
        if include.form != IncludeForm::Macro {
            return MacroExpansion::NotMacro;
        }
        let Some(defs) = self.by_name.get(&include.content) else {
            return MacroExpansion::Unresolved;
        };
        MacroExpansion::Expanded {
            definitions: defs.iter().collect(),
        }
    }
}

fn include_with_expanded_macro(original: &Include, form: IncludeForm, content: &str) -> Include {
    Include {
        form,
        content: content.to_string(),
        line: original.line,
        argument_range: original.argument_range.clone(),
        trailing_range: original.trailing_range.clone(),
        trailing_comment_style: original.trailing_comment_style,
        has_cross_line_block_trailing: original.has_cross_line_block_trailing,
    }
}

fn build_macro_index(project_root: &Path, relpaths: &[PathBuf]) -> MacroIndex {
    let mut by_name: BTreeMap<String, Vec<MacroDefinition>> = BTreeMap::new();
    for relpath in relpaths {
        let abs = project_root.join(relpath);
        let Ok(bytes) = std::fs::read(&abs) else {
            continue;
        };
        let (_, body) = strip_bom(&bytes);
        let Ok(source) = std::str::from_utf8(body) else {
            continue;
        };
        let source = source.to_string();
        for raw in macro_define::scan(&source) {
            by_name
                .entry(raw.name.clone())
                .or_default()
                .push(macro_definition(relpath, &source, raw));
        }
    }
    MacroIndex { by_name }
}

fn macro_definition(relpath: &Path, source: &str, raw: HeaderMacroDefinition) -> MacroDefinition {
    MacroDefinition {
        form: raw.form,
        content: raw.content,
        relpath: relpath.to_path_buf(),
        line: raw.line,
        value_range: raw.value_range,
        source: source.to_string(),
    }
}

fn evaluate_direct_action(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
) -> EvaluatedAction {
    let out = action::evaluate_action_with_resolution(
        rule,
        include,
        source,
        file_relpath,
        project_root,
        resolved_header,
    );
    action_outcome_to_evaluated(out, file_relpath.to_path_buf(), source)
}

fn evaluate_macro_definition_action(
    rule: &CompiledRule<'_>,
    original_include: &Include,
    definition: &MacroDefinition,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
) -> EvaluatedAction {
    if matches!(rule.rule.action, ResolvedAction::Skip) {
        return EvaluatedAction::Skip;
    }

    if matches!(
        rule.rule.action,
        ResolvedAction::Remove { .. } | ResolvedAction::CommentOut { .. }
    ) {
        return EvaluatedAction::EvaluationFailure {
            message: format!(
                "rule `{}`: action is not supported for macro-form include `#include {}`; use resolve, replace, keep, error, or skip",
                rule.rule.name, original_include.content
            ),
        };
    }

    let virtual_include = Include {
        form: definition.form,
        content: definition.content.clone(),
        line: definition.line,
        argument_range: definition.value_range.clone(),
        trailing_range: definition.value_range.end..definition.value_range.end,
        trailing_comment_style: None,
        has_cross_line_block_trailing: false,
    };

    let out = action::evaluate_action_with_resolution(
        rule,
        &virtual_include,
        &definition.source,
        file_relpath,
        project_root,
        resolved_header,
    );
    action_outcome_to_evaluated(out, definition.relpath.clone(), &definition.source)
}

fn evaluate_macro_use_site_action(
    rule: &CompiledRule<'_>,
    original_include: &Include,
    match_include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
) -> EvaluatedAction {
    if matches!(rule.rule.action, ResolvedAction::Skip) {
        return EvaluatedAction::Skip;
    }
    let virtual_include = Include {
        form: match_include.form,
        content: match_include.content.clone(),
        line: original_include.line,
        argument_range: original_include.argument_range.clone(),
        trailing_range: original_include.trailing_range.clone(),
        trailing_comment_style: original_include.trailing_comment_style,
        has_cross_line_block_trailing: original_include.has_cross_line_block_trailing,
    };
    evaluate_direct_action(
        rule,
        &virtual_include,
        source,
        file_relpath,
        project_root,
        resolved_header,
    )
}

fn action_outcome_to_evaluated(
    outcome: ActionOutcome,
    target_relpath: PathBuf,
    target_source: &str,
) -> EvaluatedAction {
    match outcome {
        ActionOutcome::Skip => EvaluatedAction::Skip,
        ActionOutcome::Apply {
            edit_range,
            new_text,
        } => {
            let original_text = target_source
                .get(edit_range.clone())
                .unwrap_or("")
                .to_string();
            EvaluatedAction::Apply {
                target: EditTarget::File(target_relpath),
                edit_range,
                new_text,
                original_text,
            }
        }
        ActionOutcome::Error { message } => EvaluatedAction::Error { message },
        ActionOutcome::EvaluationFailure { message } => {
            EvaluatedAction::EvaluationFailure { message }
        }
    }
}

struct RuleEvaluation {
    rule_name: String,
    action: EvaluatedAction,
    trailing: TrailingOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum EditTarget {
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EvaluatedAction {
    Skip,
    Apply {
        target: EditTarget,
        edit_range: Range<usize>,
        new_text: String,
        original_text: String,
    },
    Error {
        message: String,
    },
    EvaluationFailure {
        message: String,
    },
}

#[derive(Debug, Clone)]
struct ActionCandidate {
    rule_name: String,
    target: EditTarget,
    edit_range: Range<usize>,
    new_text: String,
    original_text: String,
}

#[derive(Debug, Clone)]
struct ActionGroup {
    target: EditTarget,
    edit_range: Range<usize>,
    new_text: String,
    original_text: String,
    rules: Vec<String>,
}

struct CollapsedAction {
    target: EditTarget,
    edit_range: Range<usize>,
    new_text: String,
    original_text: String,
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
    relpath: &Path,
    evaluations: Vec<RuleEvaluation>,
) -> IncludeOutcome {
    for ev in &evaluations {
        if let EvaluatedAction::Error { message } = &ev.action {
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
        if let EvaluatedAction::EvaluationFailure { message } = &ev.action {
            return IncludeOutcome::EvaluationFailure {
                rule: ev.rule_name.clone(),
                message: message.clone(),
            };
        }
    }

    let mut action_candidates: Vec<(String, EditTarget, Range<usize>, String, String)> = Vec::new();
    let mut trailing_candidates: Vec<(String, String)> = Vec::new();
    for ev in &evaluations {
        match &ev.action {
            EvaluatedAction::Apply {
                target,
                edit_range,
                new_text,
                original_text,
            } => action_candidates.push((
                ev.rule_name.clone(),
                target.clone(),
                edit_range.clone(),
                new_text.clone(),
                original_text.clone(),
            )),
            EvaluatedAction::Skip
            | EvaluatedAction::Error { .. }
            | EvaluatedAction::EvaluationFailure { .. } => {}
        }
        match &ev.trailing {
            TrailingOutcome::Apply { new_text } => {
                trailing_candidates.push((ev.rule_name.clone(), new_text.clone()));
            }
            TrailingOutcome::Skip | TrailingOutcome::Error { .. } => {}
        }
    }

    let current_target = EditTarget::File(relpath.to_path_buf());
    let action =
        match collapse_action_candidates(include, source, &current_target, action_candidates) {
            Ok(action) => action,
            Err(conflict) => return conflict,
        };
    let trailing = match collapse_trailing_candidates(
        include,
        source,
        &current_target,
        &action,
        trailing_candidates,
    ) {
        Ok(trailing) => trailing,
        Err(conflict) => return conflict,
    };

    let original_trailing = &source[include.trailing_range.clone()];
    let action_targets_current_arg =
        action.target == current_target && action.edit_range == include.argument_range;
    if action.target == current_target && !action_targets_current_arg {
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
        return rewritten_or_keep(
            source,
            relpath,
            action.edit_range,
            action.new_text,
            action.rules,
        );
    }

    if action_targets_current_arg {
        let edit_range = argument_and_trailing_range(include);
        let new_text = format!("{}{}", action.new_text, trailing.new_text);
        let rules = fallback_to_matched_rules_if_empty(
            merge_rules(action.rules, trailing.rules),
            &evaluations,
        );
        return rewritten_or_keep(source, relpath, edit_range, new_text, rules);
    }

    let mut edits = Vec::new();
    if action.new_text != action.original_text {
        let EditTarget::File(target_relpath) = &action.target;
        edits.push(PlannedEdit {
            relpath: target_relpath.clone(),
            edit_range: action.edit_range.clone(),
            new_text: action.new_text.clone(),
        });
    }

    let edit_range = argument_and_trailing_range(include);
    let original_arg = &source[include.argument_range.clone()];
    let new_text = format!("{original_arg}{}", trailing.new_text);
    if new_text != source[edit_range.clone()] {
        edits.push(PlannedEdit {
            relpath: relpath.to_path_buf(),
            edit_range: edit_range.clone(),
            new_text: new_text.clone(),
        });
    }

    let rules =
        fallback_to_matched_rules_if_empty(merge_rules(action.rules, trailing.rules), &evaluations);
    if edits.is_empty() {
        IncludeOutcome::Keep { rules }
    } else {
        IncludeOutcome::Rewritten {
            rules,
            edit_range,
            new_text,
            edits,
        }
    }
}

fn collapse_macro_outcomes(
    include: &Include,
    source: &str,
    relpath: &Path,
    evaluations: Vec<RuleEvaluation>,
) -> IncludeOutcome {
    for ev in &evaluations {
        if let EvaluatedAction::Error { message } = &ev.action {
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
        if let EvaluatedAction::EvaluationFailure { message } = &ev.action {
            return IncludeOutcome::EvaluationFailure {
                rule: ev.rule_name.clone(),
                message: message.clone(),
            };
        }
    }

    let mut action_candidates: Vec<ActionCandidate> = Vec::new();
    let mut trailing_candidates: Vec<(String, String)> = Vec::new();
    for ev in &evaluations {
        match &ev.action {
            EvaluatedAction::Apply {
                target,
                edit_range,
                new_text,
                original_text,
            } => action_candidates.push(ActionCandidate {
                rule_name: ev.rule_name.clone(),
                target: target.clone(),
                edit_range: edit_range.clone(),
                new_text: new_text.clone(),
                original_text: original_text.clone(),
            }),
            EvaluatedAction::Skip
            | EvaluatedAction::Error { .. }
            | EvaluatedAction::EvaluationFailure { .. } => {}
        }
        match &ev.trailing {
            TrailingOutcome::Apply { new_text } => {
                trailing_candidates.push((ev.rule_name.clone(), new_text.clone()));
            }
            TrailingOutcome::Skip | TrailingOutcome::Error { .. } => {}
        }
    }

    let current_target = EditTarget::File(relpath.to_path_buf());
    let action_groups =
        match collapse_macro_action_candidates(include, source, &current_target, action_candidates)
        {
            Ok(groups) => groups,
            Err(conflict) => return conflict,
        };

    let original_arg = &source[include.argument_range.clone()];
    let current_arg_group = action_groups
        .iter()
        .find(|group| group.target == current_target && group.edit_range == include.argument_range);
    let action_arg = current_arg_group
        .map(|group| group.new_text.as_str())
        .unwrap_or(original_arg);
    let trailing = match collapse_macro_trailing_candidates(
        include,
        source,
        action_arg,
        trailing_candidates,
    ) {
        Ok(trailing) => trailing,
        Err(conflict) => return conflict,
    };

    let original_trailing = &source[include.trailing_range.clone()];
    let current_non_arg_group = action_groups
        .iter()
        .find(|group| group.target == current_target && group.edit_range != include.argument_range);
    if let Some(group) = current_non_arg_group
        && !trailing.rules.is_empty()
        && trailing.new_text != original_trailing
    {
        let mut rule_outputs: Vec<(String, String)> = group
            .rules
            .iter()
            .map(|rule| (rule.clone(), group.new_text.clone()))
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

    let mut edits = Vec::new();
    let mut action_rules = Vec::new();
    for group in &action_groups {
        action_rules = merge_rules(action_rules, group.rules.clone());
        if group.target == current_target && group.edit_range == include.argument_range {
            continue;
        }
        if group.new_text != group.original_text {
            let EditTarget::File(target_relpath) = &group.target;
            edits.push(PlannedEdit {
                relpath: target_relpath.clone(),
                edit_range: group.edit_range.clone(),
                new_text: group.new_text.clone(),
            });
        }
    }

    let edit_range = argument_and_trailing_range(include);
    let use_site_text = format!("{action_arg}{}", trailing.new_text);
    if current_non_arg_group.is_none() && use_site_text != source[edit_range.clone()] {
        edits.push(PlannedEdit {
            relpath: relpath.to_path_buf(),
            edit_range: edit_range.clone(),
            new_text: use_site_text.clone(),
        });
    }

    let rules =
        fallback_to_matched_rules_if_empty(merge_rules(action_rules, trailing.rules), &evaluations);
    if edits.is_empty() {
        IncludeOutcome::Keep { rules }
    } else {
        IncludeOutcome::Rewritten {
            rules,
            edit_range,
            new_text: use_site_text,
            edits,
        }
    }
}

fn collapse_macro_action_candidates(
    include: &Include,
    source: &str,
    current_target: &EditTarget,
    candidates: Vec<ActionCandidate>,
) -> std::result::Result<Vec<ActionGroup>, IncludeOutcome> {
    let mut groups: BTreeMap<(EditTarget, usize, usize), ActionGroup> = BTreeMap::new();
    for candidate in candidates {
        let key = (
            candidate.target.clone(),
            candidate.edit_range.start,
            candidate.edit_range.end,
        );
        if let Some(group) = groups.get_mut(&key) {
            if group.new_text != candidate.new_text {
                let rule_outputs = vec![
                    (
                        group.rules.join(", "),
                        action_candidate_display(
                            include,
                            source,
                            current_target,
                            &group.target,
                            &group.edit_range,
                            &group.new_text,
                        ),
                    ),
                    (
                        candidate.rule_name,
                        action_candidate_display(
                            include,
                            source,
                            current_target,
                            &candidate.target,
                            &candidate.edit_range,
                            &candidate.new_text,
                        ),
                    ),
                ];
                let mut differing_aspects = compute_differing_aspects(
                    &rule_outputs
                        .iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>(),
                );
                if differing_aspects.is_empty() {
                    differing_aspects.push(DiffAspect::IncludePath);
                }
                return Err(IncludeOutcome::Conflict {
                    rule_outputs,
                    differing_aspects,
                });
            }
            if !group.rules.contains(&candidate.rule_name) {
                group.rules.push(candidate.rule_name);
            }
        } else {
            groups.insert(
                key,
                ActionGroup {
                    target: candidate.target,
                    edit_range: candidate.edit_range,
                    new_text: candidate.new_text,
                    original_text: candidate.original_text,
                    rules: vec![candidate.rule_name],
                },
            );
        }
    }
    Ok(groups.into_values().collect())
}

fn collapse_macro_trailing_candidates(
    include: &Include,
    source: &str,
    action_arg: &str,
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

    let rule_outputs: Vec<(String, String)> = candidates
        .into_iter()
        .map(|(name, trailing)| (name, format!("{action_arg}{trailing}")))
        .collect();
    Err(IncludeOutcome::Conflict {
        rule_outputs,
        differing_aspects: vec![DiffAspect::TrailingComment],
    })
}

fn collapse_action_candidates(
    include: &Include,
    source: &str,
    current_target: &EditTarget,
    candidates: Vec<(String, EditTarget, Range<usize>, String, String)>,
) -> std::result::Result<CollapsedAction, IncludeOutcome> {
    if candidates.is_empty() {
        let original = source[include.argument_range.clone()].to_string();
        return Ok(CollapsedAction {
            target: current_target.clone(),
            edit_range: include.argument_range.clone(),
            new_text: original.clone(),
            original_text: original,
            rules: Vec::new(),
        });
    }
    let (first_target, first_range, first_text, first_original) = (
        candidates[0].1.clone(),
        candidates[0].2.clone(),
        candidates[0].3.clone(),
        candidates[0].4.clone(),
    );
    let all_same = candidates.iter().all(|(_, target, range, text, _)| {
        *target == first_target && *range == first_range && *text == first_text
    });
    if all_same {
        return Ok(CollapsedAction {
            target: first_target,
            edit_range: first_range,
            new_text: first_text,
            original_text: first_original,
            rules: candidates.into_iter().map(|(n, _, _, _, _)| n).collect(),
        });
    }

    let rule_outputs: Vec<(String, String)> = candidates
        .into_iter()
        .map(|(name, target, range, text, _)| {
            let display =
                action_candidate_display(include, source, current_target, &target, &range, &text);
            (name, display)
        })
        .collect();
    let mut differing_aspects = compute_differing_aspects(
        &rule_outputs
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>(),
    );
    if differing_aspects.is_empty() {
        differing_aspects.push(DiffAspect::IncludePath);
    }
    Err(IncludeOutcome::Conflict {
        rule_outputs,
        differing_aspects,
    })
}

fn collapse_trailing_candidates(
    include: &Include,
    source: &str,
    current_target: &EditTarget,
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

    let action_arg =
        if &action.target == current_target && action.edit_range == include.argument_range {
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
    current_target: &EditTarget,
    target: &EditTarget,
    range: &Range<usize>,
    text: &str,
) -> String {
    if target == current_target && *range == include.argument_range {
        format!("{text}{}", &source[include.trailing_range.clone()])
    } else {
        text.to_string()
    }
}

fn rewritten_or_keep(
    source: &str,
    relpath: &Path,
    edit_range: Range<usize>,
    new_text: String,
    rules: Vec<String>,
) -> IncludeOutcome {
    if new_text == source[edit_range.clone()] {
        IncludeOutcome::Keep { rules }
    } else {
        IncludeOutcome::Rewritten {
            rules,
            edit_range: edit_range.clone(),
            new_text: new_text.clone(),
            edits: vec![PlannedEdit {
                relpath: relpath.to_path_buf(),
                edit_range,
                new_text,
            }],
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

fn fallback_to_matched_rules_if_empty(
    rules: Vec<String>,
    evaluations: &[RuleEvaluation],
) -> Vec<String> {
    if !rules.is_empty() {
        return rules;
    }
    let mut out = Vec::with_capacity(evaluations.len());
    for ev in evaluations {
        if !out.contains(&ev.rule_name) {
            out.push(ev.rule_name.clone());
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

    fn file_result<'a>(summary: &'a Summary, relpath: &str) -> &'a FileResult {
        summary
            .files
            .iter()
            .find(|file| file.relpath == Path::new(relpath))
            .unwrap_or_else(|| panic!("missing file result for {relpath}"))
    }

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
    fn config_mode_compiles_include_resolved_match_globs() {
        let err = config_compile_error(
            r#"
            [[rule]]
            name = "base"
            include_resolved_match = ["["]
            "#,
        );
        assert!(err.contains("include_resolved_match glob"), "{err}");
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
        match &f.include_results[0].outcome {
            IncludeOutcome::Keep { rules } => assert_eq!(rules, &["base"]),
            other => panic!("unexpected outcome: {other:?}"),
        }
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
    fn include_resolved_match_filters_candidates_before_ambiguity() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["first", "second"]
            include_resolved_match = ["second/foo.h"]
            action = { type = "resolve", relative_to = "." }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");
        proj.write("first/foo.h", "");
        proj.write("second/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        match &summary.files[0].include_results[0].outcome {
            IncludeOutcome::Rewritten { new_text, .. } => {
                assert_eq!(new_text, "\"../second/foo.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn include_resolved_match_can_skip_when_no_candidate_matches() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**/*"]
            include_directories = ["include"]
            include_resolved_match = ["vendor/**"]
            include_on_unresolved = "skip"
            action = { type = "replace", with = "lib/${original}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("src/main.c", "#include \"foo.h\"\n");
        proj.write("include/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(matches!(
            summary.files[0].include_results[0].outcome,
            IncludeOutcome::NoMatch
        ));
        assert!(summary.files[0].rewritten.is_none());
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
    fn macro_include_resolve_rewrites_definition_not_use_site() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["**/*"]
            include_directories = ["include"]
            action = { type = "resolve", relative_to = "${current_file}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("config.h", "#define DEVICE_HEADER \"foo.h\"\n");
        proj.write("src/main.c", "#include DEVICE_HEADER\n");
        proj.write("include/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(summary.conflicts.is_empty());
        let src = file_result(&summary, "src/main.c");
        match &src.include_results[0].outcome {
            IncludeOutcome::Rewritten { edits, .. } => {
                assert_eq!(edits.len(), 1);
                assert_eq!(edits[0].relpath, PathBuf::from("config.h"));
                assert_eq!(edits[0].new_text, "\"../include/foo.h\"");
            }
            other => panic!("unexpected outcome: {other:?}"),
        }

        let written = apply(&summary).unwrap();
        assert_eq!(written, 1);
        assert_eq!(
            proj.read_to_string("src/main.c"),
            "#include DEVICE_HEADER\n"
        );
        assert_eq!(
            proj.read_to_string("config.h"),
            "#define DEVICE_HEADER \"../include/foo.h\"\n"
        );
    }

    #[test]
    fn macro_include_reuses_identical_definition_edit() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["**/*"]
            include_directories = ["include"]
            action = { type = "resolve", relative_to = "${current_file}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("config.h", "#define DEVICE_HEADER \"foo.h\"\n");
        proj.write("src/a.c", "#include DEVICE_HEADER\n");
        proj.write("src/b.c", "#include DEVICE_HEADER\n");
        proj.write("include/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert!(summary.conflicts.is_empty());
        let written = apply(&summary).unwrap();
        assert_eq!(written, 1);
        assert_eq!(
            proj.read_to_string("config.h"),
            "#define DEVICE_HEADER \"../include/foo.h\"\n"
        );
    }

    #[test]
    fn macro_include_definition_edit_conflict_is_unfixable() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["**/*"]
            include_directories = ["include"]
            action = { type = "resolve", relative_to = "${current_file}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("config.h", "#define DEVICE_HEADER \"foo.h\"\n");
        proj.write("src/main.c", "#include DEVICE_HEADER\n");
        proj.write("src/deep/main.c", "#include DEVICE_HEADER\n");
        proj.write("include/foo.h", "");

        let summary = run(None, proj.path(), &[], None, CheckMode::Run).unwrap();
        assert_eq!(summary_exit_code(&summary), 3);
        assert_eq!(summary.conflicts.len(), 2);
        let written = apply(&summary).unwrap();
        assert_eq!(written, 0);
        assert_eq!(
            proj.read_to_string("config.h"),
            "#define DEVICE_HEADER \"foo.h\"\n"
        );
    }

    #[test]
    fn macro_definition_edit_outside_path_filter_is_unfixable() {
        let rule = r#"
            [[rule]]
            name = "base"
            file_paths = ["**/*"]
            include_directories = ["include"]
            action = { type = "resolve", relative_to = "${current_file}" }
        "#;
        let proj = TmpProject::create_with_rules(rule);
        proj.write("config.h", "#define DEVICE_HEADER \"foo.h\"\n");
        proj.write("src/main.c", "#include DEVICE_HEADER\n");
        proj.write("include/foo.h", "");

        let filtered = proj.path().join("src/main.c");
        let summary = run(None, proj.path(), &[filtered], None, CheckMode::Run).unwrap();
        assert_eq!(summary_exit_code(&summary), 3);
        let src = file_result(&summary, "src/main.c");
        match &src.include_results[0].outcome {
            IncludeOutcome::EvaluationFailure { message, .. } => {
                assert!(message.contains("outside the requested path filter"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        let written = apply(&summary).unwrap();
        assert_eq!(written, 0);
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
