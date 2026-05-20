//! `inclean validate <DIR>` — verify the inclean.toml configuration only.
//!
//! Checks every load-time invariant: TOML syntax, the [project].root sigil,
//! sub-config restrictions, global rule-name uniqueness, the `extends` graph
//! (existence and acyclicity), `@std.*` constant references, and the v1
//! exclusions (e.g. layer-5 `match_resolved`). Never reads or rewrites any
//! source file.

use std::path::PathBuf;

use anyhow::Result;

use crate::config::{discover, inherit, lint};

pub fn run(dir: PathBuf) -> Result<u8> {
    let configs = discover::load_all_configs(&dir)?;
    discover::validate_loaded(&configs, &dir)?;
    let resolved = inherit::resolve(&configs)?;
    let warnings = lint::check(&resolved);

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

    if !warnings.is_empty() {
        eprintln!();
        for w in &warnings {
            eprintln!("warning: {w}");
        }
    }
    Ok(0)
}
