//! `inclean lint <DIR>` — validate inclean.toml configuration only.
//!
//! Performs every load-time check (TOML parse, structural invariants,
//! `extends` resolution, cycle detection, constant expansion) but never
//! reads or rewrites any source file.

use std::path::PathBuf;

use anyhow::Result;

use crate::config::{discover, inherit};

pub fn run(dir: PathBuf) -> Result<u8> {
    let configs = discover::load_all_configs(&dir)?;
    discover::validate_loaded(&configs, &dir)?;
    let resolved = inherit::resolve(&configs)?;

    println!(
        "ok: loaded {} config file(s), {} rule(s)",
        configs.len(),
        resolved.len()
    );
    for cfg in &configs {
        println!("  config: {}", cfg.path.display());
    }
    for (name, rule) in &resolved {
        let extends = rule
            .extends
            .as_deref()
            .map(|p| format!(" extends `{p}`"))
            .unwrap_or_default();
        println!(
            "  rule:   `{name}`{extends}  ({} :: #{})",
            rule.origin.config_path.display(),
            rule.origin.index
        );
    }
    Ok(0)
}
