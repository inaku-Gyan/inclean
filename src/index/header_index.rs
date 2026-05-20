//! Resolve `#include` text against a list of search directories, in the
//! preprocessor's literal-concatenation style: the candidate path is
//! `<project_root>/<dir>/<include_text>`. The first directory in which a
//! regular file exists wins.
//!
//! v1 deliberately keeps this as a stateless lookup rather than a
//! precomputed index. The number of `#include`s in any single project is
//! small enough that on-demand filesystem checks are negligible. If a
//! later milestone wants ambiguity detection across an `-I` set or
//! basename-only fallback lookup, this is where it goes.

use std::path::{Path, PathBuf};

/// Try to resolve `include_text` (the literal string between `"..."` or
/// `<...>`) under each `dir` in order. Returns the first path that exists
/// as a regular file. Paths in `dirs` are interpreted relative to
/// `project_root`.
pub fn resolve_in_dirs(
    project_root: &Path,
    dirs: &[String],
    include_text: &str,
) -> Option<PathBuf> {
    for dir in dirs {
        let candidate = project_root.join(dir).join(include_text);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Outcome of a unique-resolution lookup. Layer 5 uses this — it refuses
/// the convenient "first wins" of `resolve_in_dirs` because by definition
/// layer 5 wants a single authoritative physical file.
pub enum UniqueResolution {
    /// No dir contained the include.
    None,
    /// Exactly one dir contained the include.
    Unique(PathBuf),
    /// Two or more dirs contained the include — the user must narrow the
    /// rule's `original_include_dirs` to disambiguate.
    Ambiguous(Vec<PathBuf>),
}

/// Like [`resolve_in_dirs`] but visits every directory and reports
/// ambiguity when more than one contains the file. Returns absolute paths.
pub fn resolve_in_dirs_unique(
    project_root: &Path,
    dirs: &[String],
    include_text: &str,
) -> UniqueResolution {
    let mut hits: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let candidate = project_root.join(dir).join(include_text);
        if candidate.is_file() {
            hits.push(candidate);
        }
    }
    match hits.len() {
        0 => UniqueResolution::None,
        1 => UniqueResolution::Unique(hits.pop().unwrap()),
        _ => UniqueResolution::Ambiguous(hits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("inclean-hi-{}-{}", std::process::id(), rand_u64()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn rand_u64() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    #[test]
    fn resolves_in_first_listed_dir_that_has_the_file() {
        let root = tmp();
        write(&root, "src/internal/foo.h", "");
        let r = resolve_in_dirs(
            &root,
            &["src/external".to_string(), "src/internal".to_string()],
            "foo.h",
        )
        .unwrap();
        assert_eq!(r, root.join("src/internal/foo.h"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn earlier_dir_wins_when_both_exist() {
        let root = tmp();
        write(&root, "a/foo.h", "first");
        write(&root, "b/foo.h", "second");
        let r = resolve_in_dirs(&root, &["a".to_string(), "b".to_string()], "foo.h").unwrap();
        assert_eq!(r, root.join("a/foo.h"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn supports_subdir_paths_in_include_text() {
        let root = tmp();
        write(&root, "src/internal/sub/bar.h", "");
        let r = resolve_in_dirs(&root, &["src/internal".to_string()], "sub/bar.h").unwrap();
        assert_eq!(r, root.join("src/internal/sub/bar.h"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn returns_none_when_not_found() {
        let root = tmp();
        assert!(resolve_in_dirs(&root, &["src".to_string()], "missing.h").is_none());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unique_returns_none_when_missing() {
        let root = tmp();
        assert!(matches!(
            resolve_in_dirs_unique(&root, &["src".into()], "missing.h"),
            UniqueResolution::None
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unique_returns_unique_when_one_match() {
        let root = tmp();
        write(&root, "a/foo.h", "");
        match resolve_in_dirs_unique(&root, &["a".into(), "b".into()], "foo.h") {
            UniqueResolution::Unique(p) => assert_eq!(p, root.join("a/foo.h")),
            _ => panic!("expected unique"),
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unique_returns_ambiguous_when_multiple() {
        let root = tmp();
        write(&root, "a/foo.h", "");
        write(&root, "b/foo.h", "");
        match resolve_in_dirs_unique(&root, &["a".into(), "b".into()], "foo.h") {
            UniqueResolution::Ambiguous(hits) => {
                assert_eq!(hits.len(), 2);
            }
            _ => panic!("expected ambiguous"),
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skips_dirs_that_dont_exist() {
        let root = tmp();
        write(&root, "real/foo.h", "");
        let r = resolve_in_dirs(&root, &["nope".to_string(), "real".to_string()], "foo.h").unwrap();
        assert_eq!(r, root.join("real/foo.h"));
        fs::remove_dir_all(&root).ok();
    }
}
