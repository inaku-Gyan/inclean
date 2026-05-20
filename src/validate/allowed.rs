//! Post-action validation: does the include — as it will exist in the
//! file after the action runs — resolve under the matched rule's
//! `allowed_include_dirs`?
//!
//! Policy:
//! - The matched rule's `allowed_include_dirs` is empty → skip (explicit
//!   "this rule does not participate in validation"; the idiom for
//!   allow-listing e.g. system headers).
//! - Quote-form include → must resolve under one of the dirs.
//! - Angle-form include → must resolve under one of the dirs. Whether to
//!   validate angle includes at all is controlled at the rule level by
//!   `forms`: a rule whose `forms` excludes `"angle"` will never match an
//!   angle include in the first place. System headers are typically
//!   handled by a dedicated `forms = ["angle"]` rule with empty
//!   `allowed_include_dirs`.
//! - Macro-form include → not validated (should already have been rejected
//!   by the action evaluator).

use std::path::Path;

use crate::config::inherit::ResolvedRule;
use crate::config::schema::IncludeForm;
use crate::index::header_index;

/// Validate the include in its post-action state. Returns `Some(message)`
/// if validation failed; `None` if it passed or was skipped.
///
/// `final_form` / `final_content` describe what the include text will be
/// **after** the action's rewrite is applied (or the original include if
/// the action was `keep`).
pub fn validate(
    final_form: IncludeForm,
    final_content: &str,
    rule: &ResolvedRule,
    project_root: &Path,
) -> Option<String> {
    if rule.allowed_include_dirs.is_empty() {
        return None;
    }
    match final_form {
        IncludeForm::Macro => None,
        IncludeForm::Quote | IncludeForm::Angle => {
            check_resolvable(final_content, rule, project_root)
        }
    }
}

fn check_resolvable(
    include_text: &str,
    rule: &ResolvedRule,
    project_root: &Path,
) -> Option<String> {
    if header_index::resolve_in_dirs(project_root, &rule.allowed_include_dirs, include_text)
        .is_some()
    {
        return None;
    }
    Some(format!(
        "include `{include_text}` cannot be resolved under the matched rule's allowed_include_dirs ({:?})",
        rule.allowed_include_dirs
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::inherit::resolve;
    use crate::config::schema::{parse, LoadedConfig};
    use std::fs;
    use std::path::PathBuf;

    fn cfg(body: &str) -> ResolvedRule {
        let configs = vec![LoadedConfig {
            path: PathBuf::from("/p/inclean.toml"),
            raw: parse(body, &PathBuf::from("/p/inclean.toml")).unwrap(),
        }];
        let map = resolve(&configs).unwrap();
        map.into_iter().next().unwrap().1
    }

    fn tmp_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "inclean-validate-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    #[test]
    fn empty_allowed_dirs_means_skip() {
        let rule = cfg(r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = []
            "#);
        let root = PathBuf::from("/p");
        assert!(validate(IncludeForm::Quote, "foo.h", &rule, &root).is_none());
        assert!(validate(IncludeForm::Angle, "stdio.h", &rule, &root).is_none());
    }

    #[test]
    fn quote_resolves_under_allowed_passes() {
        let root = tmp_root();
        touch(&root, "include/foo.h");
        let rule = cfg(r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            "#);
        assert!(validate(IncludeForm::Quote, "foo.h", &rule, &root).is_none());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn quote_unresolvable_fails() {
        let root = tmp_root();
        let rule = cfg(r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            "#);
        let err = validate(IncludeForm::Quote, "missing.h", &rule, &root).unwrap();
        assert!(err.contains("cannot be resolved"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn angle_validated_like_quote_when_allowed_dirs_nonempty() {
        let root = tmp_root();
        touch(&root, "include/mylib/foo.h");
        let rule = cfg(r#"
            [[rule]]
            name = "r"
            forms = ["angle"]
            allowed_include_dirs = ["include"]
            "#);
        // Resolves → pass.
        assert!(validate(IncludeForm::Angle, "mylib/foo.h", &rule, &root).is_none());
        // Doesn't resolve → fail.
        let err = validate(IncludeForm::Angle, "stdio.h", &rule, &root).unwrap();
        assert!(err.contains("cannot be resolved"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn macro_form_is_not_validated() {
        let root = tmp_root();
        let rule = cfg(r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["include"]
            "#);
        assert!(validate(IncludeForm::Macro, "MY_HEADER", &rule, &root).is_none());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn first_matching_allowed_dir_wins() {
        let root = tmp_root();
        touch(&root, "second/foo.h");
        let rule = cfg(r#"
            [[rule]]
            name = "r"
            allowed_include_dirs = ["first", "second"]
            "#);
        assert!(validate(IncludeForm::Quote, "foo.h", &rule, &root).is_none());
        fs::remove_dir_all(&root).ok();
    }
}
