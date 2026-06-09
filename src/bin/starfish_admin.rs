use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Starfish administration tool")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {}

fn main() {
    let _cli = Cli::parse();
}
