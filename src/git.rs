use std::{io::BufReader, process::Command};
use anyhow::{Context as _, Result};
use chrono::naive::NaiveDate;
use std::io::BufRead;
use std::io::Read;
use futures::TryStreamExt;
use git2::{build::CheckoutBuilder, Repository};
use tokio::pin;
use xshell::{cmd, Shell};

use crate::PrInfo;

#[derive(Debug)]
struct PullRequest {
    repo: String,
    title: String,
    url: String,
    updated: NaiveDate,

    additions: u64,
    deletions: u64,

    comments_by_author: Vec<String>,
}

#[tokio::main]
pub async fn list_recent_reviews() -> Result<()> {
    let crab = octocrab::instance();
    let stream = crab
        .search()
        .issues_and_pull_requests("author:connor15mcc type:pr state:closed")
        .order("asc")
        .send()
        .await?
        .into_stream(&crab);
    pin!(stream);

    let mut prs = vec![];
    while let Some(issue) = stream.try_next().await? {
        let mut parts = issue
            .url
            .path_segments()
            .context("url should have segments")?
            .skip(1); // first segment is `/repos/`

        let (owner, repo_name) = (
            parts.next().context("should be owned")?,
            parts.next().context("should be associated w/ a repo")?,
        );
        let pull_request = crab
            .pulls(owner, repo_name)
            .get(issue.number)
            .await
            .context("query specifies `type:pr`")?;

        let mut comments_by_author = vec![];

        let comments = crab
            .pulls(owner, repo_name)
            .list_comments(Some(issue.number))
            .send()
            .await?
            .into_stream(&crab);
        pin!(comments);

        while let Some(comment) = comments.try_next().await? {
            let author = comment
                .user
                .expect("how can a comment not have an author..");
            comments_by_author.push(author.login)
        }

        let reviews = crab
            .pulls(owner, repo_name)
            .list_reviews(issue.number)
            .send()
            .await?
            .into_stream(&crab);
        pin!(reviews);

        while let Some(review) = reviews.try_next().await? {
            let author = review.user.expect("how can a review not have an author..");
            comments_by_author.push(author.login)
        }

        prs.push(PullRequest {
            repo: format!("github.com/{}/{}", owner, repo_name),
            title: pull_request.title.unwrap(),
            url: issue.repository_url.to_string(),
            updated: pull_request
                .updated_at
                .expect("PR must have been updated if existss")
                .date_naive(),

            additions: pull_request.additions.unwrap_or(0),
            deletions: pull_request.deletions.unwrap_or(0),

            comments_by_author,
        });
    }
    println!("PRs: {:?}", prs);

    Ok(())
}

pub fn tidy_merged_go_mod() -> Result<()> {
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

pub fn poor_mans_refactorator(id: &str, pr_info: Option<&PrInfo>, cmd: &str, repos: BufReader<impl Read>) -> Result<()> {
    let dir = &format!(".run-{}", id);
    let sh = Shell::new()?;
    if !sh.path_exists(dir) {
        sh.create_dir(dir)?;
    }
    sh.change_dir(dir);

    // TODO: figure out tracing, tmp path (should be logged in some way)

    for repo in repos.lines() {
        let repo = repo?;

        let context = Context { sh: &sh };
        let Some((user, repo)) = repo.strip_prefix("github.com:").unwrap_or(&repo).rsplit_once('/') else {
            anyhow::bail!("invalid repo: `{repo}` (expected github.com/user/repo)")
        };
        context.clone(user, repo)?;
        let pr_url = {
            let _guard = sh.push_dir(repo);
            let _out = cmd!(sh, "sh -c").arg(cmd).read()?;
            context.commit(&format!("`sh -c {}`", cmd), Some(id))?;
            context.create_pr(Some(id), pr_info)?
        };
        println!("{}", pr_url);
    }
    Ok(())
}

struct Context<'a> {
    sh: &'a Shell,
}

impl<'a> Context<'a> {
    fn clone(&self, user: &str, repo: &str) -> Result<()> {
        let dotgit_exists = self.sh.path_exists(self.sh.current_dir().join(repo).join(".git"));
        if !dotgit_exists {
            cmd!(self.sh, "git clone git@github.com:{user}/{repo}")
                .quiet().ignore_stdout().ignore_stderr().run()?;
        }

        Ok(())
    }

    fn commit(&self, message: &str, branch: Option<&str>) -> Result<()> {
        cmd!(self.sh, "git add --all").quiet().ignore_stdout().run()?;
        cmd!(self.sh, "git --no-pager diff --cached --color=always")
            .quiet().ignore_stdout().run()?;
        match branch {
            Some(branch) => {
                cmd!(self.sh, "git switch -c {branch}").quiet().ignore_stdout().ignore_stderr().run()?;
                cmd!(self.sh, "git commit -m {message}").quiet().ignore_stdout().run()?;
                cmd!(self.sh, "git switch -").quiet().ignore_stdout().ignore_stderr().run()?;
            }
            None => cmd!(self.sh, "git commit -m {message}").quiet().ignore_stdout().run()?,
        }
        Ok(())
    }

    // TODO: support title and body
    fn create_pr(&self, branch: Option<&str>, pr_info: Option<&PrInfo>) -> Result<String> {
        if let Some(branch) = branch {
            cmd!(self.sh, "git switch {branch}").quiet().ignore_stdout().ignore_stderr().run()?;
        }
        cmd!(self.sh, "git push --force-with-lease").quiet().ignore_stdout().ignore_stderr().run()?;

        let pr_url = match cmd!(self.sh, "gh pr view --json url --jq '.url'").quiet().ignore_stderr().read() {
            Ok(url) => url,
            Err(_) => {
                if let Some(PrInfo{title, body}) = pr_info {
                    cmd!(self.sh, "gh pr create --title {title} --body {body}").quiet().read()?
                } else {
                    cmd!(self.sh, "gh pr create --fill-verbose").quiet().read()?
                }
            },
        };

        if branch.is_some() {
            cmd!(self.sh, "git switch -").quiet().ignore_stdout().ignore_stderr().run()?;
        }
        Ok(pr_url)
    }
}
