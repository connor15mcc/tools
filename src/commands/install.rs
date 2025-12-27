use anyhow::Result;
use clap::Parser;

use crate::command::{CommandRunner, COMMANDS};

#[derive(Parser)]
#[command(name = "install", about = "Install symlinks for all commands")]
pub struct Install;

impl CommandRunner for Install {
    fn run(self) -> Result<()> {
        install_symlinks()
    }
}

fn install_symlinks() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not determine binary directory"))?;
    let exe_name = exe_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Could not determine binary name"))?;

    for cmd in COMMANDS.iter() {
        let command = cmd.command();
        let name = command.get_name();

        // Skip creating a symlink for "install" itself
        if name == "install" {
            continue;
        }

        let link_path = exe_dir.join(name);

        // Remove existing symlink/file if it exists
        if link_path.exists() || link_path.symlink_metadata().is_ok() {
            std::fs::remove_file(&link_path)?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(exe_name, &link_path)?;

        #[cfg(windows)]
        std::os::windows::fs::symlink_file(exe_name, &link_path)?;

        eprintln!("created symlink: {}", link_path.display());
    }

    Ok(())
}
