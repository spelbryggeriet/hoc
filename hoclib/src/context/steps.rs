use std::{
    collections::HashMap,
    fs::{self, Permissions},
    io,
    os::unix::prelude::PermissionsExt,
};

use hoclog::error;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize};
use thiserror::Error;

use crate::{
    dir_state,
    procedure::{self, Attributes, ProcedureState, ProcedureStep},
    process, Context, DirComparison, DirState, Procedure,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct StepsIndex(pub(super) String, pub(super) Attributes);

impl StepsIndex {
    pub fn name(&self) -> &str {
        self.0.as_str()
    }

    pub fn attributes(&self) -> &Attributes {
        &self.1
    }
}

#[derive(Debug, Default)]
pub struct StepsMap(pub(super) HashMap<StepsIndex, Steps>);

impl Serialize for StepsMap {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Output<'a> {
            attributes: &'a Attributes,
            #[serde(flatten)]
            cache: &'a Steps,
        }

        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        let cache_map: HashMap<_, Vec<_>> =
            self.0
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

impl<'de> Deserialize<'de> for StepsMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InputVisitor;

        impl<'de> Visitor<'de> for InputVisitor {
            type Value = StepsMap;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map of attributed caches")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                #[derive(Deserialize)]
                struct Input {
                    attributes: Attributes,
                    #[serde(flatten)]
                    cache: Steps,
                }

                let mut cache_map = HashMap::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((key, caches)) = map.next_entry::<String, Vec<Input>>()? {
                    for attr_cache in caches {
                        let cache_index = StepsIndex(key.clone(), attr_cache.attributes);
                        if cache_map.contains_key(&cache_index) {
                            let key = cache_index.name();
                            let attrs = cache_index.attributes();
                            return Err(serde::de::Error::custom(format!(
                                "duplicate cache {key} with attributes {{{}}}",
                                attrs
                                    .iter()
                                    .map(|(k, v)| format!("{k:?}: {v}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )));
                        }
                        cache_map.insert(cache_index, attr_cache.cache);
                    }
                }
                Ok(StepsMap(cache_map))
            }
        }

        deserializer.deserialize_map(InputVisitor)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("procedure step: {0}")]
    ProcedureStep(#[from] procedure::Error),

    #[error("process: {0}")]
    Process(#[from] process::Error),

    #[error("directory state: {0}")]
    DirState(#[from] dir_state::Error),
}

impl From<Error> for hoclog::Error {
    fn from(err: Error) -> Self {
        error!(err.to_string()).unwrap_err()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Steps {
    #[serde(rename = "completed_steps")]
    completed: Vec<ProcedureStep>,
    #[serde(rename = "current_step")]
    current: Option<ProcedureStep>,
}

impl Steps {
    pub fn new<P: Procedure>(procedure: &P) -> Result<Self, Error> {
        Ok(Self {
            completed: Vec::new(),
            current: Some(ProcedureStep::new(procedure)?),
        })
    }

    pub fn completed(&self) -> &[ProcedureStep] {
        &self.completed
    }

    pub fn current(&self) -> Option<&ProcedureStep> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut ProcedureStep> {
        self.current.as_mut()
    }

    pub fn next<S: ProcedureState>(&mut self, state: &Option<S>) -> Result<(), Error> {
        if let Some(state) = state {
            if let Some(mut current_step) = self.current.take() {
                current_step.work_dir_state_mut().commit()?;
                current_step.work_dir_state_mut().refresh()?;

                let work_dir_state = current_step.work_dir_state().clone();
                self.completed.push(current_step);
                self.current
                    .replace(ProcedureStep::from_states(state, work_dir_state)?);
            }
        } else if let Some(current_step) = self.current.take() {
            self.completed.push(current_step);
        }

        Ok(())
    }

    pub fn invalidate_state<S: ProcedureState>(&mut self, id: S::Id) -> Result<(), Error> {
        for (index, step) in self.completed.iter().enumerate() {
            if step.id::<S>()? == id {
                self.completed.truncate(index + 1);
                self.current.replace(self.completed.remove(index));
                break;
            }
        }

        Ok(())
    }

    pub fn oldest_invalid_state<P: Procedure>(
        &self,
        procedure: &P,
    ) -> Result<Option<(usize, <P::State as ProcedureState>::Id)>, Error> {
        let work_dir = Context::get_work_dir(procedure);
        let completed_steps = self.completed.len();
        let mut invalid_state = None;
        let mut invalidate_previous_step = false;
        let mut cur_dir_state = DirState::from_dir(&work_dir)?;

        for (step, index) in self
            .current
            .iter()
            .chain(self.completed.iter().rev())
            .zip((0..=completed_steps).rev())
        {
            if invalidate_previous_step {
                invalid_state.replace((index, step.id::<P::State>()?));
            }

            let comp = DirComparison::compare(step.work_dir_state(), &cur_dir_state);
            if matches!(comp, DirComparison::Same) {
                break;
            }

            let added_dirs: Vec<_> = comp
                .all_added_dirs()
                .map(|dir| dir.path().to_path_buf())
                .collect();
            let added_files: Vec<_> = comp
                .all_added_files()
                .map(|file| file.path().to_path_buf())
                .collect();

            let comp = if !added_dirs.is_empty() || !added_files.is_empty() {
                for file in added_files {
                    fs::remove_file(&file)?;
                    cur_dir_state.untrack(&file.strip_prefix(&work_dir).unwrap());
                }

                for dir in added_dirs {
                    fs::remove_dir_all(&dir)?;
                    cur_dir_state.untrack(&dir.strip_prefix(&work_dir).unwrap());
                }

                cur_dir_state.commit()?;

                let comp = DirComparison::compare(step.work_dir_state(), &cur_dir_state);
                if matches!(comp, DirComparison::Same) {
                    break;
                }

                comp
            } else {
                comp
            };

            invalidate_previous_step = comp.has_removed_paths();
            for file in comp.all_modified_files() {
                if file.new_checksum().is_none() {
                    if let Some(old_mode) = file.old_mode() {
                        fs::set_permissions(file.path(), Permissions::from_mode(old_mode))?;
                        continue;
                    }
                }

                invalidate_previous_step = true;
            }

            for dir in comp.all_modified_dirs() {
                if let Some(old_mode) = dir.old_mode() {
                    fs::set_permissions(dir.path(), Permissions::from_mode(old_mode))?;
                }
            }

            cur_dir_state.refresh_modes()?;
        }

        Ok(invalid_state)
    }
}
