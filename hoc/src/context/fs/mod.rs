use std::{
    borrow::Cow,
    future::{Future, IntoFuture},
    path::{Path, PathBuf},
    pin::Pin,
};

use tokio::fs::File;

use crate::{
    context::{self, key::Key},
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
    pub async fn get(self) -> (File, PathBuf) {
        context::get_context().files().await.get_file(self.key)?
    }

    #[throws(anyhow::Error)]
    pub async fn create(self) -> (File, PathBuf) {
        let context = context::get_context();
        let mut previous_path = None;
        let (had_previous_file, (file, path)) = context
            .files_mut()
            .await
            .create_file(self.key.as_ref(), |path| async {
                let (_, temp_path) = context.temp_mut().await.create_file()?;
                tokio::fs::rename(path, &temp_path).await?;
                previous_path.replace(temp_path);
                Ok(())
            })
            .await?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init()
                .lock()
                .await
                .add(files::ledger::Create::new(
                    self.key.into_owned(),
                    path.clone(),
                    previous_path,
                ));
        }

        (file, path)
    }

    pub fn cached<F>(self, file_cacher: F) -> FileBuilder<Cached<F>>
    where
        F: for<'a> CachedFileFn<'a>,
    {
        FileBuilder {
            key: self.key,
            state: Cached { file_cacher },
        }
    }
}

impl<F> FileBuilder<Cached<F>>
where
    F: for<'a> CachedFileFn<'a>,
{
    #[throws(anyhow::Error)]
    pub async fn get_or_create(self) -> (File, PathBuf) {
        let context = context::get_context();
        let mut previous_path = None;
        let (had_previous_file, (file, path)) = context
            .cache_mut()
            .await
            .get_or_create_file(self.key.as_ref(), self.state.file_cacher, |path| async {
                let (_, temp_path) = context.temp_mut().await.create_file()?;
                tokio::fs::rename(path, &temp_path).await?;
                previous_path.replace(temp_path);
                Ok(())
            })
            .await?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init()
                .lock()
                .await
                .add(cache::ledger::Create::new(
                    self.key.into_owned(),
                    path.clone(),
                    previous_path,
                ));
        }

        (file, path)
    }

    #[throws(anyhow::Error)]
    pub async fn _create_or_overwrite(self) -> (File, PathBuf) {
        let context = context::get_context();
        let mut previous_path = None;
        let (had_previous_file, (file, path)) = context
            .cache_mut()
            .await
            ._create_or_overwrite_file(self.key.as_ref(), self.state.file_cacher, |path| async {
                let (_, temp_path) = context.temp_mut().await.create_file()?;
                tokio::fs::rename(path, &temp_path).await?;
                previous_path.replace(temp_path);
                Ok(())
            })
            .await?;

        if !had_previous_file || previous_path.is_some() {
            Ledger::get_or_init()
                .lock()
                .await
                .add(cache::ledger::Create::new(
                    self.key.into_owned(),
                    path.clone(),
                    previous_path,
                ));
        }

        (file, path)
    }
}

type FileBuilderFuture = Pin<Box<dyn Future<Output = anyhow::Result<(File, PathBuf)>>>>;

impl IntoFuture for FileBuilder<Persisted> {
    type IntoFuture = FileBuilderFuture;
    type Output = <FileBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get())
    }
}

impl<F> IntoFuture for FileBuilder<Cached<F>>
where
    F: for<'a> CachedFileFn<'a> + 'static,
{
    type IntoFuture = FileBuilderFuture;
    type Output = <FileBuilderFuture as Future>::Output;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.get_or_create())
    }
}

pub trait CachedFileFn<'a>: Fn(&'a mut File, &'a Path, bool) -> Self::Fut {
    type Fut: Future<Output = Result<(), Self::Error>>;
    type Error: Into<anyhow::Error> + 'static;
}

impl<'a, F, Fut, E> CachedFileFn<'a> for F
where
    F: Fn(&'a mut File, &'a Path, bool) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Into<anyhow::Error> + 'static,
{
    type Fut = Fut;
    type Error = E;
}
