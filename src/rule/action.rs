//! Evaluate a matched rule's action against an `#include`.
//!
//! Inputs: the [`Match`] (rule + regex captures), the [`Include`] being
//! rewritten, where in the project tree the file lives, and the project
//! root. Output: an [`Outcome`] describing whether the include should be
//! rewritten, kept as-is, or whether to abort the file with an error.
//!
//! Placeholder grammar in `rewrite.to`, `error.message`, and
//! `trailing_comment.to` is `${name}` or `${N}` (regex capture).
//! `${comment.N}` / `${comment.text}` reference the `trailing_comment.match`
//! captures and are only valid inside `trailing_comment.to`. Use `$$` for a
//! literal `$` (rarely needed).

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;

use super::engine::Match;
use crate::config::inherit::{ResolvedAction, ResolvedTrailingComment};
use crate::config::schema::{AutoRelativeTo, IncludeForm, OutputForm, TrailingForm};
use crate::index::header_index;
use crate::lex::include_line::Include;

/// What the engine should do with the matched include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Replace bytes in `edit_range` with `new_text`. `edit_range` usually
    /// covers just the include argument (delimiters + path), but when the
    /// rule also rewrites the trailing comment it widens to include the
    /// post-argument whitespace and comment up to (but not including) the
    /// line-terminating newline.
    Rewrite {
        edit_range: Range<usize>,
        new_text: String,
    },
    /// Leave the include unchanged.
    Keep,
    /// Abort processing of the current file. The caller surfaces `message`
    /// and increments the error count.
    Error { message: String },
}

/// Evaluate the action that the matched rule attaches to this include.
///
/// `file_relpath` is the source file's path relative to `project_root`.
/// `source` is the full text of that file — needed when the rule injects
/// a trailing comment, because that requires inspecting whatever is on
/// the line after the argument.
pub fn evaluate(
    matched: &Match<'_>,
    include: &Include,
    source: &str,
    file_relpath: &Path,
    project_root: &Path,
) -> Result<Outcome> {
    // Macro-form includes that hit any rule trip the v1 "not supported"
    // hatch. Matching a macro is allowed in config (`forms = ["macro"]`);
    // executing the action against one is not.
    if include.form == IncludeForm::Macro {
        bail!(
            "rule `{}` matched a macro-form include (`#include {}`); macro-form evaluation is not supported in v1",
            matched.rule.rule.name,
            include.content,
        );
    }

    let ctx = TemplateCtx {
        captures: &matched.captures,
        include,
        file_relpath,
        resolved: matched.resolved.as_deref(),
        comment_captures: None,
    };

    let rule = matched.rule.rule;
    let trailing = rule.trailing_comment.as_ref();
    let trailing_regex = matched.rule.trailing_comment_regex.as_ref();

    let new_arg: Option<String> = match &rule.action {
        // `error` aborts the file — trailing-comment settings are
        // intentionally ignored.
        ResolvedAction::Error { message } => {
            return Ok(Outcome::Error {
                message: substitute(message, &ctx)?,
            });
        }
        ResolvedAction::Keep => None,
        ResolvedAction::Rewrite { to, form } => {
            let new_content = substitute(to, &ctx)?;
            let form_out = pick_form(*form, include.form);
            Some(format_argument(&new_content, form_out))
        }
        ResolvedAction::Auto { relative_to, form } => Some(evaluate_auto_arg(
            matched,
            include,
            file_relpath,
            project_root,
            *relative_to,
            *form,
        )?),
    };

    finalize_outcome(include, source, new_arg, trailing, trailing_regex, &ctx)
}

