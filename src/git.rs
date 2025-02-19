use crate::PrInfo;
use anyhow::Result;
use git2::{build::CheckoutBuilder, Repository};
use rayon::prelude::*;
use std::io::BufRead;
use std::io::Read;
use std::io::{self, Write};
use std::path::PathBuf;
use std::{io::BufReader, process::Command};
use xshell::{cmd, Shell};

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

pub fn poor_mans_refactorator(
    id: &str,
    pr_info: Option<&PrInfo>,
    dry_run: bool,
    cmd: &str,
    repos: BufReader<impl Read>,
    dir: &PathBuf,
) -> Result<()> {
    let repos = repos.lines().collect::<Result<Vec<_>, _>>()?;

    repos
        .par_iter()
        .map(|repo| clone_and_do_work(id, pr_info, dry_run, cmd, repo, dir))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(())
}

fn clone_and_do_work(
    id: &str,
    pr_info: Option<&PrInfo>,
    dry_run: bool,
    cmd: &str,
    repo: &str,
    dir: &PathBuf,
) -> Result<()> {
    let sh = Shell::new()?;
    if !sh.path_exists(dir) {
        sh.create_dir(dir)?;
    }
    sh.change_dir(dir);

    // TODO: figure out tracing, tmp path (should be logged in some way)

    let context = Context { sh: &sh };
    let Some((user, repo)) = repo
        .strip_prefix("github.com/")
        .unwrap_or(&repo)
        .rsplit_once('/')
    else {
        anyhow::bail!("invalid repo: `{repo}` (expected github.com/user/repo)")
    };
    context.clone(user, repo)?;
    {
        let _guard = sh.push_dir(repo);

        // TODO: there's surely a better way to do this...
        let cmd_out = cmd!(sh, "sh -c").arg(cmd).quiet().read();
        if let Err(e) = cmd_out {
            println!("encountered {e}, skipping");
            return Ok(());
        }
        let cmd_out = cmd_out?;

        context.commit(&format!("`sh -c {}`", cmd), Some(id))?;
        let diff_out = match dry_run {
            true => context.diff(id)?,
            false => context.create_pr(Some(id), pr_info)?,
        };

        // minimize time with the lock, but lock to avoid clobbering between threads
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{repo}")?;
        writeln!(stdout, "{cmd_out}")?;
        writeln!(stdout, "{diff_out}")?;
    }
    Ok(())
}

struct Context<'a> {
    sh: &'a Shell,
}

impl<'a> Context<'a> {
    fn clone(&self, user: &str, repo: &str) -> Result<()> {
        let dotgit_exists = self
            .sh
            .path_exists(self.sh.current_dir().join(repo).join(".git"));
        if !dotgit_exists {
            cmd!(
                self.sh,
                "git clone git@github.com:{user}/{repo} --filter=blob:limit=100k"
            )
            .quiet()
            .ignore_stdout()
            .ignore_stderr()
            .run()?;
        }
        let _guard = self.sh.push_dir(repo);

        cmd!(self.sh, "git pull --force")
            .quiet()
            .ignore_stdout()
            .ignore_stderr()
            .run()?;

        let branch = cmd!(self.sh, "git branch --show-current")
            .quiet()
            .ignore_stderr()
            .read()?;
        if !matches!(branch.as_str(), "main" | "master") {
            anyhow::bail!("HEAD points to unexpected branch `{}`", branch);
        }

        Ok(())
    }

    fn commit(&self, message: &str, branch: Option<&str>) -> Result<()> {
        cmd!(self.sh, "git add --all")
            .quiet()
            .ignore_stdout()
            .run()?;
        cmd!(self.sh, "git --no-pager diff --cached --color=always")
            .quiet()
            .ignore_stdout()
            .run()?;
        match branch {
            Some(branch) => {
                cmd!(self.sh, "git switch -C {branch}")
                    .quiet()
                    .ignore_stdout()
                    .ignore_stderr()
                    .run()?;
                cmd!(self.sh, "git commit -m {message} --no-verify")
                    .quiet()
                    .ignore_stdout()
                    .run()
                    .unwrap_or(());
                cmd!(self.sh, "git switch -")
                    .quiet()
                    .ignore_stdout()
                    .ignore_stderr()
                    .run()?;
            }
            None => cmd!(self.sh, "git commit -m {message} --no-verify")
                .quiet()
                .ignore_stdout()
                .run()
                .unwrap_or(()),
        }
        Ok(())
    }

    // TODO: support title and body
    fn create_pr(&self, branch: Option<&str>, pr_info: Option<&PrInfo>) -> Result<String> {
        if let Some(branch) = branch {
            cmd!(self.sh, "git switch {branch}")
                .quiet()
                .ignore_stdout()
                .ignore_stderr()
                .run()?;
        }
        cmd!(self.sh, "git push --force-with-lease")
            .quiet()
            .ignore_stdout()
            .ignore_stderr()
            .run()?;

        let pr_url = match cmd!(self.sh, "gh pr view --json url --jq '.url'")
            .quiet()
            .ignore_stderr()
            .read()
        {
            Ok(url) => url,
            Err(_) => {
                if let Some(PrInfo { title, body }) = pr_info {
                    cmd!(self.sh, "gh pr create --title {title} --body {body}")
                        .quiet()
                        .read()?
                } else {
                    cmd!(self.sh, "gh pr create --fill-verbose")
                        .quiet()
                        .read()?
                }
            }
        };

        if branch.is_some() {
            cmd!(self.sh, "git switch -")
                .quiet()
                .ignore_stdout()
                .ignore_stderr()
                .run()?;
        }
        Ok(pr_url)
    }

    fn diff(&self, branch: &str) -> Result<String> {
        Ok(cmd!(self.sh, "git diff HEAD...{branch} --numstat")
            .quiet()
            .ignore_stderr()
            .read()?)
    }
}
