use std::{
    borrow::Cow,
    fs::{self, File},
    path::{Path, PathBuf},
};

use crate::{
    context::{key::Key, Context, Error},
    ledger::Ledger,
    prelude::*,
    temp,
};

pub mod cache;
pub mod files;

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
    pub fn get(self) -> (File, PathBuf) {
        Context::get_or_init().files().get_file(self.key)?
    }

    #[throws(anyhow::Error)]
    pub fn create(self) -> (File, PathBuf) {
        let context = Context::get_or_init();
        let mut previous_path = None;
        let (had_previous_file, (file, path)) =
            context.files_mut().create_file(&self.key, |path| {
                let (_, temp_path) = temp::create_file()?;
                fs::rename(path, &temp_path)?;
                previous_path.replace(temp_path);
                Ok(())
            })?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init().add(files::ledger::Create::new(
                self.key.into_owned(),
                path.clone(),
                previous_path,
            ));
        }

        (file, path)
    }

    pub fn cached<'a, F>(self, file_cacher: F) -> FileBuilder<Cached<F>>
    where
        F: Fn(&'a mut File, &'a Path, bool) -> Result<(), Error>,
    {
        FileBuilder {
            key: self.key,
            state: Cached { file_cacher },
        }
    }
}

impl<F> FileBuilder<Cached<F>>
where
    F: Fn(&mut File, &Path, bool) -> Result<(), Error>,
{
    #[throws(anyhow::Error)]
    pub fn get_or_create(self) -> (File, PathBuf) {
        let context = Context::get_or_init();
        let mut previous_path = None;
        let (had_previous_file, (file, path)) =
            context
                .cache_mut()
                .get_or_create_file(&self.key, &self.state.file_cacher, |path| {
                    let (_, temp_path) = temp::create_file()?;
                    fs::rename(path, &temp_path)?;
                    previous_path.replace(temp_path);
                    Ok(())
                })?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init().add(cache::ledger::Create::new(
                self.key.into_owned(),
                path.clone(),
                previous_path,
            ));
        }

        (file, path)
    }

    #[throws(anyhow::Error)]
    pub fn _create_or_overwrite(self) -> (File, PathBuf) {
        let context = Context::get_or_init();
        let mut previous_path = None;
        let (had_previous_file, (file, path)) = context.cache_mut()._create_or_overwrite_file(
            self.key.as_ref(),
            self.state.file_cacher,
            |path| {
                let (_, temp_path) = temp::create_file()?;
                fs::rename(path, &temp_path)?;
                previous_path.replace(temp_path);
                Ok(())
            },
        )?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init().add(cache::ledger::Create::new(
                self.key.into_owned(),
                path.clone(),
                previous_path,
            ));
        }

        (file, path)
    }
}
