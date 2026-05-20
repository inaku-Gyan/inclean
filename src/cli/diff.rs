use std::path::PathBuf;

use anyhow::Result;

pub fn run(_path: Option<PathBuf>, _validate: bool) -> Result<u8> {
    anyhow::bail!("`inclean diff` is not yet implemented")
}
