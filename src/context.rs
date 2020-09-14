use std::collections::HashMap as Map;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};

use crate::prelude::*;

#[derive(Serialize, Deserialize, Default, Debug)]
struct CacheConfig {
    #[serde(flatten)]
    states: Map<String, isize>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct NodesConfig {
    #[serde(flatten)]
    nodes: Map<String, NodeConfig>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
struct NodeConfig {
    ssh_identity_name: Option<String>,
    username: Option<String>,
}

pub(super) struct AppContext {
    cached: bool,
    cache_config_file: File,
    cache_config: CacheConfig,
    nodes_config_file: File,
    nodes_config: NodesConfig,
    named_files: Map<String, NamedFile>,
}

fn flush_config(file: &mut File, config: &impl Serialize) -> AppResult<()> {
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0).context("Clearing cache config file")?;
    serde_yaml::to_writer(file, config)?;

    Ok(())
}

impl AppContext {
    pub fn configure(cached: bool) -> AppResult<Self> {
        fs::create_dir_all(HOME_DIR.join("cache")).context("Creating cache directory")?;

        let cache_config_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o644)
            .open(CACHE_DIR.join("config.yml"))
            .context("Opening cache config file")?;

        let nodes_config_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o644)
            .open(HOME_DIR.join("nodes_config.yml"))
            .context("Opening nodes config file")?;

        let cache_config: CacheConfig =
            serde_yaml::from_reader(&cache_config_file).unwrap_or_default();

        let nodes_config: NodesConfig =
            serde_yaml::from_reader(&nodes_config_file).unwrap_or_default();

        Ok(Self {
            cached,
            cache_config_file,
            cache_config,
            nodes_config_file,
            nodes_config,
            named_files: Map::new(),
        })
    }

    pub fn start_cache_writing(
        &mut self,
        key: impl AsRef<str>,
    ) -> AppResult<(isize, &mut NamedFile)> {
        // Remove the state and flush the config file, so that we know the file will be corrupt
        // if the program gets interrupted in the middle of the cache file update operation.
        let current_state = self
            .cache_config
            .states
            .remove(key.as_ref())
            .unwrap_or_default();
        self.flush_configs()
            .with_context(|| format!("Writing config file for '{}'", key.as_ref()))?;

        Ok((current_state, self.get_named_file(key.as_ref())?))
    }

    pub fn stop_cache_writing(
        &mut self,
        key: impl AsRef<str>,
        achieved_state: isize,
    ) -> AppResult<()> {
        // Change the current state to the desired state and flush the config file, so we can
        // continue work from this point if needed.
        let updated_state = self
            .cache_config
            .states
            .insert(key.as_ref().to_string(), achieved_state);

        // Only flush if the state was updated.
        if updated_state != Some(achieved_state) {
            self.flush_configs()
                .with_context(|| format!("Writing config file for '{}'", key.as_ref()))?;
        }

        Ok(())
    }

    pub fn get_named_file(&mut self, key: impl AsRef<str>) -> AppResult<&mut NamedFile> {
        if !self.named_files.contains_key(key.as_ref()) {
            let file = if self.cached {
                NamedFile::open(CACHE_DIR.join(key.as_ref()))
                    .with_context(|| format!("Opening cached file for '{}'", key.as_ref()))?
            } else {
                NamedFile::new_temp().context("Opening temporary file")?
            };
            self.named_files.insert(key.as_ref().to_owned(), file);
        }

        let named_file = self.named_files.get_mut(key.as_ref()).unwrap();
        named_file.seek(SeekFrom::Start(0))?;
        Ok(named_file)
    }

    pub fn clear_node_config(&mut self, node: &str) -> AppResult<()> {
        self.nodes_config.nodes.remove(node);
        self.flush_configs()
    }

    pub fn update_ssh_identity_name(&mut self, node: &str, identity_name: String) -> AppResult<()> {
        self.nodes_config
            .nodes
            .entry(node.to_string())
            .or_default()
            .ssh_identity_name
            .replace(identity_name);
        self.flush_configs()
    }

    pub fn update_username(&mut self, node: &str, username: String) -> AppResult<()> {
        self.nodes_config
            .nodes
            .entry(node.to_string())
            .or_default()
            .username
            .replace(username);
        self.flush_configs()
    }

    pub fn get_ssh_identity_name(&mut self, node: &str) -> Option<&str> {
        self.nodes_config
            .nodes
            .get(node)
            .and_then(|v| v.ssh_identity_name.as_ref())
            .map(String::as_str)
    }

    pub fn get_username(&mut self, node: &str) -> Option<&str> {
        self.nodes_config
            .nodes
            .get(node)
            .and_then(|v| v.username.as_ref())
            .map(String::as_str)
    }

    fn flush_configs(&mut self) -> AppResult<()> {
        flush_config(&mut self.cache_config_file, &self.cache_config)?;
        flush_config(&mut self.nodes_config_file, &self.nodes_config)?;
        Ok(())
    }
}
