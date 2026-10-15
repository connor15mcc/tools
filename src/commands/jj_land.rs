use anyhow::{bail, Context, Result};
use clap::Parser;
use xshell::{cmd, Shell};

use super::jj::{find_active_megamerge, list_change_ids};
use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "jj-land",
    about = "Land a staged branch and advance the local trunk bookmark"
)]
pub struct JjLand {
    /// A direct parent of the active megamerge.
    #[arg(short = 'r', long = "revision")]
    revision: String,

    /// Local trunk bookmark to advance (inferred when unambiguous).
    #[arg(long)]
    bookmark: Option<String>,
}

impl CommandRunner for JjLand {
    fn run(self) -> Result<()> {
        let sh = Shell::new()?;
        let rev = self.revision.trim();
        if rev.is_empty() {
            bail!("--revision must not be empty");
        }
        let matches = list_change_ids(&sh, rev)?;
        if matches.len() != 1 {
            bail!(
                "revision must resolve to exactly one commit (matched {})",
                matches.len()
            );
        }
        let target = &matches[0];
        let merge = find_active_megamerge(&sh)?.context(
            "no active megamerge; use `jj tug --to r` if you only want to advance trunk",
        )?;
        let parents = list_change_ids(&sh, &format!("{merge}-"))?;
        if !parents.contains(target) {
            bail!("revision is not a direct parent of the active megamerge");
        }
        let trunk = list_change_ids(&sh, "trunk()")?;
        if trunk.len() != 1 {
            bail!("trunk() must resolve to exactly one commit");
        }
        let trunk = &trunk[0];
        if list_change_ids(&sh, &format!("{target} & {trunk}::"))?.is_empty() {
            bail!("revision must descend from trunk()");
        }
        // Removing merge machinery must not discard edits or conflict resolutions.
        if !list_change_ids(&sh, &format!("{merge} ~ empty()"))?.is_empty() {
            bail!("active megamerge has changes; move them to a child before landing");
        }
        let bookmark = trunk_bookmark(&sh, self.bookmark.as_deref())?;
        let bookmark_revset = format!("bookmarks(exact:{})", serde_json::to_string(&bookmark)?);
        let bookmark_targets = list_change_ids(&sh, &bookmark_revset)?;
        if bookmark_targets.len() != 1
            || list_change_ids(&sh, &format!("{bookmark_revset} & ::{target}"))?.len() != 1
        {
            bail!(
                "local bookmark {bookmark:?} must resolve to one ancestor of the landed revision"
            );
        }

        let remaining: Vec<_> = parents.iter().filter(|id| *id != target).cloned().collect();
        let remaining = remaining.join(" | ");
        // Rebase whole branches, not just their tips, while excluding history
        // already included in the landed branch.
        let roots = list_change_ids(
            &sh,
            &format!("roots(({trunk}..({remaining})) ~ ::{target})"),
        )?;
        if !roots.is_empty() {
            let sources = roots.join(" | ");
            if !list_change_ids(&sh, &format!("({sources}):: & immutable()"))?.is_empty() {
                bail!("remaining staged branches have immutable descendants; cannot land safely");
            }
            cmd!(sh, "jj rebase -s {sources} -d {target}").run()?;
        }
        let heads = list_change_ids(&sh, &format!("heads(({remaining}) ~ ::{target})"))?;
        let destinations = if heads.is_empty() {
            vec![target.clone()]
        } else {
            heads.clone()
        };
        // Keep descendants attached to the merge while changing its parents.
        // -r would fill its old position by giving children its former parents.
        let mut args = vec!["rebase".to_owned(), "-s".to_owned(), merge.clone()];
        for destination in destinations {
            args.extend(["-d".to_owned(), destination]);
        }
        sh.cmd("jj").args(args).run()?;
        if heads.len() < 2 {
            cmd!(sh, "jj abandon {merge}").run()?;
        }
        // Advance last, so a failed graph operation never advances the bookmark.
        let bookmark = format!("exact:{bookmark}");
        cmd!(sh, "jj bookmark move {bookmark} --to {target}").run()?;
        Ok(())
    }
}

fn trunk_bookmark(sh: &Shell, explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_owned());
    }
    let template = r#"local_bookmarks.map(|b| b.name()).join("\n") ++ "\n""#;
    let local = cmd!(sh, "jj log -r trunk() --no-graph -T {template}").read()?;
    let mut candidates: Vec<String> = local
        .lines()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    // trunk() may still point at main@origin after local main has advanced.
    let template = r#"remote_bookmarks.map(|b| b.name()).join("\n") ++ "\n""#;
    let remote = cmd!(sh, "jj log -r trunk() --no-graph -T {template}").read()?;
    for name in remote.lines().filter(|s| !s.is_empty()) {
        let revset = format!("bookmarks(exact:{})", serde_json::to_string(name)?);
        if !list_change_ids(sh, &revset)?.is_empty() {
            candidates.push(name.to_owned());
        }
    }
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [name] => Ok(name.clone()),
        _ => bail!("cannot infer a single local trunk bookmark; pass --bookmark <name>"),
    }
}
