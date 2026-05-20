//! Top-level orchestration: load configs, walk the file tree, evaluate
//! rules per include, and report per-file outcomes. The CLI subcommands
//! consume the [`Summary`] this returns to render check / diff / apply
//! output.
//!
//! v1 keeps things simple:
//! - The file walk uses `ignore::WalkBuilder` honoring `.gitignore` /
//!   `.ignore` plus our hard-coded skip dirs (`.git`, `target`,
//!   `node_modules`).
//! - Filtering is opportunistic: a file is opened only if at least one
//!   compiled rule's PathMatcher matches it and its config_dir is an
//!   ancestor of (or equal to) the file's directory.
//! - Per-file edits are applied in reverse byte order so earlier ranges
//!   stay valid.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

use crate::config::discover::{self, CONFIG_FILENAME};
use crate::config::{inherit, lint};
use crate::config::schema::IncludeForm;
use crate::lex::include_line::{self, Include};
use crate::rule::action::{self, Outcome};
use crate::rule::engine::{self, CompiledRule};
use crate::validate::allowed as validate_allowed;

/// Outcome for a whole `inclean` invocation against a project root.
#[derive(Debug)]
pub struct Summary {
    pub project_root: PathBuf,
    pub files: Vec<FileResult>,
    pub config_warnings: Vec<lint::Warning>,
}

/// Per-file result.
#[derive(Debug)]
pub struct FileResult {
    pub relpath: PathBuf,
    pub original: String,
    /// `Some(_)` if any include was rewritten; equal to `original` otherwise.
    pub rewritten: Option<String>,
    pub include_results: Vec<IncludeResult>,
}

#[derive(Debug)]
pub struct IncludeResult {
    pub include: Include,
    pub outcome: IncludeOutcome,
    /// Post-action validation message; `Some(_)` when the resulting
    /// include cannot be resolved under the matched rule's
    /// `allowed_include_dirs` (or angle-pattern subset thereof).
    pub validation_error: Option<String>,
}

#[derive(Debug)]
pub enum IncludeOutcome {
    NoMatch,
    Keep {
        rule: String,
    },
    Rewritten {
        rule: String,
        argument_range: Range<usize>,
        new_text: String,
    },
    /// Action evaluation chose to abort the file or report an error.
    Error {
        rule: String,
        message: String,
    },
    /// Evaluation itself failed (resolution missed, etc.) — distinct from
    /// the `action.error` variant.
    EvaluationFailure {
        rule: String,
        message: String,
    },
}

/// Run the full pipeline against `project_root`. When `validate` is true,
/// each include is also checked against the matched rule's
/// `allowed_include_dirs` after the action runs.
pub fn run(project_root: &Path, validate: bool) -> Result<Summary> {
    let project_root_abs = std::fs::canonicalize(project_root)
        .with_context(|| format!("canonicalize {}", project_root.display()))?;

    let configs = discover::load_all_configs(&project_root_abs)?;
    discover::validate_loaded(&configs, &project_root_abs)?;
    let resolved = inherit::resolve(&configs)?;
    let config_warnings = lint::check(&resolved);

    let compiled = compile_rules(&resolved, &project_root_abs)?;

    let mut files: Vec<FileResult> = Vec::new();
    for entry in source_files(&project_root_abs) {
        let entry = entry?;
        let relpath = entry
            .strip_prefix(&project_root_abs)
            .unwrap_or(&entry)
            .to_path_buf();

        if !any_rule_eligible(&compiled, &relpath) {
            continue;
        }

        let original = std::fs::read_to_string(&entry)
            .with_context(|| format!("reading {}", entry.display()))?;
        let result = process_file(&compiled, &relpath, &original, &project_root_abs, validate);
        files.push(FileResult {
            relpath,
            original: result.original,
            rewritten: result.rewritten,
            include_results: result.include_results,
        });
    }

    Ok(Summary {
        project_root: project_root_abs,
        files,
        config_warnings,
    })
}

/// Apply the rewrites in `summary` to disk. Files whose include results
/// include any `Error` or `EvaluationFailure` outcome are **skipped**:
/// partial writes risk leaving the file in an inconsistent state.
/// Returns the number of files actually written.
pub fn apply(summary: &Summary) -> Result<usize> {
    let mut written = 0;
    for f in &summary.files {
        if file_has_errors(f) {
            continue;
        }
        if let Some(new) = &f.rewritten {
            let path = summary.project_root.join(&f.relpath);
            std::fs::write(&path, new).with_context(|| format!("writing {}", path.display()))?;
            written += 1;
        }
    }
    Ok(written)
}

fn file_has_errors(f: &FileResult) -> bool {
    f.include_results.iter().any(|r| {
        matches!(
            r.outcome,
            IncludeOutcome::Error { .. } | IncludeOutcome::EvaluationFailure { .. }
        ) || r.validation_error.is_some()
    })
}

