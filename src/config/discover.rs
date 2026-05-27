//! Locate, load, and validate the single `inclean.toml` for a project.
//!
//! Entry points:
//! - [`find_root_config`] climbs from a starting directory looking for the
//!   nearest `inclean.toml`.
//! - [`load_root_config`] reads, parses, and runs the two-direction
//!   version check.
//! - [`resolve_project_root`] joins the file's directory with `[project].root`
//!   (default `"."`) to produce the actual project root.
//! - [`assert_no_extra_configs`] errors if any other `inclean.toml` exists
//!   under the resolved project root — sub-configs are not a feature.
//!
//! Tree-walking is unfiltered — per refactor.md §Engine "所有忽略和包含
//! 文件都由配置文件显示指定。不要自动加料"; we do NOT honor `.gitignore`
//! or skip `.git` / `target` / `node_modules` implicitly. Stray
//! `inclean.toml` files anywhere under the resolved project root are
//! flagged regardless of where they sit.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

use super::schema::{self, LoadedConfig, RawProject};

pub const CONFIG_FILENAME: &str = "inclean.toml";

/// This CLI's own version (from `Cargo.toml`).
fn cli_current() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("CARGO_PKG_VERSION must be valid semver")
}

/// Minimum config `version` this CLI can read. Bump when a breaking
/// schema change lands. Compared against `[project].version`.
///
/// Semantics: `CLI_COMPAT_MIN <= config.version` must hold. If the user's
/// config was written before this CLI's last breaking change, they need
/// to update the config first.
pub const CLI_COMPAT_MIN: &str = "0.3.0";

fn cli_compat_min() -> semver::Version {
    semver::Version::parse(CLI_COMPAT_MIN).expect("CLI_COMPAT_MIN must be valid semver")
}

/// Climb from `start` (or its parent if it is a file) toward `/` looking
/// for the nearest `inclean.toml`.
pub fn find_root_config(start: &Path) -> Result<PathBuf> {
    let mut current = if start.is_file() {
        start
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        start.to_path_buf()
    };

    loop {
        let candidate = current.join(CONFIG_FILENAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !current.pop() {
            anyhow::bail!(
                "no {CONFIG_FILENAME} found in {} or any parent directory",
                start.display(),
            );
        }
    }
}

/// Load and parse the `inclean.toml` at `config_path`, then run the
/// two-direction version check. The root config must declare `[project]`
/// with both `version` and `min_inclean_version`.
pub fn load_root_config(config_path: &Path) -> Result<LoadedConfig> {
    let text = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let raw = schema::parse(&text, config_path)?;
    let project = raw.project.as_ref().ok_or_else(|| {
        anyhow::anyhow!("{} must declare a [project] block", config_path.display())
    })?;
    check_version_compatibility(config_path, project)?;
    Ok(LoadedConfig {
        path: config_path.to_path_buf(),
        raw,
    })
}

/// Two-direction version check:
/// - `CLI_COMPAT_MIN <= config.version` — the config wasn't written for a
///   CLI older than the last breaking change we still understand.
/// - `config.min_inclean_version <= cli.current` — the user said they need
///   at least that CLI version to parse correctly, and we are at least
///   that new.
///
/// Either failure is a hard error citing both sides.
fn check_version_compatibility(config_path: &Path, project: &RawProject) -> Result<()> {
    let cli_min = cli_compat_min();
    let cli_now = cli_current();

    let cfg_version_raw = project.version.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "{} must declare `version` in [project] (the CLI version that wrote this config)",
            config_path.display(),
        )
    })?;
    let cfg_version = semver::Version::parse(cfg_version_raw).with_context(|| {
        format!(
            "{}: [project].version = \"{}\" is not valid semver",
            config_path.display(),
            cfg_version_raw,
        )
    })?;

    let cfg_compat_min_raw = project.min_inclean_version.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "{} must declare `min_inclean_version` in [project] (lowest CLI version that can parse this config)",
            config_path.display(),
        )
    })?;
    let cfg_compat_min = semver::Version::parse(cfg_compat_min_raw).with_context(|| {
        format!(
            "{}: [project].min_inclean_version = \"{}\" is not valid semver",
            config_path.display(),
            cfg_compat_min_raw,
        )
    })?;

    if cfg_version < cli_min {
        anyhow::bail!(
            "{}: config was written for inclean {} but this CLI can only read configs >= {} \
             (the last breaking schema change). Update the config to the current schema and \
             set [project].version = \"{}\".",
            config_path.display(),
            cfg_version,
            cli_min,
            cli_now,
        );
    }
    if cfg_compat_min > cli_now {
        anyhow::bail!(
            "{}: config requires inclean >= {} but this CLI is {}. Upgrade the CLI.",
            config_path.display(),
            cfg_compat_min,
            cli_now,
        );
    }

    Ok(())
}

