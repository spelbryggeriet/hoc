use std::{
    error::Error as StdError,
    fmt::{self, Display, Formatter},
    hash::{Hash, Hasher},
    ops::{Deref, DerefMut},
    str::FromStr,
};

use hoc_log::error;
use indexmap::{IndexMap, IndexSet};
use serde::{
    de::{DeserializeOwned, Visitor},
    ser::SerializeMap,
    Deserialize, Deserializer, Serialize, Serializer,
};
use thiserror::Error;

use crate::{kv::WriteStore, process};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Key(String, Attributes);

impl Key {
    pub fn new(name: String, attributes: Attributes) -> Self {
        Self(name, attributes)
    }

    pub fn name(&self) -> &str {
        self.0.as_str()
    }

    pub fn attributes(&self) -> &Attributes {
        &self.1
    }
}

impl Display for Key {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.0)?;

        let mut iter = self.1.iter();
        if let Some((last_name, last_attr)) = iter.next_back() {
            write!(f, "(")?;
            for (name, attr) in iter {
                write!(f, r#"{name}="{attr}", "#)?;
            }
            write!(f, r#"{last_name}="{last_attr}")"#)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attributes(IndexMap<String, String>);

impl Hash for Attributes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (key, value) in self.0.iter() {
            state.write(key.as_bytes());
            state.write(value.as_bytes());
        }
    }
}

impl Deref for Attributes {
    type Target = IndexMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Attributes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct Dependencies(IndexSet<Key>);

impl Deref for Dependencies {
    type Target = IndexSet<Key>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Dependencies {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Serialize for Dependencies {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for key in self.0.iter() {
            map.serialize_entry(&key.0, &key.1)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Dependencies {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct InputVisitor;

        impl<'de> Visitor<'de> for InputVisitor {
            type Value = Dependencies;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map of dependencies")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut dependencies = IndexSet::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((name, attributes)) = map.next_entry::<String, Attributes>()? {
                    dependencies.insert(Key::new(name, attributes));
                }
                Ok(Dependencies(dependencies))
            }
        }

        deserializer.deserialize_map(InputVisitor)
    }
}

pub enum HaltState<S> {
    Halt(S),
    Finish,
}

pub struct Halt<S> {
    pub persist: bool,
    pub state: HaltState<S>,
}

pub trait Procedure: Sized {
    type State: State;

    const NAME: &'static str;

    fn key(&self) -> Key {
        Key(Self::NAME.to_string(), self.get_attributes())
    }

    fn get_attributes(&self) -> Attributes {
        Attributes::default()
    }

    fn get_dependencies(&self) -> Dependencies {
        Dependencies::default()
    }

    fn run(
        &mut self,
        state: Self::State,
        registry: &impl WriteStore,
    ) -> hoc_log::Result<Halt<Self::State>>;
}

pub trait Id:
    Clone + Copy + Eq + Ord + FromStr<Err = Self::DeserializeError> + Into<&'static str>
where
    Self: Sized,
{
    type DeserializeError: 'static + StdError;

    fn description(&self) -> &'static str;

    fn as_str(self) -> &'static str {
        self.into()
    }

    fn parse<S: AsRef<str>>(input: S) -> Result<Self, Self::DeserializeError> {
        match Self::from_str(input.as_ref()) {
            Ok(id) => Ok(id),
            Err(err) => Err(err),
        }
    }
}

pub trait State: Serialize + DeserializeOwned + Default {
    type Id: Id;

    fn id(&self) -> Self::Id;
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("serde json: {0}")]
    SerdeJson(#[from] serde_json::Error),

    #[error("id: {0}")]
    Id(Box<dyn StdError>),

    #[error("process: {0}")]
    Process(#[from] process::Error),
}

impl From<Error> for hoc_log::Error {
    fn from(err: Error) -> Self {
        error!("{err}").unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Step {
    state: String,
    id: String,
}

impl Step {
    pub fn new<P: Procedure>() -> Result<Self, Error> {
        let state = P::State::default();
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(&state)?,
        })
    }

    pub fn from_state<S: State>(state: &S) -> Result<Self, Error> {
        Ok(Self {
            id: state.id().as_str().to_string(),
            state: serde_json::to_string(state)?,
        })
    }

    pub fn id<S: State>(&self) -> Result<S::Id, Error> {
        S::Id::parse(&self.id).map_err(|e| Error::Id(Box::new(e)))
    }

    pub fn state<S: State>(&self) -> Result<S, Error> {
        serde_json::from_str(&self.state).map_err(|e| Error::Id(Box::new(e)))
    }
}
