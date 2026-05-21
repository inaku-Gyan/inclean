//! End-to-end coverage of the `trailing-comment-policies/` fixture, which
//! exercises the four common idioms expressible in the new
//! `trailing_comment` schema (plain replace, fill-if-absent,
//! append-with-idempotency, and line-vs-block `form` switching) in one
//! apply pass plus an idempotency check.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn trailing_comment_apply_exercises_all_idioms() {
    let src = fixture_path("trailing-comment-policies");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 1, "only src/main.c should be rewritten");
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    let after = fs::read_to_string(dst.join("src/main.c")).unwrap();

    // Plain replace with `form = "block"`: empty trailing → `/* note A */`.
    assert!(
        after.contains("\"mylib/internal/foo.h\"  /* note A */"),
        "got:\n{after}"
    );
    // Fill-if-absent (`match = "^$"`) with existing user comment: regex
    // doesn't match → trailing untouched.
    assert!(
        after.contains("\"mylib/private/bar.h\"  // user note"),
        "fill_if_absent must not touch the existing comment; got:\n{after}"
    );
    assert!(
        !after.contains("bar.h\"  // user note  //") && !after.contains("bar.h\"  // note B"),
        "fill_if_absent must not stack notes; got:\n{after}"
    );
    // Fill-if-absent with no existing comment: inject. `form = preserve`
    // with no existing comment falls back to line style.
    assert!(after.contains("\"mylib/private/baz.h\"  // note B"));
    // Append-with-idempotency: existing `/* please keep */` keeps the
    // block style (preserve) and gets " note C" appended to the body.
    assert!(
        after.contains("\"mylib/helper/qux.h\" /* please keep note C */"),
        "got:\n{after}"
    );
    // Plain replace (line form): overwrites the legacy comment.
    assert!(after.contains("\"mylib/legacy/old.h\"  // note D"));
    assert!(
        !after.contains("legacy comment to be overwritten"),
        "replace should have removed the user note; got:\n{after}"
    );

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn trailing_comment_apply_is_idempotent() {
    // Re-applying must not stack copies of the configured text. The new
    // model relies on byte equality at the end of `finalize_outcome`:
    // - plain replace runs produce identical bytes on the second pass;
    // - the fill-if-absent rule's regex (`^$`) doesn't match once a
    //   comment is present;
    // - the append rule's non-greedy + optional-suffix pattern consumes
    //   the already-appended " note C" on the second pass.
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
    assert_eq!(
        after.matches("note A").count(),
        1,
        "replace should not duplicate; got:\n{after}"
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
