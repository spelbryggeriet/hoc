use std::{
    fs::{self, File},
    io,
    path::PathBuf,
};

use crate::{
    context::{fs::ContextFile, Error},
    prelude::*,
    util,
};

const RAND_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVXYZ\
                          abcdefghijklmnopqrstuvxyz\
                          0123456789";

pub struct Temp {
    temp_dir: PathBuf,
}

impl Temp {
    pub(in crate::context) fn new() -> Self {
        Self {
            temp_dir: crate::local_temp_dir(),
        }
    }

    #[throws(Error)]
    pub fn create_file(&self) -> ContextFile {
        let mut file_options = File::options();
        file_options.write(true).truncate(true).read(true);

        let mut path = self.temp_dir.clone();

        let mut random_key;
        let mut attempt = 1;
        let file = loop {
            random_key = util::random_string(RAND_CHARS, 10);
            path.push(&random_key);
            if attempt == 1 {
                debug!("Creating temporary file: {path:?}");
            } else {
                warn!("Creating temporary file: {path:?} (attempt {attempt})");
            }

            match file_options
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => break file,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => path.pop(),
                Err(err) => throw!(err),
            };

            attempt += 1;
        };

        ContextFile::new(file, path, crate::container_temp_dir().join(random_key))
    }

    #[throws(Error)]
    pub fn cleanup(&self) {
        let read_dir = match fs::read_dir(&self.temp_dir) {
            Ok(read_dir) => read_dir,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return,
            Err(err) => throw!(err),
        };

        for entry in read_dir {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
    }
}

impl Default for Temp {
    fn default() -> Self {
        Self {
            temp_dir: crate::local_temp_dir(),
        }
    }
}
