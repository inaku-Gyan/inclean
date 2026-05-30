//! `inclean init <PATH>` / `inclean config new <PATH>` — write a
//! documented starter `inclean.toml`. PATH semantics:
//!
//! - existing directory → create `inclean.toml` inside it
//! - existing file → error
//! - nonexistent path ending in `/` or that looks like a directory
//!   (no extension) → mkdirp the directory and create `inclean.toml`
//!   inside
//! - nonexistent path with a file-like name → mkdirp the parent and
//!   create the file at that path
//! - PATH omitted (caller passed `"."`) → equivalent to "current dir"

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::style as cli_style;
use crate::profile::{CFG_VERSION, CLI_VERSION, CONFIG_FILENAME, MIN_COMPAT_CLI_VERSION};

const TEMPLATE: &str = include_str!("template.inclean.toml");

pub fn run(path: Option<&Path>) -> Result<u8> {
    let path = path.unwrap_or_else(|| Path::new("."));
    let target = resolve_target(path);
    if target.exists() {
        anyhow::bail!(
            "{} already exists; remove it first or pick a different path",
            target.display()
        );
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let contents = construct_inclean_toml_template();
    std::fs::write(&target, contents).with_context(|| format!("writing {}", target.display()))?;
    println!(
        "{} {}",
        cli_style::success("wrote"),
        cli_style::path(target.display())
    );
    println!("{}", cli_style::status("next:"));
    println!(
        "  edit {} to taste, then run:",
        cli_style::path(target.display())
    );
    println!(
        "    {}",
        cli_style::command(format!(
            "inclean config check {}",
            target.parent().unwrap_or(Path::new(".")).display()
        ))
    );
    println!(
        "    {}",
        cli_style::command(format!(
            "inclean check {}",
            target.parent().unwrap_or(Path::new(".")).display()
        ))
    );
    Ok(0)
}

/// Apply the PATH-resolution rules from the module docstring.
fn resolve_target(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.join(CONFIG_FILENAME);
    }
    if path.is_file() {
        return path.to_path_buf();
    }
    // Doesn't exist yet — decide between "create file at this path" vs
    // "treat as new directory + create CONFIG_FILENAME inside".
    use crate::utils::PathExt;
    if path.looks_like_directory() {
        path.join(CONFIG_FILENAME)
    } else {
        path.to_path_buf()
    }
}

fn construct_inclean_toml_template() -> String {
    let body_start = TEMPLATE
        .find('\n')
        .expect("template: must have at least one newline");

    let body = TEMPLATE[body_start + 1..]
        .replace("{{CFG_VERSION}}", CFG_VERSION)
        .replace("{{MIN_COMPAT_CLI_VERSION}}", MIN_COMPAT_CLI_VERSION);

    // `#:schema` header injected into the generated file so editors pick up
    // validation. Pinned to the CLI version that wrote the file.
    format!(
        "#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v{CLI_VERSION}/schemas/inclean.toml.schema.json\n{body}"
    )
}

#[cfg(test)]
mod tests {
    use crate::utils::testing::fs::{TmpDir, TmpProject};

    use super::*;

    #[test]
    fn writes_into_existing_dir() {
        let dir = TmpDir::new();
        run(Some(dir.path())).unwrap();
        let target = dir.path().join(CONFIG_FILENAME);
        assert!(target.exists());
    }

    #[test]
    fn refuses_to_overwrite_existing_file() {
        let dir = TmpProject::create_with_min_config();
        let err = run(Some(dir.path())).unwrap_err();
        assert!(format!("{err:#}").contains("already exists"));
    }

    #[test]
    fn nonexistent_directory_like_path_creates_dir_and_file() {
        let dir = TmpDir::new();
        let new_subdir = dir.path().join("new-sub-dir");
        run(Some(&new_subdir)).unwrap();
        assert!(new_subdir.join(CONFIG_FILENAME).exists());
    }

    #[test]
    fn nonexistent_file_like_path_creates_file_at_path() {
        let dir = TmpDir::new();
        let target = dir.path().join("custom.toml");
        run(Some(&target)).unwrap();
        assert!(target.exists());
    }
}
