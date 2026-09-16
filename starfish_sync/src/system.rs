//! The host operations the agent needs, and the Ubuntu implementation of them.
//!
//! Everything that changes the system goes through standard Ubuntu tools
//! (`useradd`, `usermod`, `gpasswd`, `getent`, `id`) rather than touching
//! `/etc/passwd` and friends directly. Putting them behind a trait also lets
//! the synchronization logic be tested without a real host.
//!
//! There is deliberately no way to create, delete or rename a group here.
//! Groups are administered outside Starfish, which only moves users in and out
//! of them, so the capability is left out of the trait rather than left unused.
//! Users are a different matter: an account Starfish created is one it may
//! rename and delete, and [`MANAGED_TAG`] is how it tells those apart from
//! accounts that were already on the host.

use anyhow::{Context, bail};
use std::path::{Path, PathBuf};

/// The group that grants sudo.
///
/// Deliberately not Ubuntu's own `sudo`. Managed accounts are created with no
/// password — authentication is by SSH key — so they could never answer the
/// prompt that the stock `%sudo ALL=(ALL:ALL) ALL` rule demands. This group is
/// the one `deploy/starfish-sudoers` gives `NOPASSWD` to, which keeps that
/// grant away from every account Starfish does not manage.
///
/// Created by `scripts/install-agent.sh`, not by a sync: like every other
/// group it has to exist on the host before anybody can be put in it.
pub const SUDO_GROUP: &str = "starfish-sudo";

/// How sudo used to be granted, before [`SUDO_GROUP`] existed.
///
/// Still managed, so that membership left behind by an older agent is taken
/// away on the next sync, but never granted. Without this an upgrade would
/// strand a grant nobody ever revokes: `sudo` would no longer be a group the
/// controller sends, and Starfish never removes anybody from a group it does
/// not manage.
pub const LEGACY_SUDO_GROUP: &str = "sudo";

/// The word that marks an account as Starfish's, written into the last field
/// of its `GECOS` followed by a hyphen and the database's id for the user:
///
/// ```text
/// Ada Lovelace,,,,STARFISH-42
/// ```
///
/// `GECOS` is the only place on a host to record this: it is free text that
/// the standard tools already write, so no file outside `/etc/passwd` has to
/// be kept in step with the accounts themselves.
///
/// A hyphen joins the two because `shadow` refuses `:`, `,` and `=` in any
/// `GECOS` field — `:` and `,` are the separators of `/etc/passwd` and of
/// `GECOS` itself — so the more obvious `STARFISH:42` cannot be written at all.
///
/// The tag answers the two questions a login name cannot. *Is this account
/// ours?* — an untagged account may be a local one that happens to share a
/// name, and deleting it would be destroying somebody else's work. *Which user
/// is it?* — the id never changes, so an account whose login was renamed in the
/// database is still recognised, and renamed on the host rather than deleted
/// and rebuilt under the new name.
pub const MANAGED_TAG: &str = "STARFISH";

/// The shell new users are given.
const DEFAULT_SHELL: &str = "/bin/bash";

/// Separates [`MANAGED_TAG`] from the id that follows it.
const TAG_SEPARATOR: char = '-';

/// `userdel` uses this to mean the account went but its home directory, or
/// part of it, did not.
const USERDEL_HOME_NOT_REMOVED: i32 = 12;

/// `getent` uses this to mean "no such entry", as opposed to a real failure.
const GETENT_NOT_FOUND: i32 = 2;

/// A Starfish managed account found on the host: one whose `GECOS` carries
/// [`MANAGED_TAG`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedUser {
    /// The login name the account has on the host *now*, which is not
    /// necessarily the one the configuration gives it.
    pub name: String,

    /// The host's numeric user id, checked before the account is touched so
    /// that a tag written onto a system account cannot hand it over.
    pub uid: u32,

    /// The database id from the tag.
    pub id: u64,
}

/// The whole `GECOS` field a managed account should have.
///
/// Five comma separated fields — full name, room, two phone numbers, and the
/// free text one at the end that carries [`MANAGED_TAG`]. The full name keeps
/// the first field, which is where `finger`, `ls -l` and everything else looks
/// for it; the three in between are deliberately emptied, because the field is
/// Starfish's outright in the same way `authorized_keys` is.
pub fn gecos(full_name: &str, id: u64) -> String {
    format!("{full_name},,,,{MANAGED_TAG}{TAG_SEPARATOR}{id}")
}

