use std::{
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File},
    io::{self, BufReader, Read},
    os::unix::prelude::MetadataExt,
    path::{Path, PathBuf},
    slice,
};

use hoclog::error;
use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

pub mod dir_comp;

const PERMISSION_BITS: u32 = 0o777;

fn serialize_mode<S: Serializer>(mode: &u32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("{mode:03o}"))
}

fn serialize_checksum<S: Serializer>(
    checksum: &[u8; 32],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(
        &checksum
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

fn deserialize_mode<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    struct ModeVisitor;

    impl<'de> Visitor<'de> for ModeVisitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if v.is_ascii() && v.len() != 3 {
                return Err(serde::de::Error::custom("expected 3 octal digits"));
            }

            u32::from_str_radix(v, 8).map_err(serde::de::Error::custom)
        }
    }

    deserializer.deserialize_str(ModeVisitor)
}

fn deserialize_checksum<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
    struct ChecksumVisitor;

    impl<'de> Visitor<'de> for ChecksumVisitor {
        type Value = [u8; 32];

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if v.is_ascii() && v.len() != 64 {
                return Err(serde::de::Error::custom("expected 64 hexadecimal digits"));
            }

            let mut output = [0; 32];
            for (i, elem) in output.iter_mut().enumerate() {
                *elem = u8::from_str_radix(&v[2 * i..2 * (i + 1)], 16)
                    .map_err(serde::de::Error::custom)?;
            }

            return Ok(output);
        }
    }

    deserializer.deserialize_str(ChecksumVisitor)
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("not a directory: {}", _0.to_string_lossy())]
    NotADir(PathBuf),

    #[error("directory not empyu: {}", _0.to_string_lossy())]
    NonEmptyDir(PathBuf),

    #[error("unexpected file: {}", _0.to_string_lossy())]
    UnexpectedFile(OsString),

    #[error("unexpected dir: {}", _0.to_string_lossy())]
    UnexpectedDir(OsString),

    #[error("io: {0}")]
    Io(#[from] io::Error),
}

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirState {
    path: PathBuf,

    #[serde(
        serialize_with = "serialize_mode",
        deserialize_with = "deserialize_mode"
    )]
    mode: u32,

    #[serde(rename = "directories")]
    dirs: Vec<DirState>,
    files: Vec<FileState>,

    #[serde(skip)]
    tracked_files: Vec<PathBuf>,

    #[serde(skip)]
    untracked_files: Vec<PathBuf>,
}

impl DirState {
    pub fn empty<P: Into<PathBuf>>(path: P) -> Result<Self, Error> {
        let path = path.into();
        let metadata = fs::metadata(&path)?;

        if !metadata.is_dir() {
            return Err(Error::NotADir(path));
        }

        Ok(Self::empty_unchecked(path, metadata.mode()))
    }

    fn empty_unchecked(path: PathBuf, mode: u32) -> Self {
        Self {
            path,
            mode: mode & PERMISSION_BITS,
            dirs: Vec::new(),
            files: Vec::new(),
            tracked_files: Vec::new(),
            untracked_files: Vec::new(),
        }
    }

    pub fn from_dir<P: Into<PathBuf>>(root_path: P) -> Result<Self, Error> {
        let root_path = root_path.into();
        let metadata = fs::metadata(&root_path)?;

        if !metadata.is_dir() {
            return Err(Error::NotADir(root_path));
        }

        Self::from_dir_impl(root_path, metadata.mode() & PERMISSION_BITS, &[])
    }

    fn from_dir_impl(
        root_path: PathBuf,
        mode: u32,
        exclude_paths: &[PathBuf],
    ) -> Result<Self, Error> {
        let mut dir_state = Self {
            path: root_path,
            mode,
            dirs: Vec::new(),
            files: Vec::new(),
            tracked_files: Vec::new(),
            untracked_files: Vec::new(),
        };

        for entry in fs::read_dir(dir_state.path())? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let path = entry.path();

            if exclude_paths.iter().any(|p| path == *p) {
                continue;
            }

            if metadata.is_dir() {
                dir_state.dirs.push(Self::from_dir_impl(
                    path,
                    metadata.mode() & PERMISSION_BITS,
                    exclude_paths,
                )?);
            } else {
                dir_state.files.push(FileState::new(
                    path,
                    metadata.mode() & PERMISSION_BITS,
                    metadata.len(),
                )?);
            }
        }

