use std::{path::Path, slice::Iter};

use crate::DirState;

use crate::dir_state::{self, FileState};

#[derive(Debug)]
pub struct ModifiedDir<'lhs, 'rhs> {
    path: &'rhs Path,
    modified_mode: Option<(u32, u32)>,
    added_files: Vec<&'rhs FileState>,
    removed_files: Vec<&'lhs FileState>,
    modified_files: Vec<FileComparison<'rhs>>,
    added_dirs: Vec<&'rhs DirState>,
    removed_dirs: Vec<&'lhs DirState>,
    modified_dirs: Vec<DirComparison<'lhs, 'rhs>>,
}

impl ModifiedDir<'_, '_> {
    pub fn path(&self) -> &Path {
        self.path
    }

    pub fn old_mode(&self) -> Option<u32> {
        self.modified_mode.map(|(old_mode, _)| old_mode)
    }

    pub fn has_removed_paths(&self) -> bool {
        !self.removed_files.is_empty()
            || !self.removed_dirs.is_empty()
            || self
                .modified_dirs
                .iter()
                .any(DirComparison::has_removed_paths)
    }

    pub fn has_modified_checksums(&self) -> bool {
        self.modified_files
            .iter()
            .any(FileComparison::has_modified_checksum)
            || self
                .modified_dirs
                .iter()
                .any(DirComparison::has_modified_checksums)
    }
}

#[derive(Debug)]
pub enum DirComparison<'lhs, 'rhs> {
    Same,
    Modified(ModifiedDir<'lhs, 'rhs>),
}

impl<'lhs, 'rhs> DirComparison<'lhs, 'rhs> {
    pub fn compare(lhs: &'lhs DirState, rhs: &'rhs DirState) -> Self {
        let modified_mode = (lhs.mode() != rhs.mode()).then(|| (lhs.mode(), rhs.mode()));

        let added_files: Vec<_> = rhs
            .files()
            .filter(|other_fs| lhs.files().all(|fs| fs.path() != other_fs.path()))
            .collect();

        let removed_files: Vec<_> = lhs
            .files()
            .filter(|fs| rhs.files().all(|other_fs| fs.path() != other_fs.path()))
            .collect();

        let modified_files: Vec<_> = rhs
            .files()
            .filter_map(|other_fs| {
                lhs.files()
                    .find(|fs| fs.path() == other_fs.path())
                    .map(|fs| FileComparison::compare(fs, other_fs))
                    .filter(|dc| matches!(dc, FileComparison::Modified { .. }))
            })
            .collect();

        let added_dirs: Vec<_> = rhs
            .dirs()
            .filter(|other_ds| lhs.dirs().all(|ds| ds.path() != other_ds.path()))
            .collect();

        let removed_dirs: Vec<_> = lhs
            .dirs()
            .filter(|ds| rhs.dirs().all(|other_ds| ds.path() != other_ds.path()))
            .collect();

        let modified_dirs: Vec<_> = rhs
            .dirs()
            .filter_map(|other_ds| {
                lhs.dirs().find(|ds| ds.path() == other_ds.path()).and_then(
                    |ds| match Self::compare(ds, other_ds) {
                        Self::Same => None,
                        dc @ Self::Modified(..) => Some(dc),
                    },
                )
            })
            .collect();

        if modified_mode.is_none()
            && added_files.is_empty()
            && removed_files.is_empty()
            && modified_files.is_empty()
            && added_dirs.is_empty()
            && removed_dirs.is_empty()
            && modified_dirs.is_empty()
        {
            Self::Same
        } else {
            Self::Modified(ModifiedDir {
                path: rhs.path(),
                modified_mode,
                added_files,
                removed_files,
                modified_files,
                added_dirs,
                removed_dirs,
                modified_dirs,
            })
        }
    }

    pub fn all_added_dirs(&self) -> AddedDirs {
        AddedDirs::new(self)
    }

    pub fn all_added_files(&self) -> AddedFiles {
        AddedFiles::new(self)
    }

    pub fn all_modified_dirs(&self) -> ModifiedDirs {
        ModifiedDirs::new(self)
    }

    pub fn all_modified_files(&self) -> ModifiedFiles {
        ModifiedFiles::new(self)
    }

    pub fn has_removed_paths(&self) -> bool {
        match self {
            Self::Same => false,
            Self::Modified(md) => md.has_removed_paths(),
        }
    }

    pub fn has_modified_checksums(&self) -> bool {
        match self {
            Self::Same => false,
            Self::Modified(md) => md.has_modified_checksums(),
        }
    }

    pub fn remove_all_added_paths(&mut self) {
        match self {
            Self::Same => (),
            Self::Modified(ModifiedDir {
                added_files,
                added_dirs,
                modified_dirs,
                ..
            }) => {
                added_files.drain(..);
                added_dirs.drain(..);
                modified_dirs
                    .iter_mut()
                    .for_each(Self::remove_all_added_paths);
                self.reduce();
            }
        }
    }

