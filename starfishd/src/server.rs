use crate::server_config::ServerConfig;
use anyhow::Context;
use async_nats::{
    ConnectOptions, ServerAddr,
    service::{Request, Service, ServiceExt, error::Error as NatsError},
};
use sf_admin_msg as msg;
use tokio::signal;
use tokio_postgres::{NoTls, Row};
use tokio_stream::StreamExt;
use tokio_util::{bytes::Bytes, sync::CancellationToken};

/// Builds the `SET` clause of an `UPDATE` from `(column, value)` pairs,
/// returning the `column = $n` fragment along with the matching bind parameters
/// in order. Placeholders are numbered starting at `start_index` so the caller
/// can append further parameters (e.g. a `WHERE` key).
fn create_set_clause<'a>(
    fields: &'a [(&str, &str)],
    start_index: usize,
) -> (String, Vec<&'a (dyn tokio_postgres::types::ToSql + Sync)>) {
    let mut clauses = Vec::new();
    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();

    for (index, (column, value)) in fields.iter().enumerate() {
        params.push(value);
        clauses.push(format!("{} = ${}", column, start_index + index));
    }

    (clauses.join(", "), params)
}

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
        let username = self.config.nats_server.username().to_string();
        let password = self.config.nats_server.password().map(str::to_string);
        let mut nats_server = self.config.nats_server.clone();
        nats_server.set_username("").ok();
        nats_server.set_password(None).ok();

        log::info!("Connecting to NATS server {}", nats_server);
        log::info!("Connecting to SQL server {}", self.config.sql_server);

        let server_addr = ServerAddr::from_url(nats_server.clone())?;
        let mut connect_options = ConnectOptions::new().name(env!("CARGO_PKG_NAME"));
        if !username.is_empty() {
            connect_options =
                connect_options.user_and_password(username, password.unwrap_or_default());
        }
        let nats_client = connect_options
            .connect(server_addr)
            .await
            .context(format!("Unable to connect to NATS server {}", nats_server))?;
        let (sql_client, sql_connection) =
            tokio_postgres::connect(&self.config.sql_server.to_string(), NoTls)
                .await
                .context("Unable to connect to SQL server")?;

        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            if let Err(e) = sql_connection.await {
                log::error!("Database disconnected: {}", e);
                cancel_token_clone.cancel();
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
        let mut user_update = group
            .endpoint("user.update")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let mut user_remove = group
            .endpoint("user.remove")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        async fn handle_query_one_result(
            request: &Request,
            result: Result<Row, tokio_postgres::Error>,
        ) -> anyhow::Result<()> {
            match result {
                Ok(row) => {
                    let id = row.get::<_, i64>(0);
                    log::info!("Add/update/delete successful with ID: {}", id);
                    let response = msg::UserAddResponse { id };
                    let vec = rmp_serde::to_vec(&response);
                    if let Ok(vec) = vec {
                        request.respond(Ok(Bytes::from(vec))).await?;
                    } else {
                        request
                            .respond(Err(NatsError {
                                status: "Internal server error".into(),
                                code: 500,
                            }))
                            .await?;
                    }
                }
                Err(e) => {
                    log::error!("Failed to add/update/delete: {:?}", e);

                    let svc_err = match e.as_db_error().map(|db| db.code()) {
                        Some(c) => NatsError {
                            status: format!("Database error {}", c.code()),
                            code: 400,
                        },
                        _ => NatsError {
                            status: "Internal database error".into(),
                            code: 500,
                        },
                    };
                    request.respond(Err(svc_err)).await?;
                }
            }

            Ok(())
        }

        async fn handle_no_fields(
            request: &Request,
            fields: &[(&str, &str)],
        ) -> anyhow::Result<()> {
            if fields.is_empty() {
                request
                    .respond(Err(NatsError {
                        status: "No fields to update".into(),
                        code: 400,
                    }))
                    .await?;
            }
            Ok(())
        }

        loop {
            tokio::select! {
                Some(request) = user_add.next() => {
                    if let Result::Ok(message) = rmp_serde::from_slice::<msg::UserAdd>(&request.message.payload) {
                        let result = sql_client
                            .query_one(
                                r#"INSERT INTO users (email, alias, first_name, last_name)
                               VALUES ($1, $2, $3, $4)
                               RETURNING id;"#,
                                &[
                                    &message.email,
                                    &message.alias,
                                    &message.first_name,
                                    &message.last_name,
                                ],
                            )
                            .await;

                        handle_query_one_result(&request, result).await?;
                    }
                }
                Some(request) = user_update.next() => {
                    if let Result::Ok(message) = rmp_serde::from_slice::<msg::UserUpdate>(&request.message.payload) {
                        // Collect the fields that were actually provided.
                        let mut fields: Vec<(&str, &str)> = Vec::new();
                        if let Some(email) = &message.email {
                            fields.push(("email", email));
                        }
                        if let Some(first_name) = &message.first_name {
                            fields.push(("first_name", first_name));
                        }
                        if let Some(last_name) = &message.last_name {
                            fields.push(("last_name", last_name));
                        }
                        handle_no_fields(&request, &fields).await?;

                        // `alias` is the lookup key ($1), so the updatable fields start at $2.
                        let (set_clause, mut params) = create_set_clause(&fields, 2);
                        params.insert(0, &message.alias);
                        let sql = format!("UPDATE users SET {} WHERE alias = $1 RETURNING id;", set_clause);
                        let result = sql_client.query_one(&sql, &params).await;

                        handle_query_one_result(&request, result).await?;
                    }
                }
                Some(request) = user_remove.next() => {
                    if let Result::Ok(message) = rmp_serde::from_slice::<msg::UserRemove>(&request.message.payload) {
                        let result = sql_client
                            .query_one(
                                r#"DELETE FROM users WHERE alias = $1 RETURNING id;"#,
                                &[&message.alias],
                            )
                            .await;

                        handle_query_one_result(&request, result).await?;
                    }
                }
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
