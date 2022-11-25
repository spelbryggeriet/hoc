use std::{
    borrow::Cow,
    cell::RefCell,
    convert::Infallible,
    ffi::OsStr,
    fmt::{self, Display, Formatter},
    fs, io, iter, mem,
    os::unix::prelude::{OpenOptionsExt, OsStrExt},
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use hoc_log::error;
use indexmap::{IndexMap, IndexSet};
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

impl From<Infallible> for Error {
    fn from(x: Infallible) -> Self {
        x.into()
    }
}

pub trait ReadStore {
    fn get<Q: AsRef<Path>>(&self, key: Q) -> Result<Item, Error>;

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
            self.put(map_key, value)?;
        }
        Ok(())
    }

    fn update<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<Value, Error>;

    fn create_file<Q: AsRef<Path>>(&self, key: Q) -> Result<FileRef, Error>;
}

enum Branch<'a> {
    Array(&'a mut Vec<Item>, usize),
    Map(&'a mut IndexMap<String, Item>, &'a str),
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

    fn check_key<Q: AsRef<Path>>(&self, key: Q) -> Result<Q, Error> {
        let key_ref = key.as_ref();

        if key_ref.is_absolute() {
            return Err(Error::LeadingForwardSlash(key_ref.to_path_buf()));
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
        let key = self.check_key(key)?;

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
            _ => unreachable!(),
        }
    }

    fn traverse(
        &self,
        key: &Path,
        leaf_handler: impl FnOnce(&Path) -> Result<Option<Item>, Error> + Copy,
        branch_handler: impl FnOnce(Branch, Item, usize, usize) -> Result<(), Error> + Copy,
    ) -> Result<Option<Item>, Error> {
        // If key does not contain any wildcards, then it is a "leaf", i.e. we can fetch the value
        // directly.
        if !key.as_os_str().as_bytes().contains(&b'*') {
            return leaf_handler(key);
        }

        let mut comps = key.components();

        // A key is "nested" if it ends with a '**' component. Nested in this case means that it
        // will traverse further down to build a map or an array structure, given the expanded
        // components.
        let is_nested = matches!(key.components().last(), Some(comp) if comp.as_os_str() == "**");

        // If the key is nested, we remove the '**' wildcard component, and use the remaining
        // components as the prefix expression for the regexes below.
        if is_nested {
            comps.next_back();
        }

        // Build the prefix expression, replacing wildcards and escaping regex tokens.
        let prefix_expr = comps
            .map(|comp| {
                if comp.as_os_str() == "**" {
                    ".*".to_string()
                } else {
                    regex::escape(&comp.as_os_str().to_string_lossy()).replace(r#"\*"#, "[^/]*")
                }
            })
            .collect::<Vec<_>>()
            .join("/");

        // Different regexes are used depending on the prefix expression and nestedness of the key.
        // Non-nested keys kan be matched directly to the prefix expression (i.e. they have no
        // suffixes). Nested keys need to capture the suffix in order to do further traversing.
        let regex = if is_nested {
            if prefix_expr.is_empty() {
                Regex::new(&format!("^(?P<suffix>.*)$")).unwrap()
            } else {
                Regex::new(&format!("^(?P<prefix>{prefix_expr})/(?P<suffix>.*)$")).unwrap()
            }
        } else {
            Regex::new(&format!("^(?P<prefix>{prefix_expr})$")).unwrap()
        };

        // Get a copy of all the keys in this instance. We can't hold references to it since we
        // might mutably borrow from the map later on.
        let keys: Vec<_> = self
            .map
            .borrow()
            .keys()
            .map(|k| k.to_string_lossy().into_owned())
            .collect();

        // Build a map of prefixes and suffixes. Each prefix might have zero or more suffixes. If
        // the prefix has no suffixes, then it is a leaf. If it has one or multple suffixes, then
        // they originated from a nested key and will built as a map or an array, depending on the
        // key structure. Multiple prefixes means the final result will be returned wrapped in an
        // outer array.
        let mut key_map = IndexMap::new();
        for k in &keys {
            if let Some(captures) = regex.captures(k) {
                match (captures.name("prefix"), captures.name("suffix")) {
                    (None, None) => (),
                    (prefix, suffix) => {
                        key_map
                            .entry(Path::new(prefix.map(|m| m.as_str()).unwrap_or_default()))
                            .and_modify(|suffixes: &mut IndexSet<_>| {
                                if let Some(suffix) = suffix {
                                    suffixes
                                        .insert(Self::nested_suffix(Path::new(suffix.as_str())));
                                }
                            })
                            .or_insert_with(|| {
                                let mut suffixes = IndexSet::new();
                                if let Some(suffix) = suffix {
                                    suffixes
                                        .insert(Self::nested_suffix(Path::new(suffix.as_str())));
                                }
                                suffixes
                            });
                    }
                }
            }
        }

        // Iterate through the key map of prefixes and suffixes.
        let mut result = Vec::new();
        for (prefix, suffixes) in key_map {
            // If there are no suffixes, then it is a leaf and the prefix is its key.
            if suffixes.is_empty() {
                if let Some(item) = leaf_handler(prefix)? {
                    let capacity = result.capacity();
                    let index = result.len();
                    branch_handler(Branch::Array(&mut result, index), item, capacity, index + 1)?;
                }
                continue;
            }

            // Iterate through all the first components of the suffix to see if they all parse as
            // indices (usize).
            let indices = suffixes
                .iter()
                .map(|suffix| {
                    suffix
                        .components()
                        .next()
                        .unwrap()
                        .as_os_str()
                        .to_str()
                        .unwrap()
                        .parse::<usize>()
                })
                .collect::<Result<Vec<_>, _>>();

            // If all suffixes begin with an index and the range of those indices start at 0 and
            // are compact (that is, if the indices where sorted, they would be consecutive), then
            // an array will be built from the suffixes.
            if let Ok(indices) = indices {
                let mut validated = vec![false; indices.len()];
                for (index, suffix) in indices.iter().zip(suffixes.iter()) {
                    validated
                        .get_mut(*index)
                        .map(|v| *v = suffix.components().count() >= 1);
                }

                if validated.into_iter().all(|v| v) {
                    let mut array = Vec::new();
                    let count = indices.len();

                    // Traverse through the suffixes and delegate the handling to the caller of
                    // this function.
                    for (index, suffix) in indices.into_iter().zip(suffixes) {
                        let nested_key = prefix.join(suffix);
                        if let Some(item) =
                            self.traverse(&nested_key, leaf_handler, branch_handler)?
                        {
                            let capacity = array.capacity();
                            branch_handler(
                                Branch::Array(&mut array, index),
                                item,
                                capacity,
                                count,
                            )?;
                        }
                    }

                    // Delegate the handling of the processed array to the caller of this function.
                    let capacity = result.capacity();
                    let index = result.len();
                    branch_handler(
                        Branch::Array(&mut result, index),
                        Item::Array(array),
                        capacity,
                        index + 1,
                    )?;
                    continue;
                }
            }

            // This prefix-suffixes pair is neither a leaf nor an array, so we process the suffixes
            // as a map.
            let mut map = IndexMap::new();

            // Traverse through the suffixes and delegate the handling to the caller of this
            // function.
            let count = suffixes.len();
            for suffix in suffixes {
                let field = suffix
                    .components()
                    .next()
                    .unwrap()
                    .as_os_str()
                    .to_string_lossy();
                let nested_key = prefix.join(&suffix);
                if let Some(item) = self.traverse(&nested_key, leaf_handler, branch_handler)? {
                    let capacity = map.capacity();
                    branch_handler(Branch::Map(&mut map, &field), item, capacity, count)?;
                };
            }

            // Delegate the handling of the processed map to the caller of this function.
            let capacity = result.capacity();
            let index = result.len();
            branch_handler(
                Branch::Array(&mut result, index),
                Item::Map(map),
                capacity,
                index + 1,
            )?;
        }

        if result.len() == 1 {
            Ok(Some(result.remove(0)))
        } else if result.len() > 1 {
            Ok(Some(Item::Array(result)))
        } else {
            Ok(None)
        }
    }

    fn nested_suffix(full_suffix: &Path) -> Cow<Path> {
        if full_suffix.components().count() == 1 {
            Cow::Borrowed(full_suffix)
        } else {
            Cow::Owned(
                full_suffix
                    .components()
                    .take(1)
                    .chain(Some(Component::Normal(OsStr::new("**"))))
                    .collect::<PathBuf>(),
            )
        }
    }
}

impl ReadStore for Store {
    fn get<Q: AsRef<Path>>(&self, key: Q) -> Result<Item, Error> {
        let key = self.check_key(key)?;

        let map_borrow = self.map.borrow();
        self.traverse(
            key.as_ref(),
            |key_match| {
                map_borrow
                    .get(key_match)
                    .map(|item| Some(item.clone()))
                    .ok_or_else(|| Error::KeyDoesNotExist(key.as_ref().to_path_buf()))
            },
            |branch, item, capacity, count| match branch {
                Branch::Array(array, index) => {
                    if capacity < count {
                        array.reserve(count - capacity);
                    }

                    if index >= array.len() {
                        array.extend(
                            iter::repeat_with(|| Item::Value(Value::Bool(false)))
                                .take(index - array.len()),
                        );

                        array.push(item);
                    } else {
                        array[index] = item;
                    }

                    Ok(())
                }
                Branch::Map(map, field) => {
                    if capacity < count {
                        map.reserve(count - capacity);
                    }

                    map.insert(field.to_string(), item);

                    Ok(())
                }
            },
        )?
        .ok_or_else(|| Error::KeyDoesNotExist(key.as_ref().to_path_buf()))
    }

    fn get_keys(&self) -> Vec<PathBuf> {
        self.map.borrow().keys().cloned().collect()
    }
}

impl WriteStore for Store {
    fn put<Q: AsRef<Path>, V: Into<Value>>(&self, key: Q, value: V) -> Result<(), Error> {
        let key = self.check_key(key)?.as_ref().to_path_buf();

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
        let key = self.check_key(key)?.as_ref().to_path_buf();

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
            Some(_) => unreachable!(),
            None => Err(Error::KeyDoesNotExist(key)),
        }
    }

    fn create_file<Q: AsRef<Path>>(&self, key: Q) -> Result<FileRef, Error> {
        let key = self.check_key(key)?.as_ref().to_path_buf();

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

impl<T> TryFrom<Item> for Vec<T>
where
    T: TryFrom<Item>,
    Error: From<<T as TryFrom<Item>>::Error>,
{
    type Error = Error;

    fn try_from(item: Item) -> Result<Self, Self::Error> {
        match item {
            Item::Array(arr) => arr
                .into_iter()
                .map(T::try_from)
                .collect::<Result<_, _>>()
                .map_err(Into::into),
            item => Ok(vec![T::try_from(item)?]),
        }
    }
}

impl<T> TryFrom<Item> for IndexMap<String, T>
where
    T: TryFrom<Item>,
    <T as TryFrom<Item>>::Error: Into<Error>,
{
    type Error = Error;

    fn try_from(item: Item) -> Result<Self, Self::Error> {
        match item {
            Item::Map(map) => map
                .into_iter()
                .map(|(k, v)| Ok((k, T::try_from(v).map_err(Into::into)?)))
                .collect(),
            item => Err(Error::MismatchedTypes(
                item.type_description(),
                TypeDescription::Array(Vec::new()),
            )),
        }
    }
}

macro_rules! impl_try_from_value_integer {
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

macro_rules! impl_try_from_value_non_integer {
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

impl_try_from_value_non_integer!(Bool for bool);
impl_try_from_value_integer!(UnsignedInteger for u8);
impl_try_from_value_integer!(UnsignedInteger for u16);
impl_try_from_value_integer!(UnsignedInteger for u32);
impl_try_from_value_integer!(UnsignedInteger for u64);
impl_try_from_value_integer!(SignedInteger for i8);
impl_try_from_value_integer!(SignedInteger for i16);
impl_try_from_value_integer!(SignedInteger for i32);
impl_try_from_value_integer!(SignedInteger for i64);
impl_try_from_value_non_integer!(FloatingPointNumber for f32);
impl_try_from_value_non_integer!(FloatingPointNumber for f64);
impl_try_from_value_non_integer!(String for String);

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Item {
    Value(Value),
    File(FileRef),
    Array(Vec<Item>),
    Map(IndexMap<String, Item>),
}

impl Item {
    fn type_description(&self) -> TypeDescription {
        match self {
            Self::Value(value) => value.type_description(),
            Self::Array(arr) => {
                TypeDescription::Array(arr.iter().map(Self::type_description).collect())
            }
            Self::Map(map) => {
                TypeDescription::Map(map.iter().map(|(_, i)| i.type_description()).collect())
            }
            Self::File(_) => TypeDescription::File,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    UnsignedInteger(u64),
    SignedInteger(i64),
    FloatingPointNumber(f64),
    String(String),
}

macro_rules! impl_from_for_value_and_item {
    ($ty:ty as $variant:ident) => {
        impl From<$ty> for Value {
            fn from(v: $ty) -> Self {
                Self::$variant(v)
            }
        }

        impl From<$ty> for Item {
            fn from(v: $ty) -> Self {
                Self::Value(Value::$variant(v))
            }
        }
    };

    ($ty:ty as $variant:ident => |$val:ident| $conversion:expr) => {
        impl From<$ty> for Value {
            fn from($val: $ty) -> Self {
                Self::$variant($conversion)
            }
        }

        impl From<$ty> for Item {
            fn from($val: $ty) -> Self {
                Self::Value(Value::$variant($conversion))
            }
        }
    };
}

impl_from_for_value_and_item!(String as String);
impl_from_for_value_and_item!(&str as String => |s| s.to_string());
impl_from_for_value_and_item!(&String as String => |s| s.clone());
impl_from_for_value_and_item!(u64 as UnsignedInteger);
impl_from_for_value_and_item!(u32 as UnsignedInteger => |i| i as u64);
impl_from_for_value_and_item!(u16 as UnsignedInteger => |i| i as u64);
impl_from_for_value_and_item!(u8 as UnsignedInteger => |i| i as u64);
impl_from_for_value_and_item!(i64 as SignedInteger);
impl_from_for_value_and_item!(i32 as SignedInteger => |i| i as i64);
impl_from_for_value_and_item!(i16 as SignedInteger => |i| i as i64);
impl_from_for_value_and_item!(i8 as SignedInteger => |i| i as i64);
impl_from_for_value_and_item!(f64 as FloatingPointNumber);
impl_from_for_value_and_item!(f32 as FloatingPointNumber => |f| f as f64);
impl_from_for_value_and_item!(bool as Bool);

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
    Array(Vec<Self>),
    Map(Vec<Self>),
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
            Self::Array(col) | Self::Map(col) => {
                let col_ty = if matches!(self, Self::Array(_)) {
                    "array"
                } else {
                    "map"
                };

                if col.is_empty() {
                    write!(f, "{col_ty}")
                } else if col.len() == 1 {
                    write!(f, "{col_ty} of {}", col[0])
                } else {
                    write!(
                        f,
                        "{col_ty} of {} and {}",
                        col.iter()
                            .take(col.len() - 1)
                            .map(Self::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                        col.last().unwrap(),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    macro_rules! item_map {
        ($map:ident => @impl $key:literal map=> $value:expr, $($rest:tt)*) => {
            $map.insert($key.to_string(), Item::Map($value));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl $key:literal array=> $value:expr, $($rest:tt)*) => {
            $map.insert($key.to_string(), Item::Array($value));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl $key:literal => $value:expr, $($rest:tt)*) => {
            $map.insert($key.to_string(), Item::from($value));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl $(,)?) => {};

        ($($input:tt)*) => {{
            let mut map = IndexMap::<String, Item>::new();
            item_map!(map => @impl $($input)*,);
            map
        }};
    }

    macro_rules! item_array {
        ($array:ident => @impl map=> $value:expr, $($rest:tt)*) => {
            $array.push(Item::Map($value));
            item_array!($array => @impl $($rest)*);
        };

        ($array:ident => @impl array=> $value:expr, $($rest:tt)*) => {
            $array.push(Item::Array($value));
            item_array!($array => @impl $($rest)*);
        };

        ($array:ident => @impl $value:expr, $($rest:tt)*) => {
            $array.push(Item::from($value));
            item_array!($array => @impl $($rest)*);
        };

        ($array:ident => @impl $(,)?) => {};

        ($($input:tt)*) => {{
            let mut array = Vec::<Item>::new();
            item_array!(array => @impl $($input)*,);
            array
        }};

    }

    macro_rules! value_map {
        ($($key:literal => $value:expr),* $(,)?) => { {
            let mut map = IndexMap::<String, Value>::new();
            $(map.insert($key.to_string(), {$value}.into());)*
            map
        }};
    }

    trait ExpectValue
    where
        Self: Sized + PartialEq + Debug,
    {
        #[track_caller]
        fn expect_val(self, v: Self) {
            assert_eq!(self, v)
        }
    }

    impl<T> ExpectValue for T where T: Sized + PartialEq + Debug {}

    fn store() -> Result<Store, Error> {
        let store = Store::new(Path::new("fakedir"));
        let ttokens = ["t1", "t2", "t3", "t4"];
        let rtokens = ["r1", "r2", "r3", "r4"];
        let alpha = value_map! {
            "string" => "hello",
            "int" => 1u32,
        };
        let alpha2 = value_map! {
            "string" => "hello",
            "int" => 2u32,
        };
        let extra = value_map! {
            "bool" => false,
            "i64" => i64::MIN,
        };
        let two = ["t1", "t2", "t3", "t4", "r1", "r2", "r3", "r4"];
        store.put("unsigned", 1u32)?;
        store.put("signed", -1)?;
        store.put("float", 1.0)?;
        store.put("u64", u64::MAX)?;
        store.put("bool", false)?;
        store.put("string", "hello")?;
        store.put("nested/one", true)?;
        store.put("nested/two/adam", false)?;
        store.put("nested/two/betsy/alpha/token", ttokens[0])?;
        store.put("nested/two/betsy/beta/token", ttokens[1])?;
        store.put("nested/two/betsy/delta/token", ttokens[2])?;
        store.put("nested/two/betsy/gamma/token", ttokens[3])?;
        store.put_array("array/one", ttokens)?;
        store.put_array("array/two", rtokens.clone())?;
        store.put_map("map/one/adam/alpha", alpha)?;
        store.put_array("map/one/adam/beta", rtokens)?;
        store.put_map("map/one/betsy/alpha", alpha2)?;
        store.put_map("map/one/betsy/alpha/extra", extra)?;
        store.put_array("map/two", two)?;
        Ok(store)
    }

    fn get_joined_vec(store: &impl ReadStore, key: &str) -> Result<String, Error> {
        Ok(Vec::<String>::try_from(store.get(key)?)?.join(","))
    }

    #[test]
    fn get_single_leaf() -> Result<(), Error> {
        let s = store()?;
        u32::try_from(s.get("unsigned")?)?.expect_val(1);
        i32::try_from(s.get("signed")?)?.expect_val(-1);
        f32::try_from(s.get("float")?)?.expect_val(1.0);
        u64::try_from(s.get("u64")?)?.expect_val(u64::MAX);
        bool::try_from(s.get("bool")?)?.expect_val(false);
        String::try_from(s.get("string")?)?.expect_val("hello".to_string());
        bool::try_from(s.get("nested/one")?)?.expect_val(true);
        bool::try_from(s.get("nested/*")?)?.expect_val(true);
        bool::try_from(s.get("nested/two/adam")?)?.expect_val(false);
        Ok(())
    }

    #[test]
    fn get_multiple_leafs() -> Result<(), Error> {
        let s = store()?;
        get_joined_vec(&s, "nested/two/betsy/*/token")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "nested/two/betsy/*ta/token")?.expect_val("t2,t3".to_string());
        get_joined_vec(&s, "nested/two/*/*/token")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "nested/*/*/*/token")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "nested/*/*/*a*a/token")?.expect_val("t1,t4".to_string());
        get_joined_vec(&s, "nested/**/token")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "nested/**/**/token")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "nested/**/betsy/*l*/token")?.expect_val("t1,t3".to_string());
        Ok(())
    }

    #[test]
    fn get_single_array() -> Result<(), Error> {
        use Value::*;

        let s = store()?;
        Vec::<Value>::try_from(s.get("*")?)?.expect_val(vec![
            UnsignedInteger(1),
            SignedInteger(-1),
            FloatingPointNumber(1.0),
            UnsignedInteger(u64::MAX),
            Bool(false),
            String("hello".into()),
        ]);
        Vec::<bool>::try_from(s.get("nested/*")?)?.expect_val(vec![true]);
        Vec::<bool>::try_from(s.get("nested/*/*")?)?.expect_val(vec![false]);
        get_joined_vec(&s, "array/one/*")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "array/one/**")?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&s, "array/*/*")?.expect_val("t1,t2,t3,t4,r1,r2,r3,r4".to_string());
        Ok(())
    }

    #[test]
    fn get_multiple_arrays() -> Result<(), Error> {
        let s = store()?;
        Vec::<Vec<String>>::try_from(s.get("array/*/**")?)?.expect_val(vec![
            vec![
                "t1".to_string(),
                "t2".to_string(),
                "t3".to_string(),
                "t4".to_string(),
            ],
            vec![
                "r1".to_string(),
                "r2".to_string(),
                "r3".to_string(),
                "r4".to_string(),
            ],
        ]);
        Ok(())
    }

    #[test]
    fn get_single_map() -> Result<(), Error> {
        let s = store()?;
        let alpha = item_map! {
            "string" => "hello",
            "int" => 1u32,
        };
        let adam = item_map! {
            "alpha" map=> alpha.clone(),
            "beta" array=> item_array!["r1", "r2", "r3", "r4"],
        };
        let one = item_map! {
            "adam" map=> adam.clone(),
            "betsy" map=> item_map! {
                "alpha" map=> item_map! {
                    "string" => "hello",
                    "int" => 2u32,
                    "extra" map=> item_map! {
                        "bool" => false,
                        "i64" => i64::MIN,
                    },
                },
            },
        };
        let map = item_map! {
            "one" map=> one.clone(),
            "two" array=> item_array!["t1", "t2", "t3", "t4", "r1", "r2", "r3", "r4"],
        };
        let root = item_map! {
            "unsigned" => 1u32,
            "signed" => -1,
            "float" => 1.0,
            "u64" => u64::MAX,
            "bool" => false,
            "string" => "hello",
            "nested" map=> item_map! {
                "one" => true,
                "two" map=> item_map! {
                    "adam" => false,
                    "betsy" map=> item_map! {
                        "alpha" map=> item_map! {
                            "token" => "t1",
                        },
                        "beta" map=> item_map! {
                            "token" => "t2",
                        },
                        "delta" map=> item_map! {
                            "token" => "t3",
                        },
                        "gamma" map=> item_map! {
                            "token" => "t4",
                        },
                    },
                },
            },
            "array" map=> item_map! {
                "one" array=> item_array!["t1", "t2", "t3", "t4"],
                "two" array=> item_array!["r1", "r2", "r3", "r4"],
            },
            "map" map=> map.clone(),
        };
        IndexMap::<String, Item>::try_from(s.get("map/one/adam/alpha/**")?)?.expect_val(alpha);
        IndexMap::<String, Item>::try_from(s.get("map/one/adam/**")?)?.expect_val(adam);
        IndexMap::<String, Item>::try_from(s.get("map/one/**")?)?.expect_val(one);
        IndexMap::<String, Item>::try_from(s.get("map/**")?)?.expect_val(map);
        IndexMap::<String, Item>::try_from(s.get("**")?)?.expect_val(root);
        Ok(())
    }

    #[test]
    fn get_multiple_maps() -> Result<(), Error> {
        let s = store()?;
        Vec::<IndexMap<String, Item>>::try_from(s.get("map/**/alpha/**")?)?.expect_val(vec![
            item_map! {
                "string" => "hello",
                "int" => 1u32,
            },
            item_map! {
                "string" => "hello",
                "int" => 2u32,
                "extra" map=> item_map! {
                    "bool" => false,
                    "i64" => i64::MIN,
                },
            },
        ]);
        Ok(())
    }
}
