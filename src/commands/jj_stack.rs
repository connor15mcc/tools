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

        // Detect the megamerge: the merge commit closest to the tip among
        // descendants of trunk. Try `closest_merge(@)` first (handles the case
        // where the user is working inside the stack); fall back to
        // `heads(merges() & (trunk()..))` for the topmost merge in the stack
        // when @ is off to the side.
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

fn find_megamerge(sh: &Shell) -> Result<Option<String>> {
    if let Some(mm) = list_change_ids(sh, "closest_merge(@)")?.into_iter().next() {
        return Ok(Some(mm));
    }
    Ok(list_change_ids(sh, "heads(merges() & (trunk()..))")?
        .into_iter()
        .next())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_revision_rejects_empty() {
        let err = validate_revision("").unwrap_err();
        assert!(err.to_string().contains("must not be empty"), "got: {err}");
    }

    #[test]
    fn validate_revision_accepts_nonempty() {
        validate_revision("abc").unwrap();
        validate_revision("description(\"foo\")").unwrap();
    }

    #[test]
    fn parse_change_ids_empty() {
        assert!(parse_change_ids("").is_empty());
        assert!(parse_change_ids("\n\n  \n").is_empty());
    }

    #[test]
    fn parse_change_ids_trims_and_drops_empty() {
        let s = "abc\n  def  \n\nghi\n";
        assert_eq!(parse_change_ids(s), vec!["abc", "def", "ghi"]);
    }

    #[test]
    fn parse_change_ids_preserves_order() {
        let s = "z\na\nm\n";
        assert_eq!(parse_change_ids(s), vec!["z", "a", "m"]);
    }

    #[test]
    fn rebase_into_stack_args_shape() {
        assert_eq!(
            rebase_into_stack_args("xyz", "mmid"),
            vec![
                "rebase",
                "-r",
                "xyz",
                "--insert-after",
                "trunk()",
                "--insert-before",
                "mmid",
            ]
        );
    }

    #[test]
    fn rebase_onto_trunk_args_shape() {
        assert_eq!(
            rebase_onto_trunk_args("xyz"),
            vec!["rebase", "-r", "xyz", "--insert-after", "trunk()"]
        );
    }

    #[test]
    fn new_megamerge_args_single_parent() {
        let parents = vec!["p1".to_string()];
        assert_eq!(
            new_megamerge_args(&parents).unwrap(),
            vec!["new", "--no-edit", "-m", "megamerge", "p1"]
        );
    }

    #[test]
    fn new_megamerge_args_multi_parent() {
        let parents = vec!["p1".to_string(), "p2".to_string(), "p3".to_string()];
        assert_eq!(
            new_megamerge_args(&parents).unwrap(),
            vec!["new", "--no-edit", "-m", "megamerge", "p1", "p2", "p3"]
        );
    }

    #[test]
    fn new_megamerge_args_empty_parents_errors() {
        let err = new_megamerge_args(&[]).unwrap_err();
        assert!(err.to_string().contains("zero parents"), "got: {err}");
    }
}