    fn reduce(&mut self) {
        if let Self::Modified(ModifiedDir {
            modified_mode: new_mode,
            added_files,
            removed_files,
            modified_files,
            added_dirs,
            removed_dirs,
            modified_dirs,
            ..
        }) = self
        {
            modified_files.iter_mut().for_each(FileComparison::reduce);
            *modified_files = modified_files
                .drain(..)
                .filter(|fc| matches!(fc, FileComparison::Modified { .. }))
                .collect();

            modified_dirs.iter_mut().for_each(Self::reduce);
            *modified_dirs = modified_dirs
                .drain(..)
                .filter(|fc| matches!(fc, Self::Modified { .. }))
                .collect();

            if new_mode.is_none()
                && added_files.is_empty()
                && removed_files.is_empty()
                && modified_files.is_empty()
                && added_dirs.is_empty()
                && removed_dirs.is_empty()
                && modified_dirs.is_empty()
            {
                *self = Self::Same;
            }
        }
    }
}

pub struct AddedDirs<'comp: 'state, 'state> {
    added_dirs: Iter<'comp, &'state DirState>,
    modified_dirs: Iter<'comp, DirComparison<'state, 'state>>,
    modified_dir_iter: Option<Box<AddedDirs<'comp, 'state>>>,
}

impl<'comp, 'state> AddedDirs<'comp, 'state> {
    fn new(dir_comp: &'comp DirComparison<'state, 'state>) -> Self {
        match dir_comp {
            DirComparison::Same => Self {
                added_dirs: [].iter(),
                modified_dirs: [].iter(),
                modified_dir_iter: None,
            },
            DirComparison::Modified(ModifiedDir {
                added_dirs,
                modified_dirs,
                ..
            }) => Self {
                added_dirs: added_dirs.iter(),
                modified_dirs: modified_dirs.iter(),
                modified_dir_iter: None,
            },
        }
    }
}

impl<'comp, 'state> Iterator for AddedDirs<'comp, 'state> {
    type Item = &'state DirState;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(ds) = self.added_dirs.next() {
            return Some(ds);
        }

        loop {
            if let Some(iter_ref) = self.modified_dir_iter.as_mut() {
                if let Some(ds) = iter_ref.next() {
                    return Some(ds);
                } else {
                    drop(iter_ref);
                    self.modified_dir_iter = None;
                }
            }

            let dc = self.modified_dirs.next()?;
            self.modified_dir_iter = Some(Box::new(dc.all_added_dirs()));
        }
    }
}

pub struct AddedFiles<'comp: 'state, 'state> {
    added_files: Iter<'comp, &'state FileState>,
    added_dirs: Iter<'comp, &'state DirState>,
    added_dir_iter: Option<dir_state::FileStateIter<'state>>,
    modified_dirs: Iter<'comp, DirComparison<'state, 'state>>,
    modified_dir_iter: Option<Box<AddedFiles<'comp, 'state>>>,
}

impl<'comp, 'state> AddedFiles<'comp, 'state> {
    fn new(dir_comp: &'comp DirComparison<'state, 'state>) -> Self {
        match dir_comp {
            DirComparison::Same => Self {
                added_files: [].iter(),
                added_dirs: [].iter(),
                added_dir_iter: None,
                modified_dirs: [].iter(),
                modified_dir_iter: None,
            },
            DirComparison::Modified(ModifiedDir {
                added_files,
                added_dirs,
                modified_dirs,
                ..
            }) => Self {
                added_files: added_files.iter(),
                added_dirs: added_dirs.iter(),
                added_dir_iter: None,
                modified_dirs: modified_dirs.iter(),
                modified_dir_iter: None,
            },
        }
    }
}

impl<'comp, 'state> Iterator for AddedFiles<'comp, 'state> {
    type Item = &'state FileState;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fs) = self.added_files.next() {
            return Some(fs);
        }

        loop {
            if let Some(iter_ref) = self.added_dir_iter.as_mut() {
                if let Some(fs) = iter_ref.next() {
                    return Some(fs);
                } else {
                    drop(iter_ref);
                    self.added_dir_iter = None;
                }
            }

            if let Some(ds) = self.added_dirs.next() {
                self.added_dir_iter = Some(ds.all_files());
            } else {
                break;
            }
        }

        loop {
            if let Some(iter_ref) = self.modified_dir_iter.as_mut() {
                if let Some(fs) = iter_ref.next() {
                    return Some(fs);
                } else {
                    drop(iter_ref);
                    self.modified_dir_iter = None;
                }
            }

            let dc = self.modified_dirs.next()?;
            self.modified_dir_iter = Some(Box::new(dc.all_added_files()));
        }
    }
}

