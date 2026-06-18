use log::LevelFilter;
use serde::Deserialize;
use std::{fs, path::PathBuf};
use toml;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct ConfigFile {
    pub nats_server: Option<Url>,
    pub nats_user: Option<String>,
    pub nats_password: Option<String>,
    pub sql_server: Option<Url>,
    pub log_level: Option<LevelFilter>,
}

impl ConfigFile {
    pub fn load(config_path: &PathBuf) -> anyhow::Result<ConfigFile> {
        let contents = fs::read_to_string(config_path)?;
        let config_file: ConfigFile = toml::from_str(&contents)?;

        Ok(config_file)
    }
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            log_level: None,
            nats_server: None,
            nats_user: None,
            nats_password: None,
            sql_server: None,
        }
    }
}
