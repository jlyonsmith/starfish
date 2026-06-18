//#![warn(unused_crate_dependencies)]

mod config_file;
mod server;
mod server_args;
mod server_config;

pub use config_file::ConfigFile;
pub use server::Server;
pub use server_args::ServerArgs;
pub use server_config::ServerConfig;

use anyhow::Context;
use clap::Parser;
use single_instance::SingleInstance;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match ServerArgs::try_parse() {
        Ok(m) => m,
        Err(err) => {
            // Help and version come back as an error
            eprintln!("{}", err.to_string());
            return Ok(());
        }
    };

    let bin_name = env!("CARGO_PKG_NAME");

    let instance = SingleInstance::new(bin_name).context("Unable to check for running instance")?;

    if !instance.is_single() {
        eprintln!("Another instance of '{}' is already running.", bin_name);
        return Ok(());
    }

    let config_file = ConfigFile::load(&args.config_path).context(format!(
        "Unable to load config file: {}",
        args.config_path.display()
    ))?;
    let config = ServerConfig::with_config_and_args(&config_file, &args);

    // Handle async construction of the server
    Server::new(&config).run().await?;

    Ok(())
}
