use std::{
    borrow::Cow,
    convert::Infallible,
    fmt::{self, Debug, Display, Formatter},
    io, iter,
    marker::PhantomData,
    vec,
};

use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    context::key::{self, Key, KeyComponent, KeyOwned},
    log,
    prelude::*,
    prompt,
    util::Opt,
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
    pub fn get_item<'key, K>(&self, key: &K) -> Item
    where
        K: AsRef<Key> + ?Sized,
    {
        let key = key.as_ref();

        // If key does not contain any wildcards, then it is a "leaf", i.e. we can fetch the value
        // directly.
        if !key.contains_wildcard() {
            debug!("Getting item: {key}");

            return self
                .map
                .get(&*key)
                .map(|value| Item::Value(value.clone()))
                .ok_or_else(|| key::Error::KeyDoesNotExist(key.to_owned()))?;
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
                    ".*".to_owned()
                } else {
                    regex::escape(comp.as_str()).replace(r#"\*"#, "[^/]*")
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
        let keys: Vec<_> = self.map.keys().map(|k| k.as_str().to_owned()).collect();

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
                            .entry(prefix.map_or(Key::empty(), |m| Key::new(m.as_str())))
                            .and_modify(|suffixes: &mut IndexSet<_>| {
                                if let Some(suffix) = suffix {
                                    suffixes.insert(Self::nested_suffix(Key::new(suffix.as_str())));
                                }
                            })
                            .or_insert_with(|| {
                                let mut suffixes = IndexSet::new();
                                if let Some(suffix) = suffix {
                                    suffixes.insert(Self::nested_suffix(Key::new(suffix.as_str())));
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
                if let Ok(item) = self.get_item(prefix) {
                    result.push(item);
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
                        .as_str()
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
                        let nested_key = prefix.join(&suffix);
                        if let Ok(item) = self.get_item(&nested_key) {
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
                if let Ok(item) = self.get_item(&prefix.join(&suffix)) {
                    map.insert(
                        suffix.components().next().unwrap().as_str().to_owned(),
                        item,
                    );
                };
            }

            result.push(Item::Map(map));
        }

        if result.len() == 1 {
            result.remove(0)
        } else if result.len() > 1 {
            Item::Array(result)
        } else {
            throw!(key::Error::KeyDoesNotExist(key.to_owned()))
        }
    }

    /// Puts a value in the key-value store.
    ///
    /// Returns `None` if no previous value was present, `Some(None)` if a value is already present
    /// but not replaced, or `Some(Some(value))` if a previous value has been replaced.
    #[throws(Error)]
    pub fn put_value<'key, K, V>(&mut self, key: K, value: V, force: bool) -> Option<Option<Value>>
    where
        K: Into<KeyOwned>,
        V: Into<Value>,
    {
        let key = key.into();
        let value = value.into();

        let mut should_overwrite = false;
        'get: {
            match self.map.get(&*key) {
                Some(existing) if *existing != value => {
                    if force {
                        should_overwrite = true;
                        break 'get;
                    }

                    error!("Key {key} is already set with a different value");

                    let opt = select!("How do you want to resolve the key conflict?")
                        .with_options([Opt::Skip, Opt::Overwrite])
                        .get()?;

                    should_overwrite = opt == Opt::Overwrite;
                    if !should_overwrite {
                        warn!("Putting value: {key} => {value} (skipping)");
                        return Some(None);
                    }
                }
                Some(_) if !force => {
                    debug!("Putting value: {key} => {value} (no change)");
                    return Some(None);
                }
                _ => (),
            }
        }

        if !should_overwrite {
            debug!("Putting value: {key} => {value}");
        } else {
            if log_enabled!(Level::Trace) {
                trace!(
                    "Old item for key {key}: {}",
                    serde_json::to_string(&self.get_item(&*key)?)?
                );
            }
            log!(
                if force { Level::Debug } else { Level::Warn },
                "Putting value: {key} => {value} (overwriting)"
            );
        }

        self.map.insert(key.to_owned(), value).map(Some)
    }

    #[throws(Error)]
    pub fn _put_array<K, V, I>(&mut self, key_prefix: K, array: I, force: bool)
    where
        K: Into<KeyOwned>,
        V: Into<Value>,
        I: IntoIterator<Item = V>,
    {
        let key_prefix = key_prefix.into();
        for (index, value) in array.into_iter().enumerate() {
            self.put_value(key_prefix.join(&index.to_string()), value, force)?;
        }
    }

    #[throws(Error)]
    pub fn _put_map<'key, K, V, Q, I>(&mut self, key_prefix: K, map: I, force: bool)
    where
        K: Into<KeyOwned>,
        V: Into<Value>,
        Q: AsRef<Key> + ?Sized + 'key,
        I: IntoIterator<Item = (&'key Q, V)>,
    {
        let key_prefix = key_prefix.into();
        for (key, value) in map.into_iter() {
            let map_key = key_prefix.join(key);
            self.put_value(map_key, value, force)?;
        }
    }

    #[throws(Error)]
    pub fn drop_value<K>(&mut self, key: &K, force: bool) -> Option<Value>
    where
        K: AsRef<Key> + ?Sized,
    {
        let key = key.as_ref();

        debug!("Drop value: {key}");

        match self.map.remove(key.as_ref()) {
            Some(existing) => Some(existing),
            None if !force => {
                error!("Key {key} does not exist");

                select!("How do you want to resolve the key conflict?")
                    .with_option(Opt::Skip)
                    .get()?;

                warn!("Skipping to drop value for key {key}");

                None
            }
            _ => None,
        }
    }

    fn nested_suffix(full_suffix: &Key) -> Cow<Key> {
        if full_suffix.components().count() == 1 {
            Cow::Borrowed(full_suffix)
        } else {
            Cow::Owned(
                full_suffix
                    .components()
                    .take(1)
                    .chain(Some(KeyComponent::new("**")))
                    .collect::<KeyOwned>(),
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

    #[throws(Error)]
    pub fn convert<T>(self) -> T
    where
        T: TryFrom<Self, Error = Error>,
    {
        T::try_from(self)?
    }

    #[throws(as Option)]
    pub fn as_bool(&self) -> bool {
        match self {
            Self::Value(Value::Bool(b)) => *b,
            _ => throw!(),
        }
    }

    #[throws(as Option)]
    pub fn get<K>(&self, key: &K) -> &Self
    where
        K: AsRef<Key> + ?Sized,
    {
        let key = key.as_ref();

        let mut current = self;
        for component in key.components() {
            match current {
                Self::Value(_) => throw!(),
                Self::Array(array) => {
                    let index = component.as_str().parse::<usize>().ok()?;
                    current = array.get(index)?;
                }
                Self::Map(map) => {
                    current = map.get(&*component.as_str())?;
                }
            }
        }

        current
    }

    #[throws(as Option)]
    pub fn take<K>(self, key: &K) -> Self
    where
        K: AsRef<Key> + ?Sized,
    {
        let key = key.as_ref();

        let mut current = self;
        for component in key.components() {
            match current {
                Self::Value(_) => throw!(),
                Self::Array(array) => {
                    let index = component.as_str().parse::<usize>().ok()?;
                    current = array.into_iter().skip(index).next()?;
                }
                Self::Map(mut map) => {
                    current = map.remove(&*component.as_str())?;
                }
            }
        }

        current
    }

    pub fn into_iter(self) -> IntoIter {
        match self {
            item @ Self::Value(_) => IntoIter::Value(Some(item).into_iter()),
            Self::Array(array) => IntoIter::Array(array.into_iter()),
            Self::Map(map) => IntoIter::Map(map.into_values()),
        }
    }
}

impl<T> From<T> for Item
where
    T: Into<Value>,
{
    fn from(value: T) -> Self {
        Item::Value(value.into())
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

macro_rules! impl_from_for_value {
    ($ty:ty as $variant:ident) => {
        impl From<$ty> for Value {
            fn from(v: $ty) -> Self {
                Self::$variant(v)
            }
        }
    };

    ($ty:ty as $variant:ident => |$val:ident| $conversion:expr) => {
        impl From<$ty> for Value {
            fn from($val: $ty) -> Self {
                Self::$variant($conversion)
            }
        }
    };
}

impl_from_for_value!(String as String);
impl_from_for_value!(&str as String => |s| s.to_owned());
impl_from_for_value!(&String as String => |s| s.clone());
impl_from_for_value!(u64 as UnsignedInteger);
impl_from_for_value!(u32 as UnsignedInteger => |i| i as u64);
impl_from_for_value!(u16 as UnsignedInteger => |i| i as u64);
impl_from_for_value!(u8 as UnsignedInteger => |i| i as u64);
impl_from_for_value!(i64 as SignedInteger);
impl_from_for_value!(i32 as SignedInteger => |i| i as i64);
impl_from_for_value!(i16 as SignedInteger => |i| i as i64);
impl_from_for_value!(i8 as SignedInteger => |i| i as i64);
impl_from_for_value!(f64 as FloatingPointNumber);
impl_from_for_value!(f32 as FloatingPointNumber => |f| f as f64);
impl_from_for_value!(bool as Bool);

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

#[derive(PartialEq)]
pub enum ValueRef<'a> {
    Bool(bool),
    UnsignedInteger(u64),
    SignedInteger(i64),
    FloatingPointNumber(f64),
    String(&'a str),
}

impl PartialEq<ValueRef<'_>> for Value {
    fn eq(&self, other: &ValueRef) -> bool {
        match (self, other) {
            (Self::Bool(value), ValueRef::Bool(other)) => value == other,
            (Self::UnsignedInteger(value), ValueRef::UnsignedInteger(other)) => value == other,
            (Self::SignedInteger(value), ValueRef::SignedInteger(other)) => value == other,
            (Self::FloatingPointNumber(value), ValueRef::FloatingPointNumber(other)) => {
                value == other
            }
            (Self::String(value), ValueRef::String(other)) => value == other,
            _ => false,
        }
    }
}

impl<'a> From<&'a str> for ValueRef<'a> {
    fn from(v: &'a str) -> Self {
        Self::String(v)
    }
}

impl<'a> From<&'a String> for ValueRef<'a> {
    fn from(v: &'a String) -> Self {
        Self::String(v)
    }
}

macro_rules! impl_from_for_value_ref {
    ($ty:ty as $variant:ident) => {
        impl From<$ty> for ValueRef<'_> {
            fn from(v: $ty) -> Self {
                Self::$variant(v)
            }
        }
    };

    ($ty:ty as $variant:ident => |$val:ident| $conversion:expr) => {
        impl From<$ty> for ValueRef<'_> {
            fn from($val: $ty) -> Self {
                Self::$variant($conversion)
            }
        }
    };
}

impl_from_for_value_ref!(u64 as UnsignedInteger);
impl_from_for_value_ref!(u32 as UnsignedInteger => |i| i as u64);
impl_from_for_value_ref!(u16 as UnsignedInteger => |i| i as u64);
impl_from_for_value_ref!(u8 as UnsignedInteger => |i| i as u64);
impl_from_for_value_ref!(i64 as SignedInteger);
impl_from_for_value_ref!(i32 as SignedInteger => |i| i as i64);
impl_from_for_value_ref!(i16 as SignedInteger => |i| i as i64);
impl_from_for_value_ref!(i8 as SignedInteger => |i| i as i64);
impl_from_for_value_ref!(f64 as FloatingPointNumber);
impl_from_for_value_ref!(f32 as FloatingPointNumber => |f| f as f64);
impl_from_for_value_ref!(bool as Bool);

pub enum IntoIter {
    Value(std::option::IntoIter<Item>),
    Array(vec::IntoIter<Item>),
    Map(indexmap::map::IntoValues<String, Item>),
}

impl Iterator for IntoIter {
    type Item = Item;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        match self {
            Self::Value(iter) => iter.next()?,
            Self::Array(iter) => iter.next()?,
            Self::Map(iter) => iter.next()?,
        }
    }
}

pub trait IteratorExt: Iterator + Sized {
    fn filter_key_value<'a, K, V>(self, key: &'a K, value: V) -> FilterKeyValue<'a, Self>
    where
        K: AsRef<Key> + ?Sized,
        V: Into<ValueRef<'a>>,
    {
        FilterKeyValue {
            inner: self,
            key: key.as_ref(),
            value: value.into(),
        }
    }

    fn try_get_key<'a, K>(self, key: &'a K) -> TryGetKey<'a, Self>
    where
        K: AsRef<Key> + ?Sized,
    {
        TryGetKey {
            inner: self,
            key: key.as_ref(),
        }
    }

    fn and_convert<T>(self) -> AndConvert<Self, T> {
        AndConvert {
            inner: self,
            _marker: Default::default(),
        }
    }
}

impl<I> IteratorExt for I where I: Iterator {}

pub struct FilterKeyValue<'a, I> {
    inner: I,
    key: &'a Key,
    value: ValueRef<'a>,
}

