use crate::server_config::ServerConfig;
use anyhow::Context;
use async_nats::{
    ConnectOptions, ServerAddr,
    service::{Service, ServiceExt, error::Error as NatsError},
};
use sf_admin_msg as msg;
use tokio::signal;
use tokio_postgres::{NoTls, error::SqlState};
use tokio_stream::StreamExt;
use tokio_util::bytes::Bytes;

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
        log::info!("Connecting to NATS server {}", self.config.nats_server);
        log::info!("Connecting to SQL server {}", self.config.sql_server);

        let server_addr = ServerAddr::from_url(self.config.nats_server.clone())?;
        let nats_client = ConnectOptions::new()
            .name(env!("CARGO_PKG_NAME"))
            .user_and_password(
                self.config.nats_user.clone(),
                self.config.nats_password.clone(),
            )
            .connect(server_addr)
            .await
            .context(format!(
                "Unable to connect to NATS server {}",
                self.config.nats_server
            ))?;
        let (sql_client, sql_connection) =
            tokio_postgres::connect(&self.config.sql_server.to_string(), NoTls)
                .await
                .context("Unable to connect to SQL server")?;

        tokio::spawn(async move {
            if let Err(e) = sql_connection.await {
                log::debug!("Connection error: {}", e);
            }
        });

        // Create and start the service
        let service_builder: Service = nats_client
            .service_builder()
            .description("Starfish administrative service")
            .start("starfish", env!("CARGO_PKG_VERSION"))
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let group = service_builder.group("v1");
        let mut user_add = group
            .endpoint("user.add")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let mut user_remove = group
            .endpoint("user.remove")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        loop {
            tokio::select! {
                Some(request) = user_add.next() => {
                    if let Result::Ok(message) = rmp_serde::from_slice::<msg::UserAdd>(&request.message.payload) {
                        log::debug!("Add user request received: {:?}", message);

                        let result = sql_client.query_one(
                            r#"INSERT INTO users (email, alias, first_name, last_name)
                               VALUES ($1, $2, $3, $4)
                               RETURNING id;"#,
                            &[&message.email, &message.alias, &message.first_name, &message.last_name],
                        ).await;

                        match result {
                            Ok(row) => {
                                let user_id = row.get::<_, i64>(0);
                                log::info!("User '{}' added successfully with ID: {}", message.alias, user_id);
                                let response = msg::UserAddResponse {
                                    id: user_id,
                                };
                                let vec = rmp_serde::to_vec(&response);
                                if let Ok(vec) = vec {
                                    request.respond(Ok(Bytes::from(vec))).await?;
                                } else {
                                    request.respond(Err(NatsError { status: "Internal server error".into(), code: 500 })).await?;
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to add user {:?}: {:?}", message, e);
                                let svc_err = match e.as_db_error().map(|db| db.code()) {
                                    Some(c) if *c == SqlState::UNIQUE_VIOLATION =>
                                        NatsError { status: "User already exists".into(), code: 409 },
                                    _ =>
                                        NatsError { status: "Internal database error".into(), code: 500 },
                                };
                                request.respond(Err(svc_err)).await?;
                            }
                        }
                    }
                }
                Some(request) = user_remove.next() => {
                    if let Result::Ok(message) = rmp_serde::from_slice::<msg::UserRemove>(&request.message.payload) {
                        log::debug!("Delete user request received: {:?}", message);
                    }
                }
                // TODO: Use a CancellationToken to trigger this loop to exit
                // _ = &mut sql_connection => {
                //     log::error!("Database disconnected");
                //     break;
                // }
                _ = signal::ctrl_c() => {
                    log::info!("Stopping server");
                    break;
                }
            }
        }

        Ok(())
    }
}
