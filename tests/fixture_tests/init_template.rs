//! Sanity check for the `init` subcommand: the starter config it writes
//! must pass `check --level config`. Runs through the actual CLI binary
//! rather than the library API.

use std::fs;
use std::process::Command;

use crate::support;

#[test]
fn init_template_passes_config_check() {
    let root = support::new_tmp_dir();
    let bin = env!("CARGO_BIN_EXE_inclean");
    let init = Command::new(bin)
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success(), "init failed: {:?}", init);
    let out = Command::new(bin)
        .args(["check", "config", "-c"])
        .arg(root.join("inclean.toml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`check config` on init template failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    fs::remove_dir_all(&root).ok();
}
