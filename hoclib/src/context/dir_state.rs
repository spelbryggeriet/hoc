use std::{
    ffi::{OsStr, OsString},
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io,
    os::unix::prelude::MetadataExt,
    path::{Iter, Path, PathBuf},
    result::Result as StdResult,
};

use hoclog::error;
use serde::{Deserialize, Serialize};
use thiserror::Error;

fn split_path_by_file_name<'p, P: AsRef<Path>>(
    relative_path: &'p P,
) -> StdResult<(Option<&'p OsStr>, Iter<'p>), PathError> {
    if relative_path.as_ref().is_absolute() {
        return Err(PathError::Absolute(relative_path.as_ref().to_path_buf()));
    }

    let mut dirs = relative_path.as_ref().iter();
    let file_name = dirs
        .next_back()
        .map(|n| {
            if n != "." {
                Ok(n)
            } else {
                Err(PathError::InvalidFileName(n.to_os_string()))
            }
        })
        .transpose()?;

    Ok((file_name, dirs))
}

#[derive(Debug, Error)]
pub enum PathError {
    #[error("path cannot be absolute")]
    Absolute(PathBuf),

    #[error("expected path to point at a directory")]
    ExpectedDirectory(PathBuf),
    #[error("invalid file name: '{}'", _0.to_string_lossy())]
    InvalidFileName(OsString),
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileStateDiff {
    pub path: PathBuf,
    pub removed: bool,
    pub modified: bool,
}

impl Display for FileStateDiff {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let change_type = if self.removed {
            "removed"
        } else if self.modified {
            "modified"
        } else {
            "unchanged"
        };
        write!(f, r#""{}" ({})"#, self.path.to_string_lossy(), change_type)
    }
}

impl FileStateDiff {
    pub fn removed(path: PathBuf) -> Self {
        Self {
            path,
            removed: true,
            modified: false,
        }
    }

