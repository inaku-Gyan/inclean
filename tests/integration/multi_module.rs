//! `multi-module-library/` has an abstract root rule + per-module
//! sub-configs (`src/moduleA/inclean.toml`, `src/moduleB/inclean.toml`).
//! Each module's rule narrows `paths` and supplies its own
//! `original_include_dirs`. The two siblings live under the same `base`
//! parent so they don't trip the cross-chain check.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn multi_module_each_module_uses_its_own_rule() {
    let src = fixture_path("multi-module-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 2, "moduleA/code.c + moduleB/code.c");
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    let a = fs::read_to_string(dst.join("src/moduleA/code.c")).unwrap();
    assert!(a.contains("\"modA/a1.h\""), "moduleA: {a}");
    assert!(a.contains("\"modA/a2.h\""), "moduleA: {a}");

    let b = fs::read_to_string(dst.join("src/moduleB/code.c")).unwrap();
    assert!(b.contains("\"modB/b1.h\""), "moduleB: {b}");
    assert!(b.contains("\"modB/b2.h\""), "moduleB: {b}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn multi_module_no_cross_chain_conflict() {
    let src = fixture_path("multi-module-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Rules).unwrap();
    assert!(summary.conflicts.is_empty(), "got: {:?}", summary.conflicts);
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    fs::remove_dir_all(&dst).ok();
}
