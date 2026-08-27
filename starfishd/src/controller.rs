use crate::agent_registry::AgentRegistry;
use anyhow::Context;
use starfish_db::{Host, HostGroupUser, SecurityGroup, SshKey, User, UserSecurityGroup};
use starfish_msg::{
    AdminResponse, ControllerMsg, Group, HostConfig, SshKey as MsgSshKey, UserAccount,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use toasty::Db;

/// State shared by everything the controller runs: the agent connections, the
/// database handle, and the generation counter stamped on each configuration.
pub struct Controller {
    db: Db,
    registry: AgentRegistry,
    generation: AtomicU64,
}

impl Controller {
    pub fn new(db: Db) -> Self {
        // Seeding from the wall clock keeps generations moving forward across a
        // restart, so an agent that reconnects never sees a configuration
        // numbered below the one it already applied.
        let seed = jiff::Timestamp::now().as_second().max(0) as u64;

        Self {
            db,
            registry: AgentRegistry::new(),
            generation: AtomicU64::new(seed),
        }
    }

    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }

    /// A handle to the database. `Db` is a cheap handle onto a shared
    /// connection pool, so each task works from its own clone.
    pub fn db(&self) -> Db {
        self.db.clone()
    }

    /// Looks up the host an agent key belongs to. Returns `None` when no host
    /// has that key.
    pub async fn host_for_agent_key(&self, agent_key: &str) -> anyhow::Result<Option<Host>> {
        let mut db = self.db();

        match Host::filter_by_agent_key(agent_key)
            .first()
            .exec(&mut db)
            .await
        {
            Ok(host) => Ok(host),
            Err(err) => Err(anyhow::Error::new(err).context("Unable to look up host by agent key")),
        }
    }

    /// Assembles the configuration for `host` from the database.
    ///
    /// A host's users and groups come from the host group it belongs to: every
    /// [`SecurityGroup`] of that group is sent, along with every user in the
    /// group and, for each, the subset of those groups they are a member of.
    pub async fn host_config(&self, host: &Host) -> anyhow::Result<HostConfig> {
        let mut db = self.db();

        let security_groups = SecurityGroup::filter_by_host_group_id(host.host_group_id)
            .exec(&mut db)
            .await
            .context("Unable to read security groups")?;

        // Group names by id, so a user's memberships can be resolved without
        // going back to the database for each one.
        let group_names: HashMap<u64, &str> = security_groups
            .iter()
            .map(|group| (group.id, group.name.as_str()))
            .collect();

        let mut groups: Vec<Group> = security_groups
            .iter()
            .map(|group| Group {
                name: group.name.clone(),
            })
            .collect();

        groups.sort_by(|a, b| a.name.cmp(&b.name));

        let memberships = HostGroupUser::filter_by_host_group_id(host.host_group_id)
            .exec(&mut db)
            .await
            .context("Unable to read host group members")?;

        let user_ids: Vec<u64> = memberships.iter().map(|m| m.user_id).collect();

        if user_ids.is_empty() {
            return Ok(HostConfig {
                hostname: host.hostname.clone(),
                generation: self.next_generation(),
                groups,
                users: vec![],
            });
        }

        let is_sudoer: HashMap<u64, bool> = memberships
            .iter()
            .map(|m| (m.user_id, m.is_sudoer))
            .collect();

        let users = User::all()
            .filter(User::fields().id().in_list(user_ids.clone()))
            .exec(&mut db)
            .await
            .context("Unable to read users")?;

        let ssh_keys = SshKey::all()
            .filter(SshKey::fields().user_id().in_list(user_ids.clone()))
            .exec(&mut db)
            .await
            .context("Unable to read SSH keys")?;

        let mut keys_by_user: HashMap<u64, Vec<MsgSshKey>> = HashMap::new();

        for ssh_key in ssh_keys {
            keys_by_user
                .entry(ssh_key.user_id)
                .or_default()
                .push(MsgSshKey {
                    name: ssh_key.name,
                    key: ssh_key.key,
                });
        }

        let user_groups = UserSecurityGroup::all()
            .filter(UserSecurityGroup::fields().user_id().in_list(user_ids))
            .exec(&mut db)
            .await
            .context("Unable to read user group memberships")?;

        let mut groups_by_user: HashMap<u64, Vec<String>> = HashMap::new();

        for user_group in user_groups {
            // A user may be in groups belonging to other host groups. Only the
            // ones this host is being sent are relevant.
            let Some(name) = group_names.get(&user_group.security_group_id) else {
                continue;
            };

            groups_by_user
                .entry(user_group.user_id)
                .or_default()
                .push((*name).to_string());
        }

        let mut users: Vec<UserAccount> = users
            .into_iter()
            .map(|user| {
                let mut groups = groups_by_user.remove(&user.id).unwrap_or_default();
                let mut ssh_keys = keys_by_user.remove(&user.id).unwrap_or_default();

                groups.sort();
                ssh_keys.sort_by(|a, b| a.name.cmp(&b.name));

                UserAccount {
                    full_name: format!("{} {}", user.first_name, user.last_name),
                    is_sudoer: is_sudoer.get(&user.id).copied().unwrap_or(false),
                    name: user.alias,
                    email: user.email,
                    groups,
                    ssh_keys,
                }
            })
            .collect();

        // Sorting everything keeps a configuration byte-for-byte stable when
        // the database returns the same rows in a different order, which makes
        // logs and agent side comparisons far easier to follow.
        users.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(HostConfig {
            hostname: host.hostname.clone(),
            generation: self.next_generation(),
            groups,
            users,
        })
    }

    /// Rebuilds configurations and pushes them to connected agents.
    ///
    /// `hostname` limits the refresh to one host; `None` refreshes every host
    /// in the database. Hosts with no connected agent are reported as offline
    /// rather than treated as an error: they pick the change up when their
    /// agent next connects.
    pub async fn refresh(&self, hostname: Option<&str>) -> anyhow::Result<AdminResponse> {
        let mut db = self.db();

        let hosts = match hostname {
            Some(hostname) => {
                let host = Host::filter_by_hostname(hostname)
                    .first()
                    .exec(&mut db)
                    .await
                    .context("Unable to look up host")?;

                match host {
                    Some(host) => vec![host],
                    None => {
                        return Ok(AdminResponse::Error {
                            message: format!("No host named '{hostname}'"),
                        });
                    }
                }
            }
            None => Host::all()
                .exec(&mut db)
                .await
                .context("Unable to read hosts")?,
        };

        let mut notified = Vec::new();
        let mut offline = Vec::new();

        for host in hosts {
            if !self.registry.is_connected(host.id) {
                offline.push(host.hostname);
                continue;
            }

            let config = self.host_config(&host).await?;

            if self
                .registry
                .send(host.id, ControllerMsg::Config(config))
                .is_ok()
            {
                notified.push(host.hostname);
            } else {
                // The agent disconnected, or stopped reading, between the check
                // above and here.
                offline.push(host.hostname);
            }
        }

        notified.sort();
        offline.sort();

        Ok(AdminResponse::Refreshed { notified, offline })
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed)
    }
}
