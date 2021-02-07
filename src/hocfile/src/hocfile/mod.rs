mod validate;

use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    ops::Deref,
};

use indexmap::IndexSet;
use serde::Deserialize;

use crate::tree::{Edge, Tree};

const CMD: &str = "command";
const ARG: &str = "argument";
const OPL: &str = "optional";
const OPS: &str = "optional set";
const OPR: &str = "optional set reference";
const SCR: &str = "script";

fn comma_separated_list(list: &[String]) -> String {
    if list.len() == 1 {
        format!("'{}'", list[0])
    } else if list.len() == 2 {
        format!(
            "{} and {}",
            comma_separated_list(&list[0..1]),
            comma_separated_list(&list[1..2])
        )
    } else if list.len() > 2 {
        (0..list.len() - 1)
            .map(|i| comma_separated_list(&list[i..i + 1]))
            .collect::<Vec<_>>()
            .join(", ")
            + ", and "
            + &comma_separated_list(&list[list.len() - 1..])
    } else {
        "".into()
    }
}

#[derive(Debug, Error)]
pub enum HocfileError {
    #[error("Hocfile YAML is invalid")]
    YamlParse(#[from] serde_yaml::Error),

    #[error(
        "Multiple definitions of {resource_type} '{resource_name}' found{for_parent}",
        for_parent = .parent.as_ref().map_or("".into(), |(t, n)| format!(" for {} '{}'", t, n)),
    )]
    MultipleDefinitions {
        resource_type: &'static str,
        resource_name: String,
        parent: Option<(&'static str, String)>,
    },

    #[error("Missing {resource_type} '{resource_name}' for {parent_type} '{parent_name}'")]
    MissingResource {
        resource_type: &'static str,
        resource_name: String,
        parent_type: &'static str,
        parent_name: String,
    },

    #[error("Cyclic {resource_type} reference{s} {list} found",
        s = if .references.len() == 1 { "" } else { "s" },
        list = comma_separated_list(.references)
    )]
    CyclicReferences {
        resource_type: &'static str,
        references: Vec<String>,
    },
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct ResourceRef(String);

impl Deref for ResourceRef {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Hocfile {
    pub commands: Vec<Command>,
    pub optional_sets: Vec<OptionalSet>,
    pub scripts: Vec<Script>,
}

impl Hocfile {
    pub fn unvalidated_from_slice(slice: &[u8]) -> Result<Hocfile, HocfileError> {
        Ok(serde_yaml::from_slice(slice)?)
    }

    pub fn from_slice(slice: &[u8]) -> Result<Hocfile, Vec<HocfileError>> {
        let hocfile: Hocfile = serde_yaml::from_slice(slice).map_err(|err| vec![err.into()])?;

        let mut errors = Vec::new();

        hocfile.validate_conflicts(&mut errors);
        let ref_error = hocfile.validate_references(&mut errors);
        if !ref_error {
            hocfile.validate_cyclic_dependencies(&mut errors);
        }

        if errors.len() == 0 {
            Ok(hocfile)
        } else {
            Err(errors)
        }
    }

    /// # Panics
    /// If the Hocfile is unvalidated and one of the following occurs:
    ///   - a cyclic dependency is found
    ///   - a dependecy is missing
    pub fn optional_set_dependencies(&self) -> Tree<&OptionalSet> {
        let (nodes, edges) = self.get_optional_set_nodes_and_edges();

        Tree::new(nodes, edges).unwrap_or_else(|err| {
            panic!(
                "{}",
                HocfileError::CyclicReferences {
                    resource_type: OPS,
                    references: err.iter().map(|ops| ops.name.0.clone()).collect(),
                }
            )
        })
    }

    pub fn find_command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|cmd| cmd.name.deref() == name)
    }

    pub fn find_optional_set(&self, name: &str) -> Option<&OptionalSet> {
        self.optional_sets
            .iter()
            .find(|optional_set| optional_set.name.deref() == name)
    }

    pub fn find_script(&self, name: &str) -> Option<&Script> {
        self.scripts
            .iter()
            .find(|script| script.name.deref() == name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub name: ResourceRef,
    pub arguments: Vec<Argument>,
    pub optionals: Vec<Optional>,
    pub procedure: Vec<ProcedureStep>,
}

impl Command {
    pub fn arguments<'a>(&self) -> impl Iterator<Item = &Argument> {
        self.arguments.iter()
    }

    pub fn optionals<'a>(
        &'a self,
        hocfile: &'a Hocfile,
    ) -> impl Iterator<Item = &'a ConcreteOptional> {
        self.optionals.iter().flat_map(move |optional| {
            let mut concrete_optionals = Vec::new();
            let mut optionals = vec![optional];
            while let Some(optional) = optionals.pop() {
                match optional {
                    Optional::Concrete(optional) => {
                        concrete_optionals.push(optional);
                    }
                    Optional::Set { from_optional_set } => {
                        if let Some(optional_set) = hocfile.find_optional_set(from_optional_set) {
                            optionals.extend(&optional_set.optionals);
                        }
                    }
                }
            }
            concrete_optionals
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Argument {
    pub name: ResourceRef,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionalSet {
    pub name: ResourceRef,
    pub optionals: Vec<Optional>,
}

impl Hash for OptionalSet {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.name.hash(hasher)
    }
}

impl PartialEq for OptionalSet {
    fn eq(&self, other: &Self) -> bool {
        self.name.eq(&other.name)
    }
}

impl Eq for OptionalSet {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum Optional {
    Concrete(ConcreteOptional),

    #[serde(rename_all = "camelCase")]
    Set {
        from_optional_set: ResourceRef,
    },
}

impl Optional {
    fn as_concrete_optional(&self) -> Option<&ConcreteOptional> {
        match self {
            Self::Concrete(concrete_optional) => Some(concrete_optional),
            _ => None,
        }
    }

    fn as_optional_set_ref(&self) -> Option<&ResourceRef> {
        match self {
            Self::Set { from_optional_set } => Some(from_optional_set),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcreteOptional {
    pub name: ResourceRef,
    pub default: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcedureStep {
    BuiltIn(BuiltInFn),
    FromScript(ResourceRef),
    Script(String),
}

impl ProcedureStep {
    fn as_script_ref(&self) -> Option<&ResourceRef> {
        match self {
            Self::FromScript(script_ref) => Some(script_ref),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub enum BuiltInFn {
    DockerBuild,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub name: ResourceRef,
    pub source: String,
}
