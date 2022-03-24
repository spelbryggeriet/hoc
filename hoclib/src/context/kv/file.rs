use std::{
    fmt::{self, Display, Formatter},
    fs,
    io::{self, BufReader, Read},
    os::unix::prelude::MetadataExt,
    path::{Path, PathBuf},
};

use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRef {
    #[serde(skip)]
    pub(super) path: PathBuf,

    #[serde(rename = "file")]
    pub(super) hash_name: String,
    mode: Mode,
    checksum: Checksum,
}

impl FileRef {
    const BLOCK_SIZE_1_MB: usize = 1_048_576;
    const BLOCK_SIZE_8_KB: usize = 8 * 1024;

    pub(super) fn new(path: PathBuf) -> Result<Self, io::Error> {
        let hash_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        let file_len = metadata.len();
        let mode = Mode::from(metadata);
        let checksum = Checksum::from(Self::calculate_checksum(file, file_len)?);

        Ok(Self {
            path,
            hash_name,
            mode,
            checksum,
        })
    }

    pub(super) fn validate(&self) -> Result<Vec<Change>, io::Error> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(vec![Change::Missing(self.path.clone())])
            }
            Err(err) => return Err(err),
        };

        let metadata = file.metadata()?;
        let file_len = metadata.len();

        let mut changes = Vec::new();

        let new_mode = Mode::from(metadata);
        if self.mode != new_mode {
            changes.push(Change::Mode(self.mode, new_mode));
        }

        let new_checksum = Checksum::from(Self::calculate_checksum(file, file_len)?);
        if self.checksum != new_checksum {
            changes.push(Change::Checksum(self.checksum, new_checksum));
        }

        Ok(changes)
    }

    pub(super) fn refresh(&mut self) -> Result<(), io::Error> {
        let file = fs::File::open(&self.path)?;
        let metadata = file.metadata()?;
        let file_len = metadata.len();
        self.mode = Mode::from(metadata);
        self.checksum = Checksum::from(Self::calculate_checksum(file, file_len)?);

        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn calculate_checksum(file: fs::File, len: u64) -> Result<[u8; 32], io::Error> {
        let block_size_bytes = if len >= 8 * Self::BLOCK_SIZE_1_MB as u64 {
            Self::BLOCK_SIZE_1_MB
        } else {
            Self::BLOCK_SIZE_8_KB
        };

        let mut reader = BufReader::with_capacity(block_size_bytes, file);
        let mut buf = vec![0; block_size_bytes];
        let mut hasher = blake3::Hasher::new();

        let mut bytes_read = 0;
        while (bytes_read as u64) < len {
            let n = reader.read(&mut buf)?;
            bytes_read += n;
            hasher.update(&buf[..n]);
        }

        Ok(hasher.finalize().into())
    }
}

pub enum Change {
    Missing(PathBuf),
    Mode(Mode, Mode),
    Checksum(Checksum, Checksum),
}

impl Display for Change {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(f, "file missing: {}", path.to_string_lossy()),
            Self::Mode(old, new) => write!(f, "mode changed: {} => {}", old, new),
            Self::Checksum(old, new) => write!(f, "checksum changed: {} => {}", old, new),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode(pub u32);

impl Display for Mode {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{:03o}", self.0)
    }
}

impl Serialize for Mode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ModeVisitor;

        impl<'de> Visitor<'de> for ModeVisitor {
            type Value = Mode;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a 3 digit octal number")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v.is_ascii() && v.len() != 3 {
                    return Err(serde::de::Error::custom("expected 3 octal digits"));
                }

                u32::from_str_radix(v, 8)
                    .map(Mode)
                    .map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(ModeVisitor)
    }
}

impl Mode {
    const PERMISSION_BITS: u32 = 0o777;
}

impl From<fs::Metadata> for Mode {
    fn from(metadata: fs::Metadata) -> Self {
        Self(metadata.mode() & Self::PERMISSION_BITS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checksum(pub [u8; 32]);

impl Display for Checksum {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }
}

impl Serialize for Checksum {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Checksum {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ChecksumVisitor;

        impl<'de> Visitor<'de> for ChecksumVisitor {
            type Value = Checksum;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a 64 digit hexadecimal number")
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

                return Ok(Checksum(output));
            }
        }

        deserializer.deserialize_str(ChecksumVisitor)
    }
}

impl From<[u8; 32]> for Checksum {
    fn from(array: [u8; 32]) -> Self {
        Self(array)
    }
}
