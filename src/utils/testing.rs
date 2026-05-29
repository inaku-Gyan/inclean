//! Utility functions for testing

pub mod config {
    use std::{path::PathBuf, sync::LazyLock};

    use crate::config::schema::{LoadedConfig, RawProject};

    impl RawProject {
        pub fn to_cfg_str(self) -> String {
            let s = format!(
                "[project]\nroot = \"{}\"\nversion = \"{}\"\nmin_inclean_version = \"{}\"\n",
                self.root, self.version, self.min_inclean_version
            );
            s
        }
    }

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

    use crate::utils::testing::config::MIN_PROJECT_BLOCK;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    type PathedFiles<'a> = [(&'a dyn AsRef<Path>, &'a dyn AsRef<[u8]>)];

    pub struct TmpDir {
        path: PathBuf,
    }

    impl Default for TmpDir {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TmpDir {
        fn temp_dir() -> PathBuf {
            // std::env::temp_dir()
            let manifest = env!("CARGO_MANIFEST_DIR");
            Path::new(manifest).join("tempdir/testspace")
        }

        pub fn create_by_label(label: &str) -> Self {
            let pid = std::process::id();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);

            let mut path = Self::temp_dir();
            if !label.is_empty() {
                path = path.join(label);
            }
            path = path.join(format!("{pid}-{ts}-{n}"));

            std::fs::create_dir_all(&path).expect("create tempdir");
            TmpDir { path }
        }

        pub fn new() -> Self {
            Self::create_by_label("")
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        pub fn write(&self, relpath: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
            let full = self.path.join(relpath);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("mkdirs");
            }
            std::fs::write(full, contents).expect("write");
        }

        pub fn create_with_files(files: &PathedFiles) -> Self {
            let tmpdir = Self::new();
            tmpdir.write_files(files);
            tmpdir
        }

        pub fn write_files(&self, files: &PathedFiles) {
            for (path, contents) in files {
                self.write(path, contents);
            }
        }

        pub fn read_to_string(&self, relpath: impl AsRef<Path>) -> String {
            std::fs::read_to_string(self.path.join(relpath)).expect("read")
        }

        pub fn read(&self, relpath: impl AsRef<Path>) -> Vec<u8> {
            std::fs::read(self.path.join(relpath)).expect("read")
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.path).expect("remove the temp dir");
        }
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
                fs::copy(&from, &to).unwrap_or_else(|_| {
                    panic!(
                        "failed to copy file from\n\t{}\nto\n\t{}\n",
                        from.display(),
                        to.display()
                    )
                });
            }
        }
    }

    pub struct TmpProject {
        dir: TmpDir,
        /// Absolute path to the config file within the project dir.
        cfg_abspath: PathBuf,
    }

    impl TmpProject {
        pub fn new(cfg_relpath: impl AsRef<Path>, cfg_content: impl AsRef<[u8]>) -> Self {
            let dir = TmpDir::new();
            dir.write(&cfg_relpath, cfg_content);
            Self {
                cfg_abspath: dir.path().join(&cfg_relpath),
                dir,
            }
        }

        /// Create a project with the given config content in `./inclean.toml`.
        /// ```rust
        /// use inclean::utils::testing::fs::TmpProject;
        /// TmpProject::create_with_config(r#"
        ///     [project]
        ///     version = "0.1.0"
        ///     min_inclean_version = "0.1.0"
        /// "#);
        /// ```
        /// Or, with a `RawProject`:
        /// ```rust
        /// use inclean::config::schema::RawProject;
        /// use inclean::utils::testing::{fs::TmpProject, config};
        /// let cfg = RawProject {
        ///     root: "src".to_string(),
        ///     version: "0.1.0".to_string(),
        ///     min_inclean_version: "0.1.0".to_string(),
        /// };
        /// TmpProject::create_with_config(cfg.to_cfg_str());
        /// ```
        pub fn create_with_config<C: AsRef<[u8]>>(cfg_content: C) -> Self {
            Self::new("inclean.toml", cfg_content)
        }

        pub fn create_with_min_config() -> Self {
            Self::create_with_config(&*MIN_PROJECT_BLOCK)
        }

        pub fn create_with_files(files: &PathedFiles) -> Self {
            let project = Self::create_with_min_config();
            project.write_files(files);
            project
        }

        pub fn write(&self, relpath: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
            self.dir.write(relpath, contents);
        }

        pub fn write_files(&self, files: &PathedFiles) {
            self.dir.write_files(files);
        }

        pub fn create_with_rules(rules: &str) -> Self {
            Self::create_with_config(format!("{}{}", &*MIN_PROJECT_BLOCK, rules))
        }

        pub fn read_to_string(&self, relpath: impl AsRef<Path>) -> String {
            self.dir.read_to_string(relpath)
        }

        pub fn read(&self, relpath: impl AsRef<Path>) -> Vec<u8> {
            self.dir.read(relpath)
        }

        pub fn path(&self) -> &Path {
            self.dir.path()
        }

        pub fn config_path(&self) -> &Path {
            &self.cfg_abspath
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tmpproject_lifecycle() {
        let project_path = {
            let project = fs::TmpProject::create_with_files(&[
                (&"file1.txt", &"Hello, world!"),
                (&"subdir/file2.txt", &"Goodbye, world!"),
            ]);

            assert!(project.path().exists());
            assert_eq!(project.read_to_string("file1.txt"), "Hello, world!");
            assert_eq!(
                project.read_to_string("subdir/file2.txt"),
                "Goodbye, world!"
            );

            project.path().to_path_buf()
        };
        // After the project goes out of scope, the temp directory should be deleted.
        assert!(!project_path.exists());
    }

    #[test]
    fn create_with_duplicate_files() {
        let project = fs::TmpProject::create_with_files(&[
            (&"file.txt", &"First content"),
            (&"file.txt", &"Second content"),
        ]);

        // The second entry should overwrite the first one.
        assert_eq!(project.read_to_string("file.txt"), "Second content");
    }
}
