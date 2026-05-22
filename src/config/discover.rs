//! Locate, load, and validate the single `inclean.toml` for a project.
//!
//! Three entry points are intended for callers:
//!
//! - [`find_root_config`] climbs from a starting directory looking for the
//!   nearest `inclean.toml`. The path of that file is the *root config*.
//! - [`load_root_config`] reads and parses the root config file.
//! - [`resolve_project_root`] joins the file's directory with `[project].root`
//!   (default `"."`) to produce the actual project root on disk.
//!
//! Additionally [`assert_no_extra_configs`] walks the resolved project root
//! and errors if any extra `inclean.toml` exists — sub-configs are not a
//! feature.
//!
//! Tree-walking honors `.gitignore` plus a few hard-coded skip directories
//! (`.git`, `target`, `node_modules`) so generated or vendored trees do not
//! poison configuration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

use super::schema::{self, LoadedConfig, RawProject};

pub const CONFIG_FILENAME: &str = "inclean.toml";

/// Minimum `[project].version` accepted by this CLI.
///
/// inclean is pre-1.0 / beta. Whenever the on-disk schema gets a
/// breaking change, bump this to the release that introduces the
/// change; older configs are then hard-rejected with no migration
/// shim. Backward compatibility is explicitly out of scope before
/// v1.0.0 — see CLAUDE.md.
pub const MIN_SUPPORTED_INCLEAN_TOML_VERSION: &str = "0.2.0";

/// Climb from `start` (or its parent if it is a file path) toward `/`
/// looking for the nearest `inclean.toml`.
///
/// Returns the absolute path of that file. Errors if no `inclean.toml` is
/// found before reaching the filesystem root.
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

/// Load and parse the single `inclean.toml` at `config_path`. The root
/// config must declare a `[project]` block, and `[project].version` must
/// be valid semver `>= MIN_SUPPORTED_INCLEAN_TOML_VERSION`.
pub fn load_root_config(config_path: &Path) -> Result<LoadedConfig> {
    let text = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let raw = schema::parse(&text, config_path)?;
    let project = raw.project.as_ref().ok_or_else(|| {
        anyhow::anyhow!("{} must declare a [project] block", config_path.display(),)
    })?;
    validate_project_version(config_path, project)?;
    Ok(LoadedConfig {
        path: config_path.to_path_buf(),
        raw,
    })
}

/// Enforce the pre-1.0 version gate: `[project].version` is required, must
/// parse as semver, and must be `>= MIN_SUPPORTED_INCLEAN_TOML_VERSION`.
/// Older configs are hard-rejected with no migration shim.
fn validate_project_version(config_path: &Path, project: &RawProject) -> Result<()> {
    let raw_version = project.version.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "{} must declare `version` in [project] (this CLI requires >= {})",
            config_path.display(),
            MIN_SUPPORTED_INCLEAN_TOML_VERSION,
        )
    })?;
    let cfg_version = semver::Version::parse(raw_version).with_context(|| {
        format!(
            "{} has invalid `version = \"{}\"` (must be semver, e.g. \"{}\")",
            config_path.display(),
            raw_version,
            MIN_SUPPORTED_INCLEAN_TOML_VERSION,
        )
    })?;
    let min_version = semver::Version::parse(MIN_SUPPORTED_INCLEAN_TOML_VERSION)
        .expect("MIN_SUPPORTED_INCLEAN_TOML_VERSION must be valid semver");
    if cfg_version < min_version {
        anyhow::bail!(
            "{} declares version {}, but this CLI requires >= {}.\n\
             inclean is pre-1.0; breaking schema changes do not carry a migration path. \
             Update the config to match the current schema and set `version = \"{}\"`.",
            config_path.display(),
            cfg_version,
            MIN_SUPPORTED_INCLEAN_TOML_VERSION,
            env!("CARGO_PKG_VERSION"),
        );
    }
    Ok(())
}

/// Compute the actual project root from `config_path` and `[project].root`.
///
/// `[project].root` is interpreted relative to the directory containing
/// `inclean.toml`. Defaults to `"."` if the field is omitted. Empty or
/// whitespace-only strings are rejected. The resolved path must exist and
/// be a directory; the result is canonicalized.
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
/// `root_config_path`. Sub-configs are not a feature; finding extra files
/// is almost always user error (forgotten leftover from a refactor, or a
/// mistaken attempt to use the old hierarchical model).
pub fn assert_no_extra_configs(project_root: &Path, root_config_path: &Path) -> Result<()> {
    let root_canon =
        std::fs::canonicalize(root_config_path).unwrap_or_else(|_| root_config_path.to_path_buf());
    let mut extras: Vec<PathBuf> = Vec::new();

    let walker = WalkBuilder::new(project_root)
        .standard_filters(true)
        .filter_entry(|entry| {
            let name = entry.file_name();
            !matches!(
                name.to_str(),
                Some(".git") | Some("target") | Some("node_modules")
            )
        })
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

    /// Build a temporary project tree for testing. Returns the project root.
    fn build_tree(files: &[(&str, &str)]) -> tempdir::Project {
        let project = tempdir::Project::new();
        for (rel, body) in files {
            project.write(rel, body);
        }
        project
    }

    #[test]
    fn find_root_config_walks_up_from_nested_dir() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"[project]
root = "."
"#,
            ),
            ("src/foo/bar.c", ""),
        ]);
        let cfg = find_root_config(&proj.path().join("src/foo")).unwrap();
        assert_eq!(cfg, proj.path().join("inclean.toml"));
    }

    #[test]
    fn find_root_config_accepts_file_path_as_start() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"[project]
