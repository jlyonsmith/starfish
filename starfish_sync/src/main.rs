//! The privileged half of the Starfish agent.
//!
//! This is the only Starfish program that needs root. It reads a `HostConfig`
//! as MessagePack on standard input, applies it, and writes a `SyncReport` back
//! on standard output. It takes no arguments, which is what lets the whole
//! `sudo` policy for the agent be a single line with nothing to get wrong:
//!
//! ```text
//! starfish ALL=(root) NOPASSWD: /usr/local/lib/starfish/starfish-sync
//! ```
//!
//! Everything that decides *what* to change lives here rather than in the
//! unprivileged agent, so a compromised agent can ask for a synchronization but
//! cannot choose which commands root runs.

mod sync;
mod system;
mod validate;

use anyhow::{Context, bail};
use starfish_msg::HostConfig;
use std::io::{Read, Write};

fn main() -> anyhow::Result<()> {
    // Failing once, clearly, beats every single operation failing on
    // permissions and burying the real problem in the report.
    let uid = unsafe { libc::geteuid() };

    if uid != 0 {
        bail!(
            "starfish-sync must run as root, but is running as uid {uid}. \
             The agent normally starts it with sudo."
        );
    }

    let mut input = Vec::new();

    std::io::stdin()
        .read_to_end(&mut input)
        .context("Unable to read the configuration from standard input")?;

    let output = apply(&system::Ubuntu, &input)?;

    std::io::stdout()
        .write_all(&output)
        .context("Unable to write the report to standard output")?;

    Ok(())
}

/// Decodes a configuration, applies it, and encodes the report.
///
/// Kept apart from `main` so the wire handling can be tested without a host to
/// change or a root process to run as.
fn apply(system: &dyn system::System, input: &[u8]) -> anyhow::Result<Vec<u8>> {
    let config: HostConfig =
        starfish_msg::from_slice(input).context("Unable to decode the configuration")?;

    let report = sync::sync(system, &config);

    starfish_msg::to_vec(&report).context("Unable to encode the report")
}

#[cfg(test)]
mod tests {
    use super::*;
    use starfish_msg::{Group, SyncReport, UserAccount};

    #[test]
    fn round_trips_a_configuration_into_a_report() {
        let config = HostConfig {
            hostname: "web-1".to_string(),
            generation: 9,
            groups: vec![Group {
                name: "developers".to_string(),
            }],
            users: vec![UserAccount {
                name: "ada".to_string(),
                full_name: "Ada Lovelace".to_string(),
                email: "ada@example.com".to_string(),
                is_sudoer: true,
                groups: vec!["developers".to_string()],
                ssh_keys: vec![],
            }],
        };

        let input = starfish_msg::to_vec(&config).unwrap();
        let output = apply(&sync::tests::FakeSystem::default(), &input).unwrap();
        let report: SyncReport = starfish_msg::from_slice(&output).unwrap();

        assert_eq!(report.hostname, "web-1");
        assert_eq!(report.generation, 9);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.users.len(), 1);
        assert_eq!(report.groups[0].name, "developers");
        assert_eq!(report.users[0].name, "ada");
    }

    #[test]
    fn refuses_input_that_is_not_a_configuration() {
        let err = apply(&sync::tests::FakeSystem::default(), b"not messagepack").unwrap_err();

        assert!(
            format!("{err:#}").contains("Unable to decode"),
            "unexpected error: {err:#}"
        );
    }
}
