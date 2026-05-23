//! The `out-of-tree-config` fixture pins down the "config above the
//! project root" layout: `inclean.toml` sits at the fixture top with
//! `[project] root = "lib"`, and all source/header files live under
//! `lib/`. Paths in the rule (`paths`, `allowed_include_dirs`,
//! `original_include_dirs`) are all relative to the resolved root
//! (`<fixture>/lib`), not to the config file's directory.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn out_of_tree_config_resolves_project_root_into_lib() {
    let src = support::get_fixture("out-of-tree-config");
    let dst = support::new_tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(
        summary.project_root,
        fs::canonicalize(dst.join("lib")).unwrap()
    );

    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 1, "lib/src/main.c should be rewritten");

    let after = fs::read_to_string(dst.join("lib/src/main.c")).unwrap();
    assert!(after.contains("\"mylib/internal/foo.h\""), "got: {after}");
    assert!(after.contains("\"mylib/internal/bar.h\""), "got: {after}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn out_of_tree_config_walks_up_from_deep_starting_path() {
    let src = support::get_fixture("out-of-tree-config");
    let dst = support::new_tmp_dir();
    support::copy_dir(&src, &dst);

    // Starting from `<dst>/lib/src` (a deep directory below the resolved
    // project root but above no inclean.toml) the walker must climb all
    // the way back to `<dst>/inclean.toml` before resolving `root = "lib"`.
    let summary = pipe::run(&dst.join("lib/src"), CheckMode::Config).unwrap();
    assert_eq!(
        summary.project_root,
        fs::canonicalize(dst.join("lib")).unwrap()
    );

    fs::remove_dir_all(&dst).ok();
}
