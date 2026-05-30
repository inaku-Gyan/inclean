//! Layer 1 (`paths`) + layer 2 (`extensions`) file-level matcher.
//!
//! Path globs are interpreted with `globset` in **literal-separator** mode
//! (modern-tool semantics, not classic gitignore): `*` does NOT cross a
//! path separator; only `**` does. Users who want "match at any depth"
//! write `**/foo.c` rather than the gitignore `foo.c` shorthand.
//!
//! All paths in rules are **relative to the resolved project root**
//! (i.e. `<config_dir>/<[project].root>`), not to the config file's
//! directory.
//!
//! Layer-2 behavior: when the matching glob contains wildcard meta-chars
//! (`*`, `?`, `[`, `{`), the file's extension must appear in `extensions`
//! for the rule to match. When the glob is an exact path, layer 2 is
//! skipped (the exact path is its own constraint).

use std::path::Path;

use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobMatcher};

#[derive(Debug)]
pub struct PathMatcher {
    globs: Vec<CompiledPathGlob>,
    extensions: Vec<String>,
}

#[derive(Debug)]
pub struct OrderedGlobMatcher {
    globs: Vec<CompiledSignedGlob>,
}

#[derive(Debug)]
struct CompiledSignedGlob {
    matcher: GlobMatcher,
    /// Original pattern string. Kept for explain-mode output (M2) and tests.
    #[allow(dead_code)]
    pattern: String,
    is_negated: bool,
}

#[derive(Debug)]
struct CompiledPathGlob {
    signed: CompiledSignedGlob,
    has_wildcards: bool,
}

#[derive(Debug)]
struct ParsedSignedPattern {
    is_negated: bool,
    glob_pattern: String,
}

impl OrderedGlobMatcher {
    /// Compile a list of ordered signed globs. Later matching patterns override
    /// earlier ones; a negated pattern makes the final result false.
    pub fn build(patterns: &[String]) -> Result<Self> {
        let mut globs = Vec::with_capacity(patterns.len());
        for p in patterns {
            globs.push(compile_signed(p).with_context(|| format!("invalid glob `{p}`"))?);
        }
        Ok(Self { globs })
    }

    /// Does `path` satisfy this ordered signed glob list?
    pub fn is_match<P: AsRef<Path>>(&self, path: P) -> bool {
        let path = path.as_ref();
        let mut matched = false;
        for g in &self.globs {
            if g.matcher.is_match(path) {
                matched = !g.is_negated;
            }
        }
        matched
    }
}

impl PathMatcher {
    /// Compile a rule's layer-1 globs and layer-2 extension list.
    pub fn build(paths: &[String], extensions: &[String]) -> Result<Self> {
        let mut globs = Vec::with_capacity(paths.len());
        for p in paths {
            globs.push(compile(p)?);
        }
        Ok(PathMatcher {
            globs,
            extensions: extensions.to_vec(),
        })
    }

    /// Does `path` (relative to the project root) satisfy this rule's
    /// layer 1 + layer 2 match?
    pub fn matches(&self, path: &Path) -> bool {
        let mut matched = false;
        for g in &self.globs {
            if !g.signed.matcher.is_match(path) {
                continue;
            }
            if g.has_wildcards && !self.matches_extension(path) {
                continue;
            }
            matched = !g.signed.is_negated;
        }
        matched
    }

    fn matches_extension(&self, path: &Path) -> bool {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        // We compare with leading-dot form (".c") since users write
        // extensions that way. An empty extension means the file has none;
        // we still emit ".", but that won't match canonical lists.
        let dotted = if ext.is_empty() {
            String::new()
        } else {
            format!(".{ext}")
        };
        if dotted.is_empty() {
            return false;
        }
        self.extensions.iter().any(|e| e == &dotted)
    }
}

fn compile(pattern: &str) -> Result<CompiledPathGlob> {
    let parsed =
        parse_signed_pattern(pattern).with_context(|| format!("invalid path glob `{pattern}`"))?;
    let signed = compile_parsed_signed(pattern, &parsed)
        .with_context(|| format!("invalid path glob `{pattern}`"))?;
    Ok(CompiledPathGlob {
        signed,
        has_wildcards: parsed
            .glob_pattern
            .chars()
            .any(|c| matches!(c, '*' | '?' | '[' | '{')),
    })
}

fn compile_signed(pattern: &str) -> Result<CompiledSignedGlob> {
    let parsed = parse_signed_pattern(pattern)?;
    compile_parsed_signed(pattern, &parsed)
}

fn compile_parsed_signed(
    original_pattern: &str,
    parsed: &ParsedSignedPattern,
) -> Result<CompiledSignedGlob> {
    let glob = GlobBuilder::new(&parsed.glob_pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("glob pattern `{original_pattern}`"))?;
    Ok(CompiledSignedGlob {
        matcher: glob.compile_matcher(),
        pattern: original_pattern.to_string(),
        is_negated: parsed.is_negated,
    })
}