fn evaluate_auto_arg(
    matched: &Match<'_>,
    include: &Include,
    file_relpath: &Path,
    project_root: &Path,
    relative_to: AutoRelativeTo,
    form: OutputForm,
) -> Result<String> {
    let rule = &matched.rule.rule;

    let resolved = header_index::resolve_in_dirs(
        project_root,
        &rule.original_include_dirs,
        &include.content,
    )
    .with_context(|| {
        format!(
            "rule `{}` action `auto`: could not resolve include `{}` under original_include_dirs ({:?})",
            rule.name, include.content, rule.original_include_dirs,
        )
    })?;

    let output_path = match relative_to {
        AutoRelativeTo::Allowed => {
            let mut found = None;
            for allowed in &rule.allowed_include_dirs {
                let allowed_abs = project_root.join(allowed);
                if let Ok(rel) = resolved.strip_prefix(&allowed_abs) {
                    found = Some(rel.to_path_buf());
                    break;
                }
            }
            found.with_context(|| {
                format!(
                    "rule `{}` action `auto`: resolved file `{}` is not under any allowed_include_dir ({:?})",
                    rule.name,
                    resolved.display(),
                    rule.allowed_include_dirs,
                )
            })?
        }
        AutoRelativeTo::FileDir => {
            let file_dir_abs = match file_relpath.parent() {
                Some(p) => project_root.join(p),
                None => project_root.to_path_buf(),
            };
            diff_paths(&resolved, &file_dir_abs)
        }
    };

    let new_content = output_path.to_string_lossy().replace('\\', "/");
    let form_out = pick_form(form, include.form);
    Ok(format_argument(&new_content, form_out))
}

/// Combine the action's new argument (if any) with the rule's
/// trailing-comment substitution into a final `Outcome`. Idempotency falls
/// out of the byte-equality collapse at the end: when the result equals
/// what's already on disk, the outcome becomes `Keep`.
fn finalize_outcome(
    include: &Include,
    source: &str,
    new_arg: Option<String>,
    trailing: Option<&ResolvedTrailingComment>,
    trailing_regex: Option<&Regex>,
    ctx: &TemplateCtx<'_>,
) -> Result<Outcome> {
    let existing_arg = &source[include.argument_range.clone()];
    let existing_trailing = &source[include.trailing_range.clone()];

    let arg_text: String = new_arg.unwrap_or_else(|| existing_arg.to_string());

    // No trailing-comment rule: only touch the argument range. Anything
    // after the argument (whitespace, comments) is left to the file's own
    // bytes.
    let Some(config) = trailing else {
        if arg_text == existing_arg {
            return Ok(Outcome::Keep);
        }
        return Ok(Outcome::Rewrite {
            edit_range: include.argument_range.clone(),
            new_text: arg_text,
        });
    };

    // The resolver guarantees a compiled regex exists whenever the rule has
    // `trailing_comment`. The `unwrap_or_else` branch is defensive — if it
    // ever fires we treat the trailing as no-op rather than panic.
    let change = match trailing_regex {
        Some(re) => compute_trailing_change(config, re, existing_trailing, ctx)?,
        None => TrailingChange::Unchanged,
    };
    let new_trailing: String = match change {
        TrailingChange::Unchanged => existing_trailing.to_string(),
        TrailingChange::Set { trailing } => trailing,
    };

    let new_combined = format!("{arg_text}{new_trailing}");
    let existing_combined = &source[include.argument_range.start..include.trailing_range.end];
    if new_combined == existing_combined {
        return Ok(Outcome::Keep);
    }
    Ok(Outcome::Rewrite {
        edit_range: include.argument_range.start..include.trailing_range.end,
        new_text: new_combined,
    })
}

enum TrailingChange {
    /// Leave the existing trailing bytes alone (regex didn't match or the
    /// trailing slice has no comment we know how to rewrite).
    Unchanged,
    /// Replace the trailing slice with this string (already includes any
    /// leading whitespace; may be empty to strip the trailing comment).
    Set { trailing: String },
}

/// What the lexer's `trailing_range` looks like after we split it into
/// whitespace + delimited body.
struct ExistingTrailing<'a> {
    /// Spaces / tabs between the argument and the delimiter (or all the
    /// way to EOL if there's no comment).
    leading_ws: &'a str,
    /// `Some(_)` only when a `//` or `/* */` comment is present.
    style: Option<ExistingStyle>,
    /// Stripped comment body. Empty string when there's no comment. For
    /// existing `// foo` / `/* foo */` we shave off one space of padding on
    /// each side so that regex authors can write `^foo$` and have it match
    /// the obvious case.
    body: &'a str,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ExistingStyle {
    Line,
    Block,
}

