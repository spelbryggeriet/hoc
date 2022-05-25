use std::{
    cell::RefCell,
    fmt::{self, Display, Formatter},
    fs, io, mem,
    os::unix::prelude::{OpenOptionsExt, OsStrExt},
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use hoc_log::error;
use indexmap::IndexMap;
use regex::Regex;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

pub use self::file::FileRef;

mod file;

pub const GLOBAL_PREFIX: &str = "$";

#[derive(Debug, Error)]
pub enum Error {
    #[error(r#"key already exists: "{0}""#)]
    KeyAlreadyExists(PathBuf),

    #[error(r#"key does not exist: {0}""#)]
    KeyDoesNotExist(PathBuf),

    #[error(r#"unexpected leading `/` in key: {0}""#)]
    LeadingForwardSlash(PathBuf),

    #[error(r#"wildcard `*` in key: {0}""#)]
    Wildcard(PathBuf),

    #[error(r#"unexpected `.` in key: {0}""#)]
    SingleDotComponent(PathBuf),

    #[error(r#"unexpected `..` in key: {0}""#)]
    DoubleDotComponent(PathBuf),

    #[error("mismatched value types: {0} ≠ {1}")]
    MismatchedTypes(TypeDescription, TypeDescription),

    #[error("{0} out of range for `{1}`")]
    OverflowingNumber(i128, &'static str),

    #[error("io: {0}")]
    Io(#[from] io::Error),
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

pub trait ReadStore {
    fn get<Q: AsRef<Path>>(&self, key: Q) -> Result<Item, Error>;

    fn get_matches<Q: AsRef<Path>>(&self, key: Q) -> Result<Vec<Item>, Error>;

    fn get_keys(&self) -> Vec<PathBuf>;
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
}

#[derive(Debug)]
pub struct Store {
    map: Rc<RefCell<IndexMap<PathBuf, Item>>>,
    file_dir: PathBuf,
    record: Record,
}

impl Store {
    pub fn new<P: Into<PathBuf>>(file_dir: P) -> Self {
        Self {
            map: Rc::default(),
            file_dir: file_dir.into(),
            record: Record::default(),
        }
    }

    fn check_key<Q: AsRef<Path>>(&self, key: Q, allow_wildcard: bool) -> Result<Q, Error> {
        let key_ref = key.as_ref();

        if key_ref.is_absolute() {
            return Err(Error::LeadingForwardSlash(key_ref.to_path_buf()));
        }

        if !allow_wildcard && key_ref.to_string_lossy().contains("*") {
            return Err(Error::Wildcard(key_ref.to_path_buf()));
        }

        for comp in key_ref.components() {
            match comp {
                Component::CurDir => return Err(Error::SingleDotComponent(key_ref.to_path_buf())),
                Component::ParentDir => {
                    return Err(Error::DoubleDotComponent(key_ref.to_path_buf()))
                }
                _ => (),
            }
        }

        Ok(key)
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

    pub fn record_inserts(&self) -> Record {
        if !self.record.is_empty() {
            panic!("recording has not finished")
        }

        Record::clone(&self.record)
    }

    pub fn remove<Q: AsRef<Path>>(&self, key: Q) -> Result<Option<Value>, Error> {
        let key = self.check_key(key, false)?;

        let item = self
            .map
            .borrow_mut()
            .remove(key.as_ref())
            .ok_or_else(|| Error::KeyDoesNotExist(key.as_ref().into()))?;

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

impl ReadStore for Store {
    fn get<Q: AsRef<Path>>(&self, key: Q) -> Result<Item, Error> {
        let key = self.check_key(key, false)?.as_ref().to_path_buf();

        let res = self
            .map
            .borrow()
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::KeyDoesNotExist(key.clone()))?;

        Ok(res)
    }

    fn get_matches<Q: AsRef<Path>>(&self, key: Q) -> Result<Vec<Item>, Error> {
        let key = self.check_key(key, true)?;

        let regex_str: String = key
            .as_ref()
            .components()
            .map(|c| regex::escape(&c.as_os_str().to_string_lossy()).replace(r#"\*"#, "[^/]*"))
            .collect();
        let regex = Regex::new(&format!("^{regex_str}$")).unwrap();
        let matches = self
            .map
            .borrow()
            .iter()
            .filter_map(|(k, v)| regex.is_match(&k.to_string_lossy()).then(|| v.clone()))
            .collect();

        Ok(matches)
    }

    fn get_keys(&self) -> Vec<PathBuf> {
        self.map.borrow().keys().cloned().collect()
    }
}

impl WriteStore for Store {
    fn put<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<(), Error> {
        let key = self.check_key(key, false)?.as_ref().to_path_buf();

        if self.map.borrow().contains_key(&key) {
            return Err(Error::KeyAlreadyExists(key));
        }

        self.map
            .borrow_mut()
            .insert(key.clone(), Item::Value(value.into()));
        self.record.add(key, None);

        Ok(())
    }

    fn update<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<Value, Error> {
        let key = self.check_key(key, false)?.as_ref().to_path_buf();

        let value = value.into();
        let value_desc = value.type_description();

        match self.map.borrow_mut().get_mut(&key) {
            Some(Item::Value(previous)) => {
                let previous_desc = previous.type_description();

                if previous_desc == value_desc {
                    let res = mem::replace(previous, value);
                    self.record.add(key, None);
                    Ok(res)
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
        let key = self.check_key(key, false)?.as_ref().to_path_buf();

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
            .mode(0o600)
            .open(&path)?;
        let file_ref = FileRef::new(path)?;
        self.map
            .borrow_mut()
            .insert(key.clone(), Item::File(file_ref.clone()));

        self.record.add(key, Some(file_ref.path.to_path_buf()));

        Ok(file_ref)
    }
}

impl Serialize for Store {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(2))?;
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
                    record: Record::default(),
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

impl TryFrom<Item> for FileRef {
    type Error = Error;

    fn try_from(item: Item) -> Result<Self, Self::Error> {
        match item {
            Item::File(file_ref) => Ok(file_ref),
            item => Err(Error::MismatchedTypes(
                item.type_description(),
                TypeDescription::File,
            )),
        }
    }
}

impl TryFrom<Item> for Value {
    type Error = Error;

    fn try_from(item: Item) -> Result<Self, Self::Error> {
        match item {
            Item::Value(value) => Ok(value),
            item => Err(Error::MismatchedTypes(
                item.type_description(),
                TypeDescription::Value,
            )),
        }
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

#[derive(Debug, Default, Clone)]
pub struct Record {
    keys: Rc<RefCell<Vec<PathBuf>>>,
    file_paths: Rc<RefCell<Vec<PathBuf>>>,
}

impl Record {
    fn is_empty(&self) -> bool {
        self.keys.borrow().is_empty() && self.file_paths.borrow().is_empty()
    }

    fn add(&self, key: PathBuf, file_path: Option<PathBuf>) {
        if !key.starts_with(GLOBAL_PREFIX) {
            self.keys.borrow_mut().push(key);
        }
        if let Some(file_path) = file_path {
            self.file_paths.borrow_mut().push(file_path)
        }
    }

    pub(crate) fn finish(self) -> Vec<PathBuf> {
        self.file_paths.borrow_mut().clear();
        mem::take(&mut self.keys.borrow_mut())
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        for file_path in &*self.file_paths.borrow() {
            let _ = fs::remove_file(file_path);
        }
    }
}

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

impl From<u32> for Value {
    fn from(i: u32) -> Self {
        Self::UnsignedInteger(i as u64)
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
    Value,
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
            Self::Value => write!(f, "value"),
        }
    }
}
