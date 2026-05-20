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
//! Layer 5 (`match_resolved`) resolves the include against the rule's
//! `original_include_dirs`. Two or more dirs containing the same include
//! is an ambiguity — surfaced separately so the user knows to narrow the
//! `-I` list. Rules without `match_resolved` skip the resolve step
//! entirely.
//!
//! Once a rule matches it is returned; later rules are not tried
//! (first-match-wins).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;

use super::glob::PathMatcher;
use crate::config::inherit::ResolvedRule;
use crate::index::header_index::{self, UniqueResolution};
use crate::lex::include_line::Include;

/// A rule with its layer-1+2 matcher and layer-4 regex pre-compiled, plus
/// the rule's config directory normalized to a project-root-relative path
/// for scope checks.
#[derive(Debug)]
pub struct CompiledRule<'a> {
    pub rule: &'a ResolvedRule,
    pub path_matcher: PathMatcher,
    pub regex: Regex,
    /// Layer-5 path regex, pre-compiled. `None` if `match_resolved` is
    /// unset or only specifies `under`.
    pub resolved_regex: Option<Regex>,
    pub config_dir_relpath: PathBuf,
}

impl<'a> CompiledRule<'a> {
    pub fn new(rule: &'a ResolvedRule, project_root: &Path) -> Result<Self> {
        let path_matcher = PathMatcher::build(&rule.paths, &rule.extensions)
            .with_context(|| format!("rule `{}`: layer 1/2 glob compile", rule.name))?;
        let regex = Regex::new(&rule.match_regex)
            .with_context(|| format!("rule `{}`: layer 4 regex compile", rule.name))?;
        let resolved_regex = match rule
            .match_resolved
            .as_ref()
            .and_then(|m| m.path_regex.as_ref())
        {
            Some(s) => Some(
                Regex::new(s)
                    .with_context(|| format!("rule `{}`: layer 5 regex compile", rule.name))?,
            ),
            None => None,
        };
        let config_dir_relpath = strip_prefix_lossy(&rule.origin.config_dir, project_root);
        Ok(CompiledRule {
            rule,
            path_matcher,
            regex,
            resolved_regex,
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
    /// Project-root-relative path of the file the include resolved to,
    /// populated only when layer 5 ran (i.e. the rule has
    /// `match_resolved`). Used by the `auto` action and `${resolved.*}`
    /// placeholders.
    pub resolved: Option<PathBuf>,
}

/// One candidate rule that passed all five layers for an include.
#[derive(Debug)]
pub struct CandidateMatch<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub captures: Vec<String>,
    pub resolved: Option<PathBuf>,
}

/// A rule whose layer-5 hit an ambiguous resolution. Not a candidate match;
/// the user is expected to narrow `original_include_dirs`.
#[derive(Debug)]
pub struct Layer5Ambiguity<'a> {
    pub rule: &'a CompiledRule<'a>,
    pub candidates: Vec<PathBuf>,
}

/// What [`match_all`] produces for a single include.
#[derive(Debug, Default)]
pub struct MatchAllOutcome<'a> {
    pub matched: Vec<CandidateMatch<'a>>,
    pub ambiguities: Vec<Layer5Ambiguity<'a>>,
}

/// Per-rule trial record used by `inclean explain`. Captures the layer
/// outcomes that find_match would have evaluated.
#[derive(Debug)]
pub struct RuleTrial<'a> {
    pub rule: &'a CompiledRule<'a>,
    /// Whether the rule's config directory is an ancestor of (or equal to)
    /// the file's directory. Non-eligible rules are reported but get no
    /// per-layer detail.
    pub eligible: bool,
    pub layer1_paths: Option<LayerTrace>,
    pub layer2_extensions: Option<LayerTrace>,
    pub layer3_forms: Option<LayerTrace>,
    pub layer4_match: Option<LayerTrace>,
    pub layer5_resolved: Option<LayerTrace>,
    pub captures: Option<Vec<String>>,
    pub resolved: Option<PathBuf>,
    pub matched_overall: bool,
}

#[derive(Debug, Clone)]
pub struct LayerTrace {
    pub passed: bool,
    pub detail: String,
}

/// Possible outcomes for evaluating one rule against one include.
enum RuleEval {
    Matched {
        captures: Vec<String>,
        resolved: Option<PathBuf>,
    },
    NotMatched,
    Ambiguous(Vec<PathBuf>),
}

