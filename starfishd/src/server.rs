use crate::server_config::ServerConfig;
use tokio::signal;
use tokio_util::sync::CancellationToken;

pub struct Server {
    config: ServerConfig,
}

impl Server {
    pub fn new(config: &ServerConfig) -> Self {
        env_logger::builder().filter_level(config.log_level).init();

        log::info!(
            "{} v{} starting",
            env!("CARGO_PKG_DESCRIPTION"),
            env!("CARGO_PKG_VERSION"),
        );

        let server = Server {
            config: config.clone(),
        };

        server
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        // Extract any credentials from the URL and connect with a credential-free URL.
        let cancel_token = CancellationToken::new();

        log::info!("Connecting to SQL server {}", self.config.sql_server);

        let db = toasty::Db::builder()
            .models(toasty::models!(starfish_db::*))
            .connect(&self.config.sql_server.to_string())
            .await?;

        db.push_schema().await?;

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    break;
                }
                _ = signal::ctrl_c() => {
                    log::info!("Stopping server");
                    break;
                }
            }
        }

        Ok(())
    }
}