fn parse_existing_trailing(raw: &str) -> ExistingTrailing<'_> {
    let ws_end = raw
        .bytes()
        .position(|b| b != b' ' && b != b'\t')
        .unwrap_or(raw.len());
    let (leading_ws, rest) = raw.split_at(ws_end);

    if let Some(after) = rest.strip_prefix("//") {
        // Trim only one leading space of padding so that a user's `// foo`
        // surfaces as `"foo"` in regex / `${comment.0}`. Trailing whitespace
        // inside a line comment is rare and harmless to keep verbatim.
        let body = after.strip_prefix(' ').unwrap_or(after);
        return ExistingTrailing {
            leading_ws,
            style: Some(ExistingStyle::Line),
            body,
        };
    }
    if rest.len() >= 4 && rest.starts_with("/*") && rest.ends_with("*/") {
        let inner = &rest[2..rest.len() - 2];
        let inner = inner.strip_prefix(' ').unwrap_or(inner);
        let inner = inner.strip_suffix(' ').unwrap_or(inner);
        return ExistingTrailing {
            leading_ws,
            style: Some(ExistingStyle::Block),
            body: inner,
        };
    }

    // No comment, or malformed trailing (e.g. `/* never closed`). Defensive
    // fallback — the lexer's `trailing_range` should not produce malformed
    // cases since it stops at EOL.
    ExistingTrailing {
        leading_ws,
        style: None,
        body: "",
    }
}

fn compute_trailing_change(
    config: &ResolvedTrailingComment,
    comment_regex: &Regex,
    existing_trailing: &str,
    ctx: &TemplateCtx<'_>,
) -> Result<TrailingChange> {
    let parsed = parse_existing_trailing(existing_trailing);

    // Run the regex against the stripped body (empty when no comment).
    let caps = match comment_regex.captures(parsed.body) {
        Some(c) => c,
        None => return Ok(TrailingChange::Unchanged),
    };
    let comment_captures: Vec<String> = caps
        .iter()
        .map(|m| m.map(|s| s.as_str().to_string()).unwrap_or_default())
        .collect();

    // A short-lived ctx that exposes the comment captures only for this
    // substitution. Constructing a fresh struct (instead of mutating the
    // caller's ctx) keeps the borrow scoped to `comment_captures` here.
    let inner_ctx = TemplateCtx {
        captures: ctx.captures,
        include: ctx.include,
        file_relpath: ctx.file_relpath,
        resolved: ctx.resolved,
        comment_captures: Some(&comment_captures),
    };
    let new_body = substitute(&config.to, &inner_ctx)?;

    // Empty body means: strip the trailing comment entirely (whitespace +
    // delimiters + body). `spacing` / `form` are ignored in this branch —
    // otherwise we'd leave dangling spaces at end-of-line.
    if new_body.is_empty() {
        return Ok(TrailingChange::Set {
            trailing: String::new(),
        });
    }

    // Pick output style.
    let out_style = match config.form {
        TrailingForm::Line => ExistingStyle::Line,
        TrailingForm::Block => ExistingStyle::Block,
        TrailingForm::Preserve => parsed.style.unwrap_or(ExistingStyle::Line),
    };

    let wrapped = match out_style {
        ExistingStyle::Line => format!("// {new_body}"),
        ExistingStyle::Block => format!("/* {new_body} */"),
    };

    let leading_ws = match config.spacing {
        Some(n) => " ".repeat(n as usize),
        None => {
            // Preserve hand-aligned whitespace; default to two spaces when
            // there was nothing before.
            if parsed.leading_ws.is_empty() {
                "  ".to_string()
            } else {
                parsed.leading_ws.to_string()
            }
        }
    };

    Ok(TrailingChange::Set {
        trailing: format!("{leading_ws}{wrapped}"),
    })
}

fn pick_form(out: OutputForm, original: IncludeForm) -> IncludeForm {
    match out {
        OutputForm::Quote => IncludeForm::Quote,
        OutputForm::Angle => IncludeForm::Angle,
        OutputForm::Preserve => original,
    }
}

