use linkme::distributed_slice;

pub trait Command: Send + Sync {
    fn command(&self) -> clap::Command;
    fn execute_from_matches(&self, matches: &clap::ArgMatches) -> anyhow::Result<()>;
}

pub trait CommandRunner {
    fn run(self) -> anyhow::Result<()>;
}

#[distributed_slice]
pub static COMMANDS: [&'static dyn Command] = [..];
