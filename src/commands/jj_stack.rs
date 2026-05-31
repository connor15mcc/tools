use anyhow::{bail, Result};
use clap::Parser;
use xshell::{cmd, Shell};

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

        let mm = find_megamerge(&sh)?;

        match mm {
            Some(mm) => {
                let args = rebase_into_stack_args(&rev, &mm);
                sh.cmd("jj").args(&args).run()?;
            }
            None => {
                let args = rebase_onto_trunk_args(&rev);
                sh.cmd("jj").args(&args).run()?;
                let parents = list_change_ids(&sh, "heads(trunk()..)")?;
                let args = new_megamerge_args(&parents)?;
                sh.cmd("jj").args(&args).run()?;
            }
        }
        Ok(())
    }
}

const MEGAMERGE_REVSET: &str = "heads(::@ & merges() & (trunk()..))";

fn find_megamerge(sh: &Shell) -> Result<Option<String>> {
    Ok(list_change_ids(sh, MEGAMERGE_REVSET)?.into_iter().next())
}

fn validate_revision(rev: &str) -> Result<()> {
    if rev.is_empty() {
        bail!("--revision must not be empty");
    }
    Ok(())
}

fn list_change_ids(sh: &Shell, revset: &str) -> Result<Vec<String>> {
    let template = r#"change_id ++ "\n""#;
    let out = cmd!(sh, "jj log -r {revset} -T {template} --no-graph").read()?;
    Ok(parse_change_ids(&out))
}

pub(crate) fn parse_change_ids(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
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

pub(crate) fn rebase_onto_trunk_args(rev: &str) -> Vec<&str> {
    vec!["rebase", "-r", rev, "--insert-after", "trunk()"]
}

pub(crate) fn new_megamerge_args(parents: &[String]) -> Result<Vec<String>> {
    if parents.is_empty() {
        bail!("cannot create megamerge with zero parents");
    }
    let mut a: Vec<String> = vec![
        "new".into(),
        "--no-edit".into(),
        "-m".into(),
        "megamerge".into(),
    ];
    a.extend(parents.iter().cloned());
    Ok(a)
}
