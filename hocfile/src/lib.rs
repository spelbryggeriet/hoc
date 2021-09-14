#[macro_use]
extern crate thiserror;

mod deserialize;
mod exec;
mod tree;

use std::collections::HashMap;

use serde::{Serialize, Deserialize};

pub use deserialize::*;
pub use exec::exec_hoc_line;

pub type HocState = HashMap<String, HocValue>;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum HocValue {
    String(String),
    List(Vec<HocValue>),
}

impl HocValue {
    pub fn as_string(self) -> Result<String, Self> {
        match self {
            Self::String(s) => Ok(s),
            _ => Err(self),
        }
    }

    pub fn as_string_ref(&self) -> Option<&String> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_string_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_list(self) -> Result<Vec<Self>, Self> {
        match self {
            Self::List(l) => Ok(l),
            _ => Err(self),
        }
    }

    pub fn as_list_ref(&self) -> Option<&Vec<Self>> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_list_mut(&mut self) -> Option<&mut Vec<Self>> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }
}
