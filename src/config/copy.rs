//! Resolve `copied_from` chains into fully-baked [`ResolvedRule`]s.
//!
//! Single-level copy semantics (replaces the old `extends` AND-merge):
//!
//! - A rule's `copied_from` names another rule that **must be declared
//!   earlier** in the config (forward-only references). Self-references
//!   are rejected.
//! - The child starts from the parent's already-resolved `ResolvedRule`
//!   (so copies are transitive — what the child sees is whatever the
//!   parent finally became, including the parent's own copies).
//! - **Top-level field omitted by the child** → inherit the parent's
//!   resolved value.
//! - **Top-level field written by the child** → replace whole-cloth; the
//!   inner fields of objects do NOT auto-inherit (asymmetric reset). The
//!   child must write `${copied}` per inner field to pull the parent's
//!   resolved value.
//!
//! `${copied}` placeholder semantics:
//!
//! - **Scalar context** (string-typed field): the entire string is
//!   `"${copied}"` → use parent's resolved scalar.
//! - **Array-element context** (string lists): an element equal to
//!   `"${copied}"` is a splat — expand parent's whole array at that
//!   position. An empty / unset parent list expands to zero elements.
//! - Using `${copied}` in a rule without `copied_from` is a hard error.
//!
//! Defaults are applied after copy resolution.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use super::constants;
use super::schema::{
    CommentStyle, IncludeForm, IncludeOnAmbiguous, IncludeOnUnresolved, LoadedConfig, MacroRewrite,
    MaybeCopiedObject, MaybeCopiedOrSkipObject, OutputCommentStyle, OutputForm, RawAction, RawRule,
    RawSuppression, RawTrailingAction, RawTrailingComment, RawTrailingTransform, RuleLocator,
};

const COPIED_TOKEN: &str = "${copied}";

/// Where a rule was declared.
#[derive(Debug, Clone)]
pub struct Origin {
    pub config_path: PathBuf,
    pub config_dir: PathBuf,
    pub index: usize,
}

/// Fully merged + defaulted view of a rule.
#[derive(Debug, Clone)]
pub struct ResolvedRule {
    pub name: String,
    pub copied_from: Option<String>,
    pub origin: Origin,

    pub file_paths: Vec<String>,
    pub file_suffixes: Vec<String>,
    pub include_forms: Vec<IncludeForm>,
    pub macro_rewrite: MacroRewrite,
    pub include_match: Vec<String>,
    pub include_directories: Vec<String>,
    pub include_resolved_match: Vec<String>,
    pub include_on_unresolved: IncludeOnUnresolved,
    pub include_on_ambiguous: IncludeOnAmbiguous,

