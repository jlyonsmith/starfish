use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub log_level: log::LevelFilter,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            log_level: log::LevelFilter::Info,
        }
    }
}
