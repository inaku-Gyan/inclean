//! Five-layer matching engine.
//!
//! Trial order for an include in a given file:
//!
//! 1. Eligible configs are the ones whose directory is an ancestor of (or
//!    equal to) the file's directory. A sub-config in `src/foo/` is **not**
//!    consulted for files outside `src/foo/`.
//! 2. Eligible configs are tried deepest-first ("closest to the file").
//! 3. Within each config, rules are tried in declaration order.
//! 4. For each rule, all five layers must match for it to fire.
//!
//! Once a rule matches it is returned; later rules are not tried
//! (first-match-wins).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;

use super::glob::PathMatcher;
use crate::config::inherit::ResolvedRule;
use crate::config::schema::IncludeForm;
use crate::lex::include_line::Include;

/// A rule with its layer-1+2 matcher and layer-4 regex pre-compiled, plus
/// the rule's config directory normalized to a project-root-relative path
/// for scope checks.
#[derive(Debug)]
pub struct CompiledRule<'a> {
    pub rule: &'a ResolvedRule,
    pub path_matcher: PathMatcher,
    pub regex: Regex,
    pub config_dir_relpath: PathBuf,
}

impl<'a> CompiledRule<'a> {
    pub fn new(rule: &'a ResolvedRule, project_root: &Path) -> Result<Self> {
        let path_matcher = PathMatcher::build(&rule.paths, &rule.extensions)
            .with_context(|| format!("rule `{}`: layer 1/2 glob compile", rule.name))?;
        let regex = Regex::new(&rule.match_regex)
            .with_context(|| format!("rule `{}`: layer 4 regex compile", rule.name))?;
        let config_dir_relpath = strip_prefix_lossy(&rule.origin.config_dir, project_root);
        Ok(CompiledRule {
            rule,
            path_matcher,
            regex,
            config_dir_relpath,
        })
    }
}

/// Returns the path relative to `base` if `path` is under `base`; otherwise
/// the original `path`. Best-effort, used only for ancestry comparison.
fn strip_prefix_lossy(path: &Path, base: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| path.to_path_buf())
}

/// The outcome of a successful match.
#[derive(Debug)]
pub struct Match<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub captures: Vec<String>,
}

/// Find the first rule matching `include` within `file_relpath`.
///
/// `file_relpath` must be relative to the project root.
pub fn find_match<'a>(
    rules: &'a [CompiledRule<'a>],
    file_relpath: &Path,
    include: &Include,
) -> Option<Match<'a>> {
    for r in ordered_eligible(rules, file_relpath) {
        if let Some(captures) = try_match(r, file_relpath, include) {
            return Some(Match {
                rule: r,
                captures,
            });
        }
    }
    None
}

/// Return the rules in trial order for `file_relpath`: configs that are
/// ancestors of the file's directory, deepest first; within each config,
/// rules in declaration order. Stable for ties.
pub fn ordered_eligible<'a, 'b>(
    rules: &'a [CompiledRule<'b>],
    file_relpath: &Path,
) -> Vec<&'a CompiledRule<'b>> {
    let file_dir = file_relpath.parent().unwrap_or_else(|| Path::new(""));
    let mut eligible: Vec<&CompiledRule<'b>> = rules
        .iter()
        .filter(|r| is_ancestor_or_self(&r.config_dir_relpath, file_dir))
        .collect();
    eligible.sort_by(|a, b| {
        // Deeper config_dir wins (more components first).
        let depth = |p: &Path| p.components().count();
        depth(&b.config_dir_relpath)
            .cmp(&depth(&a.config_dir_relpath))
            // Within same config_dir: preserve declaration order.
            .then(a.rule.origin.index.cmp(&b.rule.origin.index))
    });
    eligible
}

/// `dir` is an ancestor of (or equal to) `descendant`?
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

