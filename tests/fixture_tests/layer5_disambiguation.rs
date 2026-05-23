//! `layer5-disambiguation/` has two leaf rules sharing the same `base` —
//! each leaf uses `match_resolved.under` to route includes resolving in
//! different original dirs through different rewrite templates. Verifies
//! the deepest-rule winner and `${resolved.basename}` substitution.

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn layer5_disambiguation_routes_by_resolved_under() {
    let src = support::fixture_path("layer5-disambiguation");
    let dst = support::tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 1);
    assert_eq!(pipe::summary_exit_code(&summary), 0);

    let after = fs::read_to_string(dst.join("src/main.c")).unwrap();
    assert!(after.contains("\"mylib/public/alpha.h\""), "got: {after}");
    assert!(after.contains("\"mylib/internal/beta.h\""), "got: {after}");

    fs::remove_dir_all(&dst).ok();
}
