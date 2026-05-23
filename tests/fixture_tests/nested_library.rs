//! `nested-library/` exercises deep source directories plus multiple
//! `original_include_dirs`. Three .c files at different depths each
//! resolve their includes against one of two original dirs and rewrite
//! to a canonical `mylib/...` form relative to `include/`.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn nested_library_apply_is_idempotent() {
    let src = support::get_fixture("nested-library");
    let dst = support::new_tmp_dir();
    support::copy_dir(&src, &dst);

    let first = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(pipe::apply(&first).unwrap(), 3);

    let second = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(
        pipe::apply(&second).unwrap(),
        0,
        "second apply must be a no-op"
    );

    fs::remove_dir_all(&dst).ok();
}
