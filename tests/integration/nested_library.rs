//! `nested-library/` exercises deep source directories plus multiple
//! `original_include_dirs`. Three .c files at different depths each
//! resolve their includes against one of two original dirs and rewrite
//! to a canonical `mylib/...` form relative to `include/`.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn nested_library_rewrites_all_depths() {
    let src = fixture_path("nested-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(
        written, 3,
        "main.c + impl.c + deep.c should each be written"
    );
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    let main_c = fs::read_to_string(dst.join("src/main.c")).unwrap();
    assert!(main_c.contains("\"mylib/core/api.h\""), "main.c: {main_c}");
    assert!(
        main_c.contains("\"mylib/core/detail/alpha.h\""),
        "main.c: {main_c}"
    );

    let impl_c = fs::read_to_string(dst.join("src/core/impl.c")).unwrap();
    assert!(
        impl_c.contains("\"mylib/core/detail/beta.h\""),
        "impl.c: {impl_c}"
    );

    let deep_c = fs::read_to_string(dst.join("src/core/more/deep.c")).unwrap();
    assert!(deep_c.contains("\"mylib/core/api.h\""), "deep.c: {deep_c}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn nested_library_apply_is_idempotent() {
    let src = fixture_path("nested-library");
    let dst = tmp();
    copy_dir(&src, &dst);

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
