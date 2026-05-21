//! Resolve `extends` chains, merge inherited fields, apply defaults, and
//! expand `@std.*` constants. Produces the fully-baked [`ResolvedRule`]s
//! the matching engine works with.
//!
//! Order of operations for a single rule:
//!
//! 1. Reject `match_resolved` (v1 unsupported).
//! 2. Recursively resolve the parent named in `extends` (cycle-detected).
//! 3. For each field, take the child's value if specified, otherwise the
//!    parent's resolved value, otherwise the language default.
//! 4. Expand `@std.*` constants in lists and substitute them in regex /
//!    template strings.

#[cfg(test)]
use std::collections::HashSet;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::constants;
use super::schema::{
    index_rules_by_name, AutoRelativeTo, IncludeForm, LoadedConfig, OutputForm, RawAction,
    RawMatchResolved, RawRule, RawTrailingComment, RuleLocator, TrailingForm,
};

/// Where a rule was declared. Carried through to the matching engine so
/// later error messages and `explain` output can pinpoint the source.
#[derive(Debug, Clone)]
pub struct Origin {
    pub config_path: PathBuf,
    pub config_dir: PathBuf,
    pub index: usize,
}

/// A fully merged, default-applied, constant-expanded view of a rule.
#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub name: String,
    pub extends: Option<String>,
    pub origin: Origin,

    // Layer 1..4 matching fields.
    pub paths: Vec<String>,
    pub extensions: Vec<String>,
    pub forms: Vec<IncludeForm>,
    pub match_regex: String,

    // Layer 5: resolved-file matching. None = layer 5 inactive (default).
    pub match_resolved: Option<ResolvedMatchResolved>,

    // Non-matching fields.
    pub allowed_include_dirs: Vec<String>,
    pub original_include_dirs: Vec<String>,
    pub action: ResolvedAction,
    pub trailing_comment: Option<ResolvedTrailingComment>,
}

/// Trailing-comment configuration after defaults and `@std.*` substitution.
///
/// `match_regex` is matched against the *stripped* existing trailing comment
/// (the text inside `//` or `/* */`, with one space of padding shaved off);
/// `".*"` by default. `to` is the new comment's stripped body (no
/// delimiters); empty `to` means "strip the trailing comment entirely".
/// `spacing` of `None` preserves whatever whitespace was already there,
/// falling back to two spaces when no whitespace was present.
#[derive(Debug, Clone)]
pub struct ResolvedTrailingComment {
    pub match_regex: String,
    pub to: String,
    pub form: TrailingForm,
    pub spacing: Option<u32>,
}

/// Layer-5 constraints after constant expansion.
#[derive(Debug, Clone)]
pub struct ResolvedMatchResolved {
    pub under: Option<String>,
    pub path_regex: Option<String>,
}

/// Action with all sub-fields defaulted.
#[derive(Debug, Clone)]
pub enum ResolvedAction {
    Auto {
        relative_to: AutoRelativeTo,
        form: OutputForm,
    },
    Rewrite {
        to: String,
        form: OutputForm,
    },
    Keep,
    Error {
        message: String,
    },
}

// ---- Defaults applied when neither child nor any ancestor specifies a value
// ---------------------------------------------------------------------------

fn default_paths() -> Vec<String> {
    vec!["**".to_string()]
}
/// Layer 2 default. The `@std.*` references will be expanded by
/// `constants::expand_list` in the next step of `merge`.
fn default_extensions() -> Vec<String> {
    vec![
        "@std.c_extensions".to_string(),
        "@std.cpp_extensions".to_string(),
    ]
}
fn default_forms() -> Vec<IncludeForm> {
    vec![IncludeForm::Quote]
}
fn default_match_regex() -> String {
    ".*".to_string()
}
fn default_action() -> ResolvedAction {
    ResolvedAction::Auto {
        relative_to: AutoRelativeTo::Allowed,
        form: OutputForm::Quote,
    }
}