        Ok(dir_state)
    }

    pub fn is_commited<P: AsRef<Path>>(&self, path_suffix: P) -> bool {
        let prefix = &self.path;

        for file in &self.files {
            if file.path.strip_prefix(prefix).unwrap() == path_suffix.as_ref() {
                return true;
            }
        }

        for dir in &self.dirs {
            if path_suffix
                .as_ref()
                .starts_with(dir.path.strip_prefix(prefix).unwrap())
            {
                return dir.is_commited(path_suffix.as_ref());
            }
        }

        false
    }

    pub fn is_tracked<P: AsRef<Path>>(&self, file_suffix: P) -> bool {
        let prefix = &self.path;
        self.tracked_files
            .iter()
            .any(|tp| file_suffix.as_ref() == tp.strip_prefix(prefix).unwrap())
    }

    pub fn is_untracked<P: AsRef<Path>>(&self, file_suffix: P) -> bool {
        let prefix = &self.path;
        self.untracked_files
            .iter()
            .any(|tp| file_suffix.as_ref() == tp.strip_prefix(prefix).unwrap())
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.dirs.is_empty()
    }

    pub fn track<P: AsRef<Path>>(&mut self, file_suffix: P) -> PathBuf {
        let mut new_path = self.path.clone();
        new_path.push(&file_suffix);

        if self.is_commited(&file_suffix) || self.is_tracked(&file_suffix) {
            return new_path;
        }

        let indices: Vec<_> = self
            .untracked_files
            .iter()
            .enumerate()
            .filter_map(|(i, p)| (*p == new_path).then(|| i))
            .collect();

        for index in indices.into_iter().rev() {
            self.untracked_files.swap_remove(index);
        }

        self.tracked_files.push(new_path.clone());
        new_path
    }

    pub fn untrack<P: AsRef<Path>>(&mut self, path_suffix: P) {
        if !self.is_commited(&path_suffix) || self.is_untracked(&path_suffix) {
            return;
        }

        let mut new_path = self.path.clone();
        new_path.push(path_suffix);

        let indices: Vec<_> = self
            .tracked_files
            .iter()
            .enumerate()
            .filter_map(|(i, p)| (*p == new_path).then(|| i))
            .collect();

        for index in indices.into_iter().rev() {
            self.tracked_files.swap_remove(index);
        }

        self.untracked_files.push(new_path);
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        let tracked_paths: Vec<_> = self.tracked_files.drain(..).collect();
        let untracked_paths: Vec<_> = self.untracked_files.drain(..).collect();

        for path in tracked_paths {
            let metadata = path.metadata()?;

            let mut tracker = self.path.clone();
            let mut dir_state = &mut *self;

            if let Some(parent) = path.strip_prefix(&tracker).unwrap().parent() {
                for name in parent.iter() {
                    tracker.push(name);

                    if let Some(index) = dir_state.dirs.iter().position(|ds| ds.path == tracker) {
                        dir_state = &mut dir_state.dirs[index];
                    } else {
                        dir_state.dirs.push(Self::empty(&tracker)?);
                        dir_state = dir_state.dirs.last_mut().unwrap();
                    }
                }
            }

            if metadata.is_dir() {
                dir_state
                    .dirs
                    .push(Self::empty_unchecked(path, metadata.mode()));
            } else {
                dir_state.files.push(FileState::new(
                    path,
                    metadata.mode() & PERMISSION_BITS,
                    metadata.len(),
                )?);
            }
        }

        for path in untracked_paths {
            let mut tracker = self.path.clone();
            let mut dir_state = &mut *self;

            if let Some(parent) = path.strip_prefix(&tracker).unwrap().parent() {
                for name in parent.iter() {
                    tracker.push(name);
                    let index = dir_state
                        .dirs
                        .iter()
                        .position(|ds| ds.path == tracker)
                        .expect("non-existing untracked path");
                    dir_state = &mut dir_state.dirs[index];
                }
            }

            if let Some(index) = self.files.iter().position(|fs| fs.path == path) {
                self.files.remove(index);
            } else if let Some(index) = self.dirs.iter().position(|ds| ds.path == path) {
                if !self.dirs[index].is_empty() {
                    return Err(Error::NonEmptyDir(self.dirs[index].path.clone()));
                }

                self.dirs.remove(index);
            } else {
                unreachable!("non-existing untracked path");
            }
        }

        Ok(())
    }

    pub fn refresh(&mut self) -> Result<(), Error> {
        let new_mode = self.path.metadata()?.mode() & PERMISSION_BITS;
        if new_mode != self.mode {
            self.mode = new_mode;
        }

        for fs in &mut self.files {
            fs.refresh()?;
        }

        for ds in &mut self.dirs {
            ds.refresh()?;
        }

        Ok(())
    }

    pub fn refresh_modes(&mut self) -> Result<(), Error> {
        let new_mode = self.path.metadata()?.mode() & PERMISSION_BITS;
        if new_mode != self.mode {
            self.mode = new_mode;
        }

        for fs in &mut self.files {
            fs.refresh_mode()?;
        }

        for ds in &mut self.dirs {
            ds.refresh_modes()?;
        }

        Ok(())
    }

    pub fn files(&self) -> slice::Iter<FileState> {
        self.files.iter()
    }

    pub fn all_files(&self) -> FileStateIter {
        FileStateIter::new(self)
    }

    pub fn dirs(&self) -> slice::Iter<Self> {
        self.dirs.iter()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn mode(&self) -> u32 {
        self.mode
    }

    pub fn name(&self) -> Option<&OsStr> {
        self.path.file_name()
    }
}

