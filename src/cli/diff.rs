//! `inclean diff <DIR>` — render a unified diff of would-be rewrites.

use std::path::PathBuf;

use anyhow::Result;

use crate::pipeline::run;

pub fn run(dir: PathBuf, _validate: bool) -> Result<u8> {
    let summary = run::run(&dir)?;
    for w in &summary.config_warnings {
        eprintln!("warning: {w}");
    }
    let d = run::render_diff(&summary);
    print!("{d}");
    Ok(run::summary_exit_code(&summary))
}
