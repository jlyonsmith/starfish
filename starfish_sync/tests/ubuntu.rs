//! Runs the privileged helper against a real Ubuntu, in a container.
//!
//! The unit tests in `sync.rs` prove the decisions are right against a fake
//! host. These prove the commands are right against a real one: that the
//! `useradd` flags do what they are meant to, that `getent` reports what the
//! code expects, that key files end up owned and moded correctly, and that the
//! sudoers rule in `deploy/` actually works.
//!
//! They need Docker (colima is fine) and are skipped unless asked for:
//!
//! ```text
//! just test-ubuntu
//! ```
//!
//! or, with the image already built:
//!
//! ```text
//! STARFISH_TEST_DOCKER=1 cargo test -p starfish_sync --test ubuntu
//! ```

use starfish_msg::{Group, HostConfig, SshKey, Status, SyncReport, UserAccount};
use std::io::Write;
use std::process::{Command, Stdio};

const IMAGE: &str = "starfish-test:latest";
const HELPER: &str = "/usr/local/lib/starfish/starfish-sync";

/// Mirrors `system::SUDO_GROUP`. The helper is a binary crate with nothing to
/// import from, so the name is repeated here; the assertions below fail loudly
/// if the two ever drift.
const SUDO_GROUP: &str = "starfish-sudo";

/// Mirrors `system::MANAGED_TAG`, repeated here for the same reason.
const MANAGED_TAG: &str = "STARFISH";

/// The database ids the users below have. Arbitrary, except that they differ:
/// the id, not the login name, is what an account is recognised by.
const ADA_ID: u64 = 42;
const LOCAL_DEV_ID: u64 = 99;

/// Mirrors `system::gecos`: the whole `GECOS` field a managed account has.
fn gecos(full_name: &str, id: u64) -> String {
    format!("{full_name},,,,{MANAGED_TAG}-{id}")
}

/// A container that removes itself, so a failing assertion does not leave one
/// running.
struct Container {
    name: String,
}

impl Container {
    fn start() -> Self {
        let name = format!("starfish-test-{}", std::process::id());

        // Left over from a run that was killed rather than allowed to finish.
        let _ = Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .expect("docker is not runnable");

        let started = Command::new("docker")
            .args(["run", "-d", "--name", &name, IMAGE])
            .output()
            .expect("unable to run docker");

        assert!(
            started.status.success(),
            "unable to start the test container. Build the image first with `just test-ubuntu`.\n{}",
            String::from_utf8_lossy(&started.stderr)
        );

        Self { name }
    }

    /// Runs a command in the container and returns its standard output,
    /// insisting it succeeded.
    fn exec(&self, args: &[&str]) -> String {
        let (status, stdout, stderr) = self.try_exec(None, args, &[]);

        assert!(
            status.success(),
            "`{}` failed in the container: {}",
            args.join(" "),
            String::from_utf8_lossy(&stderr)
        );

        String::from_utf8_lossy(&stdout).to_string()
    }

    /// Runs a command in the container as `user`, feeding it `stdin`, without
    /// insisting on success.
    fn try_exec(
        &self,
        user: Option<&str>,
        args: &[&str],
        stdin: &[u8],
    ) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
        // No `-t`: a TTY would translate newlines and corrupt MessagePack.
        let mut command = Command::new("docker");

        command.args(["exec", "-i"]);

        // `--user` is an argument to `docker exec`, so it has to come before
        // the container name; after it, docker would treat it as the command.
        if let Some(user) = user {
            command.args(["--user", user]);
        }

        command.arg(&self.name);
        command.args(args);

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("unable to run docker exec");

        child
            .stdin
            .take()
            .expect("stdin was piped")
            .write_all(stdin)
            .expect("unable to write to the helper");

        let output = child
            .wait_with_output()
            .expect("docker exec never finished");