    pub fn modified(path: PathBuf) -> Self {
        Self {
            path,
            removed: false,
            modified: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum DirectoryStateError {
    #[error("unexpected file: {}", _0.to_string_lossy())]
    UnexpectedFile(OsString),

    #[error("unexpected dir: {}", _0.to_string_lossy())]
    UnexpectedDir(OsString),

    #[error("path: {0}")]
    Path(#[from] PathError),

    #[error("io: {0}")]
    Io(#[from] io::Error),
}

impl From<DirectoryStateError> for hoclog::Error {
    fn from(err: DirectoryStateError) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryState {
    root_path: PathBuf,
    directories: Vec<DirectoryState>,
    files: Vec<FileState>,
}

impl DirectoryState {
    pub fn new<P: Into<PathBuf>>(root_path: P) -> Result<Self, DirectoryStateError> {
        let root_path = root_path.into();

        let metadata = fs::metadata(&root_path)?;
        if metadata.is_dir() {
            Ok(Self {
                root_path,
                directories: Vec::new(),
                files: Vec::new(),
            })
        } else {
            Err(PathError::ExpectedDirectory(root_path).into())
        }
    }

    pub fn new_unchecked<P: Into<PathBuf>>(root_path: P) -> Self {
        Self {
            root_path: root_path.into(),
            directories: Vec::new(),
            files: Vec::new(),
        }
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn name(&self) -> Option<&OsStr> {
        self.root_path.file_name()
    }

    pub fn contains<P: AsRef<Path>>(&self, relative_path: P) -> Result<bool, DirectoryStateError> {
        let (file_name, dir_names) = split_path_by_file_name(&relative_path)?;

        let mut dir_state = self;
        for dir_name in dir_names {
            dir_state = if let Some(index) = dir_state
                .directories
                .iter()
                .position(|d| d.name() == Some(dir_name))
            {
                &dir_state.directories[index]
            } else {
                return Ok(false);
            };
        }

        if let Some(file_name) = file_name {
            if dir_state
                .directories
                .iter()
                .any(|d| d.name() == Some(file_name))
            {
                Ok(true)
            } else if dir_state.files.iter().any(|f| f.name() == file_name) {
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(true)
        }
    }

    pub fn register_file<P: AsRef<Path>>(
        &mut self,
        relative_path: P,
    ) -> Result<(), DirectoryStateError> {
        let (file_name, dir_state) = self.traverse_mut(&relative_path)?;

        let mut path = dir_state.root_path.clone();
        fs::create_dir_all(&path)?;
        path.push(file_name.ok_or(PathError::InvalidFileName(OsString::new()))?);

        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                File::create(&path)?.metadata()?
            }
            Err(error) => return Err(error.into()),
        };

        if metadata.file_type().is_dir() {
            Err(DirectoryStateError::UnexpectedDir(path.into_os_string()).into())
        } else if !metadata.file_type().is_file() {
            let file_name = path.file_name().unwrap();

            if dir_state.files.iter().all(|fs| fs.name() != file_name) {
                dir_state.files.push(FileState::new_unchecked(
                    path,
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                ))
            }

            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn register_dir<P: AsRef<Path>>(
        &mut self,
        relative_path: P,
    ) -> Result<(), DirectoryStateError> {
        let (file_name, dir_state) = self.traverse_mut(&relative_path)?;

        let mut path = dir_state.root_path.clone();
        if let Some(file_name) = file_name {
            path.push(file_name);
        }

        fs::create_dir_all(&path)?;
        let metadata = fs::metadata(&path)?;

        if metadata.file_type().is_file() {
            Err(DirectoryStateError::UnexpectedFile(path.into_os_string()).into())
        } else if metadata.file_type().is_dir() {
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                let file_name = entry.file_name();
                let file_type = entry.file_type()?;

                if file_type.is_file() {
                    if dir_state.files.iter().all(|fs| fs.name() != file_name) {
                        let mut path = path.clone();
                        path.push(file_name);

                        let metadata = entry.metadata()?;

                        dir_state.files.push(FileState::new_unchecked(
                            path,
                            metadata.ctime(),
                            metadata.ctime_nsec(),
                        ));
                    };
                } else if file_type.is_dir() {
                    if dir_state
                        .directories
                        .iter()
                        .all(|ds| ds.name() != Some(&file_name))
                    {
                        let mut path = path.clone();
                        path.push(&file_name);

                        let ds = DirectoryState::new_unchecked(&path);
                        let mut subpath = relative_path.as_ref().to_path_buf();
                        subpath.push(&file_name);
                        dir_state.register_dir(&subpath)?;
                        dir_state.directories.push(ds);
                    }
                }
            }

            Ok(())
        } else {
            Ok(())
        }
    }

    pub fn update_states(&mut self) -> Result<(), DirectoryStateError> {
        let mut i = 0;
        while i < self.files.len() {
            match fs::metadata(&self.files[i].path) {
                Ok(metadata) => {
                    self.files[i].modified = (metadata.ctime(), metadata.ctime_nsec());
                    i += 1;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.files.remove(i);
                }
                Err(error) => return Err(error.into()),
            }
        }

        i = 0;
        while i < self.directories.len() {
            match fs::metadata(&self.directories[i].root_path) {
                Ok(_) => {
                    self.directories[i].update_states()?;
                    i += 1;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    self.directories.remove(i);
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(())
    }

    pub fn unregister_path<P: AsRef<Path>>(
        &mut self,
        relative_path: P,
    ) -> Result<(), DirectoryStateError> {
        if !self.contains(&relative_path)? {
            return Ok(());
        }

        let (file_name, dir_state) = self.traverse_mut(&relative_path)?;

        if let Some(file_name) = file_name {
            if let Some(index) = dir_state.files.iter().position(|fs| fs.name() == file_name) {
                dir_state.files.remove(index);
            } else if let Some(index) = dir_state
                .directories
                .iter()
                .position(|ds| ds.name() == Some(file_name))
            {
                dir_state.directories.remove(index);
            }
        } else {
            dir_state.directories.clear();
            dir_state.files.clear();
        }

        Ok(())
    }

    pub fn unregister_files(&mut self, other: &Self) -> Self {
        let mut removed_files = Self::new_unchecked(&self.root_path);

        let mut i = 0;
        while i < self.files.len() {
            if other
                .files
                .iter()
                .find(|fs_other| self.files[i].name() == fs_other.name())
                .is_some()
            {
                removed_files.files.push(self.files.remove(i));
            } else {
                i += 1;
            }
        }

        i = 0;
        while i < self.directories.len() {
            if let Some(ds_other) = other
                .directories
                .iter()
                .find(|ds_other| self.directories[i].name() == ds_other.name())
            {
                if ds_other.is_empty() {
                    removed_files.directories.push(self.directories.remove(i));
                } else {
                    removed_files
                        .directories
                        .push(self.directories[i].unregister_files(ds_other))
                }
            }
        }

        removed_files
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.directories.is_empty()
    }

    pub fn merge(&mut self, other: Self) {
        for other_fs in other.files {
            if let Some(fs) = self
                .files
                .iter_mut()
                .find(|fs| fs.name() == other_fs.name())
            {
                if fs.modified < other_fs.modified {
                    *fs = other_fs;
                }
            } else {
                self.files.push(other_fs);
            }
        }

        for other_ds in other.directories {
            if let Some(ds) = self
                .directories
                .iter_mut()
                .find(|ds| ds.name() == other_ds.name())
            {
                ds.merge(other_ds);
            } else {
                self.directories.push(other_ds);
            }
        }
    }

    pub fn changed_files(&self, other: &Self) -> Self {
        let mut modified = Self::new_unchecked(&self.root_path);

        for fs in &self.files {
            if let Some(other_fs) = other
                .files
                .iter()
                .find(|other_fs| fs.name() == other_fs.name())
            {
                if fs.modified != other_fs.modified {
                    modified.files.push(fs.clone());
                }
            } else {
                modified.files.push(fs.clone());
            }
        }

        for ds in &self.directories {
            if let Some(other_ds) = other
                .directories
                .iter()
                .find(|other_ds| ds.name() == other_ds.name())
            {
                let ds_modified = ds.changed_files(other_ds);
                if !ds_modified.is_empty() {
                    modified.directories.push(ds_modified);
                }
            }
        }

        modified
    }

    pub fn diff_files(&self, other: &Self) -> Vec<FileStateDiff> {
        let mut file_state_changes = Vec::new();

        let mut stack = vec![((self, other), PathBuf::new())];

        while let Some(((dir, other_dir), mut path)) = stack.pop() {
            for ds in dir.directories.iter().rev() {
                if let Some(other_d) = other_dir
                    .directories
                    .iter()
                    .find(|other_ds| ds.name() == other_ds.name())
                {
                    let mut path = path.clone();
                    path.push(&ds.name().unwrap());
                    stack.push(((ds, other_d), path));
                }
            }

            for fs in dir.files.iter() {
                path.push(&fs.name());

                if let Some(other_fs) = other_dir
                    .files
                    .iter()
                    .find(|other_fs| fs.name() == other_fs.name())
                {
                    if fs.modified != other_fs.modified {
                        file_state_changes.push(FileStateDiff::modified(path.clone()));
                    }
                } else {
                    file_state_changes.push(FileStateDiff::removed(path.clone()));
                }

                path.pop();
            }
        }

        file_state_changes
    }

    fn traverse_mut<'p, P: AsRef<Path>>(
        &mut self,
        relative_path: &'p P,
    ) -> StdResult<(Option<&'p OsStr>, &mut Self), PathError> {
        let (file_name, dir_names) = split_path_by_file_name(relative_path)?;

        let mut dir_state = self;
        for dir_name in dir_names {
            dir_state = if let Some(index) = dir_state
                .directories
                .iter()
                .position(|d| d.name() == Some(dir_name))
            {
                &mut dir_state.directories[index]
            } else {
                let mut path = dir_state.root_path.clone();
                path.push(dir_name);

                dir_state
                    .directories
                    .push(DirectoryState::new_unchecked(path));
                dir_state.directories.last_mut().unwrap()
            };
        }

        Ok((file_name, dir_state))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    path: PathBuf,
    modified: (i64, i64),
}

impl FileState {
    fn new_unchecked<P: Into<PathBuf>>(path: P, ctime: i64, ctime_nsec: i64) -> Self {
        let path = path.into();

        Self {
            path,
            modified: (ctime, ctime_nsec),
        }
    }

    pub fn name(&self) -> &OsStr {
        self.path.file_name().unwrap()
    }
}
