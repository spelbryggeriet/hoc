use std::{
    collections::HashMap,
    fs::{self, Permissions},
    io, mem,
    os::unix::prelude::PermissionsExt,
    path::PathBuf,
};

use hoclog::error;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize};
use thiserror::Error;

use crate::{
    dir_state,
    procedure::{self, Attribute, Procedure, State, Step},
    process, DirComparison, DirState,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct StepHistoryIndex(pub(super) String, pub(super) Vec<Attribute>);

impl StepHistoryIndex {
    pub fn name(&self) -> &str {
        self.0.as_str()
    }

    pub fn attributes(&self) -> &[Attribute] {
        &self.1
    }
}

#[derive(Debug, Default)]
pub struct StepHistoryMap(pub(super) HashMap<StepHistoryIndex, StepHistory>);

impl Serialize for StepHistoryMap {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Output<'a> {
            attributes: &'a [Attribute],
            #[serde(flatten)]
            cache: &'a StepHistory,
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

impl<'de> Deserialize<'de> for StepHistoryMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InputVisitor;

        impl<'de> Visitor<'de> for InputVisitor {
            type Value = StepHistoryMap;

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
                    cache: StepHistory,
                }

                let mut cache_map = HashMap::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((key, caches)) = map.next_entry::<String, Vec<Input>>()? {
                    for attr_cache in caches {
                        let cache_index = StepHistoryIndex(key.clone(), attr_cache.attributes);
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
                Ok(StepHistoryMap(cache_map))
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
pub struct StepHistory {
    #[serde(rename = "completed_steps")]
    completed: Vec<Step>,
    #[serde(rename = "current_step")]
    current: Option<Step>,
}

impl StepHistory {
    pub fn new<P: Procedure>(procedure: &P) -> Result<Self, Error> {
        Ok(Self {
            completed: Vec::new(),
            current: Some(Step::from_procedure(procedure)?),
        })
    }

    pub fn completed(&self) -> &[Step] {
        &self.completed
    }

    pub fn current(&self) -> Option<&Step> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut Step> {
        self.current.as_mut()
    }

    pub fn next<S: State>(&mut self, state: &Option<S>) -> Result<(), Error> {
        if let Some(state) = state {
            if let Some(ref mut current_step) = self.current {
                current_step.work_dir_state_mut().commit()?;
                current_step.work_dir_state_mut().refresh()?;

                let work_dir_state = DirState::new_from(current_step.work_dir_state());
                let completed_step =
                    mem::replace(current_step, Step::from_states(state, work_dir_state)?);
                self.completed.push(completed_step);
            }
        } else if let Some(mut current_step) = self.current.take() {
            current_step.work_dir_state_mut().commit()?;
            current_step.work_dir_state_mut().refresh()?;

            self.completed.push(current_step);
        }

        Ok(())
    }

    pub fn read_work_dir_state(&self) -> Result<DirState, Error> {
        Ok(DirState::from_dir(
            self.current
                .as_ref()
                .or(self.completed.last())
                .unwrap()
                .work_dir_state()
                .path(),
        )?)
    }

    pub fn added_work_dir_paths(&self, cur_dir_state: &DirState) -> Result<Vec<PathBuf>, Error> {
        if let Some(step) = self.current.as_ref().or(self.completed.last()) {
            let comp = DirComparison::compare(step.work_dir_state(), &cur_dir_state);
            let mut added_paths: Vec<_> = comp
                .all_added_files()
                .map(|fs| fs.path().to_path_buf())
                .chain(comp.all_added_dirs().map(|ds| ds.path().to_path_buf()))
                .collect();
            added_paths.sort();
            Ok(added_paths)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn is_work_dir_corrupt(&self, cur_dir_state: &DirState) -> bool {
        let comp = DirComparison::compare(
            self.current
                .as_ref()
                .or(self.completed.last())
                .unwrap()
                .work_dir_state(),
            cur_dir_state,
        );
        comp.has_removed_paths() || comp.has_modified_checksums()
    }

    pub fn invalidate_state<S: State>(&mut self, id: S::Id) -> Result<(), Error> {
        for (index, step) in self.completed.iter().enumerate() {
            if id == step.id::<S>()? {
                self.completed.truncate(index + 1);
                let mut current = self.completed.remove(index);
                if let Some(prev) = self.completed.get(index) {
                    *current.work_dir_state_mut() = DirState::new_from(prev.work_dir_state());
                } else {
                    current.work_dir_state_mut().clear();
                }
                self.current.replace(current);
                break;
            }
        }

        Ok(())
    }

    pub fn restore_work_dir<S: State>(
        &self,
        mut cur_dir_state: DirState,
    ) -> Result<Option<(usize, S::Id)>, Error> {
        let completed_steps = self.completed.len();
        let mut first_invalid_state = None;

        for (step, index) in self
            .current
            .iter()
            .chain(self.completed.iter().rev())
            .zip((0..=completed_steps).rev())
        {
            let comp = DirComparison::compare(step.work_dir_state(), &cur_dir_state);
            if matches!(comp, DirComparison::Same) {
                return Ok(first_invalid_state);
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
                    cur_dir_state.untrack_file(&file.strip_prefix(cur_dir_state.path()).unwrap());
                }

                for dir in added_dirs {
                    fs::remove_dir_all(&dir)?;
                    cur_dir_state.untrack_file(&dir.strip_prefix(cur_dir_state.path()).unwrap());
                }

                cur_dir_state.commit()?;

                let comp = DirComparison::compare(step.work_dir_state(), &cur_dir_state);
                if matches!(comp, DirComparison::Same) {
                    return Ok(first_invalid_state);
                }

                comp
            } else {
                comp
            };

            if !comp.has_removed_paths() && !comp.has_modified_checksums() {
                for file in comp.all_modified_files() {
                    if let Some(old_mode) = file.old_mode() {
                        fs::set_permissions(file.path(), Permissions::from_mode(old_mode))?;
                    }
                }

                for dir in comp.all_modified_dirs() {
                    if let Some(old_mode) = dir.old_mode() {
                        fs::set_permissions(dir.path(), Permissions::from_mode(old_mode))?;
                    }
                }

                cur_dir_state.refresh_modes()?;

                let comp = DirComparison::compare(step.work_dir_state(), &cur_dir_state);
                if matches!(comp, DirComparison::Same) {
                    return Ok(first_invalid_state);
                }
            }

            first_invalid_state.replace((index, step.id::<S>()?));
        }

        for dir in cur_dir_state.dirs() {
            fs::remove_dir_all(&dir.path())?;
        }

        for file in cur_dir_state.files() {
            fs::remove_file(&file.path())?;
        }

        Ok(first_invalid_state)
    }
}
