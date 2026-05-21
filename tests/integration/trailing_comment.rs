//! End-to-end coverage of the `trailing-comment-policies/` fixture, which
//! exercises all four `trailing_comment` policies (prepend / fill_if_absent
//! / append / replace) in one apply pass plus an idempotency check.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn trailing_comment_apply_exercises_all_policies() {
    let src = fixture_path("trailing-comment-policies");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 1, "only src/main.c should be rewritten");
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    let after = fs::read_to_string(dst.join("src/main.c")).unwrap();

    // prepend: text precedes the (empty) trailing — two-space gutter.
    assert!(
        after.contains("\"mylib/internal/foo.h\"  /* note A */"),
        "got:\n{after}"
    );
    // fill_if_absent with existing user comment: keep user note verbatim.
    assert!(after.contains("\"mylib/private/bar.h\"  // user note"));
    assert!(
        !after.contains("bar.h\"  // user note  //") && !after.contains("bar.h\"  // note B"),
        "fill_if_absent must not touch the existing comment; got:\n{after}"
    );
    // fill_if_absent with no existing comment: inject text.
    assert!(after.contains("\"mylib/private/baz.h\"  // note B"));
    // append: place text after existing block comment.
    assert!(
        after.contains("\"mylib/helper/qux.h\" /* please keep */ // note C"),
        "got:\n{after}"
    );
    // replace: text overwrites the legacy comment.
    assert!(after.contains("\"mylib/legacy/old.h\"  // note D"));
    assert!(
        !after.contains("legacy comment to be overwritten"),
        "replace policy should have removed the user note; got:\n{after}"
    );

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn trailing_comment_apply_is_idempotent() {
    // Re-applying must not stack copies of the configured text — the
    // prepend / append idempotency check protects against that, and the
    // replace / fill_if_absent paths converge on the byte-equality
    // shortcut.
    let src = fixture_path("trailing-comment-policies");
    let dst = tmp();
    copy_dir(&src, &dst);

    let first = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(pipe::apply(&first).unwrap(), 1);

    let second = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(
        pipe::apply(&second).unwrap(),
        0,
        "second apply must be a no-op"
    );

    let after = fs::read_to_string(dst.join("src/main.c")).unwrap();
    // No duplicated trailing comments anywhere.
    assert_eq!(
        after.matches("note A").count(),
        1,
        "prepend should not duplicate; got:\n{after}"
    );
    assert_eq!(
        after.matches("note C").count(),
        1,
        "append should not duplicate; got:\n{after}"
    );
    assert_eq!(
        after.matches("note B").count(),
        1,
        "fill_if_absent should not duplicate; got:\n{after}"
    );
    assert_eq!(
        after.matches("note D").count(),
        1,
        "replace should not duplicate; got:\n{after}"
    );

    fs::remove_dir_all(&dst).ok();
}
