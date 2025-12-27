use anyhow::Result;
use clap::{Arg, ArgAction, Command as ClapCommand};

mod command;
mod commands;

use command::COMMANDS;

fn main() -> Result<()> {
    // Detect how we were invoked (main binary vs symlink)
    let argv0 = std::env::args()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "tools".to_string());

    let is_main_binary = argv0 == "tools";

    let mut app = ClapCommand::new("tools")
        .version(env!("CARGO_PKG_VERSION"))
        .about("personal tools binary manager")
        .multicall(!is_main_binary) // Only use multicall mode for symlinks
        .arg_required_else_help(true);

    // Add all commands as subcommands, adding verbose flag to each
    // Note: We add the verbose flag to each subcommand individually because
    // clap's multicall mode doesn't support global arguments. This ensures
    // all subcommands have consistent -v/--verbose support for logging.
    for cmd in COMMANDS.iter() {
        let subcommand = cmd.command().arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Increase logging verbosity (-v, -vv, -vvv)")
                .action(ArgAction::Count)
                .global(false),
        );
        app = app.subcommand(subcommand);
    }

    let matches = app.get_matches();
    let (subcmd_name, sub_matches) = matches.subcommand().expect("clap should ensure subcommand");

    // Initialize logging with verbosity from the subcommand matches
    let verbosity = sub_matches.get_count("verbose") as usize;
    stderrlog::new()
        .verbosity(verbosity)
        .module(module_path!())
        .init()
        .unwrap();

    // Find and execute the matching command
    for cmd in COMMANDS.iter() {
        let command = cmd.command();
        if command.get_name() == subcmd_name {
            return cmd.execute_from_matches(sub_matches);
        }
    }

    unreachable!("clap should only match registered subcommands")
}
