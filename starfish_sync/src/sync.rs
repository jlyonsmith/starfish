//! Bringing the host in line with the configuration the controller sent.
//!
//! What Starfish will change on a host is decided by
//! [`MANAGED_TAG`](crate::system::MANAGED_TAG), which every account it creates
//! carries in its `GECOS` field along with the database's id for the user.
//! Within that set the host is made to match the configuration exactly: an
//! account whose user is no longer in the configuration is deleted, and one
//! whose login name has changed is renamed rather than deleted and rebuilt, so
//! that a uid and a home directory survive a change of alias. Outside it,
//! nothing is touched: an account with no tag is somebody else's.
//!
//! Group membership is the one thing revoked without a tag to go on, and only
//! for groups the controller sent, because that is the only way to take access
//! away from an account that stays.
//!
//! Groups themselves are not Starfish's to manage. They are expected to exist
//! on the host already, put there by whatever administers the host's groups,
//! and Starfish only ever moves users in and out of them. A group in the
//! configuration that the host does not have is reported as
//! [`Status::Missing`] and warned about, never created — and so is
//! [`SUDO_GROUP`], which `scripts/install-agent.sh` creates at install time.

use crate::system::{LEGACY_SUDO_GROUP, ManagedUser, SUDO_GROUP, System, gecos, tagged_id, warn};
use crate::validate;
use starfish_msg::{HostConfig, ItemReport, Status, SyncReport, UserAccount};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Written at the top of every `authorized_keys` the agent manages.
const AUTHORIZED_KEYS_HEADER: &str =
    "# Managed by starfish. Local changes are overwritten on the next sync.\n";

/// Applies `config` to the host and reports what happened to each group and
/// user.
///
/// One failure does not stop the rest: every item is attempted and reported on
/// its own, so a single broken account cannot hold up everybody else's access.
pub fn sync(system: &dyn System, config: &HostConfig) -> SyncReport {
    // The sudo group is checked here rather than being sent by the controller,
    // which knows nothing about it: it is not a security group anybody
    // configures, but a sudoer cannot be put in it unless it exists.  Only
    // looked for when somebody actually needs it, so a host with no sudoers is
    // not warned about a group it will never use.
    let mut names: Vec<String> = config.groups.iter().map(|g| g.name.clone()).collect();

    if config.users.iter().any(|user| user.is_sudoer) && !names.iter().any(|n| n == SUDO_GROUP) {
        names.push(SUDO_GROUP.to_string());
    }

    // Which of those groups the host actually has.  Nothing is created: the
    // set is worked out once here and then used to decide which memberships
    // can be granted at all.
    let mut present: BTreeSet<String> = BTreeSet::new();
    let mut groups = Vec::with_capacity(names.len());

    for name in names {
        let status = into_status(check_group(system, &name));

        if status == Status::Unchanged {
            present.insert(name.clone());
        }

        groups.push(ItemReport { name, status });
    }

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

    // What Starfish has already put on this host, taken once, before anything
    // is renamed out from under it. Failing to read it must not stop the rest:
    // the additive half of a sync does not need it, so the sync degrades to
    // what it would have done before tagging existed rather than doing nothing
    // at all — but nothing is deleted on the strength of an empty list.
    let existing = match system.managed_users() {
        Ok(existing) => existing,
        Err(err) => {
            warn(&format!(
                "Unable to read this host's accounts, so no account will be \
                 renamed or removed on this pass: {err:#}"
            ));

            Vec::new()
        }
    };

    let by_id: HashMap<u64, &ManagedUser> = existing.iter().map(|m| (m.id, m)).collect();

    let mut users: Vec<ItemReport> = config
        .users
        .iter()
        .map(|user| ItemReport {
            name: user.name.clone(),
            status: into_status(sync_user(system, user, &by_id, &managed, &present)),
        })
        .collect();

    // Anything left tagged on the host that the configuration no longer has is
    // an account whose user has gone. Renames have already happened, so an
    // account still listed here under an old name is one whose id is in the
    // configuration and is therefore not touched.
    let wanted: HashSet<u64> = config.users.iter().map(|user| user.id).collect();

    for account in &existing {
        if wanted.contains(&account.id) {
            continue;
        }

        let status = match remove_user(system, account) {
            Ok(None) => continue,
            Ok(Some(status)) => status,
            Err(err) => into_status(Err(err)),
        };

        users.push(ItemReport {
            name: account.name.clone(),
            status,
        });
    }

    SyncReport {
        hostname: config.hostname.clone(),
        generation: config.generation,
        groups,
        users,
    }
}