        (output.status, output.stdout, output.stderr)
    }

    /// Applies a configuration as root, the way the agent's `sudo` would.
    fn sync(&self, config: &HostConfig) -> SyncReport {
        self.sync_as(None, &[HELPER], config)
    }

    fn sync_as(&self, user: Option<&str>, args: &[&str], config: &HostConfig) -> SyncReport {
        let input = starfish_msg::to_vec(config).expect("unable to encode the configuration");
        let (status, stdout, stderr) = self.try_exec(user, args, &input);

        assert!(
            status.success(),
            "`{}` failed with {status}\nstderr: {}\nstdout: {} bytes",
            args.join(" "),
            String::from_utf8_lossy(&stderr),
            stdout.len()
        );

        starfish_msg::from_slice(&stdout).expect("unable to decode the report")
    }

    /// The `passwd` entry for a user, split on `:`, or `None` if there is none.
    fn passwd(&self, user: &str) -> Option<Vec<String>> {
        let (status, stdout, _) = self.try_exec(None, &["getent", "passwd", user], &[]);

        if !status.success() {
            return None;
        }

        Some(
            String::from_utf8_lossy(&stdout)
                .trim_end()
                .split(':')
                .map(str::to_string)
                .collect(),
        )
    }

    fn groups(&self, user: &str) -> Vec<String> {
        self.exec(&["id", "--name", "--groups", user])
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    fn exists(&self, path: &str) -> bool {
        self.try_exec(None, &["test", "-e", path], &[]).0.success()
    }

    /// Owner and mode of a path, as `user:group mode`.
    fn stat(&self, path: &str) -> String {
        self.exec(&["stat", "-c", "%U:%G %a", path])
            .trim()
            .to_string()
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

fn config(groups: &[&str], users: Vec<UserAccount>) -> HostConfig {
    HostConfig {
        hostname: "web-1".to_string(),
        generation: 1,
        groups: groups
            .iter()
            .map(|name| Group {
                name: name.to_string(),
            })
            .collect(),
        users,
    }
}

fn ada(groups: &[&str], is_sudoer: bool) -> UserAccount {
    UserAccount {
        id: ADA_ID,
        name: "ada".to_string(),
        full_name: "Ada Lovelace".to_string(),
        email: "ada@example.com".to_string(),
        is_sudoer,
        groups: groups.iter().map(|g| g.to_string()).collect(),
        ssh_keys: vec![SshKey {
            name: "laptop".to_string(),
            key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5-ada-laptop ada@laptop".to_string(),
        }],
    }
}

/// A user with no keys, for the accounts whose point is whether they exist at
/// all rather than who can log into them.
fn account(id: u64, name: &str, full_name: &str) -> UserAccount {
    UserAccount {
        id,
        name: name.to_string(),
        full_name: full_name.to_string(),
        email: format!("{name}@example.com"),
        is_sudoer: false,
        groups: vec![],
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

/// One test, run in order against one container, because each step is the
/// starting state of the next: that is how a real host is synchronized.
#[test]
fn synchronizes_a_real_ubuntu_host() {
    if std::env::var("STARFISH_TEST_DOCKER").is_err() {
        eprintln!("skipping: STARFISH_TEST_DOCKER is not set");
        return;
    }

    let host = Container::start();
    let base = config(
        &["developers", "deploy"],
        vec![ada(&["developers", "deploy"], false)],
    );

    // --- a group the host does not have -------------------------------------
    // Groups are administered outside Starfish, so the very first sync finds
    // neither of these and must not make them.  The user is still created:
    // a group the host does not have is nobody's failure.
    let report = host.sync(&base);

    assert_eq!(status(&report, "developers"), Status::Missing);
    assert_eq!(status(&report, "deploy"), Status::Missing);
    assert_eq!(status(&report, "ada"), Status::Created);
    assert!(
        host.try_exec(None, &["getent", "group", "developers"], &[])
            .0
            .code()
            == Some(2),
        "the helper created a group"
    );

    let groups = host.groups("ada");

    assert!(!groups.contains(&"developers".to_string()), "{groups:?}");

    // --- the groups exist, put there by whoever administers this host -------
    host.exec(&["groupadd", "developers"]);
    host.exec(&["groupadd", "deploy"]);

    let report = host.sync(&base);

    assert_eq!(status(&report, "developers"), Status::Unchanged);
    assert_eq!(status(&report, "deploy"), Status::Unchanged);
    assert_eq!(status(&report, "ada"), Status::Updated);

    let passwd = host.passwd("ada").expect("ada was not created");

    // The tag in the last GECOS field is what every later sync recognises the
    // account by, and the full name still has the first field to itself.
    assert_eq!(
        passwd[4],
        gecos("Ada Lovelace", ADA_ID),
        "the GECOS field is wrong"
    );
    assert_eq!(passwd[5], "/home/ada");
    assert_eq!(passwd[6], "/bin/bash");
    assert!(
        passwd[2].parse::<u32>().unwrap() >= 1000,
        "ada should not be a system account: {passwd:?}"
    );

    let groups = host.groups("ada");

    assert!(groups.contains(&"developers".to_string()), "{groups:?}");
    assert!(groups.contains(&"deploy".to_string()), "{groups:?}");

    // SSH ignores a key file with the wrong owner or mode, so this is the part
    // that decides whether anybody can actually log in.
    assert_eq!(host.stat("/home/ada/.ssh"), "ada:ada 700");
    assert_eq!(host.stat("/home/ada/.ssh/authorized_keys"), "ada:ada 600");
    assert!(
        host.exec(&["cat", "/home/ada/.ssh/authorized_keys"])
            .contains("ada-laptop"),
        "the key was not installed"
    );

    // --- a second run should find nothing to do -----------------------------
    let report = host.sync(&base);

    assert_eq!(status(&report, "developers"), Status::Unchanged);
    assert_eq!(status(&report, "ada"), Status::Unchanged);

    // --- granting and revoking sudo -----------------------------------------
    let sudoer = config(
        &["developers", "deploy"],
        vec![ada(&["developers", "deploy"], true)],
    );

    assert_eq!(status(&host.sync(&sudoer), "ada"), Status::Updated);
    assert!(
        host.groups("ada").contains(&SUDO_GROUP.to_string()),
        "sudo was not granted: {:?}",
        host.groups("ada")
    );

    // Membership is not the point; being able to run something is.  Managed
    // accounts have no password, so without the NOPASSWD rule in
    // deploy/starfish-sudoers this is a group they sit in uselessly.
    let (status_code, _, stderr) = host.try_exec(Some("ada"), &["sudo", "-n", "/bin/true"], b"");

    assert!(
        status_code.success(),
        "a granted sudoer could not actually use sudo: {}",
        String::from_utf8_lossy(&stderr)
    );

    assert_eq!(status(&host.sync(&base), "ada"), Status::Updated);
    assert!(
        !host.groups("ada").contains(&SUDO_GROUP.to_string()),
        "sudo was not revoked: {:?}",
        host.groups("ada")
    );

    // ...and revoking it takes the capability away, not just the membership.
    let (status_code, _, _) = host.try_exec(Some("ada"), &["sudo", "-n", "/bin/true"], b"");

    assert!(
        !status_code.success(),
        "sudo still worked after it was revoked"
    );

    // Ubuntu's own `sudo` group is left out of this entirely, so a local
    // administrator already in it is unaffected by anything Starfish does.
    assert!(
        !host.groups("ada").contains(&"sudo".to_string()),
        "a managed sudoer was put in Ubuntu's sudo group: {:?}",
        host.groups("ada")
    );

    // --- a group Starfish was never told about is left alone ----------------
    host.exec(&["groupadd", "docker"]);
    host.exec(&["gpasswd", "--add", "ada", "docker"]);

    assert_eq!(status(&host.sync(&base), "ada"), Status::Unchanged);
    assert!(
        host.groups("ada").contains(&"docker".to_string()),
        "an unmanaged group was taken away: {:?}",
        host.groups("ada")
    );

    // --- but membership of a managed group is removed -----------------------
    let dropped = config(&["developers", "deploy"], vec![ada(&["developers"], false)]);

    assert_eq!(status(&host.sync(&dropped), "ada"), Status::Updated);

    let groups = host.groups("ada");

    assert!(!groups.contains(&"deploy".to_string()), "{groups:?}");
    assert!(groups.contains(&"developers".to_string()), "{groups:?}");
    assert!(groups.contains(&"docker".to_string()), "{groups:?}");

    // --- a changed full name is corrected -----------------------------------
    let mut renamed = dropped.clone();

    renamed.users[0].full_name = "Ada A. Lovelace".to_string();

    assert_eq!(status(&host.sync(&renamed), "ada"), Status::Updated);
    assert_eq!(
        host.passwd("ada").unwrap()[4],
        gecos("Ada A. Lovelace", ADA_ID)
    );

    // --- a group deleted on the host while a user was in it -----------------
    // `groupdel` takes the memberships with it, so the next sync finds ada out
    // of a group the configuration still puts her in.  Starfish reports it and
    // leaves it alone rather than putting the group back.
    assert!(host.groups("ada").contains(&"developers".to_string()));
    host.exec(&["groupdel", "developers"]);

    let report = host.sync(&renamed);

    assert_eq!(status(&report, "developers"), Status::Missing);
    assert_eq!(
        host.try_exec(None, &["getent", "group", "developers"], &[])
            .0
            .code(),
        Some(2),
        "the deleted group was recreated"
    );
    // The rest of the account is untouched by its group going missing.
    assert_eq!(
        host.passwd("ada").unwrap()[4],
        gecos("Ada A. Lovelace", ADA_ID)
    );
    assert!(
        host.exec(&["cat", "/home/ada/.ssh/authorized_keys"])
            .contains("ada-laptop")
    );

    // Put back by the host's administrator, and the next sync rejoins her.
    host.exec(&["groupadd", "developers"]);

    assert_eq!(status(&host.sync(&renamed), "ada"), Status::Updated);
    assert!(
        host.groups("ada").contains(&"developers".to_string()),
        "membership was not restored: {:?}",
        host.groups("ada")
    );

    // --- a system account is refused ----------------------------------------
    let before = host.passwd("daemon").expect("daemon should exist");
    let mut system = ada(&[], true);

    // A user of its own rather than ada's id, so this exercises a
    // configuration reaching for a system account and not a rename onto one,
    // which is refused a little further down.
    system.id = 7;
    system.name = "daemon".to_string();

    let report = host.sync(&config(&[], vec![system]));

    assert!(
        matches!(status(&report, "daemon"), Status::Failed { message } if message.contains("system account")),
        "unexpected status: {:?}",
        status(&report, "daemon")
    );
    assert_eq!(
        host.passwd("daemon").unwrap(),
        before,
        "a system account was modified"
    );

    // --- a name that would be read as an option is refused -------------------
    let mut injected = ada(&[], true);

    injected.name = "--uid=0".to_string();

    let report = host.sync(&config(&["-o"], vec![injected]));

    assert!(matches!(status(&report, "-o"), Status::Failed { .. }));
    assert!(matches!(status(&report, "--uid=0"), Status::Failed { .. }));

    let root_accounts = host.exec(&["awk", "-F:", "$3 == 0 { print $1 }", "/etc/passwd"]);

    assert_eq!(
        root_accounts.split_whitespace().collect::<Vec<_>>(),
        vec!["root"],
        "something else acquired uid 0"
    );

    // --- an untagged account is adopted, an unmentioned one is left alone ----
    // Every account on a host upgraded to a tagging agent looks like
    // `localdev`: made by Starfish, but before there was a tag to put on it.
    // `localadmin` is the other case entirely — somebody's own account, which
    // no configuration names and nothing here may touch.
    host.exec(&[
        "useradd",
        "--create-home",
        "--comment",
        "Local Developer",
        "localdev",
    ]);
    host.exec(&[
        "useradd",
        "--create-home",
        "--comment",
        "Local Admin",
        "localadmin",
    ]);

    let mut adopted = base.clone();

    adopted
        .users
        .push(account(LOCAL_DEV_ID, "localdev", "Local Developer"));

    let report = host.sync(&adopted);

    assert_eq!(status(&report, "localdev"), Status::Updated);
    assert_eq!(
        host.passwd("localdev").unwrap()[4],
        gecos("Local Developer", LOCAL_DEV_ID),
        "an existing account was not adopted"
    );
    assert_eq!(
        host.passwd("localadmin").unwrap()[4],
        "Local Admin",
        "an account no configuration names was tagged"
    );

    // --- a changed alias renames the account rather than rebuilding it -------
    // This is the whole reason the tag carries an id. `ada` becomes `adalove`
    // in the database, and the account has to follow it with its uid and its
    // home directory intact, not be deleted and made again.
    let before = host.passwd("ada").expect("ada should exist");

    let mut realiased = adopted.clone();

    realiased.users[0].name = "adalove".to_string();

    let report = host.sync(&realiased);

    assert_eq!(status(&report, "adalove"), Status::Updated);
    assert!(
        host.passwd("ada").is_none(),
        "the account is still under its old login name"
    );

    let after = host.passwd("adalove").expect("the account was not renamed");

    assert_eq!(
        after[2], before[2],
        "the uid changed, so this was a rebuild"
    );
    assert_eq!(after[5], "/home/adalove", "the home directory did not move");
    // `base`, which this was built from, carries the original full name, so the
    // tag is the only part of the GECOS that had to survive the rename.
    assert_eq!(after[4], gecos("Ada Lovelace", ADA_ID));
    assert!(!host.exists("/home/ada"), "the old home was left behind");

    // The keys moved with the home directory, so the rename did not lock
    // anybody out of a host they still have access to.
    assert!(
        host.exec(&["cat", "/home/adalove/.ssh/authorized_keys"])
            .contains("ada-laptop"),
        "the key file did not survive the rename"
    );

    // The account's private group keeps the name the login had when it was
    // created: renaming it would mean administering a group, which this helper
    // deliberately cannot do. Nothing depends on the two names matching,
    // because the key file is chowned to the login group rather than to a
    // group assumed to be named after the user.
    let login_group = host.exec(&["id", "--name", "--group", "adalove"]);

    assert_eq!(
        host.stat("/home/adalove/.ssh/authorized_keys"),
        format!("adalove:{} 600", login_group.trim())
    );

    // --- a user who leaves the configuration loses the account --------------
    let report = host.sync(&config(&["developers", "deploy"], vec![]));

    assert_eq!(status(&report, "adalove"), Status::Removed);
    assert_eq!(status(&report, "localdev"), Status::Removed);
    assert!(host.passwd("adalove").is_none());
    assert!(host.passwd("localdev").is_none());
    assert!(
        !host.exists("/home/adalove") && !host.exists("/home/localdev"),
        "a removed account kept its home directory"
    );

    // ...but an account Starfish never created is none of its business, however
    // little the configuration says about it.
    assert!(
        host.passwd("localadmin").is_some() && host.exists("/home/localadmin"),
        "an account without the tag was removed"
    );
    assert!(
        report.users.iter().all(|item| item.name != "localadmin"),
        "an unmanaged account was reported on: {:?}",
        report.users
    );

    // --- the sudoers rule from deploy/ actually works ------------------------
    // This is the whole point of the privilege split: the unprivileged account
    // can run this one command, and nothing else.
    let report = host.sync_as(Some("starfish"), &["sudo", "-n", HELPER], &base);

    assert_eq!(status(&report, "developers"), Status::Unchanged);

    let (status_code, _, stderr) = host.try_exec(
        Some("starfish"),
        &["sudo", "-n", "/usr/sbin/useradd", "evil"],
        &[],
    );

    assert!(!status_code.success(), "the sudo rule allowed useradd");
    assert!(
        String::from_utf8_lossy(&stderr).contains("password is required"),
        "unexpected sudo refusal: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(host.passwd("evil").is_none(), "'evil' was created");
}