    pub suppression: ResolvedSuppression,
    pub action: ResolvedAction,
    pub trailing_comment: ResolvedTrailingComment,
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedSuppression {
    pub block_start: Option<String>,
    pub block_end: Option<String>,
    pub line: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ResolvedAction {
    Skip,
    Resolve {
        relative_to: String,
        output_form: OutputForm,
        message: String,
    },
    Replace {
        with: String,
        output_form: OutputForm,
        message: String,
    },
    Keep {
        output_form: OutputForm,
        message: String,
    },
    Remove {
        keep_blank_line: bool,
        keep_trailing_comment: bool,
        message: String,
    },
    CommentOut {
        style: CommentStyle,
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedTrailingComment {
    pub skip: bool,
    pub transform: Option<ResolvedTrailingTransform>,
    pub append_if_absent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTrailingTransform {
    pub match_styles: Vec<CommentStyle>,
    pub content_regex: String,
    pub action: ResolvedTrailingAction,
}

#[derive(Debug, Clone)]
pub enum ResolvedTrailingAction {
    Replace {
        with: String,
        output_style: OutputCommentStyle,
        message: String,
    },
    Keep {
        output_style: OutputCommentStyle,
        message: String,
    },
    Remove {
        message: String,
    },
    Error {
        message: String,
    },
}

// ---- Defaults ------------------------------------------------------------

fn default_file_paths() -> Vec<String> {
    vec!["**/*".to_string()]
}
fn default_file_suffixes() -> Vec<String> {
    vec![
        "@std.c.extensions".to_string(),
        "@std.cpp.extensions".to_string(),
    ]
}
fn default_include_forms() -> Vec<IncludeForm> {
    vec![IncludeForm::Quote]
}
fn default_macro_rewrite() -> MacroRewrite {
    MacroRewrite::Definitions
}
fn default_include_match() -> Vec<String> {
    vec!["**".to_string()]
}
fn default_include_resolved_match() -> Vec<String> {
    vec!["**".to_string()]
}
fn default_action() -> ResolvedAction {
    ResolvedAction::Skip
}
fn default_trailing_comment() -> ResolvedTrailingComment {
    ResolvedTrailingComment {
        skip: true,
        ..ResolvedTrailingComment::default()
    }
}

// ---- Entry point ---------------------------------------------------------

/// Resolve every rule across `configs`. Rules are walked in declaration
/// order so that `copied_from` references can point only at already-
/// resolved earlier rules.
///
/// Returns a `Vec<(name, rule)>` in declaration order. Downstream code
/// (engine, CLI, conflict diagnostics) depends on this order. Use
/// [`find_resolved`] for name lookup.
pub fn resolve(configs: &[LoadedConfig]) -> Result<Vec<(String, ResolvedRule)>> {
    let mut out: Vec<(String, ResolvedRule)> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();

    for cfg in configs {
        for (idx, raw) in cfg.raw.rules.iter().enumerate() {
            let locator = RuleLocator {
                config_path: &cfg.path,
                index: idx,
                rule: raw,
            };
            if by_name.contains_key(&raw.name) {
                bail!(
                    "duplicate rule name `{}`: re-declared at {} (rule #{})",
                    raw.name,
                    cfg.path.display(),
                    idx,
                );
            }
            let parent = match raw.copied_from.as_deref() {
                None => None,
                Some(parent_name) => {
                    if parent_name == raw.name {
                        bail!(
                            "rule `{}` at {}: cannot copy itself",
                            raw.name,
                            cfg.path.display()
                        );
                    }
                    let parent_idx = by_name.get(parent_name).ok_or_else(|| {
                        anyhow::anyhow!(
                            "rule `{}` at {}: copied_from = `{}` refers to a rule that is not declared earlier (forward-only references)",
                            raw.name,
                            cfg.path.display(),
                            parent_name,
                        )
                    })?;
                    Some(out[*parent_idx].1.clone())
                }
            };
            let rule = build(&locator, parent.as_ref())?;
            by_name.insert(rule.name.clone(), out.len());
            out.push((rule.name.clone(), rule));
        }
    }

    Ok(out)
}

/// Look up a resolved rule by name. Linear scan over the declaration-
/// ordered vec; fine for sizes we care about. Returns `None` when not found.
pub fn find_resolved<'a>(
    resolved: &'a [(String, ResolvedRule)],
    name: &str,
) -> Option<&'a ResolvedRule> {
    resolved.iter().find(|(n, _)| n == name).map(|(_, r)| r)
}

// ---- Per-rule build -------------------------------------------------------

fn build(locator: &RuleLocator<'_>, parent: Option<&ResolvedRule>) -> Result<ResolvedRule> {
    let raw: &RawRule = locator.rule;
    let ctx = format!("rule `{}` at {}", raw.name, locator.config_path.display());
    let has_parent = parent.is_some();

    let file_paths = resolve_str_list(
        raw.file_paths.as_deref(),
        parent.map(|p| &p.file_paths),
        default_file_paths,
        &ctx,
        "file_paths",
        has_parent,
    )?;
    let file_suffixes_raw = resolve_str_list(
        raw.file_suffixes.as_deref(),
        parent.map(|p| &p.file_suffixes),
        default_file_suffixes,
        &ctx,
        "file_suffixes",
        has_parent,
    )?;
    let file_suffixes = with_ctx(
        constants::expand_list(&file_suffixes_raw),
        &ctx,
        "file_suffixes",
    )?;

    let include_forms = match raw.include_forms.as_ref() {
        Some(v) => v.clone(),
        None => parent
            .map(|p| p.include_forms.clone())
            .unwrap_or_else(default_include_forms),
    };

    let macro_rewrite = raw
        .macro_rewrite
        .or_else(|| parent.map(|p| p.macro_rewrite))
        .unwrap_or_else(default_macro_rewrite);

    let include_match = resolve_str_list(
        raw.include_match.as_deref(),
        parent.map(|p| &p.include_match),
        default_include_match,
        &ctx,
        "include_match",
        has_parent,
    )?;

    let include_directories = resolve_str_list(
        raw.include_directories.as_deref(),
        parent.map(|p| &p.include_directories),
        Vec::new,
        &ctx,
        "include_directories",
        has_parent,
    )?;

    let include_resolved_match = resolve_str_list(
        raw.include_resolved_match.as_deref(),
        parent.map(|p| &p.include_resolved_match),
        default_include_resolved_match,
        &ctx,
        "include_resolved_match",
        has_parent,
    )?;

    let include_on_unresolved = raw
        .include_on_unresolved
        .or_else(|| parent.map(|p| p.include_on_unresolved))
        .unwrap_or(IncludeOnUnresolved::Error);

    let include_on_ambiguous = raw
        .include_on_ambiguous
        .or_else(|| parent.map(|p| p.include_on_ambiguous))
        .unwrap_or(IncludeOnAmbiguous::Error);

    let suppression = match raw.suppression_comments_regex.as_ref() {
        Some(MaybeCopiedObject::Copied) => {
            if !has_parent {
                bail!(
                    "{ctx}: `suppression_comments_regex = \"${{copied}}\"` requires `copied_from`"
                );
            }
            parent.map(|p| p.suppression.clone()).unwrap_or_default()
        }
        Some(MaybeCopiedObject::Object(s)) => {
            build_suppression(s, parent.map(|p| &p.suppression), has_parent, &ctx)?
        }
        None => parent.map(|p| p.suppression.clone()).unwrap_or_default(),
    };

    let action = match raw.action.as_ref() {
        Some(MaybeCopiedOrSkipObject::Copied) => {
            if !has_parent {
                bail!("{ctx}: `action = \"${{copied}}\"` requires `copied_from`");
            }
            parent
                .map(|p| p.action.clone())
                .unwrap_or_else(default_action)
        }
        Some(MaybeCopiedOrSkipObject::Skip) => ResolvedAction::Skip,
        Some(MaybeCopiedOrSkipObject::Object(a)) => {
            build_action(a, parent.map(|p| &p.action), has_parent, &ctx)?
        }
        None => parent
            .map(|p| p.action.clone())
            .unwrap_or_else(default_action),
    };

    if matches!(include_on_unresolved, IncludeOnUnresolved::Allow)
        && matches!(action, ResolvedAction::Resolve { .. })
    {
        bail!("{ctx}: `include_on_unresolved = \"allow\"` cannot be used with action `resolve`");
    }

    let trailing_comment = match raw.trailing_comment.as_ref() {
        Some(MaybeCopiedOrSkipObject::Copied) => {
            if !has_parent {
                bail!("{ctx}: `trailing_comment = \"${{copied}}\"` requires `copied_from`");
            }
            parent
                .map(|p| p.trailing_comment.clone())
                .unwrap_or_default()
        }
        Some(MaybeCopiedOrSkipObject::Skip) => ResolvedTrailingComment {
            skip: true,
            ..ResolvedTrailingComment::default()
        },
        Some(MaybeCopiedOrSkipObject::Object(t)) => {
            build_trailing(t, parent.map(|p| &p.trailing_comment), has_parent, &ctx)?
        }
        None => parent
            .map(|p| p.trailing_comment.clone())
            .unwrap_or_else(default_trailing_comment),
    };

    Ok(ResolvedRule {
        name: raw.name.clone(),
        copied_from: raw.copied_from.clone(),
        origin: Origin {
            config_path: locator.config_path.to_path_buf(),
            config_dir: locator
                .config_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(".")),
            index: locator.index,
        },
        file_paths,
        file_suffixes,
        include_forms,
        macro_rewrite,
        include_match,
        include_directories,
        include_resolved_match,
        include_on_unresolved,
        include_on_ambiguous,
        suppression,
        action,
        trailing_comment,
    })
}

// ---- ${copied} helpers ----------------------------------------------------

fn resolve_str_list(
    child: Option<&[String]>,
    parent: Option<&Vec<String>>,
    default_fn: impl FnOnce() -> Vec<String>,
    ctx: &str,
    field: &str,
    has_parent: bool,
) -> Result<Vec<String>> {
    let raw = match child {
        Some(v) => v,
        None => {
            return Ok(match parent {
                Some(v) => v.clone(),
                None => default_fn(),
            });
        }
    };
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for elem in raw {
        if elem == COPIED_TOKEN {
            if !has_parent {
                bail!("{ctx}: `{field}` uses `{COPIED_TOKEN}` but this rule has no `copied_from`");
            }
            if let Some(pl) = parent {
                out.extend(pl.iter().cloned());
            }
        } else {
            out.push(elem.clone());
        }
    }
    Ok(out)
}

/// Resolve a scalar string inside a child-wrote-the-outer-object context.
/// Asymmetric reset: when the child omits the inner field, the field
/// resets to its schema default — it does NOT auto-inherit. The child
/// must write `${copied}` to pull the parent's value explicitly.
fn resolve_str(
    child: Option<&str>,
    parent: Option<String>,
    default: &str,
    ctx: &str,
    field: &str,
    has_parent: bool,
) -> Result<String> {
    match child {
        None => Ok(default.to_string()),
        Some(s) if s == COPIED_TOKEN => {
            if !has_parent {
                bail!("{ctx}: `{field}` uses `{COPIED_TOKEN}` but this rule has no `copied_from`");
            }
            parent.ok_or_else(|| {
                anyhow::anyhow!(
                    "{ctx}: `{field}` uses `{COPIED_TOKEN}` but the parent's value is unset"
                )
            })
        }
        Some(s) => with_ctx(constants::substitute_in_string(s), ctx, field),
    }
}

/// Optional-scalar variant of [`resolve_str`]. Child=None → None
/// (asymmetric reset); `${copied}` pulls parent's value (which may itself
/// be None — that's fine).
fn resolve_opt_str(
    child: Option<&str>,
    parent: Option<String>,
    ctx: &str,
    field: &str,
    has_parent: bool,
) -> Result<Option<String>> {
    match child {
        None => Ok(None),
        Some(s) if s == COPIED_TOKEN => {
            if !has_parent {
                bail!("{ctx}: `{field}` uses `{COPIED_TOKEN}` but this rule has no `copied_from`");
            }
            Ok(parent)
        }
        Some(s) => Ok(Some(with_ctx(
            constants::substitute_in_string(s),
            ctx,
            field,
        )?)),
    }
}

// ---- Action / trailing / suppression sub-builders -------------------------

fn build_suppression(
    raw: &RawSuppression,
    parent: Option<&ResolvedSuppression>,
    has_parent: bool,
    ctx: &str,
) -> Result<ResolvedSuppression> {
    let block_start = resolve_opt_str(
        raw.block_start.as_deref(),
        parent.and_then(|p| p.block_start.clone()),
        ctx,
        "suppression_comments_regex.block_start",
        has_parent,
    )?;
    let block_end = resolve_opt_str(
        raw.block_end.as_deref(),
        parent.and_then(|p| p.block_end.clone()),
        ctx,
        "suppression_comments_regex.block_end",
        has_parent,
    )?;
    let line = resolve_opt_str(
        raw.line.as_deref(),
        parent.and_then(|p| p.line.clone()),
        ctx,
        "suppression_comments_regex.line",
        has_parent,
    )?;
    Ok(ResolvedSuppression {
        block_start,
        block_end,
        line,
    })
}

fn build_action(
    raw: &RawAction,
    parent: Option<&ResolvedAction>,
    has_parent: bool,
    ctx: &str,
) -> Result<ResolvedAction> {
    // Asymmetric reset: when the child writes `action = { ... }`, the
    // omitted inner fields default to the schema's defaults — not to the
    // parent's resolved values. Only `${copied}` pulls a per-field
    // value from the parent, and only when the parent's resolved action
    // shares the same variant.
    let p_resolve_relative_to = parent.and_then(|p| match p {
        ResolvedAction::Resolve { relative_to, .. } => Some(relative_to.clone()),
        _ => None,
    });
    let p_replace_with = parent.and_then(|p| match p {
        ResolvedAction::Replace { with, .. } => Some(with.clone()),
        _ => None,
    });
    let p_message = parent.map(|p| match p {
        ResolvedAction::Resolve { message, .. }
        | ResolvedAction::Replace { message, .. }
        | ResolvedAction::Keep { message, .. }
        | ResolvedAction::Remove { message, .. }
        | ResolvedAction::CommentOut { message, .. }
        | ResolvedAction::Error { message } => message.clone(),
        ResolvedAction::Skip => String::new(),
    });

    match raw {
        RawAction::Resolve {
            relative_to,
            output_form,
            message,
        } => Ok(ResolvedAction::Resolve {
            relative_to: resolve_str(
                Some(relative_to.as_str()),
                p_resolve_relative_to,
                "${current_file}",
                ctx,
                "action.relative_to",
                has_parent,
            )?,
            output_form: output_form.unwrap_or(OutputForm::Preserve),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "action.message",
                has_parent,
            )?,
        }),
        RawAction::Replace {
            with,
            output_form,
            message,
        } => Ok(ResolvedAction::Replace {
            with: resolve_str(
                Some(with.as_str()),
                p_replace_with,
                "",
                ctx,
                "action.with",
                has_parent,
            )?,
            output_form: output_form.unwrap_or(OutputForm::Preserve),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "action.message",
                has_parent,
            )?,
        }),
        RawAction::Keep {
            output_form,
            message,
        } => Ok(ResolvedAction::Keep {
            output_form: output_form.unwrap_or(OutputForm::Preserve),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "action.message",
                has_parent,
            )?,
        }),
        RawAction::Remove {
            keep_blank_line,
            keep_trailing_comment,
            message,
        } => Ok(ResolvedAction::Remove {
            keep_blank_line: keep_blank_line.unwrap_or(false),
            keep_trailing_comment: keep_trailing_comment.unwrap_or(true),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "action.message",
                has_parent,
            )?,
        }),
        RawAction::CommentOut { style, message } => Ok(ResolvedAction::CommentOut {
            style: style.unwrap_or(CommentStyle::Line),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "action.message",
                has_parent,
            )?,
        }),
        RawAction::Error { message } => Ok(ResolvedAction::Error {
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "action.message",
                has_parent,
            )?,
        }),
    }
}

