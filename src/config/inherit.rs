//! Resolve `copied_from` chains into fully-baked [`ResolvedRule`]s.
//!
//! ⚠️ M1 transitional stub. This module's logic is being replaced by
//! `src/config/copy.rs` in M2. For now it provides just enough type
//! plumbing for the rest of the crate to compile after the schema
//! rewrite: each [`ResolvedRule`] is built straight from its [`RawRule`]
//! with defaults applied per field; no copy resolution, no `${copied}`
//! substitution, no constant expansion. Tests live in M2's `copy::`
//! tests, not here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;

use super::schema::{
    index_rules_by_name, CommentStyle, IncludeForm, LoadedConfig, OutputCommentStyle, OutputForm,
    RawAction, RawRule, RawSuppression, RawTrailingAction, RawTrailingComment, RawTrailingTransform,
    RuleLocator,
};

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
    pub match_forms: Vec<IncludeForm>,
    pub include_match: Vec<String>,
    pub include_directories: Vec<String>,

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

fn default_file_paths() -> Vec<String> {
    vec!["**/*".to_string()]
}
fn default_file_suffixes() -> Vec<String> {
    vec![
        "@std.c.extensions".to_string(),
        "@std.cpp.extensions".to_string(),
    ]
}
fn default_match_forms() -> Vec<IncludeForm> {
    vec![IncludeForm::Quote]
}
fn default_include_match() -> Vec<String> {
    vec!["**".to_string()]
}
fn default_action() -> ResolvedAction {
    ResolvedAction::Keep {
        output_form: OutputForm::Preserve,
        message: String::new(),
    }
}

/// Stub resolver used during the M1 transition. Walks rules in
/// declaration order, applies per-field defaults, and ignores
/// `copied_from`. Real copy resolution arrives in M2 (`copy::resolve`).
pub fn resolve(configs: &[LoadedConfig]) -> Result<BTreeMap<String, ResolvedRule>> {
    let by_name = index_rules_by_name(configs)?;
    let mut out: BTreeMap<String, ResolvedRule> = BTreeMap::new();
    for (name, locator) in &by_name {
        out.insert(name.clone(), defaulted(locator));
    }
    Ok(out)
}

fn defaulted(locator: &RuleLocator<'_>) -> ResolvedRule {
    let raw: &RawRule = locator.rule;
    ResolvedRule {
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
        file_paths: raw.file_paths.clone().unwrap_or_else(default_file_paths),
        file_suffixes: raw
            .file_suffixes
            .clone()
            .unwrap_or_else(default_file_suffixes),
        match_forms: raw.match_forms.clone().unwrap_or_else(default_match_forms),
        include_match: raw
            .include_match
            .clone()
            .unwrap_or_else(default_include_match),
        include_directories: raw.include_directories.clone().unwrap_or_default(),
        suppression: raw
            .suppression_comments_regex
            .as_ref()
            .map(suppression_from)
            .unwrap_or_default(),
        action: raw.action.as_ref().map(action_from).unwrap_or_else(default_action),
        trailing_comment: raw
            .trailing_comment
            .as_ref()
            .map(trailing_from)
            .unwrap_or_default(),
    }
}

fn suppression_from(raw: &RawSuppression) -> ResolvedSuppression {
    ResolvedSuppression {
        block_start: raw.block_start.clone(),
        block_end: raw.block_end.clone(),
        line: raw.line.clone(),
    }
}

fn action_from(raw: &RawAction) -> ResolvedAction {
    match raw {
        RawAction::Resolve {
            relative_to,
            output_form,
            message,
        } => ResolvedAction::Resolve {
            relative_to: relative_to.clone(),
            output_form: output_form.unwrap_or(OutputForm::Preserve),
            message: message.clone().unwrap_or_default(),
        },
        RawAction::Replace {
            with,
            output_form,
            message,
        } => ResolvedAction::Replace {
            with: with.clone(),
            output_form: output_form.unwrap_or(OutputForm::Preserve),
            message: message.clone().unwrap_or_default(),
        },
        RawAction::Keep {
            output_form,
            message,
        } => ResolvedAction::Keep {
            output_form: output_form.unwrap_or(OutputForm::Preserve),
            message: message.clone().unwrap_or_default(),
        },
        RawAction::Remove {
            keep_blank_line,
            keep_trailing_comment,
            message,
        } => ResolvedAction::Remove {
            keep_blank_line: keep_blank_line.unwrap_or(false),
            keep_trailing_comment: keep_trailing_comment.unwrap_or(true),
            message: message.clone().unwrap_or_default(),
        },
        RawAction::CommentOut { style, message } => ResolvedAction::CommentOut {
            style: style.unwrap_or(CommentStyle::Line),
            message: message.clone().unwrap_or_default(),
        },
        RawAction::Error { message } => ResolvedAction::Error {
            message: message.clone().unwrap_or_default(),
        },
    }
}

fn trailing_from(raw: &RawTrailingComment) -> ResolvedTrailingComment {
    ResolvedTrailingComment {
        transform: raw.transform.as_ref().map(transform_from),
        append_if_absent: raw.append_if_absent.clone(),
    }
}

fn transform_from(raw: &RawTrailingTransform) -> ResolvedTrailingTransform {
    ResolvedTrailingTransform {
        match_styles: raw
            .match_styles
            .clone()
            .unwrap_or_else(|| vec![CommentStyle::Line, CommentStyle::Block]),
        content_regex: raw.content_regex.clone().unwrap_or_else(|| ".*".to_string()),
        action: raw
            .action
            .as_ref()
            .map(transform_action_from)
            .unwrap_or(ResolvedTrailingAction::Keep {
                output_style: OutputCommentStyle::Preserve,
                message: String::new(),
            }),
    }
}

fn transform_action_from(raw: &RawTrailingAction) -> ResolvedTrailingAction {
    match raw {
        RawTrailingAction::Replace {
            with,
            output_style,
            message,
        } => ResolvedTrailingAction::Replace {
            with: with.clone(),
            output_style: output_style.unwrap_or(OutputCommentStyle::Preserve),
            message: message.clone().unwrap_or_default(),
        },
        RawTrailingAction::Keep {
            output_style,
            message,
        } => ResolvedTrailingAction::Keep {
            output_style: output_style.unwrap_or(OutputCommentStyle::Preserve),
            message: message.clone().unwrap_or_default(),
        },
        RawTrailingAction::Remove { message } => ResolvedTrailingAction::Remove {
            message: message.clone().unwrap_or_default(),
        },
        RawTrailingAction::Error { message } => ResolvedTrailingAction::Error {
            message: message.clone().unwrap_or_default(),
        },
    }
}
