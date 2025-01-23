mod git;
use clap::{Parser, Subcommand};
use git::{list_recent_reviews, tidy_merged_go_mod};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Git,
    GoModMerge,
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Git => list_recent_reviews().expect("request should succeed"),
        Commands::GoModMerge => tidy_merged_go_mod().expect("couldn't tidy go.mod / go.sum"),
    }
}