fn build_trailing(
    raw: &RawTrailingComment,
    parent: Option<&ResolvedTrailingComment>,
    has_parent: bool,
    ctx: &str,
) -> Result<ResolvedTrailingComment> {
    // Asymmetric reset: when the child writes `trailing_comment`, inner
    // fields default to None unless the child writes them.
    let transform = match raw.transform.as_ref() {
        Some(t) => Some(build_trailing_transform(
            t,
            parent.and_then(|p| p.transform.as_ref()),
            has_parent,
            ctx,
        )?),
        None => None,
    };
    let parent_append = parent.and_then(|p| p.append_if_absent.clone());
    let append_if_absent = resolve_opt_str(
        raw.append_if_absent.as_deref(),
        parent_append,
        ctx,
        "trailing_comment.append_if_absent",
        has_parent,
    )?;
    if let Some(s) = append_if_absent.as_deref()
        && (s.contains('\n') || s.contains('\r'))
    {
        bail!(
            "{ctx}: `trailing_comment.append_if_absent` must not contain line terminators (\\n / \\r); it is appended onto the same line as the include"
        );
    }
    Ok(ResolvedTrailingComment {
        skip: false,
        transform,
        append_if_absent,
    })
}

fn build_trailing_transform(
    raw: &RawTrailingTransform,
    parent: Option<&ResolvedTrailingTransform>,
    has_parent: bool,
    ctx: &str,
) -> Result<ResolvedTrailingTransform> {
    // Asymmetric reset: child wrote `transform = { ... }`; omitted inner
    // fields default to schema defaults rather than parent's values.
    let match_styles = raw
        .match_styles
        .clone()
        .unwrap_or_else(|| vec![CommentStyle::Line, CommentStyle::Block]);
    let content_regex = resolve_str(
        raw.content_regex.as_deref(),
        parent.map(|p| p.content_regex.clone()),
        ".*",
        ctx,
        "trailing_comment.transform.content_regex",
        has_parent,
    )?;
    let action = build_trailing_action(&raw.action, parent.map(|p| &p.action), has_parent, ctx)?;
    Ok(ResolvedTrailingTransform {
        match_styles,
        content_regex,
        action,
    })
}

