//! Synthetic performance benchmarks. These are `#[ignore]`d so a plain
//! `cargo test` skips them; run them explicitly with:
//!
//!     cargo test --release --test perf -- --ignored --nocapture
//!
//! Each test generates a fake C library of `N` files and times the full
//! pipeline. Output is printed via `println!` so `--nocapture` is needed
//! to see it.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use inclean::pipeline::run as pipe;
use pipe::CheckMode;

/// Generate a synthetic project with `n_dirs` internal directories and
/// `headers_per_dir` headers in each, plus one source file per directory
/// that #includes each of the local headers by bare name. Total files:
/// `n_dirs * (headers_per_dir + 1) + 1` (the +1 is the inclean.toml).
fn make_project(root: &Path, n_dirs: usize, headers_per_dir: usize) -> usize {
    fs::create_dir_all(root).unwrap();
    fs::create_dir_all(root.join("include")).unwrap();

    // Public headers so allowed_include_dirs has somewhere to land them.
    for d in 0..n_dirs {
        fs::create_dir_all(root.join(format!("include/internal/d{d}"))).unwrap();
        for h in 0..headers_per_dir {
            fs::write(root.join(format!("include/internal/d{d}/h{h}.h")), "").unwrap();
        }
    }

    // Mirror the layout under src/ — sources #include their siblings by
    // bare basename so `auto` has work to do.
    let mut total = 0usize;
    for d in 0..n_dirs {
        let dir = root.join(format!("src/d{d}"));
        fs::create_dir_all(&dir).unwrap();
        for h in 0..headers_per_dir {
            fs::write(dir.join(format!("h{h}.h")), "").unwrap();
            total += 1;
        }
        let mut c = String::new();
        for h in 0..headers_per_dir {
            c.push_str(&format!("#include \"h{h}.h\"\n"));
        }
        c.push_str("int main(){return 0;}\n");
        fs::write(dir.join("main.c"), c).unwrap();
        total += 1;
    }

    // The on-disk layout has src/d*/h*.h but the canonical "allowed" form
    // we want post-rewrite is include/internal/d*/h*.h. The auto action
    // can't synthesize that since the resolved file lives under src/, so
    // we use rewrite with a template that references the include text.
    let toml = r#"
        [project]
        root = "."

        [[rule]]
        name = "base"
        paths = ["src/**"]
        forms = ["quote"]
        allowed_include_dirs = ["include"]
        original_include_dirs = ["src/d0", "src/d1", "src/d2", "src/d3", "src/d4"]
        action = { type = "keep" }
    "#;
    fs::write(root.join("inclean.toml"), toml).unwrap();

    total
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("inclean-perf-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
#[ignore]
fn perf_10k_files() {
    // 1000 directories × 10 headers + 1000 sources = 11_000 files.
    let root = tmp("10k");
    let total = make_project(&root, 1000, 10);
    println!("generated {total} files at {}", root.display());

    let start = Instant::now();
    let summary = pipe::run(&root, CheckMode::Full).unwrap();
    let elapsed = start.elapsed();

    println!(
        "Full pipeline over {} files: {:?}  (mode=Full, {} include results, {} conflicts)",
        summary.files.len(),
        elapsed,
        summary
            .files
            .iter()
            .map(|f| f.include_results.len())
            .sum::<usize>(),
        summary.conflicts.len(),
    );
    assert!(elapsed.as_secs() < 30, "10k-file run took longer than 30s");

    fs::remove_dir_all(&root).ok();
}

#[test]
#[ignore]
fn perf_10k_files_rules_mode() {
    // Same fixture, Rules mode (no action evaluation). Useful to see the
    // marginal cost of action + allowed-dirs validation.
    let root = tmp("10k-rules");
    make_project(&root, 1000, 10);

    let start = Instant::now();
    let summary = pipe::run(&root, CheckMode::Rules).unwrap();
    let elapsed = start.elapsed();
    println!(
        "Rules pipeline over {} files: {:?}",
        summary.files.len(),
        elapsed
    );
    fs::remove_dir_all(&root).ok();
}
