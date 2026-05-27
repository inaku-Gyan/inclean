//! Four-layer matching engine.
//!
//! Layers (all must pass for the rule to fire):
//! 1. `file_paths` glob + `file_suffixes` literal extension — handled by
//!    [`PathMatcher`] (see [`crate::rule::glob`]).
//! 2. *(Folded into layer 1 — the same matcher checks both.)*
//! 3. `match_forms` — `include.form` must be in the set.
//! 4. `include_match` — at least one glob must match the stripped include
//!    text (`include.content`).
//!
//! `suppression_comments_regex` filters includes out *before* layer
//! checks: if the include's `#`-line falls inside a per-rule "off-limits"
//! region (a `line`-regex match, or between a `block_start`/`block_end`
//! pair), the rule does not fire on that include.
//!
//! Unlike the old v0.2 engine there is no chain check and no "deepest
//! wins" selection — every matched rule is returned. Conflict detection
//! happens later by comparing the final-line text each rule would
//! produce (see `pipeline::run`).
//!
//! Macro-form includes are matched normally; the rule's action always
//! produces an error in [`crate::rule::action::evaluate`] (v1 cannot
//! resolve macro arguments).

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;

use super::glob::PathMatcher;
use crate::config::copy::{ResolvedRule, ResolvedSuppression};
use crate::lex::include_line::Include;

/// A rule with all of its matchers pre-compiled.
#[derive(Debug)]
pub struct CompiledRule<'a> {
    pub rule: &'a ResolvedRule,
    pub path_matcher: PathMatcher,
    pub include_matcher: GlobSet,
    pub suppression: CompiledSuppression,
    pub trailing_content_regex: Option<Regex>,
}

#[derive(Debug, Default)]
pub struct CompiledSuppression {
    pub block_start: Option<Regex>,
    pub block_end: Option<Regex>,
    pub line: Option<Regex>,
}

impl CompiledSuppression {
    pub fn is_empty(&self) -> bool {
        self.block_start.is_none() && self.block_end.is_none() && self.line.is_none()
    }
}

impl<'a> CompiledRule<'a> {
    pub fn new(rule: &'a ResolvedRule) -> Result<Self> {
        let path_matcher = PathMatcher::build(&rule.file_paths, &rule.file_suffixes)
            .with_context(|| format!("rule `{}`: file_paths/file_suffixes compile", rule.name))?;

        let mut gsb = GlobSetBuilder::new();
        for p in &rule.include_match {
            let g = build_include_glob(p)
                .with_context(|| format!("rule `{}`: include_match glob `{}`", rule.name, p))?;
            gsb.add(g);
        }
        let include_matcher = gsb
            .build()
            .with_context(|| format!("rule `{}`: include_match GlobSet build", rule.name))?;

        let suppression = compile_suppression(&rule.suppression, &rule.name)?;
        let trailing_content_regex = match &rule.trailing_comment.transform {
            Some(t) => Some(Regex::new(&t.content_regex).with_context(|| {
                format!(
                    "rule `{}`: trailing_comment.transform.content_regex compile",
                    rule.name
                )
            })?),
            None => None,
        };

        Ok(CompiledRule {
            rule,
            path_matcher,
            include_matcher,
            suppression,
            trailing_content_regex,
        })
    }
}

fn build_include_glob(pattern: &str) -> Result<Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("invalid include_match glob `{pattern}`"))
}

fn compile_suppression(raw: &ResolvedSuppression, rule_name: &str) -> Result<CompiledSuppression> {
    Ok(CompiledSuppression {
        block_start: compile_opt(raw.block_start.as_deref(), rule_name, "block_start")?,
        block_end: compile_opt(raw.block_end.as_deref(), rule_name, "block_end")?,
        line: compile_opt(raw.line.as_deref(), rule_name, "line")?,
    })
}

fn compile_opt(s: Option<&str>, rule_name: &str, field: &str) -> Result<Option<Regex>> {
    match s {
        Some(p) => Ok(Some(Regex::new(p).with_context(|| {
            format!("rule `{rule_name}`: suppression_comments_regex.{field} compile")
        })?)),
        None => Ok(None),
    }
}