fn build_trailing_action(
    raw: &RawTrailingAction,
    parent: Option<&ResolvedTrailingAction>,
    has_parent: bool,
    ctx: &str,
) -> Result<ResolvedTrailingAction> {
    let p_replace_with = parent.and_then(|p| match p {
        ResolvedTrailingAction::Replace { with, .. } => Some(with.clone()),
        _ => None,
    });
    let p_message = parent.map(|p| match p {
        ResolvedTrailingAction::Replace { message, .. }
        | ResolvedTrailingAction::Keep { message, .. }
        | ResolvedTrailingAction::Remove { message }
        | ResolvedTrailingAction::Error { message } => message.clone(),
    });

    match raw {
        RawTrailingAction::Replace {
            with,
            output_style,
            message,
        } => Ok(ResolvedTrailingAction::Replace {
            with: resolve_str(
                Some(with.as_str()),
                p_replace_with,
                "",
                ctx,
                "trailing_comment.transform.action.with",
                has_parent,
            )?,
            output_style: output_style.unwrap_or(OutputCommentStyle::Preserve),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "trailing_comment.transform.action.message",
                has_parent,
            )?,
        }),
        RawTrailingAction::Keep {
            output_style,
            message,
        } => Ok(ResolvedTrailingAction::Keep {
            output_style: output_style.unwrap_or(OutputCommentStyle::Preserve),
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "trailing_comment.transform.action.message",
                has_parent,
            )?,
        }),
        RawTrailingAction::Remove { message } => Ok(ResolvedTrailingAction::Remove {
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "trailing_comment.transform.action.message",
                has_parent,
            )?,
        }),
        RawTrailingAction::Error { message } => Ok(ResolvedTrailingAction::Error {
            message: resolve_str(
                message.as_deref(),
                p_message,
                "",
                ctx,
                "trailing_comment.transform.action.message",
                has_parent,
            )?,
        }),
    }
}

