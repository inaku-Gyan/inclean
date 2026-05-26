//! ⚠️ M1 transitional stub.
//!
//! The five-layer engine is being replaced by the four-layer engine in M4.
//! For now this module exposes the public types and function signatures
//! that the rest of the crate (pipeline, action) still imports, but every
//! body is a stub that either returns "no match" or an empty result. The
//! real implementation lands in M4 (`pipeline / four-layer + include_match
//! glob + suppression regions + conflict-by-final-text`).

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::glob::PathMatcher;
use crate::config::copy::ResolvedRule;
use crate::lex::include_line::Include;

#[derive(Debug)]
pub struct CompiledRule<'a> {
    pub rule: &'a ResolvedRule,
    pub path_matcher: PathMatcher,
    pub config_dir_relpath: PathBuf,
}

impl<'a> CompiledRule<'a> {
    pub fn new(rule: &'a ResolvedRule, project_root: &Path) -> Result<Self> {
        let path_matcher = PathMatcher::build(&rule.file_paths, &rule.file_suffixes)?;
        let config_dir_relpath = rule
            .origin
            .config_dir
            .strip_prefix(project_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        Ok(CompiledRule {
            rule,
            path_matcher,
            config_dir_relpath,
        })
    }
}

#[derive(Debug)]
pub struct Match<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub captures: Vec<String>,
    pub resolved: Option<PathBuf>,
}

#[derive(Debug)]
pub struct CandidateMatch<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub captures: Vec<String>,
    pub resolved: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Layer5Ambiguity<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub candidates: Vec<PathBuf>,
}

#[derive(Debug, Default)]
pub struct MatchAllOutcome<'a> {
    pub matched: Vec<CandidateMatch<'a>>,
    pub ambiguities: Vec<Layer5Ambiguity<'a>>,
}

#[derive(Debug)]
pub struct RuleTrial<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub eligible: bool,
    pub matched_overall: bool,
}

#[derive(Debug, Clone)]
pub struct LayerTrace {
    pub passed: bool,
    pub detail: String,
}

pub fn find_match<'a>(
    _rules: &'a [CompiledRule<'a>],
    _file_relpath: &Path,
    _include: &Include,
    _project_root: &Path,
) -> Option<Match<'a>> {
    None
}

pub fn match_all<'a>(
    _rules: &'a [CompiledRule<'a>],
    _file_relpath: &Path,
    _include: &Include,
    _project_root: &Path,
) -> MatchAllOutcome<'a> {
    MatchAllOutcome::default()
}

pub fn ordered_eligible<'a, 'b>(
    rules: &'a [CompiledRule<'b>],
    _file_relpath: &Path,
) -> Vec<&'a CompiledRule<'b>> {
    rules.iter().collect()
}

pub fn trace_match<'a>(
    _rules: &'a [CompiledRule<'a>],
    _file_relpath: &Path,
    _include: &Include,
    _project_root: &Path,
) -> Vec<RuleTrial<'a>> {
    Vec::new()
}
