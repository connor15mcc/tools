use clap::{Args, Parser, Subcommand};
use clap_stdin::FileOrStdin;
use decay::*;
use git::{poor_mans_refactorator, tidy_merged_go_mod};
use ilimit::interactive_tail;
use petname::Generator;
use resolve_path::PathResolveExt;
use std::io::BufReader;
use std::path::PathBuf;

mod decay;
mod git;
mod ilimit;

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
    Decay {
        /// Rate with which to decay / depreciate older values (annual)
        #[arg(short, long)]
        rate: Option<f64>,
    },
    Petname,
    Ilimit(IlimitCommand),
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct PoorMansRefactorator {
    /// ID of this upsert "idea" (to enable re-using PRs)
    #[arg(short, long)]
    id: Option<String>,

    #[clap(flatten)]
    pr_info: Option<PrInfo>,

    /// True to skip creating a PR and instead print a diff summary to stdout
    #[arg(long)]
    dry_run: bool,

    /// Command that will be invoked on each repo, with the resulting changes
    /// applied in a commit + put for review
    #[arg(required = true)]
    command: String,

    #[arg(short = 'p', long)]
    checkout_path: Option<PathBuf>,

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

#[derive(Parser)]
struct IlimitCommand {
    /// Number of lines to display from the end of input
    #[arg(long, default_value_t = 10)]
    limit: usize,

    /// File from which to read, defaulting to stdin
    #[clap(default_value = "-")]
    input: FileOrStdin,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::GoModMerge => tidy_merged_go_mod().expect("couldn't tidy go.mod / go.sum"),
        Commands::Pmr(PoorMansRefactorator {
            id,
            pr_info,
            dry_run,
            command,
            checkout_path,
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
            let checkout_path = checkout_path.unwrap_or("~/mono/".resolve().to_path_buf());
            poor_mans_refactorator(
                &id,
                pr_info.as_ref(),
                dry_run,
                &command,
                reader,
                &checkout_path,
            )
            .expect("couldn't create / update all PRs")
        }
        Commands::Decay { rate } => {
            let score = score(InterestRate::new(rate)).expect("couldn't calculate the score");
            println!("Decay score: {score:.2}")
        }
        Commands::Petname => {
            let name = petname::Petnames::default()
                .generate_one(2, "-")
                .expect("couldn't generate name");
            println!("{}", name);
        }
        Commands::Ilimit(IlimitCommand { limit, input }) => {
            let reader = input.into_reader().expect("failed to convert to reader");
            interactive_tail(reader, limit).expect("failed to tail input");
        }
    }
}
