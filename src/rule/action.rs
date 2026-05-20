//! Evaluate a matched rule's action against an `#include`.
//!
//! Inputs: the [`Match`] (rule + regex captures), the [`Include`] being
//! rewritten, where in the project tree the file lives, and the project
//! root. Output: an [`Outcome`] describing whether the include should be
//! rewritten, kept as-is, or whether to abort the file with an error.
//!
//! Placeholder grammar in `rewrite.to` and `error.message` is `${name}`
//! or `${N}` (regex capture). Use `$$` for a literal `$` (rarely needed).

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::engine::Match;
use crate::config::inherit::ResolvedAction;
use crate::config::schema::{AutoRelativeTo, IncludeForm, OutputForm};
use crate::index::header_index;
use crate::lex::include_line::Include;

/// What the engine should do with the matched include.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Replace bytes in `argument_range` with `new_text`.
    Rewrite {
        argument_range: Range<usize>,
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
pub fn evaluate(
    matched: &Match<'_>,
    include: &Include,
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
    };

    match &matched.rule.rule.action {
        ResolvedAction::Keep => Ok(Outcome::Keep),
        ResolvedAction::Error { message } => Ok(Outcome::Error {
            message: substitute(message, &ctx)?,
        }),
        ResolvedAction::Rewrite { to, form } => {
            let new_content = substitute(to, &ctx)?;
            let form_out = pick_form(*form, include.form);
            Ok(Outcome::Rewrite {
                argument_range: include.argument_range.clone(),
                new_text: format_argument(&new_content, form_out),
            })
        }
        ResolvedAction::Auto { relative_to, form } => evaluate_auto(
            matched,
            include,
            file_relpath,
            project_root,
            *relative_to,
            *form,
        ),
    }
}

fn evaluate_auto(
    matched: &Match<'_>,
    include: &Include,
    file_relpath: &Path,
    project_root: &Path,
    relative_to: AutoRelativeTo,
    form: OutputForm,
) -> Result<Outcome> {
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
    Ok(Outcome::Rewrite {
        argument_range: include.argument_range.clone(),
        new_text: format_argument(&new_content, form_out),
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
    // Numeric capture group?
    if let Ok(n) = name.parse::<usize>() {
        return ctx
            .captures
            .get(n)
            .cloned()
            .with_context(|| format!("placeholder `${{{n}}}` exceeds available captures"));
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
        Include {
            form,
            content: content.to_string(),
            line: 1,
            // Tests don't replay rewriting at byte level here; arbitrary range.
            argument_range: 0..content.len() + 2,
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
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
        let out = evaluate(&m, &inc, Path::new("src/foo/main.c"), &root).unwrap();
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
        let err = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap_err();
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
        let err = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap_err();
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
        let err = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap_err();
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
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
        let out = evaluate(&m, &inc, Path::new("src/foo/main.c"), &root).unwrap();
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
        let err = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap_err();
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
        let err = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap_err();
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
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
        let out = evaluate(&m, &inc, Path::new("src/main.c"), &root).unwrap();
        match out {
            Outcome::Rewrite { new_text, .. } => assert_eq!(new_text, "\"price$1\""),
            _ => panic!("expected Rewrite"),
        }
    }
}
