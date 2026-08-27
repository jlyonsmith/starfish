use log::LevelFilter;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use url::Url;

/// Listens on every interface by default: agents connect from across the local
/// network, so binding to loopback would leave the controller unreachable.
const DEFAULT_LISTEN: &str = "0.0.0.0:9600";

const DEFAULT_ADMIN_SOCKET: &str = "/run/starfishd.sock";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// The PostgreSQL database holding the configuration. Required: there is no
    /// database a controller could sensibly guess at.
    pub sql_server: Url,

    /// File holding the database password, so it never appears in `ps` output
    /// or shell history. Takes precedence over a password in `sql_server`.
    #[serde(default)]
    pub password_file: Option<PathBuf>,

    /// Address the agent WebSocket server listens on.
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,

    /// PEM certificate chain to serve agents over TLS. Agents connect over
    /// plain `ws://` when this and `tls_key` are both unset.
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key matching `tls_cert`.
    #[serde(default)]
    pub tls_key: Option<PathBuf>,

    /// Unix domain socket `starfish_admin` sends commands to.
    #[serde(default = "default_admin_socket")]
    pub admin_socket: PathBuf,

    /// Group whose members may use the administration socket. Without one the
    /// socket is mode 0600 and only the controller's own user can reach it.
    #[serde(default)]
    pub admin_socket_group: Option<String>,

    #[serde(default = "default_log_level")]
    pub log_level: LevelFilter,
}

impl ServerConfig {
    /// The URL to connect to the database with, and a warning to log if the
    /// connection will not be verified.
    pub fn database_url(&self) -> anyhow::Result<(url::Url, Option<String>)> {
        let url = starfish_db::connection_url(&self.sql_server, self.password_file.as_deref())?;
        let warning = starfish_db::tls_warning(&url);

        Ok((url, warning))
    }

    /// The certificate and key to serve TLS with, or `None` to serve plain
    /// `ws://`.
    ///
    /// Configuring one without the other is a mistake worth stopping for
    /// rather than quietly falling back to an unencrypted listener.
    pub fn tls(&self) -> anyhow::Result<Option<(&PathBuf, &PathBuf)>> {
        match (&self.tls_cert, &self.tls_key) {
            (Some(cert), Some(key)) => Ok(Some((cert, key))),
            (None, None) => Ok(None),
            (Some(_), None) => anyhow::bail!("A TLS certificate was given with no matching key"),
            (None, Some(_)) => anyhow::bail!("A TLS key was given with no matching certificate"),
        }
    }
}

fn default_listen() -> SocketAddr {
    DEFAULT_LISTEN.parse().expect("the default listen address")
}

fn default_admin_socket() -> PathBuf {
    PathBuf::from(DEFAULT_ADMIN_SOCKET)
}

fn default_log_level() -> LevelFilter {
    LevelFilter::Info
}