/// Render a unified diff for every changed file in `summary`.
pub fn render_diff(summary: &Summary) -> String {
    use similar::TextDiff;
    let mut out = String::new();
    for f in &summary.files {
        let Some(new) = &f.rewritten else { continue };
        let diff = TextDiff::from_lines(&f.original, new);
        let a_label = format!("a/{}", f.relpath.display());
        let b_label = format!("b/{}", f.relpath.display());
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

fn compile_rules<'a>(
    rules: &'a BTreeMap<String, inherit::ResolvedRule>,
    project_root: &Path,
) -> Result<Vec<CompiledRule<'a>>> {
    rules
        .values()
        .map(|r| CompiledRule::new(r, project_root))
        .collect()
}

fn source_files(root: &Path) -> impl Iterator<Item = Result<PathBuf>> {
    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .filter_entry(|entry| {
            let name = entry.file_name();
            !matches!(
                name.to_str(),
                Some(".git") | Some("target") | Some("node_modules")
            )
        })
        .build();
    walker
        .filter_map(|res| match res {
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
    let file_dir = file_relpath.parent().unwrap_or_else(|| Path::new(""));
    rules.iter().any(|r| {
        is_ancestor_or_self(&r.config_dir_relpath, file_dir) && r.path_matcher.matches(file_relpath)
    })
}

fn is_ancestor_or_self(dir: &Path, descendant: &Path) -> bool {
    let dir_components: Vec<_> = dir.components().collect();
    let desc_components: Vec<_> = descendant.components().collect();
    if dir_components.len() > desc_components.len() {
        return false;
    }
    dir_components
        .iter()
        .zip(desc_components.iter())
        .all(|(a, b)| a == b)
}

struct FileProcessing {
    original: String,
    rewritten: Option<String>,
    include_results: Vec<IncludeResult>,
}

fn process_file(
    rules: &[CompiledRule<'_>],
    relpath: &Path,
    original: &str,
    project_root: &Path,
    validate: bool,
) -> FileProcessing {
    let includes = include_line::scan(original);
    let mut include_results = Vec::with_capacity(includes.len());
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();

    for include in includes {
        let m = engine::find_match(rules, relpath, &include);
        let (outcome, matched_rule_for_validation): (IncludeOutcome, Option<&inherit::ResolvedRule>) =
            match m {
                None => (IncludeOutcome::NoMatch, None),
                Some(matched) => {
                    let rule_name = matched.rule.rule.name.clone();
                    let rule_ref = matched.rule.rule;
                    match action::evaluate(&matched, &include, relpath, project_root) {
                        Ok(Outcome::Keep) => (
                            IncludeOutcome::Keep { rule: rule_name },
                            Some(rule_ref),
                        ),
                        Ok(Outcome::Rewrite { argument_range, new_text }) => {
                            let unchanged = original
                                .get(argument_range.clone())
                                .map(|s| s == new_text)
                                .unwrap_or(false);
                            if unchanged {
                                (
                                    IncludeOutcome::Keep { rule: rule_name },
                                    Some(rule_ref),
                                )
                            } else {
                                edits.push((argument_range.clone(), new_text.clone()));
                                (
                                    IncludeOutcome::Rewritten {
                                        rule: rule_name,
                                        argument_range,
                                        new_text,
                                    },
                                    Some(rule_ref),
                                )
                            }
                        }
                        Ok(Outcome::Error { message }) => (
                            IncludeOutcome::Error {
                                rule: rule_name,
                                message,
                            },
                            None,
                        ),
                        Err(err) => (
                            IncludeOutcome::EvaluationFailure {
                                rule: rule_name,
                                message: format!("{err:#}"),
                            },
                            None,
                        ),
                    }
                }
            };

        let validation_error = if validate {
            run_validation(&include, &outcome, matched_rule_for_validation, project_root)
        } else {
            None
        };

        include_results.push(IncludeResult {
            include,
            outcome,
            validation_error,
        });
    }

    let rewritten = if edits.is_empty() {
        None
    } else {
        Some(apply_edits(original, &edits))
    };
    FileProcessing {
        original: original.to_string(),
        rewritten,
        include_results,
    }
}

/// Compute the include text as it will exist after the action runs, then
/// dispatch to `validate::allowed::validate`. Returns `Some(_)` when the
/// final include cannot resolve under the matched rule's allowed dirs.
fn run_validation(
    include: &Include,
    outcome: &IncludeOutcome,
    rule: Option<&inherit::ResolvedRule>,
    project_root: &Path,
) -> Option<String> {
    let rule = rule?;
    let (form, content): (IncludeForm, String) = match outcome {
        IncludeOutcome::Keep { .. } => (include.form, include.content.clone()),
        IncludeOutcome::Rewritten { new_text, .. } => parse_argument_text(new_text)?,
        // NoMatch / Error / EvaluationFailure are not validated.
        _ => return None,
    };
    validate_allowed::validate(form, &content, rule, project_root)
}

/// Parse a freshly-formatted include argument like `"foo.h"` or `<bar.h>`
/// into a `(form, content)` pair. Returns `None` for malformed inputs.
fn parse_argument_text(s: &str) -> Option<(IncludeForm, String)> {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        Some((IncludeForm::Quote, s[1..s.len() - 1].to_string()))
    } else if bytes.len() >= 2 && bytes[0] == b'<' && bytes[bytes.len() - 1] == b'>' {
        Some((IncludeForm::Angle, s[1..s.len() - 1].to_string()))
    } else {
        Some((IncludeForm::Macro, s.to_string()))
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

// ---- Exit-code helpers used by check / apply -------------------------------

/// Highest-severity outcome across the whole summary, in the order the
/// CLI exit codes describe (1 = user config error, 2 = action.error,
/// 3 = evaluation failure / would-be-validation failure, 0 = clean).
pub fn summary_exit_code(summary: &Summary) -> u8 {
    let mut code: u8 = 0;
    for f in &summary.files {
        for r in &f.include_results {
            match &r.outcome {
                IncludeOutcome::Error { .. } => code = code.max(2),
                IncludeOutcome::EvaluationFailure { .. } => code = code.max(3),
                _ => {}
            }
            if r.validation_error.is_some() {
                code = code.max(3);
            }
        }
    }
    code
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

    #[test]
    fn end_to_end_auto_rewrite_under_allowed_include() {
        let root = tmp();
        touch(&root, "include/internal/foo.h", "");
        touch(
            &root,
            "src/main.c",
            "#include \"foo.h\"\nint main(){return 0;}\n",
        );
        touch(
            &root,
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            allowed_include_dirs = ["include"]
            original_include_dirs = ["include/internal"]
            "#,
        );

        let summary = run(&root, true).unwrap();
        let file = &summary.files[0];
        assert_eq!(file.relpath, PathBuf::from("src/main.c"));
        let rewritten = file.rewritten.as_ref().expect("should be rewritten");
        assert!(rewritten.contains("\"internal/foo.h\""));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn end_to_end_error_action_is_reported() {
        let root = tmp();
        touch(&root, "src/main.c", "#include \"old_x.h\"\n");
        touch(
            &root,
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^old_(.+)$'
            action = { type = "error", message = "deprecated: ${1}" }
            "#,
        );
        let summary = run(&root, true).unwrap();
        let outcomes: Vec<_> = summary
            .files
            .iter()
            .flat_map(|f| f.include_results.iter().map(|r| &r.outcome))
            .collect();
        assert!(matches!(outcomes[0], IncludeOutcome::Error { .. }));
        assert_eq!(summary_exit_code(&summary), 2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn apply_writes_rewritten_files_only() {
        let root = tmp();
        touch(&root, "include/foo.h", "");
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(&root, "src/other.c", "int x;\n");
        touch(
            &root,
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            allowed_include_dirs = ["include"]
            original_include_dirs = ["include"]
            "#,
        );
        let summary = run(&root, true).unwrap();
        let written = apply(&summary).unwrap();
        // only main.c is rewritten (no-op for other.c which had no includes)
        assert_eq!(written, 0); // include "foo.h" is already canonical → no edits
        let new = std::fs::read_to_string(root.join("src/main.c")).unwrap();
        assert!(new.contains("\"foo.h\""));
        // other.c untouched.
        let other = std::fs::read_to_string(root.join("src/other.c")).unwrap();
        assert_eq!(other, "int x;\n");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn apply_skips_files_that_have_errors() {
        let root = tmp();
        touch(
            &root,
            "src/main.c",
            "#include \"old_x.h\"\n#include \"new_y.h\"\n",
        );
        touch(
            &root,
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "deprecate"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^old_(.+)$'
            action = { type = "error", message = "deprecated" }

            [[rule]]
            name = "rewrite"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^new_(.+)$'
            action = { type = "rewrite", to = "renamed/${1}" }
            "#,
        );
        let summary = run(&root, true).unwrap();
        let written = apply(&summary).unwrap();
        // The file mixes an error and a rewrite → apply skips it entirely.
        assert_eq!(written, 0);
        let after = std::fs::read_to_string(root.join("src/main.c")).unwrap();
        assert!(after.contains("\"new_y.h\"")); // unmodified
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn render_diff_emits_changed_files_only() {
        let root = tmp();
        touch(&root, "include/foo.h", "");
        touch(&root, "src/main.c", "#include \"foo.h\"\n");
        touch(&root, "src/other.c", "int x;\n");
        touch(
            &root,
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            allowed_include_dirs = ["include"]
            original_include_dirs = ["include"]
            action = { type = "rewrite", to = "renamed/${include.text}" }
            "#,
        );
        let summary = run(&root, true).unwrap();
        let d = render_diff(&summary);
        assert!(d.contains("--- a/src/main.c"));
        assert!(!d.contains("other.c"));
        fs::remove_dir_all(&root).ok();
    }
}
