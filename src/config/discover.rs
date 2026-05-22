//! Locate and load every `inclean.toml` in a project tree.
//!
//! Two entry points:
//!
//! - [`discover_project_root`] climbs from a starting directory looking for
//!   the nearest `inclean.toml`; the directory containing that file is the
//!   project root.
//! - [`load_all_configs`] walks the project root downward collecting every
//!   `inclean.toml`. The result is sorted by path depth so the project root's
//!   config is first.
//!
//! Tree-walking honors `.gitignore` plus a few hard-coded skip directories
//! (`.git`, `target`, `node_modules`) so generated or vendored trees do not
//! poison configuration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

use super::schema::{self, LoadedConfig};

pub const CONFIG_FILENAME: &str = "inclean.toml";

/// Validate structural invariants the loader cannot express in the serde
/// schema:
///
/// - The first loaded config (the one at `root_dir/inclean.toml`) must be
///   present and must declare a `[project]` block whose `root` field is
///   set explicitly. This sigil distinguishes the root config from
///   sub-configs.
/// - No sub-config may declare a `[project]` block.
///
/// Call after [`load_all_configs`] and before resolving rule inheritance.
pub fn validate_loaded(configs: &[LoadedConfig], root_dir: &Path) -> Result<()> {
    if configs.is_empty() {
        anyhow::bail!("no {CONFIG_FILENAME} configs loaded");
    }
    let root_cfg = &configs[0];
    let expected = root_dir.join(CONFIG_FILENAME);
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canon(&root_cfg.path) != canon(&expected) {
        anyhow::bail!(
            "expected root config at {} but the shallowest found was {}",
            expected.display(),
            root_cfg.path.display(),
        );
    }
    let project = root_cfg.raw.project.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "root config {} must declare a [project] block with `root = ...` set",
            root_cfg.path.display(),
        )
    })?;
    let root_value = project.root.as_deref().map(str::trim).unwrap_or("");
    if root_value.is_empty() {
        anyhow::bail!(
            "root config {}: [project].root must be set to a non-empty value",
            root_cfg.path.display(),
        );
    }
    for sub in &configs[1..] {
        if sub.raw.project.is_some() {
            anyhow::bail!(
                "sub-config {} must not declare a [project] block; only the root {CONFIG_FILENAME} may",
                sub.path.display(),
            );
        }
    }
    Ok(())
}

/// Climb from `start` (or its parent if it is a file path) toward `/`
/// looking for the nearest `inclean.toml`. The directory containing that
/// file is the project root.
///
/// Returns an error if no `inclean.toml` is found before reaching the
/// filesystem root.
pub fn discover_project_root(start: &Path) -> Result<PathBuf> {
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
            return Ok(current);
        }
        if !current.pop() {
            anyhow::bail!(
                "no {CONFIG_FILENAME} found in {} or any parent directory",
                start.display(),
            );
        }
    }
}

