use lazy_static::lazy_static;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub sql_server: Url,
    pub log_level: LevelFilter,
}

lazy_static! {
    static ref DEFAULT_POSTGRESQL_SERVER: Url =
        Url::parse("postgresql://postgres@localhost:5432").unwrap();
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            log_level: LevelFilter::Info,
            sql_server: (*DEFAULT_POSTGRESQL_SERVER).clone(),
        }
    }
}
