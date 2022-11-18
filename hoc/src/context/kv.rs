use std::{
    borrow::Cow,
    convert::Infallible,
    ffi::OsStr,
    fmt::{self, Debug, Display, Formatter},
    io, iter,
    path::{Component, Path, PathBuf},
};

use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::key::{self, Key, KeyOwned},
    log,
    prelude::*,
    prompt,
};

#[derive(Serialize, Deserialize)]
pub struct Kv {
    #[serde(flatten)]
    map: IndexMap<KeyOwned, Value>,
}

impl Kv {
    pub(super) fn new() -> Self {
        Self {
            map: IndexMap::new(),
        }
    }

    #[throws(Error)]
    pub fn get_item<'key, K>(&self, key: K) -> Item
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        // If key does not contain any wildcards, then it is a "leaf", i.e. we can fetch the value
        // directly.
        if !key.contains_wildcard() {
            debug!("Get item: {key}");

            return self
                .map
                .get(&*key)
                .map(|value| Item::Value(value.clone()))
                .ok_or_else(|| key::Error::KeyDoesNotExist(key.into_owned()))?;
        }

        let mut comps = key.components();

        // A key is "nested" if it ends with a '**' component. Nested in this case means that it
        // will traverse further down to build a map or an array structure, given the expanded
        // components.
        let is_nested = matches!(key.components().last(), Some(comp) if comp.is_nested_wildcard());

        // If the key is nested, we remove the '**' wildcard component, and use the remaining
        // components as the prefix expression for the regexes below.
        if is_nested {
            comps.next_back();
        }

