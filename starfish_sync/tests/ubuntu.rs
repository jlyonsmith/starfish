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

    // --- creating what is missing -------------------------------------------
    let report = host.sync(&base);

    assert_eq!(status(&report, "developers"), Status::Created);
    assert_eq!(status(&report, "deploy"), Status::Created);
    assert_eq!(status(&report, "ada"), Status::Created);

    let passwd = host.passwd("ada").expect("ada was not created");

    assert_eq!(passwd[4], "Ada Lovelace", "the GECOS field is wrong");
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
        host.groups("ada").contains(&"sudo".to_string()),
        "sudo was not granted: {:?}",
        host.groups("ada")
    );

    assert_eq!(status(&host.sync(&base), "ada"), Status::Updated);
    assert!(
        !host.groups("ada").contains(&"sudo".to_string()),
        "sudo was not revoked: {:?}",
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
    assert_eq!(host.passwd("ada").unwrap()[4], "Ada A. Lovelace");

    // --- a system account is refused ----------------------------------------
    let before = host.passwd("daemon").expect("daemon should exist");
    let mut system = ada(&[], true);

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
