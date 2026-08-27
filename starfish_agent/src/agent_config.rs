use serde::{Deserialize, Serialize};
use starfish_msg::AgentKey;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// How often the agent checks in when nothing says otherwise.
const DEFAULT_HEARTBEAT_SECS: u64 = 300;

/// Where the privileged helper is installed.
const DEFAULT_HELPER_PATH: &str = "/usr/local/lib/starfish/starfish-sync";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// The controller's WebSocket address, for example
    /// `ws://starfish.example.com:9600`.
    pub controller_url: Url,

    /// The key the controller assigned to this host.
    pub agent_key: AgentKey,

    /// Seconds between heartbeats. Configured in seconds rather than as a
    /// duration so it reads naturally in the TOML file.
    #[serde(default = "default_heartbeat_secs")]
    pub heartbeat_secs: u64,

    /// The privileged helper that makes the actual changes to the host. The
    /// agent itself never needs root.
    #[serde(default = "default_helper_path")]
    pub helper_path: PathBuf,

    /// Run the helper directly rather than through `sudo`, for an agent that is
    /// already root. Deployments should leave this alone and use the sudoers
    /// rule instead.
    #[serde(default)]
    pub no_sudo: bool,

    #[serde(default = "default_log_level")]
    pub log_level: log::LevelFilter,
}

impl AgentConfig {
    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs(self.heartbeat_secs)
    }

    /// The command that applies a configuration, as a program and its
    /// arguments.
    ///
    /// `sudo -n` never prompts: the agent has no terminal to prompt on, and a
    /// rule that asks for a password is a misconfiguration worth failing on
    /// rather than hanging.
    ///
    /// These are strings rather than paths because `duct` reads a path as
    /// literally that, rewriting a bare `sudo` to `./sudo`. A string is looked
    /// up on `PATH` in the usual way.
    pub fn sync_command(&self) -> (String, Vec<String>) {
        let helper = self.helper_path.to_string_lossy().to_string();

        if self.no_sudo {
            (helper, vec![])
        } else {
            ("sudo".to_string(), vec!["-n".to_string(), helper])
        }
    }
}

fn default_helper_path() -> PathBuf {
    PathBuf::from(DEFAULT_HELPER_PATH)
}

fn default_heartbeat_secs() -> u64 {
    DEFAULT_HEARTBEAT_SECS
}

fn default_log_level() -> log::LevelFilter {
    log::LevelFilter::Info
}