root = "."
"#,
            ),
            ("src/foo/bar.c", ""),
        ]);
        let cfg = find_root_config(&proj.path().join("src/foo/bar.c")).unwrap();
        assert_eq!(cfg, proj.path().join("inclean.toml"));
    }

    #[test]
    fn find_root_config_errors_when_no_config() {
        let proj = build_tree(&[("src/foo.c", "")]);
        let start = proj.path().join("src");
        let err = find_root_config(&start).unwrap_err();
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
    fn load_root_config_accepts_minimal_project_block() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "0.2.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        assert!(cfg.raw.project.is_some());
        assert_eq!(cfg.raw.rules.len(), 1);
    }

    #[test]
    fn resolve_project_root_defaults_to_dot_when_field_omitted() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            version = "0.2.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let resolved = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(proj.path()).unwrap());
    }

    #[test]
    fn resolve_project_root_accepts_dot() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "0.2.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let resolved = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(proj.path()).unwrap());
    }

    #[test]
    fn resolve_project_root_joins_relative_subdir() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"
                [project]
                root = "lib"
                version = "0.2.0"

                [[rule]]
                name = "base"
                "#,
            ),
            ("lib/keep", ""),
        ]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let resolved = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(proj.path().join("lib")).unwrap()
        );
    }

    #[test]
    fn resolve_project_root_accepts_dotdot() {
        let proj = build_tree(&[
            (
                "host/inclean.toml",
                r#"
                [project]
                root = "../sibling"
                version = "0.2.0"

                [[rule]]
                name = "base"
                "#,
            ),
            ("sibling/keep", ""),
        ]);
        let cfg = load_root_config(&proj.path().join("host/inclean.toml")).unwrap();
        let resolved = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(proj.path().join("sibling")).unwrap()
        );
    }

    #[test]
    fn resolve_project_root_errors_when_target_missing() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "nowhere"
            version = "0.2.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let err = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("nowhere"));
    }

    #[test]
    fn resolve_project_root_rejects_empty_string() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = ""
            version = "0.2.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let err = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn resolve_project_root_rejects_whitespace_string() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "   "
            version = "0.2.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let err = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn resolve_project_root_errors_when_target_is_file_not_dir() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"
                [project]
                root = "afile"
                version = "0.2.0"

                [[rule]]
                name = "base"
                "#,
            ),
            ("afile", "i am a regular file"),
        ]);
        let cfg = load_root_config(&proj.path().join("inclean.toml")).unwrap();
        let err = resolve_project_root(&cfg.path, cfg.raw.project.as_ref().unwrap()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not a directory") || msg.contains("does not exist"));
    }

    #[test]
    fn load_root_config_rejects_missing_version() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "base"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("must declare `version`"),
            "want missing-version message, got: {msg}"
        );
    }

    #[test]
    fn load_root_config_rejects_version_below_minimum() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "0.1.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains(">=") && msg.contains(MIN_SUPPORTED_INCLEAN_TOML_VERSION),
            "want too-old message mentioning >= and {MIN_SUPPORTED_INCLEAN_TOML_VERSION}, got: {msg}"
        );
    }

    #[test]
    fn load_root_config_rejects_malformed_version() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "not-semver"

            [[rule]]
            name = "base"
            "#,
        )]);
        let err = load_root_config(&proj.path().join("inclean.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("invalid") && msg.contains("not-semver"),
            "want invalid-semver message, got: {msg}"
        );
    }

    #[test]
    fn load_root_config_accepts_version_equal_to_minimum() {
        let body = format!(
            r#"
            [project]
            root = "."
            version = "{MIN_SUPPORTED_INCLEAN_TOML_VERSION}"

            [[rule]]
            name = "base"
            "#
        );
        let proj = build_tree(&[("inclean.toml", body.as_str())]);
        load_root_config(&proj.path().join("inclean.toml")).unwrap();
    }

    #[test]
    fn load_root_config_accepts_version_above_minimum() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            version = "99.0.0"

            [[rule]]
            name = "base"
            "#,
        )]);
        load_root_config(&proj.path().join("inclean.toml")).unwrap();
    }

    #[test]
    fn assert_no_extra_configs_accepts_lone_root() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."
            "#,
        )]);
        let root_cfg = proj.path().join("inclean.toml");
        assert_no_extra_configs(proj.path(), &root_cfg).unwrap();
    }

    #[test]
    fn assert_no_extra_configs_errors_when_extras_present() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"
                [project]
                root = "."
                "#,
            ),
            (
                "src/inclean.toml",
                r#"
                [[rule]]
                name = "sub"
                "#,
            ),
        ]);
        let root_cfg = proj.path().join("inclean.toml");
        let err = assert_no_extra_configs(proj.path(), &root_cfg).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("sub-configs are not supported"));
        let expected_extra = std::path::Path::new("src")
            .join("inclean.toml")
            .display()
            .to_string();
        assert!(msg.contains(&expected_extra), "got: {msg}");
    }

    #[test]
    fn assert_no_extra_configs_skips_target_and_node_modules() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"
                [project]
                root = "."
                "#,
            ),
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
        assert_no_extra_configs(proj.path(), &root_cfg).unwrap();
    }

    #[test]
    fn assert_no_extra_configs_skips_ignored_subtree() {
        let proj = build_tree(&[
            (".ignore", "ignored/\n"),
            (
                "inclean.toml",
                r#"
                [project]
                root = "."
                "#,
            ),
            (
                "ignored/inclean.toml",
                r#"[[rule]]
name = "should-not-load""#,
            ),
        ]);
        let root_cfg = proj.path().join("inclean.toml");
        assert_no_extra_configs(proj.path(), &root_cfg).unwrap();
    }

    /// A small inline temp-dir helper to avoid adding the `tempfile` crate
    /// just for tests. It deletes the directory on drop.
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
