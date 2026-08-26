// Starfish Database

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

    pub name: String,

    #[has_many]
    pub hosts: toasty::Deferred<Vec<Host>>,

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

    pub hostname: String,
    pub info: String,
    pub contacted_at: Option<jiff::Timestamp>,

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

// Host Group User Security Group
#[derive(Debug, toasty::Model)]
pub struct HostGroupUserSecurityGroup {
    #[key]
    #[auto]
    pub id: u64,

    #[index]
    pub host_group_id: u64,
    #[belongs_to]
    pub host_group: toasty::Deferred<HostGroup>,
    pub sec_group: String,

    #[auto]
    pub created_at: jiff::Timestamp,
    #[auto]
    pub updated_at: jiff::Timestamp,
}
