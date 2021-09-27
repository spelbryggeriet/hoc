use std::{
    borrow::Cow,
    ffi::OsStr,
    fmt::{self, Display, Formatter},
    fs::{self, File, Metadata, OpenOptions},
    io,
    io::Write,
    os::unix::prelude::MetadataExt,
    path::{Iter, Path, PathBuf},
    result::Result as StdResult,
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Result;

fn split_virtual_path_by_file_name<'p, P: AsRef<Path>>(
    virtual_path: &'p P,
) -> StdResult<(&'p OsStr, Iter<'p>), VirtualPathError> {
    if virtual_path.as_ref().is_absolute() {
        return Err(VirtualPathError::AbsolutePath);
    }

    let mut dirs = virtual_path.as_ref().iter();
    let file_name = dirs.next_back().ok_or(VirtualPathError::EmptyPath)?;
    Ok((file_name, dirs))
}

fn ctime_to_system_time(metadata: Metadata) -> SystemTime {
    let ctime = metadata.ctime() as u64;
    let ctime_nsec = metadata.ctime_nsec() as u32;
    SystemTime::UNIX_EPOCH + Duration::new(ctime, ctime_nsec)
}

#[derive(Debug, Error)]
pub enum VirtualPathError {
    #[error("`FileRef` cannot be created from an absolute path")]
    AbsolutePath,

    #[error("`FileRef` cannot be created from an empty path")]
    EmptyPath,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryState {
    dir_name: String,
    dir_states: Vec<DirectoryState>,
    file_states: Vec<FileState>,
}

impl DirectoryState {
    pub fn new<S: AsRef<OsStr>>(dir_name: S) -> Self {
        Self {
            dir_name: dir_name.as_ref().to_string_lossy().into_owned(),
            dir_states: Vec::new(),
            file_states: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.dir_states.clear();
        self.file_states.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.file_states.is_empty() && self.dir_states.iter().all(Self::is_empty)
    }

    pub fn get_snapshot<P>(path: &P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let mut dir_states = Vec::new();
        let mut file_states = Vec::new();

        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dir_states.push(DirectoryState::get_snapshot(&entry.path())?);
            } else {
                file_states.push(FileState::get_snapshot(&entry.path())?);
            }
        }

        Ok(Self {
            dir_name: path
                .as_ref()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            dir_states,
            file_states,
        })
    }

    pub fn merge(&mut self, other: Self) {
        if self.dir_name != other.dir_name {
            return;
        }

        for other_fs in other.file_states {
            if let Some(fs) = self
                .file_states
                .iter_mut()
                .find(|fs| fs.file_name == other_fs.file_name)
            {
                if fs.modified < other_fs.modified {
                    *fs = other_fs;
                }
            } else {
                self.file_states.push(other_fs);
            }
        }

        for other_ds in other.dir_states {
            if let Some(ds) = self
                .dir_states
                .iter_mut()
                .find(|ds| ds.dir_name == other_ds.dir_name)
            {
                ds.merge(other_ds);
            } else {
                self.dir_states.push(other_ds);
            }
        }
    }

    pub fn remove_files(&mut self, other: &Self) -> Self {
        let mut removed_files = Self::new(self.dir_name.clone());

        if self.dir_name != other.dir_name {
            return removed_files;
        }

        let mut i = 0;
        while i < self.file_states.len() {
            if other
                .file_states
                .iter()
                .find(|fs_other| self.file_states[i].file_name == fs_other.file_name)
                .is_some()
            {
                removed_files.file_states.push(self.file_states.remove(i));
            } else {
                i += 1;
            }
        }

        i = 0;
        while i < self.dir_states.len() {
            let ds = &mut self.dir_states[i];
            if let Some(ds_other) = other
                .dir_states
                .iter()
                .find(|ds_other| ds.dir_name == ds_other.dir_name)
            {
                removed_files.dir_states.push(ds.remove_files(ds_other));
            } else {
                i += 1;
            }
        }

        removed_files
    }

    pub fn changed_files(&self, other: &Self) -> Self {
        let mut modified = Self::new(&self.dir_name);

        if self.dir_name != other.dir_name {
            return modified;
        }

        for fs in &self.file_states {
            if let Some(other_fs) = other
                .file_states
                .iter()
                .find(|other_fs| fs.file_name == other_fs.file_name)
            {
                if fs.modified != other_fs.modified {
                    modified.file_states.push(fs.clone());
                }
            } else {
                modified.file_states.push(fs.clone());
            }
        }

        for ds in &self.dir_states {
            if let Some(other_ds) = other
                .dir_states
                .iter()
                .find(|other_ds| ds.dir_name == other_ds.dir_name)
            {
                let ds_modified = ds.changed_files(other_ds);
                if !ds_modified.is_empty() {
                    modified.dir_states.push(ds_modified);
                }
            }
        }

        modified
    }

