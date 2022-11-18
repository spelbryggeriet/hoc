use std::{
    fs::{self as blocking_fs, File as BlockingFile},
    io,
    path::PathBuf,
};

use tokio::{
    fs::{self, File},
    runtime::Handle,
    task,
};

use crate::{context::Error, prelude::*, util};

pub struct Temp {
    pub(in crate::context) files_dir: PathBuf,
}

impl Temp {
    const RAND_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVXYZ\
                              abcdefghijklmnopqrstuvxyz\
                              0123456789";

    pub(in crate::context) fn new<P>(files_dir: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            files_dir: files_dir.into(),
        }
    }

    pub(in crate::context) fn empty() -> Self {
        Self {
            files_dir: PathBuf::new(),
        }
    }

    #[throws(Error)]
    pub fn create_file(&mut self) -> (File, PathBuf) {
        debug!("Create temporary file");

        let mut file_options = BlockingFile::options();
        file_options.write(true).truncate(true).read(true);

        blocking_fs::create_dir_all(&self.files_dir)?;

        let mut path = self.files_dir.clone();
        let file = loop {
            path.push(util::random_string(Self::RAND_CHARS, 10));
            match file_options
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => break file,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    path.pop();
                }
                Err(err) => throw!(err),
            };
        };

        (File::from_std(file), path)
    }

    #[throws(Error)]
    pub async fn clean(&self) {
        let mut read_dir = match fs::read_dir(&self.files_dir).await {
            Ok(read_dir) => read_dir,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return,
            Err(err) => throw!(err),
        };

        while let Some(entry) = read_dir.next_entry().await? {
            if entry.file_type().await?.is_file() {
                fs::remove_file(entry.path()).await?;
            }
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = task::block_in_place(|| Handle::current().block_on(self.clean()));
    }
}
