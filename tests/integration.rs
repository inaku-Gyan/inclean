//! End-to-end smoke tests against `tests/fixtures/`. The pipeline is
//! exercised through the public library API. The fixtures themselves are
//! never mutated — each test copies the relevant fixture into a tempdir
//! so apply / write operations can run safely.

use std::fs;
use std::path::{Path, PathBuf};

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

fn fixture_path(name: &str) -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest).join("tests/fixtures").join(name)
}

fn tmp() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "inclean-it-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

#[test]
fn flat_library_check_reports_two_rewrites_and_no_errors() {
    let root = fixture_path("flat-library");
    let summary = pipe::run(&root, CheckMode::Full).unwrap();

    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c should appear in the summary");

    let outcomes: Vec<&pipe::IncludeOutcome> =
        main_c.include_results.iter().map(|r| &r.outcome).collect();

    // Two quote includes should be rewritten; the <stdio.h> angle include
    // should fall through with NoMatch (forms = ["quote"] excludes it).
    assert!(
        matches!(outcomes[0], pipe::IncludeOutcome::Rewritten { .. }),
        "got: {outcomes:?}"
    );
    assert!(matches!(
        outcomes[1],
        pipe::IncludeOutcome::Rewritten { .. }
    ));
    assert!(matches!(outcomes[2], pipe::IncludeOutcome::NoMatch));

    if let pipe::IncludeOutcome::Rewritten { new_text, .. } = &outcomes[0] {
        assert_eq!(new_text, "\"mylib/internal/foo.h\"");
    }
    if let pipe::IncludeOutcome::Rewritten { new_text, .. } = &outcomes[1] {
        assert_eq!(new_text, "\"mylib/internal/bar.h\"");
    }

    assert!(summary.conflicts.is_empty());
    assert_eq!(pipe::summary_exit_code(&summary), 0);
}

#[test]
fn flat_library_apply_rewrites_files_in_place() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 1, "only src/main.c should be written");

    let main_after = fs::read_to_string(dst.join("src/main.c")).unwrap();
    assert!(main_after.contains("\"mylib/internal/foo.h\""));
    assert!(main_after.contains("\"mylib/internal/bar.h\""));
    // Trailing comment must be preserved verbatim.
    assert!(main_after.contains("// pulled in for mylib_foo"));
    // Angle includes are left untouched.
    assert!(main_after.contains("<stdio.h>"));

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn flat_library_apply_is_idempotent() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    // First pass: rewrites happen.
    let first = pipe::run(&dst, CheckMode::Full).unwrap();
    let n1 = pipe::apply(&first).unwrap();
    assert_eq!(n1, 1);

    // Second pass: nothing should change because the relative include
    // paths already match what `auto` would emit.
    let second = pipe::run(&dst, CheckMode::Full).unwrap();
    let n2 = pipe::apply(&second).unwrap();
    assert_eq!(n2, 0, "second apply must be a no-op (idempotency)");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn validation_flags_unresolvable_keep_includes() {
    // A "keep" rule with an include that doesn't exist under allowed_include_dirs.
    let root = tmp();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("include")).unwrap();
    fs::write(root.join("src/main.c"), "#include \"missing.h\"\n").unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "base"
        paths = ["src/**"]
        forms = ["quote"]
        allowed_include_dirs = ["include"]
        action = { type = "keep" }
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, CheckMode::Full).unwrap();
    let main_c = &summary.files[0];
    let r = &main_c.include_results[0];
    assert!(r.validation_error.is_some(), "got: {:?}", r);
    assert_eq!(pipe::summary_exit_code(&summary), 3);

    // Rules mode skips the allowed_include_dirs validation entirely.
    let summary = pipe::run(&root, CheckMode::Rules).unwrap();
    let r = &summary.files[0].include_results[0];
    assert!(r.validation_error.is_none());
    assert!(summary.conflicts.is_empty());
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn angle_includes_validate_against_allowed_dirs() {
    // Two cooperating rules along a single chain: the parent allows the
    // whole `mylib/*` angle namespace and validates it against include/,
    // the child specializes to stdio.h and opts out of validation by
    // setting allowed_include_dirs = []. The chain check accepts the pair
    // because they share an extends relationship.
    let root = tmp();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("include/mylib")).unwrap();
    fs::write(root.join("include/mylib/foo.h"), "").unwrap();
    fs::write(
        root.join("src/main.c"),
        "#include <stdio.h>\n#include <mylib/foo.h>\n#include <mylib/missing.h>\n",
    )
    .unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        # All angle includes must resolve under include/.
        [[rule]]
        name = "mylib-angle"
        paths = ["src/**"]
        forms = ["angle"]
        allowed_include_dirs = ["include"]
        action = { type = "keep" }

        # stdio.h is whitelisted — empty allowed_include_dirs opts out.
        [[rule]]
        name = "stdlib"
        extends = "mylib-angle"
        match = '^stdio\.h$'
        allowed_include_dirs = []
        action = { type = "keep" }
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, CheckMode::Full).unwrap();
    assert!(summary.conflicts.is_empty(), "got: {:?}", summary.conflicts);
    let results = &summary.files[0].include_results;
    // <stdio.h> matched the stdlib rule with empty allowed_include_dirs → skipped.
    assert!(results[0].validation_error.is_none());
    // <mylib/foo.h> resolves → passes.
    assert!(results[1].validation_error.is_none());
    // <mylib/missing.h> does not resolve → fails.
    assert!(results[2].validation_error.is_some());

    fs::remove_dir_all(&root).ok();
}

