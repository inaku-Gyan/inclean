pub const CONFIG_FILENAME: &str = "inclean.toml";

/// Current version of the config format. Always the same as the CLI version.
pub const CFG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Current version of the Inclean CLI. Always the same as the config version.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Minimum version of the config format that this version of Inclean CLI can read.
pub const MIN_COMPAT_CFG_VERSION: &str = "0.3.0-alpha.3";

/// Minimum version of the Inclean CLI that can read this version of the config format.
pub const MIN_COMPAT_CLI_VERSION: &str = "0.4.0-alpha.1";

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::*;

    #[test]
    fn version_constants_have_consistent_order() {
        let cfg_version = Version::parse(CFG_VERSION).unwrap();
        let cli_version = Version::parse(CLI_VERSION).unwrap();
        let min_cfg_version = Version::parse(MIN_COMPAT_CFG_VERSION).unwrap();
        let min_cli_version = Version::parse(MIN_COMPAT_CLI_VERSION).unwrap();

        assert_eq!(cfg_version, cli_version);
        assert!(min_cfg_version <= cfg_version);
        assert!(min_cfg_version <= min_cli_version);
        assert!(min_cli_version <= cli_version);
    }
}
