use anyhow::Result;
use clap::Command as ClapCommand;

mod command;
mod commands;

use command::COMMANDS;

fn main() -> Result<()> {
    let mut app = ClapCommand::new("tools")
        .version(env!("CARGO_PKG_VERSION"))
        .about("personal tools binary manager")
        .multicall(true)
        .arg_required_else_help(true);

    // Add all commands as subcommands
    for cmd in COMMANDS.iter() {
        app = app.subcommand(cmd.command());
    }

    let matches = app.get_matches();
    let (subcmd_name, sub_matches) = matches.subcommand().expect("clap should ensure subcommand");

    // Find and execute the matching command
    for cmd in COMMANDS.iter() {
        let command = cmd.command();
        if command.get_name() == subcmd_name {
            return cmd.execute_from_matches(sub_matches);
        }
    }

    unreachable!("clap should only match registered subcommands")
}
