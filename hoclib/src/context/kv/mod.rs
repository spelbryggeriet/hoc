use std::{
    cell::RefCell,
    fmt::{self, Display, Formatter},
    fs, io, mem,
    os::unix::prelude::OsStrExt,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use indexmap::IndexMap;
use log::error;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use self::file::FileRef;

mod file;

#[derive(Debug, Error)]
pub enum Error {
    #[error(r#"key already exists: "{0}""#)]
    KeyAlreadyExists(PathBuf),

    #[error(r#"key does not exist: {0}""#)]
    KeyDoesNotExist(PathBuf),

    #[error(r#"key prefix blocked: {0}""#)]
    BlockedKey(PathBuf),

    #[error(r#"unexpected leading `/` in key: {0}""#)]
    LeadingForwardSlash(PathBuf),

    #[error(r#"unexpected `.` in key: {0}""#)]
    SingleDotComponent(PathBuf),

    #[error(r#"unexpected `..` in key: {0}""#)]
    DoubleDotComponent(PathBuf),

    #[error("merging unrelated splits")]
    UnrelatedSplits,

    #[error("mismatched value types: {0} ≠ {1}")]
    MismatchedTypes(TypeDescription, TypeDescription),

    #[error("{0} out of range for `{1}`")]
    OverflowingNumber(i128, &'static str),

    #[error("io: {0}")]
    Io(#[from] io::Error),
}

impl From<Error> for log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

pub trait ReadStore {
    #[must_use]
    fn get<Q: AsRef<Path>>(&self, key: Q) -> Result<Item, Error>;
}

pub trait WriteStore: ReadStore {
    fn put<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<(), Error>;

    fn put_array<K, V, I>(&self, key_prefix: K, array: I) -> Result<(), Error>
    where
        K: Into<PathBuf>,
        V: Into<Value>,
        I: IntoIterator<Item = V>,
    {
        let key_prefix = key_prefix.into();
        for (index, value) in array.into_iter().enumerate() {
            let index_key = key_prefix.join(index.to_string());
            self.put(index_key, value)?;
        }
        Ok(())
    }

    fn put_map<K, V, Q, I>(&self, key_prefix: K, map: I) -> Result<(), Error>
    where
        K: Into<PathBuf>,
        V: Into<Value>,
        Q: AsRef<Path>,
        I: IntoIterator<Item = (Q, V)>,
    {
        let key_prefix = key_prefix.into();
        for (key, value) in map.into_iter() {
            let map_key = key_prefix.join(key);
            self.put(map_key, value.into())?;
        }
        Ok(())
    }

    fn update<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<Value, Error>;

    fn create_file<Q: AsRef<Path>>(&self, key: Q) -> Result<FileRef, Error>;

    fn remove<Q: AsRef<Path>>(&self, key: Q) -> Result<Option<Value>, Error>;
}

#[derive(Debug)]
pub struct Store {
    map: Rc<RefCell<IndexMap<PathBuf, Item>>>,
    file_dir: PathBuf,
    key_prefix: PathBuf,
    blocked_key_prefixes: Vec<PathBuf>,
}

impl Store {
    pub fn new<P: Into<PathBuf>>(file_dir: P) -> Self {
        Self {
            map: Rc::new(RefCell::new(IndexMap::default())),
            file_dir: file_dir.into(),
            key_prefix: PathBuf::new(),
            blocked_key_prefixes: Vec::new(),
        }
    }

    fn validate_key<Q: AsRef<Path>>(&self, key: Q) -> Result<(), Error> {
        let key = key.as_ref();

        if key.is_absolute() {
            return Err(Error::LeadingForwardSlash(key.to_path_buf()));
        }

        for comp in key.components() {
            match comp {
                Component::CurDir => return Err(Error::SingleDotComponent(key.to_path_buf())),
                Component::ParentDir => return Err(Error::DoubleDotComponent(key.to_path_buf())),
                _ => (),
            }
        }

        if let Some(key_prefix) = self
            .blocked_key_prefixes
            .iter()
            .find(|key_prefix| key.starts_with(key_prefix))
        {
            return Err(Error::BlockedKey(key_prefix.clone()));
        }

        Ok(())
    }

    pub fn validate(&self) -> Result<Vec<(PathBuf, file::Change)>, Error> {
        let mut changes = Vec::new();
        for (key, value) in self.map.borrow().iter() {
            match value {
                Item::File(file_ref) => {
                    changes.extend(file_ref.validate()?.into_iter().map(|c| (key.clone(), c)))
                }
                _ => (),
            }
        }

        changes.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
        Ok(changes)
    }

    pub fn register_file_changes(&self) -> Result<(), Error> {
        for value in self.map.borrow_mut().values_mut() {
            match value {
                Item::File(file_ref) => file_ref.refresh()?,
                _ => (),
            }
        }

        Ok(())
    }

    pub fn split<K: Into<PathBuf>>(&mut self, key_prefix: K) -> Result<Self, Error> {
        let key_prefix = key_prefix.into();
        self.validate_key(&key_prefix)?;

        let blocked_key_prefixes;
        (blocked_key_prefixes, self.blocked_key_prefixes) = self
            .blocked_key_prefixes
            .drain(..)
            .partition(|kp| kp.starts_with(&key_prefix));
        self.blocked_key_prefixes.push(key_prefix.clone());

        Ok(Store {
            map: Rc::clone(&self.map),
            file_dir: self.file_dir.clone(),
            key_prefix,
            blocked_key_prefixes,
        })
    }

    pub fn merge(&mut self, other: Self) -> Result<(), Error> {
        if let Some(index) = self
            .blocked_key_prefixes
            .iter()
            .position(|key_prefix| *key_prefix == other.key_prefix)
        {
            self.blocked_key_prefixes.remove(index);
            self.blocked_key_prefixes.extend(other.blocked_key_prefixes);
            Ok(())
        } else {
            Err(Error::UnrelatedSplits)
        }
    }
}

impl ReadStore for Store {
    fn get<Q: AsRef<Path>>(&self, key: Q) -> Result<Item, Error> {
        self.validate_key(key.as_ref())?;
        let key = self.key_prefix.join(key);

        self.map
            .borrow()
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::KeyDoesNotExist(key))
    }
}

impl WriteStore for Store {
    fn put<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<(), Error> {
        self.validate_key(key.as_ref())?;
        let key = self.key_prefix.join(key);

        if self.map.borrow().contains_key(&key) {
            return Err(Error::KeyAlreadyExists(key));
        }

        self.map.borrow_mut().insert(key, Item::Value(value.into()));

        Ok(())
    }

    fn update<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<Value, Error> {
        self.validate_key(key.as_ref())?;
        let key = self.key_prefix.join(key);

        let value = value.into();
        let value_desc = value.type_description();

        match self.map.borrow_mut().get_mut(&key) {
            Some(Item::Value(previous)) => {
                let previous_desc = previous.type_description();

                if previous_desc == value_desc {
                    Ok(mem::replace(previous, value))
                } else {
                    Err(Error::MismatchedTypes(previous_desc, value_desc))
                }
            }
            Some(Item::File { .. }) => {
                Err(Error::MismatchedTypes(TypeDescription::File, value_desc))
            }
            None => Err(Error::KeyDoesNotExist(key)),
        }
    }

    fn create_file<Q: AsRef<Path>>(&self, key: Q) -> Result<FileRef, Error> {
        self.validate_key(key.as_ref())?;
        let key = self.key_prefix.join(key);

        if self.map.borrow().contains_key(&key) {
            return Err(Error::KeyAlreadyExists(key));
        }

        let mut hasher = blake3::Hasher::new();
        hasher.update(key.as_os_str().as_bytes());
        let hash: [u8; 32] = hasher.finalize().into();

        let hash_name: String = hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = self.file_dir.join(hash_name);

        fs::File::options()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let file_ref = FileRef::new(path)?;
        self.map
            .borrow_mut()
            .insert(key, Item::File(file_ref.clone()));

        Ok(file_ref)
    }

    fn remove<Q: AsRef<Path>>(&self, key: Q) -> Result<Option<Value>, Error> {
        self.validate_key(key.as_ref())?;
        let key = self.key_prefix.join(key);

        let item = self
            .map
            .borrow_mut()
            .remove(&key)
            .ok_or_else(|| Error::KeyDoesNotExist(key))?;

        match item {
            Item::Value(value) => Ok(Some(value)),
            Item::File(file_ref) => match fs::remove_file(file_ref.path()) {
                Ok(()) => Ok(None),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err.into()),
            },
        }
    }
}

impl Serialize for Store {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("map", &*self.map)?;
        map.serialize_entry("file_dir", &self.file_dir)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Store {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[allow(non_camel_case_types)]
        enum Field {
            Map,
            FileDir,
        }

        struct FieldVisitor;
        impl<'de> Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, f: &mut Formatter) -> fmt::Result {
                write!(f, "a field identifier")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "map" => Ok(Field::Map),
                    "file_dir" => Ok(Field::FileDir),
                    key => return Err(serde::de::Error::custom(format!("unexpected key: {key}"))),
                }
            }
        }

        impl<'de> Deserialize<'de> for Field {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_identifier(FieldVisitor)
            }
        }

        struct StoreVisitor;

        impl<'de> Visitor<'de> for StoreVisitor {
            type Value = Store;

            fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
                formatter.write_str("a map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut store_map = None;
                let mut file_dir = None;
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::Map => store_map = Some(map.next_value()?),
                        Field::FileDir => file_dir = Some(map.next_value()?),
                    }
                }

                let mut store_map: IndexMap<_, _> = if let Some(store_map) = store_map {
                    store_map
                } else {
                    return Err(serde::de::Error::custom("missing key: map"));
                };

                let file_dir: PathBuf = if let Some(file_dir) = file_dir {
                    file_dir
                } else {
                    return Err(serde::de::Error::custom("missing key: file_dir"));
                };

                store_map.values_mut().for_each(|item| {
                    if let Item::File(file_ref) = item {
                        file_ref.path = file_dir.join(&file_ref.hash_name);
                    }
                });

                Ok(Store {
                    map: Rc::new(RefCell::new(store_map)),
                    file_dir,
                    key_prefix: PathBuf::new(),
                    blocked_key_prefixes: Vec::new(),
                })
            }
        }

        deserializer.deserialize_map(StoreVisitor)
    }
}

