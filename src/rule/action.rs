//! Evaluate a matched rule's action against an `#include`.
//!
//! Each rule produces an [`Outcome`] for each include it matches. The
//! pipeline collects every rule's outcome for a single include and
//! decides conflict-by-final-text: identical `Rewrite::new_text` across
//! all matches means "no conflict"; any divergence is a conflict.
//!
//! Action variants in v0.3 (one per `RawAction`):
//!
//! * `Resolve` — consume the header path selected by the matcher from
//!   `include_directories`; rewrite the path relative to `relative_to`.
//! * `Replace` — textual substitution of the include argument with `with`.
//! * `Keep` — leave the argument unchanged; `output_form` may still
//!   rewrite quote↔angle.
//! * `Remove` — delete the whole include line (with knobs for blank line
//!   and trailing comment preservation).
//! * `CommentOut` — wrap the include line in `//` or `/* */`.
//! * `Error` — produce a configured `Error` outcome (exit code 2).
//! * whole-field `action = "skip"` — the rule contributes no action
//!   candidate; its trailing-comment policy may still run.
//!
//! Placeholders:
//! * `${current_file}` — project-relative path of the file being edited
//!   (forward slashes).
//! * `${original}` — the original include argument (no quotes/angles)
//!   when used in an action template, or the original trailing-comment
//!   body when used in a trailing-comment template.
//! * `${copied}` is substituted at copy-resolution time (M2), not here.

use std::ops::Range;
use std::path::{Path, PathBuf};

use super::engine::CompiledRule;
use crate::config::copy::{ResolvedAction, ResolvedTrailingAction, ResolvedTrailingComment};
use crate::config::schema::{CommentStyle, IncludeForm, OutputCommentStyle, OutputForm};
use crate::lex::include_line::Include;
use crate::utils::PathExt;

/// The result of evaluating one rule's action against one include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No edit. The bytes on disk are kept as-is for this rule.
    Keep,
    /// Replace bytes in `edit_range` with `new_text`. `edit_range`
    /// always spans the full physical line of the include (including
    /// the line-terminating `\n` when present) for `Remove`/`CommentOut`,
    /// and spans `[argument_start, line_end_excl_newline)` for
    /// `Resolve` / `Replace` / `Keep`. This makes pipeline-side conflict
    /// detection a clean string compare.
    Rewrite {
        edit_range: Range<usize>,
        new_text: String,
    },
    /// Rule's `action = error` matched.
    Error { message: String },
    /// Rule's `trailing_comment.transform.action = error` matched. Kept
    /// separate from `Error` so the unfixable report can label the cause
    /// distinctly per refactor.md §"inclean apply".
    TrailingCommentError { message: String },
    /// Action evaluation failed at runtime (e.g. `resolve` had no selected
    /// header path, a directory-resolution policy failed, ...).
    EvaluationFailure { message: String },
}

/// The action-side result for a matched rule. `Skip` means this rule matched
/// but does not participate in action conflict detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Skip,
    Apply {
        edit_range: Range<usize>,
        new_text: String,
    },
    Error {
        message: String,
    },
    EvaluationFailure {
        message: String,
    },
}

/// The trailing-comment-side result for a matched rule. `Skip` means this
/// rule matched but does not participate in trailing-comment conflict
/// detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrailingOutcome {
    Skip,
    Apply { new_text: String },
    Error { message: String },
}

/// Evaluate `rule.rule.action` against `include` in `source`.
pub fn evaluate(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
) -> Outcome {
    evaluate_with_resolution(rule, include, source, file_relpath, project_root, None)
}

/// Evaluate only the rule's action field. Trailing-comment policy is handled
/// separately by [`evaluate_trailing`].
pub fn evaluate_action(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
) -> ActionOutcome {
    evaluate_action_with_resolution(rule, include, source, file_relpath, project_root, None)
}