/// The database id in a `GECOS` field's tag, or `None` if it carries none.
///
/// A `GECOS` an administrator wrote, or one from an account Starfish never
/// created, has no last field of the form `STARFISH-<id>` and comes back
/// `None`, which is what stops Starfish deleting somebody else's account.
pub fn tagged_id(gecos: &str) -> Option<u64> {
    gecos
        .rsplit(',')
        .next()?
        .strip_prefix(MANAGED_TAG)?
        .strip_prefix(TAG_SEPARATOR)?
        .parse()
        .ok()
}

/// Writes a warning for the agent to pick up.
///
/// The helper's standard output carries the report, so this goes to standard
/// error, which the agent captures and logs. Anything an administrator ought
/// to see from the controller instead belongs in the report as a
/// `starfish_msg::Status`.
pub fn warn(message: &str) {
    eprintln!("warning: {message}");
}

/// The host operations the agent performs.
pub trait System: Send + Sync {
    /// Whether the host has this group. There is no counterpart that creates
    /// one, by design.
    fn group_exists(&self, group: &str) -> anyhow::Result<bool>;

    /// The user's numeric id, or `None` if there is no such user. The id is
    /// what tells a Starfish managed account apart from one the distribution
    /// created.
    fn user_id(&self, user: &str) -> anyhow::Result<Option<u32>>;

    /// Every account on the host whose `GECOS` carries [`MANAGED_TAG`].
    ///
    /// This is the whole basis for renaming and removing: it is the only way
    /// to ask the host what Starfish has already put on it, rather than only
    /// asking after the accounts the current configuration happens to name.
    fn managed_users(&self) -> anyhow::Result<Vec<ManagedUser>>;

    /// Creates a user with a home directory and the default shell, carrying
    /// the `GECOS` that [`gecos`] describes.
    fn create_user(&self, user: &str, full_name: &str, id: u64) -> anyhow::Result<()>;

    /// Changes an account's login name, keeping its uid, its home directory
    /// and everything in it.
    fn rename_user(&self, user: &str, new_name: &str) -> anyhow::Result<()>;

    /// Deletes an account and its home directory.
    fn delete_user(&self, user: &str) -> anyhow::Result<()>;

    /// The user's whole `GECOS` field, full name and tag together, exactly as
    /// `/etc/passwd` holds it.
    fn user_gecos(&self, user: &str) -> anyhow::Result<String>;

    /// Writes the full name and the tag, leaving the account carrying exactly
    /// the `GECOS` that [`gecos`] returns for the same arguments.
    fn set_user_identity(&self, user: &str, full_name: &str, id: u64) -> anyhow::Result<()>;

    /// Every group the user is in, primary and supplementary.
    fn user_groups(&self, user: &str) -> anyhow::Result<Vec<String>>;
    fn add_to_group(&self, user: &str, group: &str) -> anyhow::Result<()>;
    fn remove_from_group(&self, user: &str, group: &str) -> anyhow::Result<()>;

    /// The contents of the user's `authorized_keys`, or `None` if there is no
    /// such file yet.
    fn authorized_keys(&self, user: &str) -> anyhow::Result<Option<String>>;
    fn set_authorized_keys(&self, user: &str, contents: &str) -> anyhow::Result<()>;
}

/// Talks to a real Ubuntu host.
pub struct Ubuntu;

impl System for Ubuntu {
    fn group_exists(&self, group: &str) -> anyhow::Result<bool> {
        Ok(getent("group", group)?.is_some())
    }

    fn user_id(&self, user: &str) -> anyhow::Result<Option<u32>> {
        let Some(entry) = getent("passwd", user)? else {
            return Ok(None);
        };

        let entry: Vec<&str> = entry.trim_end().split(':').collect();

        let uid = entry
            .get(2)
            .with_context(|| format!("The passwd entry for '{user}' has no uid"))?;

        Ok(Some(uid.parse().with_context(|| {
            format!("The passwd entry for '{user}' has an unreadable uid '{uid}'")
        })?))
    }

