use std::path::PathBuf;

use anyhow::Result;

pub fn run(_file: PathBuf, _include: Option<String>) -> Result<u8> {
    anyhow::bail!("`inclean explain` is not yet implemented (M2)")
}