/// Evaluate only the rule's action field with an optional header path selected
/// by the matcher.
pub fn evaluate_action_with_resolution(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
) -> ActionOutcome {
    if matches!(rule.rule.action, ResolvedAction::Skip) {
        return ActionOutcome::Skip;
    }
    if include.form == IncludeForm::Macro {
        return ActionOutcome::Error {
            message: macro_form_error(rule, include, file_relpath),
        };
    }

    let ctx = TemplateCtx {
        current_file: file_relpath.to_slash(),
        original_include: include.content.clone(),
    };

    match &rule.rule.action {
        ResolvedAction::Skip => ActionOutcome::Skip,
        ResolvedAction::Error { message } => ActionOutcome::Error {
            message: substitute_action(message, &ctx),
        },
        ResolvedAction::Resolve {
            relative_to,
            output_form,
            message: _,
        } => apply_resolve_action(
            rule,
            include,
            file_relpath,
            project_root,
            resolved_header,
            relative_to,
            *output_form,
            &ctx,
        ),
        ResolvedAction::Replace {
            with,
            output_form,
            message: _,
        } => apply_replace_action(include, with, *output_form, &ctx),
        ResolvedAction::Keep {
            output_form,
            message: _,
        } => apply_keep_action(include, source, *output_form),
        ResolvedAction::Remove {
            keep_blank_line,
            keep_trailing_comment,
            message: _,
        } => outcome_to_action(apply_remove(
            include,
            source,
            *keep_blank_line,
            *keep_trailing_comment,
        )),
        ResolvedAction::CommentOut { style, message: _ } => {
            outcome_to_action(apply_comment_out(include, source, *style))
        }
    }
}

/// Evaluate only the rule's trailing-comment field. Rules whose action is
/// `remove`, `comment_out`, or `error` keep the historical behavior where
/// trailing-comment policy is ignored.
pub fn evaluate_trailing(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
) -> TrailingOutcome {
    if rule.rule.trailing_comment.skip
        || matches!(
            rule.rule.action,
            ResolvedAction::Remove { .. }
                | ResolvedAction::CommentOut { .. }
                | ResolvedAction::Error { .. }
        )
    {
        return TrailingOutcome::Skip;
    }

    let ctx = TemplateCtx {
        current_file: file_relpath.to_slash(),
        original_include: include.content.clone(),
    };
    match process_trailing(rule, include, source, &ctx) {
        Ok(new_text) => TrailingOutcome::Apply { new_text },
        Err(Outcome::TrailingCommentError { message }) => TrailingOutcome::Error { message },
        Err(other) => unreachable!("process_trailing can only return trailing errors: {other:?}"),
    }
}

/// Evaluate an action with an optional header path selected by the matcher.
pub fn evaluate_with_resolution(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
) -> Outcome {
    // Macro-form hatch: never evaluate an action against a macro #include.
    if include.form == IncludeForm::Macro && !matches!(rule.rule.action, ResolvedAction::Skip) {
        return Outcome::Error {
            message: macro_form_error(rule, include, file_relpath),
        };
    }

    let ctx = TemplateCtx {
        current_file: file_relpath.to_slash(),
        original_include: include.content.clone(),
    };

    match &rule.rule.action {
        ResolvedAction::Skip => Outcome::Keep,
        ResolvedAction::Error { message } => Outcome::Error {
            message: substitute_action(message, &ctx),
        },
        ResolvedAction::Resolve {
            relative_to,
            output_form,
            message: _,
        } => apply_resolve(
            rule,
            include,
            source,
            file_relpath,
            project_root,
            resolved_header,
            relative_to,
            *output_form,
            &ctx,
        ),
        ResolvedAction::Replace {
            with,
            output_form,
            message: _,
        } => apply_replace(rule, include, source, with, *output_form, &ctx),
        ResolvedAction::Keep {
            output_form,
            message: _,
        } => apply_keep(rule, include, source, *output_form, &ctx),
        ResolvedAction::Remove {
            keep_blank_line,
            keep_trailing_comment,
            message: _,
        } => apply_remove(include, source, *keep_blank_line, *keep_trailing_comment),
        ResolvedAction::CommentOut { style, message: _ } => {
            apply_comment_out(include, source, *style)
        }
    }
}

fn macro_form_error(rule: &CompiledRule<'_>, include: &Include, file_relpath: &Path) -> String {
    format!(
        "macro-form include was not statically expanded; rule `{}` matched `#include {}` at {}:{}",
        rule.rule.name,
        include.content,
        file_relpath.display(),
        include.line,
    )
}

// ---- Template context ----------------------------------------------------

struct TemplateCtx {
    current_file: String,
    /// Original include argument with no quotes/angles. Used in action
    /// templates; for trailing-comment templates we pass a different
    /// `${original}` (the comment body) via [`substitute_trailing`].
    original_include: String,
}

fn substitute_action(template: &str, ctx: &TemplateCtx) -> String {
    template
        .replace("${current_file}", &ctx.current_file)
        .replace("${original}", &ctx.original_include)
}

