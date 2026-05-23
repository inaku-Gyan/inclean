//! Two top-level rules both match the same include; neither extends the
//! other → `CrossChain` conflict. Rules mode is enough to surface it.

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use crate::support;

#[test]
fn cross_chain_conflict_reported_in_rules_mode() {
    let src = support::get_fixture("cross-chain");
    let dst = support::new_tmp_dir();
    support::copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Rules).unwrap();
    assert_eq!(summary.conflicts.len(), 1);
    assert!(matches!(
        &summary.conflicts[0].kind,
        pipe::ConflictKindOwned::CrossChain { .. }
    ));
    assert_eq!(pipe::summary_exit_code(&summary), 3);

    std::fs::remove_dir_all(&dst).ok();
}
