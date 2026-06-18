use anyhow::{Context, bail};
use async_nats::{ConnectOptions, ServerAddr};
use clap::{Parser, Subcommand};
use sf_admin_msg as msg;
use tokio_util::bytes::Bytes;
use url::Url;

#[derive(Parser)]
#[command(version, about = "Starfish administration tool")]
struct AdminArgs {
    #[command(subcommand)]
    entity: Entity,

    /// Address of the game NATS server
    #[arg(long, default_value = "nats://localhost:4222")]
    pub nats_server: Url,

    /// NATS user
    #[arg(long)]
    pub nats_user: Option<String>,

    /// NATS user password
    #[arg(long)]
    pub nats_password: Option<String>,
}

#[derive(Subcommand)]
enum Entity {
    User {
        #[command(subcommand)]
        op: UserOp,
    },
    Host,
    HostGroup,
}

#[derive(Subcommand)]
enum UserOp {
    /// Add a new user
    Add {
        #[arg(long)]
        alias: String,
        #[arg(long)]
        first_name: String,
        #[arg(long)]
        last_name: String,
        #[arg(long)]
        email: String,
    },
    /// Remove an existing user
    Remove {
        #[arg(short, long)]
        alias: String,
    },
    /// List all users
    List {
        #[arg(short, long)]
        verbose: bool,
    },
    /// Update a user's information
    Update {
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        sudoer: Option<bool>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match AdminArgs::try_parse() {
        Ok(m) => m,
        Err(err) => {
            // Help and version come back as an error
            eprintln!("{}", err.to_string());
            return Ok(());
        }
    };

    let server_addr = ServerAddr::from_url(args.nats_server.clone())?;
    let mut nats_options = ConnectOptions::new().name(env!("CARGO_PKG_NAME"));

    if let (Some(user), Some(password)) = (args.nats_user, args.nats_password) {
        // Both are present — use user_str and password_str here
        nats_options = nats_options.user_and_password(user, password);
    }

    let nats_client = nats_options.connect(server_addr).await.context(format!(
        "Unable to connect to NATS server {}",
        args.nats_server
    ))?;

    match &args.entity {
        Entity::User { op } => match op {
            UserOp::Add {
                alias,
                first_name,
                last_name,
                email,
            } => {
                let payload = msg::UserAdd {
                    alias: alias.clone(),
                    first_name: first_name.clone(),
                    last_name: last_name.clone(),
                    email: email.clone(),
                };
                use async_nats::service::{NATS_SERVICE_ERROR, NATS_SERVICE_ERROR_CODE};

                let reply = nats_client
                    .request("v1.user.add", Bytes::from(rmp_serde::to_vec(&payload)?))
                    .await?;

                if let Some(headers) = &reply.headers {
                    if let Some(err) = headers.get(NATS_SERVICE_ERROR) {
                        let code = headers
                            .get(NATS_SERVICE_ERROR_CODE)
                            .map(|c| c.as_str())
                            .unwrap_or("?");
                        bail!("Failed to add user '{}': {} ({})", alias, err, code);
                    }
                }
                let response: msg::UserAddResponse = rmp_serde::from_slice(&reply.payload)?;
                println!("User '{}' added successfully (id {})", alias, response.id);
            }
            UserOp::Remove { .. } => {}
            UserOp::List { .. } => {}
            UserOp::Update { .. } => {}
        },
        Entity::Host => {}
        Entity::HostGroup => {}
    }

    Ok(())
}