fn substitute_trailing(template: &str, ctx: &TemplateCtx, original_comment_body: &str) -> String {
    template
        .replace("${current_file}", &ctx.current_file)
        .replace("${original}", original_comment_body)
}

// ---- Edit-range helpers ---------------------------------------------------

/// Byte range covering the include's argument + any same-line trailing
/// content (excluding `\r` / `\n` at end of line).
fn argument_and_trailing_range(include: &Include) -> Range<usize> {
    let start = include.argument_range.start;
    let end = include.trailing_range.end.max(include.argument_range.end);
    start..end
}

/// Byte range covering the entire physical line containing `include`,
/// from line start through the line-terminating `\n` (inclusive). Used
/// for `Remove` / `CommentOut` which act on the whole line.
fn full_line_range(include: &Include, source: &str) -> Range<usize> {
    let bytes = source.as_bytes();
    // Find the start of this line by walking back from argument_range.start.
    let mut start = include.argument_range.start;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    // Find the line end (one past the `\n`, or EOF).
    let mut end = include.trailing_range.end.max(include.argument_range.end);
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    if end < bytes.len() {
        end += 1; // include the `\n`
    }
    start..end
}

/// Detect this line's indent (whitespace before the include's `#`).
fn line_indent<'a>(include: &Include, source: &'a str) -> &'a str {
    let bytes = source.as_bytes();
    let mut start = include.argument_range.start;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    // `start` now sits at the first byte of the line. Find the `#`.
    let mut p = start;
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    &source[start..p]
}

/// Line-terminator (LF or CRLF) used by this line.
fn line_terminator<'a>(include: &Include, source: &'a str) -> &'a str {
    let bytes = source.as_bytes();
    let mut end = include.trailing_range.end.max(include.argument_range.end);
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    if end >= bytes.len() {
        return "";
    }
    if end > 0 && bytes[end - 1] == b'\r' {
        &source[end - 1..end + 1]
    } else {
        &source[end..end + 1]
    }
}

// ---- format_argument -----------------------------------------------------

fn format_argument(content: &str, form: OutputForm, fallback: IncludeForm) -> String {
    let resolved_form = match form {
        OutputForm::Quote => IncludeForm::Quote,
        OutputForm::Angle => IncludeForm::Angle,
        OutputForm::Preserve => fallback,
    };
    match resolved_form {
        IncludeForm::Quote => format!("\"{content}\""),
        IncludeForm::Angle => format!("<{content}>"),
        IncludeForm::Macro => content.to_string(),
    }
}

// ---- Action implementations ----------------------------------------------

fn apply_resolve_action(
    rule: &CompiledRule<'_>,
    include: &Include,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
    relative_to: &str,
    output_form: OutputForm,
    ctx: &TemplateCtx,
) -> ActionOutcome {
    let dirs = &rule.rule.include_directories;
    if dirs.is_empty() {
        return ActionOutcome::EvaluationFailure {
            message: format!(
                "rule `{}`: action `resolve` requires `include_directories`",
                rule.rule.name
            ),
        };
    }
    let Some(resolved_abs) = resolved_header else {
        return ActionOutcome::EvaluationFailure {
            message: format!(
                "rule `{}`: action `resolve` requires a resolved include path from `include_directories`",
                rule.rule.name
            ),
        };
    };
    let resolved_rel = resolved_abs
        .strip_prefix(project_root)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| resolved_abs.to_path_buf());

    let relative_to_substituted = substitute_action(relative_to, ctx);
    let base_dir: PathBuf = if relative_to_substituted == ctx.current_file {
        file_relpath
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    } else {
        PathBuf::from(&relative_to_substituted)
    };
    let new_path = relative_path(&resolved_rel, &base_dir);
    let new_path_str = new_path.to_slash();
    let new_arg = format_argument(&new_path_str, output_form, include.form);

    ActionOutcome::Apply {
        edit_range: include.argument_range.clone(),
        new_text: new_arg,
    }
}

fn apply_replace_action(
    include: &Include,
    with: &str,
    output_form: OutputForm,
    ctx: &TemplateCtx,
) -> ActionOutcome {
    let new_content = substitute_action(with, ctx);
    let new_arg = format_argument(&new_content, output_form, include.form);
    ActionOutcome::Apply {
        edit_range: include.argument_range.clone(),
        new_text: new_arg,
    }
}