pub struct ModifiedDirs<'comp: 'state, 'state> {
    root: Option<&'comp ModifiedDir<'state, 'state>>,
    modified_dirs: Iter<'comp, DirComparison<'state, 'state>>,
    modified_dir_iter: Option<Box<ModifiedDirs<'comp, 'state>>>,
}

impl<'comp, 'state> ModifiedDirs<'comp, 'state> {
    fn new(dir_comp: &'comp DirComparison<'state, 'state>) -> Self {
        match dir_comp {
            DirComparison::Same => Self {
                root: None,
                modified_dirs: [].iter(),
                modified_dir_iter: None,
            },
            DirComparison::Modified(md @ ModifiedDir { modified_dirs, .. }) => Self {
                root: Some(md),
                modified_dirs: modified_dirs.iter(),
                modified_dir_iter: None,
            },
        }
    }
}

impl<'comp, 'state> Iterator for ModifiedDirs<'comp, 'state> {
    type Item = &'comp ModifiedDir<'state, 'state>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(md) = self.root.take() {
            return Some(md);
        }

        loop {
            if let Some(iter_ref) = self.modified_dir_iter.as_mut() {
                if let Some(md) = iter_ref.next() {
                    return Some(md);
                } else {
                    drop(iter_ref);
                    self.modified_dir_iter = None;
                }
            }

            let dc = self.modified_dirs.next()?;
            self.modified_dir_iter = Some(Box::new(dc.all_modified_dirs()));
        }
    }
}

pub struct ModifiedFiles<'comp: 'state, 'state> {
    root: Iter<'comp, FileComparison<'state>>,
    modified_dirs: Iter<'comp, DirComparison<'state, 'state>>,
    modified_files_iter: Option<Box<ModifiedFiles<'comp, 'state>>>,
}

impl<'comp, 'state> ModifiedFiles<'comp, 'state> {
    fn new(dir_comp: &'comp DirComparison<'state, 'state>) -> Self {
        match dir_comp {
            DirComparison::Same => Self {
                root: [].iter(),
                modified_dirs: [].iter(),
                modified_files_iter: None,
            },
            DirComparison::Modified(ModifiedDir {
                modified_files,
                modified_dirs,
                ..
            }) => Self {
                root: modified_files.iter(),
                modified_dirs: modified_dirs.iter(),
                modified_files_iter: None,
            },
        }
    }
}

impl<'comp, 'state> Iterator for ModifiedFiles<'comp, 'state> {
    type Item = &'comp ModifiedFile<'state>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(FileComparison::Modified(mf)) = self.root.next() {
            return Some(mf);
        }

        loop {
            if let Some(iter_ref) = self.modified_files_iter.as_mut() {
                if let Some(md) = iter_ref.next() {
                    return Some(md);
                } else {
                    drop(iter_ref);
                    self.modified_files_iter = None;
                }
            }

            let dc = self.modified_dirs.next()?;
            self.modified_files_iter = Some(Box::new(dc.all_modified_files()));
        }
    }
}

#[derive(Debug)]
pub struct ModifiedFile<'rhs> {
    path: &'rhs Path,
    modified_mode: Option<(u32, u32)>,
    new_checksum: Option<[u8; 32]>,
}

impl ModifiedFile<'_> {
    pub fn path(&self) -> &Path {
        self.path
    }

    pub fn old_mode(&self) -> Option<u32> {
        self.modified_mode.map(|(old_mode, _)| old_mode)
    }

    pub fn new_checksum(&self) -> Option<[u8; 32]> {
        self.new_checksum
    }
}

#[derive(Debug)]
pub enum FileComparison<'rhs> {
    Same,
    Modified(ModifiedFile<'rhs>),
}

impl<'rhs> FileComparison<'rhs> {
    pub fn compare(lhs: &FileState, rhs: &'rhs FileState) -> Self {
        let modified_mode = (lhs.mode() != rhs.mode()).then(|| (lhs.mode(), rhs.mode()));
        let new_checksum = (lhs.checksum() != rhs.checksum()).then(|| rhs.checksum());

        if modified_mode.is_none() && new_checksum.is_none() {
            FileComparison::Same
        } else {
            FileComparison::Modified(ModifiedFile {
                path: rhs.path(),
                modified_mode,
                new_checksum,
            })
        }
    }

    pub fn reduce(&mut self) {
        if let Self::Modified(ModifiedFile {
            modified_mode: None,
            new_checksum: None,
            ..
        }) = self
        {
            *self = Self::Same;
        }
    }

    pub fn has_modified_checksum(&self) -> bool {
        matches!(
            self,
            Self::Modified(ModifiedFile {
                new_checksum: Some(_),
                ..
            })
        )
    }
}