/// Looks for a group, without creating it.
///
/// Groups belong to whoever administers the host, so a missing one is reported
/// and warned about rather than made.
fn check_group(system: &dyn System, group: &str) -> anyhow::Result<Status> {
    validate::name("Group", group)?;

    if system.group_exists(group)? {
        return Ok(Status::Unchanged);
    }

    warn(&format!(
        "Group '{group}' does not exist on this host. Starfish does not create \
         groups, so nobody will be put in it."
    ));

    Ok(Status::Missing)
}

fn sync_user(
    system: &dyn System,
    user: &UserAccount,
    by_id: &HashMap<u64, &ManagedUser>,
    managed: &BTreeSet<&str>,
    present: &BTreeSet<String>,
) -> anyhow::Result<Status> {
    validate::name("User", &user.name)?;
    validate::full_name(&user.full_name)?;

    for group in &user.groups {
        validate::name("Group", group)?;
    }

    let renamed = rename_if_needed(system, user, by_id)?;
    let wanted_gecos = gecos(&user.full_name, user.id);

    let created = match system.user_id(&user.name)? {
        Some(uid) => {
            // The name exists already. If it belongs to the distribution then
            // this configuration is asking for something it should not have,
            // and the account is left exactly as it is.
            validate::managed_uid(&user.name, uid)?;
            false
        }
        None => {
            system.create_user(&user.name, &user.full_name, user.id)?;
            true
        }
    };

    let mut changed = created || renamed;

    // A freshly created account already carries the right GECOS, so there is
    // nothing to compare against.
    if !created {
        let current = system.user_gecos(&user.name)?;

        // An account this configuration names but that Starfish did not create
        // is taken over rather than left alone, because otherwise an agent
        // upgraded onto a host full of accounts made before tagging existed
        // would stop managing every one of them. That does mean an unrelated
        // local account whose name happens to collide is taken over too, so it
        // is said out loud: whoever reads the agent's log is the person who can
        // tell the two cases apart.
        match tagged_id(&current) {
            Some(id) if id == user.id => {}
            Some(id) => warn(&format!(
                "Account '{}' on this host is tagged for Starfish user {id}, but the \
                 configuration gives that name to user {}. Taking the account over; \
                 check that two users have not been given the same alias.",
                user.name, user.id
            )),
            None => warn(&format!(
                "Account '{}' already exists on this host and is not tagged \
                 {tag}, so Starfish did not create it. Taking it over: its \
                 full name and authorized_keys are now Starfish's. If this is \
                 a local account that happens to share the name, rename one of \
                 them.",
                user.name,
                tag = crate::system::MANAGED_TAG,
            )),
        }

        if current != wanted_gecos {
            system.set_user_identity(&user.name, &user.full_name, user.id)?;
            changed = true;
        }
    }

    changed |= sync_groups(system, user, managed, present)?;
    changed |= sync_authorized_keys(system, user)?;

    Ok(if created {
        Status::Created
    } else if changed {
        Status::Updated
    } else {
        Status::Unchanged
    })
}

