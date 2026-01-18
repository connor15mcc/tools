use std::fs::create_dir_all;

use anyhow::Context;
use clap::Parser;
use petname::{Generator, Petnames};
use xshell::{cmd, Shell};

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "jj-iso",
    about = "Create a random JJ workspace and launch a specified binary"
)]
pub struct JjIso {
    #[arg(help = "A command to launch (e.g., build, vim, opencode, ...)")]
    cmd: String,
}

impl CommandRunner for JjIso {
    fn run(self) -> anyhow::Result<()> {
        // Check if in Zellij session
        if std::env::var("ZELLIJ").is_err() {
            anyhow::bail!("Error: This tool must be run from within a Zellij session. Start Zellij first with `zellij`.");
        }

        let sh = Shell::new()?;

        // Check if already in a workspace
        let current_ws_root = cmd!(sh, "jj workspace root").ignore_status().read()?;
        let repo_root = cmd!(sh, "jj root").read()?;
        if !current_ws_root.trim().is_empty() && current_ws_root.trim() != repo_root.trim() {
            anyhow::bail!(
                "Error: Already in a JJ workspace ({}). Exit to main repo first.",
                current_ws_root.trim()
            );
        }

        // name is a petname, path is `tmpdir / repo_base / petname`
        let repo_name = std::path::Path::new(&repo_root.trim())
            .file_name()
            .and_then(|s| s.to_str())
            .context("unknown base repo name")?
            .to_string();
        let ws_name = Petnames::default()
            .generate_one(2, "-")
            .context("no possible petname")?;
        let tmpdir = std::env::temp_dir().join("jj");
        let ws_path = tmpdir.join(&repo_name).join(&ws_name);

        // Add JJ workspace
        create_dir_all(ws_path.parent().expect("is already nested"))
            .context("failed to create workspace dir")?;
        let ws_path_str = ws_path.to_string_lossy().to_string();
        cmd!(sh, "jj workspace add {ws_path_str}").run()?;

        // Define script
        let script = format!(
            "cd {}; trap 'jj workspace forget {}; rm -rf {}' TERM INT EXIT; {}",
            ws_path_str, ws_name, ws_path_str, self.cmd
        );

        // Create new Zellij tab with command pane, replacing the default pane.
        // Zellij tabs start with a default pane, so we create the tab, add the command pane,
        // focus back to the default pane, and close it to leave only the command pane.
        cmd!(sh, "zellij action new-tab --name {ws_name}").run()?;
        sh.cmd("zellij")
            .args(&[
                "action",
                "new-pane",
                "--name",
                &ws_name,
                "--cwd",
                &ws_path_str,
                "--",
                "sh",
                "-c",
                &script,
            ])
            .run()?;
        cmd!(sh, "zellij action focus-previous-pane").run()?;
        cmd!(sh, "zellij action close-pane").run()?;

        Ok(())
    }
}
