use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use rand::{distributions::Alphanumeric, Rng};

use crate::prelude::*;

fn temp_path() -> PathBuf {
    let mut temp_path = fs::canonicalize(env::temp_dir()).unwrap();

    temp_path.push(format!(
        ".tmp_{}",
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(20)
            .collect::<String>()
    ));

    temp_path
}

pub(crate) struct NamedFile {
    path: PathBuf,
    file: File,
    temp: bool,
}

impl NamedFile {
    pub fn new_temp() -> AppResult<Self> {
        Self::create(temp_path(), true, true)
    }

    pub fn new(path: PathBuf) -> AppResult<Self> {
        Self::create(path, true, false)
    }

    pub fn open(path: PathBuf) -> AppResult<Self> {
        Self::create(path, false, false)
    }

    fn create(path: PathBuf, create_new: bool, temp: bool) -> AppResult<Self> {
        let mut open_options = OpenOptions::new();
        open_options.read(true).write(true);

        if create_new {
            open_options.create_new(true);
        } else {
            open_options.create(true);
        }

        let file = open_options.mode(0o666).open(&path)?;

        Ok(Self { path, file, temp })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn as_file(&self) -> &File {
        &self.file
    }

    pub fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

impl Drop for NamedFile {
    fn drop(&mut self) {
        if self.temp {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Read for NamedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.as_file_mut().read(buf)
    }
}

impl<'a> Read for &'a NamedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.as_file().read(buf)
    }
}

impl Write for NamedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.as_file_mut().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.as_file_mut().flush()
    }
}

impl<'a> Write for &'a NamedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.as_file().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.as_file().flush()
    }
}

impl Seek for NamedFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.as_file_mut().seek(pos)
    }
}

impl<'a> Seek for &'a NamedFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.as_file().seek(pos)
    }
}

pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> AppResult<Self> {
        Self::new_in(temp_path())
    }

    pub fn new_in(path: PathBuf) -> AppResult<Self> {
        fs::create_dir(&path)?;

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        println!("Dropping temp dir");
        let _ = fs::remove_dir_all(&self.path);
    }
}
