//! The base rule has a wide-open `keep` action; the child specializes via
//! layer 5 (`match_resolved.under`) and rewrites to a canonical
//! `mylib/internal/<basename>` form. Verifies the deepest-rule winner
//! semantics in Full mode plus `${resolved.basename}` substitution.

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn layer5_under_constraint_drives_rewrite() {
    let src = support::fixture_path("layer5-under");
    let dst = support::tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c missing");
    match &main_c.include_results[0].outcome {
        pipe::IncludeOutcome::Rewritten { new_text, rule, .. } => {
            assert_eq!(new_text, "\"mylib/internal/foo.h\"");
            assert_eq!(rule, "internal");
        }
        other => panic!("expected Rewritten, got {other:?}"),
    }

    std::fs::remove_dir_all(&dst).ok();
}