fn apply_keep_action(include: &Include, source: &str, output_form: OutputForm) -> ActionOutcome {
    let new_arg = format_argument(&include.content, output_form, include.form);
    let original = &source[include.argument_range.clone()];
    ActionOutcome::Apply {
        edit_range: include.argument_range.clone(),
        new_text: if new_arg == original {
            original.to_string()
        } else {
            new_arg
        },
    }
}

fn outcome_to_action(outcome: Outcome) -> ActionOutcome {
    match outcome {
        Outcome::Keep => ActionOutcome::Skip,
        Outcome::Rewrite {
            edit_range,
            new_text,
        } => ActionOutcome::Apply {
            edit_range,
            new_text,
        },
        Outcome::Error { message } => ActionOutcome::Error { message },
        Outcome::EvaluationFailure { message } => ActionOutcome::EvaluationFailure { message },
        Outcome::TrailingCommentError { .. } => {
            unreachable!("action-only evaluation cannot produce trailing errors")
        }
    }
}

fn apply_resolve(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
    resolved_header: Option<&Path>,
    relative_to: &str,
    output_form: OutputForm,
    ctx: &TemplateCtx,
) -> Outcome {
    let dirs = &rule.rule.include_directories;
    if dirs.is_empty() {
        return Outcome::EvaluationFailure {
            message: format!(
                "rule `{}`: action `resolve` requires `include_directories`",
                rule.rule.name
            ),
        };
    }
    let Some(resolved_abs) = resolved_header else {
        return Outcome::EvaluationFailure {
            message: format!(
                "rule `{}`: action `resolve` requires a resolved include path from `include_directories`",
                rule.rule.name
            ),
        };
    };
    let resolved_rel = resolved_abs
        .strip_prefix(project_root)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| resolved_abs.to_path_buf());

    // Compute the path relative to `relative_to` (after `${current_file}`
    // substitution). When `relative_to` resolves to `${current_file}`, we
    // mean the directory of the current file.
    let relative_to_substituted = substitute_action(relative_to, ctx);
    let base_dir: PathBuf = if relative_to_substituted == ctx.current_file {
        file_relpath
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    } else {
        PathBuf::from(&relative_to_substituted)
    };
    let new_path = relative_path(&resolved_rel, &base_dir);
    let new_path_str = new_path.to_slash();
    let new_arg = format_argument(&new_path_str, output_form, include.form);

    rewrite_argument_and_trailing(rule, include, source, &new_arg, ctx)
}

fn apply_replace(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    with: &str,
    output_form: OutputForm,
    ctx: &TemplateCtx,
) -> Outcome {
    let new_content = substitute_action(with, ctx);
    let new_arg = format_argument(&new_content, output_form, include.form);
    rewrite_argument_and_trailing(rule, include, source, &new_arg, ctx)
}

fn apply_keep(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    output_form: OutputForm,
    ctx: &TemplateCtx,
) -> Outcome {
    let new_arg = format_argument(&include.content, output_form, include.form);
    rewrite_argument_and_trailing(rule, include, source, &new_arg, ctx)
}

fn apply_remove(
    include: &Include,
    source: &str,
    keep_blank_line: bool,
    keep_trailing_comment: bool,
) -> Outcome {
    let range = full_line_range(include, source);
    let mut new_text = String::new();
    if keep_trailing_comment && !include.trailing_range.is_empty() {
        // Preserve the trailing comment on its own line.
        let trailing = &source[include.trailing_range.clone()];
        new_text.push_str(trailing.trim_start());
        new_text.push_str(line_terminator(include, source));
    } else if keep_blank_line {
        new_text.push_str(line_terminator(include, source));
    }
    Outcome::Rewrite {
        edit_range: range,
        new_text,
    }
}

fn apply_comment_out(include: &Include, source: &str, style: CommentStyle) -> Outcome {
    let range = full_line_range(include, source);
    let indent = line_indent(include, source);
    let terminator = line_terminator(include, source);
    // Content of the line excluding indent and terminator.
    let line_inner_start = range.start + indent.len();
    let line_inner_end = range.end - terminator.len();
    let inner = &source[line_inner_start..line_inner_end];
    let new_text = match style {
        CommentStyle::Line => format!("{indent}// {inner}{terminator}"),
        CommentStyle::Block => format!("{indent}/* {inner} */{terminator}"),
    };
    Outcome::Rewrite {
        edit_range: range,
        new_text,
    }
}

