use clap::Parser;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use starfish_msg::AgentKey;
use std::path::PathBuf;
use url::Url;

#[skip_serializing_none]
#[derive(Parser, Serialize, Deserialize)]
#[command(version, about = "Starfish agent")]
pub struct AgentArgs {
    /// Config file path
    #[arg(
        long = "config",
        short = 'c',
        default_value = "/etc/starfish_agent.conf"
    )]
    #[serde(skip)]
    pub config_path: PathBuf,

    /// WebSocket address of the controller, e.g. ws://starfish:9600
    #[arg(long)]
    pub controller_url: Option<Url>,

    /// The 16 character key the controller assigned to this host
    #[arg(long)]
    pub agent_key: Option<AgentKey>,

    /// Seconds between heartbeats
    #[arg(long)]
    pub heartbeat_secs: Option<u64>,

    /// Path to the privileged starfish-sync helper
    #[arg(long)]
    pub helper_path: Option<PathBuf>,

    /// Run the helper directly instead of through sudo
    #[arg(long)]
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_sudo: bool,

    /// Set the logging level (e.g., info, debug, trace)
    #[arg(long)]
    pub log_level: Option<LevelFilter>,
}
