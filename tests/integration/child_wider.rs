//! Child rule widens `paths` past its parent's constraint. A file outside
//! the parent's `paths` then triggers the child without triggering the
//! parent → `ChildWiderThanParent`.

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::support;

#[test]
fn child_wider_than_parent_reported_in_rules_mode() {
    let src = support::fixture_path("child-wider");
    let dst = support::tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Rules).unwrap();
    assert_eq!(summary.conflicts.len(), 1);
    match &summary.conflicts[0].kind {
        pipe::ConflictKindOwned::ChildWiderThanParent {
            child,
            missing_ancestor,
        } => {
            assert_eq!(child, "child");
            assert_eq!(missing_ancestor, "parent");
        }
        other => panic!("unexpected: {other:?}"),
    }

    std::fs::remove_dir_all(&dst).ok();
}
