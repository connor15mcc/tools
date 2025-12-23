use crate::command::CommandRunner;
use anyhow::{Context, Result};
use clap::Parser;
use std::fs::File;
use std::io::Write;
use std::process::Command;
use xshell::{cmd, Shell};

#[derive(Parser)]
#[command(name = "gh-review", about = "Review GitHub PRs from a search query")]
pub struct Review {
    /// GitHub search query string (e.g., "is:pr is:open author:username")
    #[arg(required = true)]
    query: Vec<String>,

    /// Limit the number of PRs to review
    #[arg(short, long, default_value = "30")]
    limit: u32,
}

impl CommandRunner for Review {
    fn run(self) -> Result<()> {
        let sh = Shell::new()?;
        let query = self.query.join(" ");

        // Search for PRs using gh search prs
        // Pass each query term as a separate argument for proper parsing
        let mut search_cmd = cmd!(sh, "gh search prs");
        for term in &self.query {
            search_cmd = search_cmd.arg(term);
        }
        let search_result = search_cmd
            .arg("--limit")
            .arg(self.limit.to_string())
            .arg("--json")
            .arg("number,repository,title,url")
            .read()
            .context("Failed to search PRs. Make sure 'gh' is installed and authenticated")?;

        // Parse the JSON result
        let prs: Vec<PrInfo> =
            serde_json::from_str(&search_result).context("Failed to parse PR search results")?;

        if prs.is_empty() {
            println!("No PRs found for query: '{}'", query);
            return Ok(());
        }

        // Create temp files for each PR diff
        let temp_dir = std::env::temp_dir();
        let mut temp_files = Vec::new();

        for pr in &prs {
            let temp_file = temp_dir.join(format!(
                "pr-{}-{}.diff",
                pr.repository.name_with_owner.replace('/', "-"),
                pr.number
            ));

            // Fetch the diff
            let diff = cmd!(sh, "gh pr diff")
                .arg(pr.number.to_string())
                .arg("--repo")
                .arg(&pr.repository.name_with_owner)
                .arg("--color")
                .arg("always")
                .read()
                .with_context(|| format!("Failed to fetch diff for PR #{}", pr.number))?;

            // Write header and diff to temp file
            let mut file = File::create(&temp_file)
                .with_context(|| format!("Failed to create temp file: {}", temp_file.display()))?;

            let header = format!(
                "================================================================================\n\
                 Repository: {}\n\
                 PR #{}: {}\n\
                 URL: {}\n\
                 ================================================================================\n\n",
                pr.repository.name_with_owner,
                pr.number,
                pr.title,
                pr.url
            );

            file.write_all(header.as_bytes())?;
            file.write_all(diff.as_bytes())?;

            temp_files.push(temp_file);
        }

        // Open all temp files in less
        // %i = current file index, %m = total files
        Command::new("less")
            .arg("-R")
            .arg("--prompt=PR Review (%i/%m) [\\:n=next | \\:p=prev | q=quit]")
            .args(&temp_files)
            .status()
            .context("Failed to run 'less'")?;

        // Clean up temp files
        for temp_file in temp_files {
            let _ = std::fs::remove_file(temp_file);
        }

        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct PrInfo {
    number: u32,
    repository: Repository,
    title: String,
    url: String,
}

#[derive(serde::Deserialize)]
struct Repository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}