/// Find the first rule matching `include` within `file_relpath`. Layer-5
/// ambiguities are silently skipped — callers that need to surface them
/// should use [`match_all`].
///
/// `file_relpath` must be relative to the project root.
pub fn find_match<'a>(
    rules: &'a [CompiledRule<'a>],
    file_relpath: &Path,
    include: &Include,
    project_root: &Path,
) -> Option<Match<'a>> {
    for r in ordered_eligible(rules, file_relpath) {
        match evaluate_rule(r, file_relpath, include, project_root) {
            RuleEval::Matched { captures, resolved } => {
                return Some(Match {
                    rule: r,
                    captures,
                    resolved,
                });
            }
            RuleEval::NotMatched | RuleEval::Ambiguous(_) => continue,
        }
    }
    None
}

/// All rules whose five layers match `include` within `file_relpath`, in
/// trial order. Unlike [`find_match`], this does not short-circuit — it
/// returns every candidate so callers can audit rule-tree invariants
/// (child ⊆ parent, cross-chain disjoint) over the project's actual
/// sources. Layer-5 ambiguities are reported separately.
pub fn match_all<'a>(
    rules: &'a [CompiledRule<'a>],
    file_relpath: &Path,
    include: &Include,
    project_root: &Path,
) -> MatchAllOutcome<'a> {
    let mut out = MatchAllOutcome::default();
    for r in ordered_eligible(rules, file_relpath) {
        match evaluate_rule(r, file_relpath, include, project_root) {
            RuleEval::Matched { captures, resolved } => {
                out.matched.push(CandidateMatch {
                    rule: r,
                    captures,
                    resolved,
                });
            }
            RuleEval::Ambiguous(candidates) => {
                out.ambiguities.push(Layer5Ambiguity {
                    rule: r,
                    candidates,
                });
            }
            RuleEval::NotMatched => {}
        }
    }
    out
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

/// Like `find_match`, but returns the per-rule trial trace used by
/// `inclean explain`. Iteration stops at the first matched rule so the
/// trace mirrors first-match-wins semantics.
pub fn trace_match<'a>(
    rules: &'a [CompiledRule<'a>],
    file_relpath: &Path,
    include: &Include,
    project_root: &Path,
) -> Vec<RuleTrial<'a>> {
    let ordered = ordered_eligible(rules, file_relpath);
    let mut out = Vec::with_capacity(ordered.len());
    for r in ordered {
        let trial = trial_for(r, file_relpath, include, project_root);
        let matched = trial.matched_overall;
        out.push(trial);
        if matched {
            break;
        }
    }
    out
}

