use std::{collections::HashMap, path::PathBuf};

use hoclog::error;
use indexmap::IndexMap;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize};
use thiserror::Error;

use crate::procedure::{Attribute, Procedure};

pub use crate::context::history::item::Item;

mod item;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Index(String, Vec<Attribute>);

fn attrs_string(attrs: &[Attribute]) -> String {
    attrs
        .iter()
        .map(|Attribute { key, value }| format!(r#""{key}": {value}"#))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Index {
    pub fn name(&self) -> &str {
        self.0.as_str()
    }

    pub fn attributes(&self) -> &[Attribute] {
        &self.1
    }
}

impl From<Index> for PathBuf {
    fn from(index: Index) -> Self {
        PathBuf::from(&index)
    }
}

impl From<&Index> for PathBuf {
    fn from(index: &Index) -> Self {
        let mut path = PathBuf::new();
        path.push(index.name());
        path.extend(index.attributes().iter().map(|a| &a.value));
        path
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "item already exists: {} with attributes {{{}}}",
        _0.name(),
        attrs_string(_0.attributes()),
    )]
    ItemAlreadyExists(Index),

    #[error(
        "item does not exist: {} with attributes {{{}}}",
        _0.name(),
        attrs_string(_0.attributes()),
    )]
    ItemDoesNotExist(Index),

    #[error("item: {0}")]
    Item(#[from] item::Error),
}

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Default)]
pub struct History {
    map: IndexMap<Index, Item>,
}

impl History {
    pub fn get_index<P: Procedure>(&self, procedure: &P) -> Option<Index> {
        let cache_index = Index(P::NAME.to_string(), procedure.get_attributes());
        self.map.contains_key(&cache_index).then(|| cache_index)
    }

    pub fn add_item<P: Procedure>(&mut self, procedure: &P) -> Result<Index, Error> {
        let item = Item::new::<P>()?;
        let index = Index(P::NAME.to_string(), procedure.get_attributes());

        if self.map.contains_key(&index) {
            return Err(Error::ItemAlreadyExists(index));
        }

        self.map.insert(index.clone(), item);
        Ok(index)
    }

    pub fn remove_item(&mut self, index: &Index) -> Result<(), Error> {
        if !self.map.contains_key(index) {
            return Err(Error::ItemDoesNotExist(index.clone()));
        }

        self.map.remove(index);
        Ok(())
    }

    pub fn item(&self, index: &Index) -> &Item {
        &self.map[index]
    }

    pub fn item_mut(&mut self, index: &Index) -> &mut Item {
        self.map.get_mut(index).unwrap()
    }

    pub fn indices(&self) -> indexmap::map::Keys<Index, Item> {
        self.map.keys()
    }
}

impl Serialize for History {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Output<'a> {
            attributes: &'a [Attribute],
            #[serde(flatten)]
            cache: &'a Item,
        }

        let mut map = serializer.serialize_map(Some(self.map.len()))?;
        let cache_map: HashMap<_, Vec<_>> =
            self.map
                .iter()
                .fold(HashMap::new(), |mut cache_map, (index, cache)| {
                    let mut data = vec![Output {
                        attributes: index.attributes(),
                        cache,
                    }];
                    cache_map
                        .entry(index.name().to_string())
                        .and_modify(|e| e.extend(data.drain(..)))
                        .or_insert(data);
                    cache_map
                });
        for (key, value) in &cache_map {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for History {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InputVisitor;

        impl<'de> Visitor<'de> for InputVisitor {
            type Value = History;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map of attributed caches")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Input {
                    attributes: Vec<Attribute>,
                    #[serde(flatten)]
                    cache: Item,
                }

                let mut cache_map = IndexMap::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((key, caches)) = map.next_entry::<String, Vec<Input>>()? {
                    for attr_cache in caches {
                        let cache_index = Index(key.clone(), attr_cache.attributes);
                        if cache_map.contains_key(&cache_index) {
                            let key = cache_index.name();
                            let attrs = cache_index.attributes();
                            return Err(serde::de::Error::custom(format!(
                                "duplicate cache {key} with attributes {{{}}}",
                                attrs
                                    .iter()
                                    .map(|Attribute { key, value }| format!("{key:?}: {value}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )));
                        }
                        cache_map.insert(cache_index, attr_cache.cache);
                    }
                }
                Ok(History { map: cache_map })
            }
        }

        deserializer.deserialize_map(InputVisitor)
    }
}
