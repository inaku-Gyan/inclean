//! Utility functions for testing

pub mod config {
    use std::{path::PathBuf, sync::LazyLock};

    use crate::config::schema::LoadedConfig;

    /// Minimum `[project]` section for testing.
    pub static MIN_PROJECT_BLOCK: LazyLock<String> = LazyLock::new(|| {
        format!(
            "[project]\nversion = \"{}\"\nmin_inclean_version = \"{}\"\n",
            crate::profile::CFG_VERSION,
            crate::profile::MIN_COMPAT_CLI_VERSION
        )
    });

    pub fn project_block(root: Option<&str>) -> String {
        let mut block = MIN_PROJECT_BLOCK.clone();
        if let Some(root) = root {
            block.push_str(&format!("root = \"{}\"\n", root));
        }
        block
    }

    /// Load a config without bothering to write a `[project]` section.
    pub fn load_rules(body: &str) -> LoadedConfig {
        use crate::config::schema::parse;
        let path = PathBuf::from("tmp_test_config.inclean.toml");
        let raw = parse(&format!("{}{}", &*MIN_PROJECT_BLOCK, body), &path).unwrap();
        LoadedConfig { path, raw }
    }
}