fn trial_for<'a>(
    r: &'a CompiledRule<'a>,
    file_relpath: &Path,
    include: &Include,
    project_root: &Path,
) -> RuleTrial<'a> {
    // Layer 1 + 2: PathMatcher decides them together; describe what we know.
    let layer1_passed = r.path_matcher.matches(file_relpath);
    let layer1 = LayerTrace {
        passed: layer1_passed,
        detail: if layer1_passed {
            format!("path globs matched `{}`", file_relpath.display())
        } else {
            format!(
                "no path glob matched `{}` (or layer 2 extension filter failed)",
                file_relpath.display()
            )
        },
    };
    // Layer 2 detail is folded into layer 1's "or extension filter failed".
    // We surface the extension list separately so users can see what would be allowed.
    let ext = file_relpath
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let layer2 = LayerTrace {
        passed: layer1_passed, // can't tell layer 2 alone from outside
        detail: format!(
            "file extension {:?}; rule allows {:?}",
            ext, r.rule.extensions
        ),
    };

    if !layer1_passed {
        return RuleTrial {
            rule: r,
            eligible: true,
            layer1_paths: Some(layer1),
            layer2_extensions: Some(layer2),
            layer3_forms: None,
            layer4_match: None,
            layer5_resolved: None,
            captures: None,
            resolved: None,
            matched_overall: false,
        };
    }

    // Layer 3
    let form_ok = r.rule.forms.contains(&include.form);
    let layer3 = LayerTrace {
        passed: form_ok,
        detail: format!(
            "include form {:?}; rule accepts {:?}",
            include.form, r.rule.forms
        ),
    };
    if !form_ok {
        return RuleTrial {
            rule: r,
            eligible: true,
            layer1_paths: Some(layer1),
            layer2_extensions: Some(layer2),
            layer3_forms: Some(layer3),
            layer4_match: None,
            layer5_resolved: None,
            captures: None,
            resolved: None,
            matched_overall: false,
        };
    }

    // Layer 4
    let caps = r.regex.captures(&include.content);
    let (layer4_passed, captures) = match &caps {
        Some(c) => {
            let v: Vec<String> = c
                .iter()
                .map(|m| m.map(|s| s.as_str().to_string()).unwrap_or_default())
                .collect();
            (true, Some(v))
        }
        None => (false, None),
    };
    let layer4 = LayerTrace {
        passed: layer4_passed,
        detail: format!(
            "regex `{}` {} `{}`",
            r.rule.match_regex,
            if layer4_passed {
                "matched"
            } else {
                "did not match"
            },
            include.content
        ),
    };
    if !layer4_passed {
        return RuleTrial {
            rule: r,
            eligible: true,
            layer1_paths: Some(layer1),
            layer2_extensions: Some(layer2),
            layer3_forms: Some(layer3),
            layer4_match: Some(layer4),
            layer5_resolved: None,
            captures,
            resolved: None,
            matched_overall: false,
        };
    }

    // Layer 5
    let (layer5_trace, resolved, layer5_passed) = match evaluate_layer5(r, include, project_root) {
        Layer5::Skipped => (None, None, true),
        Layer5::Matched(p) => (
            Some(LayerTrace {
                passed: true,
                detail: format!("resolved to `{}` and satisfied constraints", p.display()),
            }),
            Some(p),
            true,
        ),
        Layer5::FailedConstraint { resolved, why } => (
            Some(LayerTrace {
                passed: false,
                detail: format!("resolved to `{}` but {}", resolved.display(), why),
            }),
            Some(resolved),
            false,
        ),
        Layer5::Unresolved => (
            Some(LayerTrace {
                passed: false,
                detail: "include does not resolve under `original_include_dirs`".to_string(),
            }),
            None,
            false,
        ),
        Layer5::Ambiguous(candidates) => (
            Some(LayerTrace {
                passed: false,
                detail: format!("ambiguous: {} candidates", candidates.len()),
            }),
            None,
            false,
        ),
    };

    RuleTrial {
        rule: r,
        eligible: true,
        layer1_paths: Some(layer1),
        layer2_extensions: Some(layer2),
        layer3_forms: Some(layer3),
        layer4_match: Some(layer4),
        layer5_resolved: layer5_trace,
        captures,
        resolved,
        matched_overall: layer5_passed,
    }
}

/// Internal layer-5 disposition (richer than the RuleEval variants —
/// `trial_for` distinguishes "constraint failed" from "unresolved" so
/// `explain` can spell it out).
enum Layer5 {
    /// Rule has no `match_resolved` — layer 5 doesn't apply.
    Skipped,
    /// Resolved uniquely and satisfied constraints.
    Matched(PathBuf),
    /// Resolved uniquely but at least one constraint failed.
    FailedConstraint { resolved: PathBuf, why: String },
    /// No `original_include_dir` contains the include.
    Unresolved,
    /// More than one `original_include_dir` contains the include.
    Ambiguous(Vec<PathBuf>),
}

fn evaluate_rule(
    r: &CompiledRule<'_>,
    file_relpath: &Path,
    include: &Include,
    project_root: &Path,
) -> RuleEval {
    // Layer 1 + 2.
    if !r.path_matcher.matches(file_relpath) {
        return RuleEval::NotMatched;
    }
    // Layer 3.
    if !r.rule.forms.contains(&include.form) {
        return RuleEval::NotMatched;
    }
    // Layer 4.
    let caps = match r.regex.captures(&include.content) {
        Some(c) => c,
        None => return RuleEval::NotMatched,
    };
    let captures: Vec<String> = caps
        .iter()
        .map(|opt| opt.map(|m| m.as_str().to_string()).unwrap_or_default())
        .collect();

    // Layer 5.
    match evaluate_layer5(r, include, project_root) {
        Layer5::Skipped => RuleEval::Matched {
            captures,
            resolved: None,
        },
        Layer5::Matched(p) => RuleEval::Matched {
            captures,
            resolved: Some(p),
        },
        Layer5::FailedConstraint { .. } | Layer5::Unresolved => RuleEval::NotMatched,
        Layer5::Ambiguous(c) => RuleEval::Ambiguous(c),
    }
}

