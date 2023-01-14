use std::{
    fmt::{self, Debug, Display, Formatter},
    iter::Enumerate,
    marker::PhantomData,
    ops::Deref,
    vec,
};

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{
    context::{
        key::{self, Key, KeyComponent, KeyOwned},
        Error,
    },
    prelude::*,
    util::Opt,
};

#[derive(Serialize, Deserialize)]
pub struct Kv {
    #[serde(flatten)]
    map: IndexMap<KeyOwned, ValueType>,
}

impl Kv {
    pub(super) fn new() -> Self {
        Self {
            map: IndexMap::new(),
        }
    }

    fn get_keys<K>(&self, template: &K) -> Vec<KeyOwned>
    where
        K: AsRef<Key> + ?Sized,
    {
        let template = template.as_ref();

        // Build the regex expression, replacing wildcards and escaping regex tokens
        let mut regex_str = String::from("^");
        regex_str += &template
            .components()
            .map(|comp| {
                if comp.is_nested_wildcard() {
                    // Nested means we are able to span multiple components
                    ".*".to_owned()
                } else {
                    // We may find '*' wildcard in the component, so we convert them to a regex
                    // expression that is bounded within the component
                    regex::escape(comp.as_str()).replace(r#"\*"#, "[^/]*")
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        regex_str += "$";
        let regex = Regex::new(&regex_str).expect("regular expression should be parsed correctly");

        // Collect all keys that matches the regular expression
        self.map
            .keys()
            .filter(|key| regex.is_match(key.as_str()))
            .cloned()
            .collect()
    }

    #[throws(Error)]
    pub fn get_item<K>(&self, template: &K) -> Item
    where
        K: AsRef<Key> + ?Sized,
    {
        let template = template.as_ref();

        if !template.contains_wildcard() {
            return self
                .map
                .get(template)
                .map(|value| Item::Value(value.deref().clone()))
                .ok_or_else(|| Error::KeyDoesNotExist(template.to_owned()))?;
        }

        let mut item_builder = ItemBuilder::new(template);

        for key in self.get_keys(template) {
            debug!("Get value: {key:?}");

            let existing = self
                .map
                .get(key.as_ref())
                .map(|value| value.deref().clone())
                .expect("key should exist");

            item_builder.push(key, existing);
        }

        item_builder
            .build()
            .ok_or_else(|| Error::KeyDoesNotExist(template.to_owned()))?
    }

    pub fn item_exists<K>(&self, key: &K) -> bool
    where
        K: AsRef<Key> + ?Sized,
    {
        self.map
            .keys()
            .any(|k| k.as_str().starts_with(key.as_ref().as_str()))
    }

    /// Puts a value in the key-value store.
    ///
    /// Returns `None` if no previous value was present, `Some(None)` if a value is already present
    /// but not replaced, or `Some(Some(value))` if a previous value has been replaced.
    #[throws(Error)]
    pub fn put_value<K, V>(
        &mut self,
        key: K,
        value: V,
        options: PutOptions,
    ) -> Option<Option<Value>>
    where
        K: Into<KeyOwned>,
        V: Into<Value> + Clone + Display,
    {
        let key = key.into();
        let into_value = value.clone();
        let value = if !options.temporary {
            ValueType::Persistent(value.into())
        } else {
            ValueType::Temporary(value.into())
        };
        let desc = if !options.temporary {
            "persistent value"
        } else {
            "temporary value"
        };
        let verb = if options.update {
            "Updating"
        } else {
            "Putting"
        };

        let mut should_overwrite = false;
        'get: {
            match self.map.get(&*key) {
                Some(existing) if **existing != *value => {
                    if existing.is_temporary() && value.is_persistent() {
                        error!("Key {key:?} is already set with a different value and is marked as temporary");
                    } else if existing.is_persistent() && value.is_temporary() {
                        error!("Key {key:?} is already set with a different value and is marked as persistent");
                    } else if !options.update {
                        error!("Key {key:?} is already set with a different value");
                    } else {
                        break 'get;
                    }

                    let opt = select!("How do you want to resolve the key conflict?")
                        .with_options([Opt::Skip, Opt::Overwrite])
                        .get()?;

                    should_overwrite = opt == Opt::Overwrite;
                    if !should_overwrite {
                        warn!("{verb} {desc}: {key:?} => {into_value} (skipping)");
                        return Some(None);
                    }
                }
                Some(existing) => {
                    if existing.is_temporary() && value.is_persistent() {
                        error!("Key {key:?} is marked as temporary");
                    } else if existing.is_persistent() && value.is_temporary() {
                        error!("Key {key:?} is marked as persistent");
                    } else {
                        debug!("{verb} {desc}: {key:?} => {into_value} (no change)");
                        return Some(None);
                    }

                    let opt = select!("How do you want to resolve the key conflict?")
                        .with_options([Opt::Skip, Opt::Overwrite])
                        .get()?;

                    should_overwrite = opt == Opt::Overwrite;
                    if !should_overwrite {
                        warn!("{verb} {desc}: {key:?} => {into_value} (skipping)");
                        return Some(None);
                    }
                }
                None if options.update => {
                    error!("Key {key:?} not found for updating");

                    let write_anyway = Opt::Custom("Write anyway");
                    let opt = select!("How do you want to resolve the key conflict?")
                        .with_options([Opt::Skip, write_anyway])
                        .get()?;

                    if opt != write_anyway {
                        warn!("{verb} {desc}: {key:?} => {into_value} (skipping)");
                        return Some(None);
                    }
                }
                None => (),
            }
        }

        if !should_overwrite {
            debug!("{verb} {desc}: {key:?} => {into_value}");
        } else {
            debug!(
                "Old item for key {key:?}: {}",
                serde_json::to_string(&self.get_item(&*key)?)?
            );
            warn!("{verb} {desc}: {key:?} => {into_value} (overwriting)");
        }

        self.map
            .insert(key, value)
            .map(ValueType::into_inner)
            .map(Some)
    }

    #[throws(Error)]
    #[allow(unused)]
    pub fn put_array<K, V, I>(&mut self, key_prefix: K, array: I, options: PutOptions)
    where
        K: Into<KeyOwned>,
        V: Into<Value> + Clone + Display,
        I: IntoIterator<Item = V>,
    {
        let key_prefix = key_prefix.into();
        for (index, value) in array.into_iter().enumerate() {
            self.put_value(key_prefix.join(&index.to_string()), value, options)?;
        }
    }

    #[throws(Error)]
    #[allow(unused)]
    pub fn put_map<'key, K, V, Q, I>(&mut self, key_prefix: K, map: I, options: PutOptions)
    where
        K: Into<KeyOwned>,
        V: Into<Value> + Clone + Display,
        Q: AsRef<Key> + ?Sized + 'key,
        I: IntoIterator<Item = (&'key Q, V)>,
    {
        let key_prefix = key_prefix.into();
        for (key, value) in map.into_iter() {
            let map_key = key_prefix.join(key);
            self.put_value(map_key, value, options)?;
        }
    }

    #[throws(Error)]
    pub fn drop_item<K, D>(&mut self, template: &K, on_drop: D) -> Option<Item>
    where
        K: AsRef<Key> + ?Sized,
        D: Fn(&mut Option<Item>, bool),
    {
        let template = template.as_ref();

        if !template.contains_wildcard() {
            return match self.map.remove(template) {
                Some(existing) => {
                    let is_temporary = existing.is_temporary();
                    let mut res = Some(Item::Value(existing.into_inner()));
                    on_drop(&mut res, is_temporary);
                    res
                }
                None => {
                    error!("Key {template:?} does not exist");

                    select!("How do you want to resolve the key conflict?")
                        .with_option(Opt::Skip)
                        .get()?;

                    warn!("Skipping to drop value for key {template:?}");

                    None
                }
            };
        }

        let mut item_builder = ItemBuilder::new(template);

        for key in self.get_keys(template) {
            debug!("Drop value: {key:?}");

            let existing = self.map.remove(key.as_ref()).expect("key should exist");
            let is_temporary = existing.is_temporary();
            let mut item = Some(Item::Value(existing.into_inner()));
            on_drop(&mut item, is_temporary);
            if let Some(Item::Value(value)) = item {
                item_builder.push(key, value);
            }
        }

        match item_builder.build() {
            Some(item) => Some(item),
            None => {
                error!("Key {template:?} does not exist");

                select!("How do you want to resolve the key conflict?")
                    .with_option(Opt::Skip)
                    .get()?;

                warn!("Skipping to drop value for key {template:?}");

                None
            }
        }
    }

    pub fn drop_temporary_values(&mut self) {
        self.map
            .retain(|_, value| matches!(value, ValueType::Persistent(_)));
    }
}

#[derive(Default, Clone, Copy)]
pub struct PutOptions {
    pub temporary: bool,
    pub update: bool,
}

struct ItemBuilder {
    known_prefix: KeyOwned,
    item: Option<Item>,
}

impl ItemBuilder {
    fn new(template: &Key) -> Self {
        Self {
            // The known prefix are all prefix components that are known to the caller
            known_prefix: key::get_known_prefix_for_template(template).into(),
            item: None,
        }
    }

    fn push(&mut self, key: KeyOwned, value: Value) {
        // Skip all components that are known
        let mut self_comps = self.known_prefix.components();
        let mut other_comps = key.components();
        let mut first_other_comp = None;
        for other_comp in &mut other_comps {
            let Some(self_comp) = self_comps.next() else {
                first_other_comp.replace(other_comp);
                break;
            };

            if self_comp != other_comp {
                panic!("key is unrelated");
            }
        }

        // Key being the same as the known prefix is only allowed if we have not pushed any values
        // prior
        let Some(KeyComponent(mut first_other_comp)) = first_other_comp else {
            if self.item.is_none() {
                self.item.replace(Item::Value(value));
                return;
            } else {
                // Key must have a unique lineage of components
                panic!("key is not unique");
            }
        };

        // Helper closure for converting linear keys to nested maps
        let nest_key = |item, KeyComponent(comp)| {
            let mut map = IndexMap::new();
            map.insert(comp.to_owned(), item);
            Item::Map(map)
        };

        // Get the current item or insert the new value as a nested map item
        let Some(mut self_item) = self.item.as_mut() else {
            let mut self_map = IndexMap::new();
            let other_item = other_comps.rev().fold(Item::Value(value), nest_key);
            self_map.insert(first_other_comp.to_owned(), other_item);
            self.item.replace(Item::Map(self_map));
            return;
        };

        loop {
            // If a map was not stored previously, then it was a value and only one value can ever
            // be pushed to a given key prefix
            let Item::Map(self_map) = self_item else {
                panic!("key would overwrite previous value");
            };

            if !self_map.contains_key(first_other_comp) {
                let other_item = other_comps.rev().fold(Item::Value(value), nest_key);
                self_map.insert(first_other_comp.to_owned(), other_item);
                break;
            }

            self_item = self_map.get_mut(first_other_comp).unwrap();

            let Some(KeyComponent(other_comp)) = other_comps.next() else {
                // Key must have a unique lineage of components
                panic!("key is not unique");
            };
            first_other_comp = other_comp;
        }
    }

    fn build(mut self) -> Option<Item> {
        let mut stack = vec![self.item.as_mut()?];

        while let Some(item) = stack.pop() {
            let Item::Map(map) = item else { continue };

            let array_indices: Option<Vec<usize>> = map.keys().map(|s| s.parse().ok()).collect();
            if let Some(mut array_indices) = array_indices {
                array_indices.sort_unstable();
                let starts_at_zero_and_are_consecutive = (0..map.len())
                    .zip(array_indices)
                    .all(|(expected, index)| expected == index);

                if starts_at_zero_and_are_consecutive {
                    map.sort_keys();
                    *item = Item::Array(map.drain(..).map(|(_, value)| value).collect());
                }
            }

            match item {
                Item::Map(map) => stack.extend(map.values_mut()),
                Item::Array(arr) => stack.extend(arr.iter_mut()),
                _ => (), // No need to inspect single values
            }
        }

        self.item
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Item {
    Value(Value),
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
                    current = map.get(component.as_str())?;
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
                    current = array.into_iter().nth(index)?;
                }
                Self::Map(mut map) => {
                    current = map.remove(component.as_str())?;
                }
            }
        }

        current
    }

    pub fn into_values(self) -> IntoValues {
        match self {
            Self::Value(value) => IntoValues::Value(Some(value).into_iter()),
            Self::Array(array) => {
                let mut array = array.into_iter();
                IntoValues::Array {
                    current: array.next().map(|item| Box::new(item.into_values())),
                    array,
                }
            }
            Self::Map(map) => {
                let mut map = map.into_values();
                IntoValues::Map {
                    current: map.next().map(|item| Box::new(item.into_values())),
                    map,
                }
            }
        }
    }

    pub fn into_key_values(self) -> IntoKeyValues {
        match self {
            Self::Value(value) => IntoKeyValues::Value(Some(value).into_iter()),
            Self::Array(array) => {
                let mut array = array.into_iter().enumerate();
                IntoKeyValues::Array {
                    current: array
                        .next()
                        .map(|(index, item)| (index, Box::new(item.into_key_values()))),
                    array,
                }
            }
            Self::Map(map) => {
                let mut map = map.into_iter();
                IntoKeyValues::Map {
                    current: map
                        .next()
                        .map(|(key, item)| (key, Box::new(item.into_key_values()))),
                    map,
                }
            }
        }
    }
}

impl IntoIterator for Item {
    type IntoIter = IntoIter;
    type Item = <IntoIter as Iterator>::Item;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            item @ Self::Value(_) => IntoIter::Value(Some(item).into_iter()),
            Self::Array(array) => IntoIter::Array(array.into_iter()),
            Self::Map(map) => IntoIter::Map(map.into_values()),
        }
    }
}