/// Resolve every rule across `configs` into a [`ResolvedRule`]. Output is
/// keyed by rule name. The caller is responsible for ordering rules for
/// trial (which depends on the file being matched, not on the rule set).
pub fn resolve(configs: &[LoadedConfig]) -> Result<BTreeMap<String, ResolvedRule>> {
    let by_name = index_rules_by_name(configs)?;

    let mut resolved: HashMap<String, ResolvedRule> = HashMap::new();
    for name in by_name.keys() {
        if !resolved.contains_key(name) {
            let mut stack: Vec<String> = Vec::new();
            resolve_one(name, &by_name, &mut resolved, &mut stack)?;
        }
    }

    // Move into BTreeMap so the consumer sees a deterministic ordering.
    Ok(resolved.into_iter().collect())
}

fn resolve_one(
    name: &str,
    by_name: &BTreeMap<String, RuleLocator<'_>>,
    resolved: &mut HashMap<String, ResolvedRule>,
    stack: &mut Vec<String>,
) -> Result<()> {
    if resolved.contains_key(name) {
        return Ok(());
    }
    if stack.iter().any(|s| s == name) {
        let cycle = stack
            .iter()
            .cloned()
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>()
            .join(" -> ");
        anyhow::bail!("`extends` cycle: {cycle}");
    }

    let locator = by_name
        .get(name)
        .with_context(|| format!("rule `{name}` not found"))?;
    let raw = locator.rule;

    let parent_resolved: Option<ResolvedRule> = match raw.extends.as_deref() {
        Some(parent_name) => {
            // Verify parent exists before recursing for a clearer error.
            if !by_name.contains_key(parent_name) {
                anyhow::bail!(
                    "rule `{}` at {} extends unknown rule `{parent_name}`",
                    name,
                    locator.config_path.display(),
                );
            }
            stack.push(name.to_string());
            resolve_one(parent_name, by_name, resolved, stack)?;
            stack.pop();
            Some(resolved[parent_name].clone())
        }
        None => None,
    };

    let merged = merge(locator, parent_resolved.as_ref())?;
    resolved.insert(name.to_string(), merged);
    Ok(())
}

fn merge(locator: &RuleLocator<'_>, parent: Option<&ResolvedRule>) -> Result<ResolvedRule> {
    let raw: &RawRule = locator.rule;
    let ctx = format!("rule `{}` at {}", raw.name, locator.config_path.display());

    let paths = pick_list(
        raw.paths.as_deref(),
        parent.map(|p| &p.paths),
        default_paths,
    )
    .and_then(|v| with_ctx(constants::expand_list(&v), &ctx, "paths"))?;

    let extensions = pick_list(
        raw.extensions.as_deref(),
        parent.map(|p| &p.extensions),
        default_extensions,
    )
    .and_then(|v| with_ctx(constants::expand_list(&v), &ctx, "extensions"))?;

    let forms = match raw.forms.as_deref() {
        Some(v) => v.to_vec(),
        None => parent
            .map(|p| p.forms.clone())
            .unwrap_or_else(default_forms),
    };

    let match_regex = match raw.match_regex.as_deref() {
        Some(s) => with_ctx(constants::substitute_in_string(s), &ctx, "match")?,
        None => parent
            .map(|p| p.match_regex.clone())
            .unwrap_or_else(default_match_regex),
    };

    let match_resolved = match raw.match_resolved.as_ref() {
        Some(m) => Some(resolve_match_resolved(m, &ctx)?),
        None => parent.and_then(|p| p.match_resolved.clone()),
    };

    let allowed_include_dirs = pick_list(
        raw.allowed_include_dirs.as_deref(),
        parent.map(|p| &p.allowed_include_dirs),
        Vec::new,
    )
    .and_then(|v| with_ctx(constants::expand_list(&v), &ctx, "allowed_include_dirs"))?;

    let original_include_dirs = pick_list(
        raw.original_include_dirs.as_deref(),
        parent.map(|p| &p.original_include_dirs),
        Vec::new,
    )
    .and_then(|v| with_ctx(constants::expand_list(&v), &ctx, "original_include_dirs"))?;

    let action = match raw.action.as_ref() {
        Some(a) => resolve_action(a, &ctx)?,
        None => parent
            .map(|p| p.action.clone())
            .unwrap_or_else(default_action),
    };

    let trailing_comment = match raw.trailing_comment.as_ref() {
        Some(t) => Some(resolve_trailing_comment(t, &ctx)?),
        None => parent.and_then(|p| p.trailing_comment.clone()),
    };

    let origin = Origin {
        config_path: locator.config_path.to_path_buf(),
        config_dir: locator
            .config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
        index: locator.index,
    };

    Ok(ResolvedRule {
        name: raw.name.clone(),
        extends: raw.extends.clone(),
        origin,
        paths,
        extensions,
        forms,
        match_regex,
        match_resolved,
        allowed_include_dirs,
        original_include_dirs,
        action,
        trailing_comment,
    })
}