/// Moves an account onto the login name the configuration now gives it.
/// Returns whether anything was renamed.
///
/// This is what makes a changed alias a rename rather than a deletion and a
/// fresh account: the uid, the home directory and everything in it stay where
/// they are. The account is found by the id in its tag, which is the one thing
/// about a user that does not change.
fn rename_if_needed(
    system: &dyn System,
    user: &UserAccount,
    by_id: &HashMap<u64, &ManagedUser>,
) -> anyhow::Result<bool> {
    let Some(existing) = by_id.get(&user.id) else {
        return Ok(false);
    };

    if existing.name == user.name {
        return Ok(false);
    }

    validate::name("User", &existing.name)?;
    validate::managed_uid(&existing.name, existing.uid)?;

    // Something already holds the name this account is moving to. `usermod`
    // would refuse anyway; failing here says why, and leaves both accounts as
    // they are rather than half moving one. The uid is checked first so that a
    // configuration renaming somebody onto `daemon` is told what it actually
    // did wrong.
    if let Some(uid) = system.user_id(&user.name)? {
        validate::managed_uid(&user.name, uid)?;

        anyhow::bail!(
            "Unable to rename '{}' to '{}': an account named '{}' already exists on this host",
            existing.name,
            user.name,
            user.name
        );
    }

    system.rename_user(&existing.name, &user.name)?;

    Ok(true)
}

/// Deletes an account whose user the configuration no longer has.
///
/// `None` means the account was not the one that was found earlier and was
/// left alone, which is not worth reporting: either it has already gone, or
/// its tag has changed, and in both cases somebody else's account is at stake
/// if this guesses wrong. Only an account still carrying the same tag is
/// deleted.
fn remove_user(system: &dyn System, account: &ManagedUser) -> anyhow::Result<Option<Status>> {
    validate::name("User", &account.name)?;

    let Some(uid) = system.user_id(&account.name)? else {
        return Ok(None);
    };

    validate::managed_uid(&account.name, uid)?;

    if tagged_id(&system.user_gecos(&account.name)?) != Some(account.id) {
        return Ok(None);
    }

    system.delete_user(&account.name)?;

    warn(&format!(
        "Account '{}' was removed from this host, with its home directory, \
         because Starfish user {} is no longer in this host's configuration.",
        account.name, account.id
    ));

    Ok(Some(Status::Removed))
}