// ---- Trailing-comment processing shared by Resolve / Replace / Keep -------

fn rewrite_argument_and_trailing(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    new_arg: &str,
    ctx: &TemplateCtx,
) -> Outcome {
    let trailing_text = match process_trailing(rule, include, source, ctx) {
        Ok(t) => t,
        Err(err) => return err,
    };
    let edit_range = argument_and_trailing_range(include);
    let new_text = format!("{new_arg}{trailing_text}");
    let original = &source[edit_range.clone()];
    if new_text == original {
        Outcome::Keep
    } else {
        Outcome::Rewrite {
            edit_range,
            new_text,
        }
    }
}

/// Compute the trailing-comment text that should sit between the new
/// argument and the EOL. Returns an error outcome if the trailing
/// transform's action is `error` and the existing comment matches it.
fn process_trailing(
    rule: &CompiledRule<'_>,
    include: &Include,
    source: &str,
    ctx: &TemplateCtx,
) -> std::result::Result<String, Outcome> {
    let tc: &ResolvedTrailingComment = &rule.rule.trailing_comment;
    let original_trailing = &source[include.trailing_range.clone()];
    if tc.skip {
        return Ok(original_trailing.to_string());
    }

    // Cross-line block comment: per refactor.md §"Trailing comment 的
    // 定义", such constructs do NOT count as trailing comments. Skip
    // both `transform` AND `append_if_absent` entirely. When the cross-line
    // block is after a complete same-line comment, `original_trailing`
    // still contains that same-line prefix and must be preserved.
    if include.has_cross_line_block_trailing {
        return Ok(original_trailing.to_string());
    }

    // Has a recognised trailing comment to start with?
    let style = include.trailing_comment_style;
    let first_comment = style.and_then(|s| split_first_trailing_comment(original_trailing, s));

    // Run the transform (if configured) and the style matches.
    if let (Some(transform), Some(content_re)) =
        (&tc.transform, rule.trailing_content_regex.as_ref())
        && let Some(s) = style
        && transform.match_styles.contains(&s)
        && let Some((first_comment_text, suffix)) = first_comment
    {
        let body = extract_comment_body(first_comment_text, s);
        if content_re.is_match(&body) {
            match run_transform_action(rule, &transform.action, s, &body, ctx) {
                Ok(Some(text)) => return Ok(format!("{text}{suffix}")),
                Ok(None) => {
                    // Removed. If later same-line trailing bytes remain,
                    // preserve them and do not append a replacement comment.
                    if suffix.is_empty() {
                        return apply_append_if_absent(tc, "");
                    }
                    return Ok(suffix.to_string());
                }
                Err(o) => return Err(o),
            }
        }
    }

    // Either no transform configured, or transform did not match. Keep
    // the existing trailing exactly as-is, and only consider
    // append_if_absent when there was no trailing comment.
    if !original_trailing.is_empty() {
        return Ok(original_trailing.to_string());
    }
    apply_append_if_absent(tc, original_trailing)
}

fn split_first_trailing_comment(trailing: &str, style: CommentStyle) -> Option<(&str, &str)> {
    let bytes = trailing.as_bytes();
    let mut start = 0usize;
    while start < bytes.len() && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }

    match style {
        CommentStyle::Line => {
            if bytes.get(start) == Some(&b'/') && bytes.get(start + 1) == Some(&b'/') {
                Some((trailing, ""))
            } else {
                None
            }
        }
        CommentStyle::Block => {
            if bytes.get(start) != Some(&b'/') || bytes.get(start + 1) != Some(&b'*') {
                return None;
            }
            let mut end = start + 2;
            while end + 1 < bytes.len() {
                if bytes[end] == b'*' && bytes[end + 1] == b'/' {
                    let first_end = end + 2;
                    return Some((&trailing[..first_end], &trailing[first_end..]));
                }
                end += 1;
            }
            None
        }
    }
}

fn extract_comment_body(trailing: &str, style: CommentStyle) -> String {
    let s = trailing.trim_start_matches([' ', '\t']);
    let body = match style {
        CommentStyle::Line => s.trim_start_matches("//"),
        CommentStyle::Block => {
            let without_open = s.trim_start_matches("/*");
            without_open.trim_end_matches("*/")
        }
    };
    body.trim().to_string()
}

