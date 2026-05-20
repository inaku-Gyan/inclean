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
    let summary = pipe::run(&root).unwrap();

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

    let summary = pipe::run(&dst).unwrap();
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
    let first = pipe::run(&dst).unwrap();
    let n1 = pipe::apply(&first).unwrap();
    assert_eq!(n1, 1);

    // Second pass: nothing should change because the relative include
    // paths already match what `auto` would emit.
    let second = pipe::run(&dst).unwrap();
    let n2 = pipe::apply(&second).unwrap();
    assert_eq!(n2, 0, "second apply must be a no-op (idempotency)");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn flat_library_diff_emits_only_changed_files() {
    let root = fixture_path("flat-library");
    let summary = pipe::run(&root).unwrap();
    let diff = pipe::render_diff(&summary);
    assert!(diff.contains("--- a/src/main.c"));
    assert!(diff.contains("+#include \"mylib/internal/foo.h\""));
    assert!(!diff.contains("foo.h\n+++ b/include/mylib/internal/foo.h"));
}
