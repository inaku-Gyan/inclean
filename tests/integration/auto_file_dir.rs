//! `auto-file-dir/` exercises `auto` action with
//! `relative_to = "file_dir"`. The same `"helper.h"` include in two source
//! files gets two different rewrites because each is expressed relative
//! to its own source directory.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn auto_file_dir_rewrites_relative_to_source() {
    let src = fixture_path("auto-file-dir");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    // lib/outer.c needs `"helper.h"` → `"inner/helper.h"`; lib/inner/core.c
    // is already pointing at the same-dir header so no rewrite.
    assert_eq!(written, 1);
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    let outer = fs::read_to_string(dst.join("lib/outer.c")).unwrap();
    assert!(outer.contains("\"inner/helper.h\""), "outer.c: {outer}");

    let core = fs::read_to_string(dst.join("lib/inner/core.c")).unwrap();
    assert!(core.contains("\"helper.h\""), "core.c: {core}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn auto_file_dir_apply_is_idempotent() {
    let src = fixture_path("auto-file-dir");
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

    fs::remove_dir_all(&dst).ok();
}