impl<I> Iterator for FilterKeyValue<'_, I>
where
    I: Iterator<Item = Item>,
{
    type Item = Item;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        let elem = self.inner.next()?;
        match elem.get(self.key)? {
            Item::Value(value) if value == &self.value => elem,
            _ => throw!(),
        }
    }
}

pub struct TryGetKey<'a, I> {
    inner: I,
    key: &'a Key,
}

impl<I> Iterator for TryGetKey<'_, I>
where
    I: Iterator<Item = Item>,
{
    type Item = Result<Item, Error>;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        self.inner
            .next()?
            .take(self.key)
            .ok_or_else(|| key::Error::KeyDoesNotExist(self.key.to_owned()).into())
    }
}

pub struct AndConvert<I, T> {
    inner: I,
    _marker: PhantomData<T>,
}

impl<I, T> Iterator for AndConvert<I, T>
where
    I: Iterator<Item = Result<Item, Error>>,
    T: TryFrom<Item, Error = Error>,
{
    type Item = Result<T, Error>;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        self.inner.next()?.and_then(Item::convert)
    }
}

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

    #[error(transparent)]
    Custom(#[from] anyhow::Error),
}

pub mod ledger {
    use std::borrow::Cow;

    use async_trait::async_trait;

    use super::{KeyOwned, Value};
    use crate::{context, ledger::Transaction};

