use crate::config_file::ConfigFile;
use crate::server_args::ServerArgs;
use lazy_static::lazy_static;
use log::LevelFilter;
use url::Url;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub log_level: LevelFilter,
    pub nats_server: Url,
    pub sql_server: Url,
}

lazy_static! {
    static ref DEFAULT_NATS_SERVER: Url = Url::parse("nats://localhost:4222").unwrap();
    static ref DEFAULT_POSTGRESQL_SERVER: Url =
        Url::parse("postgresql://postgres@localhost:5432").unwrap();
}

impl ServerConfig {
    pub fn with_config_and_args(config_file: &ConfigFile, args: &ServerArgs) -> ServerConfig {
        ServerConfig {
            log_level: args
                .log_level
                .or(config_file.log_level)
                .unwrap_or(LevelFilter::Info),
            nats_server: config_file
                .nats_server
                .clone()
                .unwrap_or((*DEFAULT_NATS_SERVER).clone())
                .clone(),
            sql_server: config_file
                .sql_server
                .clone()
                .unwrap_or((*DEFAULT_POSTGRESQL_SERVER).clone())
                .clone(),
        }
    }
}
