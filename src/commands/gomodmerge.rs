use crate::command::CommandRunner;
use anyhow::Result;
use clap::Parser;
use git2::{build::CheckoutBuilder, Repository};
use std::process::Command;

#[derive(Parser)]
#[command(name = "gomodmerge", about = "Tidy merged go.mod and go.sum files")]
pub struct GoModMerge;

impl CommandRunner for GoModMerge {
    fn run(self) -> Result<()> {
        let cwd = std::env::current_dir()?;
        let repo = Repository::open(&cwd)?;
        repo.checkout_index(
            None,
            Some(
                CheckoutBuilder::default()
                    .use_ours(true)
                    .path("go.mod")
                    .path("go.sum")
                    .force(),
            ),
        )?;

        Command::new("go")
            .args(["mod", "tidy"])
            .current_dir(&cwd)
            .output()?;

        Ok(())
    }
}
