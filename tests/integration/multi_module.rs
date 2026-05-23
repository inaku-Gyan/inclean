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
fn multi_module_no_cross_chain_conflict() {
    let src = fixture_path("multi-module-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Rules).unwrap();
    assert!(summary.conflicts.is_empty(), "got: {:?}", summary.conflicts);
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    fs::remove_dir_all(&dst).ok();
}
