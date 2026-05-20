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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "inclean-hi-{}-{}",
            std::process::id(),
            rand_u64()
        ));
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
        let r = resolve_in_dirs(
            &root,
            &["a".to_string(), "b".to_string()],
            "foo.h",
        )
        .unwrap();
        assert_eq!(r, root.join("a/foo.h"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn supports_subdir_paths_in_include_text() {
        let root = tmp();
        write(&root, "src/internal/sub/bar.h", "");
        let r = resolve_in_dirs(
            &root,
            &["src/internal".to_string()],
            "sub/bar.h",
        )
        .unwrap();
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
    fn skips_dirs_that_dont_exist() {
        let root = tmp();
        write(&root, "real/foo.h", "");
        let r = resolve_in_dirs(
            &root,
            &["nope".to_string(), "real".to_string()],
            "foo.h",
        )
        .unwrap();
        assert_eq!(r, root.join("real/foo.h"));
        fs::remove_dir_all(&root).ok();
    }
}