fn parse_signed_pattern(pattern: &str) -> Result<ParsedSignedPattern> {
    let (is_negated, body) = if let Some(body) = pattern.strip_prefix('!') {
        if body.is_empty() {
            bail!("negated glob `!` is missing a pattern");
        }
        (true, body)
    } else {
        (false, pattern)
    };

    let glob_pattern = match body.strip_prefix(r"\!") {
        Some(rest) => format!("!{rest}"),
        None => body.to_string(),
    };

    Ok(ParsedSignedPattern {
        is_negated,
        glob_pattern,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pm(paths: &[&str], exts: &[&str]) -> PathMatcher {
        let p: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        let e: Vec<String> = exts.iter().map(|s| s.to_string()).collect();
        PathMatcher::build(&p, &e).unwrap()
    }

    #[test]
    fn empty_rule_matches_nothing() {
        let m = pm(&[], &[]);
        assert!(!m.matches(&PathBuf::from("foo.c")));
    }

    #[test]
    fn exact_path_match_skips_extension_check() {
        // No extensions configured; an exact glob still matches.
        let m = pm(&["src/foo.c"], &[]);
        assert!(m.matches(&PathBuf::from("src/foo.c")));
        assert!(!m.matches(&PathBuf::from("src/bar.c")));
    }

    #[test]
    fn wildcard_glob_requires_matching_extension() {
        let m = pm(&["src/**"], &[".c", ".h"]);
        assert!(m.matches(&PathBuf::from("src/foo.c")));
        assert!(m.matches(&PathBuf::from("src/deep/inner.h")));
        // Wrong extension.
        assert!(!m.matches(&PathBuf::from("src/foo.cpp")));
        // Not under src/.
        assert!(!m.matches(&PathBuf::from("lib/foo.c")));
    }

    #[test]
    fn star_does_not_cross_separator() {
        let m = pm(&["src/*.c"], &[".c"]);
        assert!(m.matches(&PathBuf::from("src/foo.c")));
        assert!(!m.matches(&PathBuf::from("src/deep/foo.c")));
    }

    #[test]
    fn double_star_crosses_separators() {
        let m = pm(&["src/**/*.c"], &[".c"]);
        assert!(m.matches(&PathBuf::from("src/foo.c")));
        assert!(m.matches(&PathBuf::from("src/deep/inner.c")));
    }

    #[test]
    fn first_matching_glob_decides_behavior() {
        // An exact-path glob comes before a wildcard glob with restrictive
        // extensions; the exact one wins for that path.
        let m = pm(&["src/special.cpp", "src/**"], &[".c"]);
        // src/special.cpp matches via the exact glob, no extension check.
        assert!(m.matches(&PathBuf::from("src/special.cpp")));
        // src/foo.c matches via the wildcard glob and .c is allowed.
        assert!(m.matches(&PathBuf::from("src/foo.c")));
        // src/foo.cpp matches only the wildcard glob but extension fails.
        assert!(!m.matches(&PathBuf::from("src/foo.cpp")));
    }

    #[test]
    fn paths_with_no_extension_dont_satisfy_wildcard_glob_with_extensions() {
        let m = pm(&["bin/**"], &[".c"]);
        assert!(!m.matches(&PathBuf::from("bin/runme")));
    }

    #[test]
    fn empty_extension_list_blocks_all_wildcard_globs() {
        let m = pm(&["src/**"], &[]);
        // With wildcards but no allowed extensions, layer 2 fails.
        assert!(!m.matches(&PathBuf::from("src/foo.c")));
    }

    #[test]
    fn invalid_glob_pattern_is_rejected_at_build_time() {
        let res = PathMatcher::build(&["[".to_string()], &[]);
        assert!(res.is_err());
    }

    #[test]
    fn pattern_with_question_mark_treated_as_wildcard() {
        let m = pm(&["src/foo?.c"], &[".c"]);
        assert!(m.matches(&PathBuf::from("src/foo1.c")));
        assert!(!m.matches(&PathBuf::from("src/foo12.c"))); // ? matches one char
    }

    #[test]
    fn negated_pattern_excludes_previous_match() {
        let m = pm(&["src/**", "!src/generated/**"], &[".c"]);
        assert!(m.matches(&PathBuf::from("src/main.c")));
        assert!(!m.matches(&PathBuf::from("src/generated/main.c")));
    }

    #[test]
    fn later_positive_pattern_reincludes_after_negation() {
        let m = pm(
            &["src/**", "!src/generated/**", "src/generated/keep.c"],
            &[".c"],
        );
        assert!(m.matches(&PathBuf::from("src/main.c")));
        assert!(!m.matches(&PathBuf::from("src/generated/skip.c")));
        assert!(m.matches(&PathBuf::from("src/generated/keep.c")));
    }

    #[test]
    fn negated_pattern_without_positive_match_does_not_include() {
        let m = pm(&["!src/**"], &[".c"]);
        assert!(!m.matches(&PathBuf::from("src/main.c")));
        assert!(!m.matches(&PathBuf::from("lib/main.c")));
    }

    #[test]
    fn escaped_leading_bang_matches_literal_bang_path() {
        let m = pm(&[r"\!foo.c"], &[]);
        assert!(m.matches(&PathBuf::from("!foo.c")));
        assert!(!m.matches(&PathBuf::from("foo.c")));
    }

    #[test]
    fn bare_bang_pattern_is_rejected() {
        let res = PathMatcher::build(&["!".to_string()], &[]);
        let err = res.unwrap_err();
        assert!(format!("{err:#}").contains("missing a pattern"));
    }
}
