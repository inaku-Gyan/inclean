//! End-to-end coverage of the `trailing-comment-policies/` fixture, which
//! exercises the four common idioms expressible in the new
//! `trailing_comment` schema (plain replace, fill-if-absent,
//! append-with-idempotency, and line-vs-block `form` switching) in one
//! apply pass plus an idempotency check.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn trailing_comment_apply_is_idempotent() {
    // Re-applying must not stack copies of the configured text. The new
    // model relies on byte equality at the end of `finalize_outcome`:
    // - plain replace runs produce identical bytes on the second pass;
    // - the fill-if-absent rule's regex (`^$`) doesn't match once a
    //   comment is present;
    // - the append rule's non-greedy + optional-suffix pattern consumes
    //   the already-appended " note C" on the second pass.
    let src = support::fixture_path("trailing-comment-policies");
    let dst = support::tmp_dir();
    support::copy_dir(&src, &dst);

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
