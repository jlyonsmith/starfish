use clap::Parser;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::PathBuf;

#[derive(Parser, Serialize, Deserialize)]
#[command(version, about = "Starfish agent")]
#[skip_serializing_none]
pub struct AgentArgs {
    /// Config file path
    #[arg(
        long = "config",
        short = 'c',
        default_value = "/etc/starfish_agent.conf"
    )]
    pub config_path: PathBuf,

    /// Set the logging level (e.g., info, debug, trace)
    #[arg(long)]
    pub log_level: Option<LevelFilter>,
}
