//! End-to-end coverage of `[project].root` semantics, driven through
//! `pipeline::run::run`. Covers:
//!
//! - default-to-`"."` when the field is omitted
//! - structural errors (missing `[project]`, empty `root`, extra
//!   sub-`inclean.toml` files)
//! - resolved project root reflected on `Summary.project_root`
//! - "out-of-tree config" via the dedicated fixture
//! - upward walk from a deep starting path

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
fn run_errors_when_extra_inclean_toml_present() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    fs::write(
        dst.join("src/inclean.toml"),
        r#"
        [[rule]]
        name = "stray"
        "#,
    )
    .unwrap();

    let err = pipe::run(&dst, CheckMode::Config).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("sub-configs are not supported"), "got: {msg}");
    let expected_extra = std::path::Path::new("src")
        .join("inclean.toml")
        .display()
        .to_string();
    assert!(msg.contains(&expected_extra), "got: {msg}");

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn run_defaults_project_root_to_dot_when_field_omitted() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    fs::write(
        dst.join("inclean.toml"),
        r#"
        [project]

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

#[test]
fn run_resolves_project_root_from_field_value() {
    // The config file lives at <dst>/inclean.toml but points at <dst>/lib
    // via `[project].root = "lib"`. `Summary.project_root` must reflect
    // the resolved root.
    let dst = tmp();
    fs::create_dir_all(dst.join("lib/src")).unwrap();
    fs::create_dir_all(dst.join("lib/include/mylib/internal")).unwrap();
    fs::write(dst.join("lib/include/mylib/internal/foo.h"), "").unwrap();
    fs::write(
        dst.join("lib/src/main.c"),
        "#include \"foo.h\"\nint main(void){return 0;}\n",
    )
    .unwrap();
    fs::write(
        dst.join("inclean.toml"),
        r#"
        [project]
        root = "lib"

        [[rule]]
        name = "base"
        paths = ["src/**", "include/**"]
        forms = ["quote"]
        allowed_include_dirs = ["include"]
        original_include_dirs = ["include/mylib/internal"]
        "#,
    )
    .unwrap();

    let summary = pipe::run(&dst, CheckMode::Full).unwrap();
    assert_eq!(
        summary.project_root,
        fs::canonicalize(dst.join("lib")).unwrap()
    );
    // The source file lives under the resolved root and got rewritten.
    let written = pipe::apply(&summary).unwrap();
    assert_eq!(written, 1);
    let rewritten = fs::read_to_string(dst.join("lib/src/main.c")).unwrap();
    assert!(
        rewritten.contains("\"mylib/internal/foo.h\""),
        "got: {rewritten}"
    );

    fs::remove_dir_all(&dst).ok();
}

#[test]
fn run_walks_up_from_deep_starting_path() {
    let src = fixture_path("flat-library");
    let dst = tmp();
    copy_dir(&src, &dst);

    // Pass a deep directory inside the project — the walker should still
    // find the root inclean.toml at <dst>.
    let summary = pipe::run(&dst.join("src"), CheckMode::Config).unwrap();
    assert_eq!(summary.project_root, fs::canonicalize(&dst).unwrap());

    fs::remove_dir_all(&dst).ok();
}
