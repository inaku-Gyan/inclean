//! Two `original_include_dirs` both contain `foo.h` → the layer-5 rule
//! cannot resolve uniquely. Pipeline must surface `Layer5Ambiguous`, exit
//! code 3, and produce no rewrite.

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn layer5_ambiguity_reports_candidates_and_fails() {
    let src = fixture_path("layer5-ambiguity");
    let dst = tmp();
    copy_dir(&src, &dst);

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    let main_c = summary
        .files
        .iter()
        .find(|f| f.relpath.ends_with("src/main.c"))
        .expect("main.c missing");
    match &main_c.include_results[0].outcome {
        pipe::IncludeOutcome::Layer5Ambiguous { rule, candidates } => {
            assert_eq!(rule, "amb");
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected Layer5Ambiguous, got {other:?}"),
    }
    assert_eq!(pipe::summary_exit_code(&summary), 3);

    std::fs::remove_dir_all(&dst).ok();
}