impl Display for Item {
    #[throws(fmt::Error)]
    fn fmt(&self, f: &mut Formatter) {
        write!(
            f,
            "{}",
            serde_yaml::to_string(self).unwrap_or_else(|_| "<invalid format>".to_owned())
        )?;
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
            item => throw!(Error::MismatchedTypes {
                expected: TypeDescription::Array(Vec::new()),
                actual: item.type_description(),
            }),
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
            item => throw!(Error::MismatchedTypes {
                expected: TypeDescription::Map(Vec::new()),
                actual: item.type_description(),
            }),
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
                    item => throw!(Error::MismatchedTypes {
                        expected: TypeDescription::$inner_variant,
                        actual: item.type_description(),
                    }),
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ValueType {
    Persistent(Value),

    #[serde(skip)]
    Temporary(Value),
}

impl ValueType {
    fn is_persistent(&self) -> bool {
        matches!(self, ValueType::Persistent(_))
    }

    fn is_temporary(&self) -> bool {
        matches!(self, ValueType::Temporary(_))
    }
}

impl ValueType {
    fn into_inner(self) -> Value {
        match self {
            Self::Persistent(value) => value,
            Self::Temporary(value) => value,
        }
    }
}

impl Deref for ValueType {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        match &self {
            Self::Persistent(value) => value,
            Self::Temporary(value) => value,
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
            item => throw!(Error::MismatchedTypes {
                expected: TypeDescription::Value,
                actual: item.type_description(),
            }),
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
                    value => throw!(Error::MismatchedTypes {
                        expected: TypeDescription::$variant,
                        actual: value.type_description(),
                    }),
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
                    value => throw!(Error::MismatchedTypes {
                        expected: TypeDescription::$variant,
                        actual: value.type_description(),
                    }),
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

    fn try_get<K>(self, key: &K) -> TryGetKey<Self>
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
        loop {
            let elem = self.inner.next()?;
            match elem.get(self.key) {
                Some(Item::Value(value)) if value == &self.value => break elem,
                _ => (),
            }
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
            .ok_or_else(|| Error::KeyDoesNotExist(self.key.to_owned()))
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

pub enum IntoValues {
    Value(std::option::IntoIter<Value>),
    Array {
        current: Option<Box<IntoValues>>,
        array: vec::IntoIter<Item>,
    },
    Map {
        current: Option<Box<IntoValues>>,
        map: indexmap::map::IntoValues<String, Item>,
    },
}

impl Iterator for IntoValues {
    type Item = Value;

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        loop {
            match self {
                Self::Value(iter) => break iter.next()?,
                Self::Array { current, array } => {
                    let Some(elem) = current.as_mut()?.next() else {
                        *current = array.next().map(|item| Box::new(item.into_values()));
                        continue;
                    };
                    break elem;
                }
                Self::Map { current, map } => {
                    let Some(elem) = current.as_mut()?.next() else {
                        *current = map.next().map(|item| Box::new(item.into_values()));
                        continue;
                    };
                    break elem;
                }
            }
        }
    }
}

pub enum IntoKeyValues {
    Value(std::option::IntoIter<Value>),
    Array {
        current: Option<(usize, Box<IntoKeyValues>)>,
        array: Enumerate<vec::IntoIter<Item>>,
    },
    Map {
        current: Option<(String, Box<IntoKeyValues>)>,
        map: indexmap::map::IntoIter<String, Item>,
    },
}

impl Iterator for IntoKeyValues {
    type Item = (KeyOwned, Value);

    #[throws(as Option)]
    fn next(&mut self) -> Self::Item {
        loop {
            match self {
                Self::Value(iter) => break iter.next().map(|value| (KeyOwned::empty(), value))?,
                Self::Array { current, array } => {
                    let current_mut = current.as_mut()?;
                    let Some((key, elem)) = current_mut.1.next() else {
                        *current = array.next().map(|(index, item)| (index, Box::new(item.into_key_values())));
                        continue;
                    };
                    break (key.join(Key::new(&current_mut.0.to_string())), elem);
                }
                Self::Map { current, map } => {
                    let current_mut = current.as_mut()?;
                    let Some((key, elem)) = current_mut.1.next() else {
                        *current = map.next().map(|(key, item)| (key, Box::new(item.into_key_values())));
                        continue;
                    };
                    break (key.join(Key::new(&current_mut.0)), elem);
                }
            }
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

#[cfg(test)]
mod tests {
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

        ($map:ident => @impl map $key_value:ident, $($rest:tt)*) => {
            $map.insert(stringify!($key_value).to_owned(), Item::Map($key_value.clone()));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl array $key_value:ident, $($rest:tt)*) => {
            $map.insert(stringify!($key_value).to_owned(), Item::Array($key_value.clone()));
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl **$value:expr, $($rest:tt)*) => {
            for (key, value) in $value {
                $map.insert(key.to_owned(), Item::from(value));
            }
            item_map!($map => @impl $($rest)*);
        };

        ($map:ident => @impl $key_value:ident, $($rest:tt)*) => {
            $map.insert(stringify!($key_value).to_owned(), Item::from($key_value));
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
            #[allow(clippy::vec_init_then_push)]
            let array = {
                let mut array = Vec::<Item>::new();
                item_array!(array => @impl $($input)*,);
                array
            };
            array
        }};
    }

    macro_rules! value_array {
         ($($value:expr),* $(,)?) => {{
             let mut array = Vec::<Value>::new();
             $(array.push({$value}.into());)*
             array
         }};
    }

    macro_rules! value_map {
        ($($key_value:ident),* $(,)?) => {{
            let mut map = IndexMap::<&str, Value>::new();
            $(map.insert(stringify!($key_value), $key_value.into());)*
            map
        }};
    }

    macro_rules! expect_equal {
        ($expected:expr, $actual:expr) => {{
            let expected = &$expected;
            let actual = &$actual;
            assert!(
                expected == actual,
                "\n\nexpected: {expected:?}\nactual: {actual:?}\n\n",
            );
        }};
    }

    macro_rules! expect_dropped {
        ($kv:expr, $key:expr) => {{
            assert!(matches!(
                $kv.get_item($key),
                Err(Error::KeyDoesNotExist(key)) if key.as_str() == $key),
                "item was not dropped");
        }};
    }

    fn item_map_to_kv(item_map: IndexMap<String, Item>) -> Kv {
        fn key_item_to_key_values(
            (key, item): (String, Item),
        ) -> Box<dyn Iterator<Item = (KeyOwned, ValueType)>> {
            match item {
                Item::Value(value) => {
                    Box::new([(KeyOwned::from(key), ValueType::Temporary(value))].into_iter())
                }
                Item::Array(array) => Box::new(array.into_iter().enumerate().flat_map(
                    move |(index, item)| key_item_to_key_values((format!("{key}/{index}"), item)),
                )),
                Item::Map(map) => Box::new(map.into_iter().flat_map(move |(inner_key, item)| {
                    key_item_to_key_values((format!("{key}/{inner_key}"), item))
                })),
            }
        }

        Kv {
            map: item_map
                .into_iter()
                .flat_map(key_item_to_key_values)
                .collect(),
        }
    }

    fn m_root() -> IndexMap<String, Item> {
        let nested = m_nested();
        let array = m_array();
        let map = m_map();

        item_map! {
            **r_root(),
            map nested,
            map array,
            map map,
        }
    }

    fn r_root() -> IndexMap<String, Item> {
        let unsigned = v_unsigned();
        let signed = v_signed();
        let float = v_float();
        let unsigned64 = v_unsigned64();
        let boolean = v_boolean();
        let string = v_string();

        item_map! {
            unsigned,
            signed,
            float,
            unsigned64,
            boolean,
            string,
        }
    }

    fn v_unsigned() -> u32 {
        1
    }

    fn v_signed() -> i32 {
        -1
    }

    fn v_float() -> f32 {
        1.0
    }

    fn v_unsigned64() -> u64 {
        u64::MAX
    }

    fn v_boolean() -> bool {
        false
    }

    fn v_string() -> &'static str {
        "hello"
    }

    fn m_nested() -> IndexMap<String, Item> {
        let adam = v_nested_two_adam();

        item_map! {
            **r_nested(),
            "two" map=> item_map! {
                adam,
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
        }
    }

    fn r_nested() -> IndexMap<String, Item> {
        let one = v_nested_one();

        item_map! {
            one,
        }
    }

    fn v_nested_one() -> bool {
        true
    }

    fn v_nested_two_adam() -> bool {
        true
    }

    fn m_array() -> IndexMap<String, Item> {
        let inner = a_array_inner();
        item_map! {
            array inner,
        }
    }

    fn a_array_inner() -> Vec<Item> {
        item_array!["t1", "t2", "t3", "t4"]
    }

    fn m_map() -> IndexMap<String, Item> {
        let one = m_map_one();
        item_map! {
            map one,
            "two" array=> item_array![
                "t1",
                "t2",
                "t3",
                "t4",
                "r1",
                "r2",
                "r3",
                "r4",
            ],
        }
    }

    fn m_map_one() -> IndexMap<String, Item> {
        let adam = m_map_one_adam();
        let betsy = m_map_one_betsy();

        item_map! {
            map adam,
            map betsy,
        }
    }

    fn m_map_one_adam() -> IndexMap<String, Item> {
        let alpha = m_map_one_adam_alpha();

        item_map! {
            map alpha,
            "beta" array=> item_array![
                "r1",
                "r2",
                "r3",
                "r4",
            ],
        }
    }

    fn m_map_one_adam_alpha() -> IndexMap<String, Item> {
        item_map! {
            "string" => "hello",
            "int" => 1,
        }
    }

    fn m_map_one_betsy() -> IndexMap<String, Item> {
        item_map! {
            "alpha" map=> item_map! {
                "string" => "hello",
                "int" => 2,
                "extra" map=> item_map! {
                    "bool" => false,
                    "i64" => i64::MIN,
                },
            },
        }
    }

    #[test]
    #[throws(Error)]
    fn get_single_value() {
        let unsigned = v_unsigned();
        let signed = v_signed();
        let float = v_float();
        let unsigned64 = v_unsigned64();
        let boolean = v_boolean();
        let string = v_string();

        let nested = m_nested();
        let root = item_map! {
            **r_root(),
            map nested,
        };

        let kv = item_map_to_kv(root);

        macro_rules! expect {
            ($expected:ident: $type:ty) => {{
                let actual = kv
                    .get_item(&stringify!($expected).replace('_', "/"))?
                    .convert::<$type>()?;
                expect_equal!($expected, actual);
            }};
        }

        let nested_one = v_nested_one();
        let nested_two_adam = v_nested_two_adam();

        expect!(unsigned: u32);
        expect!(signed: i32);
        expect!(float: f32);
        expect!(unsigned64: u64);
        expect!(boolean: bool);
        expect!(string: String);
        expect!(nested_one: bool);
        expect!(nested_two_adam: bool);
    }

    #[test]
    #[throws(Error)]
    fn get_multiple_values() {
        let nested = m_nested();
        let root = item_map! {
            map nested,
        };

        let kv = item_map_to_kv(root);

        macro_rules! expect {
            ($key:literal => $($expected:literal),*) => {{
                let actual: Vec<_> = kv.get_item($key)?.into_values().map(Item::Value).collect();
                expect_equal!(item_array![$($expected),*], actual);
            }};
        }

        expect!("nested/two/betsy/*/token" => "t1", "t2", "t3", "t4");
        expect!("nested/two/betsy/*ta/token" => "t2", "t3");
        expect!("nested/two/*/*/token" => "t1", "t2", "t3", "t4");
        expect!("nested/*/*/*/token" => "t1", "t2", "t3", "t4");
        expect!("nested/*/*/*a*a/token" => "t1", "t4");
        expect!("nested/**/token" => "t1", "t2", "t3", "t4");
        expect!("nested/**/**/token" => "t1", "t2", "t3", "t4");
        expect!("nested/**/betsy/*l*/token" => "t1", "t3");
    }

    #[test]
    #[throws(Error)]
    fn get_array() {
        let array = m_array();
        let root = item_map! {
            map array,
        };

        let kv = item_map_to_kv(root);

        macro_rules! expect {
            ($key:literal => $expected:expr) => {{
                let actual = kv.get_item($key)?.convert::<Vec<_>>()?;
                expect_equal!($expected, actual);
            }};
        }

        let array_inner = a_array_inner();

        expect!("array/inner/*" => array_inner);
        expect!("array/inner/**" => array_inner);
    }

    #[test]
    #[should_panic]
    #[throws(Error)]
    fn get_array_no_zero_index() {
        let kv = item_map_to_kv(item_map! {
            "invalid_array" map=> item_map! {
                "1" => true,
            },
        });

        kv.get_item("invalid_array/**")?.convert::<Vec<bool>>()?;
    }

    #[test]
    #[throws(Error)]
    fn get_map() {
        let kv = item_map_to_kv(m_root());

        macro_rules! expect {
            ($key:literal => $expected:expr) => {{
                let actual = kv.get_item($key)?.convert::<IndexMap<_, _>>()?;
                expect_equal!($expected, actual);
            }};
        }

        // Map of values at a single level
        expect!("*" => r_root());
        expect!("nested/*" => r_nested());
        expect!("nested/*/*" => item_map! {
            "two" map=> item_map! {
                "adam" => v_nested_two_adam(),
            },
        });

        // Map of values at multiple nested levels with single wildcard
        expect!("map/one/adam/alpha/**" => m_map_one_adam_alpha());
        expect!("map/one/adam/**" => m_map_one_adam());
        expect!("map/one/**" => m_map_one());
        expect!("map/**" => m_map());
        expect!("**" => m_root());

        // Map of values at multiple nested levels with multiple wildcards
        let alpha = m_map_one_adam_alpha();
        let betsy = m_map_one_betsy();
        expect!("map/**/alpha/**" => item_map! {
            "one" map=> item_map! {
                "adam" map=> item_map! {
                    map alpha,
                },
                map betsy,
            },
        });
        expect!("array/*/**" => m_array());
    }

    #[test]
    #[throws(Error)]
    fn put_value() {
        let unsigned = v_unsigned();
        let signed = v_signed();
        let float = v_float();
        let unsigned64 = v_unsigned64();
        let boolean = v_boolean();
        let string = v_string();

        let mut kv = Kv::new();

        macro_rules! expect {
            ($expected:ident: $type:ty) => {{
                kv.put_value(
                    &stringify!($expected),
                    $expected,
                    PutOptions {
                        temporary: true,
                        update: false,
                    },
                )?;
                let actual = kv.get_item(&stringify!($expected))?.convert::<$type>()?;
                expect_equal!($expected, actual);
            }};
        }

        expect!(unsigned: u32);
        expect!(signed: i32);
        expect!(float: f32);
        expect!(unsigned64: u64);
        expect!(boolean: bool);
        expect!(string: String);
    }

    #[test]
    #[throws(Error)]
    fn put_array() {
        let unsigned = v_unsigned();
        let signed = v_signed();
        let float = v_float();
        let unsigned64 = v_unsigned64();
        let boolean = v_boolean();
        let string = v_string();

        let array1 = value_array![unsigned];
        let array2 = value_array![unsigned, unsigned];
        let array3 = value_array![unsigned, signed];
        let array4 = value_array![unsigned, signed, unsigned64];
        let array5 = value_array![unsigned, signed, float, unsigned64, boolean, string];

        let mut kv = Kv::new();

        macro_rules! expect {
            ($expected:ident: $type:ty) => {{
                kv.put_array(
                    &stringify!($expected),
                    $expected.clone(),
                    PutOptions {
                        temporary: true,
                        update: false,
                    },
                )?;
                let expected = $expected
                    .into_iter()
                    .map(<$type>::try_from)
                    .collect::<Result<Vec<_>, _>>()?;

                // Test getting whole array at once
                let actual = kv
                    .get_item(&concat!(stringify!($expected), "/*"))?
                    .convert::<Vec<$type>>()?;
                expect_equal!(expected, actual);

                // Test getting single elements
                for i in 0..expected.len() {
                    let actual = kv
                        .get_item(&format!(concat!(stringify!($expected), "/{}"), i))?
                        .convert::<$type>()?;
                    expect_equal!(expected[i], actual);
                }
            }};
        }

        expect!(array1: u32);
        expect!(array2: u32);
        expect!(array3: Value);
        expect!(array4: Value);
        expect!(array5: Value);
    }

    #[test]
    #[throws(Error)]
    fn put_map() {
        let unsigned = v_unsigned();
        let signed = v_signed();
        let float = v_float();
        let unsigned64 = v_unsigned64();
        let boolean = v_boolean();
        let string = v_string();

        let map1 = value_map![unsigned];
        let map2 = value_map![unsigned, unsigned];
        let map3 = value_map![unsigned, signed];
        let map4 = value_map![unsigned, signed, unsigned64];
        let map5 = value_map![unsigned, signed, float, unsigned64, boolean, string];

        let mut kv = Kv::new();

        macro_rules! expect {
            ($expected:ident: $type:ty) => {{
                kv.put_map(
                    &stringify!($expected),
                    $expected.clone(),
                    PutOptions {
                        temporary: true,
                        update: false,
                    },
                )?;
                let expected = $expected
                    .into_iter()
                    .map(|(key, value)| {
                        Ok::<_, <$type as TryFrom<Value>>::Error>((
                            key.to_owned(),
                            <$type>::try_from(value)?,
                        ))
                    })
                    .collect::<Result<IndexMap<_, _>, _>>()?;

                // Test getting whole map at once
                let actual = kv
                    .get_item(&concat!(stringify!($expected), "/*"))?
                    .convert::<IndexMap<_, $type>>()?;
                expect_equal!(expected, actual);

                // Test getting single elements
                for key in expected.keys() {
                    let actual = kv
                        .get_item(&format!(concat!(stringify!($expected), "/{}"), key))?
                        .convert::<$type>()?;
                    expect_equal!(expected[key], actual);
                }
            }};
        }

        expect!(map1: u32);
        expect!(map2: u32);
        expect!(map3: Value);
        expect!(map4: Value);
        expect!(map5: Value);
    }

    #[test]
    #[throws(Error)]
    fn drop_single_value() {
        let unsigned = v_unsigned();
        let signed = v_signed();
        let float = v_float();
        let unsigned64 = v_unsigned64();
        let boolean = v_boolean();
        let string = v_string();

        let nested = m_nested();
        let root = item_map! {
            **r_root(),
            map nested,
        };

        let mut kv = item_map_to_kv(root);

        macro_rules! expect {
            ($expected:ident: $type:ty) => {{
                let actual = kv
                    .drop_item(&stringify!($expected).replace('_', "/"), |_, _| ())?
                    .expect(concat!(stringify!($expected), " should contain an item"))
                    .convert::<$type>()?;
                expect_equal!($expected, actual);
                expect_dropped!(kv, stringify!($expected));
            }};
        }

        let nested_one = v_nested_one();
        let nested_two_adam = v_nested_two_adam();

        expect!(unsigned: u32);
        expect!(signed: i32);
        expect!(float: f32);
        expect!(unsigned64: u64);
        expect!(boolean: bool);
        expect!(string: String);
        expect!(nested_one: bool);
        expect!(nested_two_adam: bool);
    }

    #[test]
    #[throws(Error)]
    fn drop_multiple_values() {
        let nested = m_nested();
        let root = item_map! {
            map nested,
        };

        let mut kv = item_map_to_kv(root);

        macro_rules! expect {
            ($key:literal => $($expected:literal),*) => {{
                let actual: Vec<_> = kv
                    .drop_item($key, |_, _| ())?
                    .expect(concat!($key, " should contain an item"))
                    .into_values()
                    .map(Item::Value)
                    .collect();
                expect_equal!(item_array![$($expected),*], actual);
                expect_dropped!(kv, $key);
            }};
        }

        expect!("nested/two/betsy/*ta/token" => "t2", "t3");
        expect!("nested/two/betsy/*/token" => "t1", "t4");
    }

    #[test]
    #[throws(Error)]
    fn drop_array() {
        let array = m_array();
        let root = item_map! {
            map array,
        };

        let mut kv = item_map_to_kv(root);

        macro_rules! expect {
            ($key:literal => $expected:expr) => {{
                let actual = kv
                    .drop_item($key, |_, _| ())?
                    .expect(concat!($key, " should contain an item"))
                    .convert::<Vec<_>>()?;
                expect_equal!($expected, actual);
                expect_dropped!(kv, $key);
            }};
        }

        let array_inner = a_array_inner();

        expect!("array/inner/*" => array_inner);
    }

    #[test]
    #[throws(Error)]
    fn drop_array_no_zero_index() {
        let invalid_array = item_map! {
            "1" => true,
        };
        let mut kv = item_map_to_kv(item_map! {
            map invalid_array,
        });

        macro_rules! expect {
            ($key:literal => $expected:expr) => {{
                let actual = kv
                    .drop_item($key, |_, _| ())?
                    .expect(concat!($key, " should contain an item"))
                    .convert::<IndexMap<_, _>>()?;
                expect_equal!($expected, actual);
                expect_dropped!(kv, $key);
            }};
        }

        expect!("invalid_array/**" => invalid_array);
    }

    #[test]
    #[throws(Error)]
    fn drop_map() {
        let mut kv = item_map_to_kv(m_root());

        macro_rules! expect {
            ($key:literal => $expected:expr) => {{
                let actual = kv
                    .drop_item($key, |_, _| ())?
                    .expect(concat!($key, " should contain an item"))
                    .convert::<IndexMap<_, _>>()?;
                expect_equal!($expected, actual);
            }};
        }

        // Map of values at a single level
        expect!("*" => r_root());
        expect!("nested/*" => r_nested());
        expect!("nested/*/*" => item_map! {
            "two" map=> item_map! {
                "adam" => v_nested_two_adam(),
            },
        });

        // Map of values at multiple nested levels with single wildcard
        expect!("map/**" => m_map());

        // Map of values at multiple nested levels with multiple wildcards
        expect!("array/*/**" => m_array());
    }
}
