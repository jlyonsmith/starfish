use clap::{Parser, Subcommand};
use std::path::PathBuf;
use url::Url;

const GLOBAL_OPTIONS: &str = "Global Options";

#[derive(Parser)]
#[command(version, about = "Starfish administration tool")]
pub struct AdminArgs {
    /// URL of the PostgreSQL server
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

    /// File holding the database password
    ///
    #[arg(
        long,
        global = true,
        help_heading = GLOBAL_OPTIONS,
        env = "STARFISH_PASSWORD_FILE",
        verbatim_doc_comment
    )]
    pub password_file: Option<PathBuf>,

    /// Controller's Unix domain socket
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
        #[arg(value_name = "USER_ALIAS")]
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
    List {},

    /// Show one user, with their keys, groups and hosts
    Show {
        #[arg(value_name = "USER_ALIAS")]
        alias: String,
    },

    /// Change a user's details
    Update {
        #[arg(value_name = "USER_ALIAS")]
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
    Remove {
        #[arg(value_name = "USER_ALIAS")]
        alias: String,
    },

    /// Add an SSH public key to a user
    AddKey {
        #[arg(value_name = "USER_ALIAS")]
        alias: String,
        /// A label for the key, so it can be told apart from the others
        #[arg(long, value_name = "KEY_NAME")]
        name: String,
        /// The public key, in authorized_keys form
        #[arg(long, value_name = "KEY_VALUE")]
        key: String,
    },

    /// Remove one of a user's SSH public keys
    RemoveKey {
        #[arg(value_name = "USER_ALIAS")]
        alias: String,
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum HostGroupOp {
    /// Add a new host group
    Add {
        #[arg(value_name = "HOST_GROUP_NAME")]
        name: String,
    },

    /// List all host groups
    List {},

    /// Show information about a host group
    Show {
        #[arg(value_name = "HOST_GROUP_NAME")]
        name: String,
    },

    /// Remove an empty host group
    Remove {
        #[arg(value_name = "HOST_GROUP_NAME")]
        name: String,
    },

    /// Give a user accounts on every host in the group
    AddUser {
        #[arg(value_name = "HOST_GROUP_NAME")]
        name: String,
        #[arg(long, short)]
        alias: String,
        /// Grant the user sudo on the group's hosts
        #[arg(long)]
        sudoer: bool,
        /// One or more Linux groups to put the user in on the group's hosts.  May be
        /// repeated or separated by commas.
        #[arg(
            long = "security-group",
            visible_alias = "sg",
            value_name = "SECURITY_GROUPS"
        )]
        security_groups: Vec<String>,
    },

    /// Update a user's accounts on every host in the group
    UpdateUser {
        #[arg(value_name = "HOST_GROUP_NAME")]
        name: String,
        #[arg(long, short, value_name = "USER_ALIAS")]
        alias: String,
        /// Grant the user sudo on the group's hosts
        #[arg(long)]
        sudoer: bool,
        /// One or more Linux groups to put the user in on the group's hosts.  May be
        /// repeated or separated by commas.
        #[arg(
            long = "security-group",
            visible_alias = "sg",
            value_name = "SECURITY_GROUPS"
        )]
        security_groups: Vec<String>,
    },

    /// Take a user's accounts on the group's hosts away
    RemoveUser {
        #[arg(value_name = "HOST_GROUP_NAME")]
        name: String,
        #[arg(long, short, value_name = "USER_ALIAS")]
        alias: String,
    },
}

#[derive(Subcommand)]
pub enum HostOp {
    /// Add a host and generate the key its agent authenticates with
    Add {
        #[arg(value_name = "HOSTNAME")]
        hostname: String,
        #[arg(long, visible_alias = "hg", value_name = "GROUP")]
        host_group: String,
        /// Free text note about the host
        #[arg(long, visible_alias = "in", default_value = "")]
        info: String,
    },

    /// List all hosts
    List {},

    /// Show one host, including its agent key
    Show { hostname: String },

    /// Change a host's details
    Update {
        #[arg(value_name = "HOSTNAME")]
        hostname: String,
        #[arg(long)]
        info: Option<String>,
        #[arg(long)]
        host_group: Option<String>,
    },

    /// Remove a host
    Remove {
        #[arg(value_name = "HOSTNAME")]
        hostname: String,
    },

    /// Replace a host's agent key
    Rekey {
        #[arg(value_name = "HOSTNAME")]
        hostname: String,
    },
}