/// Returns `Ok(Some(text))` for a fresh trailing-comment string,
/// `Ok(None)` for "removed", or `Err(Outcome)` for `error` / failure.
fn run_transform_action(
    _rule: &CompiledRule<'_>,
    action: &ResolvedTrailingAction,
    existing_style: CommentStyle,
    existing_body: &str,
    ctx: &TemplateCtx,
) -> std::result::Result<Option<String>, Outcome> {
    match action {
        ResolvedTrailingAction::Error { message } => Err(Outcome::TrailingCommentError {
            message: substitute_trailing(message, ctx, existing_body),
        }),
        ResolvedTrailingAction::Remove { message: _ } => Ok(None),
        ResolvedTrailingAction::Keep {
            output_style,
            message: _,
        } => {
            let out_style = pick_comment_style(*output_style, existing_style);
            Ok(Some(format_trailing(existing_body, out_style)))
        }
        ResolvedTrailingAction::Replace {
            with,
            output_style,
            message: _,
        } => {
            let new_body = substitute_trailing(with, ctx, existing_body);
            let out_style = pick_comment_style(*output_style, existing_style);
            Ok(Some(format_trailing(&new_body, out_style)))
        }
    }
}

fn pick_comment_style(out: OutputCommentStyle, existing: CommentStyle) -> CommentStyle {
    match out {
        OutputCommentStyle::Line => CommentStyle::Line,
        OutputCommentStyle::Block => CommentStyle::Block,
        OutputCommentStyle::Preserve => existing,
    }
}

fn format_trailing(body: &str, style: CommentStyle) -> String {
    match style {
        CommentStyle::Line => format!("  // {body}"),
        CommentStyle::Block => format!("  /* {body} */"),
    }
}

fn apply_append_if_absent(
    tc: &ResolvedTrailingComment,
    existing_trailing: &str,
) -> std::result::Result<String, Outcome> {
    if existing_trailing.is_empty()
        && let Some(text) = &tc.append_if_absent
    {
        return Ok(text.clone());
    }
    Ok(existing_trailing.to_string())
}

// ---- relative-path helper -------------------------------------------------