fn resolve_trailing_comment(
    raw: &RawTrailingComment,
    ctx: &str,
) -> Result<ResolvedTrailingComment> {
    let (match_regex_raw, to_raw, form, spacing) = match raw {
        RawTrailingComment::Shortcut(s) => {
            if s.is_empty() {
                anyhow::bail!(
                    "{ctx}: `trailing_comment` shortcut must be a non-empty string; \
                     use the table form `{{ to = \"\" }}` to strip the trailing comment"
                );
            }
            (None, s.clone(), TrailingForm::Preserve, None)
        }
        RawTrailingComment::Full(f) => {
            let to = f.to.clone().ok_or_else(|| {
                anyhow::anyhow!("{ctx}: `trailing_comment.to` is required")
            })?;
            (
                f.match_regex.clone(),
                to,
                f.form.unwrap_or(TrailingForm::Preserve),
                f.spacing,
            )
        }
    };

    let match_regex_raw = match_regex_raw.unwrap_or_else(|| ".*".to_string());
    let match_regex = with_ctx(
        constants::substitute_in_string(&match_regex_raw),
        ctx,
        "trailing_comment.match",
    )?;
    let to = with_ctx(
        constants::substitute_in_string(&to_raw),
        ctx,
        "trailing_comment.to",
    )?;
    Ok(ResolvedTrailingComment {
        match_regex,
        to,
        form,
        spacing,
    })
}

fn resolve_match_resolved(raw: &RawMatchResolved, ctx: &str) -> Result<ResolvedMatchResolved> {
    if raw.under.is_none() && raw.path_regex.is_none() {
        anyhow::bail!("{ctx}: `match_resolved` must specify at least one of `under` / `match`");
    }
    let path_regex = match raw.path_regex.as_deref() {
        Some(s) => Some(with_ctx(
            constants::substitute_in_string(s),
            ctx,
            "match_resolved.match",
        )?),
        None => None,
    };
    Ok(ResolvedMatchResolved {
        under: raw.under.clone(),
        path_regex,
    })
}

fn resolve_action(raw: &RawAction, ctx: &str) -> Result<ResolvedAction> {
    match raw {
        RawAction::Auto { relative_to, form } => Ok(ResolvedAction::Auto {
            relative_to: relative_to.unwrap_or(AutoRelativeTo::Allowed),
            form: form.unwrap_or(OutputForm::Quote),
        }),
        RawAction::Rewrite { to, form } => Ok(ResolvedAction::Rewrite {
            to: with_ctx(constants::substitute_in_string(to), ctx, "action.to")?,
            form: form.unwrap_or(OutputForm::Preserve),
        }),
        RawAction::Keep => Ok(ResolvedAction::Keep),
        RawAction::Error { message } => Ok(ResolvedAction::Error {
            message: message.clone(),
        }),
    }
}

fn pick_list(
    own: Option<&[String]>,
    parent: Option<&Vec<String>>,
    default_fn: impl FnOnce() -> Vec<String>,
) -> Result<Vec<String>> {
    Ok(match own {
        Some(v) => v.to_vec(),
        None => match parent {
            Some(v) => v.clone(),
            None => default_fn(),
        },
    })
}

fn with_ctx<T>(r: Result<T>, ctx: &str, field: &str) -> Result<T> {
    r.with_context(|| format!("{ctx}: while expanding `{field}`"))
}

