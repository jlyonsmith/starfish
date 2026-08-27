use clap::Parser;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::net::SocketAddr;
use std::path::PathBuf;
use url::Url;

#[skip_serializing_none]
#[derive(Parser, Deserialize, Serialize)]
#[clap(version, about, long_about = None)]
pub struct ServerArgs {
    /// Config file path
    #[arg(long = "config", short = 'c', default_value = "/etc/starfishd.conf")]
    #[serde(skip)]
    pub config_path: PathBuf,

    /// Address of the PostgreSQL server
    #[arg(long)]
    pub sql_server: Option<Url>,

    /// File holding the database password, mode 0600
    #[arg(long)]
    pub password_file: Option<PathBuf>,

    /// Address to listen on for agent connections
    #[arg(long)]
    pub listen: Option<SocketAddr>,

    /// PEM certificate chain to serve agents over TLS
    #[arg(long, requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,

    /// PEM private key matching the TLS certificate
    #[arg(long, requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,

    /// Unix domain socket to accept administration commands on
    #[arg(long)]
    pub admin_socket: Option<PathBuf>,

    /// Group allowed to use the administration socket
    #[arg(long)]
    pub admin_socket_group: Option<String>,

    /// Set the logging level (e.g., info, debug, trace)
    #[arg(long)]
    pub log_level: Option<LevelFilter>,
}
