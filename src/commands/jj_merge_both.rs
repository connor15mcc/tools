use crate::command::CommandRunner;
use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use xshell::{cmd, Shell};

/// Resolve merge conflicts by accepting both sides (union merge).
///
/// Performs a 3-way merge where conflicting hunks are resolved by
/// concatenating both sides, while non-conflicting regions are merged
/// normally. Intended for use as a jj merge-tool.
///
/// Configure in jj config.toml:
///
///     [merge-tools.":both"]
///     program = "jj-merge-both"
///     merge-args = ["$base", "$left", "$right", "$output"]
///
/// Usage: jj resolve --tool :both
#[derive(Parser)]
#[command(
    name = "jj-merge-both",
    about = "jj merge-tool that accepts both sides"
)]
pub struct JjMergeBoth {
    /// Path to the base (common ancestor) file
    base: PathBuf,

    /// Path to the left side of the conflict
    left: PathBuf,

    /// Path to the right side of the conflict
    right: PathBuf,

    /// Path to write the merged output
    output: PathBuf,
}

impl CommandRunner for JjMergeBoth {
    fn run(self) -> Result<()> {
        let sh = Shell::new()?;
        let base = &self.base;
        let left = &self.left;
        let right = &self.right;

        let merged = cmd!(sh, "git merge-file --union -p {left} {base} {right}")
            .output()
            .context("failed to run git merge-file")?;

        std::fs::write(&self.output, &merged.stdout)
            .with_context(|| format!("failed to write output to {}", self.output.display()))?;

        Ok(())
    }
}
