use anyhow::{bail, Result};
use xshell::{cmd, Shell};

// jj normalizes descriptions with a trailing newline. Match only the reserved
// description, allowing that newline, rather than accepting a first-line prefix.
const ACTIVE_MEGAMERGE: &str =
    r#"heads(::@ & merges() & mutable() & (trunk()..) & description(regex:"^megamerge\n?$"))"#;

pub(super) fn find_active_megamerge(sh: &Shell) -> Result<Option<String>> {
    let matches = list_change_ids(sh, ACTIVE_MEGAMERGE)?;
    match matches.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(id.clone())),
        _ => bail!(
            "multiple active megamerges; select a working copy with a single active megamerge"
        ),
    }
}

pub(super) fn list_change_ids(sh: &Shell, revset: &str) -> Result<Vec<String>> {
    let template = r#"change_id ++ "\n""#;
    let out = cmd!(sh, "jj log -r {revset} -T {template} --no-graph").read()?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect())
}
