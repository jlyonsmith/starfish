//! The host operations the agent needs, and the Ubuntu implementation of them.
//!
//! Everything that changes the system goes through standard Ubuntu tools
//! (`useradd`, `usermod`, `groupadd`, `gpasswd`, `getent`, `id`) rather than
//! touching `/etc/passwd` and friends directly. Putting them behind a trait
//! also lets the synchronization logic be tested without a real host.

use anyhow::{Context, bail};
use std::path::{Path, PathBuf};

/// The group that grants sudo.
///
/// Deliberately not Ubuntu's own `sudo`. Managed accounts are created with no
/// password — authentication is by SSH key — so they could never answer the
/// prompt that the stock `%sudo ALL=(ALL:ALL) ALL` rule demands. This group is
/// the one `deploy/starfish-sudoers` gives `NOPASSWD` to, which keeps that
/// grant away from every account Starfish does not manage.
pub const SUDO_GROUP: &str = "starfish-sudo";

/// How sudo used to be granted, before [`SUDO_GROUP`] existed.
///
/// Still managed, so that membership left behind by an older agent is taken
/// away on the next sync, but never granted. Without this an upgrade would
/// strand a grant nobody ever revokes: `sudo` would no longer be a group the
/// controller sends, and Starfish never removes anybody from a group it does
/// not manage.
pub const LEGACY_SUDO_GROUP: &str = "sudo";

/// The shell new users are given.
const DEFAULT_SHELL: &str = "/bin/bash";

/// `getent` uses this to mean "no such entry", as opposed to a real failure.
const GETENT_NOT_FOUND: i32 = 2;

/// The host operations the agent performs.
pub trait System: Send + Sync {
    fn group_exists(&self, group: &str) -> anyhow::Result<bool>;
    fn create_group(&self, group: &str) -> anyhow::Result<()>;

    /// The user's numeric id, or `None` if there is no such user. The id is
    /// what tells a Starfish managed account apart from one the distribution
    /// created.
    fn user_id(&self, user: &str) -> anyhow::Result<Option<u32>>;

    /// Creates a user with a home directory and the default shell.
    fn create_user(&self, user: &str, full_name: &str) -> anyhow::Result<()>;

    /// The user's full name, taken from the first `GECOS` field.
    fn user_full_name(&self, user: &str) -> anyhow::Result<String>;
    fn set_user_full_name(&self, user: &str, full_name: &str) -> anyhow::Result<()>;

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

    fn create_group(&self, group: &str) -> anyhow::Result<()> {
        run(&["groupadd", group])?;

        Ok(())
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

    fn create_user(&self, user: &str, full_name: &str) -> anyhow::Result<()> {
        run(&[
            "useradd",
            "--create-home",
            "--shell",
            DEFAULT_SHELL,
            "--comment",
            full_name,
            user,
        ])?;

        Ok(())
    }

    fn user_full_name(&self, user: &str) -> anyhow::Result<String> {
        let entry = passwd_entry(user)?;

        // GECOS holds comma separated fields; the full name is the first.
        Ok(entry
            .get(4)
            .map(|gecos| gecos.split(',').next().unwrap_or_default())
            .unwrap_or_default()
            .to_string())
    }

    fn set_user_full_name(&self, user: &str, full_name: &str) -> anyhow::Result<()> {
        run(&["usermod", "--comment", full_name, user])?;

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
        let owner = format!("{user}:{user}");

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
