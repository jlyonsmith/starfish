use clap::Parser;

#[derive(Parser)]
#[command(version, about = "Starfish agent")]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