fn format_argument(content: &str, form: IncludeForm) -> String {
    match form {
        IncludeForm::Quote => format!("\"{content}\""),
        IncludeForm::Angle => format!("<{content}>"),
        // Macro-form output is exotic; emit content verbatim. The
        // pre-evaluation check above already rejected matched-macro
        // includes, so this only happens if a `rewrite` action produces a
        // bare identifier for a non-macro include — surprising but legal.
        IncludeForm::Macro => content.to_string(),
    }
}

/// Compute `target` relative to `base`. Best-effort using `..` segments.
fn diff_paths(target: &Path, base: &Path) -> PathBuf {
    let t: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = t.iter().zip(b.iter()).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

// ---- Template substitution -------------------------------------------------

struct TemplateCtx<'a> {
    captures: &'a [String],
    include: &'a Include,
    file_relpath: &'a Path,
    /// Project-root-relative path produced by layer 5. `None` when the
    /// matching rule had no `match_resolved` block.
    resolved: Option<&'a Path>,
    /// Captures of `trailing_comment.match`. Only `Some(_)` while
    /// substituting `trailing_comment.to`; elsewhere `${comment.*}` errors.
    comment_captures: Option<&'a [String]>,
}

fn substitute(template: &str, ctx: &TemplateCtx<'_>) -> Result<String> {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // `$$` → literal `$`
        if b == b'$' && bytes.get(i + 1) == Some(&b'$') {
            out.push('$');
            i += 2;
            continue;
        }
        // `${...}` → placeholder
        if b == b'$' && bytes.get(i + 1) == Some(&b'{') {
            let end = bytes[i + 2..]
                .iter()
                .position(|&c| c == b'}')
                .map(|p| i + 2 + p)
                .with_context(|| format!("unterminated `${{...}}` in template: {template:?}"))?;
            let name = &template[i + 2..end];
            out.push_str(&resolve_placeholder(name, ctx)?);
            i = end + 1;
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    Ok(out)
}

fn resolve_placeholder(name: &str, ctx: &TemplateCtx<'_>) -> Result<String> {
    // Numeric capture group? Layer-4 captures, available everywhere a
    // template runs (including `trailing_comment.to`).
    if let Ok(n) = name.parse::<usize>() {
        return ctx
            .captures
            .get(n)
            .cloned()
            .with_context(|| format!("placeholder `${{{n}}}` exceeds available captures"));
    }

    // `${comment.*}` — only valid inside `trailing_comment.to` (the only
    // call site that sets `ctx.comment_captures`).
    if let Some(rest) = name.strip_prefix("comment.") {
        let caps = ctx.comment_captures.with_context(|| {
            format!("placeholder `${{{name}}}` is only valid inside `trailing_comment.to`")
        })?;
        if rest == "text" {
            return Ok(caps.first().cloned().unwrap_or_default());
        }
        let idx: usize = rest.parse().with_context(|| {
            format!("placeholder `${{comment.{rest}}}` is not a valid capture index")
        })?;
        return caps
            .get(idx)
            .cloned()
            .with_context(|| format!("placeholder `${{comment.{idx}}}` exceeds available captures"));
    }

    Ok(match name {
        "include.text" => ctx.include.content.clone(),
        "include.dirname" => Path::new(&ctx.include.content)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "include.basename" => Path::new(&ctx.include.content)
            .file_name()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "file.path" => ctx.file_relpath.to_string_lossy().into_owned(),
        "file.dir" => ctx
            .file_relpath
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        "file.relpath" => ctx.file_relpath.to_string_lossy().into_owned(),
        "resolved.path" | "resolved.relpath" => {
            let p = ctx.resolved.with_context(|| {
                format!("placeholder `${{{name}}}` requires layer 5 (`match_resolved`)")
            })?;
            p.to_string_lossy().replace('\\', "/")
        }
        "resolved.dir" => {
            let p = ctx.resolved.with_context(|| {
                format!("placeholder `${{{name}}}` requires layer 5 (`match_resolved`)")
            })?;
            p.parent()
                .map(|q| q.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default()
        }
        "resolved.basename" => {
            let p = ctx.resolved.with_context(|| {
                format!("placeholder `${{{name}}}` requires layer 5 (`match_resolved`)")
            })?;
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        }
        other => bail!("unknown placeholder `${{{other}}}`"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::inherit::{resolve, ResolvedRule};
    use crate::config::schema::{parse, LoadedConfig};
    use crate::lex::include_line::Include;
    use crate::rule::engine::CompiledRule;
    use std::fs;

    fn cfg(path: &str, body: &str) -> LoadedConfig {
        LoadedConfig {
            path: PathBuf::from(path),
            raw: parse(body, &PathBuf::from(path)).unwrap(),
        }
    }

    fn build_one_rule(body: &str, root: &Path) -> (CompiledRule<'static>, ResolvedRule) {
        let configs = vec![cfg("/proj/inclean.toml", body)];
        let resolved = resolve(&configs).unwrap();
        let (_, rule) = resolved.into_iter().next().unwrap();
        // Leak to obtain a 'static borrow, fine for tests.
        let rule: &'static ResolvedRule = Box::leak(Box::new(rule.clone()));
        let compiled = CompiledRule::new(rule, root).unwrap();
        (compiled, rule.clone())
    }

    fn inc(content: &str, form: IncludeForm) -> Include {
        let arg_len = match form {
            IncludeForm::Quote | IncludeForm::Angle => content.len() + 2,
            IncludeForm::Macro => content.len(),
        };
        Include {
            form,
            content: content.to_string(),
            line: 1,
            argument_range: 0..arg_len,
            trailing_range: arg_len..arg_len,
        }
    }

    /// Synthesize a source string that matches the byte ranges in `inc`.
    /// Tests use it to satisfy `evaluate`'s `source: &str` argument
    /// without needing a real file on disk.
    fn src_of(inc: &Include) -> String {
        match inc.form {
            IncludeForm::Quote => format!("\"{}\"", inc.content),
            IncludeForm::Angle => format!("<{}>", inc.content),
            IncludeForm::Macro => inc.content.clone(),
        }
    }

    fn matched<'a>(c: &'a CompiledRule<'a>, captures: Vec<String>) -> Match<'a> {
        Match {
            rule: c,
            captures,
            resolved: None,
        }
    }

    fn matched_resolved<'a>(
        c: &'a CompiledRule<'a>,
        captures: Vec<String>,
        resolved: PathBuf,
    ) -> Match<'a> {
        Match {
            rule: c,
            captures,
            resolved: Some(resolved),
        }
    }

    fn tmp_root() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "inclean-action-{}-{}",
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

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    #[test]
    fn keep_action_returns_keep() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let inc = inc("foo.h", IncludeForm::Quote);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn error_action_substitutes_placeholders() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "error", message = "deprecated: ${include.text}" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let inc = inc("foo.h", IncludeForm::Quote);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Error { message } => assert_eq!(message, "deprecated: foo.h"),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn rewrite_action_uses_captures_and_preserves_form() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            match = '^old_(.+)$'
            action = { type = "rewrite", to = "new_${1}" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["old_foo.h".into(), "foo.h".into()]);
        let inc = inc("old_foo.h", IncludeForm::Angle);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "<new_foo.h>"),
            _ => panic!("expected Rewrite"),
        }
    }

    #[test]
    fn rewrite_action_can_change_form_to_quote() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            forms = ["angle"]
            action = { type = "rewrite", to = "x.h", form = "quote" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["x.h".into()]);
        let inc = inc("x.h", IncludeForm::Angle);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"x.h\""),
            _ => panic!("expected Rewrite"),
        }
    }

    #[test]
    fn rewrite_action_supports_file_placeholders() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "rewrite", to = "${file.dir}/x.h" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["x.h".into()]);
        let inc = inc("x.h", IncludeForm::Quote);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/foo/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"src/foo/x.h\""),
            _ => panic!("expected Rewrite"),
        }
    }

    #[test]
    fn unknown_placeholder_errors() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "rewrite", to = "${nope}" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["x.h".into()]);
        let inc = inc("x.h", IncludeForm::Quote);
        let err = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap_err();
        assert!(format!("{err:#}").contains("unknown placeholder"));
    }

    #[test]
    fn macro_include_at_evaluation_time_errors() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            forms = ["macro"]
            action = { type = "keep" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["MY_HEADER".into()]);
        let inc = inc("MY_HEADER", IncludeForm::Macro);
        let err = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap_err();
        assert!(format!("{err:#}").contains("macro-form"));
    }

    #[test]
    fn auto_action_resolves_and_relativizes_to_allowed() {
        let root = tmp_root();
        touch(&root, "src/internal/foo.h");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            original_include_dirs = ["src", "src/internal"]
            forms = ["quote"]
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let inc = inc("foo.h", IncludeForm::Quote);
        let err = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap_err();
        // The resolved file lives under src/internal, not under include/.
        assert!(format!("{err:#}").contains("not under any allowed_include_dir"));

        // Now move the header into include/internal/ and re-resolve.
        touch(&root, "include/internal/foo.h");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            original_include_dirs = ["include/internal", "src"]
            forms = ["quote"]
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"internal/foo.h\""),
            _ => panic!("expected Rewrite"),
        }

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn auto_action_relative_to_file_dir() {
        let root = tmp_root();
        touch(&root, "include/foo.h");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            original_include_dirs = ["include"]
            forms = ["quote"]
            action = { type = "auto", relative_to = "file_dir", form = "quote" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let inc = inc("foo.h", IncludeForm::Quote);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/foo/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"../../include/foo.h\""),
            _ => panic!("expected Rewrite"),
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn auto_action_errors_when_include_not_resolvable() {
        let root = tmp_root();
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            original_include_dirs = ["src"]
            forms = ["quote"]
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["missing.h".into()]);
        let inc = inc("missing.h", IncludeForm::Quote);
        let err = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap_err();
        assert!(format!("{err:#}").contains("could not resolve include"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolved_placeholders_require_layer5() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "rewrite", to = "${resolved.relpath}" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["x.h".into()]);
        let inc = inc("x.h", IncludeForm::Quote);
        let err = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap_err();
        assert!(format!("{err:#}").contains("requires layer 5"));
    }

    #[test]
    fn resolved_placeholders_expand_when_layer5_ran() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "rewrite", to = "${resolved.dir}/X.h" }
            "#,
            &root,
        );
        let m = matched_resolved(
            &compiled,
            vec!["foo.h".into()],
            PathBuf::from("src/internal/foo.h"),
        );
        let inc = inc("foo.h", IncludeForm::Quote);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => {
                assert_eq!(new_text, "\"src/internal/X.h\"");
            }
            _ => panic!("expected Rewrite"),
        }
    }

    #[test]
    fn dollar_dollar_escapes_to_literal_dollar() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "rewrite", to = "price$$1" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["x.h".into()]);
        let inc = inc("x.h", IncludeForm::Quote);
        let out = evaluate(&m, &inc, &src_of(&inc), Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"price$1\""),
            _ => panic!("expected Rewrite"),
        }
    }

    // ---- Trailing-comment tests -------------------------------------------

    /// Build an include + matching source that includes an arbitrary
    /// trailing string after the argument. The argument is rendered in
    /// the requested form so byte ranges line up.
    fn inc_with_trailing(content: &str, form: IncludeForm, trailing: &str) -> (Include, String) {
        let arg = match form {
            IncludeForm::Quote => format!("\"{content}\""),
            IncludeForm::Angle => format!("<{content}>"),
            IncludeForm::Macro => content.to_string(),
        };
        let arg_len = arg.len();
        let source = format!("{arg}{trailing}");
        let trailing_len = trailing.len();
        let include = Include {
            form,
            content: content.to_string(),
            line: 1,
            argument_range: 0..arg_len,
            trailing_range: arg_len..(arg_len + trailing_len),
        };
        (include, source)
    }

    fn run(toml: &str, captures: Vec<String>, trailing: &str) -> Outcome {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(toml, &root);
        let m = matched(&compiled, captures);
        let (inc, src) = inc_with_trailing("foo.h", IncludeForm::Quote, trailing);
        evaluate(&m, &inc, &src, Path::new("src/main.c"), &root).unwrap()
    }

    fn assert_rewrite(out: Outcome, expected: &str) {
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, expected),
            _ => panic!("expected Rewrite, got {out:?}"),
        }
    }

    #[test]
    fn trailing_to_empty_strips_existing() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "" }
            "#,
            vec!["foo.h".into()],
            "  // some note",
        );
        assert_rewrite(out, "\"foo.h\"");
    }

    #[test]
    fn trailing_to_empty_noop_when_absent() {
        // No comment and `to = ""` means "result is empty trailing"; bytes
        // already match → Keep.
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "" }
            "#,
            vec!["foo.h".into()],
            "",
        );
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn trailing_default_match_overwrites_existing() {
        // Default match is `.*` → captures any existing comment and the
        // template emits a fresh `// note` body. `form = preserve` keeps
        // the existing `//` style.
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "note" }
            "#,
            vec!["foo.h".into()],
            "  // old",
        );
        // Existing whitespace preserved, style preserved as line.
        assert_rewrite(out, "\"foo.h\"  // note");
    }

    #[test]
    fn trailing_match_empty_only_injects_when_absent() {
        let toml = r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { match = "^$", to = "note" }
            "#;
        // No existing comment → regex matches empty body → inject.
        assert_rewrite(
            run(toml, vec!["foo.h".into()], ""),
            "\"foo.h\"  // note",
        );
        // Existing comment → regex doesn't match → trailing left alone.
        let out = run(toml, vec!["foo.h".into()], "  // user");
        assert_eq!(out, Outcome::Keep);
    }

    #[test]
    fn trailing_comment_captures_into_template() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { match = '^todo: (.*)$', to = "fixme: ${comment.1}" }
            "#,
            vec!["foo.h".into()],
            "  // todo: revisit",
        );
        assert_rewrite(out, "\"foo.h\"  // fixme: revisit");
    }

    #[test]
    fn trailing_comment_text_alias_matches_full_body() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X ${comment.text}" }
            "#,
            vec!["foo.h".into()],
            "  // body",
        );
        assert_rewrite(out, "\"foo.h\"  // X body");
    }

    #[test]
    fn trailing_form_line_overrides_block_style() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X", form = "line" }
            "#,
            vec!["foo.h".into()],
            " /* old */",
        );
        assert_rewrite(out, "\"foo.h\" // X");
    }

    #[test]
    fn trailing_form_block_overrides_line_style() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X", form = "block" }
            "#,
            vec!["foo.h".into()],
            "  // old",
        );
        assert_rewrite(out, "\"foo.h\"  /* X */");
    }

    #[test]
    fn trailing_form_preserve_keeps_block_style() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X" }
            "#,
            vec!["foo.h".into()],
            " /* old */",
        );
        assert_rewrite(out, "\"foo.h\" /* X */");
    }

    #[test]
    fn trailing_form_preserve_defaults_to_line_when_no_existing_comment() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X" }
            "#,
            vec!["foo.h".into()],
            "",
        );
        assert_rewrite(out, "\"foo.h\"  // X");
    }

    #[test]
    fn trailing_spacing_zero_butts_delimiter_against_argument() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X", spacing = 0 }
            "#,
            vec!["foo.h".into()],
            "",
        );
        assert_rewrite(out, "\"foo.h\"// X");
    }

    #[test]
    fn trailing_spacing_n_overrides_existing_whitespace() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X", spacing = 4 }
            "#,
            vec!["foo.h".into()],
            " // old",
        );
        assert_rewrite(out, "\"foo.h\"    // X");
    }

    #[test]
    fn trailing_spacing_default_preserves_existing_whitespace() {
        // Single-space alignment should round-trip — the default policy is
        // "keep whatever whitespace was already there."
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X" }
            "#,
            vec!["foo.h".into()],
            " // old",
        );
        assert_rewrite(out, "\"foo.h\" // X");
    }

    #[test]
    fn trailing_spacing_default_falls_back_to_two_spaces() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X" }
            "#,
            vec!["foo.h".into()],
            "",
        );
        assert_rewrite(out, "\"foo.h\"  // X");
    }

    #[test]
    fn trailing_existing_block_inner_padding_is_normalized() {
        // Inner padding of one space on each side is shaved when we read
        // the existing body. The template here just echoes that body.
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "[${comment.0}]" }
            "#,
            vec!["foo.h".into()],
            "  /* X */",
        );
        assert_rewrite(out, "\"foo.h\"  /* [X] */");
    }

    #[test]
    fn trailing_idempotent_plain_replace() {
        // Default match `.*` + literal `to`. First run rewrites; second run
        // produces identical bytes → Keep.
        let out1 = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X" }
            "#,
            vec!["foo.h".into()],
            "  // old",
        );
        assert_rewrite(out1, "\"foo.h\"  // X");
        // Second run: trailing is now "  // X".
        let out2 = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "X" }
            "#,
            vec!["foo.h".into()],
            "  // X",
        );
        assert_eq!(out2, Outcome::Keep);
    }

    #[test]
    fn trailing_idempotent_optional_prefix_prepend() {
        // The documented prepend idiom for the `regex` crate (no lookaround):
        // an optional non-capturing group eats the prefix when it's already
        // there, so the captured tail is the same on both runs.
        let toml = r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { match = '^(?:X )?(.*)$', to = "X ${comment.1}" }
            "#;
        let out1 = run(toml, vec!["foo.h".into()], "  // foo");
        assert_rewrite(out1, "\"foo.h\"  // X foo");
        // Second run: body is "X foo"; the `(?:X )?` eats the prefix, capture
        // group is still "foo", template re-emits "X foo" → bytes unchanged.
        let out2 = run(toml, vec!["foo.h".into()], "  // X foo");
        assert_eq!(out2, Outcome::Keep);
    }

    #[test]
    fn trailing_to_supports_layer4_captures() {
        // `${1}` inside `trailing_comment.to` still refers to layer-4
        // include captures, not comment captures.
        let out = run(
            r#"
            [[rule]]
            name = "r"
            match = '^(.+)\.h$'
            action = { type = "keep" }
            trailing_comment = { to = "for ${1}" }
            "#,
            vec!["foo.h".into(), "foo".into()],
            "",
        );
        assert_rewrite(out, "\"foo.h\"  // for foo");
    }

    #[test]
    fn trailing_to_supports_resolved_basename() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            match_resolved = { under = "src" }
            action = { type = "keep" }
            trailing_comment = { to = "for ${resolved.basename}" }
            "#,
            &root,
        );
        let m = matched_resolved(&compiled, vec!["foo.h".into()], PathBuf::from("src/foo.h"));
        let (inc, src) = inc_with_trailing("foo.h", IncludeForm::Quote, "");
        let out = evaluate(&m, &inc, &src, Path::new("src/main.c"), &root).unwrap();
        assert_rewrite(out, "\"foo.h\"  // for foo.h");
    }

    #[test]
    fn trailing_to_supports_dollar_dollar_escape() {
        let out = run(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { to = "price$$1" }
            "#,
            vec!["foo.h".into()],
            "",
        );
        assert_rewrite(out, "\"foo.h\"  // price$1");
    }

    #[test]
    fn comment_placeholder_outside_trailing_to_errors() {
        // Using `${comment.0}` in `rewrite.to` is a config-context error:
        // there are no comment captures in scope outside trailing rendering.
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "rewrite", to = "${comment.0}" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let (inc, src) = inc_with_trailing("foo.h", IncludeForm::Quote, "");
        let err = evaluate(&m, &inc, &src, Path::new("src/main.c"), &root).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("only valid inside `trailing_comment.to`"),
            "got: {msg}"
        );
    }

    #[test]
    fn comment_placeholder_unknown_index_errors() {
        let root = PathBuf::from("/proj");
        let (compiled, _) = build_one_rule(
            r#"
            [[rule]]
            name = "r"
            action = { type = "keep" }
            trailing_comment = { match = '^(.*)$', to = "${comment.5}" }
            "#,
            &root,
        );
        let m = matched(&compiled, vec!["foo.h".into()]);
        let (inc, src) = inc_with_trailing("foo.h", IncludeForm::Quote, "  // body");
        let err = evaluate(&m, &inc, &src, Path::new("src/main.c"), &root).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("exceeds available captures"),
            "got: {msg}"
        );
    }
}
