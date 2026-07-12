//! Locate, load, and validate the single `inclean.toml` for a project.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::schema::{self, LoadedConfig, RawProject};
use semver::Version as SemVer;

use crate::profile::{CLI_VERSION, CONFIG_FILENAME, MIN_COMPAT_CFG_VERSION};

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
    check_version_compatibility(config_path, &raw.project)?;
    Ok(LoadedConfig {
        path: config_path.to_path_buf(),
        raw,
    })
}

/// Two-direction version check:
/// - `cli.MIN_COMPAT_CFG_VERSION <= config.version`
/// - `config.min_inclean_version <= cli.CLI_VERSION`
///
/// Either failure is a hard error citing both sides.
fn check_version_compatibility(config_path: &Path, project: &RawProject) -> Result<()> {
    let cli_min_cfg =
        SemVer::parse(MIN_COMPAT_CFG_VERSION).expect("MIN_COMPAT_CFG_VERSION must be valid semver");
    let cli_version = SemVer::parse(CLI_VERSION).expect("CLI_VERSION must be valid semver");

    let cfg_version = SemVer::parse(&project.version).with_context(|| {
        format!(
            "{}: [project].version = \"{}\" is not valid semver",
            config_path.display(),
            project.version,
        )
    })?;

    let cfg_min_cli = SemVer::parse(&project.min_inclean_version).with_context(|| {
        format!(
            "{}: [project].min_inclean_version = \"{}\" is not valid semver",
            config_path.display(),
            project.min_inclean_version,
        )
    })?;

    if cfg_version < cfg_min_cli {
        // config broken
        anyhow::bail!(
            "{}: config requires a higher CLI version {} than its own version {}.",
            config_path.display(),
            cfg_min_cli,
            cfg_version,
        );
    }

    if cfg_version < cli_min_cfg {
        anyhow::bail!(
            "{}: config was written for inclean {} but this CLI can only read configs >= {} \
             (the last breaking schema change). Update the config to the current schema and \
             set [project].version = \"{}\".",
            config_path.display(),
            cfg_version,
            cli_min_cfg,
            cli_version,
        );
    }
    if cfg_min_cli > cli_version {
        anyhow::bail!(
            "{}: config requires inclean >= {} but this CLI is {}. Upgrade the CLI.",
            config_path.display(),
            cfg_min_cli,
            cli_version,
        );
    }

    Ok(())
}

/// Compute the actual project root from `config_path` and `[project].root`.
pub fn resolve_project_root(config_path: &Path, project: &RawProject) -> Result<PathBuf> {
    let raw_value = &project.root;
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

#[cfg(test)]
mod tests {
    use crate::profile::{CFG_VERSION, MIN_COMPAT_CLI_VERSION};
    use crate::utils::testing::config::project_block;
    use crate::utils::testing::fs::{TmpDir, TmpProject};

    use super::*;

    #[test]
    fn find_root_config_walks_up_from_nested_dir() {
        let proj = TmpProject::create_with_files(&[(&"src/foo/bar.c", &"")]);
        let cfg = find_root_config(&proj.path().join("src/foo")).unwrap();
        assert_eq!(cfg, proj.config_path());
    }

    #[test]
    fn find_root_config_accepts_file_path_as_start() {
        let proj = TmpProject::create_with_files(&[(&"src/foo/bar.c", &"")]);
        let cfg = find_root_config(&proj.path().join("src/foo/bar.c")).unwrap();
        assert_eq!(cfg, proj.config_path());
    }

    #[test]
    fn find_root_config_errors_when_no_config() {
        let proj = TmpDir::create_with_files(&[(&"src/foo.c", &"")]);
        let err = find_root_config(&proj.path().join("src")).unwrap_err();
        assert!(format!("{err:#}").contains(CONFIG_FILENAME));
    }

    #[test]
    fn config_project_block_missing_fields() {
        let proj = TmpProject::create_with_config(format!(
            r#"
            [project]
            root = "."
            min_inclean_version = "{MIN_COMPAT_CLI_VERSION}"
            "#
        ));
        let err = load_root_config(proj.config_path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("`version`"),
            "Should require [project].version"
        );

        let proj = TmpProject::create_with_config(format!(
            r#"
            [project]
            root = "."
            version = "{CFG_VERSION}"
            "#
        ));
        let err = load_root_config(proj.config_path()).unwrap_err();
        assert!(
            format!("{err:#}").contains("`min_inclean_version`"),
            "Should require [project].min_inclean_version"
        );
    }

    #[test]
    fn config_project_block_default_root() {
        let proj = TmpProject::create_with_min_config();
        let cfg = load_root_config(proj.config_path()).unwrap();
        let resolved = resolve_project_root(&cfg.path, &cfg.raw.project).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(proj.path()).unwrap());

        let proj = TmpProject::create_with_config(project_block(Some(".")));
        let cfg = load_root_config(proj.config_path()).unwrap();
        let resolved = resolve_project_root(&cfg.path, &cfg.raw.project).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(proj.path()).unwrap());
    }

    #[test]
    fn config_project_block_incompatible_versions() {
        // --------- Broken config ----------
        let proj = TmpProject::create_with_config(
            r#"
            [project]
            version = "0.2.5"
            min_inclean_version = "0.2.6"
            "#,
        );
        load_root_config(proj.config_path()).unwrap_err();

        // --------- Outdated config ----------
        let proj = TmpProject::create_with_config(
            r#"
            [project]
            version = "0.2.5"
            min_inclean_version = "0.2.0"
            "#,
        );
        load_root_config(proj.config_path()).unwrap_err();

        // --------- Outdated CLI ----------
        let proj = TmpProject::create_with_config(
            r#"
            [project]
            version = "999.2.3"
            min_inclean_version = "999.0.0"
            "#,
        );
        load_root_config(proj.config_path()).unwrap_err();
    }

    #[test]
    fn resolve_project_root_joins_relative_subdir() {
        let proj = TmpProject::create_with_files(&[
            (&"inclean.toml", &project_block(Some("lib"))), // overwrite the default config
            (&"lib/keep", &""),
        ]);
        let cfg = load_root_config(proj.config_path()).unwrap();
        let resolved = resolve_project_root(&cfg.path, &cfg.raw.project).unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(proj.path().join("lib")).unwrap()
        );
    }

    #[test]
    fn resolve_project_root_rejects_empty_string() {
        let proj = TmpProject::create_with_config(project_block(Some("")));
        let cfg = load_root_config(proj.config_path()).unwrap();
        let err = resolve_project_root(&cfg.path, &cfg.raw.project).unwrap_err();
        assert!(format!("{err:#}").contains("[project].root"));
    }

    #[test]
    fn resolve_project_root_errors_when_target_missing() {
        let proj = TmpProject::create_with_config(project_block(Some("nowhere/to/land")));
        let cfg = load_root_config(proj.config_path()).unwrap();
        let err = resolve_project_root(&cfg.path, &cfg.raw.project).unwrap_err();
        assert!(format!("{err:#}").contains("nowhere"));
    }
}