/// Walk the file once per rule and produce the set of 1-based line
/// numbers that are off-limits for that rule.
///
/// Per refactor.md §"Config File":
///   "匹配时按行匹配，不区分 // 和 /* */ 注释。匹配时，会把该行首尾的空白去
///    掉再进行正则表达式匹配"
///
/// We interpret this as: for each line, extract its comment body (if the
/// line is wholly a `//` comment or wholly a same-line `/* ... */`
/// comment), trim, and feed *that* to the regex. Non-comment lines are
/// matched against the trimmed raw text too (so suppression can also
/// flag plain markers without requiring the user to escape `//`).
pub fn compute_suppressed_lines(
    rule: &CompiledRule<'_>,
    src: &str,
    line_table: &[Range<usize>],
) -> HashSet<usize> {
    let mut suppressed: HashSet<usize> = HashSet::new();
    if rule.suppression.is_empty() {
        return suppressed;
    }
    let mut in_block = false;
    for (idx, line_range) in line_table.iter().enumerate() {
        let line_text = &src[line_range.clone()];
        let probe = comment_body_or_raw(line_text);
        let lineno = idx + 1;

        // The `line` regex always wins, regardless of block state.
        if let Some(re) = &rule.suppression.line
            && re.is_match(probe)
        {
            suppressed.insert(lineno);
            continue;
        }

        if in_block {
            suppressed.insert(lineno);
            if let Some(re) = &rule.suppression.block_end
                && re.is_match(probe)
            {
                in_block = false;
            }
            continue;
        }

        if let Some(re) = &rule.suppression.block_start
            && re.is_match(probe)
        {
            in_block = true;
            suppressed.insert(lineno);
        }
    }
    suppressed
}

/// Extract a comment body for suppression matching:
/// - Line is `// ...`        → return `...` (delimiter stripped, trimmed).
/// - Line is `/* ... */`     → return `...` (delimiters stripped, trimmed).
/// - Otherwise               → return the trimmed line as-is.
fn comment_body_or_raw(line: &str) -> &str {
    let s = line.trim();
    if let Some(rest) = s.strip_prefix("//") {
        return rest.trim();
    }
    if let Some(rest) = s.strip_prefix("/*") {
        return rest.trim_end_matches("*/").trim();
    }
    s
}

/// Pre-compute suppression sets for every rule against a single file.
pub fn compute_all_suppressed(
    rules: &[CompiledRule<'_>],
    src: &str,
    line_table: &[Range<usize>],
) -> BTreeMap<String, HashSet<usize>> {
    rules
        .iter()
        .map(|r| {
            (
                r.rule.name.clone(),
                compute_suppressed_lines(r, src, line_table),
            )
        })
        .collect()
}

#[derive(Debug)]
pub struct CandidateMatch<'a> {
    pub rule: &'a CompiledRule<'a>,
}

#[derive(Debug, Default)]
pub struct MatchAllOutcome<'a> {
    pub matched: Vec<CandidateMatch<'a>>,
}

