//! `inclean diff` — render a unified diff of would-be rewrites.

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run::{self, CheckMode};

pub fn run(dir: PathBuf) -> Result<u8> {
    let summary = run::run(None, &dir, &[], None, CheckMode::Run)?;
    let d = run::render_diff(&summary);
    print!("{d}");
    Ok(run::summary_exit_code(&summary))
}