impl TryFrom<FileRef> for PathBuf {
    type Error = Error;

    fn try_from(file_ref: FileRef) -> Result<Self, Self::Error> {
        Ok(file_ref.path().to_path_buf())
    }
}

macro_rules! impl_try_from_integer {
    ($variant:ident for $impl_type:ty) => {
        impl TryFrom<Value> for $impl_type {
            type Error = Error;

            fn try_from(value: Value) -> Result<Self, Self::Error> {
                match value {
                    Value::$variant(n) => <$impl_type>::try_from(n)
                        .map_err(|_| Error::OverflowingNumber(n as i128, stringify!($impl_type))),
                    value => Err(Error::MismatchedTypes(
                        value.type_description(),
                        TypeDescription::$variant,
                    )),
                }
            }
        }
    };
}

macro_rules! impl_try_from_non_integer {
    ($variant:ident for $impl_type:ty) => {
        impl TryFrom<Value> for $impl_type {
            type Error = Error;

            fn try_from(value: Value) -> Result<Self, Self::Error> {
                match value {
                    Value::$variant(v) => Ok(v as $impl_type),
                    value => Err(Error::MismatchedTypes(
                        value.type_description(),
                        TypeDescription::$variant,
                    )),
                }
            }
        }
    };
}

macro_rules! impl_try_from_item {
    ($variant:ident::$inner_variant:ident for $impl_type:ty) => {
        impl TryFrom<Item> for $impl_type {
            type Error = Error;

            fn try_from(item: Item) -> Result<Self, Self::Error> {
                match item {
                    Item::$variant(v) => v.try_into(),
                    item => Err(Error::MismatchedTypes(
                        item.type_description(),
                        TypeDescription::$inner_variant,
                    )),
                }
            }
        }
    };
}

