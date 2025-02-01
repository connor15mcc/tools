mod git;
use clap::{Parser, Subcommand, Args};
use clap_stdin::FileOrStdin;
use std::io::BufReader;
use git::{list_recent_reviews, tidy_merged_go_mod, poor_mans_refactorator};
use petname::Generator;

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
    Pmr(PoorMansRefactorator),
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct PoorMansRefactorator {
    /// ID of this upsert "idea" (to enable re-using PRs)
    #[arg(short, long)]
    id: Option<String>,

    #[clap(flatten)]
    pr_info: Option<PrInfo>,

    /// Command that will be invoked on each repo, with the resulting changes
    /// applied in a commit + put for review
    #[arg(required = true)]
    command: String,

    /// File from which to read, new-line separated
    #[clap(default_value="-")]
    repos_file: FileOrStdin,
}

#[derive(Args)]
struct PrInfo {
    /// Title to use for newly-created PRs
    #[arg(short, long, required = true)]
    title: String,

    /// Body to use for newly-created PRs
    #[arg(short, long, required = true)]
    body: String,
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Git => list_recent_reviews().expect("request should succeed"),
        Commands::GoModMerge => tidy_merged_go_mod().expect("couldn't tidy go.mod / go.sum"),
        Commands::Pmr(PoorMansRefactorator{id, pr_info, command, repos_file}) => {
            let id = id.to_owned().unwrap_or_else(|| {
                petname::Petnames::default().generate_one(2, "-").expect("couldn't generate RNG name")
            });
            let reader = BufReader::new(repos_file.clone().into_reader().expect("failed to convert to reader"));
            poor_mans_refactorator(&id, pr_info.as_ref(), command, reader).expect("couldn't create / update all PRs")
        }
    }
}