/// Run all four layers + suppression for every rule. Returns every rule
/// that matched, in declaration order. Conflict detection (final-text
/// equality across all matches) is the pipeline's job.
pub fn match_all<'a>(
    rules: &'a [CompiledRule<'a>],
    file_relpath: &Path,
    include: &Include,
    suppressed_per_rule: &BTreeMap<String, HashSet<usize>>,
) -> MatchAllOutcome<'a> {
    let mut out = MatchAllOutcome::default();
    for r in rules {
        // Layer 1 + 2.
        if !r.path_matcher.matches(file_relpath) {
            continue;
        }
        // Suppression (per rule).
        if let Some(set) = suppressed_per_rule.get(&r.rule.name)
            && set.contains(&include.line)
        {
            continue;
        }
        // Layer 3.
        if !r.rule.match_forms.contains(&include.form) {
            continue;
        }
        // Layer 4.
        if !r.include_matcher.is_match(&include.content) {
            continue;
        }
        out.matched.push(CandidateMatch { rule: r });
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

    fn inc(form: IncludeForm, content: &str, line: usize) -> Include {
        Include {
            form,
            content: content.to_string(),
            line,
            argument_range: 0..0,
            trailing_range: 0..0,
            trailing_comment_style: None,
            has_cross_line_block_trailing: false,
        }
    }

    #[test]
    fn quote_form_matches_default_rule() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            "#,
        );
        let sup = BTreeMap::new();
        let out = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "foo.h", 1),
            &sup,
        );
        assert_eq!(out.matched.len(), 1);
    }

    #[test]
    fn angle_form_does_not_match_default_quote_only_rule() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            "#,
        );
        let sup = BTreeMap::new();
        let out = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Angle, "stdio.h", 1),
            &sup,
        );
        assert!(out.matched.is_empty());
    }

    #[test]
    fn include_match_glob_filters() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "old-only"
            include_match = ["old_*.h"]
            "#,
        );
        let sup = BTreeMap::new();
        let out_old = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "old_foo.h", 1),
            &sup,
        );
        let out_new = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "foo.h", 1),
            &sup,
        );
        assert_eq!(out_old.matched.len(), 1);
        assert!(out_new.matched.is_empty());
    }

    #[test]
    fn file_paths_glob_filters() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "src-only"
            file_paths = ["src/**/*"]
            "#,
        );
        let sup = BTreeMap::new();
        let in_src = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "x.h", 1),
            &sup,
        );
        let outside_src = match_all(
            &rules,
            Path::new("lib/main.c"),
            &inc(IncludeForm::Quote, "x.h", 1),
            &sup,
        );
        assert_eq!(in_src.matched.len(), 1);
        assert!(outside_src.matched.is_empty());
    }

    #[test]
    fn file_suffixes_filters_when_glob_has_wildcards() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "c-only"
            file_paths = ["**/*"]
            file_suffixes = [".c"]
            "#,
        );
        let sup = BTreeMap::new();
        let c_file = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "x.h", 1),
            &sup,
        );
        let cpp_file = match_all(
            &rules,
            Path::new("src/main.cpp"),
            &inc(IncludeForm::Quote, "x.h", 1),
            &sup,
        );
        assert_eq!(c_file.matched.len(), 1);
        assert!(cpp_file.matched.is_empty());
    }

    #[test]
    fn multiple_rules_all_match_return() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "a"
            include_match = ["foo.h"]

            [[rule]]
            name = "b"
            include_match = ["**/foo.h", "foo.h"]
            "#,
        );
        let sup = BTreeMap::new();
        let out = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "foo.h", 1),
            &sup,
        );
        assert_eq!(out.matched.len(), 2);
    }

    #[test]
    fn suppressed_line_skips_rule() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            suppression_comments_regex = { line = "^inclean: skip$" }
            "#,
        );
        let src = "// inclean: skip\n#include \"foo.h\"\nfoo;\n";
        let line_table = crate::lex::include_line::line_table(src);
        let sup = compute_all_suppressed(&rules, src, &line_table);
        // line 1 is suppressed; the #include on line 2 isn't, but the
        // suppression-line regex specifically matches the *comment*, not
        // the include line. This test confirms the suppression is per-line:
        // include on line 2 still matches.
        let out = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "foo.h", 2),
            &sup,
        );
        assert_eq!(out.matched.len(), 1);
        assert!(sup.get("base").unwrap().contains(&1));
    }

    #[test]
    fn block_suppression_covers_inner_lines() {
        let rules = compile_rules(
            r#"
            [[rule]]
            name = "base"
            suppression_comments_regex = {
                block_start = "^USER CODE BEGIN.*$",
                block_end = "^USER CODE END.*$",
            }
            "#,
        );
        let src = "// USER CODE BEGIN here\n#include \"foo.h\"\n// USER CODE END here\n#include \"bar.h\"\n";
        let line_table = crate::lex::include_line::line_table(src);
        let sup = compute_all_suppressed(&rules, src, &line_table);
        // The block covers lines 1–3.
        let s = sup.get("base").unwrap();
        assert!(s.contains(&1));
        assert!(s.contains(&2));
        assert!(s.contains(&3));
        assert!(!s.contains(&4));

        let inside = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "foo.h", 2),
            &sup,
        );
        let outside = match_all(
            &rules,
            Path::new("src/main.c"),
            &inc(IncludeForm::Quote, "bar.h", 4),
            &sup,
        );
        assert!(inside.matched.is_empty());
        assert_eq!(outside.matched.len(), 1);
    }
}
