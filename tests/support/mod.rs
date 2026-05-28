//! Shared test helpers.

// Each binary compiles this module separately; functions unused by a
// given binary look dead from its perspective — the module-level
// `allow(dead_code)` silences that without per-fn ceremony.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub use inclean::utils::testing::*;

pub fn get_fixture(name: &str) -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest).join("tests/fixtures").join(name)
}
