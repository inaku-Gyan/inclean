//! End-to-end coverage of `[project].root` structural rules, driven through
//! `pipeline::run::run` in `CheckMode::Config` (the lightest mode that still
//! exercises `validate_loaded`).

use std::fs;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

use super::common::*;

#[test]
fn run_errors_when_root_config_missing_project_block() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    fs::write(
        dst.join("inclean.toml"),
        r#"
        [[rule]]
        name = "base"
        "#,
    )
    .unwrap();

    let err = pipe::run(&dst, CheckMode::Config).unwrap_err();
    assert!(format!("{err:#}").contains("[project]"), "got: {err:#}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn run_errors_when_subconfig_declares_project_block() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    fs::write(
        dst.join("src/inclean.toml"),
        r#"
        [project]
        root = "."

        [[rule]]
        name = "src-rule"
        extends = "base"
        "#,
    )
    .unwrap();

    let err = pipe::run(&dst, CheckMode::Config).unwrap_err();
    assert!(format!("{err:#}").contains("sub-config"), "got: {err:#}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn run_errors_when_project_root_is_empty() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    fs::write(
        dst.join("inclean.toml"),
        r#"
        [project]
        root = ""

        [[rule]]
        name = "base"
        "#,
    )
    .unwrap();

    let err = pipe::run(&dst, CheckMode::Config).unwrap_err();
    assert!(
        format!("{err:#}").contains("[project].root"),
        "got: {err:#}"
    );

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn run_accepts_non_dot_project_root() {
    // `[project].root` is a sigil: any non-empty value is accepted and does
    // not influence the resolved project root, which always comes from the
    // location of the root `inclean.toml`.
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    fs::write(
        dst.join("inclean.toml"),
        r#"
        [project]
        root = "src"

        [[rule]]
        name = "base"
        paths = ["src/**", "include/**"]
        forms = ["quote"]
        allowed_include_dirs = ["include"]
        original_include_dirs = ["include/mylib/internal"]
        "#,
    )
    .unwrap();

    let summary = pipe::run(&dst, CheckMode::Config).unwrap();
    let expected = fs::canonicalize(&dst).unwrap();
    assert_eq!(summary.project_root, expected);

    fs::remove_dir_all(&dst).ok();
}
