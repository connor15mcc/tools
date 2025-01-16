mod git;
use clap::{Parser, Subcommand};
use git::list_recent_reviews;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Git,
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Git => list_recent_reviews().expect("request should succeed"),
    }
}
