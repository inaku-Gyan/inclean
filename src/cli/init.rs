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

use crate::config::discover::CONFIG_FILENAME;

/// `#:schema` header injected into the generated file so editors pick up
/// validation. Pinned to the CLI version that wrote the file.
const SCHEMA_HEADER: &str = concat!(
    "#:schema https://raw.githubusercontent.com/inaku-Gyan/inclean/v",
    env!("CARGO_PKG_VERSION"),
    "/schemas/inclean.toml.schema.json",
);

const TEMPLATE: &str = include_str!("template.inclean.toml");

pub fn run(path: PathBuf) -> Result<u8> {
    let target = resolve_target(&path)?;
    if target.exists() {
        anyhow::bail!(
            "{} already exists; remove it first or pick a different path",
            target.display()
        );
    }
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let contents = construct_inclean_toml_template();
    std::fs::write(&target, contents).with_context(|| format!("writing {}", target.display()))?;
    println!("wrote {}", target.display());
    println!("next:");
    println!("  edit {} to taste, then run:", target.display());
    println!("    inclean config check {}", target.parent().unwrap_or(Path::new(".")).display());
    println!("    inclean check {}", target.parent().unwrap_or(Path::new(".")).display());
    Ok(0)
}

/// Apply the PATH-resolution rules from the module docstring.
fn resolve_target(path: &Path) -> Result<PathBuf> {
    if path.is_dir() {
        return Ok(path.join(CONFIG_FILENAME));
    }
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    // Doesn't exist yet — decide between "create file at this path" vs
    // "treat as new directory + create CONFIG_FILENAME inside".
    let looks_like_dir = path_looks_like_directory(path);
    if looks_like_dir {
        Ok(path.join(CONFIG_FILENAME))
    } else {
        Ok(path.to_path_buf())
    }
}

fn path_looks_like_directory(path: &Path) -> bool {
    // Trailing slash → directory.
    let as_str = path.as_os_str().to_string_lossy();
    if as_str.ends_with('/') || as_str.ends_with(std::path::MAIN_SEPARATOR) {
        return true;
    }
    // No extension → directory (e.g. `lib`, `foo/bar`).
    path.extension().is_none()
}

fn construct_inclean_toml_template() -> String {
    let body_start = TEMPLATE
        .find('\n')
        .expect("template: must have at least one newline");
    let body = TEMPLATE[body_start + 1..].replace("{{CARGO_PKG_VERSION}}", env!("CARGO_PKG_VERSION"));
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
    fn writes_into_existing_dir() {
        let dir = scratch_dir();
        run(dir.clone()).unwrap();
        let target = dir.join(CONFIG_FILENAME);
        assert!(target.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refuses_to_overwrite_existing_file() {
        let dir = scratch_dir();
        let target = dir.join(CONFIG_FILENAME);
        std::fs::write(&target, "pre-existing").unwrap();
        let err = run(dir.clone()).unwrap_err();
        assert!(format!("{err:#}").contains("already exists"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nonexistent_directory_like_path_creates_dir_and_file() {
        let dir = scratch_dir();
        let new_subdir = dir.join("new-sub-dir");
        run(new_subdir.clone()).unwrap();
        assert!(new_subdir.join(CONFIG_FILENAME).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nonexistent_file_like_path_creates_file_at_path() {
        let dir = scratch_dir();
        let target = dir.join("custom.toml");
        run(target.clone()).unwrap();
        assert!(target.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn template_substitutes_cargo_version() {
        let dir = scratch_dir();
        run(dir.clone()).unwrap();
        let body = std::fs::read_to_string(dir.join(CONFIG_FILENAME)).unwrap();
        let expected = format!("version = \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(body.contains(&expected), "got: {body}");
        assert!(!body.contains("{{CARGO_PKG_VERSION}}"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn template_includes_min_inclean_version_pinned_to_cli() {
        let dir = scratch_dir();
        run(dir.clone()).unwrap();
        let body = std::fs::read_to_string(dir.join(CONFIG_FILENAME)).unwrap();
        let expected = format!("min_inclean_version = \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(body.contains(&expected), "got: {body}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
