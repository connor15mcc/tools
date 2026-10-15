use anyhow::{bail, Result};
use clap::Parser;
use xshell::{cmd, Shell};

use super::jj::{find_active_megamerge, list_change_ids};
use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "jj-stack",
    about = "Stack changes under megamerge, creating one if needed"
)]
pub struct JjStack {
    /// Revset to stage (change-id, commit-id, or any jj revset).
    #[arg(short = 'r', long = "revision")]
    revision: String,
}

impl CommandRunner for JjStack {
    fn run(self) -> Result<()> {
        let sh = Shell::new()?;
        let rev = self.revision.trim().to_string();
        validate_revision(&rev)?;

        let matched = list_change_ids(&sh, &rev)?;
        if matched.is_empty() {
            bail!("no commits match revset: {rev}");
        }

        // Resolve before mutating: expressions such as @- or heads(...) may
        // select different changes after parallelization or rebasing.
        let selected = matched.join(" | ");
        let mm = find_active_megamerge(&sh)?;

        match mm {
            Some(mm) => {
                let args = rebase_into_stack_args(&selected, &mm);
                sh.cmd("jj").args(&args).run()?;
            }
            None => {
                cmd!(sh, "jj parallelize {selected}").run()?;
                let heads = list_change_ids(&sh, &format!("heads({selected})"))?;
                if heads.len() >= 2 {
                    cmd!(sh, "jj new {heads...} -m megamerge").run()?;
                    cmd!(sh, "jj new").run()?;
                }
            }
        }
        Ok(())
    }
}

fn validate_revision(rev: &str) -> Result<()> {
    if rev.is_empty() {
        bail!("--revision must not be empty");
    }
    Ok(())
}

pub(crate) fn rebase_into_stack_args<'a>(rev: &'a str, megamerge: &'a str) -> Vec<&'a str> {
    vec![
        "rebase",
        "-r",
        rev,
        "--insert-after",
        "trunk()",
        "--insert-before",
        megamerge,
    ]
}
