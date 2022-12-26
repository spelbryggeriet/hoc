use std::{
    borrow::Cow,
    fs::{self, File},
    io::{self, IoSlice, IoSliceMut, Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use crate::{
    context::{key::Key, Context, Error},
    ledger::Ledger,
    prelude::*,
};

pub mod cache;
pub mod files;
pub mod temp;

pub struct FileBuilder<S> {
    key: Cow<'static, Key>,
    state: S,
}

pub struct Persisted(());
pub struct Cached<F> {
    file_cacher: F,
}

impl FileBuilder<Persisted> {
    pub fn new(key: Cow<'static, Key>) -> Self {
        Self {
            key,
            state: Persisted(()),
        }
    }

    #[throws(anyhow::Error)]
    pub fn get(self) -> ContextFile {
        Context::get_or_init().files().get_file(self.key)?
    }

    #[throws(anyhow::Error)]
    pub fn create(self) -> ContextFile {
        let context = Context::get_or_init();
        let mut previous_path = None;
        let (had_previous_file, file) = context.files_mut().create_file(&self.key, |path| {
            let temp_file = temp_file!()?;
            fs::rename(path, &temp_file.local_path)?;
            previous_path.replace(temp_file.local_path);
            Ok(())
        })?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init().add(files::ledger::Create::new(
                self.key.clone().into_owned(),
                file.local_path.clone(),
                previous_path,
            ));
        }

        file
    }

    pub fn cached<F>(self, file_cacher: F) -> FileBuilder<Cached<F>>
    where
        F: Fn(&mut ContextFile, bool) -> Result<(), Error>,
    {
        FileBuilder {
            key: self.key,
            state: Cached { file_cacher },
        }
    }
}

impl<F> FileBuilder<Cached<F>>
where
    F: Fn(&mut ContextFile, bool) -> Result<(), Error>,
{
    #[throws(anyhow::Error)]
    pub fn get_or_create(self) -> ContextFile {
        let context = Context::get_or_init();
        let mut previous_path = None;
        let (had_previous_file, file) =
            context
                .cache_mut()
                .get_or_create_file(&self.key, &self.state.file_cacher, |path| {
                    let temp_file = temp_file!()?;
                    fs::rename(path, &temp_file.local_path)?;
                    previous_path.replace(temp_file.local_path);
                    Ok(())
                })?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init().add(cache::ledger::Create::new(
                self.key.clone().into_owned(),
                file.local_path.clone(),
                previous_path,
            ));
        }

        file
    }

    #[allow(unused)]
    #[throws(anyhow::Error)]
    pub fn create_or_overwrite(self) -> ContextFile {
        let context = Context::get_or_init();
        let mut previous_path = None;
        let (had_previous_file, file) = context.cache_mut().create_or_overwrite_file(
            self.key.as_ref(),
            self.state.file_cacher,
            |path| {
                let temp_file = temp_file!()?;
                fs::rename(path, &temp_file.local_path)?;
                previous_path.replace(temp_file.local_path);
                Ok(())
            },
        )?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init().add(cache::ledger::Create::new(
                self.key.clone().into_owned(),
                file.local_path.clone(),
                previous_path,
            ));
        }

        file
    }
}

pub struct ContextFile {
    pub file: File,
    pub local_path: PathBuf,
    pub container_path: PathBuf,
}

impl ContextFile {
    pub fn new<L, C>(file: File, local_path: L, container_path: C) -> Self
    where
        L: Into<PathBuf>,
        C: Into<PathBuf>,
    {
        Self {
            file,
            local_path: local_path.into(),
            container_path: container_path.into(),
        }
    }

    #[throws(io::Error)]
    pub fn set_len(&self, size: u64) {
        self.file.set_len(size)?
    }
}

impl Read for ContextFile {
    #[throws(io::Error)]
    fn read(&mut self, buf: &mut [u8]) -> usize {
        self.file.read(buf)?
    }

    #[throws(io::Error)]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut]) -> usize {
        self.file.read_vectored(bufs)?
    }

    #[throws(io::Error)]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> usize {
        self.file.read_to_end(buf)?
    }

    #[throws(io::Error)]
    fn read_to_string(&mut self, buf: &mut String) -> usize {
        self.file.read_to_string(buf)?
    }
}

impl Write for ContextFile {
    #[throws(io::Error)]
    fn write(&mut self, buf: &[u8]) -> usize {
        self.file.write(buf)?
    }

    #[throws(io::Error)]
    fn write_vectored(&mut self, bufs: &[IoSlice]) -> usize {
        self.file.write_vectored(bufs)?
    }

    #[throws(io::Error)]
    fn flush(&mut self) {
        self.file.flush()?
    }
}

impl Seek for ContextFile {
    #[throws(io::Error)]
    fn seek(&mut self, pos: SeekFrom) -> u64 {
        self.file.seek(pos)?
    }
}
