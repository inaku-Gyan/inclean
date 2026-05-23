//! End-to-end coverage of the `flat-library/` fixture: a single-level
//! `include/mylib/internal/` layout. Exercises check, apply, diff, and
//! idempotency.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

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
fn flat_library_apply_is_idempotent() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let first = pipe::run(&dst, CheckMode::Full).unwrap();
    let n1 = pipe::apply(&first).unwrap();
    assert_eq!(n1, 1);

    let second = pipe::run(&dst, CheckMode::Full).unwrap();
    let n2 = pipe::apply(&second).unwrap();
    assert_eq!(n2, 0, "second apply must be a no-op (idempotency)");

    fs::remove_dir_all(&dst).ok();
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