pub struct FileStateIter<'state> {
    files: slice::Iter<'state, FileState>,
    dirs: slice::Iter<'state, DirState>,
    dir_files: Option<Box<FileStateIter<'state>>>,
}

impl<'state> FileStateIter<'state> {
    fn new(dir_state: &'state DirState) -> Self {
        Self {
            files: dir_state.files.iter(),
            dirs: dir_state.dirs.iter(),
            dir_files: None,
        }
    }
}

impl<'state> Iterator for FileStateIter<'state> {
    type Item = &'state FileState;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fs) = self.files.next() {
            return Some(fs);
        }

        loop {
            if let Some(iter_ref) = self.dir_files.as_mut() {
                if let Some(fs) = iter_ref.next() {
                    return Some(fs);
                } else {
                    drop(iter_ref);
                    self.dir_files = None;
                }
            }

            let ds = self.dirs.next()?;
            self.dir_files = Some(Box::new(ds.all_files()));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    path: PathBuf,

    #[serde(
        serialize_with = "serialize_mode",
        deserialize_with = "deserialize_mode"
    )]
    mode: u32,

    #[serde(
        serialize_with = "serialize_checksum",
        deserialize_with = "deserialize_checksum"
    )]
    checksum: [u8; 32],
}

impl FileState {
    const BLOCK_SIZE_1_MB: usize = 1_048_576;
    const BLOCK_SIZE_8_KB: usize = 8 * 1024;

    fn new(path: PathBuf, mode: u32, len: u64) -> Result<Self, io::Error> {
        Ok(Self {
            checksum: Self::calculate_checksum(&path, len)?,
            path,
            mode,
        })
    }

    pub fn calculate_checksum(file_path: &Path, file_len: u64) -> Result<[u8; 32], io::Error> {
        let file = File::open(file_path)?;
        let block_size_bytes = if file_len >= 8 * Self::BLOCK_SIZE_1_MB as u64 {
            Self::BLOCK_SIZE_1_MB
        } else {
            Self::BLOCK_SIZE_8_KB
        };

        let mut reader = BufReader::with_capacity(block_size_bytes, file);
        let mut buf = vec![0; block_size_bytes];
        let mut hasher = blake3::Hasher::new();

        let mut bytes_read = 0;
        loop {
            match reader.read(&mut buf) {
                Ok(n) => {
                    bytes_read += n;
                    hasher.update(&buf[..n]);
                }
                Err(err) => return Err(err.into()),
            }

            if bytes_read as u64 == file_len {
                break;
            }
        }

        Ok(hasher.finalize().into())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn mode(&self) -> u32 {
        self.mode
    }

    pub fn checksum(&self) -> [u8; 32] {
        self.checksum
    }

    pub fn name(&self) -> &OsStr {
        self.path.file_name().unwrap()
    }

    pub fn refresh(&mut self) -> Result<(), io::Error> {
        let metadata = self.path.metadata()?;
        let new_checksum = Self::calculate_checksum(&self.path, metadata.len())?;
        self.mode = metadata.mode() & PERMISSION_BITS;
        self.checksum = new_checksum;

        Ok(())
    }

    pub fn refresh_mode(&mut self) -> Result<(), io::Error> {
        let metadata = self.path.metadata()?;
        self.mode = metadata.mode() & PERMISSION_BITS;

        Ok(())
    }
}
