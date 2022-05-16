use std::collections::HashMap;

use hoc_log::error;
use indexmap::IndexMap;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize, Serializer};
use thiserror::Error;

use crate::procedure::{Attributes, Key, Procedure};

pub use crate::context::history::item::Item;

mod item;

#[derive(Debug, Error)]
pub enum Error {
    #[error("item already exists: {_0}")]
    ItemAlreadyExists(Key),

    #[error("item does not exist: {_0}")]
    ItemDoesNotExist(Key),

    #[error("item: {0}")]
    Item(#[from] item::Error),
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Default)]
pub struct History {
    map: IndexMap<Key, Item>,
}

impl History {
    pub fn get_item_key<P: Procedure>(&self, procedure: &P) -> Option<Key> {
        let key = procedure.key();
        self.map.contains_key(&key).then(|| key)
    }

    pub fn add_item<P: Procedure>(&mut self, procedure: &P) -> Result<Key, Error> {
        let item = Item::new(procedure)?;
        let key = procedure.key();

        if self.map.contains_key(&key) {
            return Err(Error::ItemAlreadyExists(key));
        }

        self.map.insert(key.clone(), item);
        Ok(key)
    }

    pub fn remove_item(&mut self, key: &Key) -> Result<(), Error> {
        if !self.map.contains_key(key) {
            return Err(Error::ItemDoesNotExist(key.clone()));
        }

        self.map.remove(key);
        Ok(())
    }

    pub fn item(&self, key: &Key) -> &Item {
        &self.map[key]
    }

    pub fn item_mut(&mut self, key: &Key) -> &mut Item {
        self.map.get_mut(key).unwrap()
    }

    pub fn iter(&self) -> indexmap::map::Iter<Key, Item> {
        self.map.iter()
    }

    pub fn keys(&self) -> indexmap::map::Keys<Key, Item> {
        self.map.keys()
    }

    pub fn items(&self) -> indexmap::map::Values<Key, Item> {
        self.map.values()
    }
}

impl Serialize for History {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Output<'a> {
            attributes: &'a Attributes,
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
                formatter.write_str("a map of history items")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Input {
                    attributes: Attributes,
                    #[serde(flatten)]
                    item: Item,
                }

                let mut history_map = IndexMap::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((name, attr_items)) = map.next_entry::<String, Vec<Input>>()? {
                    for attr_item in attr_items {
                        let key = Key::new(name.clone(), attr_item.attributes);
                        if history_map.contains_key(&key) {
                            return Err(serde::de::Error::custom(format!(
                                "duplicate history item: {key}"
                            )));
                        }
                        history_map.insert(key, attr_item.item);
                    }
                }
                Ok(History { map: history_map })
            }
        }

        deserializer.deserialize_map(InputVisitor)
    }
}