fn evaluate_layer5(r: &CompiledRule<'_>, include: &Include, project_root: &Path) -> Layer5 {
    let Some(spec) = r.rule.match_resolved.as_ref() else {
        return Layer5::Skipped;
    };
    let abs = match header_index::resolve_in_dirs_unique(
        project_root,
        &r.rule.original_include_dirs,
        &include.content,
    ) {
        UniqueResolution::None => return Layer5::Unresolved,
        UniqueResolution::Ambiguous(c) => return Layer5::Ambiguous(c),
        UniqueResolution::Unique(p) => p,
    };
    let relpath = abs
        .strip_prefix(project_root)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| abs.clone());
    let rel_str = relpath.to_string_lossy().replace('\\', "/");

    if let Some(under) = &spec.under {
        let norm_under = under.trim_end_matches('/');
        let starts = rel_str == norm_under || rel_str.starts_with(&format!("{norm_under}/"));
        if !starts {
            return Layer5::FailedConstraint {
                resolved: relpath,
                why: format!("not under `{under}`"),
            };
        }
    }
    if let Some(re) = r.resolved_regex.as_ref() {
        if !re.is_match(&rel_str) {
            return Layer5::FailedConstraint {
                resolved: relpath,
                why: format!("did not match `{}`", re.as_str()),
            };
        }
    }
    Layer5::Matched(relpath)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::inherit::{resolve, ResolvedRule};
    use crate::config::schema::{parse, IncludeForm, LoadedConfig};
    use crate::lex::include_line::Include;
    use std::fs;

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

    fn compile<'a>(
        rules: &'a std::collections::BTreeMap<String, ResolvedRule>,
    ) -> Vec<CompiledRule<'a>> {
        let root = PathBuf::from("/proj");
        rules
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect()
    }

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "inclean-engine-{}-{}",
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

    const DUMMY: &str = "/proj";

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
        let m = find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("foo.h"),
            Path::new(DUMMY),
        );
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
        assert!(find_match(
            &compiled,
            Path::new("lib/main.c"),
            &quote_inc("x.h"),
            Path::new(DUMMY)
        )
        .is_none());
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
        assert!(find_match(
            &compiled,
            Path::new("src/main.c"),
            &angle_inc("foo.h"),
            Path::new(DUMMY)
        )
        .is_none());
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
        assert!(find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("foo.h"),
            Path::new(DUMMY)
        )
        .is_none());
        assert!(find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("old_foo.h"),
            Path::new(DUMMY)
        )
        .is_some());
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
        let m = find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("old_foo.h"),
            Path::new(DUMMY),
        )
        .unwrap();
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
        let m = find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("old_x.h"),
            Path::new(DUMMY),
        )
        .unwrap();
        assert_eq!(m.rule.rule.name, "specific");
    }

    #[test]
    fn deeper_config_is_tried_first() {
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
        let m = find_match(
            &compiled,
            Path::new("src/foo/main.c"),
            &quote_inc("x.h"),
            Path::new(DUMMY),
        )
        .unwrap();
        assert_eq!(m.rule.rule.name, "deep-rule");
    }

    #[test]
    fn deeper_config_does_not_apply_outside_its_subtree() {
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
        let m = find_match(
            &compiled,
            Path::new("lib/baz.c"),
            &quote_inc("x.h"),
            Path::new(DUMMY),
        )
        .unwrap();
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
        assert!(find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("x.h"),
            Path::new(DUMMY)
        )
        .is_none());
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
    fn match_all_returns_every_passing_rule_in_trial_order() {
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
        let out = match_all(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("old_x.h"),
            Path::new(DUMMY),
        );
        let names: Vec<_> = out
            .matched
            .iter()
            .map(|c| c.rule.rule.name.as_str())
            .collect();
        assert_eq!(names, vec!["specific", "fallback"]);
        assert!(out.ambiguities.is_empty());
    }

    #[test]
    fn match_all_skips_rules_that_fail_any_layer() {
        let configs = vec![cfg_at(
            "/proj/inclean.toml",
            r#"
            [[rule]]
            name = "quote-only"
            paths = ["src/**"]
            forms = ["quote"]

            [[rule]]
            name = "angle-only"
            paths = ["src/**"]
            forms = ["angle"]
            "#,
        )];
        let rules = resolve(&configs).unwrap();
        let compiled = compile(&rules);
        let out = match_all(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("x.h"),
            Path::new(DUMMY),
        );
        let names: Vec<_> = out
            .matched
            .iter()
            .map(|c| c.rule.rule.name.as_str())
            .collect();
        assert_eq!(names, vec!["quote-only"]);
    }

    #[test]
    fn is_ancestor_handles_root_and_descendants() {
        assert!(is_ancestor_or_self(Path::new(""), Path::new("a/b")));
        assert!(is_ancestor_or_self(Path::new("a"), Path::new("a")));
        assert!(is_ancestor_or_self(Path::new("a"), Path::new("a/b")));
        assert!(!is_ancestor_or_self(Path::new("a"), Path::new("b/a")));
        assert!(!is_ancestor_or_self(Path::new("a/b"), Path::new("a")));
    }

    #[test]
    fn layer5_under_constraint_matches() {
        let root = tmp();
        touch(&root, "src/internal/foo.h");
        let body = r#"
            [[rule]]
            name = "internal-only"
            paths = ["src/**"]
            forms = ["quote"]
            original_include_dirs = ["src/internal"]
            match_resolved = { under = "src/internal" }
        "#;
        let cfg = LoadedConfig {
            path: root.join("inclean.toml"),
            raw: parse(body, &root.join("inclean.toml")).unwrap(),
        };
        let resolved = resolve(&[cfg]).unwrap();
        let compiled: Vec<CompiledRule<'_>> = resolved
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect();
        let m = find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("foo.h"),
            &root,
        )
        .unwrap();
        assert_eq!(m.rule.rule.name, "internal-only");
        assert_eq!(m.resolved.as_deref(), Some(Path::new("src/internal/foo.h")));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn layer5_under_constraint_blocks_when_path_does_not_start_with_dir() {
        let root = tmp();
        touch(&root, "src/external/foo.h");
        let body = r#"
            [[rule]]
            name = "internal-only"
            paths = ["src/**"]
            forms = ["quote"]
            original_include_dirs = ["src/external"]
            match_resolved = { under = "src/internal" }
        "#;
        let cfg = LoadedConfig {
            path: root.join("inclean.toml"),
            raw: parse(body, &root.join("inclean.toml")).unwrap(),
        };
        let resolved = resolve(&[cfg]).unwrap();
        let compiled: Vec<CompiledRule<'_>> = resolved
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect();
        let m = find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("foo.h"),
            &root,
        );
        assert!(m.is_none());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn layer5_path_regex_constraint() {
        let root = tmp();
        touch(&root, "src/internal/foo.h");
        let body = r#"
            [[rule]]
            name = "regex"
            paths = ["src/**"]
            forms = ["quote"]
            original_include_dirs = ["src/internal"]
            match_resolved = { match = '\.h$' }
        "#;
        let cfg = LoadedConfig {
            path: root.join("inclean.toml"),
            raw: parse(body, &root.join("inclean.toml")).unwrap(),
        };
        let resolved = resolve(&[cfg]).unwrap();
        let compiled: Vec<CompiledRule<'_>> = resolved
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect();
        assert!(find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("foo.h"),
            &root
        )
        .is_some());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn layer5_ambiguity_is_reported_via_match_all() {
        let root = tmp();
        touch(&root, "a/foo.h");
        touch(&root, "b/foo.h");
        let body = r#"
            [[rule]]
            name = "ambiguous"
            paths = ["src/**"]
            forms = ["quote"]
            original_include_dirs = ["a", "b"]
            match_resolved = { under = "a" }
        "#;
        let cfg = LoadedConfig {
            path: root.join("inclean.toml"),
            raw: parse(body, &root.join("inclean.toml")).unwrap(),
        };
        touch(&root, "src/main.c");
        let resolved = resolve(&[cfg]).unwrap();
        let compiled: Vec<CompiledRule<'_>> = resolved
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect();
        let out = match_all(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("foo.h"),
            &root,
        );
        assert!(out.matched.is_empty());
        assert_eq!(out.ambiguities.len(), 1);
        assert_eq!(out.ambiguities[0].rule.rule.name, "ambiguous");
        assert_eq!(out.ambiguities[0].candidates.len(), 2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn layer5_unresolved_does_not_match() {
        let root = tmp();
        let body = r#"
            [[rule]]
            name = "needs-resolve"
            paths = ["src/**"]
            forms = ["quote"]
            original_include_dirs = ["src/internal"]
            match_resolved = { under = "src/internal" }
        "#;
        let cfg = LoadedConfig {
            path: root.join("inclean.toml"),
            raw: parse(body, &root.join("inclean.toml")).unwrap(),
        };
        let resolved = resolve(&[cfg]).unwrap();
        let compiled: Vec<CompiledRule<'_>> = resolved
            .values()
            .map(|r| CompiledRule::new(r, &root).unwrap())
            .collect();
        assert!(find_match(
            &compiled,
            Path::new("src/main.c"),
            &quote_inc("missing.h"),
            &root
        )
        .is_none());
        fs::remove_dir_all(&root).ok();
    }
}