fn with_ctx<T>(r: Result<T>, ctx: &str, field: &str) -> Result<T> {
    r.with_context(|| format!("{ctx}: while expanding `{field}`"))
}

// ---- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::utils::testing::config::load_rules;

    use super::*;

    fn get<'a>(rules: &'a [(String, ResolvedRule)], name: &str) -> &'a ResolvedRule {
        find_resolved(rules, name).unwrap_or_else(|| panic!("rule `{name}` not found"))
    }

    #[test]
    fn standalone_rule_gets_defaults() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let r = get(&resolved, "base");
        assert_eq!(r.file_paths, vec!["**/*"]);
        assert!(r.file_suffixes.contains(&".c".to_string()));
        assert!(r.file_suffixes.contains(&".h".to_string()));
        assert!(r.file_suffixes.contains(&".cpp".to_string()));
        assert_eq!(r.include_forms, vec![IncludeForm::Quote]);
        assert_eq!(r.macro_rewrite, MacroRewrite::Definitions);
        assert_eq!(r.include_match, vec!["**"]);
        assert_eq!(r.include_resolved_match, vec!["**"]);
        assert!(matches!(
            r.include_on_unresolved,
            IncludeOnUnresolved::Error
        ));
        assert!(matches!(r.include_on_ambiguous, IncludeOnAmbiguous::Error));
        assert!(matches!(r.action, ResolvedAction::Skip));
        assert!(r.trailing_comment.skip);
    }

    #[test]
    fn child_inherits_unspecified_fields() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**"]
            include_forms = ["quote", "angle"]
            macro_rewrite = "use_site"
            include_directories = ["src", "src/internal"]
            include_resolved_match = ["src/internal/**"]
            include_on_unresolved = "skip"
            include_on_ambiguous = "first"

            [[rule]]
            name = "child"
            copied_from = "base"
            action = { type = "replace", with = "x" }
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let child = get(&resolved, "child");
        assert_eq!(child.file_paths, vec!["src/**"]);
        assert_eq!(
            child.include_forms,
            vec![IncludeForm::Quote, IncludeForm::Angle]
        );
        assert_eq!(child.macro_rewrite, MacroRewrite::UseSite);
        assert_eq!(child.include_directories, vec!["src", "src/internal"]);
        assert_eq!(child.include_resolved_match, vec!["src/internal/**"]);
        assert!(matches!(
            child.include_on_unresolved,
            IncludeOnUnresolved::Skip
        ));
        assert!(matches!(
            child.include_on_ambiguous,
            IncludeOnAmbiguous::First
        ));
        assert!(matches!(child.action, ResolvedAction::Replace { .. }));
    }

    #[test]
    fn child_overrides_parent_at_top_level() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            file_paths = ["src/**"]
            include_forms = ["quote", "angle"]
            macro_rewrite = "use_site"
            include_resolved_match = ["src/**"]
            include_on_unresolved = "allow"
            include_on_ambiguous = "first"

            [[rule]]
            name = "narrow"
            copied_from = "base"
            file_paths = ["src/foo/**"]
            include_forms = ["macro"]
            macro_rewrite = "definitions"
            include_resolved_match = ["src/foo/**"]
            include_on_unresolved = "skip"
            include_on_ambiguous = "skip"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let narrow = get(&resolved, "narrow");
        assert_eq!(narrow.file_paths, vec!["src/foo/**"]);
        assert_eq!(narrow.include_forms, vec![IncludeForm::Macro]);
        assert_eq!(narrow.macro_rewrite, MacroRewrite::Definitions);
        assert_eq!(narrow.include_resolved_match, vec!["src/foo/**"]);
        assert!(matches!(
            narrow.include_on_unresolved,
            IncludeOnUnresolved::Skip
        ));
        assert!(matches!(
            narrow.include_on_ambiguous,
            IncludeOnAmbiguous::Skip
        ));
    }

    #[test]
    fn allow_unresolved_with_resolve_action_is_rejected() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "bad"
            include_directories = ["include"]
            include_on_unresolved = "allow"
            action = { type = "resolve", relative_to = "include" }
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("include_on_unresolved = \"allow\"") && msg.contains("resolve"),
            "{msg}"
        );
    }

    #[test]
    fn transitive_copy_through_intermediate() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "a"
            file_paths = ["src/**"]

            [[rule]]
            name = "b"
            copied_from = "a"

            [[rule]]
            name = "c"
            copied_from = "b"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        assert_eq!(get(&resolved, "c").file_paths, vec!["src/**"]);
    }

    #[test]
    fn copied_token_splats_parent_list() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            file_suffixes = [".c", ".h"]

            [[rule]]
            name = "child"
            copied_from = "base"
            file_suffixes = ["${copied}", ".inl"]
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        assert_eq!(
            get(&resolved, "child").file_suffixes,
            vec![".c", ".h", ".inl"]
        );
    }

    #[test]
    fn copied_token_in_scalar_inherits_parent_value() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            suppression_comments_regex = { line = "^inclean: skip$" }

            [[rule]]
            name = "child"
            copied_from = "base"
            suppression_comments_regex = { line = "${copied}" }
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        assert_eq!(
            get(&resolved, "child").suppression.line.as_deref(),
            Some("^inclean: skip$")
        );
    }

    #[test]
    fn asymmetric_reset_for_nested_object() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            suppression_comments_regex = {
                block_start = "BEGIN",
                block_end = "END",
                line = "^inclean: skip$",
            }

            [[rule]]
            name = "child"
            copied_from = "base"
            suppression_comments_regex = { line = "${copied}" }
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let s = &get(&resolved, "child").suppression;
        assert_eq!(s.line.as_deref(), Some("^inclean: skip$"));
        assert!(s.block_start.is_none());
        assert!(s.block_end.is_none());
    }

    #[test]
    fn unset_top_level_object_inherits_parent_wholesale() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            suppression_comments_regex = {
                block_start = "BEGIN",
                line = "L",
            }

            [[rule]]
            name = "child"
            copied_from = "base"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let s = &get(&resolved, "child").suppression;
        assert_eq!(s.block_start.as_deref(), Some("BEGIN"));
        assert_eq!(s.line.as_deref(), Some("L"));
    }

    #[test]
    fn resolve_preserves_declaration_order() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "zeta"

            [[rule]]
            name = "alpha"

            [[rule]]
            name = "middle"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let names: Vec<&str> = resolved.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha", "middle"]);
    }

    #[test]
    fn copied_token_without_parent_errors() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "lone"
            file_suffixes = ["${copied}"]
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err:#}").contains("${copied}"));
    }

    #[test]
    fn copied_from_pointing_at_later_rule_is_rejected() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "first"
            copied_from = "second"

            [[rule]]
            name = "second"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("forward-only") || msg.contains("not declared earlier"));
    }

    #[test]
    fn self_copy_is_rejected() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "a"
            copied_from = "a"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err:#}").contains("itself"));
    }

    #[test]
    fn unknown_copied_from_target_is_rejected() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "child"
            copied_from = "nonexistent"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err:#}").contains("nonexistent"));
    }

    #[test]
    fn constant_expansion_in_file_suffixes() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "r"
            file_suffixes = ["@std.c.extensions", ".inl"]
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let s = &get(&resolved, "r").file_suffixes;
        assert!(s.contains(&".c".to_string()));
        assert!(s.contains(&".h".to_string()));
        assert!(s.contains(&".inl".to_string()));
    }

    #[test]
    fn duplicate_rule_names_are_rejected() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "x"

            [[rule]]
            name = "x"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err}").contains("duplicate rule name"));
    }

    #[test]
    fn action_message_inherits_via_copied_token() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "base"
            action = { type = "error", message = "deprecated" }

            [[rule]]
            name = "child"
            copied_from = "base"
            action = { type = "error", message = "${copied}" }
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        match &get(&resolved, "child").action {
            ResolvedAction::Error { message } => assert_eq!(message, "deprecated"),
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn trailing_comment_transform_inherits_when_omitted() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "p"
            trailing_comment = {
                transform = {
                    content_regex = "^TODO.*$",
                    action = { type = "remove" },
                },
            }

            [[rule]]
            name = "c"
            copied_from = "p"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let tc = &get(&resolved, "c").trailing_comment;
        let t = tc.transform.as_ref().unwrap();
        assert_eq!(t.content_regex, "^TODO.*$");
        assert!(matches!(t.action, ResolvedTrailingAction::Remove { .. }));
    }

    #[test]
    fn object_context_copied_action() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "p"
            action = { type = "error", message = "deprecated" }

            [[rule]]
            name = "c"
            copied_from = "p"
            action = "${copied}"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        match &get(&resolved, "c").action {
            ResolvedAction::Error { message } => assert_eq!(message, "deprecated"),
            other => panic!("expected inherited Error action, got {other:?}"),
        }
    }

    #[test]
    fn skip_sentinel_resolves_for_action_and_trailing_comment() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "r"
            action = "skip"
            trailing_comment = "skip"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let r = get(&resolved, "r");
        assert!(matches!(r.action, ResolvedAction::Skip));
        assert!(r.trailing_comment.skip);
        assert!(r.trailing_comment.transform.is_none());
        assert!(r.trailing_comment.append_if_absent.is_none());
    }

    #[test]
    fn object_context_copied_trailing_inherits_whole_object() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "p"
            trailing_comment = {
                transform = {
                    content_regex = "^TODO.*$",
                    action = { type = "remove" },
                },
                append_if_absent = " // note",
            }

            [[rule]]
            name = "c"
            copied_from = "p"
            trailing_comment = "${copied}"
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let tc = &get(&resolved, "c").trailing_comment;
        assert!(tc.transform.is_some());
        assert_eq!(tc.append_if_absent.as_deref(), Some(" // note"));
    }

    #[test]
    fn object_context_copied_without_parent_is_rejected() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "lone"
            action = "${copied}"
            "#,
        );
        let err = resolve(&[cfg]).unwrap_err();
        assert!(format!("{err:#}").contains("${copied}"));
    }

    #[test]
    fn append_if_absent_with_newline_is_rejected() {
        let cfg = load_rules(
            "[[rule]]\nname = \"r\"\ntrailing_comment = { append_if_absent = \"x\\ny\" }\n",
        );
        let err = resolve(&[cfg]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must not contain line terminators"));
    }

    #[test]
    fn trailing_comment_writes_reset_transform_to_none() {
        let cfg = load_rules(
            r#"
            [[rule]]
            name = "p"
            trailing_comment = {
                transform = {
                    content_regex = "^TODO.*$",
                    action = { type = "remove" },
                },
            }

            [[rule]]
            name = "c"
            copied_from = "p"
            trailing_comment = { append_if_absent = " // note" }
            "#,
        );
        let resolved = resolve(&[cfg]).unwrap();
        let c = get(&resolved, "c");
        assert!(c.trailing_comment.transform.is_none());
        assert_eq!(
            c.trailing_comment.append_if_absent.as_deref(),
            Some(" // note")
        );
    }
}
