//! Top-level orchestration for v0.3.
//!
//! Two modes:
//!
//! * [`CheckMode::Config`] — parse + validate + run copy resolution only.
//!   No source files are opened.
//! * [`CheckMode::Run`] — walk source files, lex includes, match every
//!   rule's four layers, evaluate every matched rule's action, then
//!   decide per-include conflict-by-final-text.
//!
//! Conflict detection (the v0.3 model): for an include matched by N
//! rules, evaluate the action against each. If all rules produce
//! identical `Outcome::Rewrite { new_text }` (or all produce `Keep`,
//! which is itself identical), there is no conflict. Otherwise the
//! include is a [`Conflict`].
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
use crate::config::discover::{self, CONFIG_FILENAME};
use crate::config::schema::IncludeForm;
use crate::lex::include_line::{self, Include};
use crate::rule::action::{self, Outcome};
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
    /// One of the matched rules produced an `action.error` (or trailing
    /// transform error). Exit code 2.
    Error { rule: String, message: String },
    /// Action evaluation failed (resolve missed, multiple matches, etc.).
    EvaluationFailure { rule: String, message: String },
    /// Matched rules disagreed on the final text. Exit code 3.
    Conflict { rule_outputs: Vec<(String, String)> },
}

/// A conflict surfaced for a specific include (final-text disagreement).
#[derive(Debug)]
pub struct Conflict {
    pub file_relpath: PathBuf,
    pub include_line: usize,
    pub include_text: String,
    /// Per-rule final-line text (the bytes that rule would have written).
    pub rule_outputs: Vec<(String, String)>,
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
                anyhow::bail!(
                    "--config path does not point at a file: {}",
                    p.display(),
                );
            }
            p.to_path_buf()
        }
        None => discover::find_root_config(start_dir)?,
    };
    let cfg = discover::load_root_config(&resolved_config_path)?;
    let project = cfg
        .raw
        .project
        .as_ref()
        .expect("load_root_config guarantees [project] is present");
    let project_root_abs = discover::resolve_project_root(&resolved_config_path, project)?;
    discover::assert_no_extra_configs(&project_root_abs, &resolved_config_path)?;
    let resolved = copy::resolve(std::slice::from_ref(&cfg))?;

    if mode == CheckMode::Config {
        return Ok(Summary {
            mode,
            project_root: project_root_abs,
            files: Vec::new(),
            conflicts: Vec::new(),
            skipped: Vec::new(),
        });
    }

    install_thread_pool(jobs);

    let compiled = compile_rules(&resolved)?;
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
    for res in per_file {
        match res {
            Ok(file_result) => {
                // Pull conflicts out of include_results into the top-level vec.
                for r in &file_result.include_results {
                    if let IncludeOutcome::Conflict { rule_outputs } = &r.outcome {
                        conflicts.push(Conflict {
                            file_relpath: file_result.relpath.clone(),
                            include_line: r.include.line,
                            include_text: format_include_text(&r.include),
                            rule_outputs: rule_outputs.clone(),
                        });
                    }
                }
                files.push(file_result);
            }
            Err(s) => skipped.push(s),
        }
    }

    Ok(Summary {
        mode,
        project_root: project_root_abs,
        files,
        conflicts,
        skipped,
    })
}

