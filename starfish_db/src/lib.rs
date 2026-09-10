// Starfish Database

mod connect;

pub use connect::{connection_url, redact, tls_warning};

// User
#[derive(Debug, toasty::Model)]
#[cfg_attr(feature = "tabled", derive(tabled::Tabled))]
#[cfg_attr(feature = "tabled", tabled(rename_all = "Upper Title Case"))]
pub struct User {
    #[key]
    #[auto]
    pub id: u64,

    #[unique]
    pub alias: String,
    #[unique]
    pub email: String,
    pub first_name: String,
    pub last_name: String,

    #[has_many]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub ssh_keys: toasty::Deferred<Vec<SshKey>>,

    #[has_many]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub host_group_users: toasty::Deferred<Vec<HostGroupUser>>,
    #[has_many(via = host_group_users.host_group)]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub host_groups: toasty::Deferred<Vec<HostGroup>>,

    #[auto]
    pub updated_at: jiff::Timestamp,
    #[auto]
    pub created_at: jiff::Timestamp,
}

// User SSH Key
#[derive(Debug, toasty::Model)]
#[cfg_attr(feature = "tabled", derive(tabled::Tabled))]
#[cfg_attr(feature = "tabled", tabled(rename_all = "Upper Title Case"))]
pub struct SshKey {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub user_id: u64,
    #[belongs_to]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub user: toasty::Deferred<User>,

    pub key: String,
    pub name: String,

    #[auto]
    pub updated_at: jiff::Timestamp,
    #[auto]
    pub created_at: jiff::Timestamp,
}

// Host Group
#[derive(Debug, toasty::Model)]
#[cfg_attr(feature = "tabled", derive(tabled::Tabled))]
#[cfg_attr(feature = "tabled", tabled(rename_all = "Upper Title Case"))]
pub struct HostGroup {
    #[key]
    #[auto]
    pub id: u64,

    #[unique]
    pub name: String,

    #[has_many]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub hosts: toasty::Deferred<Vec<Host>>,

    #[has_many]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub host_group_users: toasty::Deferred<Vec<HostGroupUser>>,
    #[has_many(via = host_group_users.user)]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub users: toasty::Deferred<Vec<User>>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

// Host
#[derive(Debug, toasty::Model)]
#[cfg_attr(feature = "tabled", derive(tabled::Tabled))]
#[cfg_attr(feature = "tabled", tabled(rename_all = "Upper Title Case"))]
pub struct Host {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub host_group_id: u64,
    #[belongs_to]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub host_group: toasty::Deferred<HostGroup>,

    #[unique]
    pub hostname: String,
    pub info: String,

    /// The 16 character alphanumeric key the agent on this host authenticates
    /// with. Assigned by the controller, see `starfish_msg::AgentKey`.
    #[unique]
    pub agent_key: String,

    /// When the last heartbeat arrived from this host's agent.
    #[cfg_attr(
        feature = "tabled",
        tabled(display("tabled::derive::display::option", ""))
    )]
    pub contacted_at: Option<jiff::Timestamp>,
    /// When the next heartbeat is due, derived from the interval the agent
    /// reported in its last heartbeat. A host past this time is unhealthy.
    #[cfg_attr(
        feature = "tabled",
        tabled(display("tabled::derive::display::option", ""))
    )]
    pub next_heartbeat_at: Option<jiff::Timestamp>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

// Host Group User - join table for HostGroup and User
#[derive(Debug, toasty::Model)]
#[cfg_attr(feature = "tabled", derive(tabled::Tabled))]
#[cfg_attr(feature = "tabled", tabled(rename_all = "Upper Title Case"))]
#[key(host_group_id, user_id)]
pub struct HostGroupUser {
    #[index]
    pub host_group_id: u64,
    #[belongs_to]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub host_group: toasty::Deferred<HostGroup>,

    #[index]
    pub user_id: u64,
    #[belongs_to]
    #[cfg_attr(feature = "tabled", tabled(skip))]
    pub user: toasty::Deferred<User>,

    pub is_sudoer: bool, // For the host group

    /// The Linux groups this user belongs to on the host group's hosts, as a
    /// `text[]`. A security group exists only by being named here, so the set
    /// a host is sent is the union of this column across the host group's
    /// members.
    #[cfg_attr(feature = "tabled", tabled(display("display_names")))]
    pub security_groups: Vec<String>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

/// Renders a list of names for `tabled`, which needs something that implements
/// `Display` and `Vec<String>` does not.
#[cfg(feature = "tabled")]
fn display_names(names: &[String]) -> String {
    names.join(", ")
}
