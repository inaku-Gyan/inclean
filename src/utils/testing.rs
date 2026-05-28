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

pub mod fs {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub struct TmpDir {
        path: PathBuf,
    }

    impl TmpDir {
        fn temp_dir() -> PathBuf {
            // std::env::temp_dir()
            let manifest = env!("CARGO_MANIFEST_DIR");
            Path::new(manifest).join("tempdir")
        }

        pub fn create_by_label(label: Option<&str>) -> Self {
            let pid = std::process::id();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);

            let mut path = Self::temp_dir();
            if let Some(l) = label
                && !l.is_empty()
            {
                path = path.join(l);
            }
            path = path.join(format!("{pid}-{ts}-{n}"));

            std::fs::create_dir_all(&path).expect("create tempdir");
            TmpDir { path }
        }

        pub fn new() -> Self {
            Self::create_by_label(None)
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        pub fn write(&self, relpath: &str, body: &str) {
            let full = self.path.join(relpath);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("mkdirs");
            }
            std::fs::write(full, body).expect("write");
        }

        pub fn create_tree(files: &[(&str, &str)]) -> Self {
            let tmpdir = Self::new();
            for (path, body) in files {
                tmpdir.write(path, body);
            }
            tmpdir
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.path).expect("remove the temp dir");
        }
    }

    pub fn copy_dir(src: &Path, dst: &Path) {
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
}