fn relative_path(target: &Path, base: &Path) -> PathBuf {
    // pathdiff-like; we want target expressed relative to base. Both are
    // project-relative. Walk up `base` until it's a prefix of `target`,
    // emitting `..` for each step.
    let target_comps: Vec<_> = target.components().collect();
    let base_comps: Vec<_> = base.components().collect();
    let mut common = 0;
    while common < target_comps.len()
        && common < base_comps.len()
        && target_comps[common] == base_comps[common]
    {
        common += 1;
    }
    let ups = base_comps.len() - common;
    let mut out = PathBuf::new();
    for _ in 0..ups {
        out.push("..");
    }
    for c in &target_comps[common..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::copy::resolve;
    use crate::config::schema::IncludeForm;
    use crate::utils::testing::config::load_rules;

    fn compile_rules(body: &str) -> Vec<CompiledRule<'static>> {
        let lc = load_rules(body);
        let resolved = resolve(&[lc]).unwrap();
        // Leak to get a 'static lifetime for the test.
        let leaked: &'static _ = Box::leak(Box::new(resolved));
        leaked
            .iter()
            .map(|(_, r)| CompiledRule::new(r).unwrap())
            .collect()
    }

    /// Lex `src`, find the first include, and return it.
    fn first_include(src: &str) -> (String, crate::lex::include_line::Include) {
        let incs = crate::lex::include_line::scan(src);
        (src.to_string(), incs.into_iter().next().unwrap())
    }

    #[test]
    fn keep_with_default_output_form_is_a_noop() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn keep_with_output_form_angle_rewrites_to_angle() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep", output_form = "angle" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "<foo.h>"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn replace_substitutes_original_placeholder() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "replace", with = "lib/${original}" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"lib/foo.h\""),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn error_action_produces_error_outcome() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "error", message = "no `${original}`" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Error { message } => assert_eq!(message, "no `foo.h`"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn resolve_without_matcher_path_emits_failure() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            include_directories = ["nonexistent"]
            action = { type = "resolve", relative_to = "${current_file}" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj-does-not-exist"),
        );
        match out {
            Outcome::EvaluationFailure { message } => {
                assert!(
                    message.contains("requires a resolved include path"),
                    "unexpected wording: {message}"
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn macro_form_always_errors() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            include_forms = ["macro"]
            action = { type = "keep" }
            "#,
        );
        let inc = Include {
            form: IncludeForm::Macro,
            content: "MY_HEADER".to_string(),
            line: 1,
            argument_range: 0..0,
            trailing_range: 0..0,
            trailing_comment_style: None,
            has_cross_line_block_trailing: false,
        };
        let out = evaluate(
            &rules[0],
            &inc,
            "",
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Error { message } => assert!(message.contains("macro")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn comment_out_line_style_wraps_with_slashes() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "comment_out" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "// #include \"foo.h\"\n"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn comment_out_block_style_wraps_with_slash_star() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "comment_out", style = "/**/" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "/* #include \"foo.h\" */\n"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn remove_default_drops_line_entirely() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "remove" }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite {
                edit_range,
                new_text,
            } => {
                // Default: keep_trailing_comment = true but there's no comment.
                assert_eq!(new_text, "");
                // The edit_range should cover the entire line including the newline.
                assert_eq!(&src[edit_range.clone()], "#include \"foo.h\"\n");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn remove_keep_blank_line_emits_terminator_only() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "remove", keep_blank_line = true, keep_trailing_comment = false }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\n"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_replace_overrides_existing() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                transform = {
                    action = { type = "replace", with = "REPLACED" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" // old\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert!(new_text.contains("REPLACED"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_replace_only_touches_first_block_comment() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                transform = {
                    content_regex = "^1st$",
                    action = { type = "replace", with = "NEW" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" /*1st*/ /*2nd*/\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert_eq!(new_text, "\"foo.h\"  /* NEW */ /*2nd*/");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_replace_preserves_line_comment_suffix_after_block() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                transform = {
                    content_regex = "^1st$",
                    action = { type = "replace", with = "NEW" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" /*1st*/ // 2nd\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert_eq!(new_text, "\"foo.h\"  /* NEW */ // 2nd");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_remove_drops_comment() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                transform = {
                    action = { type = "remove" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" // unwanted\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert_eq!(new_text, "\"foo.h\"");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_remove_first_block_preserves_suffix_without_append() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                append_if_absent = " // generated",
                transform = {
                    content_regex = "^drop$",
                    action = { type = "remove" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" /*drop*/ /*keep*/\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert_eq!(new_text, "\"foo.h\" /*keep*/");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_append_if_absent_adds_for_uncommented() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                append_if_absent = "  // IWYU pragma: export",
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\"\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert_eq!(new_text, "\"foo.h\"  // IWYU pragma: export");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn cross_line_block_trailing_skips_append_if_absent() {
        // Per refactor.md §"Trailing comment 的定义": cross-line block
        // comments are NOT trailing comments. append_if_absent must NOT
        // fire even though `original_trailing` is empty.
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                append_if_absent = " // IWYU pragma: export",
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" /* opens\nnever closes */\n");
        assert!(inc.has_cross_line_block_trailing);
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        // Nothing must change on the include line.
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn later_cross_line_block_skips_transform_and_append() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                append_if_absent = " // generated",
                transform = {
                    content_regex = "^1st$",
                    action = { type = "replace", with = "NEW" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" /*1st*/ /* open\n*/\n");
        assert!(inc.has_cross_line_block_trailing);
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn trailing_comment_append_if_absent_skips_when_comment_present() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                append_if_absent = "  // extra",
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" // existing\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        // existing trailing comment is preserved; no append.
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn trailing_comment_error_produces_error_outcome() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "keep" }
            trailing_comment = {
                transform = {
                    content_regex = "^TODO.*$",
                    action = { type = "error", message = "no TODO comments" },
                },
            }
            "#,
        );
        let (src, inc) = first_include("#include \"foo.h\" // TODO: rename\n");
        let out = evaluate(
            &rules[0],
            &inc,
            &src,
            Path::new("src/main.c"),
            Path::new("/proj"),
        );
        match out {
            Outcome::TrailingCommentError { message } => {
                assert!(message.contains("no TODO comments"))
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn relative_path_walks_up_for_diverging_paths() {
        assert_eq!(
            relative_path(Path::new("include/foo.h"), Path::new("src/sub")),
            PathBuf::from("../../include/foo.h"),
        );
    }

    #[test]
    fn relative_path_same_dir_is_just_filename() {
        assert_eq!(
            relative_path(Path::new("src/foo.h"), Path::new("src")),
            PathBuf::from("foo.h"),
        );
    }
}