/// Walk `project_root` and load every `inclean.toml` underneath. The root
/// config must exist; sub-configs are optional. Results are sorted such that
/// shallower configs (closer to the root) come first.
pub fn load_all_configs(project_root: &Path) -> Result<Vec<LoadedConfig>> {
    let mut configs: Vec<LoadedConfig> = Vec::new();

    let walker = WalkBuilder::new(project_root)
        .standard_filters(true) // honors .gitignore, .ignore, hidden files
        .filter_entry(|entry| {
            // Skip well-known directories that frequently exist outside the
            // .gitignore (e.g. for fresh checkouts) to keep walks fast.
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
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let raw = schema::parse(&text, &path)?;
        configs.push(LoadedConfig { path, raw });
    }

    if configs.is_empty() {
        anyhow::bail!(
            "no {CONFIG_FILENAME} found under {}",
            project_root.display()
        );
    }

    // Sort by depth (component count) then by path. The root config (lowest
    // depth) ends up first, which matches the "closest first" trial order
    // semantics that pipeline::run wants reversed when evaluating rules.
    configs.sort_by(|a, b| {
        let depth = |p: &Path| p.components().count();
        depth(&a.path)
            .cmp(&depth(&b.path))
            .then(a.path.cmp(&b.path))
    });

    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a temporary project tree for testing. Returns the project root.
    fn build_tree(files: &[(&str, &str)]) -> tempdir::Project {
        let project = tempdir::Project::new();
        for (rel, body) in files {
            project.write(rel, body);
        }
        project
    }

    #[test]
    fn discover_walks_up_from_nested_dir() {
        let proj = build_tree(&[("inclean.toml", ""), ("src/foo/bar.c", "")]);
        let root = discover_project_root(&proj.path().join("src/foo")).unwrap();
        assert_eq!(root, proj.path());
    }

    #[test]
    fn discover_accepts_file_path_as_start() {
        let proj = build_tree(&[("inclean.toml", ""), ("src/foo/bar.c", "")]);
        let root = discover_project_root(&proj.path().join("src/foo/bar.c")).unwrap();
        assert_eq!(root, proj.path());
    }

    #[test]
    fn discover_errors_when_no_config() {
        let proj = build_tree(&[("src/foo.c", "")]);
        // Use a deep dir under proj to make the climb run, but never find a
        // config inside the project tree.
        let start = proj.path().join("src");
        let err = discover_project_root(&start).unwrap_err();
        assert!(format!("{err:#}").contains(CONFIG_FILENAME));
    }

    #[test]
    fn load_all_collects_root_and_nested_configs() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"[[rule]]
name = "base""#,
            ),
            (
                "src/inclean.toml",
                r#"[[rule]]
name = "src-only""#,
            ),
            (
                "src/internal/inclean.toml",
                r#"[[rule]]
name = "internal""#,
            ),
        ]);
        let mut configs = load_all_configs(proj.path()).unwrap();
        assert_eq!(configs.len(), 3);
        // shallowest first
        assert!(configs[0].path.ends_with("inclean.toml"));
        assert!(configs[0].path.parent().unwrap() == proj.path());
        // deepest last
        let last = configs.pop().unwrap();
        assert!(last.path.to_string_lossy().contains("internal"));
    }

    #[test]
    fn load_all_skips_ignored_subtree() {
        // We use `.ignore` (not `.gitignore`) so the test does not need a
        // fake `.git` directory. The `ignore` crate honors `.ignore` files
        // unconditionally; `.gitignore` only inside an actual git repo.
        let proj = build_tree(&[
            (".ignore", "ignored/\n"),
            (
                "inclean.toml",
                r#"[[rule]]
name = "base""#,
            ),
            (
                "ignored/inclean.toml",
                r#"[[rule]]
name = "should-not-load""#,
            ),
        ]);
        let configs = load_all_configs(proj.path()).unwrap();
        assert_eq!(configs.len(), 1, "ignored/ should be skipped");
        assert!(!configs[0].path.to_string_lossy().contains("ignored"));
    }

    #[test]
    fn load_all_errors_when_no_config_anywhere() {
        let proj = build_tree(&[("src/foo.c", "")]);
        let err = load_all_configs(proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains(CONFIG_FILENAME));
    }

    #[test]
    fn validate_loaded_accepts_root_with_project_root() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "."

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        validate_loaded(&configs, proj.path()).unwrap();
    }

    #[test]
    fn validate_loaded_rejects_missing_project_block() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        let err = validate_loaded(&configs, proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains("[project]"));
    }

    #[test]
    fn validate_loaded_rejects_unset_project_root() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        let err = validate_loaded(&configs, proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn validate_loaded_rejects_empty_project_root() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = ""

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        let err = validate_loaded(&configs, proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn validate_loaded_rejects_whitespace_project_root() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "   "

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        let err = validate_loaded(&configs, proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn validate_loaded_rejects_subconfig_with_empty_project_block() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"
                [project]
                root = "."

                [[rule]]
                name = "base"
                "#,
            ),
            (
                "src/inclean.toml",
                r#"
                [project]

                [[rule]]
                name = "src-rule"
                "#,
            ),
        ]);
        let configs = load_all_configs(proj.path()).unwrap();
        let err = validate_loaded(&configs, proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains("sub-config"));
    }

    #[test]
    fn validate_loaded_accepts_arbitrary_relative_root_value() {
        // [project].root is a sigil; its value is not resolved against the
        // filesystem. A non-"." value (with no matching directory on disk) is
        // still accepted.
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "src"

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        validate_loaded(&configs, proj.path()).unwrap();
    }

    #[test]
    fn validate_loaded_accepts_absolute_root_value() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "/nonexistent/abs/path"

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        validate_loaded(&configs, proj.path()).unwrap();
    }

    #[test]
    fn validate_loaded_accepts_dotdot_root_value() {
        let proj = build_tree(&[(
            "inclean.toml",
            r#"
            [project]
            root = "../elsewhere"

            [[rule]]
            name = "base"
            "#,
        )]);
        let configs = load_all_configs(proj.path()).unwrap();
        validate_loaded(&configs, proj.path()).unwrap();
    }

    #[test]
    fn validate_loaded_rejects_subconfig_with_project_block() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"
                [project]
                root = "."

                [[rule]]
                name = "base"
                "#,
            ),
            (
                "src/inclean.toml",
                r#"
                [project]
                root = "."

                [[rule]]
                name = "src-rule"
                "#,
            ),
        ]);
        let configs = load_all_configs(proj.path()).unwrap();
        let err = validate_loaded(&configs, proj.path()).unwrap_err();
        assert!(format!("{err:#}").contains("sub-config"));
    }

    #[test]
    fn load_all_skips_target_and_node_modules() {
        let proj = build_tree(&[
            (
                "inclean.toml",
                r#"[[rule]]
name = "base""#,
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
        let configs = load_all_configs(proj.path()).unwrap();
        assert_eq!(configs.len(), 1);
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

    // Avoid an unused import warning about std::fs above.
    #[allow(dead_code)]
    fn _touch_fs() {
        let _ = fs::metadata("/");
    }
}
