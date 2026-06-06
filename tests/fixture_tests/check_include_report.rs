use std::process::Command;

use crate::support;

const MATCHED_INCLUDE_OUTPUT: &str = "\"matched.h\"";
const UNMATCHED_INCLUDE_OUTPUT: &str = "#include \"unmatched.h\"";
const MATCHED_RULE_OUTPUT: &str = "rules: matched";
const SKIPPED_INCLUDE_OUTPUT: &str = "\"skipped.h\"";

#[test]
fn check_defaults_to_matched_includes_only() {
    let project = create_project();
    let out = run_check(project.path(), &[]);

    assert!(
        out.status.success(),
        "check failed: {}",
        render_output(&out)
    );
    assert_stdout_contains(&out, MATCHED_INCLUDE_OUTPUT);
    assert_stdout_contains(&out, MATCHED_RULE_OUTPUT);
    assert_stdout_not_contains(&out, UNMATCHED_INCLUDE_OUTPUT);
}

#[test]
fn check_can_show_unmatched_includes_too() {
    let project = create_project();
    let out = run_check(project.path(), &["--show-unmatched"]);

    assert!(
        out.status.success(),
        "check failed: {}",
        render_output(&out)
    );
    assert_stdout_contains(&out, MATCHED_INCLUDE_OUTPUT);
    assert_stdout_contains(&out, MATCHED_RULE_OUTPUT);
    assert_stdout_contains(&out, UNMATCHED_INCLUDE_OUTPUT);
}

#[test]
fn check_can_show_only_unmatched_includes() {
    let project = create_project();
    let out = run_check(project.path(), &["--only-unmatched"]);

    assert!(
        out.status.success(),
        "check failed: {}",
        render_output(&out)
    );
    assert_stdout_not_contains(&out, MATCHED_INCLUDE_OUTPUT);
    assert_stdout_contains(&out, UNMATCHED_INCLUDE_OUTPUT);
}

#[test]
fn check_hides_skipped_includes() {
    let project = create_skip_project();
    let default = run_check(project.path(), &[]);
    let show_unmatched = run_check(project.path(), &["--show-unmatched"]);

    assert!(
        default.status.success(),
        "default check failed: {}",
        render_output(&default)
    );
    assert!(
        show_unmatched.status.success(),
        "show unmatched check failed: {}",
        render_output(&show_unmatched)
    );
    assert_stdout_not_contains(&default, SKIPPED_INCLUDE_OUTPUT);
    assert_stdout_not_contains(&default, UNMATCHED_INCLUDE_OUTPUT);
    assert_stdout_not_contains(&show_unmatched, SKIPPED_INCLUDE_OUTPUT);
    assert_stdout_contains(&show_unmatched, UNMATCHED_INCLUDE_OUTPUT);
}

#[test]
fn check_all_can_show_unmatched_includes_too() {
    let project = create_project();
    let bare = run_check(project.path(), &["--show-unmatched"]);
    let all = run_check(project.path(), &["all", "--show-unmatched"]);

    assert!(
        bare.status.success(),
        "bare check failed: {}",
        render_output(&bare)
    );
    assert!(
        all.status.success(),
        "check all failed: {}",
        render_output(&all)
    );
    assert_eq!(stdout(&bare), stdout(&all));
}

#[test]
fn check_unfixable_does_not_accept_unmatched_include_flags() {
    let project = create_project();
    let out = run_check(project.path(), &["unfixable", "--show-unmatched"]);

    assert!(!out.status.success(), "check unexpectedly passed");
    assert_stderr_contains(&out, "unexpected argument");
    assert_stderr_contains(&out, "--show-unmatched");
}

fn create_project() -> support::fs::TmpProject {
    let config = format!(
        r#"[project]
root = "."
version = "{}"
min_inclean_version = "{}"

[[rule]]
name = "matched"
file_paths = ["src/**/*"]
include_match = ["matched.h"]
action = {{ type = "keep" }}
"#,
        inclean::profile::CFG_VERSION,
        inclean::profile::MIN_COMPAT_CLI_VERSION
    );
    let project = support::fs::TmpProject::create_with_config(config);
    project.write(
        "src/main.c",
        "#include \"matched.h\"\n#include \"unmatched.h\"\nint main(void) { return 0; }\n",
    );
    project
}

fn create_skip_project() -> support::fs::TmpProject {
    let config = format!(
        r#"[project]
root = "."
version = "{}"
min_inclean_version = "{}"

[[rule]]
name = "skipped"
file_paths = ["src/**/*"]
include_match = ["skipped.h"]
"#,
        inclean::profile::CFG_VERSION,
        inclean::profile::MIN_COMPAT_CLI_VERSION
    );
    let project = support::fs::TmpProject::create_with_config(config);
    project.write(
        "src/main.c",
        "#include \"skipped.h\"\n#include \"unmatched.h\"\nint main(void) { return 0; }\n",
    );
    project
}

fn run_check(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_inclean");
    let mut cmd = Command::new(bin);
    cmd.arg("check").args(args).arg(root).env("NO_COLOR", "1");
    cmd.output().unwrap()
}

fn assert_stdout_contains(out: &std::process::Output, needle: &str) {
    let stdout = stdout(out);
    assert!(
        stdout.contains(needle),
        "stdout did not contain {needle:?}: {}",
        render_output(out)
    );
}

fn assert_stdout_not_contains(out: &std::process::Output, needle: &str) {
    let stdout = stdout(out);
    assert!(
        !stdout.contains(needle),
        "stdout unexpectedly contained {needle:?}: {}",
        render_output(out)
    );
}

fn assert_stderr_contains(out: &std::process::Output, needle: &str) {
    let stderr = stderr(out);
    assert!(
        stderr.contains(needle),
        "stderr did not contain {needle:?}: {}",
        render_output(out)
    );
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn render_output(out: &std::process::Output) -> String {
    format!("stdout={:?} stderr={:?}", stdout(out), stderr(out))
}
