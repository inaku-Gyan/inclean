//! `inclean diff <DIR>` — render a unified diff of would-be rewrites.
//! Always runs the full pipeline (rule-tree conflict check +
//! `allowed_include_dirs` validation). Non-zero exit code if anything
//! about the pipeline would block apply.

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run::{self, CheckMode};

pub fn run(dir: PathBuf) -> Result<u8> {
    let summary = run::run(&dir, CheckMode::Full)?;
    let d = run::render_diff(&summary);
    print!("{d}");
    Ok(run::summary_exit_code(&summary))
}
