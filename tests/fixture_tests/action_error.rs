//! `action-error/` exercises the `error` action: a regex match on the
//! include triggers an explicit failure with `${include.content}`
//! substituted into the message. Exit code is 2 (distinct from the
//! structural-failure exit code 3).

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn action_error_returns_exit_code_2() {
    let src = support::fixture_path("action-error");
    let dst = support::tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(pipe::summary_exit_code(&summary), 2);

    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c missing");
    match &main_c.include_results[0].outcome {
        pipe::IncludeOutcome::Error { rule, message } => {
            assert_eq!(rule, "deprecated");
            assert_eq!(message, "include `deprecated_old.h` is deprecated");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    std::fs::remove_dir_all(&dst).ok();
}