/// Apply rewrites to disk. Refuses if any conflict is present. Files
/// whose include results include any `Error` / `EvaluationFailure` /
/// `Conflict` outcome are skipped (no partial writes). Returns the
/// number of files actually written.
pub fn apply(summary: &Summary) -> Result<usize> {
    if !summary.conflicts.is_empty() {
        anyhow::bail!(
            "refusing to apply: {} conflict(s) must be resolved first",
            summary.conflicts.len()
        );
    }
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

fn file_has_errors(f: &FileResult) -> bool {
    f.include_results.iter().any(|r| {
        matches!(
            r.outcome,
            IncludeOutcome::Error { .. }
                | IncludeOutcome::EvaluationFailure { .. }
                | IncludeOutcome::Conflict { .. }
        )
    })
}

/// Render a unified diff for every changed file in `summary`.
pub fn render_diff(summary: &Summary) -> String {
    use similar::TextDiff;
    let mut out = String::new();
    for f in &summary.files {
        let Some(new) = &f.rewritten else { continue };
        let diff = TextDiff::from_lines(&f.original, new);

        use crate::util::PathSlashExt;
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
}

fn process_file(
    rules: &[CompiledRule<'_>],
    relpath: &Path,
    original: &str,
    project_root: &Path,
) -> FileProcessing {
    let includes = include_line::scan(original);
    let line_table = include_line::line_table(original);
    let suppressed = engine::compute_all_suppressed(rules, original, &line_table);

    let mut include_results: Vec<IncludeResult> = Vec::with_capacity(includes.len());
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();

    for include in includes {
        let matched = engine::match_all(rules, relpath, &include, &suppressed);

        if matched.matched.is_empty() {
            include_results.push(IncludeResult {
                include,
                outcome: IncludeOutcome::NoMatch,
            });
            continue;
        }

        // Evaluate every matched rule's action; collect outcomes.
        let outcomes: Vec<(String, Outcome)> = matched
            .matched
            .iter()
            .map(|cm| {
                let outcome = action::evaluate(cm.rule, &include, original, relpath, project_root);
                (cm.rule.rule.name.clone(), outcome)
            })
            .collect();

        let outcome = collapse_outcomes(&include, original, outcomes);

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
    }
}

/// Conflict-by-final-text rules:
///
/// 1. If any matched rule errored (`Outcome::Error`) → `IncludeOutcome::Error`.
/// 2. If any matched rule had an `EvaluationFailure` → propagate.
/// 3. If matched rules all produced `Keep` → `IncludeOutcome::Keep`.
/// 4. If matched rules all produced `Rewrite` with identical
///    `(edit_range, new_text)` → `IncludeOutcome::Rewritten`.
/// 5. Otherwise → `IncludeOutcome::Conflict { rule_outputs }` where
///    `rule_outputs[i] = (rule_name, final_line_text)`.
fn collapse_outcomes(
    include: &Include,
    source: &str,
    outcomes: Vec<(String, Outcome)>,
) -> IncludeOutcome {
    // (1) any Error wins
    for (rule_name, o) in &outcomes {
        if let Outcome::Error { message } = o {
            return IncludeOutcome::Error {
                rule: rule_name.clone(),
                message: message.clone(),
            };
        }
    }
    // (2) any EvaluationFailure wins
    for (rule_name, o) in &outcomes {
        if let Outcome::EvaluationFailure { message } = o {
            return IncludeOutcome::EvaluationFailure {
                rule: rule_name.clone(),
                message: message.clone(),
            };
        }
    }

    // Compute "final line" text for each rule. For Keep: take the
    // existing argument + trailing bytes. For Rewrite: take new_text.
    let mut finals: Vec<(String, Range<usize>, String)> = Vec::new();
    for (rule_name, o) in outcomes {
        match o {
            Outcome::Keep => {
                let r = argument_and_trailing_range(include);
                finals.push((rule_name, r.clone(), source[r].to_string()));
            }
            Outcome::Rewrite {
                edit_range,
                new_text,
            } => {
                finals.push((rule_name, edit_range, new_text));
            }
            Outcome::Error { .. } | Outcome::EvaluationFailure { .. } => unreachable!(),
        }
    }

    // All identical (edit_range, new_text)?
    let (first_range, first_text) = (finals[0].1.clone(), finals[0].2.clone());
    let all_same = finals
        .iter()
        .all(|(_, r, t)| *r == first_range && *t == first_text);
    if all_same {
        let original_text = &source[first_range.clone()];
        if first_text == original_text {
            IncludeOutcome::Keep {
                rules: finals.into_iter().map(|(n, _, _)| n).collect(),
            }
        } else {
            IncludeOutcome::Rewritten {
                rules: finals.into_iter().map(|(n, _, _)| n).collect(),
                edit_range: first_range,
                new_text: first_text,
            }
        }
    } else {
        IncludeOutcome::Conflict {
            rule_outputs: finals.into_iter().map(|(n, _, t)| (n, t)).collect(),
        }
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
    let root_canon = std::fs::canonicalize(project_root).with_context(|| {
        format!("canonicalize project root {}", project_root.display())
    })?;
    let cwd = std::env::current_dir().context("read current working directory")?;
    let mut prefixes: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        let absolute = if p.is_absolute() { p.clone() } else { cwd.join(p) };
        let canon = std::fs::canonicalize(&absolute).with_context(|| {
            format!("path filter entry {} does not exist", absolute.display())
        })?;
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
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "inclean-pipe-{}-{}",
            std::process::id(),
            inc_counter()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn inc_counter() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }
    fn touch(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    fn min_inclean_toml() -> String {
        format!(
            "[project]\nroot = \".\"\nversion = \"{v}\"\nmin_inclean_version = \"{v}\"\n",
            v = env!("CARGO_PKG_VERSION"),
        )
    }

    #[test]
    fn config_mode_skips_source_scan() {
        let root = tmp();
        touch(&root, "src/main.c", "#include \"x.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!("{}\n[[rule]]\nname = \"base\"\n", min_inclean_toml()),
        );
        let summary = run(None, &root, &[], None, CheckMode::Config).unwrap();
        assert!(summary.files.is_empty());
        assert!(summary.conflicts.is_empty());
        assert_eq!(summary_exit_code(&summary), 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keep_action_produces_no_edits() {
        let root = tmp();
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"base\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"keep\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
        let f = &summary.files[0];
        assert!(matches!(
            f.include_results[0].outcome,
            IncludeOutcome::Keep { .. }
        ));
        assert!(f.rewritten.is_none());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn replace_action_writes_back() {
        let root = tmp();
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"base\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"replace\", with = \"lib/${{original}}\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
        let f = &summary.files[0];
        assert!(matches!(
            f.include_results[0].outcome,
            IncludeOutcome::Rewritten { .. }
        ));
        let written = apply(&summary).unwrap();
        assert_eq!(written, 1);
        let new = fs::read_to_string(root.join("src/main.c")).unwrap();
        assert!(new.contains("\"lib/foo.h\""));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn error_action_produces_error_outcome_and_exit_2() {
        let root = tmp();
        touch(&root, "src/main.c", "#include \"old.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"base\"\nfile_paths = [\"src/**/*\"]\ninclude_match = [\"old.h\"]\naction = {{ type = \"error\", message = \"deprecated\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
        assert!(matches!(
            summary.files[0].include_results[0].outcome,
            IncludeOutcome::Error { .. }
        ));
        assert_eq!(summary_exit_code(&summary), 2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn conflicting_rules_produce_conflict_and_exit_3() {
        // Two rules that both match but rewrite to different texts.
        let root = tmp();
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"a\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"replace\", with = \"A/foo.h\" }}\n\n[[rule]]\nname = \"b\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"replace\", with = \"B/foo.h\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
        assert_eq!(summary.conflicts.len(), 1);
        assert_eq!(summary_exit_code(&summary), 3);
        // apply refuses
        let err = apply(&summary).unwrap_err();
        assert!(format!("{err:#}").contains("conflict"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn agreeing_rules_collapse_to_a_single_rewrite() {
        // Two rules that both rewrite to the same text: no conflict.
        let root = tmp();
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"a\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"replace\", with = \"new/foo.h\" }}\n\n[[rule]]\nname = \"b\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"replace\", with = \"new/foo.h\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
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
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bom_is_preserved_across_apply() {
        let root = tmp();
        // \u{FEFF} is the BOM in source. We write raw bytes to ensure the
        // BOM is at the very start.
        let bom = [0xEF, 0xBB, 0xBFu8];
        let body = b"#include \"foo.h\"\n";
        let mut payload = Vec::new();
        payload.extend_from_slice(&bom);
        payload.extend_from_slice(body);
        let main_path = root.join("src/main.c");
        fs::create_dir_all(main_path.parent().unwrap()).unwrap();
        fs::write(&main_path, &payload).unwrap();

        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"base\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"replace\", with = \"lib/${{original}}\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
        assert!(summary.files[0].had_bom);
        apply(&summary).unwrap();
        let read_back = fs::read(&main_path).unwrap();
        assert!(read_back.starts_with(&bom));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parse_failure_is_skipped_not_a_hard_error() {
        let root = tmp();
        // Invalid UTF-8 byte sequence.
        let payload: &[u8] = &[0xFF, 0xFF, 0xFE, b'\n'];
        let p = root.join("src/bad.c");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, payload).unwrap();
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(
            &root,
            "inclean.toml",
            &format!(
                "{}\n[[rule]]\nname = \"base\"\nfile_paths = [\"src/**/*\"]\naction = {{ type = \"keep\" }}\n",
                min_inclean_toml()
            ),
        );
        let summary = run(None, &root, &[], None, CheckMode::Run).unwrap();
        assert_eq!(summary.skipped.len(), 1);
        assert_eq!(summary.skipped[0].relpath, PathBuf::from("src/bad.c"));
        assert_eq!(summary_exit_code(&summary), 0);
        fs::remove_dir_all(&root).ok();
    }
}
