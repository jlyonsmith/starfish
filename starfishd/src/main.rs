//#![warn(unused_crate_dependencies)]

mod admin_socket;
mod agent_registry;
mod agent_session;
mod controller;
mod server;
mod server_args;
mod server_config;
mod tls;

use server_args::ServerArgs;

pub use server::Server;
pub use server_config::ServerConfig;

use anyhow::Context;
use clap::Parser;
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use single_instance::SingleInstance;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match ServerArgs::try_parse() {
        Ok(m) => m,
        Err(err) => {
            // Help and version come back as an error
            eprintln!("{err}");
            return Ok(());
        }
    };

    let bin_name = env!("CARGO_PKG_NAME");
    let instance = SingleInstance::new(bin_name).context("Unable to check for running instance")?;

    if !instance.is_single() {
        eprintln!("Another instance of '{}' is already running.", bin_name);
        return Ok(());
    }

    let config = Figment::new()
        .merge(Toml::file(&args.config_path))
        .merge(Serialized::defaults(&args))
        .extract()
        .with_context(|| {
            format!(
                "Unable to read the configuration from {} and the command line",
                args.config_path.display()
            )
        })?;

    // Handle async construction of the server
    Server::new(&config).run().await?;

    Ok(())
}
