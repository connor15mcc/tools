use clap::Parser;
use xshell::cmd;

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "jj-stack",
    about = "Stack changes under megamerge, creating one if needed"
)]
pub struct JjStack {
    #[arg(allow_hyphen_values = true, last = true)]
    revision: Vec<String>,
}

impl CommandRunner for JjStack {
    fn run(self) -> anyhow::Result<()> {
        let sh = xshell::Shell::new()?;

        let has_megamerge = cmd!(sh, "jj log -r 'closest_merge(@)' -T none -q")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !has_megamerge {
            cmd!(sh, "jj new --merge -d trunk() -m megamerge --ignore-working-copy").run()?;
        }

        let revision_arg = self.revision.join(" ");
        if revision_arg.is_empty() {
            cmd!(sh, "jj rebase --after trunk() --before closest_merge(@) --ignore-working-copy").run()?;
        } else {
            cmd!(
                sh,
                "jj rebase --after trunk() --before closest_merge(@) --revision {revision_arg} --ignore-working-copy"
            )
            .run()?;
        }

        Ok(())
    }
}