/// Adds the user to the groups they should be in and removes them from managed
/// groups they should not. Returns whether anything changed.
///
/// `present` is the subset of the configuration's groups the host actually has.
/// A group outside it is skipped rather than attempted: `gpasswd` would fail,
/// and a group the host does not have is not this user's fault, so it must not
/// turn their whole account into a failure. It is warned about per user, which
/// is what says *who* is going without the access.
fn sync_groups(
    system: &dyn System,
    user: &UserAccount,
    managed: &BTreeSet<&str>,
    present: &BTreeSet<String>,
) -> anyhow::Result<bool> {
    let mut wanted: BTreeSet<&str> = user.groups.iter().map(String::as_str).collect();

    if user.is_sudoer {
        wanted.insert(SUDO_GROUP);
    }

    let current: BTreeSet<String> = system.user_groups(&user.name)?.into_iter().collect();
    let mut changed = false;

    for group in &wanted {
        if !present.contains(*group) {
            warn(&format!(
                "User '{}' should be in group '{group}', which does not exist on \
                 this host. {}",
                user.name,
                if *group == SUDO_GROUP {
                    "They are not getting sudo. Reinstall the agent, or create \
                     the group by hand, to restore it."
                } else {
                    "They are not getting whatever access it grants."
                }
            ));

            continue;
        }

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
        gecos: String,
        groups: BTreeSet<String>,
        authorized_keys: Option<String>,
    }

    impl FakeSystem {
        fn with_group(self, group: &str) -> Self {
            self.state.lock().unwrap().groups.insert(group.to_string());
            self
        }

        /// A tagged account belonging to the user of the same name, the way a
        /// previous sync would have left it.
        fn with_managed_user(self, name: &str, full_name: &str, groups: &[&str]) -> Self {
            self.with_managed_account(name, id_for(name), full_name, groups)
        }

        /// A tagged account belonging to a chosen user, for the rename and
        /// removal tests where whose account it is, is the whole question.
        fn with_managed_account(
            self,
            name: &str,
            id: u64,
            full_name: &str,
            groups: &[&str],
        ) -> Self {
            self.with_account(
                name,
                validate::MIN_MANAGED_UID,
                &gecos(full_name, id),
                groups,
            )
        }

        /// An account Starfish did not create: no tag, just a full name.
        fn with_user(self, name: &str, full_name: &str, groups: &[&str]) -> Self {
            self.with_account(name, validate::MIN_MANAGED_UID, full_name, groups)
        }

        /// Adds a user with a chosen uid and a literal GECOS, for the system
        /// account and tagging checks.
        fn with_account(self, name: &str, uid: u32, gecos: &str, groups: &[&str]) -> Self {
            self.state.lock().unwrap().users.insert(
                name.to_string(),
                FakeUser {
                    uid,
                    gecos: gecos.to_string(),
                    groups: groups.iter().map(|g| g.to_string()).collect(),
                    authorized_keys: None,
                },
            );
            self
        }

        /// Deletes a group the way `groupdel` does, taking its memberships
        /// with it: a supplementary membership lives in the group's own entry,
        /// so removing the group removes everybody from it.
        fn remove_group(&self, group: &str) {
            let mut state = self.state.lock().unwrap();

            state.groups.remove(group);

            for user in state.users.values_mut() {
                user.groups.remove(group);
            }
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

        fn has_user(&self, name: &str) -> bool {
            self.state.lock().unwrap().users.contains_key(name)
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

        fn user_id(&self, user: &str) -> anyhow::Result<Option<u32>> {
            Ok(self.state.lock().unwrap().users.get(user).map(|u| u.uid))
        }

        fn managed_users(&self) -> anyhow::Result<Vec<ManagedUser>> {
            let state = self.state.lock().unwrap();

            if let Some(message) = state.failures.get("managed_users") {
                anyhow::bail!("{message}");
            }

            let mut users: Vec<ManagedUser> = state
                .users
                .iter()
                .filter_map(|(name, user)| {
                    tagged_id(&user.gecos).map(|id| ManagedUser {
                        name: name.clone(),
                        uid: user.uid,
                        id,
                    })
                })
                .collect();

            // A `HashMap` has no order of its own, and the order accounts are
            // removed in shows up in the report.
            users.sort_by(|a, b| a.name.cmp(&b.name));

            Ok(users)
        }

        fn create_user(&self, user: &str, full_name: &str, id: u64) -> anyhow::Result<()> {
            self.record(format!("create_user {user}"))?;
            self.state.lock().unwrap().users.insert(
                user.to_string(),
                FakeUser {
                    uid: validate::MIN_MANAGED_UID,
                    gecos: gecos(full_name, id),
                    // Ubuntu gives every new user their own primary group.
                    groups: [user.to_string()].into_iter().collect(),
                    authorized_keys: None,
                },
            );

            Ok(())
        }

        fn rename_user(&self, user: &str, new_name: &str) -> anyhow::Result<()> {
            self.record(format!("rename_user {user} {new_name}"))?;

            let mut state = self.state.lock().unwrap();
            let account = state.users.remove(user).unwrap();

            state.users.insert(new_name.to_string(), account);

            Ok(())
        }

        fn delete_user(&self, user: &str) -> anyhow::Result<()> {
            self.record(format!("delete_user {user}"))?;

            let mut state = self.state.lock().unwrap();

            state.users.remove(user);

            // `userdel` takes the account's private group with it, and the
            // memberships along with the account.
            for account in state.users.values_mut() {
                account.groups.remove(user);
            }

            Ok(())
        }

        fn user_gecos(&self, user: &str) -> anyhow::Result<String> {
            Ok(self.state.lock().unwrap().users[user].gecos.clone())
        }

        fn set_user_identity(&self, user: &str, full_name: &str, id: u64) -> anyhow::Result<()> {
            self.record(format!("set_identity {user}"))?;
            self.state
                .lock()
                .unwrap()
                .users
                .get_mut(user)
                .unwrap()
                .gecos = gecos(full_name, id);

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

    /// A stable database id for a name, so that a fixture and the account it
    /// stands for agree on whose it is without every test spelling an id out.
    fn id_for(name: &str) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        name.hash(&mut hasher);
        hasher.finish()
    }

    fn user(name: &str, groups: &[&str], is_sudoer: bool) -> UserAccount {
        user_with_id(name, id_for(name), groups, is_sudoer)
    }

    /// A configured user with a chosen database id, for the rename and removal
    /// tests, where the id is the whole point.
    fn user_with_id(name: &str, id: u64, groups: &[&str], is_sudoer: bool) -> UserAccount {
        UserAccount {
            id,
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
    fn creates_missing_users_and_joins_them_to_existing_groups() {
        let system = FakeSystem::default().with_group("developers");
        let report = sync(
            &system,
            &config(&["developers"], vec![user("ada", &["developers"], false)]),
        );

        // The group was already there, and is never something a sync makes.
        assert_eq!(status(&report, "developers"), Status::Unchanged);
        assert_eq!(status(&report, "ada"), Status::Created);
        assert_eq!(
            system.actions(),
            vec![
                "create_user ada",
                "add_to_group ada developers",
                "set_authorized_keys ada",
            ]
        );
    }

    #[test]
    fn never_creates_a_group_the_host_does_not_have() {
        let system = FakeSystem::default();
        let report = sync(
            &system,
            &config(&["developers"], vec![user("ada", &["developers"], false)]),
        );

        // Reported, not failed: the host is entitled not to have the group,
        // and the user is still created and given their keys.
        assert_eq!(status(&report, "developers"), Status::Missing);
        assert_eq!(status(&report, "ada"), Status::Created);
        assert_eq!(
            system.actions(),
            vec!["create_user ada", "set_authorized_keys ada"]
        );
    }

    #[test]
    fn reports_a_group_deleted_from_the_host_while_users_were_still_in_it() {
        // The first sync is an ordinary one, with the group in place.
        let system = FakeSystem::default().with_group("developers");
        let config = config(&["developers"], vec![user("ada", &["developers"], false)]);

        assert_eq!(status(&sync(&system, &config), "ada"), Status::Created);
        assert!(system.user("ada").groups.contains("developers"));

        // Then somebody deletes the group on the host, which takes its
        // memberships with it.  Starfish does not put it back.
        system.remove_group("developers");

        let before = system.actions().len();
        let report = sync(&system, &config);

        assert_eq!(status(&report, "developers"), Status::Missing);
        assert_eq!(
            &system.actions()[before..],
            &[] as &[String],
            "the host was changed over a group it no longer has"
        );
        // Nothing else about ada is disturbed by the group going away.
        assert_eq!(status(&report, "ada"), Status::Unchanged);
    }

    #[test]
    fn moves_a_sudoer_off_the_group_an_older_agent_used() {
        // A host configured before starfish-sudo existed: ada is a sudoer, and
        // her grant is membership of Ubuntu's own `sudo`.
        let system = FakeSystem::default()
            .with_group(LEGACY_SUDO_GROUP)
            .with_group(SUDO_GROUP)
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
        let system = FakeSystem::default()
            .with_group("developers")
            .with_group(SUDO_GROUP)
            .with_user("ada", "Ada Lovelace", &["ada"]);

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
        assert_eq!(
            system.user("ada").gecos,
            gecos("Ada Lovelace", id_for("ada"))
        );
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

        // Nothing at all was done, to this account or to the host.
        assert!(system.actions().is_empty(), "{:?}", system.actions());
        assert_eq!(system.user("daemon").gecos, "daemon");
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
    fn tags_every_account_it_creates() {
        let system = FakeSystem::default();

        sync(&system, &config(&[], vec![user("ada", &[], false)]));

        // The tag is what every later sync recognises the account by, so it
        // has to go on at creation and not on some second pass.
        assert_eq!(
            system.user("ada").gecos,
            format!("Ada Lovelace,,,,STARFISH-{}", id_for("ada"))
        );
        assert_eq!(tagged_id(&system.user("ada").gecos), Some(id_for("ada")));
    }

    #[test]
    fn renames_an_account_when_the_alias_changes() {
        // The host has the account under the alias the user had yesterday.
        let system = FakeSystem::default()
            .with_group("developers")
            .with_managed_account("adalove", 42, "Ada Lovelace", &["adalove", "developers"]);

        let report = sync(
            &system,
            &config(
                &["developers"],
                vec![user_with_id("ada", 42, &["developers"], false)],
            ),
        );

        assert_eq!(status(&report, "ada"), Status::Updated);
        assert!(system.has_user("ada"));
        assert!(
            !system.has_user("adalove"),
            "the old account was left behind"
        );

        // Renamed, not rebuilt: nothing was created and nothing was deleted,
        // which is what keeps the uid and the home directory.
        let actions = system.actions();

        assert!(
            actions.contains(&"rename_user adalove ada".to_string()),
            "{actions:?}"
        );
        assert!(
            !actions.iter().any(|a| a.starts_with("create_user")),
            "the account was rebuilt rather than renamed: {actions:?}"
        );
        assert!(
            !actions.iter().any(|a| a.starts_with("delete_user")),
            "the old account was deleted: {actions:?}"
        );
        assert_eq!(system.user("ada").uid, validate::MIN_MANAGED_UID);
        assert_eq!(tagged_id(&system.user("ada").gecos), Some(42));
    }

    #[test]
    fn refuses_to_rename_onto_a_name_that_is_already_taken() {
        // Somebody else's account already holds the name this user is moving
        // to. Renaming would either fail or, worse, collide.
        let system = FakeSystem::default()
            .with_managed_account("adalove", 42, "Ada Lovelace", &["adalove"])
            .with_user("ada", "Ada Other", &["ada"]);

        let report = sync(
            &system,
            &config(&[], vec![user_with_id("ada", 42, &[], false)]),
        );

        assert!(
            matches!(status(&report, "ada"), Status::Failed { message } if message.contains("already exists")),
            "unexpected status: {:?}",
            status(&report, "ada")
        );

        // Both accounts are left exactly as they were.
        assert!(system.has_user("adalove"));
        assert_eq!(system.user("ada").gecos, "Ada Other");
        assert!(
            !system
                .actions()
                .iter()
                .any(|a| a.starts_with("rename_user")),
            "{:?}",
            system.actions()
        );
    }

    #[test]
    fn refuses_to_rename_an_account_onto_a_system_one() {
        let system = FakeSystem::default()
            .with_managed_account("adalove", 42, "Ada Lovelace", &["adalove"])
            .with_account("daemon", 1, "daemon", &["daemon"]);

        let report = sync(
            &system,
            &config(&[], vec![user_with_id("daemon", 42, &[], false)]),
        );

        assert!(
            matches!(status(&report, "daemon"), Status::Failed { message } if message.contains("system account")),
            "unexpected status: {:?}",
            status(&report, "daemon")
        );
        assert_eq!(system.user("daemon").gecos, "daemon");
        assert!(system.has_user("adalove"), "the renamed account was lost");
    }

    #[test]
    fn removes_an_account_whose_user_is_no_longer_configured() {
        let system = FakeSystem::default()
            .with_managed_account("bob", 7, "Bob Bobson", &["bob"])
            .with_managed_user("ada", "Ada Lovelace", &["ada"]);

        let report = sync(&system, &config(&[], vec![user("ada", &[], false)]));

        assert_eq!(status(&report, "bob"), Status::Removed);
        assert!(!system.has_user("bob"), "the account is still there");
        assert!(
            system.actions().contains(&"delete_user bob".to_string()),
            "{:?}",
            system.actions()
        );

        // Everybody still in the configuration is untouched by it.
        assert!(system.has_user("ada"));
    }

    #[test]
    fn never_removes_an_account_starfish_did_not_create() {
        // No tag, so Starfish has no reason to believe this account is its to
        // delete, however little the configuration knows about it.
        let system = FakeSystem::default().with_user("localadmin", "Local Admin", &["localadmin"]);

        let report = sync(&system, &config(&[], vec![user("ada", &[], false)]));

        assert!(system.has_user("localadmin"));
        assert!(
            report.users.iter().all(|item| item.name != "localadmin"),
            "an account Starfish does not manage was reported on: {:?}",
            report.users
        );
        assert!(
            !system
                .actions()
                .iter()
                .any(|action| action.contains("localadmin")),
            "{:?}",
            system.actions()
        );
    }

    #[test]
    fn never_removes_a_system_account_however_it_is_tagged() {
        // A tag on a system account, whether somebody put it there by hand or
        // an earlier configuration did, must not be enough to delete it.
        let system = FakeSystem::default().with_account("daemon", 1, &gecos("Daemon", 7), &[]);

        let report = sync(&system, &config(&[], vec![user("ada", &[], false)]));

        assert!(
            matches!(status(&report, "daemon"), Status::Failed { message } if message.contains("system account")),
            "unexpected status: {:?}",
            status(&report, "daemon")
        );
        assert!(system.has_user("daemon"), "a system account was deleted");
    }

    #[test]
    fn adopts_an_untagged_account_that_shares_a_name() {
        // What every host looks like on the first sync after an upgrade: the
        // accounts are Starfish's, but were made before there was a tag to put
        // on them. Taking them over is what keeps them managed.
        let system = FakeSystem::default().with_user("ada", "Ada Lovelace", &["ada"]);

        let report = sync(&system, &config(&[], vec![user("ada", &[], false)]));

        assert_eq!(status(&report, "ada"), Status::Updated);
        assert_eq!(tagged_id(&system.user("ada").gecos), Some(id_for("ada")));
        assert!(
            !system
                .actions()
                .iter()
                .any(|a| a.starts_with("create_user") || a.starts_with("delete_user")),
            "the account was rebuilt rather than adopted: {:?}",
            system.actions()
        );

        // And from here on it is an ordinary managed account: a second sync
        // has nothing left to do, and dropping the user removes it.
        assert_eq!(
            status(
                &sync(&system, &config(&[], vec![user("ada", &[], false)])),
                "ada"
            ),
            Status::Unchanged
        );

        let report = sync(&system, &config(&[], vec![]));

        assert_eq!(status(&report, "ada"), Status::Removed);
    }

    #[test]
    fn removes_nobody_when_the_host_cannot_be_read() {
        // Without the list of tagged accounts there is no way to tell an
        // account whose user has gone from one Starfish never created, so the
        // sync does the additive half and leaves every account alone.
        let system = FakeSystem::default()
            .with_managed_account("bob", 7, "Bob Bobson", &["bob"])
            .failing("managed_users", "getent passwd failed");

        let report = sync(&system, &config(&[], vec![user("ada", &[], false)]));

        assert_eq!(status(&report, "ada"), Status::Created);
        assert!(system.has_user("bob"), "an account was removed blindly");
        assert!(
            !system
                .actions()
                .iter()
                .any(|a| a.starts_with("delete_user")),
            "{:?}",
            system.actions()
        );
    }

    #[test]
    fn carries_the_generation_it_applied() {
        let system = FakeSystem::default();
        let report = sync(&system, &config(&[], vec![]));

        assert_eq!(report.generation, 42);
        assert_eq!(report.hostname, "web-1");
    }
}
