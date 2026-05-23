//! A `keep` rule whose `allowed_include_dirs` contains nothing matching the
//! include trips post-action validation in Full mode (exit 3) but is
//! silent in Rules mode.

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn validation_flags_unresolvable_keep_includes() {
    let src = support::get_fixture("validation-keep");
    let dst = support::new_tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let main_c = &summary.files[0];
    let r = &main_c.include_results[0];
    assert!(r.validation_error.is_some(), "got: {:?}", r);
    assert_eq!(pipe::summary_exit_code(&summary), 3);

    // Rules mode skips the allowed_include_dirs validation entirely.
    let summary = pipe::run(&dst, CheckMode::Rules).unwrap();
    let r = &summary.files[0].include_results[0];
    assert!(r.validation_error.is_none());
    assert!(summary.conflicts.is_empty());
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    std::fs::remove_dir_all(&dst).ok();
}
