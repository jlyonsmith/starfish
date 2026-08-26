use crate::agent_config::AgentConfig;

pub struct Agent {
    config: AgentConfig,
}

impl Agent {
    pub fn new(config: &AgentConfig) -> Self {
        env_logger::builder().filter_level(config.log_level).init();

        log::info!(
            "{} v{} starting",
            env!("CARGO_PKG_DESCRIPTION"),
            env!("CARGO_PKG_VERSION"),
        );

        let agent = Agent {
            config: config.clone(),
        };

        agent
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        log::info!("Running agent with log level: {}", self.config.log_level);
        Ok(())
    }
}