#[test]
fn init_template_passes_config_check() {
    use std::process::Command;
    let root = tmp();
    let bin = env!("CARGO_BIN_EXE_inclean");
    let init = Command::new(bin)
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success(), "init failed: {:?}", init);
    let out = Command::new(bin)
        .args(["check", "--level", "config", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`check --level config` on init template failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn flat_library_diff_emits_only_changed_files() {
    let root = fixture_path("flat-library");
    let summary = pipe::run(&root, CheckMode::Full).unwrap();
    let diff = pipe::render_diff(&summary);
    assert!(diff.contains("--- a/src/main.c"));
    assert!(diff.contains("+#include \"mylib/internal/foo.h\""));
    assert!(!diff.contains("foo.h\n+++ b/include/mylib/internal/foo.h"));
}

#[test]
fn cross_chain_conflict_reported_in_rules_mode() {
    // Two top-level rules both match the same include; neither extends
    // the other → CrossChain conflict.
    let root = tmp();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.c"), "#include \"x.h\"\n").unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "a"
        paths = ["src/**"]
        forms = ["quote"]
        action = { type = "keep" }

        [[rule]]
        name = "b"
        paths = ["src/**"]
        forms = ["quote"]
        action = { type = "keep" }
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, CheckMode::Rules).unwrap();
    assert_eq!(summary.conflicts.len(), 1);
    assert!(matches!(
        &summary.conflicts[0].kind,
        pipe::ConflictKindOwned::CrossChain { .. }
    ));
    assert_eq!(pipe::summary_exit_code(&summary), 3);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn layer5_under_constraint_drives_rewrite() {
    // The base rule has a wide-open auto action; the child specializes via
    // layer 5 to only fire when the include actually resolves under
    // src/internal, and rewrites it to a canonical mylib/internal/<file>
    // path.
    let root = tmp();
    fs::create_dir_all(root.join("src/internal")).unwrap();
    fs::create_dir_all(root.join("include")).unwrap();
    fs::write(root.join("src/internal/foo.h"), "").unwrap();
    fs::write(root.join("src/main.c"), "#include \"foo.h\"\n").unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "base"
        paths = ["src/**"]
        forms = ["quote"]
        original_include_dirs = ["src/internal"]
        action = { type = "keep" }

        [[rule]]
        name = "internal"
        extends = "base"
        match_resolved = { under = "src/internal" }
        action = { type = "rewrite", to = "mylib/internal/${resolved.basename}" }
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, CheckMode::Full).unwrap();
    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c missing");
    match &main_c.include_results[0].outcome {
        pipe::IncludeOutcome::Rewritten { new_text, rule, .. } => {
            assert_eq!(new_text, "\"mylib/internal/foo.h\"");
            assert_eq!(rule, "internal");
        }
        other => panic!("expected Rewritten, got {other:?}"),
    }
    fs::remove_dir_all(&root).ok();
}

#[test]
fn layer5_ambiguity_reports_candidates_and_fails() {
    // Two original_include_dirs both contain foo.h → the layer-5 rule cannot
    // resolve uniquely. Pipeline must surface Layer5Ambiguous, exit code 3,
    // and not produce a rewrite.
    let root = tmp();
    fs::create_dir_all(root.join("src/a")).unwrap();
    fs::create_dir_all(root.join("src/b")).unwrap();
    fs::write(root.join("src/a/foo.h"), "").unwrap();
    fs::write(root.join("src/b/foo.h"), "").unwrap();
    fs::write(root.join("src/main.c"), "#include \"foo.h\"\n").unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "amb"
        paths = ["src/**"]
        forms = ["quote"]
        original_include_dirs = ["src/a", "src/b"]
        match_resolved = { match = '\.h$' }
        action = { type = "keep" }
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, CheckMode::Full).unwrap();
    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c missing");
    match &main_c.include_results[0].outcome {
        pipe::IncludeOutcome::Layer5Ambiguous { rule, candidates } => {
            assert_eq!(rule, "amb");
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected Layer5Ambiguous, got {other:?}"),
    }
    assert_eq!(pipe::summary_exit_code(&summary), 3);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn child_wider_than_parent_reported_in_rules_mode() {
    // Child widens `paths` from `src/**` to `**`. A file outside src/ then
    // triggers the child without triggering the parent → ChildWiderThanParent.
    let root = tmp();
    fs::write(root.join("main.c"), "#include \"x.h\"\n").unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "parent"
        paths = ["src/**"]
        forms = ["quote"]
        action = { type = "keep" }

        [[rule]]
        name = "child"
        extends = "parent"
        paths = ["**"]
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, CheckMode::Rules).unwrap();
    assert_eq!(summary.conflicts.len(), 1);
    match &summary.conflicts[0].kind {
        pipe::ConflictKindOwned::ChildWiderThanParent {
            child,
            missing_ancestor,
        } => {
            assert_eq!(child, "child");
            assert_eq!(missing_ancestor, "parent");
        }
        other => panic!("unexpected: {other:?}"),
    }
    fs::remove_dir_all(&root).ok();
}
