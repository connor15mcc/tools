mod git;
use clap::{Args, Parser, Subcommand};
use clap_stdin::FileOrStdin;
use git::{poor_mans_refactorator, tidy_merged_go_mod};
use petname::Generator;
use std::io::BufReader;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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

    // True to skip creating a PR and instead print a diff summary to stdout
    #[arg(long)]
    dry_run: bool,

    /// Command that will be invoked on each repo, with the resulting changes
    /// applied in a commit + put for review
    #[arg(required = true)]
    command: String,

    /// File from which to read, new-line separated
    #[clap(default_value = "-")]
    repos_file: FileOrStdin,
}

#[derive(Args, Debug)]
#[group(requires_all = ["title", "body"])] // https://github.com/clap-rs/clap/issues/5092
struct PrInfo {
    /// Title to use for newly-created PRs
    #[arg(short, long, required = false)]
    title: String,

    /// Body to use for newly-created PRs
    #[arg(short, long, required = false)]
    body: String,
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::GoModMerge => tidy_merged_go_mod().expect("couldn't tidy go.mod / go.sum"),
        Commands::Pmr(PoorMansRefactorator {
            id,
            pr_info,
            dry_run,
            command,
            repos_file,
        }) => {
            let id = id.to_owned().unwrap_or_else(|| {
                petname::Petnames::default()
                    .generate_one(2, "-")
                    .expect("couldn't generate RNG name")
            });
            let reader = BufReader::new(
                repos_file
                    .clone()
                    .into_reader()
                    .expect("failed to convert to reader"),
            );
            poor_mans_refactorator(&id, pr_info.as_ref(), *dry_run, command, reader)
                .expect("couldn't create / update all PRs")
        }
    }
}
