//! Messages exchanged between the controller and an agent.

use crate::AgentKey;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A message sent by an agent to the controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMsg {
    /// The first message on a connection. The controller replies with
    /// [`ControllerMsg::Config`] once the key is recognised.
    Hello(Hello),

    /// Reports that the agent is still alive.
    Heartbeat(Heartbeat),

    /// Reports the outcome of applying a [`HostConfig`].
    SyncReport(SyncReport),

    /// Asks the controller to send the current [`HostConfig`] again.
    ConfigRequest,
}

/// A message sent by the controller to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControllerMsg {
    /// The users and groups this host should have. Sent when an agent
    /// connects, and again whenever the host's configuration changes or an
    /// administrator forces a refresh.
    Config(HostConfig),

    /// Acknowledges a [`Heartbeat`].
    HeartbeatAck,

    /// The agent's last message could not be handled.
    Error(ProtocolError),
}

/// Identifies and authenticates an agent to the controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// The agent's [`crate::PROTOCOL_VERSION`].
    pub protocol_version: u32,

    /// The key the controller assigned to this host.
    pub agent_key: AgentKey,

    /// The host's own name, for diagnostics. The controller identifies the
    /// host by `agent_key`, not by this.
    pub hostname: String,

    /// The `starfish_agent` package version.
    pub agent_version: String,
}

/// Tells the controller the agent is alive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    /// How long until the agent intends to send its next heartbeat. The
    /// controller treats the host as unhealthy if nothing arrives within it.
    pub next_in: Duration,
}

/// The users and groups an agent should synchronize onto its host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostConfig {
    /// The name the controller knows this host by.
    pub hostname: String,

    /// Identifies this push. It increases with every configuration the
    /// controller sends, so a [`SyncReport`] can say which push it describes
    /// and an agent can ignore a configuration older than one it has already
    /// applied. It is not a content hash: an unchanged configuration sent
    /// twice carries two different generations.
    pub generation: u64,

    /// Every group this host's users are placed in. A user's `groups` may
    /// only name groups from this list. Starfish does not create them; they
    /// are expected to exist on the host already.
    pub groups: Vec<Group>,

    /// Every user the host should have.
    pub users: Vec<UserAccount>,
}

/// A group the host is expected to have.
///
/// Groups are administered outside Starfish: the agent never creates or
/// deletes one, it only puts users in and takes them out. A group named here
/// that the host does not have is reported as [`Status::Missing`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
}

/// A user the agent should create on the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAccount {
    /// The database's identifier for this user, and the only part of an
    /// account that never changes.
    ///
    /// The agent writes it into the host's `GECOS` field so that an account
    /// can still be recognised after its login name has been changed, and so
    /// that an account belonging to a user who has gone away can be told from
    /// one Starfish never created. A login name cannot do either job: it is
    /// exactly the thing that changes.
    pub id: u64,

    /// The login name, which is the user's alias in the database. Unlike
    /// [`UserAccount::id`] this may change, and the agent renames the host's
    /// account to follow it.
    pub name: String,

    /// The user's full name, used for the GECOS field.
    pub full_name: String,

    pub email: String,

    /// Whether the user gets `sudo` on this host.
    pub is_sudoer: bool,

    /// Names of the groups in [`HostConfig::groups`] this user belongs to.
    pub groups: Vec<String>,

    /// The keys to install in the user's `authorized_keys`.
    pub ssh_keys: Vec<SshKey>,
}

/// An SSH public key authorized for a user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKey {
    /// A label for the key, so an administrator can tell keys apart.
    pub name: String,

    /// The public key, in `authorized_keys` form.
    pub key: String,
}

/// What an agent made of a [`HostConfig`]: one entry per group and per user it
/// was asked to synchronize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub hostname: String,

    /// The [`HostConfig::generation`] this report describes.
    pub generation: u64,

    pub groups: Vec<ItemReport>,
    pub users: Vec<ItemReport>,
}

/// The outcome for a single group or user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemReport {
    /// The group or user name from the configuration.
    pub name: String,

    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// The host already matched the configuration.
    Unchanged,

    /// The user did not exist and was added. Groups are never created, so this
    /// never appears for one.
    Created,

    /// The account existed on the host, carried Starfish's `GECOS` tag, and
    /// belonged to a user the configuration no longer has, so it was deleted
    /// along with its home directory. Users only: a group is never removed.
    ///
    /// The name is the login the account had on the host, which is not
    /// necessarily one the configuration ever mentioned.
    Removed,

    /// The group or user existed but had to be changed.
    Updated,

    /// The group does not exist on the host, so nobody could be put in it.
    ///
    /// Not a failure: groups are administered outside Starfish, and the host
    /// is free not to have one. It is worth an administrator's attention
    /// though, because everybody the configuration puts in that group is
    /// going without the access it was meant to give them.
    Missing,

    /// The update failed and the host does not match the configuration.
    Failed { message: String },
}

impl Status {
    pub fn is_failure(&self) -> bool {
        matches!(self, Status::Failed { .. })
    }
}

/// Why the controller rejected an agent's message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub kind: ProtocolErrorKind,

    /// A human readable explanation, for the agent's log.
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolErrorKind {
    /// No host in the database has the key the agent sent.
    UnknownAgentKey,

    /// The agent's protocol version is not one the controller speaks.
    UnsupportedVersion,

    /// The agent sent something other than [`AgentMsg::Hello`] first.
    NotAuthenticated,

    /// The message could not be decoded.
    Malformed,

    /// The controller failed to handle an otherwise valid message.
    Internal,
}
