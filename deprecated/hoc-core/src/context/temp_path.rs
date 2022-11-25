use std::{
    fmt::{self, Debug, Formatter},
    fs,
    path::{Path, PathBuf},
};

pub struct TempPath {
    path: Box<Path>,
}

impl TempPath {
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into().into_boxed_path(),
        }
    }
}

impl Debug for TempPath {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.path.fmt(f)
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = match self.path.metadata() {
            Ok(md) if md.is_file() => fs::remove_file(&self.path),
            Ok(md) if md.is_dir() => fs::remove_dir_all(&self.path),
            _ => return,
        };
    }
}
