//! ⚠️ M1 transitional stub.
//!
//! The action evaluator is being rewritten in M4 with the new 6-variant
//! action surface (resolve / replace / keep / remove / comment_out /
//! error) plus the new trailing_comment.{transform, append_if_absent}
//! shape. For now this module exposes just enough surface for pipeline
//! to compile.

use std::ops::Range;
use std::path::Path;

use anyhow::{bail, Result};

use super::engine::Match;
use crate::lex::include_line::Include;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rewrite {
        edit_range: Range<usize>,
        new_text: String,
    },
    Keep,
    Error {
        message: String,
    },
}

pub fn evaluate(
    matched: &Match<'_>,
    _include: &Include,
    _source: &str,
    _file_relpath: &Path,
    _project_root: &Path,
) -> Result<Outcome> {
    bail!(
        "action evaluation is disabled during the v0.3 refactor transition (M1\u{2013}M3); rule `{}`",
        matched.rule.rule.name,
    );
}
