use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use url::Url;

#[derive(Parser)]
#[command(version, about = "Starfish administration tool")]
pub struct AdminArgs {
    /// Address of the PostgreSQL server.  Can include a user name and password.
    /// Also read from STARFISH_SQL_SERVER, which saves an administrator
    /// retyping the socket URL that `peer` authentication needs.
    #[arg(
        long,
        short = 'p',
        global = true,
        env = "STARFISH_SQL_SERVER",
        default_value = "postgresql://localhost:5432/starfish"
    )]
    pub postgres_server: Url,

    /// File holding the database password, mode 0600.  Keeps it out of `ps`
    /// output and shell history.  Also read from STARFISH_PASSWORD_FILE.
    #[arg(long, global = true, env = "STARFISH_PASSWORD_FILE")]
    pub password_file: Option<PathBuf>,

    /// Unix domain socket the controller listens for commands on
    #[arg(long, global = true, default_value = "/run/starfishd.sock")]
    pub socket: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create the database schema
    InitDb,

    /// Manage users
    User {
        #[command(subcommand)]
        op: UserOp,
    },

    /// Manage host groups, which tie users to hosts
    HostGroup {
        #[command(subcommand)]
        op: HostGroupOp,
    },

    /// Manage the Linux groups a host group's users can belong to
    SecurityGroup {
        #[command(subcommand)]
        op: SecurityGroupOp,
    },

    /// Manage hosts
    Host {
        #[command(subcommand)]
        op: HostOp,
    },

    /// Tell the controller to push configuration to its agents now
    Refresh {
        /// Refresh one host.  Every host is refreshed when this is omitted.
        #[arg(long)]
        hostname: Option<String>,
    },
}

/// Parses a `NAME:KEY` pair, splitting on the first colon so the key itself may
/// contain one.
fn parse_ssh_key(s: &str) -> Result<(String, String), String> {
    let Some((name, key)) = s.split_once(':') else {
        return Err("expected NAME:KEY".to_string());
    };

    if name.is_empty() || key.is_empty() {
        return Err("expected NAME:KEY, with both parts filled in".to_string());
    }

    Ok((name.to_string(), key.to_string()))
}

#[derive(Args)]
pub struct UserRef {
    /// The user's login name
    #[arg(long, short)]
    pub alias: String,
}

#[derive(Subcommand)]
pub enum UserOp {
    /// Add a new user
    Add {
        #[arg(long, short)]
        alias: String,
        #[arg(long)]
        first_name: String,
        #[arg(long)]
        last_name: String,
        #[arg(long)]
        email: String,
        /// An SSH public key, as NAME:KEY.  May be repeated.
        #[arg(long = "ssh-key", value_name = "NAME:KEY", value_parser = parse_ssh_key)]
        ssh_keys: Vec<(String, String)>,
    },

    /// List all users
    List {
        #[arg(long, short)]
        verbose: bool,
    },

    /// Show one user, with their keys, groups and hosts
    Show {
        #[command(flatten)]
        user: UserRef,
    },

    /// Change a user's details
    Update {
        #[command(flatten)]
        user: UserRef,
        #[arg(long)]
        first_name: Option<String>,
        #[arg(long)]
        last_name: Option<String>,
        #[arg(long)]
        email: Option<String>,
    },

    /// Remove a user, along with their keys and memberships
    Remove {
        #[command(flatten)]
        user: UserRef,
    },

    /// Add an SSH public key to a user
    AddKey {
        #[command(flatten)]
        user: UserRef,
        /// A label for the key, so it can be told apart from the others
        #[arg(long)]
        name: String,
        /// The public key, in authorized_keys form
        #[arg(long)]
        key: String,
    },

    /// Remove one of a user's SSH public keys
    RemoveKey {
        #[command(flatten)]
        user: UserRef,
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum HostGroupOp {
    /// Add a new host group
    Add {
        #[arg(long, short)]
        name: String,
    },

    /// List all host groups
    List {
        #[arg(long, short)]
        verbose: bool,
    },

    /// Remove an empty host group
    Remove {
        #[arg(long, short)]
        name: String,
    },

    /// Give a user accounts on every host in the group
    AddUser {
        #[arg(long, short)]
        name: String,
        #[arg(long, short)]
        alias: String,
        /// Grant the user sudo on the group's hosts
        #[arg(long)]
        sudoer: bool,
        /// Mark the user as an administrator of the group
        #[arg(long)]
        admin: bool,
    },

    /// Take a user's accounts on the group's hosts away
    RemoveUser {
        #[arg(long, short)]
        name: String,
        #[arg(long, short)]
        alias: String,
    },
}

#[derive(Subcommand)]
pub enum SecurityGroupOp {
    /// Add a Linux group to a host group
    Add {
        #[arg(long)]
        host_group: String,
        #[arg(long, short)]
        name: String,
    },

    /// List security groups
    List {
        /// Limit the listing to one host group
        #[arg(long)]
        host_group: Option<String>,
    },

    /// Remove a security group and everyone's membership of it
    Remove {
        #[arg(long)]
        host_group: String,
        #[arg(long, short)]
        name: String,
    },

    /// Put a user in a security group
    AddUser {
        #[arg(long)]
        host_group: String,
        #[arg(long, short)]
        name: String,
        #[arg(long, short)]
        alias: String,
    },

    /// Take a user out of a security group
    RemoveUser {
        #[arg(long)]
        host_group: String,
        #[arg(long, short)]
        name: String,
        #[arg(long, short)]
        alias: String,
    },
}

#[derive(Subcommand)]
pub enum HostOp {
    /// Add a host and generate the key its agent authenticates with
    Add {
        #[arg(long)]
        hostname: String,
        #[arg(long)]
        host_group: String,
        /// Free text note about the host
        #[arg(long, default_value = "")]
        info: String,
    },

    /// List all hosts
    List {
        #[arg(long, short)]
        verbose: bool,
    },

    /// Show one host, including its agent key
    Show {
        #[arg(long)]
        hostname: String,
    },

    /// Change a host's details
    Update {
        #[arg(long)]
        hostname: String,
        #[arg(long)]
        info: Option<String>,
        #[arg(long)]
        host_group: Option<String>,
    },

    /// Remove a host
    Remove {
        #[arg(long)]
        hostname: String,
    },

    /// Replace a host's agent key
    Rekey {
        #[arg(long)]
        hostname: String,
    },
}