    pub fn file_changes(&self, other: &Self) -> Vec<FileStateDiff> {
        let mut file_state_changes = Vec::new();

        if self.dir_name != other.dir_name {
            return file_state_changes;
        }

        let mut stack = vec![((self, other), PathBuf::from(&self.dir_name))];
        while let Some(((dir_state, other_dir_state), mut path)) = stack.pop() {
            for ds in dir_state.dir_states.iter().rev() {
                if let Some(other_ds) = other_dir_state
                    .dir_states
                    .iter()
                    .find(|other_ds| ds.dir_name == other_ds.dir_name)
                {
                    let mut path = path.clone();
                    path.push(&ds.dir_name);
                    stack.push(((ds, other_ds), path));
                }
            }

            for fs in &dir_state.file_states {
                path.push(&fs.file_name);

                if let Some(other_fs) = other_dir_state
                    .file_states
                    .iter()
                    .find(|other_fs| fs.file_name == other_fs.file_name)
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

    pub fn file_reader<P: AsRef<Path>>(&self, virtual_path: P) -> Result<File> {
        Ok(File::open(virtual_path)?)
    }

    pub fn file_writer<P1, P2>(&mut self, virtual_path: P1, actual_path: P2) -> Result<FileWriter>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        let file_state = self.get_or_create_file_state_mut(virtual_path)?;

        FileWriter::new(&actual_path, file_state)
    }

    fn get_or_create_file_state_mut<P: AsRef<Path>>(
        &mut self,
        virtual_path: P,
    ) -> Result<&mut FileState> {
        let (file_name, dirs) = split_virtual_path_by_file_name(&virtual_path)?;

        let mut dir_state = self;
        for dir_name in dirs {
            if let Some(index) = dir_state
                .dir_states
                .iter()
                .position(|ds| OsStr::new(&ds.dir_name) == dir_name)
            {
                dir_state = &mut dir_state.dir_states[index];
            } else {
                dir_state.dir_states.push(Self::new(&dir_name));
                dir_state = dir_state.dir_states.last_mut().unwrap();
            }
        }

        let file_state = if let Some(index) = dir_state
            .file_states
            .iter()
            .position(|fs| OsStr::new(&fs.file_name) == file_name)
        {
            &mut dir_state.file_states[index]
        } else {
            dir_state.file_states.push(FileState::new(&file_name));
            dir_state.file_states.last_mut().unwrap()
        };

        Ok(file_state)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    file_name: String,
    modified: SystemTime,
}

impl FileState {
    fn new<S: AsRef<OsStr>>(file_name: &S) -> Self {
        Self {
            file_name: file_name.as_ref().to_string_lossy().into_owned(),
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    fn get_snapshot<P>(path: &P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let mut path = Cow::Borrowed(path.as_ref());

        while fs::symlink_metadata(&path)?.file_type().is_symlink() {
            path = Cow::Owned(fs::read_link(path)?);
        }

        let modified = ctime_to_system_time(fs::metadata(&path)?);

        Ok(Self {
            file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
            modified,
        })
    }

    fn update_modified_time(&mut self, modified: SystemTime) {
        self.modified = modified;
    }
}

pub struct FileWriter<'a> {
    file: File,
    file_state: &'a mut FileState,
    is_finished: bool,
}

impl Write for FileWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Drop for FileWriter<'_> {
    fn drop(&mut self) {
        assert!(self.is_finished, "cannot drop an unfinished `FileWriter`");
    }
}

impl<'f> FileWriter<'f> {
    pub fn new<P: AsRef<Path>>(path: P, file_state: &'f mut FileState) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            match fs::metadata(parent) {
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir_all(parent)?;
                }
                Err(error) => return Err(error.into()),
            }
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;

        file_state.modified = ctime_to_system_time(file.metadata()?);

        Ok(Self {
            file,
            file_state,
            is_finished: false,
        })
    }

    pub fn write_and_finish(mut self, buf: &[u8]) -> Result<()> {
        self.write(buf)?;
        self.finish()
    }

    pub fn finish(mut self) -> Result<()> {
        self.file_state
            .update_modified_time(ctime_to_system_time(self.file.metadata()?));
        self.is_finished = true;
        Ok(())
    }
}
