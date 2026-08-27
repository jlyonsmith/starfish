//! The Starfish wire protocol.
//!
//! Every message is encoded as MessagePack. The controller and its agents
//! exchange [`ControllerMsg`] and [`AgentMsg`] over a WebSocket, which frames
//! messages itself; `starfish_admin` exchanges [`AdminRequest`] and
//! [`AdminResponse`] with the controller over a Unix domain socket, which does
//! not, so those use the length prefixed [`frame`] helpers.

mod admin;
mod agent;
mod agent_key;
pub mod frame;

pub use admin::*;
pub use agent::*;
pub use agent_key::AgentKey;

use serde::{Serialize, de::DeserializeOwned};

/// The protocol an agent reports in its [`Hello`]. The controller rejects
/// agents that do not match.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Unable to encode message")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("Unable to decode message")]
    Decode(#[from] rmp_serde::decode::Error),

    #[error("Agent key must be {len} alphanumeric characters, got '{0}'", len = AgentKey::LEN)]
    InvalidAgentKey(String),

    #[error("Unable to generate an agent key")]
    KeyGeneration(#[from] getrandom::Error),

    #[error("Message of {0} bytes exceeds the {max} byte limit", max = frame::MAX_LEN)]
    FrameTooLarge(usize),

    #[error("Unable to read or write a message")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Encodes a message as MessagePack.
///
/// Fields are encoded by name so that adding a field to a message does not
/// break a peer built against an older version of this crate.
pub fn to_vec<T: Serialize>(msg: &T) -> Result<Vec<u8>> {
    Ok(rmp_serde::to_vec_named(msg)?)
}

/// Decodes a message encoded by [`to_vec`].
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(rmp_serde::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn round_trips_a_host_config() {
        let config = ControllerMsg::Config(HostConfig {
            hostname: "web-1".to_string(),
            generation: 7,
            groups: vec![Group {
                name: "developers".to_string(),
            }],
            users: vec![UserAccount {
                name: "jls".to_string(),
                full_name: "John Lyon-Smith".to_string(),
                email: "john@lyon-smith.org".to_string(),
                is_sudoer: true,
                groups: vec!["developers".to_string()],
                ssh_keys: vec![SshKey {
                    name: "laptop".to_string(),
                    key: "ssh-ed25519 AAAA".to_string(),
                }],
            }],
        });

        let bytes = to_vec(&config).unwrap();
        let decoded: ControllerMsg = from_slice(&bytes).unwrap();

        assert_eq!(format!("{config:?}"), format!("{decoded:?}"));
    }

    #[test]
    fn round_trips_a_sync_report() {
        let report = AgentMsg::SyncReport(SyncReport {
            hostname: "web-1".to_string(),
            generation: 7,
            groups: vec![ItemReport {
                name: "developers".to_string(),
                status: Status::Created,
            }],
            users: vec![ItemReport {
                name: "jls".to_string(),
                status: Status::Failed {
                    message: "useradd exited with 1".to_string(),
                },
            }],
        });

        let bytes = to_vec(&report).unwrap();
        let decoded: AgentMsg = from_slice(&bytes).unwrap();

        assert_eq!(format!("{report:?}"), format!("{decoded:?}"));
    }

    #[test]
    fn round_trips_a_heartbeat() {
        let heartbeat = AgentMsg::Heartbeat(Heartbeat {
            next_in: Duration::from_secs(300),
        });

        let bytes = to_vec(&heartbeat).unwrap();
        let decoded: AgentMsg = from_slice(&bytes).unwrap();

        let AgentMsg::Heartbeat(decoded) = decoded else {
            panic!("expected a heartbeat, got {decoded:?}");
        };

        assert_eq!(decoded.next_in, Duration::from_secs(300));
    }

    #[test]
    fn round_trips_an_admin_exchange() {
        let request = AdminRequest::Refresh {
            hostname: Some("web-1".to_string()),
        };
        let decoded: AdminRequest = from_slice(&to_vec(&request).unwrap()).unwrap();

        assert_eq!(format!("{request:?}"), format!("{decoded:?}"));

        let response = AdminResponse::Refreshed {
            notified: vec!["web-1".to_string()],
            offline: vec![],
        };
        let decoded: AdminResponse = from_slice(&to_vec(&response).unwrap()).unwrap();

        assert_eq!(format!("{response:?}"), format!("{decoded:?}"));
    }

    /// A peer built against an older version of a message must still decode a
    /// message carrying a field it does not know about.
    #[test]
    fn ignores_unknown_fields() {
        #[derive(serde::Serialize)]
        struct NewHello {
            protocol_version: u32,
            agent_key: AgentKey,
            hostname: String,
            agent_version: String,
            os_release: String,
        }

        let bytes = to_vec(&NewHello {
            protocol_version: PROTOCOL_VERSION,
            agent_key: AgentKey::generate().unwrap(),
            hostname: "web-1".to_string(),
            agent_version: "0.1.0".to_string(),
            os_release: "24.04".to_string(),
        })
        .unwrap();

        let hello: Hello = from_slice(&bytes).unwrap();

        assert_eq!(hello.hostname, "web-1");
    }
}
