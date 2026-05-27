use std::path::Path;

pub trait PathExt {
    /// Converts a path to a string using forward slashes `/` as the separator.
    /// This is useful for generating diffs or paths in cross-platform formats.
    fn to_slash(&self) -> String;

    fn looks_like_directory(&self) -> bool;
}

impl PathExt for Path {
    fn to_slash(&self) -> String {
        self.iter()
            .map(|c| c.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn looks_like_directory(&self) -> bool {
        // Trailing slash → directory.
        let as_str = self.as_os_str().to_string_lossy();
        if as_str.ends_with('/') || as_str.ends_with(std::path::MAIN_SEPARATOR) {
            return true;
        }
        // No extension → directory (e.g. `lib`, `foo/bar`).
        self.extension().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_to_slash() {
        let path: PathBuf = ["foo", "bar", "baz.txt"].iter().collect();
        assert_eq!(path.to_slash(), "foo/bar/baz.txt");

        assert_eq!(
            Path::new("already/slashes.txt").to_slash(),
            "already/slashes.txt"
        );
    }

    #[test]
    fn test_looks_like_directory() {
        assert!(Path::new("foo/bar").looks_like_directory());
        assert!(Path::new("foo").looks_like_directory());

        assert!(Path::new("foo/bar/").looks_like_directory());
        assert!(Path::new("foo.dir/").looks_like_directory());

        let with_sep = format!("some_dir{}", std::path::MAIN_SEPARATOR);
        assert!(Path::new(&with_sep).looks_like_directory());

        assert!(!Path::new("foo.txt").looks_like_directory());
        assert!(!Path::new("foo/bar.txt").looks_like_directory());

        let file_path: PathBuf = ["foo", "bar", "baz.txt"].iter().collect();
        assert!(!file_path.looks_like_directory());
        let dir_path: PathBuf = ["foo", "bar"].iter().collect();
        assert!(dir_path.looks_like_directory());

        // Need to decide the following cases.
        // For now, we treat them per the implementation of `.extension()`
        assert!(!Path::new("foo.").looks_like_directory());
        assert!(Path::new(".hidden").looks_like_directory());
    }
}