    pub struct Put {
        key: KeyOwned,
        current_value: Value,
        previous_value: Option<Value>,
    }

    impl Put {
        pub fn new(key: KeyOwned, current_value: Value, previous_value: Option<Value>) -> Self {
            Self {
                key,
                current_value,
                previous_value,
            }
        }
    }

    #[async_trait]
    impl Transaction for Put {
        fn description(&self) -> Cow<'static, str> {
            "Put value".into()
        }

        fn detail(&self) -> Cow<'static, str> {
            format!("Value to revert: {}", self.current_value).into()
        }

        async fn revert(mut self: Box<Self>) -> anyhow::Result<()> {
            let mut kv = context::get_context().kv_mut().await;
            match self.previous_value.take() {
                Some(previous_value) => {
                    kv.put_value(self.key, previous_value, true)?;
                }
                None => {
                    kv.drop_value(&self.key, true)?;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    macro_rules! item_map {
        ($map:ident => @impl $key:literal map=> $value:expr, $($rest:tt)*) => {
            $map.insert($key.to_owned(), Item::Map($value));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl $key:literal array=> $value:expr, $($rest:tt)*) => {
            $map.insert($key.to_owned(), Item::Array($value));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl $key:literal => $value:expr, $($rest:tt)*) => {
            $map.insert($key.to_owned(), Item::from($value));
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
        ($($key:literal => $value:expr),* $(,)?) => {{
            let mut map = IndexMap::<&str, Value>::new();
            $(map.insert($key, {$value}.into());)*
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
        kv.put_value("unsigned", 1u32, true)?;
        kv.put_value("signed", -1, true)?;
        kv.put_value("float", 1.0, true)?;
        kv.put_value("u64", u64::MAX, true)?;
        kv.put_value("bool", false, true)?;
        kv.put_value("string", "hello", true)?;
        kv.put_value("nested/one", true, true)?;
        kv.put_value("nested/two/adam", false, true)?;
        kv.put_value("nested/two/betsy/alpha/token", ttokens[0], true)?;
        kv.put_value("nested/two/betsy/beta/token", ttokens[1], true)?;
        kv.put_value("nested/two/betsy/delta/token", ttokens[2], true)?;
        kv.put_value("nested/two/betsy/gamma/token", ttokens[3], true)?;
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
    fn get_joined_vec<K: AsRef<Key> + ?Sized>(kv: &Kv, key: &K) -> String {
        Vec::<String>::try_from(kv.get_item(key)?)?.join(",")
    }

    #[test]
    #[throws(Error)]
    fn get_single_leaf() {
        let kv = kv()?;
        u32::try_from(kv.get_item("unsigned")?)?.expect_val(1);
        i32::try_from(kv.get_item("signed")?)?.expect_val(-1);
        f32::try_from(kv.get_item("float")?)?.expect_val(1.0);
        u64::try_from(kv.get_item("u64")?)?.expect_val(u64::MAX);
        bool::try_from(kv.get_item("bool")?)?.expect_val(false);
        String::try_from(kv.get_item("string")?)?.expect_val("hello".to_owned());
        bool::try_from(kv.get_item("nested/one")?)?.expect_val(true);
        bool::try_from(kv.get_item("nested/*")?)?.expect_val(true);
        bool::try_from(kv.get_item("nested/two/adam")?)?.expect_val(false);
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_leafs() {
        let kv = kv()?;
        get_joined_vec(&kv, "nested/two/betsy/*/token")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "nested/two/betsy/*ta/token")?.expect_val("t2,t3".to_owned());
        get_joined_vec(&kv, "nested/two/*/*/token")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "nested/*/*/*/token")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "nested/*/*/*a*a/token")?.expect_val("t1,t4".to_owned());
        get_joined_vec(&kv, "nested/**/token")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "nested/**/**/token")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "nested/**/betsy/*l*/token")?.expect_val("t1,t3".to_owned());
    }

    #[test]
    #[throws(Error)]
    fn get_single_array() {
        use Value::*;

        let kv = kv()?;
        Vec::<Value>::try_from(kv.get_item("*")?)?.expect_val(vec![
            UnsignedInteger(1),
            SignedInteger(-1),
            FloatingPointNumber(1.0),
            UnsignedInteger(u64::MAX),
            Bool(false),
            String("hello".into()),
        ]);
        Vec::<bool>::try_from(kv.get_item("nested/*")?)?.expect_val(vec![true]);
        Vec::<bool>::try_from(kv.get_item("nested/*/*")?)?.expect_val(vec![false]);
        get_joined_vec(&kv, "array/one/*")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "array/one/**")?.expect_val("t1,t2,t3,t4".to_owned());
        get_joined_vec(&kv, "array/*/*")?.expect_val("t1,t2,t3,t4,r1,r2,r3,r4".to_owned());
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_arrays() {
        let kv = kv()?;
        Vec::<Vec<String>>::try_from(kv.get_item("array/*/**")?)?.expect_val(vec![
            vec![
                "t1".to_owned(),
                "t2".to_owned(),
                "t3".to_owned(),
                "t4".to_owned(),
            ],
            vec![
                "r1".to_owned(),
                "r2".to_owned(),
                "r3".to_owned(),
                "r4".to_owned(),
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
        IndexMap::<String, Item>::try_from(kv.get_item("map/one/adam/alpha/**")?)?
            .expect_val(alpha);
        IndexMap::<String, Item>::try_from(kv.get_item("map/one/adam/**")?)?.expect_val(adam);
        IndexMap::<String, Item>::try_from(kv.get_item("map/one/**")?)?.expect_val(one);
        IndexMap::<String, Item>::try_from(kv.get_item("map/**")?)?.expect_val(map);
        IndexMap::<String, Item>::try_from(kv.get_item("**")?)?.expect_val(root);
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_maps() {
        let kv = kv()?;
        Vec::<IndexMap<String, Item>>::try_from(kv.get_item("map/**/alpha/**")?)?.expect_val(vec![
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
    }

    #[test]
    #[should_panic]
    #[throws(Error)]
    fn get_array_no_zero_index() {
        let mut kv = kv()?;
        kv.put_value("invalid_array/1", false, true)?;
        Vec::<bool>::try_from(kv.get_item("invalid_array/**")?)?;
    }
}
