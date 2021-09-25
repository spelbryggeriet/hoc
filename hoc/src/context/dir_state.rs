use std::{
    borrow::Cow,
    ffi::OsStr,
    fs::{self, File, Metadata, OpenOptions},
    io,
    io::Write,
    os::unix::prelude::MetadataExt,
    path::{Iter, Path},
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
pub struct DirectoryStateDiff<'a> {
    pub dir_name: &'a str,
    pub removed: bool,
    pub dir_states_changes: Vec<DirectoryStateDiff<'a>>,
    pub file_states_changes: Vec<FileStateDiff<'a>>,
}

impl<'a> DirectoryStateDiff<'a> {
    pub fn new<S: AsRef<str>>(dir_name: &'a S) -> Self {
        Self {
            dir_name: dir_name.as_ref(),
            removed: false,
            dir_states_changes: Vec::new(),
            file_states_changes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.removed
            && self.dir_states_changes.iter().all(Self::is_empty)
            && self.file_states_changes.iter().all(FileStateDiff::is_empty)
    }

    pub fn changed_paths(&self) -> Vec<(String, &'static str)> {
        if self.removed {
            vec![(self.dir_name.to_owned(), "removed")]
        } else {
            self.file_states_changes
                .iter()
                .map(|fsc| {
                    (
                        format!("{}/{}", self.dir_name, fsc.file_name),
                        if fsc.removed {
                            "removed"
                        } else {
                            fsc.modified_change.map_or("unchanged", |_| "modified")
                        },
                    )
                })
                .chain(self.dir_states_changes.iter().flat_map(|dsc| {
                    dsc.changed_paths()
                        .into_iter()
                        .map(move |(path, change_type)| {
                            (format!("{}/{}", self.dir_name, path), change_type)
                        })
                }))
                .collect()
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileStateDiff<'a> {
    pub file_name: &'a str,
    pub removed: bool,
    pub modified_change: Option<SystemTime>,
}

impl<'a> FileStateDiff<'a> {
    pub fn new<S: AsRef<str>>(file_name: &'a S) -> Self {
        Self {
            file_name: file_name.as_ref(),
            removed: false,
            modified_change: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.removed && self.modified_change.is_none()
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    pub fn diff(&self, other: &Self) -> DirectoryStateDiff {
        let mut diff = DirectoryStateDiff::new(&self.dir_name);

        if self.dir_name != other.dir_name {
            diff.removed = true;
            return diff;
        }

        diff.dir_states_changes = self
            .dir_states
            .iter()
            .map(|ds| {
                other
                    .dir_states
                    .iter()
                    .find(|ds_other| ds_other.dir_name == ds.dir_name)
                    .map(|ds_other| ds.diff(ds_other))
                    .unwrap_or_else(|| {
                        let mut ds_diff = DirectoryStateDiff::new(&ds.dir_name);
                        ds_diff.removed = true;
                        ds_diff
                    })
            })
            .filter(|diff| !diff.is_empty())
            .collect();

        diff.file_states_changes = self
            .file_states
            .iter()
            .map(|fs| {
                other
                    .file_states
                    .iter()
                    .find(|fs_other| fs.file_name == fs_other.file_name)
                    .map(|fs_other| fs.diff(&fs_other))
                    .unwrap_or_else(|| {
                        let mut fs_diff = FileStateDiff::new(&fs.file_name);
                        fs_diff.removed = true;
                        fs_diff
                    })
            })
            .filter(|diff| !diff.is_empty())
            .collect();

        diff
    }

    pub fn file_reader<P: AsRef<Path>>(&self, path: P) -> Result<File> {
        Ok(File::open(path)?)
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

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    fn diff(&self, other: &Self) -> FileStateDiff {
        let mut diff = FileStateDiff::new(&self.file_name);
        diff.modified_change = (self.modified != other.modified).then(|| other.modified);
        diff
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
