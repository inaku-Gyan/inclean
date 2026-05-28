//! Golden-test driver for inclean. Discovers every case under
//! `tests/golden/<name>/`, copies each `input/` tree into
//! `tests/.workdir/<name>/`, runs `pipe::run(Full) + pipe::apply`, then
//! asserts strict tree equality with `<name>/expected/` — skipping any
//! file named `inclean.toml` on either side, since the config is a
//! driver artifact rather than a rewrite target.
//!
//! Each case directory shows up as its own line in `cargo test --test
//! golden` output. To add a case: drop a new `tests/golden/<name>/`
//! with `input/` and `expected/`. No `case.toml`, no per-case knobs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use inclean::pipeline::run as pipe;
use libtest_mimic::{Arguments, Failed, Trial};
use pipe::CheckMode;
use similar::{ChangeTag, TextDiff};

mod support;

fn main() -> ExitCode {
    let args = Arguments::from_args();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases_root = manifest.join("tests/golden_tests");

    let cases = discover_cases(&cases_root);
    let trials: Vec<Trial> = cases
        .into_iter()
        .map(|case| {
            let name = case.name.clone();
            Trial::test(name, move || run_case(&case))
        })
        .collect();

    libtest_mimic::run(&args, trials).exit_code()
}

struct Case {
    name: String,
    input: PathBuf,
    expected: PathBuf,
}

fn discover_cases(root: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    let entries = fs::read_dir(root).unwrap_or_else(|e| panic!("read {}: {e}", root.display()));
    for entry in entries {
        let entry = entry.expect("read_dir entry");
        if !entry.file_type().expect("file_type").is_dir() {
            continue;
        }
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let input = dir.join("input");
        let expected = dir.join("expected");
        assert!(
            input.is_dir(),
            "golden case `{name}` missing `input/` dir at {}",
            input.display()
        );
        assert!(
            expected.is_dir(),
            "golden case `{name}` missing `expected/` dir at {}",
            expected.display()
        );
        cases.push(Case {
            name,
            input,
            expected,
        });
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

fn run_case(case: &Case) -> Result<(), Failed> {
    let workdir = support::fs::TmpDir::create_by_label(&case.name);
    let dirpath = workdir.path();
    support::fs::copy_dir(&case.input, &dirpath);

    let summary = pipe::run(None, &dirpath, &[], None, CheckMode::Run)
        .map_err(|e| format!("pipe::run: {e:#}"))?;
    pipe::apply(&summary).map_err(|e| format!("pipe::apply: {e:#}"))?;

    compare_trees(&dirpath, &case.expected).map_err(Failed::from)?;
    Ok(())
}

/// Strict tree equality, modulo `inclean.toml` (skipped on both sides).
/// Mismatches surface as a single error message with a unified diff.
fn compare_trees(actual_root: &Path, expected_root: &Path) -> Result<(), String> {
    let mut actual = list_files(actual_root)
        .map_err(|e| format!("walk workdir {}: {e}", actual_root.display()))?;
    let mut expected = list_files(expected_root)
        .map_err(|e| format!("walk expected {}: {e}", expected_root.display()))?;
    actual.sort();
    expected.sort();

    if let Some(missing) = expected.iter().find(|p| !actual.contains(p)) {
        return Err(format!(
            "missing in workdir: {} (declared in expected/)",
            missing.display()
        ));
    }
    if let Some(extra) = actual.iter().find(|p| !expected.contains(p)) {
        return Err(format!(
            "unexpected file in workdir: {} (not in expected/)",
            extra.display()
        ));
    }

    for rel in &actual {
        let a = fs::read(actual_root.join(rel))
            .map_err(|e| format!("read workdir/{}: {e}", rel.display()))?;
        let b = fs::read(expected_root.join(rel))
            .map_err(|e| format!("read expected/{}: {e}", rel.display()))?;
        if a != b {
            return Err(format!(
                "mismatch at {}\n{}",
                rel.display(),
                render_diff(&b, &a)
            ));
        }
    }
    Ok(())
}

fn list_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(root, &path, out)?;
        } else if ft.is_file() {
            if entry.file_name() == "inclean.toml" {
                continue;
            }
            let rel = path.strip_prefix(root).expect("strip_prefix").to_path_buf();
            out.push(rel);
        }
    }
    Ok(())
}

fn render_diff(expected: &[u8], actual: &[u8]) -> String {
    let exp = String::from_utf8_lossy(expected);
    let act = String::from_utf8_lossy(actual);
    let diff = TextDiff::from_lines(exp.as_ref(), act.as_ref());
    let mut out = String::new();
    out.push_str("--- expected\n+++ actual\n");
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        out.push_str(sign);
        out.push_str(change.value());
        if !change.value().ends_with('\n') {
            out.push('\n');
        }
    }
    out
}
