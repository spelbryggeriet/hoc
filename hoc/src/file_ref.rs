use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::Result;

#[derive(Serialize, Deserialize)]
pub struct FileRef {
    path: PathBuf,
}

impl FileRef {
    pub fn new<S>(path: S) -> Self
    where
        S: AsRef<OsStr>,
    {
        Self {
            path: PathBuf::from(&path),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> Result<bool> {
        match fs::metadata(&self.path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}
