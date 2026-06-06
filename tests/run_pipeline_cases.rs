//! Pipeline-result fixture driver for cases that need to assert failures,
//! exit codes, and unfixable diagnostics in addition to optional apply output.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use inclean::pipeline::run as pipe;
use libtest_mimic::{Arguments, Failed, Trial};
use serde::Deserialize;
use similar::{ChangeTag, TextDiff};

mod support;

fn main() -> ExitCode {
    let args = Arguments::from_args();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases_root = manifest.join("tests/pipeline_cases");

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
    expected: Option<PathBuf>,
    spec: CaseSpec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseSpec {
    exit_code: u8,
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    summary: SummarySpec,
    #[serde(default)]
    unfixable: Vec<UnfixableSpec>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummarySpec {
    conflicts: Option<usize>,
    unfixable: Option<usize>,
    rewritten_files: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnfixableSpec {
    path: String,
    line: usize,
    kind: String,
    #[serde(default)]
    message_contains: Vec<String>,
    #[serde(default)]
    differs_in: Vec<String>,
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
        let spec_path = dir.join("case.toml");
        assert!(
            input.is_dir(),
            "pipeline case `{name}` missing `input/` dir at {}",
            input.display()
        );
        assert!(
            spec_path.is_file(),
            "pipeline case `{name}` missing case.toml at {}",
            spec_path.display()
        );
        let spec_text = fs::read_to_string(&spec_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", spec_path.display()));
        let spec = toml::from_str::<CaseSpec>(&spec_text)
            .unwrap_or_else(|e| panic!("parse {}: {e}", spec_path.display()));
        cases.push(Case {
            name,
            input,
            expected: expected.is_dir().then_some(expected),
            spec,
        });
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

fn run_case(case: &Case) -> Result<(), Failed> {
    let workdir = support::fs::TmpDir::create_by_label(&case.name);
    let dirpath = workdir.path();
    support::fs::copy_dir(&case.input, dirpath);

    let summary = pipe::run(None, dirpath, &[], None, pipe::CheckMode::Run)
        .map_err(|e| format!("pipe::run: {e:#}"))?;
    let exit_code = pipe::summary_exit_code(&summary);
    if exit_code != case.spec.exit_code {
        return Err(Failed::from(format!(
            "exit_code mismatch: expected {}, got {}\n{}",
            case.spec.exit_code,
            exit_code,
            pipe::render_unfixable_report(&summary)
        )));
    }
    assert_summary(&case.spec.summary, &summary)?;
    assert_unfixable(&case.spec.unfixable, &summary)?;

    if case.spec.apply {
        pipe::apply(&summary).map_err(|e| format!("pipe::apply: {e:#}"))?;
        let expected = case
            .expected
            .as_ref()
            .ok_or_else(|| Failed::from("case has apply = true but no expected/ directory"))?;
        compare_trees(dirpath, expected).map_err(Failed::from)?;
    } else if let Some(expected) = &case.expected {
        compare_trees(dirpath, expected).map_err(Failed::from)?;
    }

    Ok(())
}

fn assert_summary(spec: &SummarySpec, summary: &pipe::Summary) -> Result<(), Failed> {
    if let Some(expected) = spec.conflicts
        && summary.conflicts.len() != expected
    {
        return Err(Failed::from(format!(
            "summary.conflicts mismatch: expected {}, got {}",
            expected,
            summary.conflicts.len()
        )));
    }
    if let Some(expected) = spec.unfixable
        && summary.unfixable.len() != expected
    {
        return Err(Failed::from(format!(
            "summary.unfixable mismatch: expected {}, got {}",
            expected,
            summary.unfixable.len()
        )));
    }
    if let Some(expected) = spec.rewritten_files {
        let actual = summary
            .files
            .iter()
            .filter(|file| file.rewritten.is_some())
            .count();
        if actual != expected {
            return Err(Failed::from(format!(
                "summary rewritten_files mismatch: expected {expected}, got {actual}"
            )));
        }
    }
    Ok(())
}

fn assert_unfixable(specs: &[UnfixableSpec], summary: &pipe::Summary) -> Result<(), Failed> {
    for spec in specs {
        let detail = summary
            .unfixable
            .iter()
            .find(|detail| {
                detail.file_relpath == Path::new(&spec.path)
                    && detail.line == spec.line
                    && unfixable_kind_name(detail.kind) == spec.kind
            })
            .ok_or_else(|| {
                Failed::from(format!(
                    "missing unfixable: path={} line={} kind={}",
                    spec.path, spec.line, spec.kind
                ))
            })?;

        let message = detail.message.as_deref().unwrap_or("");
        for expected in &spec.message_contains {
            if !message.contains(expected) {
                return Err(Failed::from(format!(
                    "unfixable message for {}:{} does not contain {:?}; got {:?}",
                    spec.path, spec.line, expected, message
                )));
            }
        }

        let actual_aspects: Vec<&str> = detail
            .differing_aspects
            .iter()
            .map(diff_aspect_name)
            .collect();
        for expected in &spec.differs_in {
            if !actual_aspects.contains(&expected.as_str()) {
                return Err(Failed::from(format!(
                    "unfixable differs_in for {}:{} missing {:?}; got {:?}",
                    spec.path, spec.line, expected, actual_aspects
                )));
            }
        }
    }
    Ok(())
}

fn unfixable_kind_name(kind: pipe::UnfixableKind) -> &'static str {
    match kind {
        pipe::UnfixableKind::Error => "error",
        pipe::UnfixableKind::EvaluationFailure => "evaluation_failure",
        pipe::UnfixableKind::Conflict => "conflict",
        pipe::UnfixableKind::TrailingCommentError => "trailing_comment_error",
    }
}

fn diff_aspect_name(aspect: &pipe::DiffAspect) -> &'static str {
    match aspect {
        pipe::DiffAspect::IncludePath => "include path",
        pipe::DiffAspect::OutputForm => "output_form",
        pipe::DiffAspect::TrailingComment => "trailing_comment",
    }
}

/// Strict tree equality, modulo `inclean.toml` (skipped on both sides).
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
