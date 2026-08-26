use anyhow::Context;
use clap::Parser;
use figment::{
    Figment,
    providers::{Format, Serialized, Toml},
};
use single_instance::SingleInstance;

mod agent;
mod agent_args;
mod agent_config;

use agent::Agent;
use agent_args::*;
pub use agent_config::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match AgentArgs::try_parse() {
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

    let config = Figment::new()
        .merge(Serialized::defaults(AgentConfig::default()))
        .merge(Serialized::defaults(&args))
        .merge(Toml::file(&args.config_path))
        .extract()?;

    // Handle async construction of the server
    Agent::new(&config).run().await?;

    Ok(())
}
