//! Bringing the host in line with the configuration the controller sent.
//!
//! Synchronizing is additive by design: users and groups that are missing get
//! created, but nothing the controller did not mention is ever removed. The one
//! thing that is taken away is membership of a group the controller *did* send,
//! because that is the only way to revoke access.

use crate::system::{LEGACY_SUDO_GROUP, SUDO_GROUP, System};
use crate::validate;
use starfish_msg::{HostConfig, ItemReport, Status, SyncReport, UserAccount};
use std::collections::BTreeSet;

/// Written at the top of every `authorized_keys` the agent manages.
const AUTHORIZED_KEYS_HEADER: &str =
    "# Managed by starfish. Local changes are overwritten on the next sync.\n";

/// Applies `config` to the host and reports what happened to each group and
/// user.
///
/// One failure does not stop the rest: every item is attempted and reported on
/// its own, so a single broken account cannot hold up everybody else's access.
pub fn sync(system: &dyn System, config: &HostConfig) -> SyncReport {
    // The sudo group is created here rather than being sent by the controller,
    // which knows nothing about it: it is not a security group anybody
    // configures, but it has to exist before a sudoer can be put in it.  Only
    // when somebody actually needs it, so a host with no sudoers does not grow
    // a group it will never use.
    let mut names: Vec<String> = config.groups.iter().map(|g| g.name.clone()).collect();

    if config.users.iter().any(|user| user.is_sudoer) && !names.iter().any(|n| n == SUDO_GROUP) {
        names.push(SUDO_GROUP.to_string());
    }

    let groups = names
        .into_iter()
        .map(|name| ItemReport {
            status: into_status(sync_group(system, &name)),
            name,
        })
        .collect();

    // Membership is only ever added or removed within this set, so groups the
    // controller knows nothing about are left alone. `sudo` is in it because
    // that is how a user's sudo status is granted and revoked.
    let mut managed: BTreeSet<&str> = config
        .groups
        .iter()
        .map(|group| group.name.as_str())
        .collect();

    managed.insert(SUDO_GROUP);

    // Only ever removed, never added: this is how sudo was granted before
    // starfish-sudo, and an upgraded host has users sitting in it. Managing it
    // is what moves them off; leaving it out would stand a grant that no later
    // revocation could reach.
    managed.insert(LEGACY_SUDO_GROUP);

    let users = config
        .users
        .iter()
        .map(|user| ItemReport {
            name: user.name.clone(),
            status: into_status(sync_user(system, user, &managed)),
        })
        .collect();

    SyncReport {
        hostname: config.hostname.clone(),
        generation: config.generation,
        groups,
        users,
    }
}

fn sync_group(system: &dyn System, group: &str) -> anyhow::Result<Status> {
    validate::name("Group", group)?;

    if system.group_exists(group)? {
        return Ok(Status::Unchanged);
    }

    system.create_group(group)?;

    Ok(Status::Created)
}

fn sync_user(
    system: &dyn System,
    user: &UserAccount,
    managed: &BTreeSet<&str>,
) -> anyhow::Result<Status> {
    validate::name("User", &user.name)?;
    validate::full_name(&user.full_name)?;

    for group in &user.groups {
        validate::name("Group", group)?;
    }

    let created = match system.user_id(&user.name)? {
        Some(uid) => {
            // The name exists already. If it belongs to the distribution then
            // this configuration is asking for something it should not have,
            // and the account is left exactly as it is.
            validate::managed_uid(&user.name, uid)?;
            false
        }
        None => {
            system.create_user(&user.name, &user.full_name)?;
            true
        }
    };

    let mut changed = created;

    // A freshly created user already has the right name, so there is nothing
    // to compare against.
    if !created && system.user_full_name(&user.name)? != user.full_name {
        system.set_user_full_name(&user.name, &user.full_name)?;
        changed = true;
    }

    changed |= sync_groups(system, user, managed)?;
    changed |= sync_authorized_keys(system, user)?;

    Ok(if created {
        Status::Created
    } else if changed {
        Status::Updated
    } else {
        Status::Unchanged
    })
}

