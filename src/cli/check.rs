use std::path::PathBuf;

use anyhow::Result;

pub fn run(_dir: PathBuf, _validate: bool) -> Result<u8> {
    anyhow::bail!("`inclean check` is not yet implemented")
}