/// Lint helper: report rule names whose ancestors form a tree (for explain
/// / debugging output). Not used by the engine; exposed for tests.
#[cfg(test)]
pub fn ancestors_of<'a>(rules: &'a BTreeMap<String, ResolvedRule>, name: &'a str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut current = name;
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some(r) = rules.get(current) {
        if !seen.insert(current) {
            break;
        }
        out.push(current);
        match r.extends.as_deref() {
            Some(p) => current = p,
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::parse;
    use std::path::Path;

    fn load(path: &str, body: &str) -> LoadedConfig {
        LoadedConfig {
            path: PathBuf::from(path),
            raw: parse(body, Path::new(path)).unwrap(),
        }
    }

    #[test]
    fn standalone_rule_gets_all_defaults() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let r = &map["base"];
        assert_eq!(r.paths, vec!["**"]);
        // Default extensions cover canonical C + C++ file extensions via
        // @std.c_extensions + @std.cpp_extensions.
        assert!(r.extensions.contains(&".c".to_string()));
        assert!(r.extensions.contains(&".h".to_string()));
        assert!(r.extensions.contains(&".cpp".to_string()));
        assert!(r.extensions.contains(&".hpp".to_string()));
        assert_eq!(r.forms, vec![IncludeForm::Quote]);
        assert_eq!(r.match_regex, ".*");
        assert!(matches!(
            r.action,
            ResolvedAction::Auto {
                relative_to: AutoRelativeTo::Allowed,
                form: OutputForm::Quote
            }
        ));
    }

    #[test]
    fn child_inherits_unspecified_fields_from_parent() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            allowed_include_dirs = ["include"]
            original_include_dirs = ["src", "src/internal"]
            forms = ["quote"]

            [[rule]]
            name = "child"
            extends = "base"
            match = '^old_(.*)$'
            action = { type = "rewrite", to = "new_${1}" }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let child = &map["child"];
        assert_eq!(child.paths, vec!["src/**"]);
        assert_eq!(child.allowed_include_dirs, vec!["include"]);
        assert_eq!(child.original_include_dirs, vec!["src", "src/internal"]);
        assert_eq!(child.forms, vec![IncludeForm::Quote]);
        assert_eq!(child.match_regex, "^old_(.*)$");
        assert!(matches!(child.action, ResolvedAction::Rewrite { .. }));
    }

    #[test]
    fn child_overrides_parent() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]

            [[rule]]
            name = "narrow"
            extends = "base"
            paths = ["src/foo/**"]
            forms = ["angle"]
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        assert_eq!(map["narrow"].paths, vec!["src/foo/**"]);
        assert_eq!(map["narrow"].forms, vec![IncludeForm::Angle]);
    }

    #[test]
    fn constants_are_expanded() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            extensions = ["@std.c_extensions", ".inl"]
            match = "^(@std.c89.system_headers_or)$"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let r = &map["base"];
        assert!(r.extensions.contains(&".c".to_string()));
        assert!(r.extensions.contains(&".inl".to_string()));
        assert!(r.match_regex.contains(r"stdio\.h"));
    }

    #[test]
    fn cycle_is_detected() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "a"
            extends = "b"

            [[rule]]
            name = "b"
            extends = "a"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("cycle"), "got: {msg}");
    }

    #[test]
    fn unknown_extends_target_errors() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "child"
            extends = "nonexistent"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err:#}").contains("unknown rule"));
    }

    #[test]
    fn match_resolved_round_trips() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "x"
            match_resolved = { under = "src/internal", match = '\.h$' }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let m = map["x"].match_resolved.as_ref().unwrap();
        assert_eq!(m.under.as_deref(), Some("src/internal"));
        assert_eq!(m.path_regex.as_deref(), Some(r"\.h$"));
    }

    #[test]
    fn match_resolved_requires_at_least_one_field() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "x"
            match_resolved = {}
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err:#}").contains("at least one"));
    }

    #[test]
    fn match_resolved_inherits_from_parent() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "p"
            match_resolved = { under = "src" }

            [[rule]]
            name = "c"
            extends = "p"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let m = map["c"].match_resolved.as_ref().unwrap();
        assert_eq!(m.under.as_deref(), Some("src"));
    }

    #[test]
    fn extends_across_configs_works_by_name() {
        let root = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            allowed_include_dirs = ["include"]
            "#,
        );
        let sub = load(
            "/p/src/inclean.toml",
            r#"
            [[rule]]
            name = "src-only"
            extends = "base"
            match = '^foo\.h$'
            "#,
        );
        let map = resolve(&[root, sub]).unwrap();
        assert_eq!(map["src-only"].paths, vec!["src/**"]);
        assert_eq!(map["src-only"].allowed_include_dirs, vec!["include"]);
    }

    #[test]
    fn action_template_is_constant_substituted() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "x"
            action = { type = "rewrite", to = "@std.c_extensions ${1}" }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        match &map["x"].action {
            ResolvedAction::Rewrite { to, .. } => {
                // @std.c_extensions is a list constant; in a string it
                // materializes as a regex alternation.
                assert!(to.starts_with(r"(?:\.h|\.c)"));
                assert!(to.ends_with("${1}"));
            }
            _ => panic!("expected rewrite"),
        }
    }

    #[test]
    fn ancestors_helper_walks_chain() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "a"

            [[rule]]
            name = "b"
            extends = "a"

            [[rule]]
            name = "c"
            extends = "b"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        assert_eq!(ancestors_of(&map, "c"), vec!["c", "b", "a"]);
    }

    #[test]
    fn trailing_comment_shortcut_fills_defaults() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = "note"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["r"].trailing_comment.as_ref().unwrap();
        assert_eq!(t.to, "note");
        assert_eq!(t.match_regex, ".*");
        assert_eq!(t.form, TrailingForm::Preserve);
        assert_eq!(t.spacing, None);
    }

    #[test]
    fn trailing_comment_full_form_minimal_fills_defaults() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = { to = "X" }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["r"].trailing_comment.as_ref().unwrap();
        assert_eq!(t.to, "X");
        assert_eq!(t.match_regex, ".*");
        assert_eq!(t.form, TrailingForm::Preserve);
        assert_eq!(t.spacing, None);
    }

    #[test]
    fn trailing_comment_full_form_round_trips_all_fields() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = { match = "^$", to = "X", form = "block", spacing = 4 }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["r"].trailing_comment.as_ref().unwrap();
        assert_eq!(t.match_regex, "^$");
        assert_eq!(t.to, "X");
        assert_eq!(t.form, TrailingForm::Block);
        assert_eq!(t.spacing, Some(4));
    }

    #[test]
    fn trailing_comment_to_empty_is_allowed_in_table_form() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = { to = "" }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["r"].trailing_comment.as_ref().unwrap();
        assert!(t.to.is_empty());
    }

    #[test]
    fn trailing_comment_shortcut_empty_is_rejected() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = ""
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("non-empty"), "got: {msg}");
        assert!(msg.contains("rule `r`"), "should pinpoint rule: {msg}");
    }

    #[test]
    fn trailing_comment_full_form_missing_to_is_rejected() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = { match = ".*" }
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("`trailing_comment.to` is required"), "got: {msg}");
    }

    #[test]
    fn trailing_comment_match_supports_std_constants() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "r"
            trailing_comment = { match = "^(@std.c89.system_headers_or)$", to = "X" }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["r"].trailing_comment.as_ref().unwrap();
        // The @std.*_or constants expand into a regex alternation.
        assert!(t.match_regex.contains(r"stdio\.h"), "got: {}", t.match_regex);
    }

    #[test]
    fn trailing_comment_inherits_from_parent() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "p"
            trailing_comment = "note"

            [[rule]]
            name = "c"
            extends = "p"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["c"].trailing_comment.as_ref().unwrap();
        assert_eq!(t.to, "note");
    }

    #[test]
    fn trailing_comment_child_overrides_parent() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "p"
            trailing_comment = "parent"

            [[rule]]
            name = "c"
            extends = "p"
            trailing_comment = { to = "child", form = "block", spacing = 1 }
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        let t = map["c"].trailing_comment.as_ref().unwrap();
        assert_eq!(t.to, "child");
        assert_eq!(t.form, TrailingForm::Block);
        assert_eq!(t.spacing, Some(1));
    }

    #[test]
    fn origin_carries_config_path_and_index() {
        let cfg = load(
            "/p/inclean.toml",
            r#"
            [[rule]]
            name = "first"

            [[rule]]
            name = "second"
            "#,
        );
        let map = resolve(&[cfg]).unwrap();
        assert_eq!(map["first"].origin.index, 0);
        assert_eq!(map["second"].origin.index, 1);
        assert_eq!(
            map["first"].origin.config_path,
            PathBuf::from("/p/inclean.toml")
        );
    }
}