    fn managed_users(&self) -> anyhow::Result<Vec<ManagedUser>> {
        // `getent passwd` with no key lists the whole database, which is the
        // only way to find accounts the configuration no longer mentions.
        let passwd = run(&["getent", "passwd"])?;
        let mut users = Vec::new();

        for line in passwd.lines() {
            let entry: Vec<&str> = line.split(':').collect();

            let (Some(name), Some(uid), Some(gecos)) = (entry.first(), entry.get(2), entry.get(4))
            else {
                continue;
            };

            let (Some(uid), Some(id)) = (uid.parse().ok(), tagged_id(gecos)) else {
                continue;
            };

            users.push(ManagedUser {
                name: name.to_string(),
                uid,
                id,
            });
        }

        Ok(users)
    }

    fn create_user(&self, user: &str, full_name: &str, id: u64) -> anyhow::Result<()> {
        // `useradd --comment` takes the full name but not the tag, because it
        // refuses a comma and the tag can only live behind one. The account is
        // therefore created named but untagged, and tagged a moment later. If
        // the second step fails the account is left as any other untagged one,
        // which the next sync adopts.
        run(&[
            "useradd",
            "--create-home",
            "--shell",
            DEFAULT_SHELL,
            "--comment",
            full_name,
            user,
        ])?;

        self.set_user_identity(user, full_name, id)
    }

    fn rename_user(&self, user: &str, new_name: &str) -> anyhow::Result<()> {
        let entry = passwd_entry(user)?;
        let home = entry.get(5).map(String::as_str).unwrap_or_default();

        // The home directory moves with the account only when it is the one
        // `useradd --create-home` would have made. A home somebody placed
        // deliberately somewhere else is left where they put it, because this
        // rename is a change of login name and nothing more.
        if home == format!("/home/{user}") {
            let new_home = format!("/home/{new_name}");

            run(&[
                "usermod",
                "--login",
                new_name,
                "--home",
                &new_home,
                "--move-home",
                user,
            ])?;
        } else {
            run(&["usermod", "--login", new_name, user])?;
        }

        // The user's private group keeps the old login's name: renaming it
        // would mean a `groupmod`, and the one thing this helper deliberately
        // cannot do is administer groups. Nothing depends on the two names
        // matching — `set_authorized_keys` chowns to the login group rather
        // than to a group it assumes is named after the user.
        Ok(())
    }

    fn delete_user(&self, user: &str) -> anyhow::Result<()> {
        let output = duct::cmd("userdel", ["--remove", user])
            .stdout_capture()
            .stderr_capture()
            .unchecked()
            .run()
            .context("Unable to run userdel")?;

        if output.status.success() {
            return Ok(());
        }

        // The account itself is gone in this case, so failing here would put a
        // permanent failure in every future report over files that need an
        // administrator, not another sync.
        if output.status.code() == Some(USERDEL_HOME_NOT_REMOVED) {
            warn(&format!(
                "Account '{user}' was deleted, but its home directory was not \
                 fully removed: {}",
                describe(&output.status, &output.stderr)
            ));

            return Ok(());
        }

        bail!(
            "userdel --remove {user} failed: {}",
            describe(&output.status, &output.stderr)
        )
    }

    fn user_gecos(&self, user: &str) -> anyhow::Result<String> {
        let entry = passwd_entry(user)?;

        Ok(entry.get(4).cloned().unwrap_or_default())
    }

    fn set_user_identity(&self, user: &str, full_name: &str, id: u64) -> anyhow::Result<()> {
        // `chfn`, not `usermod --comment`, which writes `GECOS` as one string
        // and so cannot write the comma that separates the tag from the name.
        // `chfn` writes the fields one at a time and never sees a comma.
        //
        // All five are passed every time, the middle three empty, so that the
        // result is exactly what `gecos` describes however the field looked
        // before: `chfn` leaves a field it is not given alone, and a room
        // number somebody had set would otherwise make every sync find a
        // difference it could never settle.
        run(&[
            "chfn",
            "--full-name",
            full_name,
            "--room",
            "",
            "--work-phone",
            "",
            "--home-phone",
            "",
            "--other",
            &format!("{MANAGED_TAG}{TAG_SEPARATOR}{id}"),
            user,
        ])?;

        Ok(())
    }

    fn user_groups(&self, user: &str) -> anyhow::Result<Vec<String>> {
        let output = run(&["id", "--name", "--groups", user])?;

        Ok(output.split_whitespace().map(str::to_string).collect())
    }

    fn add_to_group(&self, user: &str, group: &str) -> anyhow::Result<()> {
        // `gpasswd` changes one membership, where `usermod --groups` would
        // replace the user's whole supplementary list and drop groups this
        // agent was never told about.
        run(&["gpasswd", "--add", user, group])?;

        Ok(())
    }