/// Adds the user to the groups they should be in and removes them from managed
/// groups they should not. Returns whether anything changed.
fn sync_groups(
    system: &dyn System,
    user: &UserAccount,
    managed: &BTreeSet<&str>,
) -> anyhow::Result<bool> {
    let mut wanted: BTreeSet<&str> = user.groups.iter().map(String::as_str).collect();

    if user.is_sudoer {
        wanted.insert(SUDO_GROUP);
    }

    let current: BTreeSet<String> = system.user_groups(&user.name)?.into_iter().collect();
    let mut changed = false;

    for group in &wanted {
        if !current.contains(*group) {
            system.add_to_group(&user.name, group)?;
            changed = true;
        }
    }

    for group in managed {
        if current.contains(*group) && !wanted.contains(group) {
            system.remove_from_group(&user.name, group)?;
            changed = true;
        }
    }

    Ok(changed)
}

/// Replaces the user's `authorized_keys` when it does not already match the
/// keys the controller sent. Returns whether anything changed.
fn sync_authorized_keys(system: &dyn System, user: &UserAccount) -> anyhow::Result<bool> {
    let wanted = authorized_keys(user);

    if system.authorized_keys(&user.name)?.as_deref() == Some(wanted.as_str()) {
        return Ok(false);
    }

    system.set_authorized_keys(&user.name, &wanted)?;

    Ok(true)
}

/// The `authorized_keys` file a user should have.
///
/// The agent owns the whole file, so a user whose keys have all been removed
/// from the database ends up with only the header, and loses key based access.
fn authorized_keys(user: &UserAccount) -> String {
    let mut contents = String::from(AUTHORIZED_KEYS_HEADER);

    for ssh_key in &user.ssh_keys {
        contents.push_str(ssh_key.key.trim());
        contents.push('\n');
    }

    contents
}