/// Compute the actual project root from `config_path` and `[project].root`.
pub fn resolve_project_root(config_path: &Path, project: &RawProject) -> Result<PathBuf> {
    let raw_value = project.root.as_deref().unwrap_or(".");
    let trimmed = raw_value.trim();
    if trimmed.is_empty() {
        anyhow::bail!(
            "{}: [project].root must be a non-empty path (omit the field to default to \".\")",
            config_path.display(),
        );
    }
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let joined = config_dir.join(trimmed);
    let canon = std::fs::canonicalize(&joined).with_context(|| {
        format!(
            "{}: [project].root = {:?} resolves to {} which does not exist",
            config_path.display(),
            raw_value,
            joined.display(),
        )
    })?;
    if !canon.is_dir() {
        anyhow::bail!(
            "{}: [project].root = {:?} resolves to {} which is not a directory",
            config_path.display(),
            raw_value,
            canon.display(),
        );
    }
    Ok(canon)
}

/// Walk `project_root` and error if any `inclean.toml` exists besides
/// `root_config_path`.
pub fn assert_no_extra_configs(project_root: &Path, root_config_path: &Path) -> Result<()> {
    let root_canon =
        std::fs::canonicalize(root_config_path).unwrap_or_else(|_| root_config_path.to_path_buf());
    let mut extras: Vec<PathBuf> = Vec::new();

    let walker = WalkBuilder::new(project_root)
        .standard_filters(false)
        .build();

    for entry in walker {
        let entry = entry.with_context(|| format!("walking {}", project_root.display()))?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        if entry.file_name() != CONFIG_FILENAME {
            continue;
        }
        let path = entry.into_path();
        let path_canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if path_canon != root_canon {
            extras.push(path);
        }
    }

    if !extras.is_empty() {
        let list = extras
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "found extra {CONFIG_FILENAME} file(s) under {}; sub-configs are not supported, consolidate all rules into the root config:\n{list}",
            project_root.display(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tree(files: &[(&str, &str)]) -> tempdir::Project {
        let project = tempdir::Project::new();
        for (rel, body) in files {
            project.write(rel, body);
        }
        project
    }

    fn min_project_block() -> String {
        format!(
            "[project]\nroot = \".\"\nversion = \"{v}\"\nmin_inclean_version = \"{v}\"\n",
            v = env!("CARGO_PKG_VERSION"),
        )
    }

    #[test]
    fn find_root_config_walks_up_from_nested_dir() {
        let proj = build_tree(&[
            ("inclean.toml", &min_project_block()),
            ("src/foo/bar.c", ""),
        ]);
        let cfg = find_root_config(&proj.path().join("src/foo")).unwrap();
        assert_eq!(cfg, proj.path().join("inclean.toml"));
    }

    #[test]
    fn find_root_config_accepts_file_path_as_start() {
        let proj = build_tree(&[
            ("inclean.toml", &min_project_block()),
            ("src/foo/bar.c", ""),
        ]);
        let cfg = find_root_config(&proj.path().join("src/foo/bar.c")).unwrap();
        assert_eq!(cfg, proj.path().join("inclean.toml"));
    }

    #[test]
    fn find_root_config_errors_when_no_config() {
        let proj = build_tree(&[("src/foo.c", "")]);
        let err = find_root_config(&proj.path().join("src")).unwrap_err();
        assert!(format!("{err:#}").contains(CONFIG_FILENAME));
    }

    #[test]
    fn load_root_config_requires_project_block() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [[rule]]
            name = "base"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("[project]"));
    }

    #[test]
    fn load_root_config_requires_version() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            min_inclean_version = "0.3.0"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("`version`"));
    }

    #[test]
    fn load_root_config_requires_min_inclean_version() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "0.3.0"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("`min_inclean_version`"));
    }

    #[test]
    fn load_root_config_rejects_config_below_cli_compat_min() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "0.2.5"
            min_inclean_version = "0.2.5"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("0.2.5") && msg.contains(CLI_COMPAT_MIN));
    }

    #[test]
    fn load_root_config_rejects_config_requiring_future_cli() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "0.3.0"
            min_inclean_version = "99.0.0"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("99.0.0"));
        assert!(msg.contains("Upgrade the CLI"));
    }

    #[test]
    fn load_root_config_rejects_malformed_version() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "not-semver"
            min_inclean_version = "0.3.0"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not valid semver") && msg.contains("not-semver"));
    }

    #[test]
    fn load_root_config_accepts_matching_versions() {
        let proj = build_tree(&[("inclean.toml", &min_project_block())]);
        load_root_config(&proj.path().join("inclean.toml")).unwrap();
    }

    #[test]
    fn resolve_project_root_defaults_to_dot_when_field_omitted() {
        let body = format!(
            "[project]\nversion = \"{v}\"\nmin_inclean_version = \"{v}\"\n",
            v = env!("CARGO_PKG_VERSION"),
        );
        let proj = build_tree(&[("inclean.toml", &body)]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let resolved = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(proj.path()).unwrap());
    }

    #[test]
    fn resolve_project_root_joins_relative_subdir() {
        let body = format!(
            "[project]\nroot = \"lib\"\nversion = \"{v}\"\nmin_inclean_version = \"{v}\"\n",
            v = env!("CARGO_PKG_VERSION"),
        );
        let proj = build_tree(&[("inclean.toml", &body), ("lib/keep", "")]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let resolved = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(proj.path().join("lib")).unwrap()
        );
    }

    #[test]
    fn resolve_project_root_rejects_empty_string() {
        let body = format!(
            "[project]\nroot = \"\"\nversion = \"{v}\"\nmin_inclean_version = \"{v}\"\n",
            v = env!("CARGO_PKG_VERSION"),
        );
        let proj = build_tree(&[("inclean.toml", &body)]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let err = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn resolve_project_root_errors_when_target_missing() {
        let body = format!(
            "[project]\nroot = \"nowhere\"\nversion = \"{v}\"\nmin_inclean_version = \"{v}\"\n",
            v = env!("CARGO_PKG_VERSION"),
        );
        let proj = build_tree(&[("inclean.toml", &body)]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let err = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("nowhere"));
    }

    #[test]
    fn assert_no_extra_configs_accepts_lone_root() {
        let proj = build_tree(&[("inclean.toml", &min_project_block())]);
        let root_cfg = proj.path().join("inclean.toml");
        assert_no_extra_configs(proj.path(), &root_cfg).unwrap();
    }

    #[test]
    fn assert_no_extra_configs_errors_when_extras_present() {
        let proj = build_tree(&[
            ("inclean.toml", &min_project_block()),
            (
                "src/inclean.toml",
                r#"[[rule]]
name = "sub""#,
            ),
        ]);
        let root_cfg = proj.path().join("inclean.toml");
        let err = assert_no_extra_configs(proj.path(), &root_cfg).unwrap_err();
        assert!(format!("{err:#}").contains("sub-configs are not supported"));
    }

    #[test]
    fn assert_no_extra_configs_detects_extras_in_target_and_node_modules() {
        // Per refactor.md §Engine "不要自动加料": no implicit skip of
        // .git/target/node_modules. Stray configs anywhere are reported.
        let proj = build_tree(&[
            ("inclean.toml", &min_project_block()),
            (
                "target/inclean.toml",
                r#"[[rule]]
name = "in-target""#,
            ),
            (
                "node_modules/inclean.toml",
                r#"[[rule]]
name = "in-nm""#,
            ),
        ]);
        let root_cfg = proj.path().join("inclean.toml");
        let err = assert_no_extra_configs(proj.path(), &root_cfg).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("target/inclean.toml") || msg.contains("target\\inclean.toml"));
        assert!(
            msg.contains("node_modules/inclean.toml") || msg.contains("node_modules\\inclean.toml")
        );
    }

    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct Project {
            path: PathBuf,
        }

        impl Project {
            pub fn new() -> Self {
                let pid = std::process::id();
                let n = COUNTER.fetch_add(1, Ordering::SeqCst);
                let path = std::env::temp_dir().join(format!("inclean-test-{pid}-{n}"));
                std::fs::create_dir_all(&path).expect("create tempdir");
                Project { path }
            }

            pub fn path(&self) -> &Path {
                &self.path
            }

            pub fn write(&self, rel: &str, body: &str) {
                let full = self.path.join(rel);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).expect("mkdirs");
                }
                std::fs::write(full, body).expect("write");
            }
        }

        impl Drop for Project {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}