        // Build the prefix expression, replacing wildcards and escaping regex tokens.
        let prefix_expr = comps
            .map(|comp| {
                if comp.is_nested_wildcard() {
                    ".*".to_string()
                } else {
                    regex::escape(&comp.to_string_lossy()).replace(r#"\*"#, "[^/]*")
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
            .keys()
            .map(|k| k.to_string_lossy().into_owned())
            .collect();

        // Build a map of prefixes and suffixes. Each prefix might have zero or more suffixes. If
        // the prefix has no suffixes, then it is a leaf. If it has one or multple suffixes, then
        // they originated from a nested key and will built as a map or an array, depending on the
        // key structure. Multiple prefixes mean the final result will be returned wrapped in an
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
                if let Some(value) = self.map.get(Key::new(prefix)?).cloned() {
                    result.push(Item::Value(value));
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
                for index in indices.iter() {
                    validated.get_mut(*index).map(|v| *v = true);
                }

                if validated.into_iter().all(|v| v) {
                    let count = indices.len();
                    let mut array = Vec::with_capacity(count);

                    // Traverse through the suffixes.
                    for (index, suffix) in indices.into_iter().zip(suffixes) {
                        let nested_key = KeyOwned::new_unchecked(prefix.join(suffix));
                        if let Ok(item) = self.get_item(nested_key) {
                            if index >= array.len() {
                                array.extend(
                                    iter::repeat_with(|| Item::Value(Value::Bool(false)))
                                        .take(index - array.len() + 1),
                                );
                            }

                            array[index] = item;
                        }
                    }

                    result.push(Item::Array(array));
                    continue;
                }
            }

            // This prefix-suffixes pair is neither a leaf nor an array, so we process the suffixes
            // as a map.
            let mut map = IndexMap::new();

            // Traverse through the suffixes and delegate the handling to the caller of this
            // function.
            for suffix in suffixes {
                let field = suffix
                    .components()
                    .next()
                    .unwrap()
                    .as_os_str()
                    .to_string_lossy();
                let nested_key = KeyOwned::new_unchecked(prefix.join(&suffix));
                if let Ok(item) = self.get_item(nested_key) {
                    map.insert(field.to_string(), item);
                };
            }

            result.push(Item::Map(map));
        }

        if result.len() == 1 {
            result.remove(0)
        } else if result.len() > 1 {
            Item::Array(result)
        } else {
            throw!(key::Error::KeyDoesNotExist(key.into_owned()))
        }
    }

    /// Puts a value in the key-value store.
    ///
    /// Returns `None` if no previous value was present, `Some(None)` if a value is already present
    /// but not replaced, or `Some(Some(value))` if a previous value has been replaced.
    #[throws(Error)]
    pub fn put_value<'key, K, V>(&mut self, key: K, value: V, force: bool) -> Option<Option<Value>>
    where
        K: Into<Cow<'key, Key>>,
        V: Into<Value>,
    {
        let key = key.into();
        let value = value.into();

        debug!("Put value: {key} => {value}");

        match self.map.get(&*key) {
            Some(existing) if *existing != value && !force => {
                error!("Key {key} is already set with a different value");

                let should_continue = select!("How do you want to resolve the key conflict?")
                    .with_option("Skip", || false)
                    .with_option("Overwrite", || true)
                    .get()?;

                if !should_continue {
                    warn!("Skipping to set value for key {key}");
                    return Some(None);
                }

                warn!("Overwriting existing item for key {key}");

                if log_enabled!(Level::Debug) {
                    trace!(
                        "Old item for key {key}: {}",
                        serde_json::to_string(&self.get_item(&*key)?)?
                    );
                }
            }
            Some(_) if !force => {
                debug!("The same value is already set, skipping");
                return Some(None);
            }
            _ => (),
        }

        self.map.insert(key.into_owned(), value).map(Some)
    }

    #[throws(Error)]
    pub fn _put_array<K, V, I>(&mut self, key_prefix: K, array: I, force: bool)
    where
        K: Into<PathBuf>,
        V: Into<Value>,
        I: IntoIterator<Item = V>,
    {
        let key_prefix = key_prefix.into();
        for (index, value) in array.into_iter().enumerate() {
            let index_key = KeyOwned::new(key_prefix.join(index.to_string()))?;
            self.put_value(index_key, value, force)?;
        }
    }

    #[throws(Error)]
    pub fn _put_map<K, V, Q, I>(&mut self, key_prefix: K, map: I, force: bool)
    where
        K: Into<PathBuf>,
        V: Into<Value>,
        Q: AsRef<Path>,
        I: IntoIterator<Item = (Q, V)>,
    {
        let key_prefix = key_prefix.into();
        for (key, value) in map.into_iter() {
            let map_key = KeyOwned::new(key_prefix.join(key))?;
            self.put_value(map_key, value, force)?;
        }
    }

    #[throws(Error)]
    pub fn drop_value<'key, K>(&mut self, key: K, force: bool) -> Option<Value>
    where
        K: Into<Cow<'key, Key>>,
    {
        let key = key.into();

        debug!("Drop value: {key}");

        match self.map.remove(&*key) {
            Some(existing) => Some(existing),
            None if !force => {
                error!("Key {key} does not exist");

                let skipping = select!("How do you want to resolve the key conflict?")
                    .with_option("Skip", || true)
                    .get()?;

                if skipping {
                    warn!("Skipping to drop value for key {key}");
                }

                None
            }
            _ => None,
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

impl From<Infallible> for Error {
    fn from(x: Infallible) -> Self {
        x.into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Item {
    Value(Value),

    #[serde(skip)]
    Array(Vec<Item>),

    #[serde(skip)]
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
        }
    }

    #[throws(<T as TryFrom<Self>>::Error)]
    pub fn convert<T: TryFrom<Self>>(self) -> T {
        T::try_from(self)?
    }

    #[throws(as Option)]
    pub fn as_bool(&self) -> bool {
        match self {
            Self::Value(Value::Bool(b)) => *b,
            _ => throw!(),
        }
    }
}

impl<T> TryFrom<Item> for Vec<T>
where
    T: TryFrom<Item>,
    Error: From<<T as TryFrom<Item>>::Error>,
{
    type Error = Error;

    #[throws(Self::Error)]
    fn try_from(item: Item) -> Self {
        match item {
            Item::Array(arr) => arr
                .into_iter()
                .map(T::try_from)
                .collect::<Result<_, _>>()
                .map_err(Into::into)?,
            item => vec![T::try_from(item)?],
        }
    }
}

impl<T> TryFrom<Item> for IndexMap<String, T>
where
    T: TryFrom<Item>,
    <T as TryFrom<Item>>::Error: Into<Error>,
{
    type Error = Error;

    #[throws(Self::Error)]
    fn try_from(item: Item) -> Self {
        match item {
            Item::Map(map) => map
                .into_iter()
                .map(|(k, v)| Ok((k, T::try_from(v).map_err(Into::into)?)))
                .collect::<Result<_, Self::Error>>()?,
            item => throw!(Error::MismatchedTypes(
                item.type_description(),
                TypeDescription::Array(Vec::new()),
            )),
        }
    }
}

macro_rules! impl_try_from_item {
    ($variant:ident::$inner_variant:ident for $impl_type:ty) => {
        impl TryFrom<Item> for $impl_type {
            type Error = Error;

            #[throws(Self::Error)]
            fn try_from(item: Item) -> Self {
                match item {
                    Item::$variant(v) => v.try_into()?,
                    item => throw!(Error::MismatchedTypes(
                        item.type_description(),
                        TypeDescription::$inner_variant,
                    )),
                }
            }
        }
    };
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    UnsignedInteger(u64),
    SignedInteger(i64),
    FloatingPointNumber(f64),
    String(String),
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

impl Display for Value {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Bool(v) => Debug::fmt(v, f)?,
            Self::UnsignedInteger(v) => Debug::fmt(v, f)?,
            Self::SignedInteger(v) => Debug::fmt(v, f)?,
            Self::FloatingPointNumber(v) => Debug::fmt(v, f)?,
            Self::String(v) => Debug::fmt(v, f)?,
        }
    }
}

impl TryFrom<Item> for Value {
    type Error = Error;

    #[throws(Self::Error)]
    fn try_from(item: Item) -> Self {
        match item {
            Item::Value(value) => value,
            item => throw!(Error::MismatchedTypes(
                item.type_description(),
                TypeDescription::Value,
            )),
        }
    }
}

macro_rules! impl_try_from_value_integer {
    ($variant:ident for $impl_type:ty) => {
        impl TryFrom<Value> for $impl_type {
            type Error = Error;

            #[throws(Self::Error)]
            fn try_from(value: Value) -> Self {
                match value {
                    Value::$variant(n) => <$impl_type>::try_from(n)
                        .map_err(|_| Error::OverflowingNumber(n as i128, stringify!($impl_type)))?,
                    value => throw!(Error::MismatchedTypes(
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

            #[throws(Self::Error)]
            fn try_from(value: Value) -> Self {
                match value {
                    Value::$variant(v) => v as $impl_type,
                    value => throw!(Error::MismatchedTypes(
                        value.type_description(),
                        TypeDescription::$variant,
                    )),
                }
            }
        }
    };
}

impl_try_from_value_integer!(UnsignedInteger for u8);
impl_try_from_value_integer!(UnsignedInteger for u16);
impl_try_from_value_integer!(UnsignedInteger for u32);
impl_try_from_value_integer!(UnsignedInteger for u64);
impl_try_from_value_integer!(SignedInteger for i8);
impl_try_from_value_integer!(SignedInteger for i16);
impl_try_from_value_integer!(SignedInteger for i32);
impl_try_from_value_integer!(SignedInteger for i64);
impl_try_from_value_non_integer!(Bool for bool);
impl_try_from_value_non_integer!(FloatingPointNumber for f32);
impl_try_from_value_non_integer!(FloatingPointNumber for f64);
impl_try_from_value_non_integer!(String for String);

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

#[derive(Debug, PartialEq)]
pub enum TypeDescription {
    Bool,
    UnsignedInteger,
    SignedInteger,
    FloatingPointNumber,
    String,
    Value,
    Array(Vec<Self>),
    Map(Vec<Self>),
}

impl Display for TypeDescription {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        match self {
            Self::Bool => write!(f, "bool")?,
            Self::UnsignedInteger => write!(f, "unsigned integer")?,
            Self::SignedInteger => write!(f, "signed integer")?,
            Self::FloatingPointNumber => write!(f, "floating point number")?,
            Self::String => write!(f, "string")?,
            Self::Value => write!(f, "value")?,
            Self::Array(col) | Self::Map(col) => {
                let col_ty = if matches!(self, Self::Array(_)) {
                    "array"
                } else {
                    "map"
                };

                if col.is_empty() {
                    write!(f, "{col_ty}")?
                } else if col.len() == 1 {
                    write!(f, "{col_ty} of {}", col[0])?
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
                    )?
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Mismatched value types: {0} ≠ {1}")]
    MismatchedTypes(TypeDescription, TypeDescription),

    #[error("{0} out of range for `{1}`")]
    OverflowingNumber(i128, &'static str),

    #[error(transparent)]
    Key(#[from] key::Error),

    #[error(transparent)]
    Log(#[from] log::Error),

    #[error(transparent)]
    Prompt(#[from] prompt::Error),

    #[error("An IO error occurred: {0}")]
    Io(#[from] io::Error),

    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    macro_rules! key {
        ($key:literal) => {
            Key::new_unchecked($key)
        };
    }

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

    #[throws(Error)]
    fn kv() -> Kv {
        let mut kv = Kv::new();
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
        kv.put_value(key!("unsigned"), 1u32, true)?;
        kv.put_value(key!("signed"), -1, true)?;
        kv.put_value(key!("float"), 1.0, true)?;
        kv.put_value(key!("u64"), u64::MAX, true)?;
        kv.put_value(key!("bool"), false, true)?;
        kv.put_value(key!("string"), "hello", true)?;
        kv.put_value(key!("nested/one"), true, true)?;
        kv.put_value(key!("nested/two/adam"), false, true)?;
        kv.put_value(key!("nested/two/betsy/alpha/token"), ttokens[0], true)?;
        kv.put_value(key!("nested/two/betsy/beta/token"), ttokens[1], true)?;
        kv.put_value(key!("nested/two/betsy/delta/token"), ttokens[2], true)?;
        kv.put_value(key!("nested/two/betsy/gamma/token"), ttokens[3], true)?;
        kv._put_array("array/one", ttokens, true)?;
        kv._put_array("array/two", rtokens.clone(), true)?;
        kv._put_map("map/one/adam/alpha", alpha, true)?;
        kv._put_array("map/one/adam/beta", rtokens, true)?;
        kv._put_map("map/one/betsy/alpha", alpha2, true)?;
        kv._put_map("map/one/betsy/alpha/extra", extra, true)?;
        kv._put_array("map/two", two, true)?;
        kv
    }

    #[throws(Error)]
    fn get_joined_vec(kv: &Kv, key: &Key) -> String {
        Vec::<String>::try_from(kv.get_item(key)?)?.join(",")
    }

    #[test]
    #[throws(Error)]
    fn get_single_leaf() {
        let kv = kv()?;
        u32::try_from(kv.get_item(key!("unsigned"))?)?.expect_val(1);
        i32::try_from(kv.get_item(key!("signed"))?)?.expect_val(-1);
        f32::try_from(kv.get_item(key!("float"))?)?.expect_val(1.0);
        u64::try_from(kv.get_item(key!("u64"))?)?.expect_val(u64::MAX);
        bool::try_from(kv.get_item(key!("bool"))?)?.expect_val(false);
        String::try_from(kv.get_item(key!("string"))?)?.expect_val("hello".to_string());
        bool::try_from(kv.get_item(key!("nested/one"))?)?.expect_val(true);
        bool::try_from(kv.get_item(key!("nested/*"))?)?.expect_val(true);
        bool::try_from(kv.get_item(key!("nested/two/adam"))?)?.expect_val(false);
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_leafs() {
        let kv = kv()?;
        get_joined_vec(&kv, key!("nested/two/betsy/*/token"))?
            .expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("nested/two/betsy/*ta/token"))?.expect_val("t2,t3".to_string());
        get_joined_vec(&kv, key!("nested/two/*/*/token"))?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("nested/*/*/*/token"))?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("nested/*/*/*a*a/token"))?.expect_val("t1,t4".to_string());
        get_joined_vec(&kv, key!("nested/**/token"))?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("nested/**/**/token"))?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("nested/**/betsy/*l*/token"))?.expect_val("t1,t3".to_string());
    }

    #[test]
    #[throws(Error)]
    fn get_single_array() {
        use Value::*;

        let kv = kv()?;
        Vec::<Value>::try_from(kv.get_item(key!("*"))?)?.expect_val(vec![
            UnsignedInteger(1),
            SignedInteger(-1),
            FloatingPointNumber(1.0),
            UnsignedInteger(u64::MAX),
            Bool(false),
            String("hello".into()),
        ]);
        Vec::<bool>::try_from(kv.get_item(key!("nested/*"))?)?.expect_val(vec![true]);
        Vec::<bool>::try_from(kv.get_item(key!("nested/*/*"))?)?.expect_val(vec![false]);
        get_joined_vec(&kv, key!("array/one/*"))?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("array/one/**"))?.expect_val("t1,t2,t3,t4".to_string());
        get_joined_vec(&kv, key!("array/*/*"))?.expect_val("t1,t2,t3,t4,r1,r2,r3,r4".to_string());
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_arrays() {
        let kv = kv()?;
        Vec::<Vec<String>>::try_from(kv.get_item(key!("array/*/**"))?)?.expect_val(vec![
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
    }

    #[test]
    #[throws(Error)]
    fn get_single_map() {
        let kv = kv()?;
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
        IndexMap::<String, Item>::try_from(kv.get_item(key!("map/one/adam/alpha/**"))?)?
            .expect_val(alpha);
        IndexMap::<String, Item>::try_from(kv.get_item(key!("map/one/adam/**"))?)?.expect_val(adam);
        IndexMap::<String, Item>::try_from(kv.get_item(key!("map/one/**"))?)?.expect_val(one);
        IndexMap::<String, Item>::try_from(kv.get_item(key!("map/**"))?)?.expect_val(map);
        IndexMap::<String, Item>::try_from(kv.get_item(key!("**"))?)?.expect_val(root);
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_maps() {
        let kv = kv()?;
        Vec::<IndexMap<String, Item>>::try_from(kv.get_item(key!("map/**/alpha/**"))?)?.expect_val(
            vec![
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
            ],
        );
    }

    #[test]
    #[should_panic]
    #[throws(Error)]
    fn get_array_no_zero_index() {
        let mut kv = kv()?;
        kv.put_value(key!("invalid_array/1"), false, true)?;
        Vec::<bool>::try_from(kv.get_item(key!("invalid_array/**"))?)?;
    }
}

pub mod ledger {
    use std::mem;

    use async_trait::async_trait;

    use super::{KeyOwned, Value};
    use crate::ledger::Transaction;

    pub struct Put {
        key: KeyOwned,
        previous_value: Option<Value>,
    }

    impl Put {
        pub fn new(key: KeyOwned, previous_value: Option<Value>) -> Self {
            Self {
                key,
                previous_value,
            }
        }
    }

    #[async_trait]
    impl Transaction for Put {
        fn description(&self) -> &'static str {
            "Put value"
        }

        async fn revert(&mut self) -> anyhow::Result<()> {
            let mut kv = crate::context::get_context().kv_mut().await;
            let key = mem::replace(&mut self.key, crate::context::key::KeyOwned::empty());
            match self.previous_value.take() {
                Some(previous_value) => {
                    kv.put_value(key, previous_value, true)?;
                }
                None => {
                    kv.drop_value(key, true)?;
                }
            }
            Ok(())
        }
    }
}
