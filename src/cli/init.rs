//! `inclean init <DIR>` — write a documented starter `inclean.toml` into
//! the given directory. Refuses to overwrite an existing file.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::discover::CONFIG_FILENAME;

/// `#:schema` header injected into the generated file so editors (VS Code's
/// Even Better TOML, Helix, Zed, …) pick up validation and completion. The
/// URL is pinned to the CLI version that wrote the file, so future schema
/// changes don't silently invalidate this config.
const SCHEMA_HEADER: &str = concat!(
    "#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v",
    env!("CARGO_PKG_VERSION"),
    "/schemas/inclean.toml.schema.json",
);

const TEMPLATE: &str = include_str!("template.inclean.toml");

pub fn run(dir: PathBuf) -> Result<u8> {
    std::fs::create_dir_all(&dir).with_context(|| format!("ensuring {} exists", dir.display()))?;
    let target = dir.join(CONFIG_FILENAME);
    if target.exists() {
        anyhow::bail!(
            "{} already exists; remove it first or pick a different directory",
            target.display()
        );
    }
    let contents = construct_inclean_toml_template();
    std::fs::write(&target, contents).with_context(|| format!("writing {}", target.display()))?;
    println!("wrote {}", target.display());
    println!("next:");
    println!("  edit {} to taste, then run:", target.display());
    println!("    inclean check --level config {}", dir.display());
    println!("    inclean check {}", dir.display());
    Ok(0)
}

fn construct_inclean_toml_template() -> String {
    let body_start = TEMPLATE
        .find('\n')
        .expect("remove_first_line: no newline found");
    let body =
        TEMPLATE[body_start + 1..].replace("{{CARGO_PKG_VERSION}}", env!("CARGO_PKG_VERSION"));
    format!("{SCHEMA_HEADER}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "inclean-init-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst),
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn writes_schema_pinned_to_cli_version() {
        let dir = scratch_dir();
        run(dir.clone()).unwrap();
        let body = std::fs::read_to_string(dir.join(CONFIG_FILENAME)).unwrap();
        let first_line = body.lines().next().expect("file is non-empty");
        assert!(
            first_line
                .starts_with("#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v"),
            "unexpected schema directive: {first_line}"
        );
        assert!(
            first_line.contains(env!("CARGO_PKG_VERSION")),
            "schema URL should embed CLI version {}: {first_line}",
            env!("CARGO_PKG_VERSION"),
        );
        assert!(
            first_line.ends_with("/schemas/inclean.toml.schema.json"),
            "schema URL should end with /schemas/inclean.toml.schema.json: {first_line}"
        );
    }

    #[test]
    fn writes_version_field_pinned_to_cli_version() {
        let dir = scratch_dir();
        run(dir.clone()).unwrap();
        let body = std::fs::read_to_string(dir.join(CONFIG_FILENAME)).unwrap();
        let expected = format!("version = \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(
            body.contains(&expected),
            "expected `{expected}` in generated config; got:\n{body}"
        );
        assert!(
            !body.contains("{{CARGO_PKG_VERSION}}"),
            "placeholder was not substituted; got:\n{body}"
        );
    }
}