/// Turns a failed operation into a status the controller can report, keeping
/// the whole error chain so the cause is not lost.
fn into_status(result: anyhow::Result<Status>) -> Status {
    match result {
        Ok(status) => status,
        Err(err) => Status::Failed {
            message: format!("{err:#}"),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use starfish_msg::{Group, SshKey};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A host that records what was done to it.
    #[derive(Default)]
    pub(crate) struct FakeSystem {
        state: Mutex<State>,
    }

    #[derive(Default)]
    struct State {
        groups: BTreeSet<String>,
        users: HashMap<String, FakeUser>,
        /// Every mutating call, in order, as `"verb arg arg"`.
        actions: Vec<String>,
        /// Operations that should fail, by the message they fail with.
        failures: HashMap<String, String>,
    }

    #[derive(Default, Clone)]
    struct FakeUser {
        uid: u32,
        full_name: String,
        groups: BTreeSet<String>,
        authorized_keys: Option<String>,
    }

    impl FakeSystem {
        fn with_group(self, group: &str) -> Self {
            self.state.lock().unwrap().groups.insert(group.to_string());
            self
        }

        fn with_user(self, name: &str, full_name: &str, groups: &[&str]) -> Self {
            self.with_account(name, validate::MIN_MANAGED_UID, full_name, groups)
        }

        /// Adds a user with a chosen uid, for the system account checks.
        fn with_account(self, name: &str, uid: u32, full_name: &str, groups: &[&str]) -> Self {
            self.state.lock().unwrap().users.insert(
                name.to_string(),
                FakeUser {
                    uid,
                    full_name: full_name.to_string(),
                    groups: groups.iter().map(|g| g.to_string()).collect(),
                    authorized_keys: None,
                },
            );
            self
        }

        fn failing(self, action: &str, message: &str) -> Self {
            self.state
                .lock()
                .unwrap()
                .failures
                .insert(action.to_string(), message.to_string());
            self
        }

        fn actions(&self) -> Vec<String> {
            self.state.lock().unwrap().actions.clone()
        }

        fn user(&self, name: &str) -> FakeUser {
            self.state.lock().unwrap().users[name].clone()
        }

        /// Records an action, failing it if the test asked for that.
        fn record(&self, action: String) -> anyhow::Result<()> {
            let mut state = self.state.lock().unwrap();

            if let Some(message) = state.failures.get(&action) {
                anyhow::bail!("{message}");
            }

            state.actions.push(action);

            Ok(())
        }
    }

    impl System for FakeSystem {
        fn group_exists(&self, group: &str) -> anyhow::Result<bool> {
            Ok(self.state.lock().unwrap().groups.contains(group))
        }

        fn create_group(&self, group: &str) -> anyhow::Result<()> {
            self.record(format!("create_group {group}"))?;
            self.state.lock().unwrap().groups.insert(group.to_string());

            Ok(())
        }

        fn user_id(&self, user: &str) -> anyhow::Result<Option<u32>> {
            Ok(self.state.lock().unwrap().users.get(user).map(|u| u.uid))
        }

        fn create_user(&self, user: &str, full_name: &str) -> anyhow::Result<()> {
            self.record(format!("create_user {user}"))?;
            self.state.lock().unwrap().users.insert(
                user.to_string(),
                FakeUser {
                    uid: validate::MIN_MANAGED_UID,
                    full_name: full_name.to_string(),
                    // Ubuntu gives every new user their own primary group.
                    groups: [user.to_string()].into_iter().collect(),
                    authorized_keys: None,
                },
            );

            Ok(())
        }

        fn user_full_name(&self, user: &str) -> anyhow::Result<String> {
            Ok(self.state.lock().unwrap().users[user].full_name.clone())
        }

        fn set_user_full_name(&self, user: &str, full_name: &str) -> anyhow::Result<()> {
            self.record(format!("set_full_name {user}"))?;
            self.state
                .lock()
                .unwrap()
                .users
                .get_mut(user)
                .unwrap()
                .full_name = full_name.to_string();

            Ok(())
        }

        fn user_groups(&self, user: &str) -> anyhow::Result<Vec<String>> {
            Ok(self.state.lock().unwrap().users[user]
                .groups
                .iter()
                .cloned()
                .collect())
        }

        fn add_to_group(&self, user: &str, group: &str) -> anyhow::Result<()> {
            self.record(format!("add_to_group {user} {group}"))?;
            self.state
                .lock()
                .unwrap()
                .users
                .get_mut(user)
                .unwrap()
                .groups
                .insert(group.to_string());

            Ok(())
        }

        fn remove_from_group(&self, user: &str, group: &str) -> anyhow::Result<()> {
            self.record(format!("remove_from_group {user} {group}"))?;
            self.state
                .lock()
                .unwrap()
                .users
                .get_mut(user)
                .unwrap()
                .groups
                .remove(group);

            Ok(())
        }

        fn authorized_keys(&self, user: &str) -> anyhow::Result<Option<String>> {
            Ok(self.state.lock().unwrap().users[user]
                .authorized_keys
                .clone())
        }

        fn set_authorized_keys(&self, user: &str, contents: &str) -> anyhow::Result<()> {
            self.record(format!("set_authorized_keys {user}"))?;
            self.state
                .lock()
                .unwrap()
                .users
                .get_mut(user)
                .unwrap()
                .authorized_keys = Some(contents.to_string());

            Ok(())
        }
    }

    fn config(groups: &[&str], users: Vec<UserAccount>) -> HostConfig {
        HostConfig {
            hostname: "web-1".to_string(),
            generation: 42,
            groups: groups
                .iter()
                .map(|name| Group {
                    name: name.to_string(),
                })
                .collect(),
            users,
        }
    }

    fn user(name: &str, groups: &[&str], is_sudoer: bool) -> UserAccount {
        UserAccount {
            name: name.to_string(),
            full_name: "Ada Lovelace".to_string(),
            email: "ada@example.com".to_string(),
            is_sudoer,
            groups: groups.iter().map(|g| g.to_string()).collect(),
            ssh_keys: vec![],
        }
    }

    fn status(report: &SyncReport, name: &str) -> Status {
        report
            .groups
            .iter()
            .chain(report.users.iter())
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("nothing reported for '{name}'"))
            .status
            .clone()
    }

    #[test]
    fn creates_missing_groups_and_users() {
        let system = FakeSystem::default();
        let report = sync(
            &system,
            &config(&["developers"], vec![user("ada", &["developers"], false)]),
        );

        assert_eq!(status(&report, "developers"), Status::Created);
        assert_eq!(status(&report, "ada"), Status::Created);
        assert_eq!(
            system.actions(),
            vec![
                "create_group developers",
                "create_user ada",
                "add_to_group ada developers",
                "set_authorized_keys ada",
            ]
        );
    }

    #[test]
    fn moves_a_sudoer_off_the_group_an_older_agent_used() {
        // A host configured before starfish-sudo existed: ada is a sudoer, and
        // her grant is membership of Ubuntu's own `sudo`.
        let system = FakeSystem::default()
            .with_group(LEGACY_SUDO_GROUP)
            .with_user("ada", "Ada Lovelace", &[LEGACY_SUDO_GROUP]);

        let report = sync(&system, &config(&[], vec![user("ada", &[], true)]));

        assert_eq!(status(&report, "ada"), Status::Updated);
        assert!(
            system.user("ada").groups.contains(SUDO_GROUP),
            "the new grant was not made: {:?}",
            system.user("ada").groups
        );
        assert!(
            !system.user("ada").groups.contains(LEGACY_SUDO_GROUP),
            "the old grant was left behind: {:?}",
            system.user("ada").groups
        );
    }

    #[test]
    fn never_grants_the_group_an_older_agent_used() {
        let system = FakeSystem::default().with_group(LEGACY_SUDO_GROUP);

        sync(&system, &config(&[], vec![user("ada", &[], true)]));

        assert!(
            !system.user("ada").groups.contains(LEGACY_SUDO_GROUP),
            "a new sudoer was put in Ubuntu's sudo group: {:?}",
            system.user("ada").groups
        );
    }

    #[test]
    fn leaves_a_host_that_already_matches_alone() {
        let system = FakeSystem::default().with_group("developers").with_user(
            "ada",
            "Ada Lovelace",
            &["ada", "developers"],
        );

        // The first sync writes the key file, the second should find nothing
        // left to do.
        let config = config(&["developers"], vec![user("ada", &["developers"], false)]);

        sync(&system, &config);

        let report = sync(&system, &config);

        assert_eq!(status(&report, "developers"), Status::Unchanged);
        assert_eq!(status(&report, "ada"), Status::Unchanged);
    }

    #[test]
    fn grants_and_revokes_sudo_through_the_sudo_group() {
        let system = FakeSystem::default().with_group("developers").with_user(
            "ada",
            "Ada Lovelace",
            &["ada"],
        );

        let granted = sync(
            &system,
            &config(&["developers"], vec![user("ada", &[], true)]),
        );

        assert_eq!(status(&granted, "ada"), Status::Updated);
        assert!(system.user("ada").groups.contains(SUDO_GROUP));

        let revoked = sync(
            &system,
            &config(&["developers"], vec![user("ada", &[], false)]),
        );

        assert_eq!(status(&revoked, "ada"), Status::Updated);
        assert!(!system.user("ada").groups.contains(SUDO_GROUP));
    }

    #[test]
    fn never_touches_a_group_the_controller_did_not_send() {
        let system = FakeSystem::default()
            .with_group("developers")
            .with_group("docker")
            .with_user("ada", "Ada Lovelace", &["ada", "docker", "developers"]);

        // 'docker' is not in the configuration, so it is none of the agent's
        // business even though ada is in it.
        let report = sync(
            &system,
            &config(&["developers"], vec![user("ada", &["developers"], false)]),
        );

        assert!(system.user("ada").groups.contains("docker"));
        assert_eq!(status(&report, "ada"), Status::Updated); // the key file
        assert!(
            !system
                .actions()
                .iter()
                .any(|action| action.contains("docker")),
            "docker was touched: {:?}",
            system.actions()
        );
    }

    #[test]
    fn removes_membership_of_a_group_the_controller_does_send() {
        let system = FakeSystem::default()
            .with_group("developers")
            .with_group("deploy")
            .with_user("ada", "Ada Lovelace", &["ada", "developers", "deploy"]);

        let report = sync(
            &system,
            &config(
                &["developers", "deploy"],
                vec![user("ada", &["developers"], false)],
            ),
        );

        assert_eq!(status(&report, "ada"), Status::Updated);
        assert!(!system.user("ada").groups.contains("deploy"));
        assert!(system.user("ada").groups.contains("developers"));
    }

    #[test]
    fn corrects_a_changed_full_name() {
        let system = FakeSystem::default().with_user("ada", "A. Lovelace", &["ada"]);

        let report = sync(&system, &config(&[], vec![user("ada", &[], false)]));

        assert_eq!(status(&report, "ada"), Status::Updated);
        assert_eq!(system.user("ada").full_name, "Ada Lovelace");
    }

    #[test]
    fn writes_the_keys_the_controller_sent() {
        let system = FakeSystem::default();
        let mut ada = user("ada", &[], false);

        ada.ssh_keys = vec![
            SshKey {
                name: "laptop".to_string(),
                key: "ssh-ed25519 AAAA-laptop".to_string(),
            },
            SshKey {
                name: "desktop".to_string(),
                key: "ssh-ed25519 AAAA-desktop".to_string(),
            },
        ];

        sync(&system, &config(&[], vec![ada]));

        let written = system.user("ada").authorized_keys.unwrap();

        assert!(written.starts_with(AUTHORIZED_KEYS_HEADER));
        assert!(written.contains("ssh-ed25519 AAAA-laptop\n"));
        assert!(written.contains("ssh-ed25519 AAAA-desktop\n"));
    }

    #[test]
    fn reports_a_failure_without_giving_up_on_everyone_else() {
        let system = FakeSystem::default().failing("create_user ada", "useradd exited with 1");

        let report = sync(
            &system,
            &config(&[], vec![user("ada", &[], false), user("jls", &[], false)]),
        );

        assert!(
            matches!(status(&report, "ada"), Status::Failed { message } if message.contains("useradd")),
            "unexpected status: {:?}",
            status(&report, "ada")
        );

        // The failure must not stop the user after it from being created.
        assert_eq!(status(&report, "jls"), Status::Created);
    }

    #[test]
    fn refuses_to_manage_a_system_account() {
        // A configuration naming an account the distribution owns must not be
        // able to reach into it, whatever else it asks for.
        let system = FakeSystem::default().with_account("daemon", 1, "daemon", &["daemon"]);

        let mut daemon = user("daemon", &[], true);

        daemon.full_name = "Owned".to_string();

        let report = sync(&system, &config(&[], vec![daemon]));

        assert!(
            matches!(status(&report, "daemon"), Status::Failed { message } if message.contains("system account")),
            "unexpected status: {:?}",
            status(&report, "daemon")
        );

        // Nothing at all was done to the account.  The sudo group is created
        // because the configuration asked for a sudoer; the point here is that
        // this account never joined it.
        assert_eq!(system.actions(), vec!["create_group starfish-sudo"]);
        assert_eq!(system.user("daemon").full_name, "daemon");
        assert!(!system.user("daemon").groups.contains(SUDO_GROUP));
    }

    #[test]
    fn refuses_a_name_that_would_be_read_as_an_option() {
        let system = FakeSystem::default();

        let report = sync(
            &system,
            &config(
                &["-o"],
                vec![user("--uid=0", &[], false), user("ada", &[], false)],
            ),
        );

        assert!(matches!(status(&report, "-o"), Status::Failed { .. }));
        assert!(matches!(status(&report, "--uid=0"), Status::Failed { .. }));

        // A bad name in the configuration must not stop the good ones.
        assert_eq!(status(&report, "ada"), Status::Created);
        assert!(
            !system.actions().iter().any(|action| action.contains("-o")),
            "{:?}",
            system.actions()
        );
    }

    #[test]
    fn refuses_a_full_name_that_would_corrupt_passwd() {
        let system = FakeSystem::default();
        let mut ada = user("ada", &[], false);

        ada.full_name = "Ada:0:0:/root:/bin/sh".to_string();

        let report = sync(&system, &config(&[], vec![ada]));

        assert!(matches!(status(&report, "ada"), Status::Failed { .. }));
        assert!(system.actions().is_empty(), "{:?}", system.actions());
    }

    #[test]
    fn carries_the_generation_it_applied() {
        let system = FakeSystem::default();
        let report = sync(&system, &config(&[], vec![]));

        assert_eq!(report.generation, 42);
        assert_eq!(report.hostname, "web-1");
    }
}
