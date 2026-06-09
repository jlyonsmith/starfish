use clap::Parser;

#[derive(Parser)]
#[command(version, about = "Starfish daemon")]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