    fn remove_from_group(&self, user: &str, group: &str) -> anyhow::Result<()> {
        run(&["gpasswd", "--delete", user, group])?;

        Ok(())
    }

    fn authorized_keys(&self, user: &str) -> anyhow::Result<Option<String>> {
        let path = authorized_keys_path(user)?;

        match std::fs::read_to_string(&path) {
            Ok(contents) => Ok(Some(contents)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => {
                Err(anyhow::Error::new(err).context(format!("Unable to read {}", path.display())))
            }
        }
    }

    fn set_authorized_keys(&self, user: &str, contents: &str) -> anyhow::Result<()> {
        let path = authorized_keys_path(user)?;
        let dir = path.parent().expect("authorized_keys always has a parent");

        std::fs::create_dir_all(dir)
            .with_context(|| format!("Unable to create {}", dir.display()))?;

        // Written next to the real file and renamed so a crash midway cannot
        // leave a user with a half written key file and no way in.
        let temp = path.with_extension("starfish-new");

        std::fs::write(&temp, contents)
            .with_context(|| format!("Unable to write {}", temp.display()))?;
        std::fs::rename(&temp, &path)
            .with_context(|| format!("Unable to replace {}", path.display()))?;

        // SSH ignores a key file the user does not own or that others can
        // write, so ownership and mode have to be set every time.
        //
        // The trailing colon with no group means "this user's login group",
        // which is not always a group named after them: a renamed account
        // keeps the private group `useradd` gave it under its old name.
        let owner = format!("{user}:");

        run(&["chown", "--recursive", &owner, &dir.to_string_lossy()])?;
        run(&["chmod", "700", &dir.to_string_lossy()])?;
        run(&["chmod", "600", &path.to_string_lossy()])?;

        Ok(())
    }
}

/// The user's `~/.ssh/authorized_keys`, from the home directory in `passwd`.
fn authorized_keys_path(user: &str) -> anyhow::Result<PathBuf> {
    let entry = passwd_entry(user)?;

    let home = entry
        .get(5)
        .filter(|home| !home.is_empty())
        .with_context(|| format!("User '{user}' has no home directory"))?;

    let home = Path::new(home);

    // The path comes from `passwd` rather than from the controller, but it
    // decides where a key file is written, so it is worth checking. A relative
    // home would resolve against whatever directory this process happens to be
    // in, and `/` would put keys in the root of the filesystem.
    if !home.is_absolute() || home.parent().is_none() {
        bail!(
            "User '{user}' has an unusable home directory '{}'",
            home.display()
        );
    }

    Ok(home.join(".ssh").join("authorized_keys"))
}

/// The `passwd` entry for a user, split on `:`.
fn passwd_entry(user: &str) -> anyhow::Result<Vec<String>> {
    let entry =
        getent("passwd", user)?.with_context(|| format!("There is no user named '{user}'"))?;

    Ok(entry.trim_end().split(':').map(str::to_string).collect())
}

/// Looks `key` up in `database`. `None` means there is no such entry, which is
/// an answer rather than a failure.
fn getent(database: &str, key: &str) -> anyhow::Result<Option<String>> {
    let output = duct::cmd("getent", [database, key])
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .run()
        .with_context(|| format!("Unable to run getent {database} {key}"))?;

    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()));
    }

    if output.status.code() == Some(GETENT_NOT_FOUND) {
        return Ok(None);
    }

    bail!(
        "getent {database} {key} failed: {}",
        describe(&output.status, &output.stderr)
    )
}

/// Runs a command, returning its standard output. A non-zero exit is an error
/// carrying whatever the command wrote to standard error.
fn run(args: &[&str]) -> anyhow::Result<String> {
    let (program, rest) = args.split_first().expect("a command needs a program");

    let output = duct::cmd(*program, rest)
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .run()
        .with_context(|| format!("Unable to run {program}"))?;

    if !output.status.success() {
        bail!(
            "{} failed: {}",
            args.join(" "),
            describe(&output.status, &output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Turns an exit status and its standard error into one readable line.
fn describe(status: &std::process::ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();

    match status.code() {
        Some(code) if stderr.is_empty() => format!("exit code {code}"),
        Some(code) => format!("exit code {code}: {stderr}"),
        None if stderr.is_empty() => "killed by a signal".to_string(),
        None => format!("killed by a signal: {stderr}"),
    }
}
