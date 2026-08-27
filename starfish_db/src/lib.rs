// Starfish Database

mod connect;

pub use connect::{connection_url, redact, tls_warning};

// User
#[derive(Debug, toasty::Model)]
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
    pub ssh_keys: toasty::Deferred<Vec<SshKey>>,

    #[has_many]
    pub host_group_users: toasty::Deferred<Vec<HostGroupUser>>,
    #[has_many(via = host_group_users.host_group)]
    pub host_groups: toasty::Deferred<Vec<HostGroup>>,

    #[has_many]
    pub user_security_groups: toasty::Deferred<Vec<UserSecurityGroup>>,
    #[has_many(via = user_security_groups.security_group)]
    pub security_groups: toasty::Deferred<Vec<SecurityGroup>>,

    #[auto]
    pub updated_at: jiff::Timestamp,
    #[auto]
    pub created_at: jiff::Timestamp,
}

// User SSH Key
#[derive(Debug, toasty::Model)]
pub struct SshKey {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub user_id: u64,
    #[belongs_to]
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
pub struct HostGroup {
    #[key]
    #[auto]
    pub id: u64,

    #[unique]
    pub name: String,

    #[has_many]
    pub hosts: toasty::Deferred<Vec<Host>>,

    #[has_many]
    pub security_groups: toasty::Deferred<Vec<SecurityGroup>>,

    #[has_many]
    pub host_group_users: toasty::Deferred<Vec<HostGroupUser>>,
    #[has_many(via = host_group_users.user)]
    pub users: toasty::Deferred<Vec<User>>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

// Host
#[derive(Debug, toasty::Model)]
pub struct Host {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub host_group_id: u64,
    #[belongs_to]
    pub host_group: toasty::Deferred<HostGroup>,

    #[unique]
    pub hostname: String,
    pub info: String,

    /// The 16 character alphanumeric key the agent on this host authenticates
    /// with. Assigned by the controller, see `starfish_msg::AgentKey`.
    #[unique]
    pub agent_key: String,

    /// When the last heartbeat arrived from this host's agent.
    pub contacted_at: Option<jiff::Timestamp>,
    /// When the next heartbeat is due, derived from the interval the agent
    /// reported in its last heartbeat. A host past this time is unhealthy.
    pub next_heartbeat_at: Option<jiff::Timestamp>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

// Host Group User - join table for HostGroup and User
#[derive(Debug, toasty::Model)]
#[key(host_group_id, user_id)]
pub struct HostGroupUser {
    #[index]
    pub host_group_id: u64,
    #[belongs_to]
    pub host_group: toasty::Deferred<HostGroup>,

    #[index]
    pub user_id: u64,
    #[belongs_to]
    pub user: toasty::Deferred<User>,

    pub is_admin: bool,  // For the host group
    pub is_sudoer: bool, // For the host group

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

// Security Group - a Linux group defined for every host in a host group. These
// are the groups sent to an agent; a user may belong to any subset of them.
#[derive(Debug, toasty::Model)]
#[unique(host_group_id, name)]
pub struct SecurityGroup {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub host_group_id: u64,
    #[belongs_to]
    pub host_group: toasty::Deferred<HostGroup>,

    pub name: String,

    #[has_many]
    pub user_security_groups: toasty::Deferred<Vec<UserSecurityGroup>>,
    #[has_many(via = user_security_groups.user)]
    pub users: toasty::Deferred<Vec<User>>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}

// User Security Group - join table recording which security groups a user
// belongs to.
#[derive(Debug, toasty::Model)]
#[key(user_id, security_group_id)]
pub struct UserSecurityGroup {
    #[index]
    pub user_id: u64,
    #[belongs_to]
    pub user: toasty::Deferred<User>,

    #[index]
    pub security_group_id: u64,
    #[belongs_to]
    pub security_group: toasty::Deferred<SecurityGroup>,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}
