//! End-to-end smoke tests against `tests/fixtures/`. The pipeline is
//! exercised through the public library API. The fixtures themselves are
//! never mutated — each test copies the relevant fixture into a tempdir
//! so apply / write operations can run safely.

use std::fs;
use std::path::{Path, PathBuf};

use inclean::pipeline::run as pipe;

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
    let summary = pipe::run(&root, true).unwrap();

    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c should appear in the summary");

    let outcomes: Vec<&pipe::IncludeOutcome> = main_c
        .include_results
        .iter()
        .map(|r| &r.outcome)
        .collect();

    // Two quote includes should be rewritten; the <stdio.h> angle include
    // should fall through with NoMatch (forms = ["quote"] excludes it).
    assert!(
        matches!(outcomes[0], pipe::IncludeOutcome::Rewritten { .. }),
        "got: {outcomes:?}"
    );
    assert!(matches!(outcomes[1], pipe::IncludeOutcome::Rewritten { .. }));
    assert!(matches!(outcomes[2], pipe::IncludeOutcome::NoMatch));

    if let pipe::IncludeOutcome::Rewritten { new_text, .. } = &outcomes[0] {
        assert_eq!(new_text, "\"mylib/internal/foo.h\"");
    }
    if let pipe::IncludeOutcome::Rewritten { new_text, .. } = &outcomes[1] {
        assert_eq!(new_text, "\"mylib/internal/bar.h\"");
    }

    assert_eq!(pipe::summary_exit_code(&summary), 0);
}

#[test]
fn flat_library_apply_rewrites_files_in_place() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, true).unwrap();
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
    let first = pipe::run(&dst, true).unwrap();
    let n1 = pipe::apply(&first).unwrap();
    assert_eq!(n1, 1);

    // Second pass: nothing should change because the relative include
    // paths already match what `auto` would emit.
    let second = pipe::run(&dst, true).unwrap();
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

    let summary = pipe::run(&root, true).unwrap();
    let main_c = &summary.files[0];
    let r = &main_c.include_results[0];
    assert!(r.validation_error.is_some(), "got: {:?}", r);
    assert_eq!(pipe::summary_exit_code(&summary), 3);

    // With validation disabled, the error goes away.
    let summary = pipe::run(&root, false).unwrap();
    let r = &summary.files[0].include_results[0];
    assert!(r.validation_error.is_none());
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn angle_includes_are_skipped_unless_pattern_matches() {
    let root = tmp();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("include")).unwrap();
    // No header exists under include/ for either include.
    fs::write(
        root.join("src/main.c"),
        "#include <stdio.h>\n#include <mylib/foo.h>\n",
    )
    .unwrap();
    fs::write(
        root.join("inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "base"
        paths = ["src/**"]
        forms = ["angle"]
        allowed_include_dirs = ["include"]
        validate_angle_patterns = ["^mylib/"]
        action = { type = "keep" }
        "#,
    )
    .unwrap();

    let summary = pipe::run(&root, true).unwrap();
    let results = &summary.files[0].include_results;
    // <stdio.h> doesn't match validate_angle_patterns → skipped.
    assert!(results[0].validation_error.is_none());
    // <mylib/foo.h> matches the pattern but the file doesn't exist → fails.
    assert!(results[1].validation_error.is_some());

    fs::remove_dir_all(&root).ok();
}

#[test]
fn init_template_passes_validate() {
    use std::process::Command;
    let root = tmp();
    let bin = env!("CARGO_BIN_EXE_inclean");
    let init = Command::new(bin)
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success(), "init failed: {:?}", init);
    let validate = Command::new(bin)
        .args(["validate", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "validate on init template failed: stdout={} stderr={}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn flat_library_diff_emits_only_changed_files() {
    let root = fixture_path("flat-library");
    let summary = pipe::run(&root, true).unwrap();
    let diff = pipe::render_diff(&summary);
    assert!(diff.contains("--- a/src/main.c"));
    assert!(diff.contains("+#include \"mylib/internal/foo.h\""));
    assert!(!diff.contains("foo.h\n+++ b/include/mylib/internal/foo.h"));
}
