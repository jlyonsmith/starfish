use clap::Parser;
use log::LevelFilter;
use std::path::PathBuf;
use url::Url;

#[derive(Parser)]
#[clap(version, about, long_about = None)]
pub struct ServerArgs {
    /// Config file path
    #[arg(long = "config", short = 'c', default_value = "/etc/starfishd.conf")]
    pub config_path: PathBuf,

    /// Address of the game NATS server
    #[arg(long)]
    pub nats_server: Option<Url>,

    /// Address of the game PostgreSQL server
    #[arg(long)]
    pub sql_server: Option<Url>,

    /// Set the logging level (e.g., info, debug, trace)
    #[arg(long)]
    pub log_level: Option<LevelFilter>,
}
