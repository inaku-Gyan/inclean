use std::path::Path;

pub trait PathSlashExt {
    /// Converts a path to a string using forward slashes `/` as the separator.
    /// This is useful for generating diffs or paths in cross-platform formats.
    fn to_slash(&self) -> String;
}

impl PathSlashExt for Path {
    fn to_slash(&self) -> String {
        self.iter()
            .map(|c| c.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}