impl_try_from_non_integer!(Bool for bool);
impl_try_from_integer!(UnsignedInteger for u8);
impl_try_from_integer!(UnsignedInteger for u16);
impl_try_from_integer!(UnsignedInteger for u32);
impl_try_from_integer!(UnsignedInteger for u64);
impl_try_from_integer!(SignedInteger for i8);
impl_try_from_integer!(SignedInteger for i16);
impl_try_from_integer!(SignedInteger for i32);
impl_try_from_integer!(SignedInteger for i64);
impl_try_from_non_integer!(FloatingPointNumber for f32);
impl_try_from_non_integer!(FloatingPointNumber for f64);
impl_try_from_non_integer!(String for String);

impl_try_from_item!(Value::Bool for bool);
impl_try_from_item!(Value::UnsignedInteger for u8);
impl_try_from_item!(Value::UnsignedInteger for u16);
impl_try_from_item!(Value::UnsignedInteger for u32);
impl_try_from_item!(Value::UnsignedInteger for u64);
impl_try_from_item!(Value::SignedInteger for i8);
impl_try_from_item!(Value::SignedInteger for i16);
impl_try_from_item!(Value::SignedInteger for i32);
impl_try_from_item!(Value::SignedInteger for i64);
impl_try_from_item!(Value::FloatingPointNumber for f32);
impl_try_from_item!(Value::FloatingPointNumber for f64);
impl_try_from_item!(Value::String for String);
impl_try_from_item!(File::File for PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Item {
    Value(Value),
    File(FileRef),
}

impl Item {
    fn type_description(&self) -> TypeDescription {
        match self {
            Self::Value(value) => value.type_description(),
            Self::File(_) => TypeDescription::File,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    UnsignedInteger(u64),
    SignedInteger(i64),
    FloatingPointNumber(f64),
    String(String),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Self::String(s.clone())
    }
}

impl Value {
    fn type_description(&self) -> TypeDescription {
        match self {
            Self::Bool(_) => TypeDescription::Bool,
            Self::UnsignedInteger(_) => TypeDescription::UnsignedInteger,
            Self::SignedInteger(_) => TypeDescription::SignedInteger,
            Self::FloatingPointNumber(_) => TypeDescription::FloatingPointNumber,
            Self::String(_) => TypeDescription::String,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum TypeDescription {
    Bool,
    UnsignedInteger,
    SignedInteger,
    FloatingPointNumber,
    String,
    File,
}

impl Display for TypeDescription {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Bool => write!(f, "bool"),
            Self::UnsignedInteger => write!(f, "unsigned integer"),
            Self::SignedInteger => write!(f, "signed integer"),
            Self::FloatingPointNumber => write!(f, "floating point number"),
            Self::String => write!(f, "string"),
            Self::File => write!(f, "file"),
        }
    }
}
