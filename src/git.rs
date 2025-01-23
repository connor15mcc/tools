use std::process::Command;

use anyhow::{Context, Result};
use chrono::naive::NaiveDate;
use futures::TryStreamExt;
use git2::{build::CheckoutBuilder, IntoCString, Repository};
use tokio::pin;

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
