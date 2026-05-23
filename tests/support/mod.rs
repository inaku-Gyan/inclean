//! Shared test helpers.

// Each binary compiles this module separately; functions unused by a
// given binary look dead from its perspective — the module-level
// `allow(dead_code)` silences that without per-fn ceremony.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn get_fixture(name: &str) -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest).join("tests/fixtures").join(name)
}

/// Fresh empty directory at `tests/.workdir/<label>/`. Wipes any prior
/// contents at that path. The label must be unique within a test
/// process to avoid concurrent collisions — use a stable name when you
/// want post-mortem inspection (e.g. a golden case name), or call
/// [`tmp`] for an auto-named anonymous workdir.
///
/// `tests/.workdir/` is gitignored.
pub fn new_workdir(label: &str) -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = Path::new(manifest).join("tests/.workdir").join(label);
    if path.exists() {
        fs::remove_dir_all(&path).unwrap();
    }
    fs::create_dir_all(&path).unwrap();
    path
}

/// Anonymous workdir with a unique auto-generated label. Backwards-
/// compatible entry point for tests that don't care about the dir
/// name (only that it's fresh and theirs).
pub fn new_tmp_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    new_workdir(&format!(
        "anon-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ))
}

pub fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}
