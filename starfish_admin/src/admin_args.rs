use clap::{Parser, Subcommand};
use std::path::PathBuf;
use url::Url;

const GLOBAL_OPTIONS: &str = "Global Options";

#[derive(Parser)]
#[command(version, about = "Starfish administration tool")]
pub struct AdminArgs {
    /// Address of the PostgreSQL server.  Can include a user name and
    /// password. This is read from STARFISH_SQL_SERVER, which is helpful
    /// when using `peer` authentication to save typing.
    ///
    #[arg(
        long,
        short = 'p',
        global = true,
        help_heading = GLOBAL_OPTIONS,
        env = "STARFISH_SQL_SERVER",
        default_value = "postgresql://localhost:5432/starfish",
        verbatim_doc_comment
    )]
    pub postgres_server: Url,

    /// File holding the database password, mode 0600. Use this to keep
    /// passwords out of `ps` output and shell history. This is also
    /// read from the  STARFISH_PASSWORD_FILE environment variable.
    ///
    #[arg(
        long,
        global = true,
        help_heading = GLOBAL_OPTIONS,
        env = "STARFISH_PASSWORD_FILE",
        verbatim_doc_comment
    )]
    pub password_file: Option<PathBuf>,

    /// Unix domain socket the controller listens for commands on
    #[arg(long, global = true, help_heading = GLOBAL_OPTIONS, default_value = "/run/starfishd.sock")]
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

#[derive(Subcommand)]
pub enum UserOp {
    /// Add a new user
    Add {
        alias: String,
        #[arg(long, visible_alias = "fn")]
        first_name: String,
        #[arg(long, visible_alias = "ln")]
        last_name: String,
        #[arg(long, visible_alias = "em")]
        email: String,
        /// An SSH public key, as NAME:KEY.  May be repeated.
        #[arg(long = "ssh-key", visible_alias = "sk", value_name = "NAME:KEY", value_parser = parse_ssh_key)]
        ssh_keys: Vec<(String, String)>,
    },

    /// List all users
    List {
        #[arg(long, short)]
        verbose: bool,
    },

    /// Show one user, with their keys, groups and hosts
    Show { alias: String },

    /// Change a user's details
    Update {
        alias: String,
        #[arg(long, visible_alias = "na")]
        new_alias: Option<String>,
        #[arg(long, visible_alias = "fn")]
        first_name: Option<String>,
        #[arg(long, visible_alias = "ln")]
        last_name: Option<String>,
        #[arg(long, visible_alias = "em")]
        email: Option<String>,
    },

    /// Remove a user, along with their keys and memberships
    Remove { alias: String },

    /// Add an SSH public key to a user
    AddKey {
        alias: String,
        /// A label for the key, so it can be told apart from the others
        #[arg(long)]
        name: String,
        /// The public key, in authorized_keys form
        #[arg(long)]
        key: String,
    },

    /// Remove one of a user's SSH public keys
    RemoveKey {
        alias: String,
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
        /// A Linux group to put the user in on the group's hosts.  May be
        /// repeated.  Repeating the command replaces the whole set.
        #[arg(long = "security-group", visible_alias = "sg", value_name = "NAME")]
        security_groups: Vec<String>,
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