fn try_match(
    r: &CompiledRule<'_>,
    file_relpath: &Path,
    include: &Include,
) -> Option<Vec<String>> {
    // Layer 1 + 2.
    if !r.path_matcher.matches(file_relpath) {
        return None;
    }
    // Layer 3: form membership.
    if !r.rule.forms.iter().any(|f| *f == include.form) {
        return None;
    }
    // Layer 4: regex on stripped include content.
    let caps = r.regex.captures(&include.content)?;
    // Materialize captures (group 0 = full, groups 1.. = user groups).
    let captures: Vec<String> = caps
        .iter()
        .map(|opt| opt.map(|m| m.as_str().to_string()).unwrap_or_default())
        .collect();
    Some(captures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::inherit::{resolve, ResolvedRule};
    use crate::config::schema::{parse, IncludeForm, LoadedConfig};
    use crate::lex::include_line::Include;

    fn cfg_at(path: &str, body: &str) -> LoadedConfig {
        LoadedConfig {
            path: PathBuf::from(path),
            raw: parse(body, &PathBuf::from(path)).unwrap(),
        }
    }

    fn quote_inc(content: &str) -> Include {
        Include {
            form: IncludeForm::Quote,
            content: content.to_string(),
            line: 1,
            argument_range: 0..0,
        }
    }

    fn angle_inc(content: &str) -> Include {
        Include {
            form: IncludeForm::Angle,
            content: content.to_string(),
            line: 1,
            argument_range: 0..0,
        }
    }

    fn compile<'a>(rules: &'a std::collections::BTreeMap<String, ResolvedRule>) -> Vec<CompiledRule<'a>> {
        let root = PathBuf::from("/proj");
        rules
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect()
    }

    #[test]
    fn simple_match_succeeds() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^foo\.h$'
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        let m = find_match(&compiled, Path::new("src/main.c"), &quote_inc("foo.h"));
        assert!(m.is_some());
        assert_eq!(m.unwrap().rule.rule.name, "base");
    }

    #[test]
    fn layer_1_paths_blocks_match() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        // file outside src/
        assert!(find_match(&compiled, Path::new("lib/main.c"), &quote_inc("x.h")).is_none());
    }

    #[test]
    fn layer_3_forms_blocks_match() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        assert!(find_match(&compiled, Path::new("src/main.c"), &angle_inc("foo.h")).is_none());
    }

    #[test]
    fn layer_4_match_regex_blocks() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^old_.*$'
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        assert!(find_match(&compiled, Path::new("src/main.c"), &quote_inc("foo.h")).is_none());
        assert!(find_match(&compiled, Path::new("src/main.c"), &quote_inc("old_foo.h")).is_some());
    }

    #[test]
    fn captures_are_materialized() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "base"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^old_(.+)$'
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        let m = find_match(&compiled, Path::new("src/main.c"), &quote_inc("old_foo.h")).unwrap();
        assert_eq!(m.captures.len(), 2);
        assert_eq!(m.captures[0], "old_foo.h");
        assert_eq!(m.captures[1], "foo.h");
    }

    #[test]
    fn first_match_wins_within_a_config() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "specific"
            paths = ["src/**"]
            forms = ["quote"]
            match = '^old_(.+)$'

            [[rule]]
            name = "fallback"
            paths = ["src/**"]
            forms = ["quote"]
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        let m = find_match(&compiled, Path::new("src/main.c"), &quote_inc("old_x.h")).unwrap();
        assert_eq!(m.rule.rule.name, "specific");
    }

    #[test]
    fn deeper_config_is_tried_first() {
        // Sub-config in src/foo wins for files under src/foo, even though
        // both rules match (sub-config rule comes first because it's deeper).
        let configs = vec![
            cfg_at(
                "/proj/inclean.toml",
                r#"
                [[rule]]
                name = "root-rule"
                paths = ["**"]
                forms = ["quote"]
                "#,
            ),
            cfg_at(
                "/proj/src/foo/inclean.toml",
                r#"
                [[rule]]
                name = "deep-rule"
                paths = ["**"]
                forms = ["quote"]
                "#,
            ),
        ];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        let m = find_match(&compiled, Path::new("src/foo/main.c"), &quote_inc("x.h")).unwrap();
        assert_eq!(m.rule.rule.name, "deep-rule");
    }

    #[test]
    fn deeper_config_does_not_apply_outside_its_subtree() {
        // The sub-config rule shouldn't fire for files outside its directory
        // tree, even if its paths glob would have matched.
        let configs = vec![
            cfg_at(
                "/proj/inclean.toml",
                r#"
                [[rule]]
                name = "root-rule"
                paths = ["**"]
                forms = ["quote"]
                "#,
            ),
            cfg_at(
                "/proj/src/foo/inclean.toml",
                r#"
                [[rule]]
                name = "deep-rule"
                paths = ["**"]
                forms = ["quote"]
                "#,
            ),
        ];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        // file outside src/foo/: only root-rule should apply
        let m = find_match(&compiled, Path::new("lib/baz.c"), &quote_inc("x.h")).unwrap();
        assert_eq!(m.rule.rule.name, "root-rule");
    }

    #[test]
    fn no_eligible_rule_returns_none() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "only-cpp"
            paths = ["src/**"]
            forms = ["angle"]
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        assert!(find_match(&compiled, Path::new("src/main.c"), &quote_inc("x.h")).is_none());
    }

    #[test]
    fn invalid_regex_at_compile_time_is_an_error() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "bad-regex"
            match = '[unclosed'
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let root = PathBuf::from("/proj");
        let err = CompiledRule::new(rules.values().next().unwrap(), &root).unwrap_err();
        assert!(format!("{err:#}").contains("layer 4"));
    }

    #[test]
    fn is_ancestor_handles_root_and_descendants() {
        assert!(is_ancestor_or_self(Path::new(""), Path::new("a/b")));
        assert!(is_ancestor_or_self(Path::new("a"), Path::new("a")));
        assert!(is_ancestor_or_self(Path::new("a"), Path::new("a/b")));
        assert!(!is_ancestor_or_self(Path::new("a"), Path::new("b/a")));
        assert!(!is_ancestor_or_self(Path::new("a/b"), Path::new("a")));
    }
}
