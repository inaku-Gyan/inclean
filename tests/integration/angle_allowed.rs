//! Parent rule covers the whole `mylib/*` angle namespace and validates
//! against `include/`; child opts out of validation for `stdio.h` by
//! setting `allowed_include_dirs = []`. The chain check accepts both
//! because they share an `extends` relationship.

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::support;

#[test]
fn angle_includes_validate_against_allowed_dirs() {
    let src = support::fixture_path("angle-allowed");
    let dst = support::tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    assert!(summary.conflicts.is_empty(), "got: {:?}", summary.conflicts);
    let results = &summary.files[0].include_results;
    // <stdio.h> matched the stdlib rule with empty allowed_include_dirs → skipped.
    assert!(results[0].validation_error.is_none());
    // <mylib/foo.h> resolves → passes.
    assert!(results[1].validation_error.is_none());
    // <mylib/missing.h> does not resolve → fails.
    assert!(results[2].validation_error.is_some());

    std::fs::remove_dir_all(&dst).ok();
}
