use clap::{Parser, Subcommand};
use url::Url;

#[derive(Parser)]
#[command(version, about = "Starfish administration tool")]
pub struct AdminArgs {
    #[command(subcommand)]
    pub entity: Entity,

    /// Address of the PostgreSQL server.  Can include a user name and password.  Defaults to `postgres://localhost:5432/starfish`.
    #[arg(
        long,
        short = 'p',
        default_value = "postgresql://localhost:5432/starfish"
    )]
    pub postgres_server: Url,
}

#[derive(Subcommand)]
pub enum Entity {
    System {
        #[command(subcommand)]
        op: SystemOp,
    },
    User {
        #[command(subcommand)]
        op: UserOp,
    },
    HostGroup {
        #[command(subcommand)]
        op: HostGroupOp,
    },
    Host {
        #[command(subcommand)]
        op: HostOp,
    },
}

fn parse_ssh_key(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.split(':').collect();

    if parts.len() != 2 {
        return Err("2 parts required: NAME:KEY".to_string());
    }

    let name = parts[0].to_string();
    let key = parts[1].to_string();

    Ok((name, key))
}

#[derive(Subcommand)]
pub enum UserOp {
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
        #[arg(long, value_name = "NAME:KEY", value_delimiter = ',', num_args = 1.., value_parser = parse_ssh_key)]
        ssh_keys: Option<Vec<(String, String)>>,
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
        alias: String,
        #[arg(long)]
        ssh_keys: Option<Vec<String>>,
    },
}

#[derive(Subcommand)]
pub enum HostGroupOp {
    Add {
        #[arg(long)]
        name: String,
    },
    Remove {},
    Update {},
    List {},
}

#[derive(Subcommand)]
pub enum HostOp {
    Add {
        #[arg(long)]
        name: String,
    },
    Remove {},
    Update {},
    List {},
}

#[derive(Subcommand)]
pub enum SystemOp {
    CreateDatabase,
}
